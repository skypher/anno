//! Main simulation dispatcher.
//!
//! Ported from FUN_00489670 (core simulation orchestrator).
//! Processes delta time in chunks of max 200ms, scaled by game speed multiplier.
//! Dispatches to 12 subsystem update functions on independent timers.

use crate::ai::{AiAction, AiController};
use crate::building::{BuildingDef, BuildingInstance, SourceBuildingCommand};
use crate::carrier;
use crate::civilian;
use crate::combat::{
    self, DiplomacyMatrix, MilitaryUnit, SourceDynamicCombatFigure, SourceKind14CombatFigure,
    SourceKind15CombatFigure,
};
use crate::coverage::CoverageMap;
use crate::data_bridge::{
    SourceCityRecord, SourceCityTable, SourceKind13DispatchState, SourceKind13Location,
    SourceKind13LocationTable, SourceKind13PromotionDefinition, SourceKind4Occupant,
};
use crate::economy;
use crate::entity::{ActionType, CargoRoute, Figure, SourceFigureRecordLayout, SourceWorkerRoute};
use crate::island_map::{IslandMap, SourceControllerCityRectangle};
use crate::ocean_map::OceanMap;
use crate::player::{Player, PlayerState};
use crate::population;
use crate::production;
use crate::source_cell::{
    source_resource_harvest_transition, SourceMapCellState, SourceResourceHarvestTransition,
    SourceTransferFigure,
};
use crate::source_figure_event::SourceFigureEventRegistry;
use crate::source_route::{
    encode_source_route_truncated, source_route_positions, SourceDynamicMapObject,
    SourceDynamicMapObjectTable, SourcePathBlockedCellDecision, SourcePathTargetRect,
    SourceResolvedDynamicTarget, SourceRouteStep, SourceShipRouteWindow,
    SourceShipTargetRouteBranch, SourceTargetDescriptor, SOURCE_ROUTE_TERMINATOR,
    SOURCE_SHIP_ROUTE_CAPACITY, SOURCE_SHIP_ROUTE_RUN_LIMIT,
};
use crate::trade::{self, TradeRoute, TradeShip};
use crate::types::{Good, TICKS_PER_MINUTE};
use crate::warehouse::Warehouse;

/// Auto-save interval in game ticks (~10 minutes of game time).
pub const AUTOSAVE_INTERVAL_MS: u32 = 599_999;
/// Physical entry count of the source `DAT_005b6060` visible-island table.
pub const SOURCE_VISIBLE_ISLAND_SLOTS: usize = 0x32;
/// `FUN_00456d00` enters its no-target branch after this many source ticks.
const SOURCE_KIND4_IDLE_TARGET_DELAY_TICKS: u32 = 20;

/// The generic shared-table route bytes at `+0x124` and their `+0x02`
/// cursor. Dynamic category-one through -three figures own this state even
/// though their compatibility records are stored separately in the Rust
/// simulation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SourceDynamicRouteProgram {
    runtime_slot: u16,
    program: Vec<u8>,
    cursor: usize,
}

/// Timer state for each subsystem.
#[derive(Debug, Clone)]
struct SubsystemTimer {
    accumulator_ms: u32,
    interval_ms: u32,
}

impl SubsystemTimer {
    fn new(interval_ms: u32) -> Self {
        Self {
            accumulator_ms: 0,
            interval_ms,
        }
    }

    /// Advance timer, returns true if a tick should fire.
    fn advance(&mut self, dt_ms: u32) -> bool {
        self.accumulator_ms += dt_ms;
        if self.accumulator_ms >= self.interval_ms {
            self.accumulator_ms -= self.interval_ms;
            true
        } else {
            false
        }
    }
}

/// Static-map mutation emitted when a source map-command root is removed.
///
/// `ruin_id` is the source `Ruinenr` byte from haeuser.cod. `0xff` means
/// `FUN_00463f40` clears the footprint without placing a ruin.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TileClear {
    pub island_id: u8,
    pub tile_x: u16,
    pub tile_y: u16,
    pub width: u8,
    pub height: u8,
    /// Low source-command orientation bits forwarded to the ruin writer.
    pub source_orientation: u8,
    /// Packed root frame selector forwarded to the replacement draw writer.
    pub source_variant: u8,
    /// Packed root map-owner selector forwarded to the replacement map writer.
    pub source_map_owner_slot: u8,
    pub ruin_id: u8,
    /// The terminal handler's source-kind 23..=27 branch selects a shifted
    /// strand ruin table instead of the ordinary ruin table.
    pub ruin_uses_strand_table: bool,
    /// Per-cell shifted-table selectors for a mismatched-footprint rewrite,
    /// indexed in the source writer's right-to-left cell order.
    pub fallback_strand_cells: u64,
    /// MSVC `rand()` outputs consumed synchronously by `FUN_00463f40`.
    /// Renderer replay uses these values without advancing simulation RNG.
    pub source_ruin_draws: Vec<u16>,
}

/// Replacement command emitted by a completed kind-13 BGruppe transition.
/// The simulation has already updated the city and location tables; the game
/// layer drains this event to replay `FUN_00463ef0`/`FUN_004631b0` against its
/// authoritative INSELHAUS command stream and refresh the static map overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceKind13ReplacementCommand {
    pub island_id: u8,
    pub tile_x: u8,
    pub tile_y: u8,
    pub target_group: u8,
    pub command: SourceBuildingCommand,
}

impl TileClear {
    #[inline]
    pub fn fallback_uses_strand_table(&self, source_order_index: usize) -> bool {
        source_order_index < u64::BITS as usize
            && (self.fallback_strand_cells & (1_u64 << source_order_index)) != 0
    }
}

/// Terminal map command emitted by `FUN_0047a650` after a deferred
/// category-6 hit reaches a command root's compiled damage threshold.
/// `FUN_0046a8c0` packages this as descriptor class `0x34` with event kind
/// seven before dispatching the map-root replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceKind6TerminalEvent {
    pub target: SourceTargetDescriptor,
    pub event_kind: u8,
}

/// Terminal control record created by `FUN_00445930` when a category-one
/// through -four figure's source energy reaches zero. `FUN_00443bf0` writes
/// control kind `0x0c`, `FUN_0045e1f0` marks the target's terminal state and
/// reinitializes its current motion slice, and `FUN_00451890` removes the
/// live figure when that slice ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceCombatTerminalEvent {
    pub target: SourceTargetDescriptor,
    pub target_figure_kind: u8,
    pub target_runtime_slot: u16,
    pub target_owner: u8,
    pub attacker_figure_kind: u8,
    pub attacker_runtime_slot: u16,
    pub attacker_owner: Option<u8>,
    pub control_kind: u8,
    pub kill_credit: bool,
}

/// One `0x84d` source figure-purchase record. The source event first credits
/// the current owner and transfers the shared figure handle, then dispatches
/// the same record with its phase cleared to debit the buyer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceFigurePurchaseEvent {
    pub buyer: u8,
    pub figure_handle: u16,
    pub amount: u16,
}

/// Controller `+0x10638` after `FUN_00417690` completes a city arrival.
/// The source addresses this per-city profile as `controller + 0x3e80 +
/// city_slot * 0x298` and writes its `+0x14` source-clock timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceCityManagementProfile {
    pub city_slot: usize,
    pub initialized_at_ticks: u32,
}

/// One raw 32-byte state-seven construction work record at controller offset
/// `+0x1df4`. `FUN_00417aa0` compares its signed priority word at `+0x1c`;
/// the producer owns the remaining source fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceControllerCityConstructionWork {
    bytes: [u8; 32],
}

impl SourceControllerCityConstructionWork {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.bytes
    }

    pub const fn priority(self) -> i16 {
        i16::from_le_bytes([self.bytes[0x1c], self.bytes[0x1d]])
    }
}

/// The `FUN_00417aa0` construction-work queue. The source retains at most
/// 256 records, rejects priorities at most one, and replaces the first
/// lowest-priority entry only when a full queue contains a priority at most
/// three and the incoming priority exceeds three.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SourceControllerCityConstructionQueue {
    entries: Vec<SourceControllerCityConstructionWork>,
}

impl SourceControllerCityConstructionQueue {
    pub const CAPACITY: usize = 0x100;

    pub fn entries(&self) -> &[SourceControllerCityConstructionWork] {
        &self.entries
    }

    pub fn insert(&mut self, work: SourceControllerCityConstructionWork) -> bool {
        if work.priority() <= 1 {
            return false;
        }
        if self.entries.len() < Self::CAPACITY {
            self.entries.push(work);
            return true;
        }
        if work.priority() <= 3 {
            return false;
        }
        let Some((index, priority)) = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.priority())
            .map(|(index, entry)| (index, entry.priority()))
        else {
            return false;
        };
        if priority > 3 {
            return false;
        }
        self.entries[index] = work;
        true
    }
}

/// Controller bytes consumed by the `FUN_0042b4b0` city-management branch.
///
/// The complete controller occupies `0x11e88` bytes in the executable. This
/// preserves the fields that select and pace `FUN_00422150` / `FUN_00422030`:
/// the action stack, live category-1/2/3 roster, active city, and its three
/// scheduler timers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourcePlayerController {
    /// Has `FUN_0040f580` initialized this controller slot?
    pub initialized: bool,
    /// Controller `+0x08`: source-clock tick at the last
    /// `FUN_0040f580` initialization. `FUN_00429070` resets the controller
    /// once the strict 36,000-tick age gate is exceeded.
    pub initialized_at_ticks: u32,
    /// Controller `+0x4`, decremented by `FUN_0042b4b0` and replenished by 50.
    pub action_timer_ms: i32,
    /// Controller `+0x0c`, the city-management timer replenished by 10,000.
    pub city_management_timer_ms: i32,
    /// Controller `+0x14`, retained for the adjacent 6,000-ms management path.
    pub maintenance_timer_ms: i32,
    /// Controller `+0x18`, the desired controlled-figure count.
    pub desired_figure_count: u32,
    /// Controller `+0x4e0`, the quotient derived from the current roster and
    /// desired figure count by `FUN_00423710`.
    pub figure_roster_ratio: u32,
    /// Controller `+0x4dc`, computed by `FUN_00423710` before this branch.
    pub figure_capacity: u32,
    /// PLAYER4 u16 `+0x9c`, the per-player capacity clamp read by
    /// `FUN_00423710`. `None` denotes an unconfigured controller outside a
    /// loaded PLAYER4 record.
    pub figure_capacity_limit: Option<u16>,
    /// Controller `+0x10638`: the city profile installed by a successful
    /// `FUN_00417690` arrival. Its presence enables the ten-second
    /// `FUN_00424bf0` branch.
    pub city_management_profile: Option<SourceCityManagementProfile>,
    /// Controller `+0x1dac`, the category-1/2/3 figure selected by
    /// `FUN_004172e0` for action state four.
    pub action_figure_handle: Option<u16>,
    /// Controller `+0x1db0`, the selected island index consumed by
    /// `FUN_00417690` to locate its managed city.
    pub action_target_island_id: Option<u8>,
    /// Controller `+0x1db4` / `+0x1db8`, the state-three target tile passed
    /// to the `FUN_004084d0` construction command in state four.
    pub action_target_tile: Option<(u8, u8)>,
    /// Controller `+0x1dc0` / `+0x1dc4`, the segment-head candidate selected
    /// by state three before it applies the construction offset.
    pub action_source_candidate_tile: Option<(i32, i32)>,
    /// Controller `+0x1dbc`, the source map-word direction retained with the
    /// selected state-three construction target.
    pub action_target_direction: Option<u8>,
    /// Controller `+0x1da8`, seeded to four by state three and decremented by
    /// `FUN_00417690` when the selected figure must re-approach its candidate.
    pub action_arrival_retries: u8,
    /// Controller `+0x1dc8`, advanced before every state-two island probe and
    /// wrapped through the source's fifty physical island slots.
    pub island_search_cursor: u8,
    /// Controller `+0x1dcc`. `None` represents the raw zero sentinel;
    /// otherwise `FUN_00416370` requires `FUN_0046aff0(island, selector)` to
    /// return `0x80` before the island is eligible.
    pub island_search_requirement: Option<u8>,
    /// Controller `+0x1dd4`, the island selected by `FUN_00416370` and
    /// consumed by the action-two rectangle producer.
    pub island_search_selected_island_id: Option<u8>,
    /// Controller `+0x4e4`, initialized to 1800 and reduced by fifty after a
    /// full unsuccessful island sweep while it remains above the minimum.
    pub island_search_area_threshold: i32,
    /// Controller `+0x4e8`, the state-two minimum area threshold, initialized
    /// to 300 by the requester and compared against candidate area totals.
    pub island_search_minimum_area: i32,
    /// Controller `+0x1dd0`, retaining a nonzero requirement when a complete
    /// sweep clears `+0x1dcc` for the next source action.
    pub island_search_deferred_requirement: Option<u8>,
    /// Controller `+0x28` / `+0x30` retry gate set by a complete exhausted
    /// unconstrained sweep. The supplier re-enters after 600 source ticks.
    pub island_search_retry_at_ticks: Option<u32>,
    /// Controller `+0x510` / `+0x50c` records emitted by the action-two
    /// `FUN_00415e70(..., 7, 5, 5)` rectangle search.
    pub source_city_rectangles: Vec<SourceControllerCityRectangle>,
    /// Controller `+0x1df0/+0x1df4`: state-seven construction work emitted
    /// by `FUN_00417c80` through `FUN_00417aa0`.
    pub source_city_construction_queue: SourceControllerCityConstructionQueue,
    /// Owner byte of the controller's `+0x3e7c` active city, when present.
    pub active_city_owner: Option<u8>,
    /// Physical source-city slot of controller `+0x3e7c`. Source controller
    /// initialization scans cities in physical pool order, so this remains
    /// distinct from the owner byte when a player owns multiple cities.
    pub active_city_slot: Option<usize>,
    /// Whether `FUN_0040fcb0` accepts the selected map city.
    pub selected_city_active: bool,
    /// Source `PLAYER4 + 0x70` bit six, which bypasses `FUN_00424bf0`.
    pub city_management_disabled: bool,
    /// Controller `+0x24` / `+0x34` action stack. The city purchase path only
    /// runs while its top action is greater than eight.
    pub action_stack: Vec<u8>,
    /// Controller `+0x3e78`, replenished to fourteen by the successful
    /// `FUN_00417690` city-arrival branch.
    pub action_budget: i32,
    /// Result of the preceding `FUN_00421d30` priority branch. A true value
    /// means that function issued a city action and therefore suppresses a
    /// purchase on this city-management pass.
    pub purchase_predecessor_issued: bool,
    /// Controller `+0x10734` roster rebuilt by `FUN_0044f000`.
    pub owned_figure_handles: Vec<u16>,
    /// Player byte `+0x7d`: `FUN_0042b4b0` rebuilds the roster when set.
    pub figure_roster_dirty: bool,
}

impl Default for SourcePlayerController {
    fn default() -> Self {
        Self {
            initialized: false,
            initialized_at_ticks: 0,
            action_timer_ms: 0,
            city_management_timer_ms: 0,
            maintenance_timer_ms: 0,
            desired_figure_count: 0,
            figure_roster_ratio: 0,
            figure_capacity: 0,
            figure_capacity_limit: None,
            city_management_profile: None,
            action_figure_handle: None,
            action_target_island_id: None,
            action_target_tile: None,
            action_source_candidate_tile: None,
            action_target_direction: None,
            action_arrival_retries: 0,
            island_search_cursor: 0,
            island_search_requirement: None,
            island_search_selected_island_id: None,
            island_search_area_threshold: 0,
            island_search_minimum_area: 0,
            island_search_deferred_requirement: None,
            island_search_retry_at_ticks: None,
            source_city_rectangles: Vec::new(),
            source_city_construction_queue: SourceControllerCityConstructionQueue::default(),
            active_city_owner: None,
            active_city_slot: None,
            selected_city_active: false,
            city_management_disabled: false,
            action_stack: Vec::new(),
            action_budget: 0,
            purchase_predecessor_issued: false,
            owned_figure_handles: Vec::new(),
            figure_roster_dirty: false,
        }
    }
}

#[derive(Clone, Copy)]
enum SourceSharedFigureEntity {
    MilitaryUnit(usize),
    TradeShip(usize),
    DynamicFigure(usize),
}

/// Live target identity for a terminal source motion slice. The source keeps
/// the figure record alive until its reinitialized slice reaches zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceCombatTerminalSliceTarget {
    TradeShip(usize),
    DynamicFigure(usize),
}

/// `FUN_0045e1f0` initializes a terminal figure's remaining motion state;
/// `FUN_00451890` consumes that state before releasing the live record.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceCombatTerminalSlice {
    pub target: SourceCombatTerminalSliceTarget,
    pub target_figure_kind: u8,
    pub target_runtime_slot: u16,
    pub remaining_distance: f32,
    pub scalar_speed: f32,
    pub velocity_x: f32,
    pub velocity_y: f32,
    pub velocity_z: f32,
}

/// `FUN_00443bf0`'s terminal figure-control kind.
pub const SOURCE_COMBAT_TERMINAL_CONTROL_KIND: u8 = 0x0c;

/// A type-three entry in the shared `FUN_00478a60` pool. `FUN_00458100`
/// writes one after a type-4 figure reaches a production-kind-22 Klinik;
/// `FUN_00478ab0` later turns it into a category-4 `0x84a` spawn.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceKind4DeferredRelocation {
    pub due_at: u32,
    pub island_id: u8,
    pub figure_definition_id: u16,
    pub origin: (f32, f32),
    pub target_descriptor: SourceTargetDescriptor,
}

#[derive(Clone, Copy)]
struct SourceTransferEventCandidate {
    slot: u16,
    x: i16,
    y: i16,
    owner: u8,
    route_radius: u8,
}

/// One entry of an island's eight-slot terrain-event table at source offset
/// `+0x6c`. A free entry has its island byte set to `0xff`; occupied entries
/// retain local map coordinates and the absolute retry deadline at `+0x70`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceTerrainEventSchedule {
    pub island_id: u8,
    pub x: u8,
    pub y: u8,
    pub due_at_ticks: u32,
}

impl Default for SourceTerrainEventSchedule {
    fn default() -> Self {
        Self {
            island_id: u8::MAX,
            x: 0,
            y: 0,
            due_at_ticks: 0,
        }
    }
}

impl SourceTerrainEventSchedule {
    pub const fn is_free(self) -> bool {
        self.island_id == u8::MAX
    }
}

/// The main game simulation state.
pub struct Simulation {
    /// Game clock in centiseconds (600 = 1 displayed minute).
    pub game_clock: u32,
    /// Fractional tick accumulator.
    clock_frac_ms: u32,

    /// Game speed multiplier (1 = normal).
    pub speed_multiplier: u32,

    /// Is the game paused?
    pub paused: bool,

    // Subsystem cadences (1000-ms tick aligned).
    timer_production: SubsystemTimer, // PRODUCTION_TICK_MS (1000)
    timer_population: SubsystemTimer, // 10_000
    timer_events: SubsystemTimer,     // 10_000
    timer_ships: SubsystemTimer,      // 1_000
    timer_market: SubsystemTimer,     // 1_000
    timer_diplomacy: SubsystemTimer,  // 5_000

    // Game state
    pub players: Vec<Player>,
    pub buildings: Vec<BuildingInstance>,
    /// Dynamic map objects reconstructed from scenario INSELHAUS loading.
    /// These occupy the same eight source slots as player-built HQ objects.
    pub source_dynamic_map_objects: Vec<SourceDynamicMapObject>,
    /// Renderer-relevant state for source INSELHAUS command roots.
    pub source_map_cell_states: Vec<SourceMapCellState>,
    /// Final-overwrite static cells addressed by `FUN_0047a650`, including
    /// map kinds that have no live selector record.
    pub source_static_map_roots: Vec<SourceMapCellState>,
    /// Loader-preserved static backing cells from source map array `+0xafc`.
    /// `FUN_004641d0` replays these when a terminal command has no ruin.
    pub source_static_map_backing_cells: Vec<SourceMapCellState>,
    /// Live source kind-13 records retained by the fixed `DAT_005a77e8` table.
    pub source_kind13_locations: SourceKind13LocationTable,
    /// Immutable BGruppe housing definitions reconstructed from haeuser.cod.
    /// The executable reloads this table at startup; save snapshots retain
    /// only the mutable kind-13 city and location records that refer to it.
    pub source_kind13_promotion_definitions: [Option<SourceKind13PromotionDefinition>; 5],
    /// Queued INSELHAUS replacements emitted by completed kind-13 transitions.
    pub source_kind13_replacement_commands: Vec<SourceKind13ReplacementCommand>,
    /// Phase clocks and physical source-table cursor for `FUN_0047b9c0`.
    pub source_kind13_dispatch: SourceKind13DispatchState,
    /// Fixed source city-record pool read by `FUN_0047f8a0`.
    pub source_cities: SourceCityTable,
    /// Shared `DAT_00505e38` event table used by map-anchored source figures.
    pub source_figure_events: SourceFigureEventRegistry,
    /// Live kind-4 occupancy reconstructed from authored `SOLDAT3` figures.
    pub source_kind4_occupants: Vec<SourceKind4Occupant>,
    /// Live `0x84a`/`0x84b` source figure records that do not have an
    /// equivalent local entity type. Categories 1 through 6 remain available
    /// to the common source candidate producer through these records.
    pub source_dynamic_combat_figures: Vec<SourceDynamicCombatFigure>,
    /// Route programs for shared dynamic categories one through three.
    pub(crate) source_dynamic_route_programs: Vec<SourceDynamicRouteProgram>,
    /// Immediate category-6 action records emitted through `FUN_004546e0`.
    /// This observability record is independent from compatibility damage.
    pub source_kind6_actions: Vec<combat::SourceKind6Action>,
    /// Immediate category-4 action records emitted through `FUN_004546e0`.
    /// The source's direct candidate rows do not become land-route targets;
    /// their delayed type-one effects resolve this recorded descriptor.
    pub source_kind4_actions: Vec<combat::SourceKind4Action>,
    /// Immediate category-one through -three actions emitted through
    /// `FUN_00452370` and `FUN_004546e0`.
    pub source_kind13_actions: Vec<combat::SourceKind13Action>,
    /// Kind-14 figures created by category-one through -three `Shotfignr:`
    /// launches through `FUN_00447e90`.
    pub source_kind14_combat_figures: Vec<SourceKind14CombatFigure>,
    /// Kind-15 figures constructed by `FUN_00447f00` from emitted category-6
    /// actions. They remain outside the category-1 through -6 candidate pool.
    pub source_kind15_combat_figures: Vec<SourceKind15CombatFigure>,
    /// Deferred kind-1 records allocated by `FUN_00447880` for category-6
    /// actions. The source drains due records before its ordinary simulation
    /// subsystems run.
    pub source_kind6_deferred_hits: Vec<combat::SourceKind6DeferredHit>,
    /// Deferred type-one records allocated by the category-4 branch of
    /// `FUN_00447880`. They occupy the same 150-record pool as category-6
    /// hits and type-three relocations.
    pub source_kind4_deferred_hits: Vec<combat::SourceKind4DeferredHit>,
    /// Deferred category-one through -three type-one records in the shared
    /// `FUN_00478a60` event pool.
    pub source_kind13_deferred_hits: Vec<combat::SourceKind13DeferredHit>,
    /// Deferred type-three relocations from `FUN_00458100`. These share the
    /// source's 150-event capacity with [`Self::source_kind6_deferred_hits`].
    pub source_kind4_deferred_relocations: Vec<SourceKind4DeferredRelocation>,
    /// Type-7 terminal commands emitted by completed map-root accumulators.
    pub source_kind6_terminal_events: Vec<SourceKind6TerminalEvent>,
    /// Terminal figure-control events emitted by category-4 deferred hits.
    /// These preserve `FUN_00445930`'s `0x0c` transition before the local
    /// entity model applies its corresponding removal.
    pub source_combat_terminal_events: Vec<SourceCombatTerminalEvent>,
    /// Live stationary terminal slices for category-one through -three
    /// compatibility entities. These use the `FUN_0045d380` initializer:
    /// remaining distance and scalar speed are both `0.02`.
    pub source_combat_terminal_slices: Vec<SourceCombatTerminalSlice>,
    /// Source player globals that select type-4 terminal-route dispatch.
    pub source_kind4_dispatch: crate::combat::SourceKind4DispatchState,
    /// Shared category-1/2/3/5 control byte at `DAT_004cf500 + handle *
    /// 0x218`. `FUN_0044ef40` and `FUN_0044df80` require bit six before a
    /// controller can purchase the corresponding figure.
    pub source_shared_figure_control_flags:
        [u8; combat::SOURCE_DYNAMIC_SHARED_SLOT_CAPACITY as usize],
    /// Per-player controller state used by `FUN_0042b4b0`.
    pub source_player_controllers: [SourcePlayerController; combat::SOURCE_KIND4_PLAYER_SLOT_COUNT],
    /// `DAT_006b111c`: physical player cursor for the controller scheduler.
    pub source_player_controller_cursor: u8,
    /// `DAT_005b6304`, the global mode used by `FUN_00423710` to scale the
    /// strongest human weighted figure roster. Source startup initializes it
    /// to two.
    pub source_controller_difficulty_mode: u8,
    /// `DAT_005b6040`: source simulation clock in 100-ms ticks.
    pub source_time_ticks: u32,
    /// Milliseconds not yet promoted into `source_time_ticks`.
    pub source_time_remainder_ms: u32,
    /// `DAT_005dbabc`: elapsed time for `FUN_0046b3e0`'s resource phase.
    pub source_resource_environment_elapsed_ms: u32,
    /// `DAT_005dbab0`: low-three-bit resource-environment phase.
    pub source_resource_environment_phase: u8,
    /// Physical island cursor corresponding to `PTR_DAT_0049a8c0`.
    pub source_resource_environment_cursor: usize,
    /// Per-island source field `+0x64`, recording the last processed
    /// resource-environment phase.
    pub source_resource_environment_last_phase: Vec<u8>,
    /// Eight physical `FUN_0046b920` terrain-event entries for each loaded
    /// island-map index. The entries are source runtime state, not authored
    /// `INSEL5` input, and survive snapshots alongside the associated
    /// type-17 figures.
    pub source_terrain_event_schedules: Vec<[SourceTerrainEventSchedule; 8]>,
    /// Per-island source field `+0x68`. `FUN_0046b3e0` advances it once per
    /// processed island phase and invokes `FUN_0046b920` after every fourth
    /// advance.
    pub source_terrain_event_schedule_counters: Vec<u8>,
    /// `DAT_005a6aec`: shared source map-root dispatch accumulator.
    pub source_map_dispatch_elapsed_ms: u32,
    /// `DAT_005a6c12`: low-three-bit phase consumed by `FUN_0047daf0`.
    pub source_map_dispatch_phase: u8,
    /// `DAT_0054a3b4`: shared source city-dispatch time accumulator.
    pub source_city_dispatch_elapsed_ms: u32,
    /// Low-three-bit phase incremented after the source accumulator exceeds
    /// 9,999 ms.
    pub source_city_dispatch_phase: u8,
    /// Physical source city-record cursor. Each dispatch visits two slots.
    pub source_city_dispatch_cursor: usize,
    /// `DAT_005b6060`: renderer-maintained visible-island flags. The source
    /// rebuilds this 50-slot table from the current map viewport before
    /// `FUN_0047f8a0` reaches the city kind-12 dispatcher; it is display
    /// state rather than save-state data.
    pub source_visible_islands: Vec<bool>,
    /// Monotone revision of the renderer-relevant source map-cell table.
    pub source_map_cell_revision: u64,
    pub building_defs: Vec<BuildingDef>,
    pub figures: Vec<Figure>,
    pub warehouses: Vec<Warehouse>,
    pub island_maps: Vec<IslandMap>,
    pub ai_controllers: Vec<AiController>,
    pub military_units: Vec<MilitaryUnit>,
    pub diplomacy: DiplomacyMatrix,
    pub trade_routes: Vec<TradeRoute>,
    pub trade_ships: Vec<TradeShip>,
    pub coverage_maps: Vec<CoverageMap>,
    pub ocean_map: Option<OceanMap>,
    /// Source-derived carrier constants. Preserved across save snapshots;
    /// the game binary reloads them from `figuren.cod` at startup.
    pub carrier_config: carrier::CarrierConfig,
    /// Source-derived KARREN constants for the type-11 city transfer.
    /// Like `carrier_config`, this is immutable game-data state reloaded at
    /// startup rather than serialized into save snapshots.
    pub city_cart_config: carrier::CityCartConfig,
    /// Source-derived TRAEGER2 constants for native KONTOR type-11
    /// transfers. The compiled `Figurnr` selects this instead of KARREN for
    /// those roots.
    pub city_cart_traeger2_config: carrier::CityCartConfig,
    /// Source-derived civilian sprite layout. Preserved across save
    /// snapshots; the game binary reloads it from `figuren.cod` at startup.
    pub civilian_config: civilian::CivilianConfig,
    /// Source-derived trade-ship cargo capacities. Preserved across
    /// save snapshots; the game binary reloads them from `figuren.cod`.
    pub ship_cargo_config: trade::ShipCargoConfig,

    pub autosave_timer_ms: u32,

    /// Footprint cleanup/replacement events from buildings destroyed
    /// in combat. The game binary drains this each frame to mutate
    /// `Island::tiles` and refresh display.
    pub tile_clears: Vec<TileClear>,

    /// Active scenario objectives for the human player. Re-evaluated each
    /// economy tick. The renderer reads `progress()` and the per-item
    /// `done` flag to draw the objectives panel.
    pub objectives: crate::objectives::ObjectiveSet,

    /// Indices of objectives that flipped to done since last drain.
    /// Game binary consumes these to play the completion cue.
    pub objective_completions: Vec<usize>,

    /// `true` once every scenario objective has completed at
    /// least once. Edge-triggered in the objectives tick: stays
    /// `false` until the last objective flips, then flips to
    /// `true` and emits a "[victory] Scenario complete!" line
    /// in `event_log`. Renderers should freeze input + draw a
    /// victory banner when this flips.
    pub scenario_complete: bool,

    /// Source-shaped MSVC `rand()` stream. Saved and restored exactly so
    /// source dispatches retain their `rand()` consumption order.
    pub(crate) rng_state: crate::source_rand::SourceRand,

    /// Per-island fog-of-war bitmap. Lazily allocated on first sighting.
    pub exploration: Vec<crate::exploration::ExplorationMap>,

    /// Sim-emitted text events (free-trader arrivals, etc.). Drained by
    /// the game binary for voice-announcement routing.
    pub event_log: Vec<String>,

    /// Roving NPC trade ships. Spawned periodically from the world edge
    /// in `tick_ships`; visit player Kontors and trade at standard
    /// prices. See `crate::free_trader`. Ephemeral — not serialised.
    pub free_traders: Vec<crate::free_trader::FreeTrader>,
    /// Cooldown counter (ship-ticks) before another free trader may
    /// spawn. Decremented every ship tick.
    pub free_trader_cooldown: u32,
    /// Last sampled human-player gold; used to fire a treasury voice
    /// alert exactly once per dip below the warning threshold.
    pub last_treasury_warn_gold: i32,
    /// End-of-game outcome for the human player. `Pending` until a
    /// scenario condition triggers victory or defeat (then the
    /// renderer surfaces a banner; the simulation keeps ticking so
    /// the player can keep building, matching the original endless
    /// behaviour).
    pub outcome: GameOutcome,
    /// Indigenous-village trade posts placed by the scenario.
    /// See `crate::native` for the barter mechanics.
    pub native_villages: Vec<crate::native::NativeVillage>,
}

/// Game-over state for the human player slot 0.
///
/// Defeat triggers (from Anno 1602 player.rs constants and manual):
/// - Sustained bankruptcy: `Player::is_game_over` (40 economy ticks
///   below `BANKRUPTCY_THRESHOLD = -1001` gold, set by `tick_economy`).
/// - All Kontors destroyed: a player without an active warehouse has
///   no economy left.
///
/// Victory triggers (manual section on assignment goals — "the
/// assignment goal has been reached when the predetermined opponent,
/// or opponents, have been defeated"):
/// - Every non-human, non-empty player slot is `Defeated`.
/// - Or all scenario `objectives` are complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum GameOutcome {
    #[default]
    Pending,
    Victory,
    Defeat,
}

fn free_trader_port_distance(
    trader: &crate::free_trader::FreeTrader,
    warehouse: &Warehouse,
) -> u32 {
    let dx = (warehouse.tile_x as i32 - trader.world_x).unsigned_abs();
    let dy = (warehouse.tile_y as i32 - trader.world_y).unsigned_abs();
    dx + dy / 4
}

fn free_trader_port_profit_score(
    trader: &crate::free_trader::FreeTrader,
    warehouse: &Warehouse,
    owner_gold: i32,
) -> i32 {
    let mut score = 0i32;
    let mut gold_left = owner_gold.max(0);
    let mut cargo_space = trader.cargo_space();

    // First, mirror `dock_trade`: trader stock sold to the port frees
    // cargo space and earns at least the standard buy price. Score only
    // accepted trades, using the buy-vs-sell spread as the trader's
    // positive reason to prefer this port.
    for &(good, available) in &trader.stock {
        if crate::prices::original_ware_id(good).is_none() {
            continue;
        }
        if available == 0 {
            continue;
        }
        let demand = warehouse.buy_demand(good);
        if demand == 0 {
            continue;
        }
        let price = crate::prices::price_of(good);
        let offered = warehouse.slider(good).buy_price.unwrap_or(price.buy);
        if offered < price.buy || gold_left < offered {
            continue;
        }
        let affordable = (gold_left / offered.max(1)) as u16;
        let qty = available.min(demand).min(affordable);
        if qty == 0 {
            continue;
        }
        gold_left -= qty as i32 * offered;
        cargo_space = cargo_space.saturating_add(qty);
        score += qty as i32 * (offered - price.sell).max(0);
    }

    // Then score surplus this port would sell to the trader. The
    // trader can resell later at its standard buy price; ports asking
    // above the standard sell price are rejected by `dock_trade`.
    for (good, _, _) in warehouse.all_stock() {
        if crate::prices::original_ware_id(good).is_none() {
            continue;
        }
        let qty = warehouse.sell_offer(good).min(cargo_space);
        if qty == 0 {
            continue;
        }
        let price = crate::prices::price_of(good);
        let asked = warehouse.slider(good).sell_price.unwrap_or(price.sell);
        if asked > price.sell {
            continue;
        }
        cargo_space -= qty;
        score += qty as i32 * (price.buy - asked).max(0);
    }

    score
}

fn free_trader_edge_point_from_bounds(
    side: u64,
    offset: u64,
    max_x: i32,
    max_y: i32,
) -> (i32, i32) {
    let max_x = max_x.max(0);
    let max_y = max_y.max(0);
    match side % 4 {
        0 => ((offset % (max_x as u64 + 1)) as i32, 0),
        1 => ((offset % (max_x as u64 + 1)) as i32, max_y),
        2 => (0, (offset % (max_y as u64 + 1)) as i32),
        _ => (max_x, (offset % (max_y as u64 + 1)) as i32),
    }
}

fn free_trader_edge_point_from_ocean(ocean: &OceanMap, side: u64, offset: u64) -> (i32, i32) {
    let max_x = i32::from(ocean.width.saturating_sub(1));
    let max_y = i32::from(ocean.height.saturating_sub(1));
    let edge_len = if side % 4 < 2 {
        max_x as u64 + 1
    } else {
        max_y as u64 + 1
    };

    for delta in 0..edge_len {
        let (x, y) = free_trader_edge_point_from_bounds(side, offset + delta, max_x, max_y);
        if ocean.is_navigable(x, y) {
            return (x, y);
        }
    }

    free_trader_edge_point_from_bounds(side, offset, max_x, max_y)
}

fn free_trader_departure_point_from_bounds(x: i32, y: i32, max_x: i32, max_y: i32) -> (i32, i32) {
    let max_x = max_x.max(0);
    let max_y = max_y.max(0);
    let cx = x.clamp(0, max_x);
    let cy = y.clamp(0, max_y);
    let candidates = [
        ((cx, 0), cy),
        ((cx, max_y), (max_y - cy).abs()),
        ((0, cy), cx),
        ((max_x, cy), (max_x - cx).abs()),
    ];
    candidates
        .into_iter()
        .min_by_key(|(_, distance)| *distance)
        .map(|(point, _)| point)
        .unwrap_or((cx, cy))
}

fn free_trader_departure_route_from_ocean(
    ocean: &OceanMap,
    x: i32,
    y: i32,
) -> Option<((i32, i32), Vec<(i32, i32)>)> {
    let start = ocean.nearest_navigable(x, y)?;
    let max_x = i32::from(ocean.width.saturating_sub(1));
    let max_y = i32::from(ocean.height.saturating_sub(1));
    let mut candidates = [
        free_trader_edge_point_from_ocean(ocean, 0, start.0.max(0) as u64),
        free_trader_edge_point_from_ocean(ocean, 1, start.0.max(0) as u64),
        free_trader_edge_point_from_ocean(ocean, 2, start.1.max(0) as u64),
        free_trader_edge_point_from_ocean(ocean, 3, start.1.max(0) as u64),
    ];
    candidates.sort_by_key(|&(tx, ty)| (tx - start.0).abs() + (ty - start.1).abs());

    for target in candidates {
        if target.0 < 0 || target.1 < 0 || target.0 > max_x || target.1 > max_y {
            continue;
        }
        if target == start {
            return Some((target, Vec::new()));
        }
        if let Some(path) = crate::ocean_map::find_ocean_path(ocean, start, target) {
            return Some((target, path));
        }
    }

    None
}

impl Simulation {
    pub fn new() -> Self {
        Self {
            game_clock: 0,
            clock_frac_ms: 0,
            speed_multiplier: 1,
            paused: false,

            // Main subsystem cadences mirror the binary's `-1000` ms
            // game-tick decrement (1602_exe.c:16110).
            timer_production: SubsystemTimer::new(crate::production::PRODUCTION_TICK_MS),
            timer_population: SubsystemTimer::new(crate::fidelity::POPULATION_TICK_MS),
            timer_events: SubsystemTimer::new(crate::fidelity::EVENT_TICK_MS),
            timer_ships: SubsystemTimer::new(crate::fidelity::SHIP_TICK_MS),
            timer_market: SubsystemTimer::new(crate::fidelity::MARKET_TICK_MS),
            timer_diplomacy: SubsystemTimer::new(crate::fidelity::DIPLOMACY_TICK_MS),

            players: Vec::new(),
            buildings: Vec::new(),
            source_dynamic_map_objects: Vec::new(),
            source_map_cell_states: Vec::new(),
            source_static_map_roots: Vec::new(),
            source_static_map_backing_cells: Vec::new(),
            source_kind13_locations: SourceKind13LocationTable::default(),
            source_kind13_promotion_definitions: std::array::from_fn(|_| None),
            source_kind13_replacement_commands: Vec::new(),
            source_kind13_dispatch: SourceKind13DispatchState::default(),
            source_cities: SourceCityTable::default(),
            source_figure_events: SourceFigureEventRegistry::default(),
            source_kind4_occupants: Vec::new(),
            source_dynamic_combat_figures: Vec::new(),
            source_dynamic_route_programs: Vec::new(),
            source_kind6_actions: Vec::new(),
            source_kind4_actions: Vec::new(),
            source_kind13_actions: Vec::new(),
            source_kind14_combat_figures: Vec::new(),
            source_kind15_combat_figures: Vec::new(),
            source_kind6_deferred_hits: Vec::new(),
            source_kind4_deferred_hits: Vec::new(),
            source_kind13_deferred_hits: Vec::new(),
            source_kind4_deferred_relocations: Vec::new(),
            source_kind6_terminal_events: Vec::new(),
            source_combat_terminal_events: Vec::new(),
            source_combat_terminal_slices: Vec::new(),
            source_kind4_dispatch: crate::combat::SourceKind4DispatchState::default(),
            source_shared_figure_control_flags: [0; combat::SOURCE_DYNAMIC_SHARED_SLOT_CAPACITY
                as usize],
            source_player_controllers: std::array::from_fn(|_| SourcePlayerController::default()),
            source_player_controller_cursor: 0,
            source_controller_difficulty_mode: 2,
            source_time_ticks: 0,
            source_time_remainder_ms: 0,
            source_resource_environment_elapsed_ms: 0,
            source_resource_environment_phase: 0,
            source_resource_environment_cursor: 0,
            source_resource_environment_last_phase: Vec::new(),
            source_terrain_event_schedules: Vec::new(),
            source_terrain_event_schedule_counters: Vec::new(),
            source_map_dispatch_elapsed_ms: 0,
            source_map_dispatch_phase: 0,
            source_city_dispatch_elapsed_ms: 0,
            source_city_dispatch_phase: 0,
            source_city_dispatch_cursor: 0,
            source_visible_islands: vec![false; SOURCE_VISIBLE_ISLAND_SLOTS],
            source_map_cell_revision: 0,
            building_defs: Vec::new(),
            figures: Vec::new(),
            warehouses: Vec::new(),
            island_maps: Vec::new(),
            ai_controllers: Vec::new(),
            military_units: Vec::new(),
            diplomacy: DiplomacyMatrix::new(),
            trade_routes: Vec::new(),
            trade_ships: Vec::new(),
            coverage_maps: Vec::new(),
            ocean_map: None,
            carrier_config: carrier::CarrierConfig::default(),
            city_cart_config: carrier::CityCartConfig::default(),
            city_cart_traeger2_config: carrier::CityCartConfig::default(),
            civilian_config: civilian::CivilianConfig::default(),
            ship_cargo_config: trade::ShipCargoConfig::default(),

            autosave_timer_ms: 0,

            tile_clears: Vec::new(),

            objectives: crate::objectives::ObjectiveSet::default(),
            objective_completions: Vec::new(),
            scenario_complete: false,

            rng_state: crate::source_rand::SourceRand::default(),

            exploration: Vec::new(),
            event_log: Vec::new(),
            free_traders: Vec::new(),
            free_trader_cooldown: 0,
            last_treasury_warn_gold: i32::MAX,
            outcome: GameOutcome::Pending,
            native_villages: Vec::new(),
        }
    }

    /// Set one `DAT_005b6060`-equivalent renderer visibility entry. The
    /// original table has exactly fifty island slots and surrounding source
    /// traversal ignores island IDs outside that range.
    pub fn set_source_island_visible(&mut self, island_id: u8, visible: bool) {
        if let Some(slot) = self.source_visible_islands.get_mut(usize::from(island_id)) {
            *slot = visible;
        }
    }

    /// Rebuild the source visible-island table for a whole-world renderer.
    /// `anno-game` currently renders every loaded island into one texture, so
    /// every loaded source island intersects its display extent.
    pub fn mark_loaded_source_islands_visible(&mut self) {
        self.source_visible_islands.fill(false);
        let island_ids: Vec<_> = self.island_maps.iter().map(|map| map.island_id).collect();
        for island_id in island_ids {
            self.set_source_island_visible(island_id, true);
        }
    }

    /// Collect source-live combat candidates across authored and dynamically
    /// spawned figures. This mirrors the population consumed by
    /// `FUN_0045cd20` before category-specific handlers select an action.
    pub fn source_combat_candidates(&self) -> Vec<combat::SourceCombatCandidate> {
        combat::source_combat_candidate_buffer(
            &self.military_units,
            &self.trade_ships,
            &self.source_dynamic_combat_figures,
        )
        .into_iter()
        .filter(|candidate| {
            !self.source_combat_terminal_slices.iter().any(|slice| {
                match (slice.target, candidate.entity) {
                    (
                        SourceCombatTerminalSliceTarget::TradeShip(slice_index),
                        combat::SourceCombatCandidateEntity::TradeShip(candidate_index),
                    )
                    | (
                        SourceCombatTerminalSliceTarget::DynamicFigure(slice_index),
                        combat::SourceCombatCandidateEntity::DynamicFigure(candidate_index),
                    ) => slice_index == candidate_index,
                    _ => false,
                }
            })
        })
        .collect()
    }

    /// Resolve the `FUN_00444fe0` target footprint required by category-6
    /// combat dispatch. Categories `0x35` and `0x36` name a live dynamic
    /// map-object slot; the other source forms resolve from the retained
    /// island map or their packed coordinate directly.
    pub fn source_kind6_target_rect(
        &self,
        descriptor: SourceTargetDescriptor,
    ) -> Option<SourcePathTargetRect> {
        match descriptor.kind() {
            SourceTargetDescriptor::WORLD_COORDINATE_KIND
            | SourceTargetDescriptor::FIXED_POINT_COORDINATE_KIND => descriptor
                .source_land_route_coordinate()
                .and_then(|origin| SourcePathTargetRect::new(origin, 1, 1)),
            0x32 | 0x33 | 0x34 => self
                .island_maps
                .iter()
                .find_map(|map| map.source_land_target_rect(descriptor)),
            0x35 | 0x36 => {
                let island = descriptor.bytes()[1];
                let slot = descriptor.bytes()[2];
                let table = self.source_dynamic_map_object_table(island);
                let object = table
                    .objects()
                    .find(|object| object.slot == slot)
                    .copied()?;
                let map = self
                    .island_maps
                    .iter()
                    .find(|map| map.island_id == island)?;
                match descriptor.kind() {
                    0x35 => map.source_land_target_rect(SourceTargetDescriptor::from_bytes([
                        0x32,
                        island,
                        object.local_position.0,
                        object.local_position.1,
                    ])),
                    0x36 => map
                        .local_to_source_world((
                            i32::from(object.local_position.0),
                            i32::from(object.local_position.1),
                        ))
                        .and_then(|origin| SourcePathTargetRect::new(origin, 1, 1)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Run the recovered category-6 score order and `FUN_00458d80` geometry
    /// gate for one live category-6 runtime slot. The action queue and the
    /// `FUN_00458e60` player-state predicate remain dispatch responsibilities.
    pub fn source_kind6_selected_target(
        &self,
        runtime_slot: u16,
    ) -> Option<combat::SourceKind6SelectedTarget> {
        let candidates = self.source_combat_candidates();
        let attacker = candidates.iter().find(|candidate| {
            candidate.figure_kind == 6 && candidate.runtime_slot == runtime_slot
        })?;
        combat::source_kind6_select_target(attacker, &candidates, &self.diplomacy, |descriptor| {
            self.source_kind6_target_rect(descriptor)
        })
    }

    /// Run `FUN_00452370`'s shared candidate ranking and direct-target gate
    /// for one live category-one through -three runtime slot. A changed
    /// attack-facing direction is installed by the generic-motion controller
    /// after this selection has completed.
    pub fn source_kind13_selected_target(
        &self,
        runtime_slot: u16,
    ) -> Option<combat::SourceKind13SelectedTarget> {
        let candidates = self.source_combat_candidates();
        let attacker = candidates.iter().find(|candidate| {
            (1..=3).contains(&candidate.figure_kind) && candidate.runtime_slot == runtime_slot
        })?;
        combat::source_kind13_select_live_target(
            attacker,
            &candidates,
            &self.diplomacy,
            |descriptor| self.source_kind6_target_rect(descriptor),
        )
    }

    /// Apply the category-one through -three `FUN_0045e1f0` opcode `0x19`
    /// payload to the addressed shared-table record.
    pub fn apply_source_kind13_score_event(&mut self, runtime_slot: u16, payload: u8) -> bool {
        let Some(figure) = self
            .source_dynamic_combat_figures
            .iter_mut()
            .find(|figure| {
                figure.active
                    && (1..=3).contains(&figure.figure_kind)
                    && figure.runtime_slot == runtime_slot
            })
        else {
            return false;
        };
        figure.source_score_state =
            combat::source_kind13_score_state_after_event(figure.source_score_state, payload);
        true
    }

    /// Execute the no-turn category-one through -three action branch in
    /// `FUN_00452370`. Its immediate executor writes the shared-table
    /// cooldown, reanchors a matching persistent target, and allocates a
    /// type-one record in `FUN_00478a60`'s common pool.
    fn dispatch_source_kind13_action(
        &mut self,
        runtime_slot: u16,
        selected: combat::SourceKind13SelectedTarget,
    ) -> Option<combat::SourceKind13Action> {
        let candidates = self.source_combat_candidates();
        let attacker = *candidates.iter().find(|candidate| {
            (1..=3).contains(&candidate.figure_kind) && candidate.runtime_slot == runtime_slot
        })?;
        let combat::SourceCombatCandidateEntity::DynamicFigure(dynamic_index) = attacker.entity
        else {
            return None;
        };
        let figure = self.source_dynamic_combat_figures.get(dynamic_index)?;
        if figure.source_action_ready_at > self.source_time_ticks {
            return None;
        }
        let attack_direction =
            combat::source_combat_turn_direction(figure.direction, selected.direction);
        if attack_direction != figure.direction {
            return None;
        }
        let action = combat::source_kind13_action(
            &attacker,
            selected,
            attack_direction,
            figure.target_descriptor,
        )?;
        let next_ready_at =
            combat::source_kind13_action_ready_at(self.source_time_ticks, &attacker)?;
        let impact_due_at = combat::source_kind13_impact_due_at(self.source_time_ticks, &attacker)?;
        let launcher_height = self
            .source_dynamic_combat_figures
            .get(dynamic_index)?
            .position_z;
        let kind14_figure = combat::source_kind14_figure_from_action(action, launcher_height);
        let figure = self.source_dynamic_combat_figures.get_mut(dynamic_index)?;
        figure.direction = action.direction;
        figure.source_action_ready_at = next_ready_at;
        if action.flags & 1 != 0 {
            combat::source_action_reanchor(
                &mut figure.position,
                &mut figure.source_motion,
                figure.figure_kind,
                figure.direction,
                action.attacker_position,
            );
        }
        self.source_kind13_actions.push(action);
        if self.source_deferred_event_count() < combat::SOURCE_KIND6_DEFERRED_HIT_CAPACITY {
            self.source_kind13_deferred_hits
                .push(combat::SourceKind13DeferredHit {
                    due_at: impact_due_at,
                    action,
                });
        }
        if let Some(kind14_figure) = kind14_figure {
            if self.source_figure_pool_has_capacity() {
                self.source_kind14_combat_figures.push(kind14_figure);
            }
        }
        Some(action)
    }

    /// Dispatch one ready category-6 action through `FUN_00458ac0` and its
    /// immediate `FUN_004546e0` record construction. The two external
    /// arguments retain the source globals used by the entry owner gate; they
    /// are deliberately independent of the type-4 dispatch state.
    pub fn dispatch_source_kind6_action(
        &mut self,
        runtime_slot: u16,
        active_owner: u8,
        remote_owner_dispatch_enabled: bool,
        attacker_owner_state: u8,
    ) -> Option<combat::SourceKind6Action> {
        let candidates = self.source_combat_candidates();
        let attacker = *candidates.iter().find(|candidate| {
            candidate.figure_kind == 6 && candidate.runtime_slot == runtime_slot
        })?;
        let combat::SourceCombatCandidateEntity::DynamicFigure(dynamic_index) = attacker.entity
        else {
            return None;
        };
        let action = (|| {
            if !combat::source_kind6_owner_dispatch_allows(
                attacker.owner,
                active_owner,
                remote_owner_dispatch_enabled,
                attacker_owner_state,
            ) {
                return None;
            }
            let ready_at = self
                .source_dynamic_combat_figures
                .get(dynamic_index)?
                .source_action_ready_at;
            if !combat::source_kind6_action_is_ready(self.source_time_ticks, ready_at) {
                return None;
            }
            // `FUN_00458ac0` applies `FUN_00458e60` to each ranked row and
            // continues after a rejected row, so exclude those rows before
            // the source ranking/geometry pass.
            let eligible_candidates = candidates
                .iter()
                .copied()
                .filter(|candidate| {
                    combat::source_kind6_target_policy_allows(attacker_owner_state, candidate)
                })
                .collect::<Vec<_>>();
            let selected = combat::source_kind6_select_target(
                &attacker,
                &eligible_candidates,
                &self.diplomacy,
                |descriptor| self.source_kind6_target_rect(descriptor),
            )?;
            let action = combat::source_kind6_action(&attacker, selected)?;
            let next_ready_at =
                combat::source_kind6_action_ready_at(self.source_time_ticks, &attacker)?;
            let impact_due_at =
                combat::source_kind6_impact_due_at(self.source_time_ticks, &attacker)?;
            let launcher_height = self
                .source_dynamic_combat_figures
                .get(dynamic_index)?
                .position_z;
            let kind15_figure = combat::source_kind15_figure_from_action(action, launcher_height);
            let figure = self.source_dynamic_combat_figures.get_mut(dynamic_index)?;
            figure.direction = action.direction;
            figure.source_action_ready_at = next_ready_at;
            self.source_kind6_actions.push(action);
            if self.source_deferred_event_count() < combat::SOURCE_KIND6_DEFERRED_HIT_CAPACITY {
                self.source_kind6_deferred_hits
                    .push(combat::SourceKind6DeferredHit {
                        due_at: impact_due_at,
                        action,
                    });
            }
            if let Some(kind15_figure) = kind15_figure {
                if self.source_figure_pool_has_capacity() {
                    self.source_kind15_combat_figures.push(kind15_figure);
                }
            }
            Some(action)
        })();
        if let Some(figure) = self.source_dynamic_combat_figures.get_mut(dynamic_index) {
            figure.source_motion = combat::source_kind6_controller_dwell_motion();
        }
        action
    }

    /// Install a live `0x84a`/`0x84b` figure in the category table selected
    /// by `FUN_0045d380`. Categories 1, 2, 3, and 5 share table slots, while
    /// categories 4 and 6 own their respective source tables. A later load of
    /// the same source table slot replaces the represented live record.
    pub fn install_source_dynamic_combat_figure(
        &mut self,
        mut figure: SourceDynamicCombatFigure,
    ) -> bool {
        let Some((table, capacity)) = combat::source_dynamic_slot_table(figure.figure_kind) else {
            return false;
        };
        if figure.runtime_slot >= capacity {
            return false;
        }
        if figure.figure_kind == 6 {
            figure.source_action_ready_at = self.source_time_ticks;
        }
        figure.source_motion =
            combat::SourceGenericMotion::stationary_from_loader_flags(figure.flags);
        if !self.source_figure_pool_has_capacity() {
            return false;
        }
        if self.source_combat_terminal_slices.iter().any(|slice| {
            matches!(
                slice.target,
                SourceCombatTerminalSliceTarget::DynamicFigure(_)
            ) && combat::source_dynamic_slot_table(slice.target_figure_kind).is_some_and(
                |(slice_table, _)| {
                    slice_table == table && slice.target_runtime_slot == figure.runtime_slot
                },
            )
        }) {
            return false;
        }

        let runtime_slot = figure.runtime_slot;
        if let Some(index) = self
            .source_dynamic_combat_figures
            .iter()
            .position(|existing| {
                combat::source_dynamic_slot_table(existing.figure_kind).is_some_and(
                    |(existing_table, _)| {
                        existing_table == table && existing.runtime_slot == figure.runtime_slot
                    },
                )
            })
        {
            self.source_dynamic_combat_figures[index] = figure;
        } else {
            self.source_dynamic_combat_figures.push(figure);
        }
        if table == 0 {
            self.clear_source_dynamic_route_program(runtime_slot);
        }
        true
    }

    /// Reconstruct one source island's dynamic map-object table from scenario
    /// records and active `Kind=HQ` building instances carrying source slots.
    pub fn source_dynamic_map_object_table(&self, island: u8) -> SourceDynamicMapObjectTable {
        let mut table = SourceDynamicMapObjectTable::new(island);
        for object in self
            .source_dynamic_map_objects
            .iter()
            .copied()
            .filter(|object| object.island == island)
        {
            debug_assert!(
                table.insert(object),
                "scenario dynamic HQ slots are unique per island"
            );
        }
        for building in &self.buildings {
            if !building.active || building.island_id != island {
                continue;
            }
            let Some(slot) = building.source_dynamic_object_slot else {
                continue;
            };
            let Some(definition) = self.building_defs.get(building.def_id as usize) else {
                continue;
            };
            if definition.kind != "HQ" {
                continue;
            }
            let (Ok(x), Ok(y)) = (u8::try_from(building.tile_x), u8::try_from(building.tile_y))
            else {
                continue;
            };
            let inserted = table.insert(SourceDynamicMapObject {
                island,
                slot,
                owner: building.owner,
                local_position: (x, y),
            });
            debug_assert!(inserted, "dynamic HQ slots are unique per island");
        }
        table
    }

    /// Evaluate `FUN_00475c60` against the simulation's live source city,
    /// kind-4, dynamic-object, and island-runtime state.
    pub fn source_diplomacy_score(&self, player: u8, peer: u8, caller_term: i32) -> i32 {
        let cities = self
            .source_cities
            .active_records()
            .into_iter()
            .map(|city| combat::SourceDiplomacyCity {
                island_id: city.island_id,
                owner_slot: city.owner_slot,
                resident_amount: city.resident_amount,
                tier_population: city.tier_population,
            })
            .collect::<Vec<_>>();
        let kind4_occupants = self
            .source_kind4_occupants
            .iter()
            .filter(|occupant| occupant.active)
            .map(|occupant| combat::SourceDiplomacyKind4Occupant {
                island_id: occupant.island_id,
                owner: occupant.owner,
            })
            .collect::<Vec<_>>();
        let dynamic_objects = self
            .source_dynamic_map_objects
            .iter()
            .map(|object| combat::SourceDiplomacyDynamicObject {
                island_id: object.island,
                owner: object.owner,
            })
            .collect::<Vec<_>>();
        self.diplomacy.source_diplomacy_score(
            player,
            peer,
            caller_term,
            self.source_time_ticks,
            &cities,
            &kind4_occupants,
            &dynamic_objects,
            |island_id| {
                self.island_maps
                    .iter()
                    .find(|map| map.island_id == island_id)
                    .map(|map| map.source_runtime_classification())
                    .unwrap_or(0)
            },
        )
    }

    /// Resolve a `0x35` or `0x36` descriptor against the live HQ objects
    /// currently represented by this simulation's building state.
    pub fn resolve_source_dynamic_map_object_target(
        &self,
        descriptor: SourceTargetDescriptor,
        islands: &[anno_formats::szs::Island],
        definitions: &[anno_formats::cod::BuildingDef],
    ) -> Option<SourceResolvedDynamicTarget> {
        let table = self.source_dynamic_map_object_table(descriptor.bytes()[1]);
        let objects = table.objects().collect::<Vec<_>>();
        descriptor.resolve_dynamic_map_object_target(&objects, islands, definitions)
    }

    /// Allocate the first free source dynamic-object slot for one live HQ
    /// building. The persisted slot later reconstructs the route target's
    /// island, owner, and local position from that building.
    pub fn allocate_source_dynamic_map_object_for_building(
        &mut self,
        building_index: usize,
    ) -> Option<SourceDynamicMapObject> {
        let building = self.buildings.get(building_index)?;
        if !building.active || building.source_dynamic_object_slot.is_some() {
            return None;
        }
        let definition = self.building_defs.get(building.def_id as usize)?;
        if definition.kind != "HQ" {
            return None;
        }
        let local_position = (
            u8::try_from(building.tile_x).ok()?,
            u8::try_from(building.tile_y).ok()?,
        );
        let island = building.island_id;
        let owner = building.owner;

        let object = self
            .source_dynamic_map_object_table(island)
            .allocate(owner, local_position)?;
        self.buildings[building_index].source_dynamic_object_slot = Some(object.slot);
        Some(object)
    }

    /// Release a building's source dynamic-object slot before it is removed
    /// or otherwise ceases to be a live map object.
    pub fn release_source_dynamic_map_object_for_building(
        &mut self,
        building_index: usize,
    ) -> Option<SourceDynamicMapObject> {
        let building = self.buildings.get_mut(building_index)?;
        let slot = building.source_dynamic_object_slot.take()?;
        let local_position = (
            u8::try_from(building.tile_x).ok()?,
            u8::try_from(building.tile_y).ok()?,
        );
        Some(SourceDynamicMapObject {
            island: building.island_id,
            slot,
            owner: building.owner,
            local_position,
        })
    }

    /// Anno 1602 uses fixed per-good prices — no supply/demand drift.
    /// Returns the static price from `prices::price_of`.
    pub fn current_price(&self, good: crate::types::Good) -> crate::prices::GoodPrice {
        crate::prices::price_of(good)
    }

    pub fn subsystem_timing_snapshot(
        &self,
    ) -> Vec<(crate::fidelity::Subsystem, crate::fidelity::TimingCadence)> {
        use crate::fidelity::{Subsystem, TimingCadence};
        vec![
            (
                Subsystem::Production,
                TimingCadence::Interval(self.timer_production.interval_ms),
            ),
            (
                Subsystem::Population,
                TimingCadence::Interval(self.timer_population.interval_ms),
            ),
            (
                Subsystem::Diplomacy,
                TimingCadence::Interval(self.timer_diplomacy.interval_ms),
            ),
            (
                Subsystem::MarketCoverage,
                TimingCadence::Interval(self.timer_market.interval_ms),
            ),
            (
                Subsystem::Ships,
                TimingCadence::Interval(self.timer_ships.interval_ms),
            ),
            (
                Subsystem::Events,
                TimingCadence::Interval(self.timer_events.interval_ms),
            ),
            (Subsystem::Military, TimingCadence::PerUpdate),
        ]
    }

    pub fn seed_source_rand(&mut self, seed: u32) {
        self.rng_state = crate::source_rand::SourceRand::new(seed);
    }

    pub fn seed_source_rand_from_get_tick_count(&mut self) {
        self.rng_state = crate::source_rand::SourceRand::from_get_tick_count();
    }

    pub fn next_source_rand(&mut self) -> u16 {
        self.rng_state.next()
    }

    fn next_rand(&mut self) -> u64 {
        self.next_source_rand() as u64
    }

    fn source_purchase_actor_is_eligible(&self, player_slot: u8) -> bool {
        player_slot == self.source_kind4_dispatch.active_player_slot
            || (self.source_kind4_dispatch.remote_owner_dispatch_enabled
                && self
                    .source_kind4_dispatch
                    .faction_states
                    .get(usize::from(player_slot))
                    .is_some_and(|state| (0x0b..=0x0e).contains(state)))
    }

    fn source_shared_figure_entity(&self, handle: u16) -> Option<SourceSharedFigureEntity> {
        if handle >= combat::SOURCE_DYNAMIC_SHARED_SLOT_CAPACITY {
            return None;
        }

        self.source_dynamic_combat_figures
            .iter()
            .rposition(|figure| {
                figure.active
                    && matches!(figure.figure_kind, 1 | 2 | 3 | 5)
                    && figure.runtime_slot == handle
            })
            .map(SourceSharedFigureEntity::DynamicFigure)
            .or_else(|| {
                self.military_units
                    .iter()
                    .position(|unit| {
                        unit.active
                            && !unit.source_terminal_pending
                            && matches!(unit.source_figure_kind, Some(1 | 2 | 3 | 5))
                            && unit.source_live_runtime_slot == Some(handle)
                    })
                    .map(SourceSharedFigureEntity::MilitaryUnit)
            })
            .or_else(|| {
                self.trade_ships
                    .iter()
                    .position(|ship| {
                        ship.active
                            && matches!(ship.source_figure_kind, Some(1 | 2 | 3 | 5))
                            && ship.source_runtime_slot == Some(handle)
                    })
                    .map(SourceSharedFigureEntity::TradeShip)
            })
    }

    /// `FUN_0044f000`, `FUN_0044f110`, and `FUN_004489e0` address only the
    /// shared `DAT_004cf358` figure table. The controller's raw handle is
    /// therefore not interchangeable with the compatibility unit or ship
    /// handles accepted by the wider event resolver above.
    fn source_controller_dynamic_figure_index(&self, handle: u16) -> Option<usize> {
        (handle < combat::SOURCE_DYNAMIC_SHARED_SLOT_CAPACITY).then_some(())?;
        self.source_dynamic_combat_figures
            .iter()
            .rposition(|figure| {
                figure.active
                    && (1..=3).contains(&figure.figure_kind)
                    && figure.runtime_slot == handle
            })
    }

    fn source_shared_figure_owner(&self, entity: SourceSharedFigureEntity) -> u8 {
        match entity {
            SourceSharedFigureEntity::MilitaryUnit(index) => self.military_units[index].owner,
            SourceSharedFigureEntity::TradeShip(index) => self.trade_ships[index].owner,
            SourceSharedFigureEntity::DynamicFigure(index) => {
                self.source_dynamic_combat_figures[index].owner
            }
        }
    }

    /// Store `FUN_004489e0`'s persistent target on the shared dynamic record
    /// selected by a player controller.
    fn set_source_controller_figure_target_descriptor(
        &mut self,
        handle: u16,
        descriptor: SourceTargetDescriptor,
    ) -> bool {
        let Some(index) = self.source_controller_dynamic_figure_index(handle) else {
            return false;
        };
        self.source_dynamic_combat_figures[index].target_descriptor = descriptor;
        self.clear_source_dynamic_route_program(handle);
        true
    }

    fn clear_source_dynamic_route_program(&mut self, runtime_slot: u16) {
        self.source_dynamic_route_programs
            .retain(|route| route.runtime_slot != runtime_slot);
    }

    fn replace_source_dynamic_route_program(&mut self, runtime_slot: u16, program: Vec<u8>) {
        let route = SourceDynamicRouteProgram {
            runtime_slot,
            program,
            cursor: 0,
        };
        if let Some(index) = self
            .source_dynamic_route_programs
            .iter()
            .position(|existing| existing.runtime_slot == runtime_slot)
        {
            self.source_dynamic_route_programs[index] = route;
        } else {
            self.source_dynamic_route_programs.push(route);
        }
    }

    fn source_dynamic_route_steps(
        start: (i32, i32),
        path: Vec<(i32, i32)>,
    ) -> Option<Vec<SourceRouteStep>> {
        let mut previous = start;
        path.into_iter()
            .map(|next| {
                let direction = match (next.0 - previous.0, next.1 - previous.1) {
                    (0, -1) => 1,
                    (1, -1) => 2,
                    (1, 0) => 3,
                    (1, 1) => 4,
                    (0, 1) => 5,
                    (-1, 1) => 6,
                    (-1, 0) => 7,
                    (-1, -1) => 8,
                    _ => return None,
                };
                previous = next;
                Some(SourceRouteStep {
                    direction,
                    metadata: 0,
                })
            })
            .collect()
    }

    /// Execute the state-four `FUN_00455a20` route branch for a shared
    /// category-one through -three figure whose controller target names a
    /// static kind-`0x34` island cell. The route program is source-owned live
    /// state, so a descriptor change or table-slot replacement clears it.
    fn advance_source_dynamic_descriptor_route(&mut self, runtime_slot: u16) -> bool {
        let Some((figure_definition_id, descriptor, start)) = self
            .source_dynamic_combat_figures
            .iter()
            .find(|figure| {
                figure.active
                    && (1..=3).contains(&figure.figure_kind)
                    && figure.runtime_slot == runtime_slot
            })
            .map(|figure| {
                (
                    figure.figure_definition_id,
                    figure.target_descriptor,
                    (
                        figure.position.0.trunc() as i32,
                        figure.position.1.trunc() as i32,
                    ),
                )
            })
        else {
            return false;
        };
        if descriptor.kind() != 0x34 {
            return false;
        }

        let route_needs_rebuild = self
            .source_dynamic_route_programs
            .iter()
            .find(|route| route.runtime_slot == runtime_slot)
            .is_none_or(|route| {
                route
                    .program
                    .get(route.cursor)
                    .is_none_or(|command| command & 0xf0 == 0xc0)
            });
        if route_needs_rebuild {
            let program = self
                .island_maps
                .iter()
                .find_map(|map| map.source_ship_target_rect(descriptor))
                .and_then(|target| {
                    self.ocean_map
                        .as_ref()?
                        .find_source_ship_path_in_window_for_resolved_target(
                            start,
                            target,
                            SourceShipTargetRouteBranch::Threshold {
                                approach_radius: 5,
                                limit: 2,
                            },
                            SourceShipRouteWindow::Normal,
                        )
                })
                .and_then(|path| Self::source_dynamic_route_steps(start, path))
                .and_then(|steps| {
                    encode_source_route_truncated(
                        &steps,
                        SOURCE_SHIP_ROUTE_RUN_LIMIT,
                        SOURCE_SHIP_ROUTE_CAPACITY,
                    )
                    .ok()
                })
                .unwrap_or_else(|| vec![SOURCE_ROUTE_TERMINATOR]);
            self.replace_source_dynamic_route_program(runtime_slot, program);
        }

        let command = {
            let Some(route) = self
                .source_dynamic_route_programs
                .iter_mut()
                .find(|route| route.runtime_slot == runtime_slot)
            else {
                return true;
            };
            let command = route.program.get(route.cursor).copied();
            if command.is_some_and(|command| command & 0xf0 != 0xc0) {
                route.cursor += 1;
            }
            command
        };
        let Some(command) = command else {
            return true;
        };
        let Some((direction, motion)) =
            combat::source_dynamic_route_run_motion(figure_definition_id, command)
        else {
            return true;
        };
        if let Some(figure) = self
            .source_dynamic_combat_figures
            .iter_mut()
            .find(|figure| {
                figure.active
                    && (1..=3).contains(&figure.figure_kind)
                    && figure.runtime_slot == runtime_slot
            })
        {
            figure.direction = direction;
            figure.source_motion = motion;
        }
        true
    }

    fn source_shared_figure_purchase_fields(
        &self,
        entity: SourceSharedFigureEntity,
    ) -> Option<(u8, u8, u16, u16)> {
        match entity {
            SourceSharedFigureEntity::MilitaryUnit(index) => {
                let unit = &self.military_units[index];
                Some((
                    unit.source_figure_kind?,
                    unit.owner,
                    unit.source_figure_definition_id?,
                    unit.source_energy,
                ))
            }
            SourceSharedFigureEntity::TradeShip(index) => {
                let ship = &self.trade_ships[index];
                Some((
                    ship.source_figure_kind?,
                    ship.owner,
                    ship.source_figure_definition_id?,
                    ship.source_energy,
                ))
            }
            SourceSharedFigureEntity::DynamicFigure(index) => {
                let figure = &self.source_dynamic_combat_figures[index];
                Some((
                    figure.figure_kind,
                    figure.owner,
                    u16::from(figure.figure_definition_id),
                    figure.source_energy,
                ))
            }
        }
    }

    fn transfer_source_shared_figure(&mut self, entity: SourceSharedFigureEntity, buyer: u8) {
        let replacement_kind = self
            .source_kind4_dispatch
            .faction_states
            .get(usize::from(buyer))
            .copied()
            .and_then(|state| match state {
                0x0e => Some(3),
                0 | 0x0c => Some(1),
                _ => None,
            });

        match entity {
            SourceSharedFigureEntity::MilitaryUnit(index) => {
                let unit = &mut self.military_units[index];
                unit.owner = buyer;
                if let Some(kind) = replacement_kind {
                    unit.source_figure_kind = Some(kind);
                }
            }
            SourceSharedFigureEntity::TradeShip(index) => {
                let ship = &mut self.trade_ships[index];
                ship.owner = buyer;
                if let Some(kind) = replacement_kind {
                    ship.source_figure_kind = Some(kind);
                }
            }
            SourceSharedFigureEntity::DynamicFigure(index) => {
                let figure = &mut self.source_dynamic_combat_figures[index];
                figure.owner = buyer;
                if let Some(kind) = replacement_kind {
                    figure.figure_kind = kind;
                }
            }
        }
    }

    /// Mirror `FUN_0044ef90`'s bit-six transfer-control write for one shared
    /// category-1/2/3/5 figure handle.
    pub fn set_source_figure_purchase_enabled(&mut self, handle: u16, enabled: bool) {
        if let Some(flags) = self
            .source_shared_figure_control_flags
            .get_mut(usize::from(handle))
        {
            *flags = (*flags & !0x40) | (u8::from(enabled) << 6);
        }
    }

    /// Execute the source `0x84d` figure-purchase event. The transfer phase
    /// credits an eligible seller, clears bit six through opcode `0x20`,
    /// changes ownership through opcode five, and immediately dispatches the
    /// matching buyer-debit phase.
    pub fn execute_source_figure_purchase(&mut self, event: SourceFigurePurchaseEvent) {
        let Some(entity) = self.source_shared_figure_entity(event.figure_handle) else {
            return;
        };
        if self.source_shared_figure_control_flags[usize::from(event.figure_handle)] & 0x40 == 0 {
            return;
        }

        let seller = self.source_shared_figure_owner(entity);
        if !self.source_purchase_actor_is_eligible(seller) {
            return;
        }

        if let Some(player) = self.players.get_mut(usize::from(seller)) {
            player.gold += i32::from(event.amount);
        }
        self.source_shared_figure_control_flags[usize::from(event.figure_handle)] &= !0x40;
        self.transfer_source_shared_figure(entity, event.buyer);
        self.mark_source_controller_figure_roster_dirty(seller);
        self.mark_source_controller_figure_roster_dirty(event.buyer);

        if self.source_purchase_actor_is_eligible(event.buyer) {
            if let Some(player) = self.players.get_mut(usize::from(event.buyer)) {
                player.gold -= i32::from(event.amount);
            }
        }
    }

    /// Execute `FUN_00422030` for one source player controller. The caller
    /// supplies its live owned-figure count and `+0x4dc` capacity; this
    /// routine scans shared handles in source-table order and retains the
    /// first strict minimum before emitting `0x84d`.
    pub fn source_controller_purchase_figure(
        &mut self,
        buyer: u8,
        owned_figure_count: u32,
        figure_capacity: u32,
    ) -> Option<SourceFigurePurchaseEvent> {
        if owned_figure_count >= figure_capacity {
            return None;
        }

        let mut selected = None;
        let mut best_cost = 100_000_i32;
        for handle in 0..combat::SOURCE_DYNAMIC_SHARED_SLOT_CAPACITY {
            if self.source_shared_figure_control_flags[usize::from(handle)] & 0x40 == 0 {
                continue;
            }
            let Some(entity) = self.source_shared_figure_entity(handle) else {
                continue;
            };
            let Some((figure_kind, owner, definition_id, source_energy)) =
                self.source_shared_figure_purchase_fields(entity)
            else {
                continue;
            };
            if !(1..=3).contains(&figure_kind) || owner == buyer {
                continue;
            }
            let cost =
                anno_formats::szs::source_figure_purchase_cost(definition_id, source_energy) as i32;
            if cost < best_cost {
                best_cost = cost;
                selected = Some(handle);
            }
        }

        let handle = selected?;
        if best_cost >= self.players.get(usize::from(buyer))?.gold {
            return None;
        }
        let event = SourceFigurePurchaseEvent {
            buyer,
            figure_handle: handle,
            amount: best_cost as u16,
        };
        self.execute_source_figure_purchase(event);
        Some(event)
    }

    /// Set player byte `+0x7d`, causing the next `FUN_0042b4b0` pass for this
    /// controller to rebuild `+0x10734` through `FUN_0044f000`.
    pub fn mark_source_controller_figure_roster_dirty(&mut self, player_slot: u8) {
        if let Some(controller) = self
            .source_player_controllers
            .get_mut(usize::from(player_slot))
        {
            controller.figure_roster_dirty = true;
        }
    }

    /// Install the PLAYER4 `+0x9c` per-player figure clamps used by
    /// `FUN_00423710`. Controllers without a PLAYER4 record retain `None`
    /// and therefore preserve an externally restored `+0x4dc` value.
    pub fn configure_source_controller_figure_capacity_limits(
        &mut self,
        players: &[anno_formats::szs::PlayerSlotInit],
    ) {
        for (slot, controller) in self.source_player_controllers.iter_mut().enumerate() {
            controller.figure_capacity_limit = players
                .get(slot)
                .map(|player| player.controller_figure_capacity_limit_0x9c);
        }
    }

    /// Seed controller `+0x3e7c` from `FUN_0040f580`'s physical city scan.
    /// The map score uses each city's retained source map-owner selector.
    pub fn configure_source_controller_active_cities(&mut self) {
        for player_slot in 0..combat::SOURCE_KIND4_PLAYER_SLOT_COUNT {
            self.configure_source_controller_active_city(player_slot);
        }
    }

    fn configure_source_controller_active_city(&mut self, player_slot: usize) {
        let city_slot = self.source_cities.source_controller_city_slot(
            player_slot as u8,
            |island_id, source_owner| {
                self.island_maps
                    .iter()
                    .find(|map| map.island_id == island_id)
                    .map(|map| map.source_controller_city_suitability_score(source_owner))
                    .unwrap_or(0)
            },
        );
        let city_owner =
            city_slot.and_then(|slot| self.source_cities.record(slot).map(|city| city.owner_slot));
        let controller = &mut self.source_player_controllers[player_slot];
        controller.active_city_slot = city_slot;
        controller.active_city_owner = city_owner;
    }

    fn source_controller_figure_capacity_from_inputs(
        desired_figure_count: u32,
        owned_figure_count: u32,
        profile_present: bool,
        player_slot: usize,
        score_totals: [u32; combat::SOURCE_KIND4_PLAYER_SLOT_COUNT],
        faction_states: [u8; combat::SOURCE_KIND4_PLAYER_SLOT_COUNT],
        difficulty_mode: u8,
        active_city_figure_metric: u16,
        figure_capacity_limit: u16,
    ) -> (u32, u32) {
        let figure_roster_ratio = if desired_figure_count > 1 {
            let quotient = owned_figure_count / desired_figure_count;
            if quotient < 1 {
                1
            } else {
                quotient.wrapping_add(1)
            }
        } else {
            owned_figure_count
        };

        let mut figure_capacity = desired_figure_count.min(4);
        if profile_present {
            let mut strongest_human_total = score_totals
                .iter()
                .copied()
                .zip(faction_states)
                .filter_map(|(total, state)| (state == 0).then_some(total))
                .max()
                .unwrap_or(0);
            strongest_human_total = match difficulty_mode {
                0 => strongest_human_total.wrapping_sub(strongest_human_total >> 2),
                2 => strongest_human_total.wrapping_add(strongest_human_total >> 1),
                3 => strongest_human_total.wrapping_mul(2),
                _ => strongest_human_total,
            };

            let local_total = score_totals.get(player_slot).copied().unwrap_or(0);
            if figure_capacity != 0
                && local_total < strongest_human_total
                && active_city_figure_metric > 0x20
                && local_total / figure_capacity > 3
            {
                figure_capacity = strongest_human_total >> 3;
            }
            figure_capacity = figure_capacity.max(desired_figure_count.wrapping_mul(2));
            if owned_figure_count < figure_capacity {
                figure_capacity = owned_figure_count.wrapping_add(1);
            }
        }

        (
            figure_roster_ratio,
            figure_capacity.min(u32::from(figure_capacity_limit)),
        )
    }

    fn refresh_source_player_controller_figure_capacity(&mut self, player_slot: usize) {
        let Some(figure_capacity_limit) =
            self.source_player_controllers[player_slot].figure_capacity_limit
        else {
            return;
        };
        let (desired_figure_count, owned_figure_count) = {
            let controller = &self.source_player_controllers[player_slot];
            (
                controller.desired_figure_count,
                controller.owned_figure_handles.len() as u32,
            )
        };
        let (figure_roster_ratio, figure_capacity) =
            Self::source_controller_figure_capacity_from_inputs(
                desired_figure_count,
                owned_figure_count,
                // `FUN_0040f580` clears the complete controller record,
                // including `+0x106f8`; the executable contains no later
                // instruction that writes that displacement. Its weighted
                // `FUN_00423710` branch is therefore inactive at runtime.
                false,
                player_slot,
                [0; combat::SOURCE_KIND4_PLAYER_SLOT_COUNT],
                self.source_kind4_dispatch.faction_states,
                self.source_controller_difficulty_mode,
                0,
                figure_capacity_limit,
            );
        let controller = &mut self.source_player_controllers[player_slot];
        controller.figure_roster_ratio = figure_roster_ratio;
        controller.figure_capacity = figure_capacity;
    }

    fn initialize_source_player_controller(&mut self, player_slot: usize) {
        let needs_initialization =
            self.source_player_controllers
                .get(player_slot)
                .is_some_and(|controller| {
                    !controller.initialized
                        || self
                            .source_time_ticks
                            .wrapping_sub(controller.initialized_at_ticks)
                            > 36_000
                });
        if !needs_initialization {
            return;
        }

        self.configure_source_controller_active_city(player_slot);
        let faction_state = self
            .source_kind4_dispatch
            .faction_states
            .get(player_slot)
            .copied()
            .unwrap_or(u8::MAX);
        let action_timer_ms = i32::from(self.next_source_rand() & 0x0fff);
        let controller = &mut self.source_player_controllers[player_slot];
        controller.initialized = true;
        controller.initialized_at_ticks = self.source_time_ticks;
        controller.action_timer_ms = action_timer_ms;
        controller.city_management_timer_ms = 20_000;
        controller.maintenance_timer_ms = 3_000;
        controller.desired_figure_count = u32::from(faction_state == 0x0c) * 3;
        controller.figure_roster_ratio = 0;
        controller.figure_capacity = 0;
        controller.city_management_profile = None;
        controller.action_figure_handle = None;
        controller.action_target_island_id = None;
        controller.action_target_tile = None;
        controller.action_source_candidate_tile = None;
        controller.action_target_direction = None;
        controller.action_arrival_retries = 0;
        controller.island_search_cursor = 0;
        controller.island_search_requirement = None;
        controller.island_search_selected_island_id = None;
        controller.island_search_area_threshold = 0;
        controller.island_search_minimum_area = 0;
        controller.island_search_deferred_requirement = None;
        controller.island_search_retry_at_ticks = None;
        controller.source_city_rectangles.clear();
        controller.selected_city_active = false;
        controller.action_stack.clear();
        controller.action_budget = 0;
        controller.purchase_predecessor_issued = false;
        controller.owned_figure_handles.clear();
        controller.figure_roster_dirty = matches!(faction_state, 0x0c | 0x0e);
        if !controller.figure_roster_dirty {
            controller.figure_roster_ratio = 1;
            return;
        }
        let desired_figure_count = controller.desired_figure_count;
        self.refresh_source_player_controller_roster(player_slot);
        let owned_figure_count = self.source_player_controllers[player_slot]
            .owned_figure_handles
            .len() as u32;
        self.source_player_controllers[player_slot].figure_roster_ratio =
            if desired_figure_count < 2 {
                1
            } else {
                owned_figure_count / desired_figure_count + 1
            };
    }

    fn refresh_source_player_controller_roster(&mut self, player_slot: usize) {
        let owner = player_slot as u8;
        let handles = (0..combat::SOURCE_DYNAMIC_SHARED_SLOT_CAPACITY)
            .filter(|&handle| {
                let Some(index) = self.source_controller_dynamic_figure_index(handle) else {
                    return false;
                };
                self.source_dynamic_combat_figures[index].owner == owner
            })
            .collect();
        let controller = &mut self.source_player_controllers[player_slot];
        controller.owned_figure_handles = handles;
        controller.figure_roster_dirty = false;
    }

    fn source_controller_active_city_owned_by(&self, player_slot: usize) -> bool {
        let controller = &self.source_player_controllers[player_slot];
        controller
            .active_city_slot
            .map(|slot| {
                self.source_cities
                    .record(slot)
                    .is_some_and(|city| city.owner_slot == player_slot as u8)
            })
            .unwrap_or(controller.active_city_owner == Some(player_slot as u8))
    }

    /// Replay `FUN_0040e570` for one state-two island candidate. The target
    /// must not contain a city owned by this controller. A foreign city owned
    /// by faction state zero or `0x0c` is permitted only after that owner's
    /// live PLAYER4 `+0x86` city count reaches three.
    fn source_controller_target_island_is_eligible(
        &self,
        player_slot: usize,
        island_id: u8,
    ) -> bool {
        self.source_cities.active_records().into_iter().all(|city| {
            if city.island_id != island_id {
                return true;
            }
            if city.owner_slot == player_slot as u8 {
                return false;
            }
            let faction_state = self
                .source_kind4_dispatch
                .faction_states
                .get(usize::from(city.owner_slot))
                .copied()
                .unwrap_or(u8::MAX);
            !matches!(faction_state, 0 | 0x0c)
                || self
                    .source_cities
                    .source_city_count_for_owner(city.owner_slot)
                    >= 3
        })
    }

    /// Begin the `FUN_00416370` supplier used by the source controller before
    /// it queues action two. `requirement` is the raw `+0x1dcc` selector from
    /// `FUN_00416ca0`; the island map provides the exact `FUN_0046aff0`
    /// strength through its retained `INSEL5` resource state.
    pub fn request_source_controller_island_search(
        &mut self,
        player_slot: usize,
        requirement: Option<u8>,
    ) -> bool {
        let Some(controller) = self.source_player_controllers.get_mut(player_slot) else {
            return false;
        };
        controller.island_search_requirement = requirement;
        controller.island_search_selected_island_id = None;
        controller.island_search_area_threshold = 0x708;
        controller.island_search_minimum_area = 300;
        controller.island_search_deferred_requirement = None;
        controller.island_search_retry_at_ticks = None;
        self.advance_source_controller_island_search(player_slot)
    }

    fn source_controller_island_is_active(&self, island_id: u8) -> bool {
        self.island_maps.iter().any(|map| map.island_id == island_id)
    }

    fn source_controller_island_satisfies_requirement(
        &self,
        island_id: u8,
        requirement: Option<u8>,
    ) -> bool {
        match requirement {
            None => true,
            Some(selector) => self
                .island_maps
                .iter()
                .find(|map| map.island_id == island_id)
                .is_some_and(|map| map.source_resource_strength(selector) == 0x80),
        }
    }

    /// Advance the full fifty-slot `FUN_00416370` island cursor once. The
    /// source assumes a populated world and therefore has unbounded loops;
    /// the model bounds its equivalent traversal at eight complete sweeps,
    /// matching the maximum island-local city-pointer count.
    fn advance_source_controller_island_search(&mut self, player_slot: usize) -> bool {
        let Some(controller) = self.source_player_controllers.get(player_slot) else {
            return false;
        };
        if controller
            .island_search_retry_at_ticks
            .is_some_and(|retry_at| self.source_time_ticks.wrapping_sub(retry_at) > u32::MAX - 600)
        {
            return false;
        }
        let faction_state = self
            .source_kind4_dispatch
            .faction_states
            .get(player_slot)
            .copied()
            .unwrap_or(u8::MAX);

        if faction_state == 0x0e {
            let mut completed_sweeps = 1_u8;
            for _ in 0..SOURCE_VISIBLE_ISLAND_SLOTS * 8 {
                let island_id = {
                    let controller = &mut self.source_player_controllers[player_slot];
                    controller.island_search_cursor =
                        (controller.island_search_cursor + 1) % SOURCE_VISIBLE_ISLAND_SLOTS as u8;
                    if controller.island_search_cursor == 0 {
                        completed_sweeps = completed_sweeps.saturating_add(1);
                    }
                    controller.island_search_cursor
                };
                if self.source_controller_island_is_active(island_id)
                    && completed_sweeps >= self.source_cities.source_city_count_on_island(island_id)
                {
                    let controller = &mut self.source_player_controllers[player_slot];
                    controller.island_search_selected_island_id = Some(island_id);
                    if !controller.action_stack.contains(&2) {
                        controller.action_stack.push(2);
                    }
                    return true;
                }
            }
            return false;
        }

        for _ in 0..SOURCE_VISIBLE_ISLAND_SLOTS {
            let (island_id, wrapped, requirement, desired_figure_count) = {
                let controller = &mut self.source_player_controllers[player_slot];
                controller.island_search_cursor =
                    (controller.island_search_cursor + 1) % SOURCE_VISIBLE_ISLAND_SLOTS as u8;
                (
                    controller.island_search_cursor,
                    controller.island_search_cursor == 0,
                    controller.island_search_requirement,
                    controller.desired_figure_count,
                )
            };
            if wrapped {
                let controller = &mut self.source_player_controllers[player_slot];
                if controller.island_search_area_threshold <= controller.island_search_minimum_area
                {
                    if requirement.is_none() && desired_figure_count > 2 {
                        controller.island_search_deferred_requirement = None;
                        controller.island_search_retry_at_ticks =
                            Some(self.source_time_ticks.wrapping_add(600));
                        return false;
                    }
                    controller.island_search_area_threshold = 0x708;
                    controller.island_search_minimum_area = 300;
                    if let Some(requirement) = controller.island_search_requirement.take() {
                        controller.island_search_deferred_requirement = Some(requirement);
                        return false;
                    }
                    controller.island_search_deferred_requirement = None;
                    controller.island_search_retry_at_ticks =
                        Some(self.source_time_ticks.wrapping_add(600));
                    return false;
                }
                controller.island_search_area_threshold -= 0x32;
            }

            if !self.source_controller_island_is_active(island_id)
                || !self.source_controller_target_island_is_eligible(player_slot, island_id)
                || !self.source_controller_island_satisfies_requirement(island_id, requirement)
            {
                continue;
            }
            let island_city_count = self.source_cities.source_city_count_on_island(island_id);
            if desired_figure_count <= 1 && island_city_count == 0 {
                continue;
            }
            let controller = &mut self.source_player_controllers[player_slot];
            if controller.island_search_minimum_area <= controller.island_search_area_threshold {
                controller.island_search_selected_island_id = Some(island_id);
                if !controller.action_stack.contains(&2) {
                    controller.action_stack.push(2);
                }
                return true;
            }
            return false;
        }
        false
    }

    /// Replay `FUN_00416fd0`: consume state two, retain the source city-search
    /// rectangle records, and queue state three when their summed area meets
    /// controller `+0x4e4`.
    pub fn advance_source_controller_city_area_search(&mut self, player_slot: usize) -> bool {
        let Some(controller) = self.source_player_controllers.get(player_slot) else {
            return false;
        };
        if controller.action_stack.last().copied() != Some(2) {
            return false;
        }
        let Some(island_id) = controller.island_search_selected_island_id else {
            return false;
        };
        let Some(map) = self
            .island_maps
            .iter()
            .find(|map| map.island_id == island_id)
        else {
            return false;
        };
        let rectangles = map.source_controller_city_rectangles(7, 5, 5);
        let total_area: i64 = rectangles
            .iter()
            .map(|rectangle| i64::from(rectangle.area))
            .sum();

        let controller = &mut self.source_player_controllers[player_slot];
        controller.action_stack.pop();
        controller.source_city_rectangles = rectangles;
        if i64::from(controller.island_search_area_threshold) <= total_area
            && !controller.action_stack.contains(&3)
        {
            controller.action_stack.push(3);
            return true;
        }
        false
    }

    /// `FUN_0040f4d0`: any source city owned by this controller with at least
    /// 1200 residents across its five BGRUPPE totals selects the short
    /// state-three line-distance branch.
    fn source_controller_city_has_large_population(&self, player_slot: usize) -> bool {
        self.source_cities.active_records().into_iter().any(|city| {
            city.owner_slot == player_slot as u8
                && city.tier_population.into_iter().sum::<u32>() >= 0x4b0
        })
    }

    /// Replay `FUN_004172e0`: consume state three, choose the strict-largest
    /// action-two rectangle, score one head from each `FUN_004160c0` segment,
    /// and queue state four for the first strict green-density maximum.
    pub fn advance_source_controller_city_candidate_search(&mut self, player_slot: usize) -> bool {
        let Some(controller) = self.source_player_controllers.get(player_slot) else {
            return false;
        };
        if controller.action_stack.last().copied() != Some(3) {
            return false;
        }
        let Some(island_id) = controller.island_search_selected_island_id else {
            return false;
        };
        let Some(map) = self
            .island_maps
            .iter()
            .find(|map| map.island_id == island_id)
        else {
            return false;
        };

        let rectangles = controller.source_city_rectangles.clone();
        let area_threshold = controller.island_search_area_threshold;
        let desired_figure_count = controller.desired_figure_count;
        let selected_figure = controller
            .action_figure_handle
            .or_else(|| controller.owned_figure_handles.first().copied());
        self.source_player_controllers[player_slot].action_stack.pop();

        let mut rectangle = None;
        let mut largest_area = 0_u32;
        for candidate in rectangles {
            if largest_area < candidate.area {
                largest_area = candidate.area;
                rectangle = Some(candidate);
            }
        }
        let Some(rectangle) = rectangle else {
            self.source_player_controllers[player_slot].action_stack.push(3);
            return false;
        };
        let rounded_quarter_threshold =
            (area_threshold.wrapping_add((area_threshold >> 31) & 3)) >> 2;
        if i64::from(rounded_quarter_threshold) >= i64::from(rectangle.area) {
            self.source_player_controllers[player_slot].action_stack.push(3);
            return false;
        }

        let faction_state = self
            .source_kind4_dispatch
            .faction_states
            .get(player_slot)
            .copied()
            .unwrap_or(u8::MAX);
        let minimum_segment_length = if faction_state == 0x0c { 4 } else { 3 };
        let long_line_mode = self.source_controller_city_has_large_population(player_slot)
            || desired_figure_count > 1;
        let segments = map.source_controller_city_candidate_segments(
            7,
            i32::from(rectangle.x0) - 1,
            i32::from(rectangle.y0) - 1,
            i32::from(rectangle.x1) + 1,
            i32::from(rectangle.y1) + 1,
            minimum_segment_length,
            minimum_segment_length,
        );
        let mut selected = None;
        let mut best_green_density = 0_u32;
        for segment in segments {
            let (x, y) = if segment.x0 == segment.x1 {
                (segment.x0, segment.y0 + 1)
            } else {
                (segment.x0 + 1, segment.y0)
            };
            if !map.source_controller_city_boundary_clear(x, y) {
                continue;
            }
            let Some(direction) = map.source_controller_city_map_direction(x, y) else {
                continue;
            };
            let line_distance =
                map.source_controller_city_line_distance(7, x, y, direction.wrapping_sub(1) & 3);
            let green_density = map.source_controller_city_green_density(7, x, y, 0x20);
            let ware_five_density = map.source_controller_city_ware_five_density(7, x, y, 0x0c);
            let eligible = if long_line_mode {
                line_distance > 12
            } else {
                line_distance > 21 && ware_five_density > 60
            };
            if eligible && best_green_density < green_density {
                selected = Some((x, y, direction));
                best_green_density = green_density;
            }
        }
        let Some((source_x, source_y, direction)) = selected else {
            self.source_player_controllers[player_slot].action_stack.push(3);
            return false;
        };
        let Some(figure_handle) = selected_figure else {
            return false;
        };

        let (target_x, target_y) = match direction {
            1 => (source_x + 1, source_y),
            2 => (source_x, source_y + 1),
            3 => (source_x - 2, source_y),
            _ => (source_x, source_y - 2),
        };
        let target_island_id = self.source_player_controllers[player_slot].island_search_cursor;
        let figure_target = SourceTargetDescriptor::from_source_kind34_island_cell(
            target_island_id,
            source_x as u8,
            source_y as u8,
        );
        if !self.set_source_controller_figure_target_descriptor(figure_handle, figure_target) {
            return false;
        }
        let controller = &mut self.source_player_controllers[player_slot];
        controller.action_figure_handle = Some(figure_handle);
        controller.action_source_candidate_tile = Some((source_x, source_y));
        controller.action_target_direction = Some(direction);
        controller.action_arrival_retries = 4;
        controller.action_target_tile = Some((target_x as u8, target_y as u8));
        controller.action_target_island_id = Some(target_island_id);
        if !controller.action_stack.contains(&4) {
            controller.action_stack.push(4);
        }
        true
    }

    /// Replay the arrival/retry branch of `FUN_00417690`. State four first
    /// resolves its kind-`0x34` candidate against the selected figure's live
    /// position. An approaching figure retains the target descriptor,
    /// consumes one `+0x1da8` retry, and requeues state four;
    /// allocation is reachable only after the source proximity predicate
    /// succeeds.
    pub fn advance_source_controller_city_arrival(&mut self, player_slot: usize) -> bool {
        let Some(controller) = self.source_player_controllers.get(player_slot) else {
            return false;
        };
        if controller.action_stack.last().copied() != Some(4) {
            return false;
        }
        let (Some(figure_handle), Some((candidate_x, candidate_y))) = (
            controller.action_figure_handle,
            controller.action_source_candidate_tile,
        ) else {
            self.clear_source_controller_city_arrival(player_slot);
            return false;
        };
        let descriptor = SourceTargetDescriptor::from_source_kind34_island_cell(
            controller.island_search_cursor,
            candidate_x as u8,
            candidate_y as u8,
        );
        let Some(figure_index) = self.source_controller_dynamic_figure_index(figure_handle) else {
            self.clear_source_controller_city_arrival(player_slot);
            return false;
        };
        let figure = &self.source_dynamic_combat_figures[figure_index];
        let Some(position) = (figure.position.0.is_finite()
            && figure.position.0 >= i32::MIN as f32
            && figure.position.0 < i32::MAX as f32
            && figure.position.1.is_finite()
            && figure.position.1 >= i32::MIN as f32
            && figure.position.1 < i32::MAX as f32)
            .then_some((figure.position.0 as i32, figure.position.1 as i32))
        else {
            self.clear_source_controller_city_arrival(player_slot);
            return false;
        };
        let reached = self
            .island_maps
            .iter()
            .find(|map| map.island_id == descriptor.bytes()[1])
            .is_some_and(|map| map.source_controller_city_target_reached(descriptor, position));
        if reached {
            return self.complete_source_controller_city_arrival(player_slot);
        }

        let retries = self.source_player_controllers[player_slot].action_arrival_retries;
        if retries == 0 {
            self.clear_source_controller_city_arrival(player_slot);
            return false;
        }

        self.source_player_controllers[player_slot]
            .action_stack
            .pop();
        self.source_player_controllers[player_slot].action_arrival_retries = retries - 1;
        if !self.set_source_controller_figure_target_descriptor(figure_handle, descriptor) {
            self.clear_source_controller_city_arrival(player_slot);
            return false;
        }
        self.source_player_controllers[player_slot]
            .action_stack
            .push(4);
        true
    }

    fn clear_source_controller_city_arrival(&mut self, player_slot: usize) {
        let controller = &mut self.source_player_controllers[player_slot];
        if controller.action_stack.last().copied() == Some(4) {
            controller.action_stack.pop();
        }
        controller.action_figure_handle = None;
        controller.action_target_island_id = None;
        controller.action_target_tile = None;
        controller.action_source_candidate_tile = None;
        controller.action_target_direction = None;
        controller.action_arrival_retries = 0;
    }

    /// Replay `FUN_00417c80`: consume state seven, rebuild the controller's
    /// construction-work queue from its newly allocated city, and request
    /// source action six only when no work record survives `FUN_00417aa0`.
    pub fn advance_source_controller_city_construction(&mut self, player_slot: usize) -> bool {
        let Some(controller) = self.source_player_controllers.get(player_slot) else {
            return false;
        };
        if controller.action_stack.last().copied() != Some(7) {
            return false;
        }

        let city_slot = controller
            .city_management_profile
            .map(|profile| profile.city_slot);
        self.source_player_controllers[player_slot]
            .action_stack
            .pop();
        self.source_player_controllers[player_slot].source_city_construction_queue =
            SourceControllerCityConstructionQueue::default();

        let calls = city_slot
            .and_then(|slot| self.source_cities.record(slot))
            .and_then(|city| {
                self.island_maps
                    .iter()
                    .find(|map| map.island_id == city.island_id)
                    .map(|map| {
                        map.source_controller_city_construction_work_calls(city.source_owner)
                    })
            })
            .unwrap_or_default();
        let controller = &mut self.source_player_controllers[player_slot];
        for bytes in calls {
            let _ = controller
                .source_city_construction_queue
                .insert(SourceControllerCityConstructionWork::from_bytes(bytes));
        }
        if controller
            .source_city_construction_queue
            .entries()
            .is_empty()
            && controller.action_budget > 0
            && !controller.action_stack.contains(&6)
        {
            controller.action_stack.push(6);
        }
        true
    }

    /// Recovered action cases from `FUN_00429aa0` with local source inputs.
    fn dispatch_source_controller_action(&mut self, player_slot: usize) {
        match self.source_player_controllers[player_slot]
            .action_stack
            .last()
            .copied()
        {
            Some(2) => {
                let _ = self.advance_source_controller_city_area_search(player_slot);
            }
            Some(3) => {
                let _ = self.advance_source_controller_city_candidate_search(player_slot);
            }
            Some(4) => {
                let _ = self.advance_source_controller_city_arrival(player_slot);
            }
            Some(7) => {
                let _ = self.advance_source_controller_city_construction(player_slot);
            }
            _ => {}
        }
    }

    /// Complete the successful `FUN_00417690` state-four arrival. State three
    /// has already selected the owned figure and target tile; this routine
    /// replays the resulting `FUN_004084d0` city allocation, records the
    /// per-city `+0x10638` profile, raises `+0x18`, restores the action budget,
    /// and appends the source's state-eight then state-seven follow-up actions
    /// without duplicating an existing stack entry. Its common cleanup clears
    /// the completed state-four target fields after those follow-up actions
    /// are queued.
    pub fn complete_source_controller_city_arrival(&mut self, player_slot: usize) -> bool {
        let Some(controller) = self.source_player_controllers.get(player_slot) else {
            return false;
        };
        if controller.action_stack.last().copied() != Some(4) {
            return false;
        }
        let (Some(figure_handle), Some(island_id), Some((tile_x, tile_y))) = (
            controller.action_figure_handle,
            controller.action_target_island_id,
            controller.action_target_tile,
        ) else {
            return false;
        };
        let Some(entity) = self.source_shared_figure_entity(figure_handle) else {
            return false;
        };
        if self.source_shared_figure_owner(entity) != player_slot as u8 {
            return false;
        }

        // `FUN_00417690` first consumes state four once its selected figure
        // is live, even when a later city lookup rejects the arrival.
        self.source_player_controllers[player_slot]
            .action_stack
            .pop();
        let Some(city_slot) = self.source_cities.allocate_source_city(
            island_id,
            tile_x,
            tile_y,
            player_slot as u8,
            self.source_time_ticks,
        ) else {
            let controller = &mut self.source_player_controllers[player_slot];
            if !controller.action_stack.contains(&3) {
                controller.action_stack.push(3);
            }
            return false;
        };

        let controller = &mut self.source_player_controllers[player_slot];
        controller.desired_figure_count = controller.desired_figure_count.wrapping_add(1);
        controller.city_management_profile = Some(SourceCityManagementProfile {
            city_slot,
            initialized_at_ticks: self.source_time_ticks,
        });
        controller.action_budget = 14;
        for action in [8, 7] {
            if !controller.action_stack.contains(&action) {
                controller.action_stack.push(action);
            }
        }
        self.clear_source_controller_city_arrival(player_slot);
        true
    }

    fn tick_source_player_controller_city_management(
        &mut self,
        player_slot: usize,
    ) -> Option<SourceFigurePurchaseEvent> {
        let controller = &self.source_player_controllers[player_slot];
        if controller.city_management_disabled
            || controller.city_management_profile.is_none()
            || controller.desired_figure_count == 0
            || !self.source_controller_active_city_owned_by(player_slot)
            || controller.selected_city_active
        {
            return None;
        }

        let action_top = controller.action_stack.last().copied().unwrap_or_default();
        if action_top <= 8
            || controller.purchase_predecessor_issued
            || !controller.owned_figure_handles.is_empty()
        {
            return None;
        }

        self.refresh_source_player_controller_figure_capacity(player_slot);
        let figure_capacity = self.source_player_controllers[player_slot].figure_capacity;
        self.source_controller_purchase_figure(player_slot as u8, 0, figure_capacity)
    }

    /// Replay the scheduling portion of `FUN_0042b4b0`. The source decrements
    /// all active-controller timers first, then advances its physical player
    /// cursor until a pass performs no controller work. Only the recovered
    /// `FUN_00429aa0` action-two/action-three/action-four cases and the
    /// `FUN_00424bf0` -> `FUN_00422150` -> `FUN_00422030` purchase branch are
    /// executed here; other action-stack handlers retain their own paths.
    fn tick_source_player_controllers(&mut self, dt_ms: u32) {
        if !self.source_kind4_dispatch.remote_owner_dispatch_enabled {
            return;
        }

        for player_slot in 0..combat::SOURCE_KIND4_PLAYER_SLOT_COUNT {
            self.initialize_source_player_controller(player_slot);
            let faction_state = self.source_kind4_dispatch.faction_states[player_slot];
            if !matches!(faction_state, 0x0c | 0x0e) {
                continue;
            }
            let controller = &mut self.source_player_controllers[player_slot];
            if controller.action_timer_ms > -50 {
                controller.action_timer_ms =
                    controller.action_timer_ms.saturating_sub(dt_ms as i32);
            }
            if controller.city_management_timer_ms >= 0 {
                controller.city_management_timer_ms = controller
                    .city_management_timer_ms
                    .saturating_sub(dt_ms as i32);
            }
            if controller.maintenance_timer_ms >= 0 {
                controller.maintenance_timer_ms =
                    controller.maintenance_timer_ms.saturating_sub(dt_ms as i32);
            }
        }

        let mut continue_scheduler = true;
        while continue_scheduler {
            let player_slot = usize::from(self.source_player_controller_cursor);
            let faction_state = self.source_kind4_dispatch.faction_states[player_slot];
            let mut no_controller_work = true;

            if faction_state == 0x0c {
                if self.source_player_controllers[player_slot].figure_roster_dirty {
                    self.refresh_source_player_controller_roster(player_slot);
                }
                if self.source_player_controllers[player_slot].action_timer_ms < 1 {
                    self.source_player_controllers[player_slot].action_timer_ms = self
                        .source_player_controllers[player_slot]
                        .action_timer_ms
                        .saturating_add(50);
                    self.dispatch_source_controller_action(player_slot);
                    no_controller_work = false;
                }
                if self.source_player_controllers[player_slot]
                    .city_management_profile
                    .is_some()
                    && self.source_player_controllers[player_slot].city_management_timer_ms < 0
                {
                    let _ = self.tick_source_player_controller_city_management(player_slot);
                    self.source_player_controllers[player_slot].city_management_timer_ms = self
                        .source_player_controllers[player_slot]
                        .city_management_timer_ms
                        .saturating_add(10_000);
                    no_controller_work = false;
                }
            }

            self.source_player_controller_cursor =
                self.source_player_controller_cursor.wrapping_add(1);
            if usize::from(self.source_player_controller_cursor)
                == combat::SOURCE_KIND4_PLAYER_SLOT_COUNT
            {
                self.source_player_controller_cursor = 0;
                return;
            }
            continue_scheduler = !no_controller_work;
        }
    }

    fn tick_events(&mut self) {
        self.tick_pirate_event();
    }

    fn tick_pirate_event(&mut self) {
        // Pirate event spawning needs the decoded source scheduler and target
        // selection. Scenario-loaded pirate ships and player surrender still
        // populate the pirate faction; do not fabricate new ships from an
        // implementation-only random gate.
    }

    /// Main simulation tick, called with real-time delta in milliseconds.
    pub fn tick(&mut self, real_dt_ms: u32) {
        if self.paused {
            return;
        }

        // Scale by game speed.
        let mut remaining = crate::fidelity::scaled_sim_ms(real_dt_ms, self.speed_multiplier);

        // Process in chunks of MAX_STEP_MS
        while remaining > 0 {
            let dt = remaining.min(crate::fidelity::MAX_STEP_MS);
            remaining -= dt;

            self.step(dt);
        }

        // Advance game clock
        self.clock_frac_ms += real_dt_ms * self.speed_multiplier;
        while self.clock_frac_ms >= 100 {
            self.clock_frac_ms -= 100;
            self.game_clock += 1;
        }

        // Auto-save check
        self.autosave_timer_ms += real_dt_ms;
    }

    /// Single simulation step (max 200ms).
    fn step(&mut self, dt_ms: u32) {
        self.advance_source_clock(dt_ms);
        self.tick_source_resource_environment(dt_ms);
        self.tick_source_map_dispatch(dt_ms);
        self.tick_source_kind4_deferred_hits();
        self.tick_source_kind6_deferred_hits();
        self.tick_source_kind4_deferred_relocations();
        self.tick_source_kind14_combat_figures(dt_ms);
        self.tick_source_kind15_combat_figures(dt_ms);
        self.tick_source_dynamic_combat_motion(dt_ms);
        self.tick_source_combat_terminal_slices(dt_ms);

        // 1. Building production
        if self.timer_production.advance(dt_ms) {
            self.tick_production();
        }

        // 2. Population/economy
        if self.timer_population.advance(dt_ms) {
            self.tick_population();
        }

        // 3. Diplomacy
        if self.timer_diplomacy.advance(dt_ms) {
            self.tick_diplomacy();
        }

        // 5. Marketplace coverage
        if self.timer_market.advance(dt_ms) {
            self.tick_market_coverage();
        }

        // 6. Ships
        if self.timer_ships.advance(dt_ms) {
            self.tick_ships();
        }

        // 7. Source-triggered event hooks.
        if self.timer_events.advance(dt_ms) {
            self.tick_events();
        }

        self.tick_source_city_dispatch(dt_ms);
        self.tick_source_kind13_dispatch(dt_ms);
        self.tick_source_player_controllers(dt_ms);

        // Entity movement (every step)
        self.tick_entities(dt_ms);
        self.tick_source_land_figures(dt_ms);
    }

    /// Advance the generic kind-15 visual records installed by
    /// `FUN_00447f00`. The source removal path clears their live slot, so a
    /// completed record is removed from this live-only collection.
    fn tick_source_kind15_combat_figures(&mut self, dt_ms: u32) {
        self.source_kind15_combat_figures
            .retain_mut(|figure| !combat::advance_source_kind15_figure(figure, dt_ms));
    }

    /// Advance generic kind-14 launch records created by `FUN_00447e90`.
    fn tick_source_kind14_combat_figures(&mut self, dt_ms: u32) {
        self.source_kind14_combat_figures
            .retain_mut(|figure| !combat::advance_source_kind14_figure(figure, dt_ms));
    }

    /// Apply the category-six branch of `FUN_00445930`: target category six
    /// redirects through its static map descriptor before the source damage
    /// accumulator reaches the terminal command root.
    fn apply_source_kind6_static_map_hit(
        &mut self,
        descriptor: SourceTargetDescriptor,
        raw_strength: u16,
    ) {
        let Some(target) = self.source_kind6_static_map_target(descriptor) else {
            return;
        };
        let terminal_root = self
            .source_static_map_roots
            .iter_mut()
            .find(|state| {
                state.matches(
                    target.bytes()[1],
                    u16::from(target.bytes()[2]),
                    u16::from(target.bytes()[3]),
                )
            })
            .and_then(|state| {
                state
                    .apply_source_kind6_map_hit(raw_strength)
                    .then_some(*state)
            });
        if let Some(root) = terminal_root {
            self.apply_source_kind6_terminal_map_command(root);
        }
    }

    /// Drain category-4 type-one records through `FUN_00445930`'s resolved
    /// figure-table cases. Categories one through four lose their stored
    /// source energy; category six redirects into the source map-hit branch.
    fn tick_source_kind4_deferred_hits(&mut self) {
        let queued_kind13_hits = std::mem::take(&mut self.source_kind13_deferred_hits);
        for hit in queued_kind13_hits {
            if hit.due_at > self.source_time_ticks {
                self.source_kind13_deferred_hits.push(hit);
                continue;
            }
            self.source_kind4_deferred_hits
                .push(combat::SourceKind4DeferredHit {
                    due_at: hit.due_at,
                    action: combat::SourceKind4Action {
                        attacker_position: hit.action.attacker_position,
                        attacker_runtime_slot: hit.action.attacker_runtime_slot,
                        raw_strength: hit.action.raw_strength,
                        attacker_figure_kind: hit.action.attacker_figure_kind,
                        direction: hit.action.direction,
                        flags: hit.action.flags,
                        target_descriptor: hit.action.target_descriptor,
                    },
                });
        }
        let queued_hits = std::mem::take(&mut self.source_kind4_deferred_hits);
        for hit in queued_hits {
            if hit.due_at > self.source_time_ticks {
                self.source_kind4_deferred_hits.push(hit);
                continue;
            }
            if hit.action.target_descriptor.kind() == 6 {
                self.apply_source_kind6_static_map_hit(
                    hit.action.target_descriptor,
                    hit.action.raw_strength,
                );
                continue;
            }

            let candidates = self.source_combat_candidates();
            let Some(target) = candidates.iter().copied().find(|candidate| {
                candidate.source_kind6_action_target_descriptor()
                    == Some(hit.action.target_descriptor)
            }) else {
                continue;
            };
            if !(1..=4).contains(&target.figure_kind) {
                continue;
            }
            let attacker_owner = candidates
                .iter()
                .find(|candidate| {
                    candidate.figure_kind == hit.action.attacker_figure_kind
                        && candidate.runtime_slot == hit.action.attacker_runtime_slot
                })
                .map(|candidate| candidate.owner);
            let terminal_control_allowed = attacker_owner.is_some_and(|owner| {
                self.source_kind4_dispatch
                    .allows_terminal_figure_control(owner)
            });
            let damage = hit.action.raw_strength;
            let mut terminal_military_unit = None;
            let mut terminal_slice_target = None;
            let mut terminal_slice_motion = None;
            let mut terminal = false;
            match target.entity {
                combat::SourceCombatCandidateEntity::MilitaryUnit(index) => {
                    let Some(unit) = self.military_units.get_mut(index) else {
                        continue;
                    };
                    if damage >= unit.source_energy && terminal_control_allowed {
                        terminal = true;
                        terminal_military_unit = Some(index);
                    } else if damage < unit.source_energy {
                        unit.source_energy -= damage;
                    }
                }
                combat::SourceCombatCandidateEntity::TradeShip(index) => {
                    let Some(ship) = self.trade_ships.get_mut(index) else {
                        continue;
                    };
                    if damage >= ship.source_energy && terminal_control_allowed {
                        terminal = true;
                        if (1..=3).contains(&target.figure_kind) {
                            terminal_slice_target =
                                Some(SourceCombatTerminalSliceTarget::TradeShip(index));
                            terminal_slice_motion = Some(combat::SourceGenericMotion::default());
                        } else {
                            ship.active = false;
                        }
                    } else if damage < ship.source_energy {
                        ship.source_energy -= damage;
                    }
                }
                combat::SourceCombatCandidateEntity::DynamicFigure(index) => {
                    let Some(figure) = self.source_dynamic_combat_figures.get_mut(index) else {
                        continue;
                    };
                    if damage >= figure.source_energy && terminal_control_allowed {
                        terminal = true;
                        if (1..=3).contains(&target.figure_kind) {
                            terminal_slice_target =
                                Some(SourceCombatTerminalSliceTarget::DynamicFigure(index));
                            let motion = figure
                                .source_motion
                                .terminal_remainder(target.figure_kind, figure.direction);
                            figure.source_motion = motion;
                            terminal_slice_motion = Some(motion);
                        } else {
                            figure.active = false;
                        }
                    } else if damage < figure.source_energy {
                        figure.source_energy -= damage;
                    }
                }
            }
            if terminal {
                if let Some(index) = terminal_military_unit {
                    if let Some(unit) = self.military_units.get_mut(index) {
                        unit.source_terminal_pending = true;
                        unit.source_terminal_remaining =
                            combat::source_terminal_motion_slice_remaining(
                                target.figure_kind,
                                unit.direction,
                                unit.source_step_remaining,
                                unit.source_motion_target.is_some(),
                            );
                    }
                }
                if let (Some(slice_target), Some(motion)) =
                    (terminal_slice_target, terminal_slice_motion)
                {
                    self.source_combat_terminal_slices
                        .push(SourceCombatTerminalSlice {
                            target: slice_target,
                            target_figure_kind: target.figure_kind,
                            target_runtime_slot: target.runtime_slot,
                            remaining_distance: motion.remaining_distance,
                            scalar_speed: motion.scalar_speed,
                            velocity_x: motion.velocity_x,
                            velocity_y: motion.velocity_y,
                            velocity_z: motion.velocity_z,
                        });
                }
                self.source_combat_terminal_events
                    .push(SourceCombatTerminalEvent {
                        target: hit.action.target_descriptor,
                        target_figure_kind: target.figure_kind,
                        target_runtime_slot: target.runtime_slot,
                        target_owner: target.owner,
                        attacker_figure_kind: hit.action.attacker_figure_kind,
                        attacker_runtime_slot: hit.action.attacker_runtime_slot,
                        attacker_owner,
                        control_kind: SOURCE_COMBAT_TERMINAL_CONTROL_KIND,
                        kill_credit: true,
                    });
            }
        }
    }

    /// Consume terminal slices initialized by the shared category-one through
    /// -three loader. `FUN_00451890` uses `dt × 0.05 × scalar_speed` and
    /// removes the record only after the remaining amount is exhausted.
    fn tick_source_combat_terminal_slices(&mut self, dt_ms: u32) {
        let mut completed = Vec::new();
        let mut advances = Vec::new();
        self.source_combat_terminal_slices.retain_mut(|slice| {
            let elapsed = dt_ms as f32 * combat::SOURCE_GENERIC_FIGURE_TIME_SCALE;
            let motion_time = if slice.scalar_speed > 0.0 {
                elapsed.min(slice.remaining_distance / slice.scalar_speed)
            } else {
                0.0
            };
            let consumed = motion_time * slice.scalar_speed;
            advances.push((
                slice.target,
                motion_time,
                slice.velocity_x,
                slice.velocity_y,
                slice.velocity_z,
            ));
            if slice.remaining_distance <= consumed {
                completed.push(slice.target);
                false
            } else {
                slice.remaining_distance -= consumed;
                true
            }
        });
        for (target, motion_time, velocity_x, velocity_y, velocity_z) in advances {
            if let SourceCombatTerminalSliceTarget::DynamicFigure(index) = target {
                if let Some(figure) = self.source_dynamic_combat_figures.get_mut(index) {
                    figure.position.0 += motion_time * velocity_x;
                    figure.position.1 += motion_time * velocity_y;
                    figure.position_z += motion_time * velocity_z;
                    figure.source_motion.remaining_distance =
                        (figure.source_motion.remaining_distance
                            - motion_time * figure.source_motion.scalar_speed)
                            .max(0.0);
                }
            }
        }
        for target in completed {
            match target {
                SourceCombatTerminalSliceTarget::TradeShip(index) => {
                    if let Some(ship) = self.trade_ships.get_mut(index) {
                        ship.active = false;
                    }
                }
                SourceCombatTerminalSliceTarget::DynamicFigure(index) => {
                    if let Some(figure) = self.source_dynamic_combat_figures.get_mut(index) {
                        figure.active = false;
                    }
                }
            }
        }
    }

    /// Execute `FUN_00451890`'s generic motion prefix for nonterminal dynamic
    /// records. The category-one through -three zero-distance controller is
    /// reconstructed separately by `FUN_00452370`.
    fn tick_source_dynamic_combat_motion(&mut self, dt_ms: u32) {
        let island_bounds = self
            .island_maps
            .iter()
            .map(|map| {
                (
                    map.island_id,
                    map.source_world_origin,
                    map.width,
                    map.height,
                )
            })
            .collect::<Vec<_>>();
        let mut controller_runtime_slots = Vec::new();
        let mut kind6_controller_runtime_slots = Vec::new();
        for (index, figure) in self.source_dynamic_combat_figures.iter_mut().enumerate() {
            let terminal_pending = self
                .source_combat_terminal_slices
                .iter()
                .any(|slice| slice.target == SourceCombatTerminalSliceTarget::DynamicFigure(index));
            if !figure.active || terminal_pending {
                continue;
            }
            let controller_due = combat::advance_source_generic_motion(
                &mut figure.source_motion,
                &mut figure.position,
                &mut figure.position_z,
                dt_ms,
            );
            if controller_due && (1..=3).contains(&figure.figure_kind) {
                let source_world = (
                    figure.position.0.trunc() as i32,
                    figure.position.1.trunc() as i32,
                );
                figure.position = (source_world.0 as f32 + 0.5, source_world.1 as f32 + 0.5);
                figure.candidate_list_key = island_bounds
                    .iter()
                    .find(|(_, origin, width, height)| {
                        let max_x = origin.0 + i32::from(*width) * 2;
                        let max_y = origin.1 + i32::from(*height) * 2;
                        source_world.0 >= origin.0 - 5
                            && source_world.0 < max_x + 5
                            && source_world.1 >= origin.1 - 5
                            && source_world.1 < max_y + 5
                    })
                    .map_or(0xff, |(island_id, _, _, _)| *island_id);
                controller_runtime_slots.push(figure.runtime_slot);
            } else if controller_due && figure.figure_kind == 6 {
                kind6_controller_runtime_slots.push(figure.runtime_slot);
            }
        }

        for runtime_slot in controller_runtime_slots {
            if self.advance_source_dynamic_descriptor_route(runtime_slot) {
                continue;
            }
            let Some(selected) = self.source_kind13_selected_target(runtime_slot) else {
                continue;
            };
            let turned = {
                let Some(figure) = self
                    .source_dynamic_combat_figures
                    .iter_mut()
                    .find(|figure| {
                        (1..=3).contains(&figure.figure_kind) && figure.runtime_slot == runtime_slot
                    })
                else {
                    continue;
                };
                if let Some(next_direction) = combat::begin_source_combat_turn(
                    figure.direction,
                    selected.direction,
                    &mut figure.source_motion,
                ) {
                    figure.direction = next_direction;
                    true
                } else {
                    false
                }
            };
            if !turned {
                self.dispatch_source_kind13_action(runtime_slot, selected);
            }
        }

        for runtime_slot in kind6_controller_runtime_slots {
            self.tick_source_kind6_controller(runtime_slot);
        }
    }

    /// Execute the controller-due category-six branch of `FUN_00451890`.
    /// `FUN_00458ac0` restores its one-unit dwell motion on every exit,
    /// including owner-gated and targetless invocations.
    fn tick_source_kind6_controller(&mut self, runtime_slot: u16) {
        let Some(owner) = self
            .source_dynamic_combat_figures
            .iter()
            .find(|figure| {
                figure.active && figure.figure_kind == 6 && figure.runtime_slot == runtime_slot
            })
            .map(|figure| figure.owner)
        else {
            return;
        };
        let dispatch = self.source_kind4_dispatch;
        let owner_state = dispatch
            .faction_states
            .get(usize::from(owner))
            .copied()
            .unwrap_or(u8::MAX);
        let _ = self.dispatch_source_kind6_action(
            runtime_slot,
            dispatch.active_player_slot,
            dispatch.remote_owner_dispatch_enabled,
            owner_state,
        );
    }

    /// Drain due category-six kind-one records. Their category-six target
    /// descriptors always use the static-map branch of `FUN_00445930`.
    fn tick_source_kind6_deferred_hits(&mut self) {
        let queued_hits = std::mem::take(&mut self.source_kind6_deferred_hits);
        for hit in queued_hits {
            if hit.due_at > self.source_time_ticks {
                self.source_kind6_deferred_hits.push(hit);
                continue;
            }
            self.apply_source_kind6_static_map_hit(
                hit.action.target_descriptor,
                hit.action.raw_strength,
            );
        }
    }

    /// Drain type-three `FUN_00478ab0` entries. The source allocates a fresh
    /// category-4 runtime slot when it consumes the record; a full category
    /// table discards this spawn after the source figure was already removed.
    fn tick_source_kind4_deferred_relocations(&mut self) {
        let queued = std::mem::take(&mut self.source_kind4_deferred_relocations);
        for relocation in queued {
            if relocation.due_at > self.source_time_ticks {
                self.source_kind4_deferred_relocations.push(relocation);
                continue;
            }
            if !self.source_figure_pool_has_capacity() {
                continue;
            }
            let Some(runtime_slot) = self.allocate_source_dynamic_kind4_slot() else {
                continue;
            };
            let Some(unit_type) = combat::source_kind4_unit_type(relocation.figure_definition_id)
            else {
                continue;
            };
            let origin_x = ((relocation.origin.0 - 0.25) * 2.0).round() as i32;
            let origin_y = ((relocation.origin.1 - 0.25) * 2.0).round() as i32;
            let origin_descriptor = SourceTargetDescriptor::from_bytes([
                SourceTargetDescriptor::FIXED_POINT_COORDINATE_KIND,
                (((origin_y >> 8) as u8 & 0x0f) << 4) | ((origin_x >> 8) as u8 & 0x0f),
                origin_x as u8,
                origin_y as u8,
            ]);
            let target = relocation
                .target_descriptor
                .source_land_route_coordinate()
                .unwrap_or((origin_x, origin_y));
            let source_energy =
                anno_formats::szs::LandFigureDefinition::from_id(relocation.figure_definition_id)
                    .expect("mapped source category-4 definition exists")
                    .source_runtime_energy_cap();
            let mut unit = MilitaryUnit::new(unit_type, 0, origin_x, origin_y);
            unit.source_position_x = relocation.origin.0;
            unit.source_position_y = relocation.origin.1;
            unit.source_position_initialized = true;
            unit.source_island_id = Some(relocation.island_id);
            unit.source_runtime_slot = Some(runtime_slot);
            unit.source_live_runtime_slot = Some(runtime_slot);
            unit.source_candidate_list_key = Some(relocation.island_id);
            unit.source_figure_kind = Some(4);
            unit.source_figure_definition_id = Some(relocation.figure_definition_id);
            unit.source_energy = source_energy;
            unit.source_kind6_target_descriptor_payload = Some([
                relocation.target_descriptor.bytes()[2],
                relocation.target_descriptor.bytes()[3],
            ]);
            unit.source_origin_descriptor = Some(origin_descriptor);
            unit.source_target_descriptor = Some(relocation.target_descriptor);
            unit.target_x = target.0;
            unit.target_y = target.1;
            self.military_units.push(unit);
            self.source_kind4_occupants.push(SourceKind4Occupant {
                runtime_slot,
                figure_definition_id: relocation.figure_definition_id,
                route_radius: combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
                route_retry_count: 0,
                route_program: combat::default_source_kind4_route_program(),
                route_program_cursor: 0,
                idle_remaining_bits: 0,
                origin_descriptor,
                position: (origin_x as u16, origin_y as u16),
                island_id: relocation.island_id,
                owner: 0,
                direction: 0,
                animation_state: 0,
                state_selector: 0,
                state_descriptor: relocation.target_descriptor,
                idle_timestamp_ticks: 0,
                state_flags: 0,
                state_payload: [0; 8],
                active: true,
            });
        }
    }

    /// `FUN_00478a60` owns one 150-record pool for every deferred event type,
    /// including category-6 impacts and `FUN_00458100` relocations.
    fn source_deferred_event_count(&self) -> usize {
        self.source_kind6_deferred_hits.len()
            + self.source_kind4_deferred_hits.len()
            + self.source_kind13_deferred_hits.len()
            + self.source_kind4_deferred_relocations.len()
    }

    /// `FUN_00449ca0`: scan the category-4 table for an inactive slot. The
    /// local type-4 military projection and runtime-only category-4 records
    /// occupy the same source table.
    fn allocate_source_dynamic_kind4_slot(&self) -> Option<u16> {
        (0..combat::SOURCE_DYNAMIC_KIND4_SLOT_CAPACITY).find(|&slot| {
            !self
                .military_units
                .iter()
                .any(|unit| unit.active && unit.source_runtime_slot == Some(slot))
                && !self.source_dynamic_combat_figures.iter().any(|figure| {
                    figure.active && figure.figure_kind == 4 && figure.runtime_slot == slot
                })
        })
    }

    /// Replay the event-kind-seven branch of `FUN_0046a630`: its
    /// `FUN_00463f40` consumer removes the command root and rewrites the
    /// oriented footprint with the root's `Ruinenr` replacement or clear.
    fn apply_source_kind6_terminal_map_command(&mut self, root: SourceMapCellState) {
        let target =
            SourceTargetDescriptor::from_source_kind34_island_cell(root.island, root.x, root.y);
        self.source_kind6_terminal_events
            .push(SourceKind6TerminalEvent {
                target,
                event_kind: 7,
            });
        let source_ruin_draws = (0..root.source_kind6_terminal_random_draw_count())
            .map(|_| self.next_source_rand())
            .collect();
        self.tile_clears.push(TileClear {
            island_id: root.island,
            tile_x: u16::from(root.x),
            tile_y: u16::from(root.y),
            width: root.footprint_width,
            height: root.footprint_height,
            source_orientation: root.source_orientation,
            source_variant: root.source_variant,
            source_map_owner_slot: root.source_map_owner_slot,
            ruin_id: root.ruin_id,
            ruin_uses_strand_table: root.ruin_uses_strand_table,
            fallback_strand_cells: root.fallback_strand_cells,
            source_ruin_draws,
        });
        self.remove_source_map_footprint(
            root.island,
            u16::from(root.x),
            u16::from(root.y),
            root.footprint_width,
            root.footprint_height,
        );

        for building in &mut self.buildings {
            if building.active
                && building.island_id == root.island
                && building.tile_x == u16::from(root.x)
                && building.tile_y == u16::from(root.y)
            {
                building.active = false;
                building.health = 0;
                building.source_dynamic_object_slot = None;
            }
        }
    }

    /// Remove one source command root from both the renderer selector table
    /// and the all-static category-6 inventory after a live map command
    /// erases it.
    pub fn remove_source_map_root(&mut self, island: u8, x: u16, y: u16) {
        self.remove_source_map_footprint(island, x, y, 1, 1);
    }

    /// Remove every static cell and selector root whose anchor lies in an
    /// oriented map-write footprint. `FUN_00463f40` applies this replacement
    /// before later category-6 impacts can resolve the old cell definitions.
    pub fn remove_source_map_footprint(
        &mut self,
        island: u8,
        x: u16,
        y: u16,
        width: u8,
        height: u8,
    ) {
        let selector_count = self.source_map_cell_states.len();
        let static_count = self.source_static_map_roots.len();
        let right = x.saturating_add(u16::from(width));
        let bottom = y.saturating_add(u16::from(height));
        let inside = |state: &SourceMapCellState| {
            state.island == island
                && u16::from(state.x) >= x
                && u16::from(state.x) < right
                && u16::from(state.y) >= y
                && u16::from(state.y) < bottom
        };
        self.source_map_cell_states.retain(|state| !inside(state));
        self.source_static_map_roots.retain(|state| !inside(state));
        if self.source_map_cell_states.len() != selector_count
            || self.source_static_map_roots.len() != static_count
        {
            self.source_map_cell_revision = self.source_map_cell_revision.wrapping_add(1);
        }
    }

    /// Replay one source map-command write into the final static-cell table.
    /// Each destination cell retains the compiled command metadata because
    /// `FUN_0047a650` can target any cell in an oriented command footprint.
    pub fn replace_source_static_map_footprint(&mut self, root: SourceMapCellState) {
        let origin_x = u16::from(root.x);
        let origin_y = u16::from(root.y);
        let width = root.footprint_width.max(1);
        let height = root.footprint_height.max(1);
        let right = origin_x.saturating_add(u16::from(width));
        let bottom = origin_y.saturating_add(u16::from(height));
        self.source_static_map_roots.retain(|cell| {
            cell.island != root.island
                || u16::from(cell.x) < origin_x
                || u16::from(cell.x) >= right
                || u16::from(cell.y) < origin_y
                || u16::from(cell.y) >= bottom
        });

        let mut wrote_cell = false;
        for dy in 0..height {
            for dx in 0..width {
                let x = origin_x + u16::from(dx);
                let y = origin_y + u16::from(dy);
                let (Ok(x), Ok(y)) = (u8::try_from(x), u8::try_from(y)) else {
                    continue;
                };
                let mut cell = root;
                cell.x = x;
                cell.y = y;
                cell.source_definition_offset = root.source_definition_offset_at(dx, dy);
                self.source_static_map_roots.push(cell);
                wrote_cell = true;
            }
        }
        if wrote_cell {
            self.source_map_cell_revision = self.source_map_cell_revision.wrapping_add(1);
        }
    }

    /// Materialize a drained terminal ruin command into the static-cell table.
    /// The frozen random draws were consumed when the source event fired, so
    /// this reconstruction never advances the simulation RNG.
    pub fn apply_source_terminal_static_replacement(
        &mut self,
        cod: &anno_formats::cod::CodFile,
        clear: &TileClear,
    ) {
        if clear.ruin_id == crate::building::NO_RUIN_ID {
            self.apply_source_terminal_no_ruin_static_replacement(cod, clear);
            return;
        }
        let Some(base) = cod.ruin_building(clear.ruin_id, clear.ruin_uses_strand_table) else {
            return;
        };
        let base_size = if matches!(clear.source_orientation & 3, 1 | 3) {
            (base.size.1, base.size.0)
        } else {
            base.size
        };
        let mut writes = Vec::new();
        if base_size == (i32::from(clear.width), i32::from(clear.height)) {
            let Some(&draw) = clear.source_ruin_draws.first() else {
                return;
            };
            let Some(definition) =
                cod.ruin_variant_building(clear.ruin_id, clear.ruin_uses_strand_table, draw)
            else {
                return;
            };
            writes.push((clear.tile_x, clear.tile_y, definition));
        } else {
            for dy in 0..clear.height {
                for dx in (0..clear.width).rev() {
                    let index = usize::from(dy) * usize::from(clear.width)
                        + usize::from(clear.width - 1 - dx);
                    let Some(&draw) = clear.source_ruin_draws.get(index) else {
                        continue;
                    };
                    let Some(definition) = cod.ruin_variant_building(
                        clear.ruin_id,
                        clear.fallback_uses_strand_table(index),
                        draw,
                    ) else {
                        continue;
                    };
                    writes.push((
                        clear.tile_x + u16::from(dx),
                        clear.tile_y + u16::from(dy),
                        definition,
                    ));
                }
            }
        }
        for (x, y, definition) in writes {
            let (Ok(x), Ok(y)) = (u8::try_from(x), u8::try_from(y)) else {
                continue;
            };
            let Some(mut root) =
                SourceMapCellState::new_static(clear.island_id, x, y, definition, 0)
            else {
                continue;
            };
            let (width, height) = if matches!(clear.source_orientation & 3, 1 | 3) {
                (definition.size.1, definition.size.0)
            } else {
                definition.size
            };
            root.set_footprint(width, height);
            root.set_source_orientation(clear.source_orientation);
            root.set_terminal_command_fields(clear.source_variant, clear.source_map_owner_slot);
            root.configure_terminal_replacement(cod);
            self.replace_source_static_map_footprint(root);
        }
    }

    /// Replay `FUN_004641d0` when the destroyed definition has
    /// `Ruinenr = 0xff`. The backing map is scanned top-to-bottom and
    /// right-to-left. Non-kind-10 commands are replayed only at their
    /// recovered source anchors; kind 10 becomes fixed definition `0x58c2`.
    fn apply_source_terminal_no_ruin_static_replacement(
        &mut self,
        cod: &anno_formats::cod::CodFile,
        clear: &TileClear,
    ) {
        let right = clear.tile_x.saturating_add(u16::from(clear.width));
        let bottom = clear.tile_y.saturating_add(u16::from(clear.height));
        let mut backing_cells: Vec<_> = self
            .source_static_map_backing_cells
            .iter()
            .copied()
            .filter(|cell| {
                cell.island == clear.island_id
                    && u16::from(cell.x) >= clear.tile_x
                    && u16::from(cell.x) < right
                    && u16::from(cell.y) >= clear.tile_y
                    && u16::from(cell.y) < bottom
            })
            .collect();
        backing_cells.sort_by_key(|cell| (cell.y, std::cmp::Reverse(cell.x)));

        for cell in backing_cells {
            if cell.kind_code == 10 {
                let Some(definition) = cod.building_by_source_id(0x58c2) else {
                    continue;
                };
                let Some(mut root) =
                    SourceMapCellState::new_static(cell.island, cell.x, cell.y, definition, 0)
                else {
                    continue;
                };
                let (width, height) = if matches!(clear.source_orientation & 3, 1 | 3) {
                    (definition.size.1, definition.size.0)
                } else {
                    definition.size
                };
                root.set_footprint(width, height);
                root.set_source_orientation(clear.source_orientation);
                root.set_terminal_command_fields(0, clear.source_map_owner_slot);
                root.configure_terminal_replacement(cod);
                self.replace_source_static_map_footprint(root);
            } else if cell.source_command_anchor_x == cell.x
                && cell.source_command_anchor_y == cell.y
            {
                let mut root = cell;
                root.set_terminal_command_fields(0, clear.source_map_owner_slot);
                root.source_damage_accumulator = 0;
                self.replace_source_static_map_footprint(root);
            }
        }
    }

    /// Resolve the static command-root descriptor consumed by the deferred
    /// source map-hit handler. A category-6 action descriptor identifies a
    /// live target slot whose `+0x10` descriptor is the static root.
    fn source_kind6_static_map_target(
        &self,
        descriptor: SourceTargetDescriptor,
    ) -> Option<SourceTargetDescriptor> {
        let target = match descriptor.kind() {
            6 => {
                let bytes = descriptor.bytes();
                let runtime_slot = u16::from_le_bytes([bytes[2], bytes[3]]);
                self.source_dynamic_combat_figures
                    .iter()
                    .find(|figure| {
                        figure.active
                            && figure.figure_kind == 6
                            && figure.candidate_list_key == bytes[1]
                            && figure.runtime_slot == runtime_slot
                    })?
                    .target_descriptor
            }
            0x32 | 0x33 | 0x34 => descriptor,
            _ => return None,
        };
        matches!(target.kind(), 0x32 | 0x33 | 0x34).then_some(target)
    }

    /// Replay the kind-12 city dispatcher in `FUN_0047f8a0`. Its city pool
    /// traversal is independent of the coarser population subsystem: each
    /// source simulation slice updates exactly two physical city slots.
    fn tick_source_city_dispatch(&mut self, dt_ms: u32) {
        self.source_city_dispatch_elapsed_ms =
            self.source_city_dispatch_elapsed_ms.saturating_add(dt_ms);
        if self.source_city_dispatch_elapsed_ms > 9_999 {
            self.source_city_dispatch_elapsed_ms = 0;
            self.source_city_dispatch_phase = self.source_city_dispatch_phase.wrapping_add(1) & 7;
        }

        for _ in 0..2 {
            let slot = self.source_city_dispatch_cursor;
            self.source_city_dispatch_cursor =
                (self.source_city_dispatch_cursor + 1) % SourceCityTable::slot_count();

            let Some(city) = self.source_cities.record(slot) else {
                continue;
            };
            if city.phase == self.source_city_dispatch_phase {
                continue;
            }
            let city_satisfaction_allowed = self.source_city_satisfaction_allows(city.owner_slot);
            let Some(city) = self.source_cities.record_mut(slot) else {
                continue;
            };
            city.phase = self.source_city_dispatch_phase;
            if city_satisfaction_allowed {
                city.satisfaction_pressure =
                    (u32::from(city.satisfaction_pressure) * 0xff >> 8) as u16;
                city.refresh_group_satisfaction();
            }
            let city = *city;
            if self.source_city_kind12_dispatch_allows(city) {
                self.spawn_source_kind12_figures(city);
            }
        }
    }

    /// Advance `DAT_005a6aec` / `DAT_005a6c12` exactly as the leading block
    /// of `FUN_0047daf0`. Root-local phase and cooldown updates occur in
    /// `tick_production`, where this simulator executes the corresponding
    /// source production and transfer branches.
    fn tick_source_map_dispatch(&mut self, dt_ms: u32) {
        self.source_map_dispatch_elapsed_ms =
            self.source_map_dispatch_elapsed_ms.saturating_add(dt_ms);
        if self.source_map_dispatch_elapsed_ms > 999 {
            self.source_map_dispatch_elapsed_ms = 0;
            self.source_map_dispatch_phase = self.source_map_dispatch_phase.wrapping_add(1) & 7;
        }
    }

    /// Replay the resource-cell portion of `FUN_0046b3e0`. Each invocation
    /// advances one physical island cursor; an island is scanned at most once
    /// per 30-second source phase. Its remaining branches update other
    /// dynamic map state and are outside this resource-cell replay.
    fn tick_source_resource_environment(&mut self, dt_ms: u32) {
        self.source_resource_environment_elapsed_ms = self
            .source_resource_environment_elapsed_ms
            .saturating_add(dt_ms);
        if self.source_resource_environment_elapsed_ms > 29_999 {
            self.source_resource_environment_elapsed_ms = 0;
            self.source_resource_environment_phase =
                self.source_resource_environment_phase.wrapping_add(1) & 7;
        }

        let map_count = self.island_maps.len();
        if map_count == 0 {
            return;
        }
        self.source_resource_environment_last_phase
            .resize(map_count, 0);
        let map_index = self.source_resource_environment_cursor % map_count;
        self.source_resource_environment_cursor = (map_index + 1) % map_count;
        if self.source_resource_environment_last_phase[map_index]
            == self.source_resource_environment_phase
        {
            return;
        }
        self.source_resource_environment_last_phase[map_index] =
            self.source_resource_environment_phase;

        let (island, width, height, deadline, attenuation) = {
            let map = &self.island_maps[map_index];
            (
                map.island_id,
                map.width,
                map.height,
                map.source_resource_transition_deadline_ticks(),
                map.source_resource_attenuation(),
            )
        };
        let mut changed = false;
        if attenuation != 0 && self.source_time_ticks < deadline {
            let resource_state = self.island_maps[map_index].source_resource_state();
            for y in 0..height {
                let mut x = i32::from(width) - i32::from(self.next_source_rand() & 3) - 1;
                while x >= 0 {
                    let x_u8 = x as u8;
                    if let Some(cell) = self
                        .source_static_map_roots
                        .iter_mut()
                        .find(|cell| cell.matches(island, u16::from(x_u8), u16::from(y)))
                    {
                        let transition = source_resource_harvest_transition(
                            resource_state.resource_strength(cell.source_output_ware_slot),
                            cell.source_resource_growth_factor,
                            attenuation,
                            cell.source_output_ware_slot,
                            island,
                            u16::from(x_u8),
                            u16::from(y),
                        );
                        changed |= cell.advance_raw_resource_to_drought(transition);
                    }
                    x -= 3;
                }
            }
        } else if attenuation != 0 {
            let attenuation = self.island_maps[map_index].decay_source_resource_attenuation();
            let resource_state = self.island_maps[map_index].source_resource_state();
            for y in 0..height {
                for x in (0..width).rev() {
                    if let Some(cell) = self
                        .source_static_map_roots
                        .iter_mut()
                        .find(|cell| cell.matches(island, u16::from(x), u16::from(y)))
                    {
                        let transition = source_resource_harvest_transition(
                            resource_state.resource_strength(cell.source_growth_resource_ware_slot),
                            cell.source_resource_growth_factor,
                            attenuation,
                            cell.source_growth_resource_ware_slot,
                            island,
                            u16::from(x),
                            u16::from(y),
                        );
                        changed |= cell.restore_dry_resource(transition);
                    }
                }
            }
        }
        if changed {
            self.source_map_cell_revision = self.source_map_cell_revision.wrapping_add(1);
        }
        self.spawn_due_source_terrain_events(map_index);
        self.source_terrain_event_schedule_counters
            .resize(self.island_maps.len(), 0);
        let counter = self.source_terrain_event_schedule_counters[map_index].wrapping_add(1);
        if counter > 3 {
            self.source_terrain_event_schedule_counters[map_index] = 0;
            self.schedule_source_terrain_events(map_index);
        } else {
            self.source_terrain_event_schedule_counters[map_index] = counter;
        }
    }

    /// Return the final live source map cell at one local terrain position.
    /// The source map array is a final-overwrite surface, so later command
    /// cells shadow earlier records at the same island coordinate.
    fn source_terrain_event_cell(
        &self,
        island_id: u8,
        x: i32,
        y: i32,
    ) -> Option<SourceMapCellState> {
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
            return None;
        };
        self.source_static_map_roots
            .iter()
            .rev()
            .copied()
            .find(|cell| cell.matches(island_id, x, y))
    }

    /// Test the five-cell plus shape read by `FUN_0046b920` and by the due
    /// row branch in `FUN_0046b3e0`. Production-kind-10 cells compare their
    /// adjacent `+0x88` definition's selector through
    /// `plantation_path_resource_ware_slot`.
    fn source_terrain_event_cross(
        &self,
        island_id: u8,
        center: (i32, i32),
    ) -> Option<(SourceMapCellState, u8)> {
        let center_cell = self.source_terrain_event_cell(island_id, center.0, center.1)?;
        let mut grass_cells = 0u8;
        for (x, y) in [
            center,
            (center.0 - 1, center.1),
            (center.0 + 1, center.1),
            (center.0, center.1 - 1),
            (center.0, center.1 + 1),
        ] {
            grass_cells = grass_cells.saturating_add(u8::from(
                self.source_terrain_event_cell(island_id, x, y)
                    .is_some_and(|cell| cell.plantation_path_resource_ware_slot() == 0x35),
            ));
        }
        Some((center_cell, grass_cells))
    }

    /// `FUN_0046b3e0` examines every due row in the currently processed
    /// island's eight-entry terrain-event table. A row remains due when event
    /// allocation is refused, so the next island phase retries it.
    fn spawn_due_source_terrain_events(&mut self, map_index: usize) {
        let Some(map) = self.island_maps.get(map_index) else {
            return;
        };
        let island_id = map.island_id;
        self.source_terrain_event_schedules.resize(
            self.island_maps.len(),
            [SourceTerrainEventSchedule::default(); 8],
        );
        let due_rows: Vec<_> = self.source_terrain_event_schedules[map_index]
            .iter()
            .copied()
            .filter(|row| {
                !row.is_free()
                    && row.island_id == island_id
                    && row.due_at_ticks <= self.source_time_ticks
            })
            .collect();
        for row in due_rows {
            let Some((center, grass_cells)) = self
                .source_terrain_event_cross(row.island_id, (i32::from(row.x), i32::from(row.y)))
            else {
                self.remove_source_terrain_event(row.island_id, row.x, row.y);
                continue;
            };
            // `FUN_0046b3e0` counts only the four neighbours here, then
            // separately requires the centre. `grass_cells` includes the
            // centre, so a surviving due row has at least three grass cells.
            if grass_cells <= 2
                || center.plantation_path_resource_ware_slot() != 0x35
                || center.source_production_kind_code == 10
            {
                self.remove_source_terrain_event(row.island_id, row.x, row.y);
                continue;
            }
            if self.figures.iter().any(|figure| {
                figure.is_active()
                    && figure.source_terrain_event_active
                    && figure.origin_island == row.island_id
                    && figure.origin_x == u16::from(row.x)
                    && figure.origin_y == u16::from(row.y)
            }) {
                continue;
            }
            if let Some(figure) = self.allocate_source_terrain_event(map_index, row) {
                self.figures.push(figure);
            }
        }
    }

    /// `FUN_0046b920`: scan the source's interior two-cell lattice for plus
    /// shapes containing at least four normalized grass selectors. At most
    /// 500 candidates are retained; the source then samples
    /// `floor((count + 5) / 8)` rows, capped at eight, using the physical
    /// prefix of the island's scheduler table in reverse free-slot order.
    fn schedule_source_terrain_events(&mut self, map_index: usize) {
        let (island_id, width, height) = {
            let Some(map) = self.island_maps.get(map_index) else {
                return;
            };
            (map.island_id, i32::from(map.width), i32::from(map.height))
        };
        let mut candidates = Vec::new();
        if width > 2 && height > 2 {
            let mut y = 1;
            while y < height - 1 && candidates.len() < 500 {
                let mut x = width - 2;
                while x > 0 {
                    if self
                        .source_terrain_event_cross(island_id, (x, y))
                        .is_some_and(|(_, grass_cells)| grass_cells > 3)
                    {
                        candidates.push((x as u8, y as u8));
                        if candidates.len() >= 500 {
                            break;
                        }
                    }
                    x -= 2;
                }
                y += 2;
            }
        }
        let selection_count = ((candidates.len() + 5) / 8).min(8);
        if selection_count == 0 {
            return;
        }
        self.source_terrain_event_schedules.resize(
            self.island_maps.len(),
            [SourceTerrainEventSchedule::default(); 8],
        );

        for _ in 0..selection_count {
            let candidate = candidates[usize::from(self.next_source_rand()) % candidates.len()];
            let Some(schedule) = ({
                let rows = &mut self.source_terrain_event_schedules[map_index];
                if rows[..selection_count].iter().any(|row| {
                    row.island_id == island_id && row.x == candidate.0 && row.y == candidate.1
                }) {
                    None
                } else if let Some(slot) = (0..selection_count)
                    .rev()
                    .find(|&slot| rows[slot].is_free())
                {
                    rows[slot] = SourceTerrainEventSchedule {
                        island_id,
                        x: candidate.0,
                        y: candidate.1,
                        due_at_ticks: self.source_time_ticks.wrapping_add(600),
                    };
                    Some(rows[slot])
                } else {
                    None
                }
            }) else {
                continue;
            };
            let allocate_now = self
                .source_terrain_event_cell(
                    island_id,
                    i32::from(candidate.0),
                    i32::from(candidate.1),
                )
                .is_some_and(|cell| cell.source_production_kind_code != 10);
            if allocate_now {
                if let Some(figure) = self.allocate_source_terrain_event(map_index, schedule) {
                    self.figures.push(figure);
                }
            }
        }
    }

    /// `FUN_0044bd00`: allocate a generic type-17 figure for one due terrain
    /// row after the shared source-event table and generic figure pool both
    /// admit it. The first INSEL5 resource selector chooses `FRAU` (`0x5b`)
    /// only when it equals one; all other islands use `ADEL` (`0x59`).
    fn allocate_source_terrain_event(
        &mut self,
        map_index: usize,
        schedule: SourceTerrainEventSchedule,
    ) -> Option<Figure> {
        if !self.source_figure_pool_has_capacity() {
            return None;
        }
        let map = self.island_maps.get(map_index)?;
        if schedule.island_id != map.island_id
            || u16::from(schedule.x) >= map.width
            || u16::from(schedule.y) >= map.height
        {
            return None;
        }
        let local = (i32::from(schedule.x), i32::from(schedule.y));
        let world = (
            map.source_world_origin
                .0
                .div_euclid(2)
                .checked_add(local.0)?,
            map.source_world_origin
                .1
                .div_euclid(2)
                .checked_add(local.1)?,
        );
        let (Ok(event_x), Ok(event_y)) = (i16::try_from(world.0), i16::try_from(world.1)) else {
            return None;
        };
        let definition = if map.source_resource_state().records[0].ware() == 1 {
            0x5b
        } else {
            0x59
        };
        let source_position_z = map.source_terrain_height(local).unwrap_or(0.0);
        let slot = self
            .source_figure_events
            .prepare_terrain_event_if_absent(event_x, event_y)?;
        if !self
            .source_figure_events
            .activate_terrain_event(slot, event_x, event_y)
        {
            return None;
        }

        let mut figure = Figure::new();
        figure.action = ActionType::Walking;
        figure.owner = 7;
        figure.origin_island = schedule.island_id;
        figure.origin_x = u16::from(schedule.x);
        figure.origin_y = u16::from(schedule.y);
        figure.tile_x = local.0;
        figure.tile_y = local.1;
        figure.target_x = local.0;
        figure.target_y = local.1;
        figure.speed = carrier::CARRIER_SPEED;
        figure.source_move_speed = self
            .civilian_config
            .movement_speed_for_definition(definition);
        figure.sprite_set = definition;
        figure.base_sprite = self.civilian_config.sprite_base_for_definition(definition);
        figure.source_position_z = source_position_z;
        figure.initialize_source_position();
        figure.source_event_slot = Some(slot);
        figure.source_terrain_event_active = true;
        Some(figure)
    }

    /// `FUN_0046b2a0`: reset a matching terrain row's retry deadline after a
    /// type-17 terminal cleanup. The source can call this after a failed
    /// route already compacted the row away, in which case it is a no-op.
    fn defer_source_terrain_event(&mut self, island_id: u8, x: u8, y: u8) {
        for rows in &mut self.source_terrain_event_schedules {
            if let Some(row) = rows
                .iter_mut()
                .find(|row| row.island_id == island_id && row.x == x && row.y == y)
            {
                row.due_at_ticks = self.source_time_ticks.wrapping_add(600);
                return;
            }
        }
    }

    /// `FUN_0046b310`: discard a scheduled terrain row by moving the last
    /// occupied physical slot into the removed position, then restoring the
    /// trailing free sentinel.
    fn remove_source_terrain_event(&mut self, island_id: u8, x: u8, y: u8) {
        for rows in &mut self.source_terrain_event_schedules {
            let Some(removed) = rows
                .iter()
                .position(|row| row.island_id == island_id && row.x == x && row.y == y)
            else {
                continue;
            };
            let Some(last) = rows.iter().rposition(|row| !row.is_free()) else {
                return;
            };
            if removed < last {
                rows[removed] = rows[last];
            }
            rows[last] = SourceTerrainEventSchedule::default();
            return;
        }
    }

    /// Execute the city-facing half of `FUN_0047b9c0` for each phase-changed
    /// kind-13 root. The phase state itself is advanced by
    /// [`SourceKind13DispatchState`]; this method supplies the source city's
    /// satisfaction, ordered route neighbors, construction balances, and
    /// deferred INSELHAUS command emission.
    fn tick_source_kind13_dispatch(&mut self, dt_ms: u32) {
        let changed = self
            .source_kind13_dispatch
            .advance_batch(&mut self.source_kind13_locations, dt_ms);
        for location in changed {
            self.apply_source_kind13_dispatch_location(location);
        }
    }

    fn apply_source_kind13_dispatch_location(&mut self, location: SourceKind13Location) {
        let Some(city_slot) = self
            .source_cities
            .slot_for_root(location.island_id, location.source_owner)
        else {
            return;
        };
        let Some(city) = self.source_cities.record(city_slot) else {
            return;
        };
        let city_owner = city.owner_slot;
        let delta = location.source_dispatch_amount_delta(city.source_kind13_transfer_inputs());
        if delta == 0 {
            return;
        }

        let neighbors = self
            .island_maps
            .iter()
            .find(|map| map.island_id == location.island_id)
            .and_then(|map| {
                map.source_kind13_transfer_neighbor_cells((
                    i32::from(location.tile_x),
                    i32::from(location.tile_y),
                ))
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(x, y)| u8::try_from(x).ok().zip(u8::try_from(y).ok()))
            .collect::<Vec<_>>();

        if delta < 0 {
            let Some(city) = self.source_cities.record_mut(city_slot) else {
                return;
            };
            let result = self.source_kind13_locations.apply_source_kind13_decrease(
                city,
                location.island_id,
                location.tile_x,
                location.tile_y,
                delta.unsigned_abs().min(u32::from(u16::MAX)) as u16,
                &neighbors,
            );
            let Some(crate::data_bridge::SourceKind13DecreaseResult::DowngradeRequired {
                target_group,
                ..
            }) = result
            else {
                return;
            };
            let Some(definition) = self
                .source_kind13_promotion_definitions
                .get(usize::from(target_group))
                .and_then(Option::as_ref)
                .cloned()
            else {
                return;
            };
            let Some(updated_location) = self.source_kind13_locations.location_at(
                location.island_id,
                location.tile_x,
                location.tile_y,
            ) else {
                return;
            };
            self.enqueue_source_kind13_replacement(
                updated_location,
                target_group,
                &definition,
                city_owner,
            );
            return;
        }

        let Some(target_group) = location.population_group.checked_add(1) else {
            return;
        };
        let Some(definition) = self
            .source_kind13_promotion_definitions
            .get(usize::from(target_group))
            .and_then(Option::as_ref)
            .cloned()
        else {
            return;
        };
        let materials = self.source_kind13_promotion_materials(city, &definition);
        let Some(city) = self.source_cities.record_mut(city_slot) else {
            return;
        };
        let Some(result) = self.source_kind13_locations.apply_source_kind13_increase(
            city,
            location.island_id,
            location.tile_x,
            location.tile_y,
            delta,
            &neighbors,
            materials,
        ) else {
            return;
        };
        if result.promotion.is_none() {
            return;
        }

        let Some(warehouse) = self.warehouses.iter_mut().find(|warehouse| {
            warehouse.active
                && warehouse.island_id == location.island_id
                && warehouse.owner == city_owner
        }) else {
            return;
        };
        // `FUN_0047b160` commits in this order. The three gated materials
        // are known to be available; cannon withdrawal deliberately takes
        // the source minimum and never blocks the completed promotion.
        warehouse.withdraw_city_good_fixed(Good::Tools, definition.tools_cost_fixed);
        warehouse.withdraw_city_good_fixed(Good::Bricks, definition.bricks_cost_fixed);
        warehouse.withdraw_city_good_fixed(Good::Wood, definition.wood_cost_fixed);
        warehouse.withdraw_city_good_fixed(Good::Cannons, definition.cannons_cost_fixed);
        if let Some(player) = self.players.get_mut(usize::from(city_owner)) {
            player.gold = player.gold.wrapping_sub(definition.money_cost as i32);
        }
        self.enqueue_source_kind13_replacement(location, target_group, &definition, city_owner);
    }

    fn enqueue_source_kind13_replacement(
        &mut self,
        location: SourceKind13Location,
        target_group: u8,
        definition: &SourceKind13PromotionDefinition,
        city_owner: u8,
    ) {
        let orientation = self
            .island_maps
            .iter()
            .find(|map| map.island_id == location.island_id)
            .map(|map| {
                map.source_kind13_replacement_orientation(
                    definition.source_size,
                    location.orientation,
                    (i32::from(location.tile_x), i32::from(location.tile_y)),
                )
            })
            .unwrap_or(location.orientation & 3);
        let variant_random = self.next_source_rand();
        let command_random = self.next_source_rand();
        if let Some(command) = definition.source_promotion_command(
            location,
            orientation,
            variant_random,
            command_random,
            city_owner,
        ) {
            self.source_kind13_replacement_commands
                .push(SourceKind13ReplacementCommand {
                    island_id: location.island_id,
                    tile_x: location.tile_x,
                    tile_y: location.tile_y,
                    target_group,
                    command,
                });
        }
    }

    fn source_kind13_promotion_materials(
        &self,
        city: SourceCityRecord,
        definition: &SourceKind13PromotionDefinition,
    ) -> Option<crate::data_bridge::SourceKind13PromotionMaterials> {
        let warehouse = self.warehouses.iter().find(|warehouse| {
            warehouse.active
                && warehouse.island_id == city.island_id
                && warehouse.owner == city.owner_slot
        })?;
        Some(
            definition.materials(
                warehouse
                    .city_stock_fixed(Good::Tools)
                    .saturating_sub(warehouse.city_reserved_fixed(Good::Tools)),
                warehouse
                    .city_stock_fixed(Good::Wood)
                    .saturating_sub(warehouse.city_reserved_fixed(Good::Wood)),
                warehouse
                    .city_stock_fixed(Good::Bricks)
                    .saturating_sub(warehouse.city_reserved_fixed(Good::Bricks)),
            ),
        )
    }

    /// The outer `FUN_0047f8a0` gate and the seven-player guard at the start
    /// of `FUN_00480370`. `FUN_0047f1f0(city, 1)` sums BGRUPPE 1 through 4,
    /// while `FUN_00486b50` reads the renderer's visible-island table.
    fn source_city_kind12_dispatch_allows(&self, city: SourceCityRecord) -> bool {
        self.source_visible_islands
            .get(usize::from(city.island_id))
            .copied()
            .unwrap_or(false)
            && city.tier_population[1..].iter().copied().sum::<u32>() > 29
            && self.source_city_activity_allows(city)
    }

    /// The seven-player guard at the start of `FUN_00480370`. Source island
    /// bytes `+0x4a..=+0x50` count live kind-4 land figures by owner; an
    /// active human or AI owner other than this city owner blocks the whole
    /// allocator before its fifteen discarded random draws.
    fn source_city_activity_allows(&self, city: SourceCityRecord) -> bool {
        self.players
            .iter()
            .enumerate()
            .filter(|(owner, player)| {
                *owner != usize::from(city.owner_slot)
                    && matches!(
                        player.state,
                        PlayerState::HumanActive | PlayerState::AiActive
                    )
            })
            .all(|(owner, _)| {
                !self.source_kind4_occupants.iter().any(|occupant| {
                    occupant.active
                        && occupant.owner == owner as u8
                        && occupant.island_id == city.island_id
                }) && !self.military_units.iter().any(|unit| {
                    unit.active
                        && unit.owner == owner as u8
                        && unit.source_island_id == Some(city.island_id)
                        && !matches!(
                            unit.unit_type,
                            crate::combat::UnitType::SmallWarship
                                | crate::combat::UnitType::LargeWarship
                                | crate::combat::UnitType::PirateShip
                        )
                })
            })
    }

    /// The fully represented local-owner branch of `FUN_0047f8a0`: after
    /// the city phase write, it updates demand satisfaction only for player
    /// state zero or twelve. Remote-owner dispatch additionally depends on
    /// a separate session global and remains outside this local gate.
    fn source_city_satisfaction_allows(&self, owner_slot: u8) -> bool {
        owner_slot == self.source_kind4_dispatch.active_player_slot
            && self
                .players
                .get(usize::from(owner_slot))
                .is_some_and(|player| {
                    matches!(
                        player.state,
                        PlayerState::HumanActive | PlayerState::AiActive
                    )
                })
    }

    /// Delete one live source kind-4 figure by its `SOLDAT3` runtime slot.
    ///
    /// The source type-4 deletion branch (`FUN_00443520` → `FUN_00443830`)
    /// removes the runtime figure and decrements the matching island-owner
    /// counter. The local combat record and source occupancy record share
    /// this slot, so they must transition together.
    pub fn deactivate_source_kind4_figure(&mut self, runtime_slot: u16) -> bool {
        let mut changed = false;
        for occupant in &mut self.source_kind4_occupants {
            if occupant.runtime_slot == runtime_slot && occupant.active {
                occupant.active = false;
                changed = true;
            }
        }
        for unit in &mut self.military_units {
            if unit.source_runtime_slot == Some(runtime_slot) && unit.active {
                unit.active = false;
                unit.health = 0.0;
                changed = true;
            }
        }
        if changed {
            let alive: Vec<bool> = self
                .military_units
                .iter()
                .map(crate::combat::MilitaryUnit::is_alive)
                .collect();
            for unit in &mut self.military_units {
                if unit.combat_target >= 0
                    && alive
                        .get(unit.combat_target as usize)
                        .is_some_and(|&target_alive| !target_alive)
                {
                    unit.combat_target = -1;
                }
            }
        }
        changed
    }

    /// `FUN_00480370`: consume its discarded rand draws, then keep sampling
    /// physical kind-13 slots until either ten nonmatching samples or one
    /// quarter of the island slice has reached the allocator.
    fn spawn_source_kind12_figures(&mut self, city: SourceCityRecord) {
        for _ in 0..15 {
            self.next_source_rand();
        }

        let locations = self
            .source_kind13_locations
            .city_slice(city.island_id)
            .to_vec();
        let mut nonmatching_samples = 0usize;
        let mut matching_calls = 0usize;
        let matching_limit = locations.len() / 4;
        loop {
            let location_index = usize::from(self.next_source_rand()) % locations.len();
            let Some(location) = locations[location_index] else {
                nonmatching_samples += 1;
                if nonmatching_samples > 9 {
                    return;
                }
                continue;
            };
            if location.island_id != city.island_id || location.source_owner != city.source_owner {
                nonmatching_samples += 1;
                if nonmatching_samples > 9 {
                    return;
                }
                continue;
            }

            let permission_branch = (self.next_source_rand() & 3) as u8;
            if let Some(figure) = self.allocate_source_kind12_figure(location, permission_branch) {
                self.figures.push(figure);
            }
            matching_calls += 1;
            if matching_calls >= matching_limit {
                return;
            }
        }
    }

    /// `FUN_0044b140`: claim the shared source event slot at one kind-13
    /// anchor, initialize a kind-12 figure, then replace its initial
    /// definition only when the threshold callback reaches a route target.
    fn allocate_source_kind12_figure(
        &mut self,
        location: crate::data_bridge::SourceKind13Location,
        permission_branch: u8,
    ) -> Option<Figure> {
        let map = self
            .island_maps
            .iter()
            .find(|map| map.island_id == location.island_id)?;
        let root = (i32::from(location.tile_x), i32::from(location.tile_y));
        let start = map.source_kind12_anchor_center(root)?;
        let event_position = (
            map.source_world_origin
                .0
                .div_euclid(2)
                .checked_add(start.0)?,
            map.source_world_origin
                .1
                .div_euclid(2)
                .checked_add(start.1)?,
        );
        let (Ok(event_x), Ok(event_y)) = (
            i16::try_from(event_position.0),
            i16::try_from(event_position.1),
        ) else {
            return None;
        };
        let event_slot = self.source_figure_events.prepare_kind12_if_absent(
            event_x,
            event_y,
            location.source_owner,
        )?;
        let mut definition = civilian::source_kind12_initial_definition(self.next_source_rand());
        if !self.source_figure_pool_has_capacity() {
            return None;
        }
        if !self
            .source_figure_events
            .activate_kind12(event_slot, event_x, event_y)
        {
            return None;
        }
        let threshold = (u32::from(self.next_source_rand() & 7) + 5) * 0x40;
        let mut grid = map.civilian_path_grid(start, location.source_owner, permission_branch);
        let route = grid
            .search_with_blocked_cell_callback(start, |_, elapsed_cost| {
                if elapsed_cost >= threshold {
                    SourcePathBlockedCellDecision::Complete
                } else {
                    SourcePathBlockedCellDecision::Expand
                }
            })
            .ok();
        let (target, path) = if let Some(route) = route {
            let route_written = self
                .source_figure_events
                .write_kind12_route(event_slot, &route.steps);
            let preserved_steps = route_written
                .then(|| {
                    self.source_figure_events
                        .kind12_route_step_count(event_slot)
                })
                .flatten()
                .unwrap_or(0)
                .min(route.steps.len());
            let path =
                source_route_positions(start, &route.steps[..preserved_steps]).unwrap_or_default();
            let target = path.last().copied().unwrap_or(start);
            if let Some(target_kind) = map.civilian_path_kind(route.position) {
                definition =
                    civilian::source_kind12_definition(target_kind, self.next_source_rand());
            }
            (target, path)
        } else {
            (start, Vec::new())
        };

        let mut figure = Figure::new();
        figure.action = ActionType::Walking;
        figure.owner = location.source_owner;
        figure.origin_island = location.island_id;
        figure.tile_x = start.0;
        figure.tile_y = start.1;
        figure.target_x = target.0;
        figure.target_y = target.1;
        figure.source_event_slot = Some(event_slot);
        figure.speed = carrier::CARRIER_SPEED;
        figure.source_move_speed = self
            .civilian_config
            .movement_speed_for_definition(definition);
        figure.sprite_set = definition;
        figure.base_sprite = self.civilian_config.sprite_base_for_definition(definition);
        figure.source_position_z = map.source_terrain_height(start).unwrap_or(0.0);
        figure.initialize_source_position();
        figure.path = path;
        figure.path_idx = 0;

        Some(figure)
    }

    /// Count the live source records represented by the generic
    /// `FUN_00446ca0` figure pool. The simulator keeps the ordinary figure,
    /// dynamic-combat, and kind-15 projections in separate collections, but
    /// the executable allocates all of them from its 2,550-record pool.
    fn source_figure_pool_occupancy(&self) -> usize {
        self.figures
            .iter()
            .filter(|figure| figure.is_active())
            .count()
            + self
                .source_dynamic_combat_figures
                .iter()
                .filter(|figure| figure.active)
                .count()
            + self
                .source_kind14_combat_figures
                .iter()
                .filter(|figure| figure.active)
                .count()
            + self
                .source_kind15_combat_figures
                .iter()
                .filter(|figure| figure.active)
                .count()
    }

    fn source_figure_pool_has_capacity(&self) -> bool {
        self.source_figure_pool_occupancy() < SourceFigureRecordLayout::CAPACITY
    }

    /// `FUN_0044b7e0`: create a production-kind-2 worker at its root. The
    /// initial `FUN_0045afd0` handler pass performs target selection later.
    fn try_spawn_source_plantation_worker(&mut self, root: SourceMapCellState) -> Option<Figure> {
        if !root.is_type12_plantation_root() || !self.source_figure_pool_has_capacity() {
            return None;
        }
        if self.figures.iter().any(|figure| {
            figure.is_active()
                && figure.source_worker_route != SourceWorkerRoute::None
                && figure.origin_island == root.island
                && figure.origin_x == u16::from(root.x)
                && figure.origin_y == u16::from(root.y)
        }) {
            return None;
        }
        let map = self
            .island_maps
            .iter()
            .find(|map| map.island_id == root.island)?;
        let start = (
            i32::from(root.x) + (i32::from(root.footprint_width.max(1)) - 1) / 2,
            i32::from(root.y) + (i32::from(root.footprint_height.max(1)) - 1) / 2,
        );

        let event = self
            .prepare_source_transfer_event_with_limit(root, 1)
            .ok()
            .flatten()?;
        if !self.activate_source_transfer_event(event) {
            self.source_figure_events.release(event.slot);
            return None;
        }

        let definition = root.source_plantation_worker_definition;
        let mut figure = Figure::new();
        figure.action = ActionType::Walking;
        figure.owner = root.source_map_owner_slot;
        figure.origin_island = root.island;
        figure.origin_x = u16::from(root.x);
        figure.origin_y = u16::from(root.y);
        figure.origin_kind = root.kind_code;
        figure.origin_source_map_owner_slot = root.source_map_owner_slot;
        figure.origin_production_kind = root.source_production_kind_code;
        figure.tile_x = start.0;
        figure.tile_y = start.1;
        figure.target_x = start.0;
        figure.target_y = start.1;
        figure.source_worker_home_x = start.0;
        figure.source_worker_home_y = start.1;
        figure.select_source_animation_state(1);
        figure.carried_good = root.source_raw_resource_ware_slot;
        figure.source_worker_route = SourceWorkerRoute::Searching;
        figure.speed = carrier::CARRIER_SPEED;
        figure.source_move_speed = self
            .civilian_config
            .movement_speed_for_definition(definition);
        figure.sprite_set = definition;
        figure.base_sprite = self.civilian_config.sprite_base_for_definition(definition);
        figure.source_position_z = map.source_terrain_height(start).unwrap_or(0.0);
        figure.initialize_source_position();
        figure.source_event_slot = Some(event.slot);
        Some(figure)
    }

    /// `FUN_0045b200`: populate the source path grid around a newly spawned
    /// worker, claim its selected raw-resource cell, and install the outbound
    /// route. A failed search leaves the worker active at its root for a
    /// later handler pass.
    fn try_assign_source_plantation_worker_target(&mut self, figure_index: usize) -> bool {
        let Some(figure) = self.figures.get(figure_index).cloned() else {
            return false;
        };
        if figure.source_worker_route != SourceWorkerRoute::Searching {
            return false;
        }
        let Some(root) =
            self.source_map_cell_states.iter().copied().find(|state| {
                state.matches(figure.origin_island, figure.origin_x, figure.origin_y)
            })
        else {
            return false;
        };
        let candidates: Vec<_> = self
            .source_static_map_roots
            .iter()
            .enumerate()
            .filter(|(_, cell)| {
                cell.island == figure.origin_island
                    && cell.is_plantation_worker_target(
                        figure.origin_source_map_owner_slot,
                        figure.carried_good,
                    )
            })
            .map(|(index, cell)| {
                (
                    index,
                    (i32::from(cell.x), i32::from(cell.y)),
                    cell.source_path_class,
                )
            })
            .collect();
        if candidates.is_empty() {
            return false;
        }
        let start = (figure.tile_x, figure.tile_y);
        let Some((target_index, route, path)) = (|| {
            let map = self
                .island_maps
                .iter()
                .find(|map| map.island_id == figure.origin_island)?;
            let mut grid = map.plantation_worker_path_grid(
                start,
                (i32::from(root.x), i32::from(root.y)),
                (root.footprint_width, root.footprint_height),
                root.source_transfer_radius,
                figure.origin_source_map_owner_slot,
                figure.carried_good,
                &self.source_static_map_roots,
            );
            for &(_, position, path_class) in &candidates {
                let target = SourcePathTargetRect::new(position, 1, 1)?;
                grid.set_target_region_metadata(target, (path_class & 0x7f) | 0x80);
            }
            let route = grid.search_source_high_metadata_target(start, 0).ok()?;
            let target_index = candidates
                .iter()
                .rev()
                .find(|(_, candidate, _)| *candidate == route.position)
                .map(|(index, _, _)| *index)?;
            let path = source_route_positions(start, &route.steps)?;
            (path.last().copied() == Some(route.position)).then_some((target_index, route, path))
        })() else {
            return false;
        };

        self.source_static_map_roots[target_index].source_resource_reserved = true;
        self.source_map_cell_revision = self.source_map_cell_revision.wrapping_add(1);
        let figure = &mut self.figures[figure_index];
        figure.target_x = route.position.0;
        figure.target_y = route.position.1;
        figure.supplier_x = route.position.0 as u16;
        figure.supplier_y = route.position.1 as u16;
        figure.source_worker_route = SourceWorkerRoute::ToResource;
        figure.path = path;
        figure.path_idx = 0;
        figure.source_step_remaining = 0.0;
        figure.source_event_route_steps = route.steps;
        if let Some(slot) = figure.source_event_slot {
            self.source_figure_events
                .write_plantation_route(slot, &figure.source_event_route_steps);
        }
        true
    }

    /// `FUN_0045b430` first clears the route and records the root target;
    /// its following `FUN_0045afd0` pass computes the worker's return route.
    fn try_assign_source_plantation_worker_return_route(&mut self, figure_index: usize) -> bool {
        let Some(figure) = self.figures.get(figure_index).cloned() else {
            return false;
        };
        if figure.source_worker_route != SourceWorkerRoute::ReturningSearch {
            return false;
        }
        let start = (figure.tile_x, figure.tile_y);
        let goal = (figure.source_worker_home_x, figure.source_worker_home_y);
        let Some(root) =
            self.source_map_cell_states.iter().copied().find(|state| {
                state.matches(figure.origin_island, figure.origin_x, figure.origin_y)
            })
        else {
            return false;
        };
        let Some((route_steps, path)) = self
            .island_maps
            .iter()
            .find(|map| map.island_id == figure.origin_island)
            .and_then(|map| {
                let mut grid = map.plantation_worker_path_grid(
                    start,
                    (i32::from(root.x), i32::from(root.y)),
                    (root.footprint_width, root.footprint_height),
                    root.source_transfer_radius,
                    figure.origin_source_map_owner_slot,
                    figure.carried_good,
                    &self.source_static_map_roots,
                );
                let root_target = SourcePathTargetRect::new(
                    (
                        i32::from(root.source_command_anchor_x),
                        i32::from(root.source_command_anchor_y),
                    ),
                    usize::from(root.footprint_width.max(1)),
                    usize::from(root.footprint_height.max(1)),
                )?;
                grid.set_target_region_metadata(root_target, 0x28);
                grid.route_to(start, goal).ok()
            })
            .and_then(|steps| source_route_positions(start, &steps).map(|path| (steps, path)))
        else {
            return false;
        };
        if path.last().copied() != Some(goal) {
            return false;
        }

        let figure = &mut self.figures[figure_index];
        figure.source_worker_route = SourceWorkerRoute::Returning;
        figure.path = path;
        figure.path_idx = 0;
        figure.source_step_remaining = 0.0;
        figure.source_event_route_steps = route_steps;
        if let Some(slot) = figure.source_event_slot {
            self.source_figure_events
                .write_plantation_route(slot, &figure.source_event_route_steps);
        }
        true
    }

    /// Derive the generic figure-8 source search view from the live root
    /// buffers. `FUN_0047daf0` updates one root before entering its transfer
    /// switch, so each dispatch observes output from roots already processed
    /// in the same scheduler pass.
    fn carrier_suppliers(&self) -> Vec<carrier::CarrierSupplier> {
        let mut suppliers: Vec<_> = self
            .buildings
            .iter()
            .filter(|building| building.active && building.output_stock != 0)
            .filter_map(|building| {
                let definition = self.building_defs.get(building.def_id as usize)?;
                let orientation = building
                    .source_placement_command
                    .map(|command| command.orientation)
                    .unwrap_or(0);
                let source_footprint = if orientation & 1 == 0 {
                    (definition.width.max(1), definition.height.max(1))
                } else {
                    (definition.height.max(1), definition.width.max(1))
                };
                (definition.output_good != Good::None).then_some(carrier::CarrierSupplier {
                    island: building.island_id,
                    owner: building.owner,
                    x: building.tile_x,
                    y: building.tile_y,
                    good: definition.output_good,
                    available: building.output_stock,
                    storage: carrier::CarrierSupplierStorage::SourceRoot,
                    source_path_class: carrier::source_path_class(definition.wegspeed[0]),
                    source_footprint,
                })
            })
            .collect();
        suppliers.extend(
            self.warehouses
                .iter()
                .filter(|warehouse| warehouse.active)
                .enumerate()
                .flat_map(|(warehouse_idx, warehouse)| {
                    warehouse.all_stock().into_iter().map(|(good, stock, _)| {
                        carrier::CarrierSupplier {
                            island: warehouse.island_id,
                            owner: warehouse.owner,
                            x: warehouse.tile_x,
                            y: warehouse.tile_y,
                            good,
                            available: stock,
                            storage: carrier::CarrierSupplierStorage::Warehouse(warehouse_idx),
                            source_path_class: warehouse.source_path_class,
                            source_footprint: warehouse.source_footprint,
                        }
                    })
                }),
        );
        suppliers
    }

    fn tick_production(&mut self) {
        self.sync_source_city_populations_to_warehouses();
        let mut new_carriers = Vec::new();
        let mut source_figure_slots_remaining =
            SourceFigureRecordLayout::CAPACITY.saturating_sub(self.source_figure_pool_occupancy());
        let source_dispatch_phase = self.source_map_dispatch_phase;
        let mut source_dispatch_roots = Vec::new();
        for state in &mut self.source_map_cell_states {
            if state.source_scheduler_due(source_dispatch_phase) {
                source_dispatch_roots.push(*state);
            }
        }
        for i in 0..self.buildings.len() {
            let def_id = self.buildings[i].def_id;
            if def_id as usize >= self.building_defs.len() {
                continue;
            }
            let def = self.building_defs[def_id as usize].clone();
            let source_raw_material_stock = self.buildings[i].input_1_stock.saturating_mul(32);
            let source_work_material_stock = self.buildings[i].input_2_stock.saturating_mul(32);
            let source_storage_fill = self.buildings[i].output_stock.saturating_mul(32);
            let source_dispatch_due = source_dispatch_roots.iter().any(|root| {
                root.matches(
                    self.buildings[i].island_id,
                    self.buildings[i].tile_x,
                    self.buildings[i].tile_y,
                )
            });
            let building_runs_source_scheduler =
                self.buildings[i].active && self.buildings[i].is_built() && source_dispatch_due;
            let state_changed = if let Some(state) =
                self.source_map_cell_states.iter_mut().find(|state| {
                    state.matches(
                        self.buildings[i].island_id,
                        self.buildings[i].tile_x,
                        self.buildings[i].tile_y,
                    )
                }) {
                let before = *state;
                // Scenario and player building stocks enter the source record
                // once. Afterwards the record remains authoritative so 1/32
                // remainders from production and type-8 transfers survive.
                if state.raw_material_stock == 0 && source_raw_material_stock != 0 {
                    state.raw_material_stock = source_raw_material_stock;
                }
                if state.work_material_stock == 0 && source_work_material_stock != 0 {
                    state.work_material_stock = source_work_material_stock;
                }
                if state.storage_fill == 0 && source_storage_fill != 0 {
                    state.storage_fill = source_storage_fill;
                }
                self.buildings[i].input_1_stock = state.raw_material_stock / 32;
                self.buildings[i].input_2_stock = state.work_material_stock / 32;
                self.buildings[i].output_stock = state.storage_fill / 32;
                if building_runs_source_scheduler {
                    state.advance_source_scheduler();
                    self.buildings[i].efficiency = state.activity;
                }
                self.buildings[i].input_1_stock = state.raw_material_stock / 32;
                self.buildings[i].input_2_stock = state.work_material_stock / 32;
                self.buildings[i].output_stock = state.storage_fill / 32;
                *state != before
            } else {
                production::tick_building(
                    &mut self.buildings[i],
                    &def,
                    self.timer_production.interval_ms,
                );
                false
            };
            if state_changed {
                self.source_map_cell_revision = self.source_map_cell_revision.wrapping_add(1);
            }

            let source_type8_transfer_input = source_dispatch_due
                .then(|| {
                    self.source_map_cell_states
                        .iter()
                        .copied()
                        .find(|state| {
                            state.matches(
                                self.buildings[i].island_id,
                                self.buildings[i].tile_x,
                                self.buildings[i].tile_y,
                            )
                        })
                        .and_then(SourceMapCellState::source_type8_transfer_input)
                })
                .flatten();
            if self.buildings[i].active && source_type8_transfer_input.is_some() {
                // Check if this building already has an active carrier
                let has_carrier = self.figures.iter().any(|f| {
                    f.is_active()
                        && f.building_idx == i as u16
                        && matches!(f.action, ActionType::CarryingGoods | ActionType::Returning)
                });

                if !has_carrier {
                    let event = match self.prepare_source_transfer_event_for_root(
                        self.buildings[i].island_id,
                        self.buildings[i].tile_x,
                        self.buildings[i].tile_y,
                    ) {
                        Ok(event) => event,
                        Err(()) => continue,
                    };
                    if source_figure_slots_remaining == 0 {
                        continue;
                    }
                    let carrier_suppliers = self.carrier_suppliers();
                    if let Some(mut c) = carrier::try_spawn_carrier_for_source_input(
                        &self.buildings[i],
                        &def,
                        source_type8_transfer_input.expect("checked above"),
                        &carrier_suppliers,
                        &mut self.source_map_cell_states,
                        &mut self.warehouses,
                        &self.island_maps,
                        self.carrier_config,
                    ) {
                        c.building_idx = i as u16;
                        if let Some(event) = event {
                            if self.activate_source_transfer_event(event) {
                                c.source_event_slot = Some(event.slot);
                            }
                        }
                        new_carriers.push(c);
                        source_figure_slots_remaining -= 1;
                    }
                }
            }
        }

        // Type 11 is scheduled from city command roots, not from the
        // production-building loop. MARKT and KONTOR roots use their city's
        // inventory to build the FUN_00480610 capacity eligibility bytes
        // before selecting and reserving a producer root.
        let city_origins: Vec<_> = source_dispatch_roots
            .iter()
            .filter_map(|root| {
                self.source_map_cell_states
                    .iter()
                    .copied()
                    .find(|state| state.matches(root.island, u16::from(root.x), u16::from(root.y)))
            })
            .filter(|state| {
                state.is_type11_transfer_root() && state.allows_source_transfer_dispatch()
            })
            .collect();
        let carrier_suppliers = self.carrier_suppliers();
        let city_cart_suppliers =
            Self::city_cart_supplier_view(&carrier_suppliers, &self.buildings, &self.building_defs);
        for origin in city_origins {
            let Some(city_cart_config) = self.city_cart_config_for(origin.source_transfer_figure)
            else {
                continue;
            };
            let Some(city_owner) =
                self.source_city_player_owner(origin.island, origin.source_map_owner_slot)
            else {
                continue;
            };
            let Some(warehouse_idx) = self
                .warehouses
                .iter()
                .enumerate()
                .find(|warehouse| {
                    warehouse.1.active
                        && warehouse.1.island_id == origin.island
                        && warehouse.1.owner == city_owner
                })
                .map(|(idx, _)| idx)
            else {
                continue;
            };
            let transfer_root_count =
                self.source_city_transfer_root_count(origin.island, origin.source_map_owner_slot);
            let city_capacity_fixed =
                self.warehouses[warehouse_idx].city_storage_capacity_fixed(transfer_root_count);
            let city = carrier::CityCartEligibility::from_city_store(
                &self.warehouses[warehouse_idx],
                city_capacity_fixed,
            );
            if origin.source_transfer_figure_limit == 0 {
                continue;
            }
            let event = match self.prepare_source_transfer_event(origin) {
                Ok(event) => event,
                Err(()) => continue,
            };
            if source_figure_slots_remaining == 0 {
                continue;
            }
            if let Some(cart) = carrier::try_spawn_city_cart(
                origin,
                city,
                &city_cart_suppliers,
                &mut self.source_map_cell_states,
                &self.island_maps,
                city_cart_config,
            ) {
                let mut cart = cart;
                if let Some(event) = event {
                    if self.activate_source_transfer_event(event) {
                        self.source_figure_events
                            .write_transfer_route(event.slot, &cart.source_event_route_steps);
                        cart.source_event_slot = Some(event.slot);
                    }
                }
                new_carriers.push(cart);
                source_figure_slots_remaining -= 1;
            }
        }

        let plantation_roots: Vec<_> = source_dispatch_roots
            .iter()
            .filter_map(|root| {
                self.source_map_cell_states
                    .iter()
                    .copied()
                    .find(|state| state.matches(root.island, u16::from(root.x), u16::from(root.y)))
            })
            .filter(|root| root.is_type12_plantation_root())
            .collect();
        for root in plantation_roots {
            if source_figure_slots_remaining == 0 {
                break;
            }
            if let Some(worker) = self.try_spawn_source_plantation_worker(root) {
                new_carriers.push(worker);
                source_figure_slots_remaining -= 1;
            }
        }

        for root in &source_dispatch_roots {
            if let Some(state) = self
                .source_map_cell_states
                .iter_mut()
                .find(|state| state.matches(root.island, u16::from(root.x), u16::from(root.y)))
            {
                state.complete_source_scheduler_run();
            }
        }

        self.figures.extend(new_carriers);
    }

    /// `FUN_0047bbc0` / `FUN_0047c080` mutate the live STADT4 population
    /// groups. Type-11 `FUN_00480610` reads those groups when it builds city
    /// eligibility bytes, so mirror the matching source-city record into all
    /// KONTOR stores on that island before dispatching carts.
    fn sync_source_city_populations_to_warehouses(&mut self) {
        let cities = self.source_cities.active_records();
        for warehouse in &mut self.warehouses {
            if let Some(city) = cities.iter().find(|city| {
                city.island_id == warehouse.island_id && city.owner_slot == warehouse.owner
            }) {
                warehouse.city_population = city.tier_population;
            }
        }
    }

    /// `FUN_00481170`, `FUN_00481fc0`, and `FUN_004820b0` maintain the
    /// city-record field at `+0x1fa` through the island map's owner-indexed
    /// city table. `FUN_0047ab00` reads that field for city capacity, so a
    /// city counts only its own MARKT and KONTOR roots.
    fn source_city_transfer_root_count(&self, island_id: u8, map_owner_slot: u8) -> usize {
        self.source_map_cell_states
            .iter()
            .filter(|state| {
                state.island == island_id
                    && state.source_map_owner_slot == map_owner_slot
                    && matches!(state.source_production_kind_code, 7 | 8)
            })
            .count()
    }

    /// `FUN_004596b0` identifies a city by the root's map-owner byte, then
    /// `FUN_00480610` uses that city's player slot for its inventory. Source
    /// scenarios provide the city table; empty tables occur only in local
    /// synthetic maps, where the prior single owner identity is retained.
    fn source_city_player_owner(&self, island_id: u8, map_owner_slot: u8) -> Option<u8> {
        let cities = self.source_cities.active_records();
        if cities.is_empty() {
            return Some(map_owner_slot);
        }
        cities
            .into_iter()
            .find(|city| city.island_id == island_id && city.source_owner == map_owner_slot)
            .map(|city| city.owner_slot)
    }

    /// Resolve the generic-figure definition selected by a type-11 root's
    /// compiled `Figurnr`. The source invokes `FUN_00446ca0` with this
    /// selector, so an unrecognized root cannot be substituted with KARREN.
    fn city_cart_config_for(
        &self,
        source_figure: SourceTransferFigure,
    ) -> Option<carrier::CityCartConfig> {
        match source_figure {
            SourceTransferFigure::Karren => Some(self.city_cart_config),
            SourceTransferFigure::Traeger2 => Some(self.city_cart_traeger2_config),
            SourceTransferFigure::Unknown => None,
        }
    }

    /// `FUN_0044ab60` / `FUN_0044ad50`: locate the source root's oriented
    /// center and initialize a free transfer event before generic-figure
    /// allocation. Type 8 rejects a matching live event; type 11 applies its
    /// authored matching-entry bound. A missing root only occurs in the
    /// simulator's synthetic-map fallback, which has no source event
    /// coordinate to register.
    fn prepare_source_transfer_event_for_root(
        &mut self,
        island: u8,
        x: u16,
        y: u16,
    ) -> Result<Option<SourceTransferEventCandidate>, ()> {
        let (Ok(x), Ok(y)) = (u8::try_from(x), u8::try_from(y)) else {
            return Ok(None);
        };
        let Some(root) = self
            .source_map_cell_states
            .iter()
            .copied()
            .find(|root| root.island == island && root.x == x && root.y == y)
        else {
            return Ok(None);
        };
        self.prepare_source_type8_transfer_event(root)
    }

    /// `FUN_0044ad50`: the type-11 root's `Figuranz` bounds the number of
    /// matching x/y/owner entries, unlike the type-8 duplicate-only gate.
    fn prepare_source_transfer_event(
        &mut self,
        root: SourceMapCellState,
    ) -> Result<Option<SourceTransferEventCandidate>, ()> {
        self.prepare_source_transfer_event_with_limit(root, root.source_transfer_figure_limit)
    }

    fn prepare_source_type8_transfer_event(
        &mut self,
        root: SourceMapCellState,
    ) -> Result<Option<SourceTransferEventCandidate>, ()> {
        self.prepare_source_transfer_event_with_limit(root, 1)
    }

    fn prepare_source_transfer_event_with_limit(
        &mut self,
        root: SourceMapCellState,
        limit: u8,
    ) -> Result<Option<SourceTransferEventCandidate>, ()> {
        let Some(map) = self
            .island_maps
            .iter()
            .find(|map| map.island_id == root.island)
        else {
            return Ok(None);
        };
        let center_x = i32::from(root.x) + (i32::from(root.footprint_width.max(1)) - 1) / 2;
        let center_y = i32::from(root.y) + (i32::from(root.footprint_height.max(1)) - 1) / 2;
        let (Some(world_x), Some(world_y)) = (
            map.source_world_origin
                .0
                .div_euclid(2)
                .checked_add(center_x),
            map.source_world_origin
                .1
                .div_euclid(2)
                .checked_add(center_y),
        ) else {
            return Ok(None);
        };
        let (Ok(x), Ok(y)) = (i16::try_from(world_x), i16::try_from(world_y)) else {
            return Ok(None);
        };
        let owner = root.source_map_owner_slot;
        let Some(slot) = self
            .source_figure_events
            .prepare_transfer_with_limit(x, y, owner, limit)
        else {
            return Err(());
        };
        Ok(Some(SourceTransferEventCandidate {
            slot,
            x,
            y,
            owner,
            route_radius: root.source_transfer_radius,
        }))
    }

    fn activate_source_transfer_event(&mut self, event: SourceTransferEventCandidate) -> bool {
        self.source_figure_events.activate_transfer(
            event.slot,
            event.x,
            event.y,
            event.owner,
            event.route_radius,
        )
    }

    fn city_cart_supplier_view(
        suppliers: &[carrier::CarrierSupplier],
        buildings: &[BuildingInstance],
        definitions: &[BuildingDef],
    ) -> Vec<carrier::CarrierSupplier> {
        suppliers
            .iter()
            .copied()
            .map(|mut supplier| {
                if supplier.storage == carrier::CarrierSupplierStorage::SourceRoot {
                    if let Some(definition) = buildings
                        .iter()
                        .find(|building| {
                            building.active
                                && building.island_id == supplier.island
                                && building.owner == supplier.owner
                                && building.tile_x == supplier.x
                                && building.tile_y == supplier.y
                        })
                        .and_then(|building| definitions.get(building.def_id as usize))
                    {
                        supplier.source_path_class =
                            carrier::source_path_class(definition.wegspeed[2]);
                    }
                }
                supplier
            })
            .collect()
    }

    pub(crate) fn tick_population(&mut self) {
        // Refresh building maintenance totals per player so the economy
        // tick has up-to-date running costs. Also compute per-player
        // housing capacity from completed WOHN residences, scaled by
        // each residence's `house_tier`. RE-cited from haeuser.cod
        // `Maxwohn` distribution (Pioneer 2, Settler 6, Citizen 15,
        // Merchant 25, Aristocrat 40 — the five distinct Maxwohn
        // values in the building table).
        const HOUSING_BY_TIER: [u32; 5] = [2, 6, 15, 25, 40];
        let mut maintenance: Vec<u32> = vec![0; self.players.len()];
        let mut housing: Vec<u32> = vec![0; self.players.len()];
        // Promotion pass: WOHN buildings whose tier is fully satisfied
        // upgrade up. Done before maintenance/cap so the housing cap
        // immediately reflects the new sizes.
        for b in self.buildings.iter_mut() {
            if !b.active || !b.is_built() {
                continue;
            }
            let def_id = b.def_id as usize;
            if def_id >= self.building_defs.len() {
                continue;
            }
            let def = &self.building_defs[def_id];
            let is_residence = def.kind == "WOHN" || def.prod_kind == "WOHN";
            if !is_residence {
                continue;
            }
            // Manual sec. 6.7.1 + haeuser.cod `Ausbauflg`: only
            // upgradeable residence shells can promote up the tier
            // ladder. Static residences (e.g. construction
            // placeholders, native huts) keep their tier.
            if !def.upgradeable {
                continue;
            }
            let owner = b.owner as usize;
            let Some(p) = self.players.get(owner) else {
                continue;
            };
            let t = b.house_tier as usize;
            if t < 4 && p.satisfaction[t] >= 100 {
                b.house_tier += 1;
            }
        }
        for b in &self.buildings {
            if !b.active || !b.is_built() {
                continue;
            }
            let owner = b.owner as usize;
            if owner >= maintenance.len() {
                continue;
            }
            let def_id = b.def_id as usize;
            if def_id < self.building_defs.len() {
                let cost = self.building_defs[def_id].maintenance_cost as u32;
                maintenance[owner] = maintenance[owner].saturating_add(cost);
                let kind = self.building_defs[def_id].kind.as_str();
                let pk = self.building_defs[def_id].prod_kind.as_str();
                if kind == "WOHN" || pk == "WOHN" {
                    let t = (b.house_tier as usize).min(4);
                    housing[owner] = housing[owner].saturating_add(HOUSING_BY_TIER[t]);
                }
            }
        }
        for (i, player) in self.players.iter_mut().enumerate() {
            player.building_maintenance = maintenance[i];
            // Update demands and consume goods from warehouses
            population::update_population_demands(player, &mut self.warehouses, i as u8);
            // Apply economy (gold balance, bankruptcy, satisfaction decay)
            economy::tick_economy(player);
            // Grow / shrink population by tier and promote satisfied tiers up,
            // clamped to current housing capacity.
            population::update_population_growth(player, housing[i]);
        }

        if !self.players.is_empty() {
            // Snapshot to break the borrow with self.objectives.
            let p0 = self.players[0].clone();
            // Re-evaluate scenario objectives against the human player.
            let just_done = self.objectives.evaluate(
                &p0,
                &self.buildings,
                &self.building_defs,
                &self.warehouses,
                &self.players,
                0,
            );
            let any_just_done = !just_done.is_empty();
            self.objective_completions.extend(just_done);
            // Edge-triggered scenario-complete: fire once when
            // the LAST objective flips from pending to done.
            if any_just_done && !self.scenario_complete {
                let (done, total) = self.objectives.progress();
                if total > 0 && done == total {
                    self.scenario_complete = true;
                    self.event_log
                        .push("[victory] Scenario complete!".to_string());
                }
            }
            let p0 = &p0;

            // Voice announcement: fire on entry into the bankruptcy
            // window. RE: `player::BANKRUPTCY_THRESHOLD = -1001` —
            // the same floor the original uses to start counting
            // `bankruptcy_ticks` toward game-over (40 ticks). Edge-
            // triggered against the previous sample so the line
            // doesn't flood every tick the player is in the red.
            let p0_gold = p0.gold;
            if p0_gold < crate::player::BANKRUPTCY_THRESHOLD
                && self.last_treasury_warn_gold >= crate::player::BANKRUPTCY_THRESHOLD
            {
                self.event_log
                    .push("[treasury] our treasury is running dangerously low".to_string());
            }
            self.last_treasury_warn_gold = p0_gold;
        }

        self.evaluate_outcomes();

        // AI decision-making
        self.tick_ai();
    }

    /// Flip player slots to `PlayerState::Defeated` when their
    /// civilisation collapses, and resolve the human player's
    /// `outcome` when victory or defeat conditions are met.
    fn evaluate_outcomes(&mut self) {
        use crate::player::PlayerState;

        // 1. Per-slot defeat: sustained bankruptcy (40 ticks below
        //    -1001 gold) or zero active Kontors. We snapshot the
        //    Kontor count to avoid an iterator-borrow conflict.
        let active_kontors_for: Vec<u32> = (0..self.players.len())
            .map(|owner| {
                self.warehouses
                    .iter()
                    .filter(|w| w.active && w.owner as usize == owner)
                    .count() as u32
            })
            .collect();
        for (i, p) in self.players.iter_mut().enumerate() {
            if matches!(p.state, PlayerState::Empty | PlayerState::Defeated) {
                continue;
            }
            let bankrupt_too_long = p.is_game_over();
            let no_economy = active_kontors_for[i] == 0 && p.population.iter().all(|n| *n == 0);
            if bankrupt_too_long || no_economy {
                p.state = PlayerState::Defeated;
                self.event_log
                    .push(format!("[defeat] player {i} has been defeated",));
            }
        }

        // 2. Human outcome (slot 0). Once decided, never revert.
        if self.outcome != GameOutcome::Pending {
            return;
        }
        let human_defeated = self
            .players
            .first()
            .map(|p| p.state == PlayerState::Defeated)
            .unwrap_or(false);
        if human_defeated {
            self.outcome = GameOutcome::Defeat;
            self.event_log.push("[outcome] DEFEAT".to_string());
            return;
        }

        // Victory by elimination: at least one rival existed at game
        // start, and all non-human / non-empty / non-pirate slots are
        // now Defeated.
        let any_rival = self
            .players
            .iter()
            .enumerate()
            .any(|(i, p)| i != 0 && !matches!(p.state, PlayerState::Empty));
        let all_rivals_down =
            self.players.iter().enumerate().all(|(i, p)| {
                i == 0 || matches!(p.state, PlayerState::Empty | PlayerState::Defeated)
            });
        if any_rival && all_rivals_down {
            self.outcome = GameOutcome::Victory;
            self.event_log
                .push("[outcome] VICTORY — all rivals defeated".to_string());
            return;
        }

        // Victory by objectives: every scenario objective complete.
        if !self.objectives.items.is_empty() && self.objectives.items.iter().all(|(_, done)| *done)
        {
            self.outcome = GameOutcome::Victory;
            self.event_log
                .push("[outcome] VICTORY — scenario objectives complete".to_string());
        }
    }

    pub(crate) fn tick_ai(&mut self) {
        for ai_idx in 0..self.ai_controllers.len() {
            let player_idx = self.ai_controllers[ai_idx].player_idx as usize;
            if player_idx >= self.players.len() {
                continue;
            }

            let actions = self.ai_controllers[ai_idx].tick(
                &self.players[player_idx],
                &self.buildings,
                &self.building_defs,
                &self.warehouses,
            );

            // Apply AI actions
            for action in actions {
                match action {
                    AiAction::SetTaxRate { .. } => {}
                    AiAction::RequestBuild { .. } => {}
                    AiAction::SellExcess => {}
                }
            }
        }
    }

    fn tick_market_coverage(&mut self) {
        // Collect warehouse positions per island
        let mut wh_by_island: std::collections::HashMap<u8, Vec<(u16, u16, u16)>> =
            std::collections::HashMap::new();
        for wh in &self.warehouses {
            if wh.active {
                // Warehouse base radius = 22 (RADIUS_HQ from original binary)
                wh_by_island
                    .entry(wh.island_id)
                    .or_default()
                    .push((wh.tile_x, wh.tile_y, 22));
            }
        }

        // Recompute coverage for each island that has a coverage map
        for cov in &mut self.coverage_maps {
            let whs = wh_by_island
                .get(&cov.island_id)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            cov.recompute(&self.buildings, &self.building_defs, whs);
        }

        // Reveal tiles around player-owned entities.
        self.tick_exploration();
    }

    fn tick_exploration(&mut self) {
        const PLAYER: u8 = 0;
        const SIGHT_RADIUS: i32 = 5;
        // Helper: get-or-create the per-island exploration map.
        let ensure = |sim: &mut Simulation, island_id: u8| -> usize {
            if let Some(idx) = sim
                .exploration
                .iter()
                .position(|e| e.island_id == island_id)
            {
                return idx;
            }
            // Pull dimensions from the island map if it exists, else default.
            let (w, h) = sim
                .island_maps
                .iter()
                .find(|m| m.island_id == island_id)
                .map(|m| (m.width, m.height))
                .unwrap_or((128, 128));
            sim.exploration
                .push(crate::exploration::ExplorationMap::new(island_id, w, h));
            sim.exploration.len() - 1
        };
        // Buildings.
        let bldg_seeds: Vec<(u8, i32, i32)> = self
            .buildings
            .iter()
            .filter(|b| b.owner == PLAYER && b.active)
            .map(|b| (b.island_id, b.tile_x as i32, b.tile_y as i32))
            .collect();
        for (iid, x, y) in bldg_seeds {
            let idx = ensure(self, iid);
            self.exploration[idx].mark_radius(x, y, SIGHT_RADIUS);
        }
        // Warehouses.
        let wh_seeds: Vec<(u8, i32, i32)> = self
            .warehouses
            .iter()
            .filter(|w| w.owner == PLAYER && w.active)
            .map(|w| (w.island_id, w.tile_x as i32, w.tile_y as i32))
            .collect();
        for (iid, x, y) in wh_seeds {
            let idx = ensure(self, iid);
            self.exploration[idx].mark_radius(x, y, SIGHT_RADIUS);
        }
        // Military units (land only — naval roams the world map, no
        // island-tile coords meaningful to per-island bitmap).
        let unit_seeds: Vec<(u8, i32, i32)> = self
            .military_units
            .iter()
            .filter(|u| u.is_alive() && u.owner == PLAYER && !u.unit_type.stats().is_naval)
            .filter_map(|u| {
                if let Some(island_id) = u.source_island_id {
                    return self
                        .island_maps
                        .iter()
                        .find(|map| map.island_id == island_id)
                        .and_then(|map| map.source_world_to_local((u.tile_x, u.tile_y)))
                        .map(|(x, y)| (island_id, x, y));
                }
                // Try to find which island this unit is on by walkability map.
                let island_id = self
                    .island_maps
                    .iter()
                    .find(|m| m.is_walkable(u.tile_x, u.tile_y))
                    .map(|m| m.island_id)
                    .unwrap_or(0);
                Some((island_id, u.tile_x, u.tile_y))
            })
            .collect();
        for (iid, x, y) in unit_seeds {
            let idx = ensure(self, iid);
            self.exploration[idx].mark_radius(x, y, SIGHT_RADIUS);
        }
    }

    fn tick_diplomacy(&mut self) {}

    fn tick_ships(&mut self) {
        for ship_idx in 0..self.trade_ships.len() {
            if !self.trade_ships[ship_idx].active {
                continue;
            }
            let route_id = self.trade_ships[ship_idx].route_id;
            if let Some(route) = self.trade_routes.iter().find(|r| r.id == route_id) {
                let route = route.clone();
                let gold = trade::tick_trade_ship(
                    &mut self.trade_ships[ship_idx],
                    &route,
                    &mut self.warehouses,
                    self.ocean_map.as_ref(),
                );
                // Apply gold to ship owner
                let owner = self.trade_ships[ship_idx].owner as usize;
                if owner < self.players.len() {
                    self.players[owner].gold += gold;
                }
            }
        }
        self.tick_free_traders();
    }

    fn tick_free_traders(&mut self) {
        if self.free_trader_cooldown > 0 {
            self.free_trader_cooldown -= 1;
        }

        // Free-trader population from the original Anno 1602 manual
        // (section 11.4.3 "Placing ships"):
        //   "A free traders' ship will automatically be placed as soon
        //   as two warehouses have been built in your island chain.
        //   This means that the more warehouses built in your chain of
        //   islands, no matter which color player has built them, the
        //   more free traders there will be."
        // So the count of free-trader ships scales with the total
        // warehouse count: roughly one ship per two warehouses,
        // counting all colours.
        let total_active_kontors = self
            .warehouses
            .iter()
            .filter(|w| w.active && w.owner < 4)
            .count();
        let target_traders = (total_active_kontors / 2).min(8);
        let alive_traders = self.free_traders.iter().filter(|t| t.active).count();
        let need_one_more = alive_traders < target_traders && self.free_trader_cooldown == 0;

        if need_one_more {
            // Spawn at a random edge of the loaded ocean map. Assetless
            // simulation tests keep the old 200-tile fallback square.
            let side = self.next_rand();
            let off = self.next_rand();
            let (sx, sy) = if let Some(ocean) = self.ocean_map.as_ref() {
                free_trader_edge_point_from_ocean(ocean, side, off)
            } else {
                free_trader_edge_point_from_bounds(side, off, 200, 200)
            };
            let mut trader = crate::free_trader::FreeTrader::spawn_at_with_capacity(
                sx,
                sy,
                self.ship_cargo_config.free_trader_capacity,
            );
            self.assign_next_port(&mut trader);
            self.free_traders.push(trader);
            self.event_log
                .push("[trader] free trader sighted at the horizon".to_string());
            self.free_trader_cooldown = 0;
        }

        // Step each active trader.
        let mut player_gold: Vec<i32> = self.players.iter().map(|p| p.gold).collect();
        for i in 0..self.free_traders.len() {
            if !self.free_traders[i].active {
                continue;
            }
            let removed = crate::free_trader::tick_one(
                &mut self.free_traders[i],
                &mut self.warehouses,
                &mut player_gold,
            );
            if removed {
                continue;
            }
            // Pick next port if just finished docking. The original
            // trader state machine only invokes `FUN_004488d0` from
            // the "seeking next target" state on `(rand() & 3) == 0`
            // (`1602_exe.c:57709-57714`), so a trader may wait at the
            // last port for a few ship ticks before choosing again.
            if self.free_traders[i].state == crate::free_trader::FreeTraderState::Sailing
                && self.free_traders[i].target_warehouse.is_none()
                && !self.free_traders[i].leaving
            {
                let r = self.next_rand();
                if (r & crate::fidelity::FREE_TRADER_TARGET_GATE_MASK) != 0 {
                    continue;
                }
                let mut t = std::mem::take(&mut self.free_traders[i]);
                self.assign_next_port(&mut t);
                self.free_traders[i] = t;
            }
        }
        for (i, g) in player_gold.into_iter().enumerate() {
            if i < self.players.len() {
                self.players[i].gold = g;
            }
        }
        // Drop departed ships and start the cooldown.
        let before = self.free_traders.len();
        self.free_traders.retain(|t| t.active);
        if self.free_traders.len() < before && self.free_trader_cooldown == 0 {
            // Respawn delay between trader visits. The fixed minimum
            // gap is 60 ship-ticks (~6 s in our 10 Hz tick rate,
            // decoded from `:98053` `DAT_005b6040 / 600` minute
            // display).
            self.free_trader_cooldown = crate::fidelity::FREE_TRADER_RESPAWN_COOLDOWN_TICKS;
        }
    }

    fn assign_next_port(&mut self, trader: &mut crate::free_trader::FreeTrader) {
        // Trader port selection mirrors `1602_exe.c:50245 FUN_004488d0`:
        //   1. Iterate player slots (`&DAT_005b7680`, stride 0xa0).
        //   2. Skip if state ≠ 0 ("none") and ≠ 0xc ("active").
        //   3. Score the slot via `FUN_00475c60(trader_owner, slot, 0)`
        //      (a relations score; lower = trader prefers this slot
        //      more) and keep the lowest-scoring slot, with a
        //      `(rand() & 0xf) + score < best` tiebreaker.
        //   4. Collect the winning slot's tiles into `aiStack_258[150]`
        //      via `FUN_0044f000` and pick a random one with
        //      `aiStack_258[rand() % count]`.
        //
        // We approximate the relations score with a sign-flip:
        // human / AI slots (0..=3) are the preferred candidates;
        // the reserved factions (4 = trader itself, 5 = natives,
        // 6 = pirates) never host trader visits. Within that set,
        // reproduce FUN_004547e0/FUN_004549f0's port choice:
        // keep the 12 nearest ports by `|dx| + |dy| / 4`, pick the
        // best positive trade-score port if one exists, otherwise
        // fall back to a random reachable port inside that shortlist.
        let r = self.next_rand() as usize;
        let mut candidates: Vec<(usize, u32)> = self
            .warehouses
            .iter()
            .enumerate()
            .filter(|(_, w)| w.active && w.owner < 4)
            .map(|(i, w)| (i, free_trader_port_distance(trader, w)))
            .collect();
        if candidates.is_empty() {
            self.send_free_trader_to_edge(trader);
            return;
        }
        candidates.sort_by_key(|(idx, distance)| (*distance, *idx));
        candidates.truncate(12);
        let mut best_profit: Option<(usize, i32)> = None;
        for (idx, _) in &candidates {
            let wh = &self.warehouses[*idx];
            let owner_gold = self
                .players
                .get(wh.owner as usize)
                .map(|p| p.gold)
                .unwrap_or(0);
            let score = free_trader_port_profit_score(trader, wh, owner_gold);
            if score <= 0 {
                continue;
            }
            if best_profit.map_or(true, |(_, best)| score > best) {
                best_profit = Some((*idx, score));
            }
        }
        let pick = best_profit
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| candidates[r % candidates.len()].0);
        let wh = &self.warehouses[pick];
        trader.path.clear();
        trader.path_idx = 0;
        trader.path_required = false;
        if let Some(ocean) = self.ocean_map.as_ref() {
            let start = ocean.nearest_navigable(trader.world_x, trader.world_y);
            let goal = ocean.nearest_navigable(wh.tile_x as i32, wh.tile_y as i32);
            let (Some(start), Some(goal)) = (start, goal) else {
                self.send_free_trader_to_edge(trader);
                return;
            };
            let path = if start == goal {
                Some(Vec::new())
            } else {
                crate::ocean_map::find_ocean_path(ocean, start, goal)
            };
            let Some(path) = path else {
                self.send_free_trader_to_edge(trader);
                return;
            };
            trader.target_warehouse = Some(pick);
            trader.target_x = goal.0;
            trader.target_y = goal.1;
            trader.path = path;
            trader.path_required = true;
        } else {
            trader.target_warehouse = Some(pick);
            trader.target_x = wh.tile_x as i32;
            trader.target_y = wh.tile_y as i32;
        }
    }

    fn send_free_trader_to_edge(&self, trader: &mut crate::free_trader::FreeTrader) {
        trader.target_warehouse = None;
        trader.leaving = true;
        trader.path.clear();
        trader.path_idx = 0;
        trader.path_required = false;
        if let Some(ocean) = self.ocean_map.as_ref() {
            if let Some(((tx, ty), path)) =
                free_trader_departure_route_from_ocean(ocean, trader.world_x, trader.world_y)
            {
                trader.target_x = tx;
                trader.target_y = ty;
                trader.path = path;
                trader.path_required = true;
                return;
            }
        }

        let (tx, ty) =
            free_trader_departure_point_from_bounds(trader.world_x, trader.world_y, 200, 200);
        trader.target_x = tx;
        trader.target_y = ty;
    }

    /// Advance the local type-4 land-figure model once per simulation step.
    ///
    /// `FUN_00451890` receives the dispatcher delta on every engine update,
    /// accumulates it on each active figure, and dispatches type 4 through
    /// `FUN_00456d00`. This keeps local source-slot removal on the same
    /// per-step path. The combat and order rules below remain the local model
    /// until the full type-4 state machine is reconstructed.
    fn tick_source_land_figures(&mut self, dt_ms: u32) {
        self.assign_native_idle_targets();
        self.apply_source_kind4_state_payload_targets();
        let source_candidates = self.source_combat_candidates();
        self.dispatch_source_kind4_live_candidate_actions(&source_candidates);
        let source_kind4_kind1_arrivals = self
            .military_units
            .iter()
            .enumerate()
            .filter_map(|(index, unit)| {
                let descriptor = unit.source_target_descriptor?;
                (unit.source_runtime_slot.is_some() && descriptor.kind() == 1)
                    .then_some((index, descriptor))
            })
            .collect::<Vec<_>>();
        let source_kind4_medical_arrivals = self
            .military_units
            .iter()
            .enumerate()
            .filter_map(|(index, unit)| {
                let descriptor = unit.source_target_descriptor?;
                (unit.source_runtime_slot.is_some() && matches!(descriptor.kind(), 0x32 | 0x33))
                    .then_some((index, descriptor))
            })
            .collect::<Vec<_>>();
        let source_kind4_live_targets = self
            .military_units
            .iter()
            .filter_map(|unit| unit.source_target_descriptor)
            .filter_map(|descriptor| {
                combat::source_figure_target_rect(descriptor, &source_candidates, |descriptor| {
                    self.source_kind6_target_rect(descriptor)
                })
                .map(|target| (descriptor, target))
            })
            .collect::<Vec<_>>();
        let mut source_rand = std::mem::take(&mut self.rng_state);
        combat::tick_unit_orders_with_maps_and_source_rand_and_dispatch_state_and_target_resolver(
            &mut self.military_units,
            dt_ms,
            self.ocean_map.as_ref(),
            &self.island_maps,
            &mut source_rand,
            self.source_kind4_dispatch,
            |descriptor| {
                source_kind4_live_targets
                    .iter()
                    .find_map(|(known, target)| (*known == descriptor).then_some(*target))
            },
        );
        self.rng_state = source_rand;
        let terminal_slots = self
            .military_units
            .iter()
            .filter(|unit| {
                unit.source_terminal_pending && !unit.active && unit.source_figure_kind == Some(4)
            })
            .filter_map(|unit| unit.source_runtime_slot)
            .collect::<Vec<_>>();
        for runtime_slot in terminal_slots {
            self.deactivate_source_kind4_figure(runtime_slot);
        }
        self.apply_source_kind4_kind1_arrivals(&source_kind4_kind1_arrivals, &source_candidates);
        self.apply_source_kind4_medical_arrivals(&source_kind4_medical_arrivals);
        self.apply_source_kind4_state_payload_targets();
        let dead = combat::tick_combat_with_maps(
            &mut self.military_units,
            &self.diplomacy,
            dt_ms,
            self.ocean_map.as_ref(),
            &self.island_maps,
        );
        let source_slots: Vec<u16> = dead
            .into_iter()
            .filter_map(|index| self.military_units.get(index)?.source_runtime_slot)
            .collect();
        for runtime_slot in source_slots {
            self.deactivate_source_kind4_figure(runtime_slot);
        }
        self.sync_source_kind4_occupants();
    }

    /// Dispatch direct category-4 attacks from `FUN_0045cd20`'s complete
    /// candidate population. `FUN_00456d00` sends the selected live row to
    /// `FUN_004546e0`; it does not install that descriptor as a land-route
    /// order. The executor records the source cooldown and queues one shared
    /// type-one hit for later `FUN_00445930` resolution.
    fn dispatch_source_kind4_live_candidate_actions(
        &mut self,
        candidates: &[combat::SourceCombatCandidate],
    ) {
        let selections = candidates
            .iter()
            .filter_map(|attacker| {
                let combat::SourceCombatCandidateEntity::MilitaryUnit(attacker_index) =
                    attacker.entity
                else {
                    return None;
                };
                let unit = self.military_units.get(attacker_index)?;
                if !unit.active
                    || unit.source_figure_kind != Some(4)
                    || unit.source_target_descriptor.is_some()
                    || !combat::source_kind4_route_program_is_terminal(unit)
                {
                    return None;
                }
                let occupant = self.source_kind4_occupants.iter().find(|occupant| {
                    occupant.active && occupant.runtime_slot == attacker.runtime_slot
                })?;
                if occupant.idle_timestamp_ticks > self.source_time_ticks {
                    return None;
                }
                let selected = combat::source_kind4_select_live_target(
                    attacker,
                    candidates,
                    &self.diplomacy,
                    |descriptor| self.source_kind6_target_rect(descriptor),
                )?;
                let target = combat::source_figure_target_rect(
                    selected.target_descriptor,
                    candidates,
                    |descriptor| self.source_kind6_target_rect(descriptor),
                )?;
                let terrain_clear = unit.source_island_id.map_or(true, |island_id| {
                    self.island_maps
                        .iter()
                        .find(|map| map.island_id == island_id)
                        .map_or(true, |map| {
                            map.source_kind4_line_of_fire_clear(
                                (unit.tile_x, unit.tile_y),
                                target.origin,
                            )
                        })
                });
                terrain_clear.then(|| {
                    Some((
                        attacker_index,
                        attacker.runtime_slot,
                        combat::source_kind4_action(attacker, selected)?,
                        combat::source_kind4_action_ready_at(self.source_time_ticks, attacker)?,
                        combat::source_kind4_impact_due_at(self.source_time_ticks, attacker)?,
                    ))
                })?
            })
            .collect::<Vec<_>>();

        for (attacker_index, runtime_slot, action, ready_at, impact_due_at) in selections {
            let Some(unit) = self.military_units.get_mut(attacker_index) else {
                continue;
            };
            if !unit.active
                || unit.source_target_descriptor.is_some()
                || unit.source_runtime_slot != Some(runtime_slot)
            {
                continue;
            }
            unit.direction = action.direction;
            unit.source_idle_remaining = 0.0;
            unit.clear_source_route_program();
            unit.combat_target = -1;
            if let Some(occupant) = self
                .source_kind4_occupants
                .iter_mut()
                .find(|occupant| occupant.active && occupant.runtime_slot == runtime_slot)
            {
                occupant.idle_timestamp_ticks = ready_at;
            }
            self.source_kind4_actions.push(action);
            if self.source_deferred_event_count() < combat::SOURCE_KIND6_DEFERRED_HIT_CAPACITY {
                self.source_kind4_deferred_hits
                    .push(combat::SourceKind4DeferredHit {
                        due_at: impact_due_at,
                        action,
                    });
            }
        }
    }

    /// Consume the kind-1 terminal branch of `FUN_00456d00`. Its target is a
    /// same-owner SHIP4 figure; `FUN_00458000` first requires a free packed
    /// cargo entry, emits `0x849` to fill the cargo table, then schedules the
    /// category-4 kind-5 payload `7` deletion of the boarding land figure.
    fn apply_source_kind4_kind1_arrivals(
        &mut self,
        arrivals: &[(usize, SourceTargetDescriptor)],
        candidates: &[combat::SourceCombatCandidate],
    ) {
        for &(actor_index, descriptor) in arrivals {
            let Some(actor) = self.military_units.get(actor_index).cloned() else {
                continue;
            };
            if !actor.active || actor.source_target_descriptor.is_some() {
                continue;
            }
            let Some(target) = candidates.iter().copied().find(|candidate| {
                candidate.figure_kind == 1
                    && candidate.source_kind6_action_target_descriptor() == Some(descriptor)
                    && candidate.owner == actor.owner
            }) else {
                continue;
            };
            let Some((ware, quantity)) = combat::source_kind4_boarding_cargo(&actor) else {
                continue;
            };

            let boarded = match target.entity {
                combat::SourceCombatCandidateEntity::MilitaryUnit(target_index) => self
                    .military_units
                    .get_mut(target_index)
                    .filter(|unit| {
                        unit.active
                            && combat::source_ship_cargo_has_free_slot(&unit.source_cargo_slots)
                    })
                    .map(|unit| {
                        combat::source_add_ship_cargo(
                            &mut unit.source_cargo_slots,
                            ware,
                            combat::SOURCE_KIND4_BOARDING_CARGO_METADATA,
                            quantity,
                        );
                    })
                    .is_some(),
                combat::SourceCombatCandidateEntity::TradeShip(target_index) => self
                    .trade_ships
                    .get_mut(target_index)
                    .filter(|ship| {
                        ship.active
                            && combat::source_ship_cargo_has_free_slot(&ship.source_cargo_slots)
                    })
                    .map(|ship| {
                        combat::source_add_ship_cargo(
                            &mut ship.source_cargo_slots,
                            ware,
                            combat::SOURCE_KIND4_BOARDING_CARGO_METADATA,
                            quantity,
                        );
                    })
                    .is_some(),
                combat::SourceCombatCandidateEntity::DynamicFigure(target_index) => self
                    .source_dynamic_combat_figures
                    .get_mut(target_index)
                    .filter(|figure| {
                        figure.active
                            && combat::source_ship_cargo_has_free_slot(&figure.source_cargo_slots)
                    })
                    .map(|figure| {
                        combat::source_add_ship_cargo(
                            &mut figure.source_cargo_slots,
                            ware,
                            combat::SOURCE_KIND4_BOARDING_CARGO_METADATA,
                            quantity,
                        );
                    })
                    .is_some(),
            };
            if boarded {
                if let Some(runtime_slot) = actor.source_runtime_slot {
                    self.deactivate_source_kind4_figure(runtime_slot);
                }
            }
        }
    }

    /// `FUN_00458100`: a type-4 figure completing a static kind-`0x32` or
    /// kind-`0x33` route to a production-kind-22 Klinik is removed at once.
    /// A free shared deferred-event record preserves its definition, island,
    /// continuous origin, and five-step reverse-route destination for the
    /// type-three consumer 100 source ticks later.
    fn apply_source_kind4_medical_arrivals(
        &mut self,
        arrivals: &[(usize, SourceTargetDescriptor)],
    ) {
        for &(actor_index, descriptor) in arrivals {
            let Some(actor) = self.military_units.get(actor_index).cloned() else {
                continue;
            };
            if !actor.active || actor.source_target_descriptor.is_some() {
                continue;
            }
            let is_klinik_target = self.source_static_map_roots.iter().any(|cell| {
                cell.matches(
                    descriptor.bytes()[1],
                    u16::from(descriptor.bytes()[2]),
                    u16::from(descriptor.bytes()[3]),
                ) && cell.source_production_kind_code == 0x16
            });
            if !is_klinik_target {
                continue;
            }

            if self.source_deferred_event_count() < combat::SOURCE_KIND6_DEFERRED_HIT_CAPACITY {
                if let Some(figure_definition_id) = actor.source_figure_definition_id {
                    let (offset_x, offset_y) = combat::source_kind4_reverse_route_offset(
                        &actor.source_route_program,
                        actor.source_route_program_cursor,
                        5,
                    );
                    let origin = if actor.source_position_initialized {
                        (actor.source_position_x, actor.source_position_y)
                    } else {
                        (
                            actor.tile_x as f32 * 0.5 + 0.25,
                            actor.tile_y as f32 * 0.5 + 0.25,
                        )
                    };
                    let target_x = actor.tile_x.wrapping_add(offset_x);
                    let target_y = actor.tile_y.wrapping_add(offset_y);
                    self.source_kind4_deferred_relocations
                        .push(SourceKind4DeferredRelocation {
                            due_at: self.source_time_ticks.wrapping_add(100),
                            island_id: actor.source_island_id.unwrap_or(descriptor.bytes()[1]),
                            figure_definition_id,
                            origin,
                            target_descriptor: SourceTargetDescriptor::from_bytes([
                                SourceTargetDescriptor::FIXED_POINT_COORDINATE_KIND,
                                (((target_y >> 8) as u8 & 0x0f) << 4)
                                    | ((target_x >> 8) as u8 & 0x0f),
                                target_x as u8,
                                target_y as u8,
                            ]),
                        });
                }
            }
            if let Some(runtime_slot) = actor.source_runtime_slot {
                self.deactivate_source_kind4_figure(runtime_slot);
            }
        }
    }

    /// Replay `FUN_00458190` and its category-4 `0x2a` consumer. After a
    /// type-4 figure has been targetless for 20 source ticks, state bit zero
    /// advances the stored selector before testing one of the two four-byte
    /// `SOLDAT3 +0x1e` descriptors. Bit one blocks the selector's wrap from
    /// entry one to entry zero; a failed scan clears only state bit zero.
    fn apply_source_kind4_state_payload_targets(&mut self) {
        let updates: Vec<(u16, Option<(u8, SourceTargetDescriptor)>)> = self
            .source_kind4_occupants
            .iter()
            .filter(|occupant| {
                occupant.active
                    && occupant.state_flags & 1 != 0
                    && self
                        .source_time_ticks
                        .wrapping_sub(occupant.idle_timestamp_ticks)
                        >= SOURCE_KIND4_IDLE_TARGET_DELAY_TICKS
                    && self.military_units.iter().any(|unit| {
                        unit.active
                            && unit.source_runtime_slot == Some(occupant.runtime_slot)
                            && unit.source_target_descriptor.is_none()
                    })
            })
            .map(|occupant| {
                let mut selector = occupant.state_selector;
                for _ in 0..2 {
                    selector = selector.wrapping_add(1);
                    if selector > 1 {
                        if occupant.state_flags & 2 != 0 {
                            return (occupant.runtime_slot, None);
                        }
                        selector = 0;
                    }
                    let offset = usize::from(selector) * 4;
                    if occupant.state_payload[offset] != 0 {
                        return (
                            occupant.runtime_slot,
                            Some((
                                selector,
                                SourceTargetDescriptor::from_bytes([
                                    occupant.state_payload[offset],
                                    occupant.state_payload[offset + 1],
                                    occupant.state_payload[offset + 2],
                                    occupant.state_payload[offset + 3],
                                ]),
                            )),
                        );
                    }
                }
                (occupant.runtime_slot, None)
            })
            .collect();

        for (runtime_slot, selection) in updates {
            match selection {
                Some((selector, descriptor)) => {
                    if let Some(unit) = self.military_units.iter_mut().find(|unit| {
                        unit.active
                            && unit.source_runtime_slot == Some(runtime_slot)
                            && unit.source_target_descriptor.is_none()
                    }) {
                        unit.source_target_descriptor = Some(descriptor);
                        unit.source_route_retry_count = 0;
                    }
                    if let Some(occupant) = self
                        .source_kind4_occupants
                        .iter_mut()
                        .find(|occupant| occupant.active && occupant.runtime_slot == runtime_slot)
                    {
                        occupant.state_selector = selector;
                        occupant.state_descriptor = descriptor;
                        occupant.route_retry_count = 0;
                    }
                }
                None => {
                    if let Some(occupant) = self
                        .source_kind4_occupants
                        .iter_mut()
                        .find(|occupant| occupant.active && occupant.runtime_slot == runtime_slot)
                    {
                        occupant.state_flags &= !1;
                    }
                }
            }
        }
    }

    /// Replay the idle-target branch in `FUN_00456d00` for slot-6 SPEER
    /// figures. Once a coordinate target is reached, the source clears it;
    /// each idle dispatch then has a one-in-four gate and up to five bounded
    /// anchor-relative attempts at offsets `9 - 2 * (rand() % 9)`. The
    /// source command scheduler first defers the native SPEER slot by its
    /// `Worktime: 0.8`, or eight source ticks.
    fn assign_native_idle_targets(&mut self) {
        for unit in &mut self.military_units {
            if unit.unit_type == crate::combat::UnitType::NativeSpearman
                && unit.owner == 6
                && unit
                    .source_target_descriptor
                    .is_some_and(|descriptor| descriptor.kind() == 0x34)
                && (unit.tile_x, unit.tile_y) == (unit.target_x, unit.target_y)
            {
                unit.source_target_descriptor = None;
            }
        }

        let idle_units: Vec<(usize, u8, (i32, i32))> = self
            .military_units
            .iter()
            .enumerate()
            .filter(|(_, unit)| {
                unit.is_alive()
                    && unit.unit_type == crate::combat::UnitType::NativeSpearman
                    && unit.owner == 6
                    && unit.source_target_descriptor.is_none()
                    && (unit.tile_x, unit.tile_y) == (unit.target_x, unit.target_y)
                    && unit.source_runtime_slot.is_some_and(|runtime_slot| {
                        self.source_kind4_occupants.iter().any(|occupant| {
                            occupant.active
                                && occupant.runtime_slot == runtime_slot
                                && occupant.state_flags & 1 == 0
                                && self
                                    .source_time_ticks
                                    .wrapping_sub(occupant.idle_timestamp_ticks)
                                    >= SOURCE_KIND4_IDLE_TARGET_DELAY_TICKS
                        })
                    })
            })
            .filter_map(|(index, unit)| {
                let island_id = unit.source_island_id?;
                let anchor = unit.source_origin_descriptor?;
                self.island_maps
                    .iter()
                    .find(|map| map.island_id == island_id)
                    .and_then(|map| map.source_land_idle_anchor(anchor))
                    .map(|anchor| (index, island_id, anchor))
            })
            .collect();

        for (index, island_id, anchor) in idle_units {
            if self.next_source_rand() & 3 != 0 {
                continue;
            }
            for _ in 0..5 {
                let offset_x = 9 - 2 * i32::from(self.next_source_rand() % 9);
                let offset_y = 9 - 2 * i32::from(self.next_source_rand() % 9);
                let target = self
                    .island_maps
                    .iter()
                    .find(|map| map.island_id == island_id)
                    .and_then(|map| {
                        let (anchor_x, anchor_y) = map.source_world_to_local(anchor)?;
                        let local = (
                            (anchor_x + offset_x).clamp(0, i32::from(map.width) - 1),
                            (anchor_y + offset_y).clamp(0, i32::from(map.height) - 1),
                        );
                        if !map.source_native_idle_target_allowed(local) {
                            return None;
                        }
                        Some((local, map.local_to_source_world(local)?))
                    });
                let Some((target_local, target)) = target else {
                    continue;
                };
                let (Ok(target_x), Ok(target_y)) =
                    (u8::try_from(target_local.0), u8::try_from(target_local.1))
                else {
                    continue;
                };
                let descriptor = SourceTargetDescriptor::from_source_kind34_island_cell(
                    island_id, target_x, target_y,
                );
                let runtime_slot = {
                    let unit = &mut self.military_units[index];
                    unit.target_x = target.0;
                    unit.target_y = target.1;
                    unit.source_target_descriptor = Some(descriptor);
                    unit.source_route_retry_count = 0;
                    unit.source_idle_remaining = 0.0;
                    unit.clear_source_route_program();
                    unit.source_runtime_slot
                };
                if let Some(runtime_slot) = runtime_slot {
                    if let Some(occupant) = self
                        .source_kind4_occupants
                        .iter_mut()
                        .find(|occupant| occupant.active && occupant.runtime_slot == runtime_slot)
                    {
                        occupant.idle_timestamp_ticks = self.source_time_ticks.wrapping_add(8);
                        occupant.route_retry_count = 0;
                        occupant.idle_remaining_bits = 0;
                        occupant.route_program =
                            crate::combat::default_source_kind4_route_program();
                        occupant.route_program_cursor = 0;
                    }
                }
                break;
            }
        }
    }

    /// Advance `DAT_005b6040` exactly as `FUN_00489670`: source time gains
    /// one integer tick for every accumulated 100 ms of scaled dispatcher
    /// input.
    fn advance_source_clock(&mut self, dt_ms: u32) {
        let elapsed = self.source_time_remainder_ms.saturating_add(dt_ms);
        self.source_time_ticks = self.source_time_ticks.wrapping_add(elapsed / 100);
        self.source_time_remainder_ms = elapsed % 100;
    }

    /// Keep the type-4 slot table aligned with its live combat entity. The
    /// executable indexes both records by the `SOLDAT3` runtime slot, so a
    /// moved source figure must not retain its load-time occupancy position.
    fn sync_source_kind4_occupants(&mut self) {
        let unit_state: Vec<(
            u16,
            (u16, u16),
            u8,
            Option<SourceTargetDescriptor>,
            u8,
            [u8; crate::combat::SOURCE_KIND4_ROUTE_PROGRAM_CAPACITY],
            u8,
            u32,
        )> = self
            .military_units
            .iter()
            .filter(|unit| unit.is_alive())
            .filter_map(|unit| {
                Some((
                    unit.source_runtime_slot?,
                    (
                        u16::try_from(unit.tile_x).ok()?,
                        u16::try_from(unit.tile_y).ok()?,
                    ),
                    unit.direction,
                    unit.source_target_descriptor,
                    unit.source_route_retry_count,
                    unit.source_route_program,
                    unit.source_route_program_cursor,
                    unit.source_idle_remaining.to_bits(),
                ))
            })
            .collect();
        for occupant in self
            .source_kind4_occupants
            .iter_mut()
            .filter(|occupant| occupant.active)
        {
            let Some((
                _,
                position,
                direction,
                target_descriptor,
                route_retry_count,
                route_program,
                route_program_cursor,
                idle_remaining_bits,
            )) = unit_state
                .iter()
                .find(|(runtime_slot, _, _, _, _, _, _, _)| *runtime_slot == occupant.runtime_slot)
            else {
                continue;
            };
            occupant.position = *position;
            occupant.direction = *direction;
            occupant.state_descriptor =
                target_descriptor.unwrap_or(SourceTargetDescriptor::from_bytes([0; 4]));
            occupant.route_retry_count = *route_retry_count;
            occupant.route_program = *route_program;
            occupant.route_program_cursor = *route_program_cursor;
            occupant.idle_remaining_bits = *idle_remaining_bits;
        }
    }

    /// Execute one native type-17 terrain-event dispatch interval. The event
    /// table remains authoritative for its lifecycle and selected target;
    /// the generic figure only supplies its continuous map position and
    /// animation state to `FUN_0045bfc0`'s equivalent branches.
    fn tick_source_terrain_event_figure(&mut self, figure_index: usize, dt_ms: u32) -> bool {
        let Some(figure) = self.figures.get(figure_index).cloned() else {
            return false;
        };
        if !figure.is_active()
            || !figure.source_terrain_event_active
            || figure.move_timer_ms.saturating_add(dt_ms) < 100
        {
            return false;
        }
        let Some(slot) = figure.source_event_slot else {
            return true;
        };
        let Some(event) = self.source_figure_events.slot(slot) else {
            return true;
        };
        if event.is_free() {
            return true;
        }
        let Some(map_index) = self
            .island_maps
            .iter()
            .position(|map| map.island_id == figure.origin_island)
        else {
            return true;
        };
        let (world_origin, width, height, resource_state, attenuation) = {
            let map = &self.island_maps[map_index];
            (
                (
                    map.source_world_origin.0.div_euclid(2),
                    map.source_world_origin.1.div_euclid(2),
                ),
                i32::from(map.width),
                i32::from(map.height),
                map.source_resource_state(),
                map.source_resource_attenuation(),
            )
        };
        let target = (event.target_x, event.target_y);
        let target_local = (target.0 != -1 && target.1 != -1)
            .then(|| {
                (
                    i32::from(target.0) - world_origin.0,
                    i32::from(target.1) - world_origin.1,
                )
            })
            .filter(|target| {
                target.0 >= 0 && target.1 >= 0 && target.0 < width && target.1 < height
            });
        let route_is_terminal = self
            .source_figure_events
            .kind12_is_terminal(slot)
            .unwrap_or(true);

        if route_is_terminal {
            if target_local == Some((figure.tile_x, figure.tile_y)) {
                match event.lifecycle {
                    0 => {
                        if let Some(figure) = self.figures.get_mut(figure_index) {
                            figure.move_timer_ms = figure.move_timer_ms.saturating_add(dt_ms) - 100;
                            figure.path.clear();
                            figure.path_idx = 0;
                            figure.source_step_remaining = 0.0;
                            figure.select_source_animation_state(1);
                        }
                        self.source_figure_events.set_lifecycle(slot, 1);
                    }
                    1 => {
                        let local = target_local.expect("terminal target is present");
                        let mut changed = false;
                        if let Some(cell) = self.source_static_map_roots.iter_mut().find(|cell| {
                            cell.matches(
                                figure.origin_island,
                                u16::try_from(local.0).unwrap_or(u16::MAX),
                                u16::try_from(local.1).unwrap_or(u16::MAX),
                            )
                        }) {
                            let transition = source_resource_harvest_transition(
                                resource_state.resource_strength(cell.source_output_ware_slot),
                                cell.source_resource_growth_factor,
                                attenuation,
                                cell.source_output_ware_slot,
                                figure.origin_island,
                                u16::try_from(local.0).unwrap_or(u16::MAX),
                                u16::try_from(local.1).unwrap_or(u16::MAX),
                            );
                            changed = cell.replace_harvested_raw_resource(transition);
                        }
                        if changed {
                            self.source_map_cell_revision =
                                self.source_map_cell_revision.wrapping_add(1);
                        }
                        self.source_figure_events.finish_terrain_harvest(slot);
                        self.source_figure_events.clear_terrain_target(slot);
                        if let Some(figure) = self.figures.get_mut(figure_index) {
                            figure.move_timer_ms = figure.move_timer_ms.saturating_add(dt_ms) - 100;
                            figure.path.clear();
                            figure.path_idx = 0;
                            figure.source_step_remaining = 0.0;
                            figure.select_source_animation_state(0);
                        }
                    }
                    2 => {
                        let local = target_local.expect("terminal target is present");
                        if let Some(cell) = self.source_static_map_roots.iter_mut().find(|cell| {
                            cell.matches(
                                figure.origin_island,
                                u16::try_from(local.0).unwrap_or(u16::MAX),
                                u16::try_from(local.1).unwrap_or(u16::MAX),
                            )
                        }) {
                            let transition = source_resource_harvest_transition(
                                resource_state.resource_strength(cell.source_output_ware_slot),
                                cell.source_resource_growth_factor,
                                attenuation,
                                cell.source_output_ware_slot,
                                figure.origin_island,
                                u16::try_from(local.0).unwrap_or(u16::MAX),
                                u16::try_from(local.1).unwrap_or(u16::MAX),
                            );
                            if cell.replace_harvested_raw_resource(transition) {
                                self.source_map_cell_revision =
                                    self.source_map_cell_revision.wrapping_add(1);
                            }
                        }
                        self.defer_source_terrain_event(
                            figure.origin_island,
                            figure.origin_x as u8,
                            figure.origin_y as u8,
                        );
                        return true;
                    }
                    _ => return true,
                }
                return false;
            }

            let start = (figure.tile_x, figure.tile_y);
            let Some((route, path, target_world)) = (|| {
                let map = self.island_maps.get(map_index)?;
                let mut grid = map.source_type17_terrain_path_grid(
                    start,
                    event.resource_ware_slot,
                    &self.source_static_map_roots,
                );
                let route = grid.search_source_high_metadata_target(start, 0xc0).ok()?;
                let path = source_route_positions(start, &route.steps)?;
                (path.last().copied() == Some(route.position)).then_some((
                    route,
                    path,
                    (
                        world_origin.0.checked_add(route.position.0)?,
                        world_origin.1.checked_add(route.position.1)?,
                    ),
                ))
            })() else {
                let current_world = (
                    world_origin.0.saturating_add(start.0),
                    world_origin.1.saturating_add(start.1),
                );
                let (Ok(x), Ok(y)) = (
                    i16::try_from(current_world.0),
                    i16::try_from(current_world.1),
                ) else {
                    return true;
                };
                self.source_figure_events.set_lifecycle(slot, 2);
                self.source_figure_events
                    .set_terrain_target(slot, (x, y), 0);
                self.remove_source_terrain_event(
                    figure.origin_island,
                    figure.origin_x as u8,
                    figure.origin_y as u8,
                );
                if let Some(figure) = self.figures.get_mut(figure_index) {
                    figure.move_timer_ms = figure.move_timer_ms.saturating_add(dt_ms) - 100;
                    figure.select_source_animation_state(2);
                }
                return false;
            };
            let (Ok(target_x), Ok(target_y)) =
                (i16::try_from(target_world.0), i16::try_from(target_world.1))
            else {
                return true;
            };
            if let Some(cell) = self.source_static_map_roots.iter_mut().find(|cell| {
                cell.matches(
                    figure.origin_island,
                    u16::try_from(route.position.0).unwrap_or(u16::MAX),
                    u16::try_from(route.position.1).unwrap_or(u16::MAX),
                )
            }) {
                cell.source_resource_reserved = true;
                self.source_map_cell_revision = self.source_map_cell_revision.wrapping_add(1);
            }
            if !self.source_figure_events.set_terrain_target(
                slot,
                (target_x, target_y),
                // `FUN_00471c50` returns its fixed success code `0x20`;
                // the wave distance only controls which high-metadata cell
                // it selects. `FUN_0045c270` accumulates that return value.
                0x20,
            ) || !self
                .source_figure_events
                .write_terrain_route(slot, &route.steps)
            {
                return true;
            }
            if let Some(figure) = self.figures.get_mut(figure_index) {
                figure.move_timer_ms = figure.move_timer_ms.saturating_add(dt_ms) - 100;
                figure.target_x = route.position.0;
                figure.target_y = route.position.1;
                figure.path = path;
                figure.path_idx = 0;
                figure.source_step_remaining = 0.0;
                figure.source_event_route_steps = route.steps;
                figure.select_source_animation_state(0);
            }
            return false;
        }

        let terrain_wegspeed = self.island_maps[map_index]
            .civilian_movement_speed((figure.tile_x, figure.tile_y))
            .unwrap_or(100);
        let next = figure
            .path
            .get(figure.path_idx)
            .copied()
            .unwrap_or((figure.target_x, figure.target_y));
        let moving = next != (figure.tile_x, figure.tile_y);
        let civilian_config = self.civilian_config;
        let figure = &mut self.figures[figure_index];
        figure.move_timer_ms = figure.move_timer_ms.saturating_add(dt_ms) - 100;
        let frame_duration_ms = carrier::source_animation_frame_duration_ms(
            civilian_config.frame_speed_for(figure) as u16,
            terrain_wegspeed,
            moving,
        );
        figure.advance_source_animation(100, frame_duration_ms, civilian_config.frames_per_dir);
        carrier::advance_source_carrier(figure, 100, terrain_wegspeed);
        self.source_figure_events
            .set_kind12_route_progress(slot, figure.path_idx);
        false
    }

    fn tick_entities(&mut self, dt_ms: u32) {
        // Refresh escort targets so warships stay glued to their assigned
        // trade ship before move orders are stepped.
        let positions: Vec<(bool, i32, i32)> = self
            .trade_ships
            .iter()
            .map(|s| (s.active, s.world_x, s.world_y))
            .collect();
        combat::tick_escort_targets(&mut self.military_units, &positions);
        // Trickle construction materials from the player's warehouses,
        // then decrement the construction timer only once the materials
        // for this building are done.
        use crate::types::Good;
        let mut take_one = |island_id: u8, owner: u8, good: Good| -> bool {
            for w in self
                .warehouses
                .iter_mut()
                .filter(|w| w.active && w.owner == owner && w.island_id == island_id)
            {
                if w.withdraw(good, 1) > 0 {
                    return true;
                }
            }
            false
        };
        for i in 0..self.buildings.len() {
            if self.buildings[i].is_built() {
                continue;
            }
            let b = &mut self.buildings[i];
            // Try to consume one unit of each pending material from a
            // warehouse on the same island. If a line is short, that
            // material simply isn't drawn this tick — construction stalls.
            if b.wood_needed > 0 && take_one(b.island_id, b.owner, Good::Wood) {
                b.wood_needed -= 1;
            }
            if b.tools_needed > 0 && take_one(b.island_id, b.owner, Good::Tools) {
                b.tools_needed -= 1;
            }
            if b.bricks_needed > 0 && take_one(b.island_id, b.owner, Good::Bricks) {
                b.bricks_needed -= 1;
            }
            // Construction time only flows once all materials are met.
            if b.wood_needed == 0
                && b.tools_needed == 0
                && b.bricks_needed == 0
                && b.construction_ms_remaining > 0
            {
                b.construction_ms_remaining = b.construction_ms_remaining.saturating_sub(dt_ms);
            }
        }

        let mut despawn_indices = Vec::new();
        let mut source_worker_harvests = Vec::new();
        let carrier_config = self.carrier_config;
        let city_cart_config = self.city_cart_config;
        let civilian_config = self.civilian_config;

        let terrain_event_indices: Vec<_> = self
            .figures
            .iter()
            .enumerate()
            .filter_map(|(index, figure)| {
                (figure.is_active() && figure.source_terrain_event_active).then_some(index)
            })
            .collect();
        for index in terrain_event_indices {
            if self.tick_source_terrain_event_figure(index, dt_ms) {
                despawn_indices.push(index);
            }
        }

        let searching_worker_indices: Vec<_> = self
            .figures
            .iter()
            .enumerate()
            .filter_map(|(index, figure)| {
                (figure.is_active()
                    && figure.speed > 0
                    && figure.move_timer_ms.saturating_add(dt_ms) >= 100
                    && figure.source_worker_route == SourceWorkerRoute::Searching)
                    .then_some(index)
            })
            .collect();
        for index in searching_worker_indices {
            self.try_assign_source_plantation_worker_target(index);
        }
        let returning_worker_indices: Vec<_> = self
            .figures
            .iter()
            .enumerate()
            .filter_map(|(index, figure)| {
                (figure.is_active()
                    && figure.speed > 0
                    && figure.move_timer_ms.saturating_add(dt_ms) >= 100
                    && figure.source_worker_route == SourceWorkerRoute::ReturningSearch)
                    .then_some(index)
            })
            .collect();
        for index in returning_worker_indices {
            self.try_assign_source_plantation_worker_return_route(index);
        }

        for (idx, figure) in self.figures.iter_mut().enumerate() {
            if !figure.is_active() {
                continue;
            }
            if figure.source_terrain_event_active {
                continue;
            }

            figure.move_timer_ms += dt_ms;
            while figure.speed > 0 && figure.move_timer_ms >= 100 {
                figure.move_timer_ms -= 100;

                if figure.source_worker_route == SourceWorkerRoute::Harvesting {
                    let Some(map) = self
                        .island_maps
                        .iter()
                        .find(|map| map.island_id == figure.origin_island)
                    else {
                        despawn_indices.push(idx);
                        continue;
                    };
                    let target = (figure.supplier_x, figure.supplier_y);
                    if let Some(cell) = self.source_static_map_roots.iter().find(|cell| {
                        cell.matches(figure.origin_island, target.0, target.1)
                            && cell.source_resource_reserved
                    }) {
                        source_worker_harvests.push((
                            figure.origin_island,
                            target.0,
                            target.1,
                            source_resource_harvest_transition(
                                map.source_resource_strength(figure.carried_good),
                                cell.source_resource_growth_factor,
                                map.source_resource_attenuation(),
                                figure.carried_good,
                                figure.origin_island,
                                target.0,
                                target.1,
                            ),
                        ));
                    }
                    figure.action = ActionType::Walking;
                    figure.target_x = figure.source_worker_home_x;
                    figure.target_y = figure.source_worker_home_y;
                    figure.source_worker_route = SourceWorkerRoute::ReturningSearch;
                    figure.path.clear();
                    figure.path_idx = 0;
                    figure.source_step_remaining = 0.0;
                    figure.source_event_route_steps.clear();
                    if let Some(slot) = figure.source_event_slot {
                        self.source_figure_events
                            .write_plantation_route(slot, &figure.source_event_route_steps);
                    }
                    figure.select_source_animation_state(0);
                    continue;
                }

                if figure.source_worker_route == SourceWorkerRoute::ReturningSearch {
                    continue;
                }

                match figure.action {
                    ActionType::CarryingGoods | ActionType::Returning => {
                        let terrain_wegspeed = if figure.cargo_route == CargoRoute::CityCart {
                            self.island_maps
                                .iter()
                                .find(|map| map.island_id == figure.origin_island)
                                .and_then(|map| {
                                    map.city_cart_movement_speed((figure.tile_x, figure.tile_y))
                                })
                        } else {
                            self.buildings
                                .get(figure.building_idx as usize)
                                .and_then(|building| {
                                    self.island_maps
                                        .iter()
                                        .find(|map| map.island_id == building.island_id)
                                })
                                .and_then(|map| {
                                    map.carrier_movement_speed((figure.tile_x, figure.tile_y))
                                })
                        }
                        .unwrap_or(100);
                        let (frame_speed_ms, frames_per_direction) =
                            if figure.cargo_route == CargoRoute::CityCart {
                                (
                                    if figure.source_animation_frame_speed_ms == 0 {
                                        city_cart_config.frame_speed_ms
                                    } else {
                                        figure.source_animation_frame_speed_ms
                                    },
                                    if figure.source_animation_frames_per_direction == 0 {
                                        city_cart_config.frames_per_direction
                                    } else {
                                        figure.source_animation_frames_per_direction
                                    },
                                )
                            } else {
                                (
                                    carrier_config.frame_speed_ms,
                                    carrier_config.frames_per_direction,
                                )
                            };
                        let next = figure
                            .path
                            .get(figure.path_idx)
                            .copied()
                            .unwrap_or((figure.target_x, figure.target_y));
                        let moving = next != (figure.tile_x, figure.tile_y);
                        let frame_duration_ms = carrier::source_animation_frame_duration_ms(
                            frame_speed_ms,
                            terrain_wegspeed,
                            moving,
                        );
                        figure.advance_source_animation(
                            100,
                            frame_duration_ms,
                            frames_per_direction,
                        );
                        let arrived =
                            carrier::advance_source_carrier(figure, 100, terrain_wegspeed);
                        if figure.cargo_route == CargoRoute::CityCart {
                            if let Some(slot) = figure.source_event_slot {
                                self.source_figure_events
                                    .set_transfer_route_progress(slot, figure.path_idx);
                            }
                        }
                        if arrived {
                            if figure.cargo_route == CargoRoute::CityCart {
                                let arrival_action = figure.action;
                                let supplier_target = (figure.supplier_x, figure.supplier_y);
                                let supplier_kind = figure.destination_kind;
                                let good = carrier::good_from_u8(figure.carried_good);
                                let requested = figure.carried_amount;
                                let requested_fixed = if figure.cargo_fixed == 0 {
                                    requested.saturating_mul(32)
                                } else {
                                    figure.cargo_fixed
                                };
                                let max_load_fixed = if figure.source_transfer_max_load_fixed == 0 {
                                    requested_fixed
                                } else {
                                    figure.source_transfer_max_load_fixed
                                };
                                let origin = (
                                    figure.origin_island,
                                    figure.origin_x,
                                    figure.origin_y,
                                    figure.origin_kind,
                                );
                                let origin_production_kind = figure.origin_production_kind;
                                let origin_source_map_owner_slot =
                                    figure.origin_source_map_owner_slot;
                                let origin_city_owner = self.source_city_player_owner(
                                    figure.origin_island,
                                    origin_source_map_owner_slot,
                                );
                                let should_despawn = carrier::handle_arrival(
                                    figure,
                                    &self.buildings,
                                    &self.island_maps,
                                );

                                if !should_despawn {
                                    if let Some(slot) = figure.source_event_slot {
                                        self.source_figure_events.write_transfer_route(
                                            slot,
                                            &figure.source_event_route_steps,
                                        );
                                    }
                                }

                                match arrival_action {
                                    ActionType::CarryingGoods if should_despawn => {
                                        if let Some(state) =
                                            self.source_map_cell_states.iter_mut().find(|state| {
                                                state.kind_code == supplier_kind
                                                    && state.matches(
                                                        origin.0,
                                                        supplier_target.0,
                                                        supplier_target.1,
                                                    )
                                            })
                                        {
                                            state.release_storage_reservation(requested_fixed);
                                            self.source_map_cell_revision =
                                                self.source_map_cell_revision.wrapping_add(1);
                                        }
                                    }
                                    ActionType::CarryingGoods => {
                                        let supplier_idx =
                                            self.buildings.iter().position(|building| {
                                                building.active
                                                    && building.island_id == origin.0
                                                    && building.tile_x == supplier_target.0
                                                    && building.tile_y == supplier_target.1
                                                    && self
                                                        .building_defs
                                                        .get(building.def_id as usize)
                                                        .is_some_and(|definition| {
                                                            definition.output_good == good
                                                        })
                                            });
                                        let mut remaining_source_fill = None;
                                        let top_up = self
                                            .source_map_cell_states
                                            .iter_mut()
                                            .find(|state| {
                                                state.kind_code == supplier_kind
                                                    && state.matches(
                                                        origin.0,
                                                        supplier_target.0,
                                                        supplier_target.1,
                                                    )
                                            })
                                            .map(|state| {
                                                let top_up = state
                                                    .collect_reserved_storage_with_top_up(
                                                        requested_fixed,
                                                        max_load_fixed,
                                                    );
                                                remaining_source_fill = Some(state.storage_fill);
                                                top_up
                                            });
                                        if let Some(top_up) = top_up {
                                            let cargo_fixed =
                                                requested_fixed.saturating_add(top_up);
                                            if let Some(supplier_idx) = supplier_idx {
                                                self.buildings[supplier_idx].output_stock =
                                                    remaining_source_fill.unwrap_or(0) / 32;
                                            }
                                            figure.carried_amount = cargo_fixed / 32;
                                            figure.cargo_fixed = cargo_fixed;
                                            self.source_map_cell_revision =
                                                self.source_map_cell_revision.wrapping_add(1);
                                        } else {
                                            figure.carried_amount = 0;
                                            figure.cargo_fixed = 0;
                                        }
                                        let uncollected_fixed = top_up
                                            .is_none()
                                            .then_some(requested_fixed)
                                            .unwrap_or(0);
                                        if uncollected_fixed != 0 {
                                            if let Some(state) = self
                                                .source_map_cell_states
                                                .iter_mut()
                                                .find(|state| {
                                                    state.kind_code == supplier_kind
                                                        && state.matches(
                                                            origin.0,
                                                            supplier_target.0,
                                                            supplier_target.1,
                                                        )
                                                })
                                            {
                                                state
                                                    .release_storage_reservation(uncollected_fixed);
                                                self.source_map_cell_revision =
                                                    self.source_map_cell_revision.wrapping_add(1);
                                            }
                                        }
                                    }
                                    ActionType::Returning => {
                                        let delivered = figure.carried_amount;
                                        let delivered_fixed = figure
                                            .source_event_slot
                                            .and_then(|slot| {
                                                self.source_figure_events
                                                    .slot(slot)
                                                    .map(|event| event.transfer_amount_fixed)
                                            })
                                            .unwrap_or_else(|| {
                                                if figure.cargo_fixed == 0 {
                                                    delivered.saturating_mul(32)
                                                } else {
                                                    figure.cargo_fixed
                                                }
                                            });
                                        if delivered_fixed != 0 {
                                            let transfer_root_count = self
                                                .source_city_transfer_root_count(
                                                    origin.0,
                                                    origin_source_map_owner_slot,
                                                );
                                            let _accepted_fixed = if let Some(city_owner) =
                                                origin_city_owner
                                            {
                                                if let Some(warehouse) =
                                                    self.warehouses.iter_mut().find(|warehouse| {
                                                        warehouse.active
                                                            && warehouse.island_id == origin.0
                                                            && warehouse.owner == city_owner
                                                    })
                                                {
                                                    let city_capacity_fixed = warehouse
                                                        .city_storage_capacity_fixed(
                                                            transfer_root_count,
                                                        );
                                                    warehouse.deposit_city_good_fixed(
                                                        good,
                                                        delivered_fixed,
                                                        city_capacity_fixed,
                                                    )
                                                } else {
                                                    0
                                                }
                                            } else {
                                                0
                                            };
                                            if origin_production_kind == 7 {
                                                if let Some(state) = self
                                                    .source_map_cell_states
                                                    .iter_mut()
                                                    .find(|state| {
                                                        state.source_production_kind_code == 7
                                                            && state.matches(
                                                                origin.0, origin.1, origin.2,
                                                            )
                                                    })
                                                {
                                                    state.accept_market_transfer(delivered_fixed);
                                                    self.source_map_cell_revision = self
                                                        .source_map_cell_revision
                                                        .wrapping_add(1);
                                                }
                                            }
                                        }
                                        figure.destination_kind = 0;
                                        figure.carried_amount = 0;
                                        figure.cargo_fixed = 0;
                                    }
                                    _ => {}
                                }
                                if arrival_action == ActionType::CarryingGoods && !should_despawn {
                                    if let Some(slot) = figure.source_event_slot {
                                        self.source_figure_events
                                            .set_transfer_amount_fixed(slot, figure.cargo_fixed);
                                    }
                                }
                                if should_despawn {
                                    despawn_indices.push(idx);
                                    break;
                                }
                                continue;
                            }

                            let arrival_action = figure.action;
                            let supplier_target = (figure.supplier_x, figure.supplier_y);
                            let supplier_kind = figure.destination_kind;
                            let good = carrier::good_from_u8(figure.carried_good);
                            let requested = figure.carried_amount;
                            let requested_fixed = if figure.cargo_fixed == 0 {
                                requested.saturating_mul(32)
                            } else {
                                figure.cargo_fixed
                            };
                            let max_load_fixed = if figure.source_transfer_max_load_fixed == 0 {
                                carrier_config.max_load.saturating_mul(32)
                            } else {
                                figure.source_transfer_max_load_fixed
                            };
                            let origin_idx = figure.building_idx as usize;
                            let should_despawn =
                                carrier::handle_arrival(figure, &self.buildings, &self.island_maps);

                            match arrival_action {
                                ActionType::CarryingGoods => {
                                    let island = self
                                        .buildings
                                        .get(origin_idx)
                                        .map(|building| building.island_id);
                                    if should_despawn {
                                        if matches!(supplier_kind, 7 | 8) {
                                            if let Some(warehouse) =
                                                self.warehouses.iter_mut().find(|warehouse| {
                                                    island.is_some_and(|island| {
                                                        warehouse.island_id == island
                                                            && warehouse.owner == figure.owner
                                                            && warehouse.tile_x == supplier_target.0
                                                            && warehouse.tile_y == supplier_target.1
                                                    })
                                                })
                                            {
                                                warehouse.release_reservation(good, requested);
                                                warehouse.release_city_good_reservation_fixed(
                                                    good,
                                                    requested_fixed,
                                                );
                                            }
                                        } else if let Some(island) = island {
                                            if let Some(state) = self
                                                .source_map_cell_states
                                                .iter_mut()
                                                .find(|state| {
                                                    state.kind_code == supplier_kind
                                                        && state.matches(
                                                            island,
                                                            supplier_target.0,
                                                            supplier_target.1,
                                                        )
                                                })
                                            {
                                                state.release_storage_reservation(requested_fixed);
                                                self.source_map_cell_revision =
                                                    self.source_map_cell_revision.wrapping_add(1);
                                            }
                                        }
                                    } else {
                                        let supplier_idx =
                                            self.buildings.iter().position(|building| {
                                                building.active
                                                    && island.is_some_and(|island| {
                                                        building.island_id == island
                                                            && building.tile_x == supplier_target.0
                                                            && building.tile_y == supplier_target.1
                                                    })
                                                    && self
                                                        .building_defs
                                                        .get(building.def_id as usize)
                                                        .is_some_and(|definition| {
                                                            definition.output_good == good
                                                        })
                                            });
                                        let warehouse_idx = supplier_idx
                                            .is_none()
                                            .then(|| {
                                                self.warehouses.iter().position(|warehouse| {
                                                    warehouse.active
                                                        && island.is_some_and(|island| {
                                                            warehouse.island_id == island
                                                                && warehouse.owner == figure.owner
                                                                && warehouse.tile_x
                                                                    == supplier_target.0
                                                                && warehouse.tile_y
                                                                    == supplier_target.1
                                                        })
                                                })
                                            })
                                            .flatten();
                                        let picked = supplier_idx
                                            .map(|_| requested_fixed / 32)
                                            .or_else(|| {
                                                warehouse_idx.map(|warehouse_idx| {
                                                    requested.min(
                                                        self.warehouses[warehouse_idx].stock(good),
                                                    )
                                                })
                                            })
                                            .unwrap_or(0);
                                        let mut remaining_source_fill = None;
                                        let mut collected_fixed = requested_fixed;
                                        let collected = if picked == 0 {
                                            false
                                        } else if let Some(island) = island {
                                            if matches!(supplier_kind, 7 | 8) {
                                                self.warehouses
                                                    .iter_mut()
                                                    .find(|warehouse| {
                                                        warehouse.active
                                                            && warehouse.island_id == island
                                                            && warehouse.owner == figure.owner
                                                            && warehouse.tile_x == supplier_target.0
                                                            && warehouse.tile_y == supplier_target.1
                                                    })
                                                    .is_some_and(|warehouse| {
                                                        if !warehouse.collect_reserved(good, picked)
                                                        {
                                                            return false;
                                                        }
                                                        warehouse
                                                            .release_city_good_reservation_fixed(
                                                                good,
                                                                picked.saturating_mul(32),
                                                            );
                                                        true
                                                    })
                                            } else {
                                                self.source_map_cell_states
                                                    .iter_mut()
                                                    .find(|state| {
                                                        state.kind_code == supplier_kind
                                                            && state.matches(
                                                                island,
                                                                supplier_target.0,
                                                                supplier_target.1,
                                                            )
                                                    })
                                                    .map(|state| {
                                                        let top_up = state
                                                            .collect_reserved_storage_with_top_up(
                                                                requested_fixed,
                                                                max_load_fixed,
                                                            );
                                                        collected_fixed =
                                                            requested_fixed.saturating_add(top_up);
                                                        remaining_source_fill =
                                                            Some(state.storage_fill);
                                                    })
                                                    .is_some()
                                            }
                                        } else {
                                            false
                                        };
                                        if let Some(supplier_idx) = supplier_idx {
                                            if collected {
                                                self.buildings[supplier_idx].output_stock =
                                                    remaining_source_fill.unwrap_or(0) / 32;
                                                self.source_map_cell_revision =
                                                    self.source_map_cell_revision.wrapping_add(1);
                                                figure.carried_amount = collected_fixed / 32;
                                                figure.cargo_fixed = collected_fixed;
                                            } else {
                                                figure.carried_amount = 0;
                                                figure.cargo_fixed = 0;
                                            }
                                        } else if let Some(warehouse_idx) = warehouse_idx {
                                            if collected {
                                                self.source_map_cell_revision =
                                                    self.source_map_cell_revision.wrapping_add(1);
                                                figure.carried_amount = picked;
                                                figure.cargo_fixed = picked.saturating_mul(32);
                                            } else {
                                                figure.carried_amount = 0;
                                                figure.cargo_fixed = 0;
                                            }
                                        } else {
                                            figure.carried_amount = 0;
                                            figure.cargo_fixed = 0;
                                        }
                                        let uncollected_fixed =
                                            (!collected).then_some(requested_fixed).unwrap_or(0);
                                        if uncollected_fixed != 0 {
                                            if matches!(supplier_kind, 7 | 8) {
                                                if let Some(warehouse) =
                                                    self.warehouses.iter_mut().find(|warehouse| {
                                                        island.is_some_and(|island| {
                                                            warehouse.island_id == island
                                                                && warehouse.owner == figure.owner
                                                                && warehouse.tile_x
                                                                    == supplier_target.0
                                                                && warehouse.tile_y
                                                                    == supplier_target.1
                                                        })
                                                    })
                                                {
                                                    warehouse.release_reservation(
                                                        good,
                                                        uncollected_fixed / 32,
                                                    );
                                                    warehouse.release_city_good_reservation_fixed(
                                                        good,
                                                        uncollected_fixed,
                                                    );
                                                }
                                            } else if let Some(island) = island {
                                                if let Some(state) = self
                                                    .source_map_cell_states
                                                    .iter_mut()
                                                    .find(|state| {
                                                        state.kind_code == supplier_kind
                                                            && state.matches(
                                                                island,
                                                                supplier_target.0,
                                                                supplier_target.1,
                                                            )
                                                    })
                                                {
                                                    state.release_storage_reservation(
                                                        uncollected_fixed,
                                                    );
                                                    self.source_map_cell_revision = self
                                                        .source_map_cell_revision
                                                        .wrapping_add(1);
                                                }
                                            }
                                        }
                                    }
                                }
                                ActionType::Returning => {
                                    let delivered = figure.carried_amount;
                                    let delivered_fixed = if figure.cargo_fixed == 0 {
                                        delivered.saturating_mul(32)
                                    } else {
                                        figure.cargo_fixed
                                    };
                                    let inputs =
                                        self.buildings.get(origin_idx).and_then(|building| {
                                            self.building_defs.get(building.def_id as usize).map(
                                                |definition| {
                                                    (
                                                        definition.input_good_1,
                                                        definition.input_good_2,
                                                    )
                                                },
                                            )
                                        });
                                    if let Some((input_1, input_2)) = inputs {
                                        // FUN_0047d940 tests Workstoff first; if both
                                        // selectors name one ware, that ware fills work.
                                        let work_input = good == input_2;
                                        let (island, x, y, accepted) = {
                                            let building = &mut self.buildings[origin_idx];
                                            let accepted = if work_input {
                                                building.input_2_stock = building
                                                    .input_2_stock
                                                    .saturating_add(delivered);
                                                true
                                            } else if good == input_1 {
                                                building.input_1_stock = building
                                                    .input_1_stock
                                                    .saturating_add(delivered);
                                                true
                                            } else {
                                                false
                                            };
                                            (
                                                building.island_id,
                                                building.tile_x,
                                                building.tile_y,
                                                accepted,
                                            )
                                        };
                                        if accepted {
                                            if let Some(state) = self
                                                .source_map_cell_states
                                                .iter_mut()
                                                .find(|state| state.matches(island, x, y))
                                            {
                                                if work_input {
                                                    state.work_material_stock = state
                                                        .work_material_stock
                                                        .saturating_add(delivered_fixed);
                                                } else {
                                                    state.raw_material_stock = state
                                                        .raw_material_stock
                                                        .saturating_add(delivered_fixed);
                                                }
                                                self.source_map_cell_revision =
                                                    self.source_map_cell_revision.wrapping_add(1);
                                            }
                                        }
                                    }
                                    figure.destination_kind = 0;
                                    figure.carried_amount = 0;
                                    figure.cargo_fixed = 0;
                                }
                                _ => {}
                            }
                            if should_despawn {
                                despawn_indices.push(idx);
                                break;
                            }
                        }
                    }
                    ActionType::Walking
                    | ActionType::Sailing
                    | ActionType::Patrolling
                    | ActionType::Exploring => {
                        if figure.source_worker_route != SourceWorkerRoute::None {
                            let Some(map) = self
                                .island_maps
                                .iter()
                                .find(|map| map.island_id == figure.origin_island)
                            else {
                                despawn_indices.push(idx);
                                continue;
                            };
                            let terrain_wegspeed = map
                                .civilian_movement_speed((figure.tile_x, figure.tile_y))
                                .unwrap_or(100);
                            let next = figure
                                .path
                                .get(figure.path_idx)
                                .copied()
                                .unwrap_or((figure.target_x, figure.target_y));
                            let moving = next != (figure.tile_x, figure.tile_y);
                            let frame_duration_ms = carrier::source_animation_frame_duration_ms(
                                civilian_config.frame_speed_for(figure) as u16,
                                terrain_wegspeed,
                                moving,
                            );
                            figure.advance_source_animation(
                                100,
                                frame_duration_ms,
                                civilian_config.frames_per_dir,
                            );
                            let arrived =
                                carrier::advance_source_carrier(figure, 100, terrain_wegspeed);
                            if let Some(slot) = figure.source_event_slot {
                                self.source_figure_events
                                    .set_kind12_route_progress(slot, figure.path_idx);
                            }
                            if !arrived {
                                continue;
                            }

                            match figure.source_worker_route {
                                SourceWorkerRoute::ToResource => {
                                    figure.action = ActionType::CarryingGoods;
                                    figure.source_worker_route = SourceWorkerRoute::Harvesting;
                                    figure.select_source_animation_state(2);
                                    if let Some(slot) = figure.source_event_slot {
                                        self.source_figure_events.set_lifecycle(slot, 2);
                                    }
                                }
                                SourceWorkerRoute::Searching => {}
                                SourceWorkerRoute::Harvesting => {}
                                SourceWorkerRoute::Returning => {
                                    if let Some(state) =
                                        self.source_map_cell_states.iter_mut().find(|state| {
                                            state.matches(
                                                figure.origin_island,
                                                figure.origin_x,
                                                figure.origin_y,
                                            )
                                        })
                                    {
                                        if state.complete_zero_amount_source_delivery() {
                                            self.source_map_cell_revision =
                                                self.source_map_cell_revision.wrapping_add(1);
                                        }
                                    }
                                    despawn_indices.push(idx);
                                }
                                SourceWorkerRoute::ReturningSearch => {}
                                SourceWorkerRoute::None => {}
                            }
                            continue;
                        }

                        if civilian_config.is_kind12(figure) && figure.source_move_speed > 0 {
                            if figure.source_event_slot.is_some_and(|slot| {
                                self.source_figure_events
                                    .kind12_is_terminal(slot)
                                    .unwrap_or(true)
                            }) {
                                despawn_indices.push(idx);
                                continue;
                            }
                            let terrain_wegspeed = self
                                .island_maps
                                .iter()
                                .find(|map| map.island_id == figure.origin_island)
                                .and_then(|map| {
                                    map.civilian_movement_speed((figure.tile_x, figure.tile_y))
                                })
                                .unwrap_or(100);
                            let next = figure
                                .path
                                .get(figure.path_idx)
                                .copied()
                                .unwrap_or((figure.target_x, figure.target_y));
                            let moving = next != (figure.tile_x, figure.tile_y);
                            let frame_duration_ms = carrier::source_animation_frame_duration_ms(
                                civilian_config.frame_speed_for(figure) as u16,
                                terrain_wegspeed,
                                moving,
                            );
                            figure.advance_source_animation(
                                100,
                                frame_duration_ms,
                                civilian_config.frames_per_dir,
                            );
                            let arrived =
                                carrier::advance_source_carrier(figure, 100, terrain_wegspeed);
                            if let Some(slot) = figure.source_event_slot {
                                self.source_figure_events
                                    .set_kind12_route_progress(slot, figure.path_idx);
                            } else if arrived {
                                despawn_indices.push(idx);
                            }
                            continue;
                        }

                        // Generic move-toward-target step for any
                        // remaining figure that isn't a carrier.
                        // Walking, Sailing, Patrolling and
                        // Exploring all share the same naive iso
                        // step semantics; other ActionTypes (Combat,
                        // TradeShipAi, FreeTrader, etc.) are handled
                        // by their own dedicated subsystems.
                        let dx = figure.target_x - figure.tile_x;
                        let dy = figure.target_y - figure.tile_y;
                        if dx == 0 && dy == 0 {
                            // Reached the waypoint — leave the action
                            // and the caller decides whether to
                            // despawn or retarget.
                        } else {
                            if dx.abs() >= dy.abs() {
                                figure.tile_x += dx.signum();
                            } else {
                                figure.tile_y += dy.signum();
                            }
                            if civilian_config.is_civilian(figure) {
                                figure.advance_source_animation(
                                    100,
                                    civilian_config.frame_speed_for(figure),
                                    civilian_config.frames_per_dir,
                                );
                            } else {
                                figure.anim_frame = figure.anim_frame.wrapping_add(1);
                            }
                        }
                    }
                    _ => {
                        // Combat / TradeShipAi / FreeTrader /
                        // Building / Mining / Fishing / Loading /
                        // Delivering / SpecialEvent /
                        // Artillery / Idle / TradeRoute /
                        // ShipCombat / Farming / Walking-other are
                        // driven elsewhere in the tick (combat,
                        // tick_ships, free-trader, production).
                        // No-op here.
                    }
                }
            }
        }

        for (island, x, y, transition) in source_worker_harvests {
            if let Some(cell) = self
                .source_static_map_roots
                .iter_mut()
                .find(|cell| cell.matches(island, x, y))
            {
                if cell.replace_harvested_raw_resource(transition) {
                    self.source_map_cell_revision = self.source_map_cell_revision.wrapping_add(1);
                }
            }
        }

        // Remove despawned figures (iterate in reverse to preserve indices)
        for &idx in despawn_indices.iter().rev() {
            let figure = self.figures.swap_remove(idx);
            if let Some(slot) = figure.source_event_slot {
                self.source_figure_events.release(slot);
            }
        }
    }

    /// Try to deduct construction materials from the player's warehouses
    /// on `island_id`. Returns true if every required cost was withdrawn,
    /// false (with no withdrawals applied) when any line is short.
    pub fn warehouse_pay_materials(
        &mut self,
        island_id: u8,
        owner: u8,
        cost_wood: u16,
        cost_tools: u16,
        cost_bricks: u16,
    ) -> bool {
        use crate::types::Good;
        let total = |good: Good| -> u16 {
            self.warehouses
                .iter()
                .filter(|w| w.active && w.owner == owner && w.island_id == island_id)
                .map(|w| w.stock(good))
                .sum::<u16>()
        };
        if total(Good::Wood) < cost_wood
            || total(Good::Tools) < cost_tools
            || total(Good::Bricks) < cost_bricks
        {
            return false;
        }
        let mut take = |good: Good, mut amount: u16| {
            for w in self
                .warehouses
                .iter_mut()
                .filter(|w| w.active && w.owner == owner && w.island_id == island_id)
            {
                if amount == 0 {
                    break;
                }
                amount -= w.withdraw(good, amount);
            }
        };
        take(Good::Wood, cost_wood);
        take(Good::Tools, cost_tools);
        take(Good::Bricks, cost_bricks);
        true
    }

    /// Apply a player-issued command to the authoritative state.
    /// Returns true if it was applied successfully.
    pub fn apply_command(&mut self, cmd: &crate::commands::Command) -> bool {
        use crate::commands::Command;
        match *cmd {
            Command::SetTaxRate { player, tier, rate } => {
                let pi = player as usize;
                let ti = tier as usize;
                if pi >= self.players.len() || ti >= 5 {
                    return false;
                }
                self.players[pi].tax_rates[ti] = rate;
                true
            }
            Command::SetDiplomacy { a, b, state } => {
                use crate::combat::Diplomacy;
                // Declaring war / breaking treaty is unilateral; nobody
                // gets to refuse. Asking for alliance or peace depends on
                // the original AI relation/cooldown state machine, so this
                // command must not synthesize acceptance from a score ratio.
                if state == Diplomacy::War {
                    self.diplomacy.set(a, b, state);
                    return true;
                }
                self.event_log.push(format!(
                    "[diplo] p{b} {state:?} proposal awaits source diplomacy acceptance"
                ));
                false
            }
            Command::ApplySourceRelationshipEvent {
                source,
                target,
                payload,
            } => {
                let queue_type_one =
                    matches!(self.diplomacy.source_attitude_code(source, target), 2 | 3);
                let applied = self
                    .diplomacy
                    .apply_source_relationship_payload(source, target, payload);
                if !applied {
                    return false;
                }
                match payload {
                    0 => {
                        if queue_type_one {
                            self.diplomacy.enqueue_source_diplomacy_event(
                                target,
                                source,
                                1,
                                self.source_time_ticks,
                            );
                        }
                        self.diplomacy.enqueue_source_diplomacy_event(
                            target,
                            source,
                            3,
                            self.source_time_ticks,
                        );
                    }
                    1 => {
                        self.diplomacy.enqueue_source_diplomacy_event(
                            target,
                            source,
                            4,
                            self.source_time_ticks,
                        );
                    }
                    3 => {}
                    _ => return false,
                }
                true
            }
            Command::ApplySourceAttitudeEvent {
                source,
                target,
                payload,
            } => {
                let applied = self
                    .diplomacy
                    .apply_source_attitude_payload(source, target, payload);
                if !applied {
                    return false;
                }
                let event_type = match payload {
                    0 => Some(1),
                    1 => Some(2),
                    3 => None,
                    _ => return false,
                };
                if let Some(event_type) = event_type {
                    self.diplomacy.enqueue_source_diplomacy_event(
                        target,
                        source,
                        event_type,
                        self.source_time_ticks,
                    );
                }
                true
            }
            Command::Buy { player, good, qty } => {
                let pi = player as usize;
                if pi >= self.players.len() {
                    return false;
                }
                let price = self.current_price(good).buy;
                let max_aff = (self.players[pi].gold / price).max(0) as u16;
                let want = qty.min(max_aff);
                if want == 0 {
                    return false;
                }
                if let Some(wh) = self
                    .warehouses
                    .iter_mut()
                    .find(|w| w.active && w.owner == player)
                {
                    let dep = wh.deposit(good, want);
                    self.players[pi].gold -= dep as i32 * price;
                    return dep > 0;
                }
                false
            }
            Command::Sell { player, good, qty } => {
                let pi = player as usize;
                if pi >= self.players.len() {
                    return false;
                }
                let price = self.current_price(good).sell;
                if let Some(wh) = self
                    .warehouses
                    .iter_mut()
                    .find(|w| w.active && w.owner == player)
                {
                    let took = wh.withdraw(good, qty);
                    self.players[pi].gold += took as i32 * price;
                    return took > 0;
                }
                false
            }
            Command::GiftGold { from, to, amount } => {
                if amount <= 0 {
                    return false;
                }
                let fi = from as usize;
                let ti = to as usize;
                if fi >= self.players.len() || ti >= self.players.len() || fi == ti {
                    return false;
                }
                let send = amount.min(self.players[fi].gold.max(0));
                if send <= 0 {
                    return false;
                }
                self.players[fi].gold -= send;
                self.players[ti].gold += send;
                self.event_log
                    .push(format!("[gift] p{from} sent {send} gold to p{to}",));
                true
            }
            Command::GiftGoods {
                from,
                to,
                good,
                qty,
            } => {
                if qty == 0 || from == to {
                    return false;
                }
                // Withdraw from sender's first warehouse with stock.
                let from_idx = self
                    .warehouses
                    .iter()
                    .position(|w| w.active && w.owner == from && w.stock(good) > 0);
                let Some(fi) = from_idx else {
                    return false;
                };
                let took = self.warehouses[fi].withdraw(good, qty);
                if took == 0 {
                    return false;
                }
                // Deposit into recipient's first active warehouse with
                // free space; refund any remainder to the sender.
                let to_idx = self
                    .warehouses
                    .iter()
                    .position(|w| w.active && w.owner == to);
                if let Some(ti) = to_idx {
                    let placed = self.warehouses[ti].deposit(good, took);
                    let leftover = took - placed;
                    if leftover > 0 {
                        // Recipient was full — return what didn't fit.
                        self.warehouses[fi].deposit(good, leftover);
                    }
                    self.event_log
                        .push(format!("[gift] p{from} sent {placed} {good:?} to p{to}",));
                    placed > 0
                } else {
                    // No recipient warehouse — refund and fail.
                    self.warehouses[fi].deposit(good, took);
                    false
                }
            }
            Command::NativeDeliver {
                player,
                village_idx,
                good,
                qty,
            } => {
                let vi = village_idx as usize;
                if vi >= self.native_villages.len() || qty == 0 {
                    return false;
                }
                // Withdraw qty of `good` from any of the player's
                // warehouses. Refund (no-op) if not enough stock.
                let mut needed = qty;
                let mut taken = 0u16;
                for w in self
                    .warehouses
                    .iter_mut()
                    .filter(|w| w.active && w.owner == player)
                {
                    if needed == 0 {
                        break;
                    }
                    let took = w.withdraw(good, needed);
                    needed -= took;
                    taken += took;
                }
                if taken == 0 {
                    return false;
                }
                let outcome = self.native_villages[vi].deliver(player, good, taken);
                if outcome == crate::native::BarterOutcome::NotWanted {
                    // Refund — natives didn't accept after all.
                    if let Some(w) = self
                        .warehouses
                        .iter_mut()
                        .find(|w| w.active && w.owner == player)
                    {
                        w.deposit(good, taken);
                    }
                    return false;
                }
                self.event_log.push(format!(
                    "[natives] delivered {taken} {good:?} to village #{vi}",
                ));
                true
            }
            Command::NativeWithdraw {
                player,
                village_idx,
                good,
                qty,
            } => {
                let vi = village_idx as usize;
                if vi >= self.native_villages.len() || qty == 0 {
                    return false;
                }
                let outcome = self.native_villages[vi].withdraw(player, good, qty);
                if outcome != crate::native::BarterOutcome::Withdrawn {
                    return false;
                }
                if let Some(w) = self
                    .warehouses
                    .iter_mut()
                    .find(|w| w.active && w.owner == player)
                {
                    let placed = w.deposit(good, qty);
                    if placed < qty {
                        // Warehouse full — refund the credit so the
                        // player can try again with more space.
                        let refund =
                            (qty - placed) as i32 * crate::prices::price_of(good).sell as i32;
                        let p = player as usize;
                        if p < 7 {
                            self.native_villages[vi].credit[p] += refund;
                        }
                    }
                    self.event_log.push(format!(
                        "[natives] received {placed} {good:?} from village #{vi}",
                    ));
                    placed > 0
                } else {
                    // No player warehouse — refund all credit and fail.
                    let refund = qty as i32 * crate::prices::price_of(good).sell as i32;
                    let p = player as usize;
                    if p < 7 {
                        self.native_villages[vi].credit[p] += refund;
                    }
                    false
                }
            }
            Command::LoadShip {
                player,
                ship_idx,
                warehouse_idx,
                good,
                qty,
            } => {
                let si = ship_idx as usize;
                let wi = warehouse_idx as usize;
                if si >= self.trade_ships.len() || wi >= self.warehouses.len() || qty == 0 {
                    return false;
                }
                if self.trade_ships[si].owner != player
                    || self.warehouses[wi].owner != player
                    || !self.trade_ships[si].active
                    || !self.warehouses[wi].active
                {
                    return false;
                }
                // Ship must be at the warehouse tile (within 2 tiles).
                let ship = &self.trade_ships[si];
                let wh = &self.warehouses[wi];
                let dx = (ship.world_x - wh.tile_x as i32).abs();
                let dy = (ship.world_y - wh.tile_y as i32).abs();
                if dx > 2 || dy > 2 {
                    return false;
                }
                let took = self.warehouses[wi].withdraw(good, qty);
                if took == 0 {
                    return false;
                }
                let loaded = self.trade_ships[si].load(good, took);
                if loaded < took {
                    // Cargo hold full — refund the surplus.
                    self.warehouses[wi].deposit(good, took - loaded);
                }
                self.event_log
                    .push(format!("[ship] loaded {loaded} {good:?} onto ship #{si}",));
                loaded > 0
            }
            Command::UnloadShip {
                player,
                ship_idx,
                warehouse_idx,
                good,
                qty,
            } => {
                let si = ship_idx as usize;
                let wi = warehouse_idx as usize;
                if si >= self.trade_ships.len() || wi >= self.warehouses.len() || qty == 0 {
                    return false;
                }
                if self.trade_ships[si].owner != player
                    || self.warehouses[wi].owner != player
                    || !self.trade_ships[si].active
                    || !self.warehouses[wi].active
                {
                    return false;
                }
                let ship = &self.trade_ships[si];
                let wh = &self.warehouses[wi];
                let dx = (ship.world_x - wh.tile_x as i32).abs();
                let dy = (ship.world_y - wh.tile_y as i32).abs();
                if dx > 2 || dy > 2 {
                    return false;
                }
                let unloaded = self.trade_ships[si].unload(good, qty);
                if unloaded == 0 {
                    return false;
                }
                let placed = self.warehouses[wi].deposit(good, unloaded);
                if placed < unloaded {
                    // Warehouse full — refund the surplus to the ship.
                    self.trade_ships[si].load(good, unloaded - placed);
                }
                self.event_log
                    .push(format!("[ship] unloaded {placed} {good:?} from ship #{si}",));
                placed > 0
            }
            Command::SellShip { player, unit_index } => {
                let pi = player as usize;
                if pi >= self.players.len() {
                    return false;
                }
                let ui = unit_index as usize;
                if ui >= self.military_units.len() {
                    return false;
                }
                let u = &self.military_units[ui];
                if u.owner != player || !u.is_alive() {
                    return false;
                }
                if !u.unit_type.stats().is_naval {
                    return false;
                }
                let cost = crate::combat::unit_build_cost(u.unit_type);
                let refund = cost / 2;
                self.players[pi].gold += refund;
                // Remove the ship by deactivating it; tick_combat
                // already filters dead/inactive units out.
                self.military_units[ui].active = false;
                self.military_units[ui].health = 0.0;
                self.event_log
                    .push(format!("[werft] sold ship #{ui} for {refund} gold",));
                true
            }
            Command::SetPatrol {
                ref player,
                ref unit_index,
                ref waypoints,
            } => {
                let ui = *unit_index as usize;
                if ui >= self.military_units.len() {
                    return false;
                }
                let unit = &mut self.military_units[ui];
                if unit.owner != *player || !unit.is_alive() {
                    return false;
                }
                if waypoints.is_empty() {
                    unit.patrol.clear();
                    unit.patrol_idx = 0;
                } else {
                    unit.patrol = waypoints.clone();
                    unit.patrol_idx = 0;
                    let first = waypoints[0];
                    unit.target_x = first.0;
                    unit.target_y = first.1;
                    unit.combat_target = -1;
                    unit.move_timer_ms = 0;
                }
                true
            }
            Command::ArmShip {
                player,
                unit_index,
                target_cannons,
            } => {
                let pi = player as usize;
                if pi >= self.players.len() {
                    return false;
                }
                let ui = unit_index as usize;
                if ui >= self.military_units.len() {
                    return false;
                }
                let unit = &self.military_units[ui];
                if unit.owner != player || !unit.is_alive() {
                    return false;
                }
                let cap = crate::combat::cannon_capacity(unit.unit_type);
                if cap == 0 {
                    return false;
                }
                let want = target_cannons.min(cap);
                let have = unit.cannons;
                if want <= have {
                    return false;
                }
                let to_install = (want - have) as u32;
                let cost_gold = to_install as i32 * 200;
                if self.players[pi].gold < cost_gold {
                    return false;
                }
                // Pull `Cannons` good from any of the player's
                // warehouses to cover the install.
                let mut needed = to_install as u16;
                for w in self
                    .warehouses
                    .iter_mut()
                    .filter(|w| w.active && w.owner == player)
                {
                    if needed == 0 {
                        break;
                    }
                    let took = w.withdraw(crate::types::Good::Cannons, needed);
                    needed -= took;
                }
                if needed > 0 {
                    // Refund anything we already pulled — withdraw is
                    // idempotent at the warehouse level so we just
                    // walk back what we still owe.
                    let recovered = to_install as u16 - needed;
                    if recovered > 0 {
                        if let Some(w) = self
                            .warehouses
                            .iter_mut()
                            .find(|w| w.active && w.owner == player)
                        {
                            w.deposit(crate::types::Good::Cannons, recovered);
                        }
                    }
                    return false;
                }
                self.players[pi].gold -= cost_gold;
                self.military_units[ui].cannons = want;
                self.event_log.push(format!(
                    "[werft] ship #{ui} armed with {want}/{cap} cannons",
                ));
                true
            }
            Command::ProposeTradeAgreement { a, b } => {
                let ok = self.diplomacy.propose_trade_agreement(a, b);
                if ok {
                    self.event_log
                        .push(format!("[diplo] trade agreement signed: p{a} ↔ p{b}",));
                }
                ok
            }
            Command::BreakTradeAgreement { a, b } => {
                let ok = self.diplomacy.break_trade_agreement(a, b);
                if ok {
                    self.event_log
                        .push(format!("[diplo] trade agreement BROKEN: p{a} ↔ p{b}",));
                }
                ok
            }
            Command::DispatchCart {
                player,
                from_warehouse,
                to_warehouse,
                good,
                qty,
            } => {
                // KARREN Maxtrag = 6 (figuren.cod `Nummer: KARREN`).
                let qty = qty.min(6);
                if qty == 0 || from_warehouse == to_warehouse {
                    return false;
                }
                let fi = from_warehouse as usize;
                let ti = to_warehouse as usize;
                if fi >= self.warehouses.len() || ti >= self.warehouses.len() {
                    return false;
                }
                if !self.warehouses[fi].active
                    || !self.warehouses[ti].active
                    || self.warehouses[fi].owner != player
                    || self.warehouses[ti].owner != player
                {
                    return false;
                }
                let took = self.warehouses[fi].withdraw(good, qty);
                if took == 0 {
                    return false;
                }
                let placed = self.warehouses[ti].deposit(good, took);
                let leftover = took - placed;
                if leftover > 0 {
                    self.warehouses[fi].deposit(good, leftover);
                }
                self.event_log
                    .push(format!("[cart] {placed} {good:?} → warehouse #{ti}",));
                placed > 0
            }
        }
    }

    /// Get the displayed game time as (minutes, seconds).
    pub fn display_time(&self) -> (u32, u32) {
        let minutes = self.game_clock / TICKS_PER_MINUTE;
        let seconds = (self.game_clock % TICKS_PER_MINUTE) / 10;
        (minutes, seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{AiController, AiPersonality, Difficulty};

    #[test]
    fn source_controller_construction_queue_replays_fun_00417aa0_priority_rules() {
        let work = |priority: i16| {
            let mut bytes = [0_u8; 32];
            bytes[0x1c..0x1e].copy_from_slice(&priority.to_le_bytes());
            SourceControllerCityConstructionWork::from_bytes(bytes)
        };
        let mut queue = SourceControllerCityConstructionQueue::default();

        assert!(!queue.insert(work(1)));
        assert!(queue.insert(work(2)));
        for _ in 1..SourceControllerCityConstructionQueue::CAPACITY {
            assert!(queue.insert(work(4)));
        }
        assert_eq!(queue.entries().len(), 0x100);

        assert!(queue.insert(work(4)));
        assert_eq!(queue.entries()[0].priority(), 4);
        assert!(!queue.insert(work(3)));
        assert!(!queue.insert(work(5)));
    }

    #[test]
    fn kind13_dispatch_promotion_debits_city_store_and_emits_replacement_command() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(2, 16, 16));
        let mut warehouse = Warehouse::with_capacity(2, 3, 8, 9, 50);
        warehouse.deposit(Good::Tools, 1);
        warehouse.deposit(Good::Wood, 1);
        warehouse.deposit(Good::Bricks, 1);
        warehouse.deposit(Good::Cannons, 1);
        sim.warehouses.push(warehouse);
        for color in 0..4 {
            sim.players.push(Player::new_human(color));
        }
        sim.players[3].gold = 5;
        sim.source_kind13_promotion_definitions[1] = Some(SourceKind13PromotionDefinition {
            target_group: 1,
            source_size: (1, 1),
            tools_cost_fixed: 32,
            wood_cost_fixed: 32,
            bricks_cost_fixed: 32,
            cannons_cost_fixed: 64,
            money_cost: 17,
            variant_definition_offsets: vec![77],
        });
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                island_id: 2,
                source_owner: 3,
                owner_slot: 3,
                phase: 1,
                tier_population: [2, 0, 0, 0, 0],
                satisfaction_by_group: [0x80, 0x80, 0, 0, 0],
                overall_satisfaction: 0x80,
                promotion_reservations: [0, 3, 0, 0, 0],
                promotion_reservation_positions: [(0, 0), (8, 9), (0, 0), (0, 0), (0, 0)],
                ..SourceCityRecord::default()
            })
        ));
        let location = SourceKind13Location {
            island_id: 2,
            tile_x: 8,
            tile_y: 9,
            orientation: 2,
            variant: 0,
            source_owner: 3,
            phase: 0,
            state_bits: 0xc0,
            population_group: 0,
            amount: 0x80,
            lifecycle_flags: 0x000c,
        };
        assert!(sim.source_kind13_locations.insert(location));
        sim.seed_source_rand(1);

        sim.apply_source_kind13_dispatch_location(location);

        assert_eq!(sim.warehouses[0].city_stock_fixed(Good::Tools), 0);
        assert_eq!(sim.warehouses[0].city_stock_fixed(Good::Wood), 0);
        assert_eq!(sim.warehouses[0].city_stock_fixed(Good::Bricks), 0);
        assert_eq!(sim.warehouses[0].city_stock_fixed(Good::Cannons), 0);
        assert_eq!(sim.players[3].gold, -12);
        assert_eq!(
            sim.source_kind13_locations
                .location_at(2, 8, 9)
                .map(|root| (root.population_group, root.amount)),
            Some((1, 143))
        );
        assert_eq!(sim.source_kind13_replacement_commands.len(), 1);
        let replacement = sim.source_kind13_replacement_commands[0];
        assert_eq!(
            (
                replacement.island_id,
                replacement.tile_x,
                replacement.tile_y
            ),
            (2, 8, 9)
        );
        assert_eq!(replacement.target_group, 1);
        assert_eq!(replacement.command.definition_offset, 77);
        assert_eq!(replacement.command.orientation, 2);
        assert_eq!(replacement.command.variant, 0);
        assert_eq!(replacement.command.metadata, 2);
        assert_eq!(replacement.command.map_owner_slot, 3);
        assert!(replacement.command.random_seed < 32);
        assert_eq!(replacement.command.dynamic_object_owner, 3);
    }

    #[test]
    fn kind13_dispatch_downgrade_mutates_root_and_emits_replacement_command() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(2, 16, 16));
        sim.source_kind13_promotion_definitions[0] = Some(SourceKind13PromotionDefinition {
            target_group: 0,
            source_size: (1, 1),
            tools_cost_fixed: 0,
            wood_cost_fixed: 0,
            bricks_cost_fixed: 0,
            cannons_cost_fixed: 0,
            money_cost: 0,
            variant_definition_offsets: vec![66],
        });
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                island_id: 2,
                source_owner: 3,
                owner_slot: 3,
                tier_population: [0, 1, 0, 0, 0],
                ..SourceCityRecord::default()
            })
        ));
        let location = SourceKind13Location {
            island_id: 2,
            tile_x: 8,
            tile_y: 9,
            orientation: 1,
            variant: 0,
            source_owner: 3,
            phase: 0,
            state_bits: 0,
            population_group: 1,
            amount: 90,
            lifecycle_flags: 0,
        };
        assert!(sim.source_kind13_locations.insert(location));
        sim.seed_source_rand(2);

        sim.apply_source_kind13_dispatch_location(location);

        let root = sim.source_kind13_locations.location_at(2, 8, 9).unwrap();
        assert_eq!(root.population_group, 0);
        assert!(root.amount < 90);
        assert_eq!(sim.source_cities.record(0).unwrap().tier_population[1], 0);
        assert_eq!(
            sim.source_cities.record(0).unwrap().tier_population[0],
            u32::from(root.amount >> 6)
        );
        assert_eq!(sim.source_kind13_replacement_commands.len(), 1);
        let replacement = sim.source_kind13_replacement_commands[0];
        assert_eq!(replacement.target_group, 0);
        assert_eq!(replacement.command.definition_offset, 66);
        assert_eq!(replacement.command.orientation, 1);
        assert_eq!(replacement.command.dynamic_object_owner, 3);
    }

    #[test]
    fn source_dynamic_figure_loader_enforces_source_category_tables() {
        let mut sim = Simulation::new();
        let figure = |figure_kind, runtime_slot, owner| SourceDynamicCombatFigure {
            active: true,
            figure_kind,
            candidate_list_key: 4,
            figure_definition_id: 31,
            direction: 1,
            source_payload: 0,
            position: (120.0, 130.0),
            position_z: 0.0,
            source_energy: 320,
            source_score_state: 0,
            source_action_ready_at: 0,
            source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
            target_descriptor: SourceTargetDescriptor::from_bytes([0x37, 0, 60, 65]),
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner,
            state: 7,
            flags: 0,
            notification: 0,
            runtime_slot,
            auxiliary_kind: 0,
            name_index: 0,
            source_motion: combat::SourceGenericMotion::default(),
        };

        assert!(sim.install_source_dynamic_combat_figure(figure(1, 149, 1)));
        assert!(sim.install_source_dynamic_combat_figure(figure(3, 149, 2)));
        assert_eq!(sim.source_dynamic_combat_figures.len(), 1);
        assert_eq!(sim.source_dynamic_combat_figures[0].figure_kind, 3);
        assert_eq!(sim.source_dynamic_combat_figures[0].owner, 2);
        sim.source_combat_terminal_slices
            .push(SourceCombatTerminalSlice {
                target: SourceCombatTerminalSliceTarget::DynamicFigure(0),
                target_figure_kind: 3,
                target_runtime_slot: 149,
                remaining_distance: combat::SOURCE_TERMINAL_STATIONARY_SPEED,
                scalar_speed: combat::SOURCE_TERMINAL_STATIONARY_SPEED,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
            });
        assert!(!sim.install_source_dynamic_combat_figure(figure(1, 149, 0)));
        sim.source_combat_terminal_slices.clear();
        assert!(!sim.install_source_dynamic_combat_figure(figure(1, 150, 0)));

        assert!(sim.install_source_dynamic_combat_figure(figure(4, 399, 3)));
        assert!(!sim.install_source_dynamic_combat_figure(figure(4, 400, 0)));
        sim.source_time_ticks = 47;
        assert!(sim.install_source_dynamic_combat_figure(figure(6, 349, 4)));
        assert!(!sim.install_source_dynamic_combat_figure(figure(6, 350, 0)));
        assert!(!sim.install_source_dynamic_combat_figure(figure(7, 0, 0)));

        let mut occupied = Figure::new();
        occupied.action = ActionType::Walking;
        sim.figures = vec![occupied; SourceFigureRecordLayout::CAPACITY];
        assert!(!sim.install_source_dynamic_combat_figure(figure(1, 148, 0)));

        assert_eq!(
            sim.source_combat_candidates()
                .iter()
                .map(|candidate| (candidate.figure_kind, candidate.runtime_slot))
                .collect::<Vec<_>>(),
            vec![(3, 149), (4, 399), (6, 349)]
        );
        assert_eq!(
            sim.source_dynamic_combat_figures[2].source_action_ready_at,
            47
        );
    }

    #[test]
    fn source_figure_purchase_transfers_a_controlled_shared_handle() {
        let mut sim = Simulation::new();
        sim.players = vec![
            Player::new_human(0),
            Player::new_ai(1, 0),
            Player::new_human(2),
        ];
        sim.players[1].gold = 200;
        sim.players[2].gold = 1_000;
        sim.source_kind4_dispatch.remote_owner_dispatch_enabled = true;
        sim.source_kind4_dispatch.faction_states[1] = 0x0c;
        sim.source_kind4_dispatch.faction_states[2] = 0x0e;
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 4,
                figure_definition_id: 0x19,
                direction: 0,
                source_payload: 0,
                position: (1.25, 3.25),
                position_z: 0.0,
                source_energy: 195,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 1,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 9,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            });

        let event = SourceFigurePurchaseEvent {
            buyer: 2,
            figure_handle: 9,
            amount: 120,
        };
        sim.execute_source_figure_purchase(event);
        assert_eq!(sim.source_dynamic_combat_figures[0].owner, 1);
        assert_eq!(sim.players[1].gold, 200);
        assert_eq!(sim.players[2].gold, 1_000);

        sim.set_source_figure_purchase_enabled(9, true);
        sim.execute_source_figure_purchase(event);

        assert_eq!(sim.source_dynamic_combat_figures[0].owner, 2);
        assert_eq!(sim.source_dynamic_combat_figures[0].figure_kind, 3);
        assert_eq!(sim.players[1].gold, 320);
        assert_eq!(sim.players[2].gold, 880);
        assert_eq!(sim.source_shared_figure_control_flags[9] & 0x40, 0);
    }

    #[test]
    fn source_controller_purchase_uses_handle_order_and_strict_gold_gate() {
        let mut sim = Simulation::new();
        sim.players = vec![Player::new_human(0), Player::new_ai(1, 0)];
        sim.players[0].gold = 1_051;
        sim.source_kind4_dispatch.remote_owner_dispatch_enabled = true;
        sim.source_kind4_dispatch.faction_states[1] = 0x0c;

        let ship = |handle| {
            let mut ship = TradeShip::new(1, 0, 0, 0);
            ship.source_figure_kind = Some(1);
            ship.source_runtime_slot = Some(handle);
            ship.source_figure_definition_id = Some(0x15);
            ship.source_energy = 150;
            ship
        };
        sim.trade_ships = vec![ship(7), ship(3)];
        sim.set_source_figure_purchase_enabled(7, true);
        sim.set_source_figure_purchase_enabled(3, true);

        let event = sim
            .source_controller_purchase_figure(0, 0, 1)
            .expect("strictly affordable source candidate");
        assert_eq!(event.figure_handle, 3);
        assert_eq!(event.amount, 1_050);
        assert_eq!(sim.players[0].gold, 1);
        assert_eq!(sim.players[1].gold, 21_050);
        assert_eq!(sim.trade_ships[0].owner, 1);
        assert_eq!(sim.trade_ships[1].owner, 0);

        sim.players[0].gold = 1_050;
        assert_eq!(sim.source_controller_purchase_figure(0, 0, 1), None);
    }

    #[test]
    fn source_controller_scheduler_runs_the_due_city_purchase_branch() {
        let mut sim = Simulation::new();
        sim.players = vec![Player::new_human(0), Player::new_ai(1, 0)];
        sim.players[0].gold = 1_051;
        sim.source_kind4_dispatch.remote_owner_dispatch_enabled = true;
        sim.source_kind4_dispatch.faction_states = [0x0c, 0, 7, 7, 7, 7, 7];

        let mut ship = TradeShip::new(1, 0, 0, 0);
        ship.source_figure_kind = Some(1);
        ship.source_runtime_slot = Some(7);
        ship.source_figure_definition_id = Some(0x15);
        ship.source_energy = 150;
        sim.trade_ships.push(ship);
        sim.set_source_figure_purchase_enabled(7, true);

        sim.source_player_controllers[0] = SourcePlayerController {
            initialized: true,
            initialized_at_ticks: 0,
            action_timer_ms: 1,
            city_management_timer_ms: -1,
            maintenance_timer_ms: 0,
            desired_figure_count: 3,
            figure_roster_ratio: 0,
            figure_capacity: 1,
            figure_capacity_limit: None,
            city_management_profile: Some(SourceCityManagementProfile {
                city_slot: 0,
                initialized_at_ticks: 0,
            }),
            action_figure_handle: None,
            action_target_island_id: None,
            action_target_tile: None,
            active_city_owner: Some(0),
            active_city_slot: None,
            selected_city_active: false,
            city_management_disabled: false,
            action_stack: vec![9],
            action_budget: 0,
            purchase_predecessor_issued: false,
            owned_figure_handles: vec![],
            figure_roster_dirty: false,
            ..Default::default()
        };

        sim.tick_source_player_controllers(1);

        assert_eq!(sim.trade_ships[0].owner, 0);
        assert_eq!(sim.players[0].gold, 1);
        assert_eq!(sim.players[1].gold, 21_050);
        assert_eq!(sim.source_player_controllers[0].action_timer_ms, 50);
        assert_eq!(
            sim.source_player_controllers[0].city_management_timer_ms,
            9_999
        );
        assert!(sim.source_player_controllers[0].figure_roster_dirty);
    }

    #[test]
    fn source_controller_scheduler_preserves_the_predecessor_gate() {
        let mut sim = Simulation::new();
        sim.players = vec![Player::new_human(0), Player::new_ai(1, 0)];
        sim.players[0].gold = 1_051;
        sim.source_kind4_dispatch.remote_owner_dispatch_enabled = true;
        sim.source_kind4_dispatch.faction_states = [0x0c, 0, 7, 7, 7, 7, 7];

        let mut ship = TradeShip::new(1, 0, 0, 0);
        ship.source_figure_kind = Some(1);
        ship.source_runtime_slot = Some(7);
        ship.source_figure_definition_id = Some(0x15);
        ship.source_energy = 150;
        sim.trade_ships.push(ship);
        sim.set_source_figure_purchase_enabled(7, true);
        sim.source_player_controllers[0] = SourcePlayerController {
            initialized: true,
            initialized_at_ticks: 0,
            action_timer_ms: 1,
            city_management_timer_ms: -1,
            maintenance_timer_ms: 0,
            desired_figure_count: 3,
            figure_roster_ratio: 0,
            figure_capacity: 1,
            figure_capacity_limit: None,
            city_management_profile: Some(SourceCityManagementProfile {
                city_slot: 0,
                initialized_at_ticks: 0,
            }),
            action_figure_handle: None,
            action_target_island_id: None,
            action_target_tile: None,
            active_city_owner: Some(0),
            active_city_slot: None,
            selected_city_active: false,
            city_management_disabled: false,
            action_stack: vec![9],
            action_budget: 0,
            purchase_predecessor_issued: true,
            owned_figure_handles: vec![],
            figure_roster_dirty: false,
            ..Default::default()
        };

        sim.tick_source_player_controllers(1);

        assert_eq!(sim.trade_ships[0].owner, 1);
        assert_eq!(sim.players[0].gold, 1_051);
        assert_eq!(
            sim.source_player_controllers[0].city_management_timer_ms,
            9_999
        );
    }

    #[test]
    fn source_controller_figure_capacity_uses_weighted_human_totals_and_player_limit() {
        let (roster_ratio, capacity) = Simulation::source_controller_figure_capacity_from_inputs(
            3,
            10,
            true,
            0,
            [20, 80, 0, 0, 0, 0, 0],
            [0x0c, 0, 0xff, 0xff, 0xff, 0xff, 0xff],
            2,
            0x21,
            10,
        );
        assert_eq!(roster_ratio, 4);
        assert_eq!(capacity, 10);

        let (roster_ratio, capacity) = Simulation::source_controller_figure_capacity_from_inputs(
            3, 7, false, 0, [0; 7], [0; 7], 2, 0, 2,
        );
        assert_eq!(roster_ratio, 3);
        assert_eq!(capacity, 2);
    }

    #[test]
    fn source_controller_loader_keeps_weighted_capacity_profile_inactive() {
        let mut sim = Simulation::new();
        let controller = &mut sim.source_player_controllers[0];
        controller.desired_figure_count = 3;
        controller.owned_figure_handles = vec![0; 10];
        controller.figure_capacity_limit = Some(100);
        controller.city_management_profile = Some(SourceCityManagementProfile {
            city_slot: 0,
            initialized_at_ticks: 0,
        });

        sim.refresh_source_player_controller_figure_capacity(0);
        assert_eq!(sim.source_player_controllers[0].figure_capacity, 3);
    }

    #[test]
    fn source_controller_city_initialization_keeps_physical_slot() {
        let mut sim = Simulation::new();
        assert!(sim.source_cities.set_record(
            2,
            Some(SourceCityRecord {
                owner_slot: 1,
                tier_population: [0, 0, 12, 0, 0],
                ..Default::default()
            })
        ));
        assert!(sim.source_cities.set_record(
            5,
            Some(SourceCityRecord {
                owner_slot: 1,
                tier_population: [0, 0, 0, 5, 5],
                ..Default::default()
            })
        ));

        sim.configure_source_controller_active_cities();

        assert_eq!(sim.source_player_controllers[1].active_city_slot, Some(5));
        assert_eq!(sim.source_player_controllers[1].active_city_owner, Some(1));
    }

    #[test]
    fn source_controller_state_two_island_eligibility_uses_live_city_counts() {
        let mut sim = Simulation::new();
        sim.source_kind4_dispatch.faction_states[1] = 0x0c;
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                island_id: 7,
                owner_slot: 1,
                ..Default::default()
            })
        ));

        assert!(!sim.source_controller_target_island_is_eligible(0, 7));

        assert!(sim.source_cities.set_record(
            1,
            Some(SourceCityRecord {
                island_id: 3,
                owner_slot: 1,
                ..Default::default()
            })
        ));
        assert!(sim.source_cities.set_record(
            2,
            Some(SourceCityRecord {
                island_id: 4,
                owner_slot: 1,
                ..Default::default()
            })
        ));
        assert!(sim.source_controller_target_island_is_eligible(0, 7));

        assert!(sim.source_cities.set_record(
            3,
            Some(SourceCityRecord {
                island_id: 7,
                owner_slot: 0,
                ..Default::default()
            })
        ));
        assert!(!sim.source_controller_target_island_is_eligible(0, 7));

        assert!(sim.source_cities.set_record(
            3,
            Some(SourceCityRecord {
                island_id: 7,
                owner_slot: 2,
                ..Default::default()
            })
        ));
        sim.source_kind4_dispatch.faction_states[2] = 0x0e;
        assert!(sim.source_controller_target_island_is_eligible(0, 7));
    }

    #[test]
    fn source_controller_state_two_cursor_selects_source_island_order_and_requirement() {
        let mut sim = Simulation::new();
        sim.source_kind4_dispatch.faction_states = [0x0c, 0x0e, 7, 7, 7, 7, 7];
        sim.island_maps.push(IslandMap::new_open(1, 8, 8));
        sim.island_maps.push(IslandMap::new_open(3, 8, 8));
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                island_id: 1,
                owner_slot: 0,
                ..Default::default()
            })
        ));
        assert!(sim.source_cities.set_record(
            1,
            Some(SourceCityRecord {
                island_id: 3,
                owner_slot: 1,
                ..Default::default()
            })
        ));

        assert!(sim.request_source_controller_island_search(0, None));
        let controller = &sim.source_player_controllers[0];
        assert_eq!(controller.island_search_cursor, 3);
        assert_eq!(controller.island_search_selected_island_id, Some(3));
        assert_eq!(controller.action_stack, vec![2]);

        let mut constrained = Simulation::new();
        constrained.source_kind4_dispatch.faction_states[0] = 0x0c;
        constrained.source_player_controllers[0].desired_figure_count = 2;
        constrained.island_maps.push(IslandMap::new_open(1, 8, 8));
        constrained.island_maps.push(
            IslandMap::new_open(2, 8, 8).with_source_resource_state(
                anno_formats::szs::IslandSourceResourceState {
                    crop_flags: 1,
                    ..Default::default()
                },
            ));

        assert!(constrained.request_source_controller_island_search(0, Some(0x2d)));
        let controller = &constrained.source_player_controllers[0];
        assert_eq!(controller.island_search_cursor, 2);
        assert_eq!(controller.island_search_requirement, Some(0x2d));
        assert_eq!(controller.island_search_selected_island_id, Some(2));
        assert_eq!(controller.action_stack, vec![2]);
    }

    #[test]
    fn source_controller_state_two_native_cursor_repeats_until_city_count() {
        let mut sim = Simulation::new();
        sim.source_kind4_dispatch.faction_states[0] = 0x0e;
        sim.island_maps.push(IslandMap::new_open(1, 8, 8));
        for slot in 0..2 {
            assert!(sim.source_cities.set_record(
                slot,
                Some(SourceCityRecord {
                    island_id: 1,
                    owner_slot: 5,
                    ..Default::default()
                })
            ));
        }

        assert!(sim.request_source_controller_island_search(0, None));
        let controller = &sim.source_player_controllers[0];
        assert_eq!(controller.island_search_cursor, 1);
        assert_eq!(controller.island_search_selected_island_id, Some(1));
        assert_eq!(controller.action_stack, vec![2]);
    }

    #[test]
    fn source_controller_state_two_consumes_and_retains_rectangle_search() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(4, 8, 8));
        sim.source_player_controllers[0] = SourcePlayerController {
            island_search_selected_island_id: Some(4),
            island_search_area_threshold: 1,
            action_stack: vec![2],
            ..Default::default()
        };

        assert!(!sim.advance_source_controller_city_area_search(0));
        let controller = &sim.source_player_controllers[0];
        assert!(controller.source_city_rectangles.is_empty());
        assert!(controller.action_stack.is_empty());
    }

    #[test]
    fn source_controller_scheduler_dispatches_action_two_on_its_50ms_gate() {
        let mut sim = Simulation::new();
        sim.source_kind4_dispatch.remote_owner_dispatch_enabled = true;
        sim.source_kind4_dispatch.faction_states[0] = 0x0c;
        sim.island_maps.push(IslandMap::new_open(4, 8, 8));
        sim.source_player_controllers[0] = SourcePlayerController {
            initialized: true,
            action_timer_ms: 0,
            island_search_selected_island_id: Some(4),
            island_search_area_threshold: 1,
            action_stack: vec![2],
            ..Default::default()
        };

        sim.tick_source_player_controllers(0);
        let controller = &sim.source_player_controllers[0];
        assert_eq!(controller.action_timer_ms, 50);
        assert!(controller.source_city_rectangles.is_empty());
        assert!(controller.action_stack.is_empty());
    }

    #[test]
    fn source_controller_state_three_requeues_when_no_segment_survives() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(4, 8, 8));
        sim.source_player_controllers[0] = SourcePlayerController {
            island_search_cursor: 4,
            island_search_selected_island_id: Some(4),
            island_search_area_threshold: 400,
            source_city_rectangles: vec![SourceControllerCityRectangle {
                x0: 0,
                y0: 0,
                x1: 6,
                y1: 6,
                area: 200,
            }],
            action_stack: vec![3],
            ..Default::default()
        };

        assert!(!sim.advance_source_controller_city_candidate_search(0));
        assert_eq!(sim.source_player_controllers[0].action_stack, vec![3]);
    }

    #[test]
    fn source_controller_state_three_population_mode_uses_all_bgruppe_totals() {
        let mut sim = Simulation::new();
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                owner_slot: 0,
                tier_population: [200, 200, 200, 200, 400],
                ..Default::default()
            })
        ));
        assert!(sim.source_controller_city_has_large_population(0));
        assert!(!sim.source_controller_city_has_large_population(1));
    }

    #[test]
    fn source_controller_writes_kind34_target_to_selected_dynamic_figure() {
        let mut sim = Simulation::new();
        let motion = combat::SourceGenericMotion {
            remaining_distance: 1.5,
            scalar_speed: 0.05,
            velocity_x: 0.05,
            velocity_y: 0.0,
            velocity_z: 0.0,
            terminal_motion_locked: false,
        };
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 4,
                figure_definition_id: 0x19,
                direction: 0,
                source_payload: 0,
                position: (0.5, 0.5),
                position_z: 0.0,
                source_energy: 0,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 0,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 7,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: motion,
            });
        sim.source_dynamic_route_programs
            .push(SourceDynamicRouteProgram {
                runtime_slot: 7,
                program: vec![0x32, SOURCE_ROUTE_TERMINATOR],
                cursor: 0,
            });

        let descriptor = SourceTargetDescriptor::from_source_kind34_island_cell(4, 12, 13);
        assert!(sim.set_source_controller_figure_target_descriptor(7, descriptor));
        assert_eq!(
            sim.source_dynamic_combat_figures[0].target_descriptor,
            descriptor
        );
        assert_eq!(sim.source_dynamic_combat_figures[0].source_motion, motion);
        assert!(sim.source_dynamic_route_programs.is_empty());
    }

    #[test]
    fn source_controller_state_four_requeues_an_approaching_figure_with_one_less_retry() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(4, 16, 16));
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 4,
                figure_definition_id: 0x19,
                direction: 0,
                source_payload: 0,
                position: (0.5, 0.5),
                position_z: 0.0,
                source_energy: 0,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 0,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 7,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            });
        sim.source_player_controllers[0] = SourcePlayerController {
            action_figure_handle: Some(7),
            action_target_island_id: Some(4),
            action_target_tile: Some((4, 4)),
            action_source_candidate_tile: Some((3, 4)),
            action_arrival_retries: 4,
            island_search_cursor: 4,
            action_stack: vec![4],
            ..Default::default()
        };

        assert!(sim.advance_source_controller_city_arrival(0));
        let controller = &sim.source_player_controllers[0];
        assert_eq!(controller.action_stack, vec![4]);
        assert_eq!(controller.action_arrival_retries, 3);
        assert_eq!(
            sim.source_dynamic_combat_figures[0].target_descriptor,
            SourceTargetDescriptor::from_source_kind34_island_cell(4, 3, 4)
        );
    }

    #[test]
    fn source_controller_state_four_checks_the_descriptor_island_before_city_allocation() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(4, 16, 16));
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 4,
                figure_definition_id: 0x19,
                direction: 0,
                source_payload: 0,
                position: (3.5, 4.5),
                position_z: 0.0,
                source_energy: 0,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 0,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 7,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            });
        sim.source_player_controllers[0] = SourcePlayerController {
            action_figure_handle: Some(7),
            action_target_island_id: Some(7),
            action_target_tile: Some((4, 4)),
            action_source_candidate_tile: Some((3, 4)),
            action_arrival_retries: 4,
            island_search_cursor: 4,
            action_stack: vec![4],
            ..Default::default()
        };

        assert!(sim.advance_source_controller_city_arrival(0));
        assert_eq!(sim.source_player_controllers[0].action_stack, vec![8, 7]);
        assert_eq!(
            sim.source_cities.record(0),
            Some(SourceCityRecord {
                island_id: 7,
                owner_slot: 0,
                tile_x: 4,
                tile_y: 4,
                ready_at_ticks: 600,
                ..Default::default()
            })
        );
    }

    #[test]
    fn source_controller_city_arrival_installs_the_physical_city_profile() {
        let mut sim = Simulation::new();
        sim.source_time_ticks = 4_321;

        let mut ship = TradeShip::new(0, 0, 0, 0);
        ship.source_figure_kind = Some(1);
        ship.source_runtime_slot = Some(7);
        sim.trade_ships.push(ship);
        sim.source_player_controllers[0] = SourcePlayerController {
            desired_figure_count: 3,
            action_figure_handle: Some(7),
            action_target_island_id: Some(7),
            action_target_tile: Some((12, 13)),
            action_stack: vec![8, 4],
            ..Default::default()
        };

        assert!(sim.complete_source_controller_city_arrival(0));
        let controller = &sim.source_player_controllers[0];
        assert_eq!(controller.desired_figure_count, 4);
        assert_eq!(
            controller.city_management_profile,
            Some(SourceCityManagementProfile {
                city_slot: 0,
                initialized_at_ticks: 4_321,
            })
        );
        assert_eq!(
            sim.source_cities.record(0),
            Some(SourceCityRecord {
                island_id: 7,
                source_owner: 0,
                owner_slot: 0,
                tile_x: 12,
                tile_y: 13,
                ready_at_ticks: 4_921,
                ..Default::default()
            })
        );
        assert_eq!(controller.action_budget, 14);
        assert_eq!(controller.action_stack, vec![8, 7]);
        assert_eq!(controller.action_figure_handle, None);
        assert_eq!(controller.action_target_island_id, None);
        assert_eq!(controller.action_target_tile, None);
        assert_eq!(controller.action_source_candidate_tile, None);
        assert_eq!(controller.action_target_direction, None);
        assert_eq!(controller.action_arrival_retries, 0);
    }

    #[test]
    fn source_controller_state_seven_clears_work_and_queues_six_when_empty() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(2, 8, 8));
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                island_id: 2,
                source_owner: 0,
                owner_slot: 0,
                ..Default::default()
            }),
        ));
        let mut retained = [0_u8; 32];
        retained[0x1c..0x1e].copy_from_slice(&2_i16.to_le_bytes());
        let mut queue = SourceControllerCityConstructionQueue::default();
        assert!(queue.insert(SourceControllerCityConstructionWork::from_bytes(retained,)));
        sim.source_player_controllers[0] = SourcePlayerController {
            city_management_profile: Some(SourceCityManagementProfile {
                city_slot: 0,
                initialized_at_ticks: 0,
            }),
            source_city_construction_queue: queue,
            action_stack: vec![8, 7],
            action_budget: 14,
            ..Default::default()
        };

        assert!(sim.advance_source_controller_city_construction(0));
        let controller = &sim.source_player_controllers[0];
        assert!(controller
            .source_city_construction_queue
            .entries()
            .is_empty());
        assert_eq!(controller.action_stack, vec![8, 6]);
    }

    #[test]
    fn source_controller_city_arrival_requeues_state_three_without_a_city_slot() {
        let mut sim = Simulation::new();
        for slot in 0..SourceCityTable::slot_count() {
            assert!(sim.source_cities.set_record(
                slot,
                Some(SourceCityRecord {
                    island_id: 1,
                    ..Default::default()
                })
            ));
        }

        let mut ship = TradeShip::new(0, 0, 0, 0);
        ship.source_figure_kind = Some(1);
        ship.source_runtime_slot = Some(7);
        sim.trade_ships.push(ship);
        sim.source_player_controllers[0] = SourcePlayerController {
            action_figure_handle: Some(7),
            action_target_island_id: Some(7),
            action_target_tile: Some((12, 13)),
            action_stack: vec![4],
            ..Default::default()
        };

        assert!(!sim.complete_source_controller_city_arrival(0));
        let controller = &sim.source_player_controllers[0];
        assert!(controller.city_management_profile.is_none());
        assert_eq!(controller.action_stack, vec![3]);
    }

    #[test]
    fn source_controller_reinitializes_after_strict_36000_tick_age() {
        let mut sim = Simulation::new();
        sim.source_kind4_dispatch.remote_owner_dispatch_enabled = true;
        sim.source_kind4_dispatch.faction_states[0] = 0x0c;
        sim.source_time_ticks = 36_001;
        sim.source_player_controllers[0] = SourcePlayerController {
            initialized: true,
            initialized_at_ticks: 0,
            action_timer_ms: -1,
            city_management_timer_ms: -1,
            maintenance_timer_ms: -1,
            desired_figure_count: 99,
            figure_roster_ratio: 8,
            figure_capacity: 9,
            figure_capacity_limit: Some(12),
            city_management_profile: Some(SourceCityManagementProfile {
                city_slot: 0,
                initialized_at_ticks: 0,
            }),
            action_figure_handle: Some(7),
            action_target_island_id: Some(0),
            action_target_tile: Some((0, 0)),
            active_city_owner: None,
            active_city_slot: None,
            selected_city_active: true,
            city_management_disabled: false,
            action_stack: vec![9],
            action_budget: 0,
            purchase_predecessor_issued: true,
            owned_figure_handles: vec![7],
            figure_roster_dirty: false,
            ..Default::default()
        };

        sim.tick_source_player_controllers(0);

        let controller = &sim.source_player_controllers[0];
        assert_eq!(controller.initialized_at_ticks, 36_001);
        assert_eq!(controller.desired_figure_count, 3);
        assert_eq!(controller.figure_roster_ratio, 1);
        assert_eq!(controller.figure_capacity, 0);
        assert!(controller.city_management_profile.is_none());
        assert_eq!(controller.action_figure_handle, None);
        assert_eq!(controller.action_target_island_id, None);
        assert_eq!(controller.action_target_tile, None);
        assert_eq!(controller.action_arrival_retries, 0);
        assert!(!controller.selected_city_active);
        assert!(controller.action_stack.is_empty());
        assert_eq!(controller.action_budget, 0);
        assert!(!controller.purchase_predecessor_issued);
        assert!(controller.owned_figure_handles.is_empty());
        assert!(!controller.figure_roster_dirty);
    }

    #[test]
    fn source_controller_reset_gate_excludes_exactly_36000_ticks() {
        let mut sim = Simulation::new();
        sim.source_time_ticks = 36_000;
        sim.source_player_controllers[0] = SourcePlayerController {
            initialized: true,
            initialized_at_ticks: 0,
            desired_figure_count: 99,
            ..Default::default()
        };

        sim.initialize_source_player_controller(0);

        let controller = &sim.source_player_controllers[0];
        assert_eq!(controller.initialized_at_ticks, 0);
        assert_eq!(controller.desired_figure_count, 99);
    }

    #[test]
    fn source_dynamic_motion_boundary_snaps_position_and_refreshes_island_key() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(7, 10, 10));
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 3,
                figure_definition_id: 0x19,
                direction: 2,
                source_payload: 0,
                position: (1.25, 3.25),
                position_z: 0.0,
                source_energy: 1,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 0,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 0,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion {
                    remaining_distance: 0.25,
                    scalar_speed: 0.25,
                    velocity_x: 0.25,
                    velocity_y: 0.0,
                    velocity_z: 0.0,
                    terminal_motion_locked: false,
                },
            });

        sim.tick_source_dynamic_combat_motion(20);

        assert_eq!(sim.source_dynamic_combat_figures[0].position, (1.5, 3.5));
        assert_eq!(sim.source_dynamic_combat_figures[0].candidate_list_key, 7);
    }

    #[test]
    fn source_kind13_score_event_supplies_the_live_candidate_state_byte() {
        let mut sim = Simulation::new();
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 7,
                figure_definition_id: 0x19,
                direction: 0,
                source_payload: 0,
                position: (0.5, 0.5),
                position_z: 0.0,
                source_energy: 195,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 0,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 4,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            });

        assert!(sim.apply_source_kind13_score_event(4, 3));
        assert!(sim.apply_source_kind13_score_event(4, 2));
        assert!(!sim.apply_source_kind13_score_event(5, 1));
        assert_eq!(sim.source_combat_candidates()[0].source_score_state, 5);
    }

    #[test]
    fn source_kind13_action_event_queues_and_delivers_the_shared_type_one_hit() {
        let mut sim = Simulation::new();
        sim.source_dynamic_combat_figures.extend([
            SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 7,
                figure_definition_id: 0x19,
                direction: 0,
                source_payload: 0,
                position: (0.5, 0.5),
                position_z: 0.0,
                source_energy: 195,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([4, 7, 1, 0]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 0,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 0,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion {
                    remaining_distance: 1.5,
                    scalar_speed: 0.25,
                    velocity_x: 0.25,
                    velocity_y: 0.0,
                    velocity_z: 0.125,
                    terminal_motion_locked: false,
                },
            },
            SourceDynamicCombatFigure {
                active: true,
                figure_kind: 4,
                candidate_list_key: 7,
                figure_definition_id: 1,
                direction: 0,
                source_payload: 0,
                position: (1.5, 0.5),
                position_z: 0.0,
                source_energy: 60,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 1,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 1,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            },
        ]);

        assert!(sim.apply_source_kind13_score_event(0, 3));
        let selected = sim
            .source_kind13_selected_target(0)
            .expect("the source candidate row has a direct target ray");
        let action = sim
            .dispatch_source_kind13_action(0, selected)
            .expect("the state event makes the category-one action live");
        assert_eq!(action.raw_strength, 12);
        assert_eq!(action.flags, 3);
        assert_eq!(
            sim.source_dynamic_combat_figures[0].source_motion,
            combat::SourceGenericMotion {
                remaining_distance: 0.25,
                scalar_speed: 0.25,
                velocity_x: 0.25,
                velocity_y: 0.0,
                velocity_z: 0.125,
                terminal_motion_locked: false,
            }
        );
        assert_eq!(sim.source_kind13_deferred_hits.len(), 1);
        assert_eq!(
            sim.source_kind14_combat_figures,
            vec![SourceKind14CombatFigure {
                active: true,
                figure_definition_id: 113,
                position: (0.5, 0.5, 0.0),
                launcher_runtime_slot: 0,
                source_step_amount: combat::SOURCE_KIND15_STEP_AMOUNT,
                remaining_work_time: 0.96,
                source_flags: 0x14,
            }]
        );

        sim.source_time_ticks = sim.source_kind13_deferred_hits[0].due_at;
        sim.tick_source_kind4_deferred_hits();

        assert!(sim.source_kind13_deferred_hits.is_empty());
        assert_eq!(sim.source_dynamic_combat_figures[1].source_energy, 48);
    }

    #[test]
    fn source_dynamic_motion_boundary_turns_toward_the_selected_live_target() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(7, 10, 10));
        sim.source_dynamic_combat_figures.extend([
            SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 7,
                figure_definition_id: 0x19,
                direction: 2,
                source_payload: 0,
                position: (0.5, 0.5),
                position_z: 0.0,
                source_energy: 195,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 0,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 0,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            },
            SourceDynamicCombatFigure {
                active: true,
                figure_kind: 4,
                candidate_list_key: 7,
                figure_definition_id: 1,
                direction: 0,
                source_payload: 0,
                position: (1.5, 0.5),
                position_z: 0.0,
                source_energy: 60,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 1,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 1,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            },
        ]);

        sim.tick_source_dynamic_combat_motion(20);

        let attacker = &sim.source_dynamic_combat_figures[0];
        assert_eq!(attacker.direction, 0);
        assert_eq!(
            attacker.source_motion,
            combat::SourceGenericMotion::stationary_turn_delay()
        );
    }

    #[test]
    fn source_dynamic_state_four_installs_the_first_static_target_route_run() {
        let scenario = anno_formats::szs::SzsFile {
            chunks: Vec::new(),
            islands: Vec::new(),
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
        };
        let mut sim = Simulation::new();
        sim.ocean_map = Some(OceanMap::from_source_scenario(&scenario, &[]));
        sim.island_maps.push(IslandMap::new_open(7, 16, 16));
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 7,
                figure_definition_id: 0x15,
                direction: 0,
                source_payload: 0,
                position: (1.5, 3.5),
                position_z: 0.0,
                source_energy: 195,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_source_kind34_island_cell(7, 12, 3),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 0,
                state: 4,
                flags: 0,
                notification: 0,
                runtime_slot: 0,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            });

        sim.tick_source_dynamic_combat_motion(20);

        let figure = &sim.source_dynamic_combat_figures[0];
        assert_eq!(figure.direction, 2);
        assert_eq!(figure.source_motion.remaining_distance, 2.0);
        assert_eq!(figure.source_motion.scalar_speed, 0.05);
        assert_eq!(figure.source_motion.velocity_x, 0.05);
        assert_eq!(figure.source_motion.velocity_y, 0.0);
        assert_eq!(sim.source_dynamic_route_programs[0].program[0], 0x32);
    }

    #[test]
    fn source_kind6_selection_resolves_the_live_target_descriptor() {
        let mut sim = Simulation::new();
        let figure = |runtime_slot, owner, position, target_descriptor| SourceDynamicCombatFigure {
            active: true,
            figure_kind: 6,
            candidate_list_key: 4,
            figure_definition_id: 0x1f,
            direction: 0,
            source_payload: 0,
            position,
            position_z: 0.0,
            source_energy: 285,
            source_score_state: 0,
            source_action_ready_at: 0,
            source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
            target_descriptor,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner,
            state: 0,
            flags: 0,
            notification: 0,
            runtime_slot,
            auxiliary_kind: 0,
            name_index: 0,
            source_motion: combat::SourceGenericMotion::default(),
        };
        let target_descriptor = SourceTargetDescriptor::from_world_coordinate(1, 0)
            .expect("source coordinate fits the descriptor encoding");

        assert!(sim.install_source_dynamic_combat_figure(figure(
            0,
            0,
            (0.0, 0.0),
            SourceTargetDescriptor::from_bytes([0; 4]),
        )));
        assert!(sim.install_source_dynamic_combat_figure(figure(
            1,
            1,
            (1.0, 0.0),
            target_descriptor,
        )));

        let selected = sim
            .source_kind6_selected_target(0)
            .expect("the target descriptor resolves inside the source firing gate");
        assert_eq!(
            selected.target.entity,
            crate::combat::SourceCombatCandidateEntity::DynamicFigure(1)
        );
        assert_eq!(selected.metric, 8);
        assert_eq!(selected.score, 40);
        assert_eq!(selected.direction, 2);
        assert_eq!(
            selected.target_descriptor,
            SourceTargetDescriptor::from_bytes([6, 4, 1, 0])
        );
    }

    #[test]
    fn source_kind6_dispatch_emits_the_executor_record_and_updates_launcher_state() {
        let mut sim = Simulation::new();
        let figure = |runtime_slot, owner, position, target_descriptor| SourceDynamicCombatFigure {
            active: true,
            figure_kind: 6,
            candidate_list_key: 4,
            figure_definition_id: 0x1f,
            direction: 0,
            source_payload: 0,
            position,
            position_z: 0.0,
            source_energy: 285,
            source_score_state: 0,
            source_action_ready_at: 0,
            source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
            target_descriptor,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner,
            state: 0,
            flags: 0,
            notification: 0,
            runtime_slot,
            auxiliary_kind: 0,
            name_index: 0,
            source_motion: combat::SourceGenericMotion::default(),
        };
        let target_descriptor = SourceTargetDescriptor::from_world_coordinate(1, 0)
            .expect("source coordinate fits the descriptor encoding");
        assert!(sim.install_source_dynamic_combat_figure(figure(
            0,
            0,
            (0.0, 0.0),
            SourceTargetDescriptor::from_bytes([0; 4]),
        )));
        assert!(sim.install_source_dynamic_combat_figure(figure(
            1,
            1,
            (1.0, 0.0),
            target_descriptor,
        )));
        sim.source_dynamic_combat_figures[0].position_z = 3.0;
        sim.source_time_ticks = 19;

        let action = sim
            .dispatch_source_kind6_action(0, 0, false, 0)
            .expect("ready current-owner pirate has a selected target");
        assert_eq!(action.attacker_position, (0.0, 0.0));
        assert_eq!(action.attacker_runtime_slot, 0);
        assert_eq!(action.raw_strength, 6);
        assert_eq!(action.attacker_figure_kind, 6);
        assert_eq!(action.direction, 2);
        assert_eq!(action.flags, crate::combat::SOURCE_KIND6_ACTION_EVENT_FLAGS);
        assert_eq!(
            action.target_descriptor,
            SourceTargetDescriptor::from_bytes([6, 4, 1, 0])
        );
        assert_eq!(action.kind15_figure_definition_id, Some(112));
        assert_eq!(sim.source_kind6_actions, vec![action]);
        assert_eq!(
            sim.source_kind6_deferred_hits,
            vec![crate::combat::SourceKind6DeferredHit { due_at: 29, action }]
        );
        assert_eq!(sim.source_dynamic_combat_figures[0].direction, 2);
        assert_eq!(
            sim.source_dynamic_combat_figures[0].source_action_ready_at,
            69
        );
        assert_eq!(
            sim.source_kind15_combat_figures,
            vec![crate::combat::SourceKind15CombatFigure {
                active: true,
                figure_definition_id: 112,
                position: (0.5, 0.0, 7.0),
                direction: 2,
                launcher_runtime_slot: 0,
                source_step_amount: crate::combat::SOURCE_KIND15_STEP_AMOUNT,
                remaining_work_time: 0.96,
                source_flags: crate::combat::SOURCE_KIND15_EXECUTOR_FLAGS,
            }]
        );
        assert!(sim.dispatch_source_kind6_action(0, 0, false, 0).is_none());
        assert_eq!(
            sim.source_dynamic_combat_figures[0].source_motion,
            combat::source_kind6_controller_dwell_motion()
        );
    }

    #[test]
    fn source_kind6_dispatch_skips_defeated_owner_policy_rows_before_selecting_a_target() {
        let mut sim = Simulation::new();
        let figure = |runtime_slot, owner, position, target_descriptor| SourceDynamicCombatFigure {
            active: true,
            figure_kind: 6,
            candidate_list_key: 4,
            figure_definition_id: 0x1f,
            direction: 0,
            source_payload: 0,
            position,
            position_z: 0.0,
            source_energy: 285,
            source_score_state: 0,
            source_action_ready_at: 0,
            source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
            target_descriptor,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner,
            state: 0,
            flags: 0,
            notification: 0,
            runtime_slot,
            auxiliary_kind: 0,
            name_index: 0,
            source_motion: combat::SourceGenericMotion::default(),
        };
        let invalid_descriptor = SourceTargetDescriptor::from_world_coordinate(1, 0)
            .expect("source coordinate fits the descriptor encoding");
        let valid_descriptor = SourceTargetDescriptor::from_world_coordinate(2, 0)
            .expect("source coordinate fits the descriptor encoding");
        let mut invalid_target = figure(2, 1, (1.0, 0.0), invalid_descriptor);
        invalid_target.figure_kind = 1;
        invalid_target.figure_definition_id = 0x19;
        invalid_target.source_energy = 195;

        assert!(sim.install_source_dynamic_combat_figure(figure(
            0,
            0,
            (0.0, 0.0),
            SourceTargetDescriptor::from_bytes([0; 4]),
        )));
        assert!(sim.install_source_dynamic_combat_figure(invalid_target));
        assert!(sim.install_source_dynamic_combat_figure(figure(
            1,
            1,
            (2.0, 0.0),
            valid_descriptor,
        )));
        sim.source_time_ticks = 19;
        let candidates = sim.source_combat_candidates();
        let attacker = *candidates
            .iter()
            .find(|candidate| candidate.figure_kind == 6 && candidate.runtime_slot == 0)
            .expect("launcher remains in the source candidate list");
        assert_eq!(
            combat::source_kind6_ranked_candidates(&attacker, &candidates, &sim.diplomacy)[0]
                .target
                .entity,
            crate::combat::SourceCombatCandidateEntity::DynamicFigure(1)
        );

        let action = sim
            .dispatch_source_kind6_action(0, 0, true, 0x0e)
            .expect("the lower-ranked category-six row remains eligible");
        assert_eq!(
            action.target_descriptor,
            SourceTargetDescriptor::from_bytes([6, 4, 2, 0])
        );
    }

    #[test]
    fn source_kind6_controller_dispatches_then_restores_fun_00458ac0_dwell_motion() {
        let mut sim = Simulation::new();
        let figure = |runtime_slot, owner, position, target_descriptor| SourceDynamicCombatFigure {
            active: true,
            figure_kind: 6,
            candidate_list_key: 4,
            figure_definition_id: 0x1f,
            direction: 0,
            source_payload: 0,
            position,
            position_z: 0.0,
            source_energy: 285,
            source_score_state: 0,
            source_action_ready_at: 0,
            source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
            target_descriptor,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner,
            state: 0,
            flags: 0,
            notification: 0,
            runtime_slot,
            auxiliary_kind: 0,
            name_index: 0,
            source_motion: combat::SourceGenericMotion::default(),
        };
        let target_descriptor = SourceTargetDescriptor::from_world_coordinate(1, 0)
            .expect("source coordinate fits the descriptor encoding");
        assert!(sim.install_source_dynamic_combat_figure(figure(
            0,
            0,
            (0.0, 0.0),
            SourceTargetDescriptor::from_bytes([0; 4]),
        )));
        assert!(sim.install_source_dynamic_combat_figure(figure(
            1,
            1,
            (1.0, 0.0),
            target_descriptor,
        )));
        sim.source_time_ticks = 19;

        sim.tick_source_dynamic_combat_motion(20);

        assert_eq!(sim.source_kind6_actions.len(), 1);
        assert_eq!(sim.source_kind6_actions[0].attacker_runtime_slot, 0);
        assert_eq!(
            sim.source_dynamic_combat_figures[0].source_motion,
            combat::source_kind6_controller_dwell_motion()
        );
        assert_eq!(
            sim.source_dynamic_combat_figures[0].source_action_ready_at,
            69
        );
        assert_eq!(sim.source_kind6_deferred_hits.len(), 1);
    }

    #[test]
    fn deferred_category_six_map_hit_accumulates_and_emits_type_seven_terminal_event() {
        let mut sim = Simulation::new();
        let mut expected_rng = Simulation::new();
        expected_rng.seed_source_rand(23);
        let expected_ruin_draw = expected_rng.next_source_rand();
        sim.seed_source_rand(23);
        let target = SourceTargetDescriptor::from_source_kind34_island_cell(3, 4, 5);
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 6,
                candidate_list_key: 2,
                figure_definition_id: 0x1f,
                direction: 0,
                source_payload: 0,
                position: (0.0, 0.0),
                position_z: 0.0,
                source_energy: 285,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: target,
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 1,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 9,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion::default(),
            });
        let definition = anno_formats::cod::BuildingDef {
            kind: "HANDWERK".into(),
            size: (2, 3),
            source_damage_threshold: 4,
            ruinenr: 4,
            ..Default::default()
        };
        sim.buildings
            .push(crate::building::BuildingInstance::new(0, 3, 4, 5, 1));
        let mut state = SourceMapCellState::new(3, 4, 5, &definition, 0).unwrap();
        state.set_terminal_command_fields(9, 6);
        state.ruin_footprint_width = 2;
        state.ruin_footprint_height = 3;
        sim.source_static_map_roots.push(state);
        sim.source_map_cell_states.push(state);
        let action = crate::combat::SourceKind6Action {
            attacker_position: (0.0, 0.0),
            attacker_runtime_slot: 0,
            raw_strength: 6,
            attacker_figure_kind: 6,
            direction: 0,
            flags: crate::combat::SOURCE_KIND6_ACTION_EVENT_FLAGS,
            target_descriptor: SourceTargetDescriptor::from_bytes([6, 2, 9, 0]),
            kind15_figure_definition_id: Some(112),
        };
        sim.source_kind6_deferred_hits = vec![
            crate::combat::SourceKind6DeferredHit { due_at: 7, action },
            crate::combat::SourceKind6DeferredHit { due_at: 8, action },
        ];

        sim.source_time_ticks = 7;
        sim.tick_source_kind6_deferred_hits();
        assert_eq!(sim.source_map_cell_states[0].source_damage_accumulator, 2);
        assert_eq!(sim.source_kind6_deferred_hits.len(), 1);
        assert!(sim.source_kind6_terminal_events.is_empty());

        sim.source_time_ticks = 8;
        sim.tick_source_kind6_deferred_hits();
        assert!(sim.source_map_cell_states.is_empty());
        assert!(sim.source_kind6_deferred_hits.is_empty());
        assert!(!sim.buildings[0].active);
        assert_eq!(sim.buildings[0].health, 0);
        assert_eq!(
            sim.source_kind6_terminal_events,
            vec![SourceKind6TerminalEvent {
                target,
                event_kind: 7,
            }]
        );
        assert_eq!(sim.source_map_cell_revision, 1);
        assert_eq!(
            sim.tile_clears,
            vec![TileClear {
                island_id: 3,
                tile_x: 4,
                tile_y: 5,
                width: 2,
                height: 3,
                source_orientation: 0,
                source_variant: 9,
                source_map_owner_slot: 6,
                ruin_id: 4,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                source_ruin_draws: vec![expected_ruin_draw],
            }]
        );
        assert_eq!(sim.next_source_rand(), expected_rng.next_source_rand());
    }

    #[test]
    fn source_root_removal_clears_selector_and_static_inventories_once() {
        let definition = anno_formats::cod::BuildingDef {
            kind: "HANDWERK".into(),
            ..Default::default()
        };
        let state =
            SourceMapCellState::new(2, 7, 9, &definition, 0).expect("selector-bearing source root");
        let mut sim = Simulation::new();
        sim.source_map_cell_states.push(state);
        sim.source_static_map_roots.push(state);

        sim.remove_source_map_root(2, 7, 9);

        assert!(sim.source_map_cell_states.is_empty());
        assert!(sim.source_static_map_roots.is_empty());
        assert_eq!(sim.source_map_cell_revision, 1);
        sim.remove_source_map_root(2, 7, 9);
        assert_eq!(sim.source_map_cell_revision, 1);
    }

    #[test]
    fn source_footprint_removal_erases_every_overwritten_static_cell() {
        let definition = anno_formats::cod::BuildingDef {
            kind: "HANDWERK".into(),
            ..Default::default()
        };
        let first =
            SourceMapCellState::new_static(2, 7, 9, &definition, 0).expect("static source cell");
        let mut second = first;
        second.x = 8;
        let mut survivor = first;
        survivor.x = 9;
        let mut sim = Simulation::new();
        sim.source_static_map_roots = vec![first, second, survivor];

        sim.remove_source_map_footprint(2, 7, 9, 2, 1);

        assert_eq!(sim.source_static_map_roots, vec![survivor]);
        assert_eq!(sim.source_map_cell_revision, 1);
    }

    #[test]
    fn static_map_write_expands_oriented_footprint_and_replaces_destination_cells() {
        let definition = anno_formats::cod::BuildingDef {
            kind: "HANDWERK".into(),
            size: (2, 3),
            ..Default::default()
        };
        let mut root =
            SourceMapCellState::new_static(2, 7, 9, &definition, 0).expect("static source command");
        root.set_footprint(3, 2);
        root.set_source_orientation(1);
        root.set_terminal_command_fields(6, 5);
        let mut overwritten = root;
        overwritten.x = 8;
        overwritten.y = 10;
        let mut survivor = root;
        survivor.x = 10;
        let mut sim = Simulation::new();
        sim.source_static_map_roots = vec![overwritten, survivor];

        sim.replace_source_static_map_footprint(root);

        assert_eq!(sim.source_static_map_roots.len(), 7);
        for y in 9..11 {
            for x in 7..10 {
                let cell = sim
                    .source_static_map_roots
                    .iter()
                    .find(|cell| cell.matches(2, x, y))
                    .expect("oriented destination cell");
                assert_eq!(cell.source_orientation, 1);
                assert_eq!((cell.footprint_width, cell.footprint_height), (3, 2));
                assert_eq!((cell.source_variant, cell.source_map_owner_slot), (6, 5));
            }
        }
        assert!(sim.source_static_map_roots.contains(&survivor));
        assert_eq!(sim.source_map_cell_revision, 1);
    }

    #[test]
    fn terminal_replacement_repopulates_targetable_static_ruin_cells() {
        let base = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE;
        let mut constants = std::collections::HashMap::new();
        constants.insert("IDRUINE".into(), base + 1);
        let cod = anno_formats::cod::CodFile {
            constants,
            buildings: vec![anno_formats::cod::BuildingDef {
                source_id: base + 1,
                kind: "RUINE".into(),
                size: (1, 1),
                ..Default::default()
            }],
        };
        let clear = TileClear {
            island_id: 2,
            tile_x: 7,
            tile_y: 9,
            width: 1,
            height: 1,
            source_orientation: 1,
            source_variant: 3,
            source_map_owner_slot: 5,
            ruin_id: 0,
            ruin_uses_strand_table: false,
            fallback_strand_cells: 0,
            source_ruin_draws: vec![12],
        };
        let mut sim = Simulation::new();

        sim.apply_source_terminal_static_replacement(&cod, &clear);

        assert_eq!(sim.source_static_map_roots.len(), 1);
        let state = sim.source_static_map_roots[0];
        assert!(state.matches(2, 7, 9));
        assert_eq!(state.kind_code, 12);
        assert_eq!(state.source_orientation, 1);
        assert_eq!(state.source_variant, 3);
        assert_eq!(state.source_map_owner_slot, 5);
    }

    #[test]
    fn no_ruin_terminal_replays_backing_command_from_its_anchor() {
        let backing_definition = anno_formats::cod::BuildingDef {
            source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 1,
            kind: "BODEN".into(),
            size: (2, 1),
            ..Default::default()
        };
        let cod = anno_formats::cod::CodFile {
            constants: Default::default(),
            buildings: vec![backing_definition.clone()],
        };
        let mut backing_root = SourceMapCellState::new_static(2, 7, 9, &backing_definition, 0)
            .expect("backing source command");
        backing_root.set_footprint(2, 1);
        backing_root.set_source_orientation(0);
        let mut backing_cell = backing_root;
        backing_cell.x = 8;
        let clear = TileClear {
            island_id: 2,
            tile_x: 7,
            tile_y: 9,
            width: 2,
            height: 1,
            source_orientation: 1,
            source_variant: 9,
            source_map_owner_slot: 5,
            ruin_id: crate::building::NO_RUIN_ID,
            ruin_uses_strand_table: false,
            fallback_strand_cells: 0,
            source_ruin_draws: vec![],
        };
        let mut sim = Simulation::new();
        sim.source_static_map_backing_cells = vec![backing_root, backing_cell];

        sim.apply_source_terminal_static_replacement(&cod, &clear);

        assert_eq!(sim.source_static_map_roots.len(), 2);
        for x in 7..9 {
            let cell = sim
                .source_static_map_roots
                .iter()
                .find(|cell| cell.matches(2, x, 9))
                .expect("replayed backing destination");
            assert_eq!(cell.kind_code, 11);
            assert_eq!(
                (cell.source_command_anchor_x, cell.source_command_anchor_y),
                (7, 9)
            );
            assert_eq!((cell.source_variant, cell.source_map_owner_slot), (0, 5));
        }
    }

    #[test]
    fn no_ruin_terminal_replaces_backing_kind_ten_with_fixed_definition() {
        let special_definition = anno_formats::cod::BuildingDef {
            source_id: 0x58c2,
            kind: "BODEN".into(),
            ..Default::default()
        };
        let backing_definition = anno_formats::cod::BuildingDef {
            source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 1,
            kind: "WALD".into(),
            ..Default::default()
        };
        let cod = anno_formats::cod::CodFile {
            constants: Default::default(),
            buildings: vec![backing_definition.clone(), special_definition],
        };
        let backing = SourceMapCellState::new_static(2, 7, 9, &backing_definition, 0)
            .expect("kind-ten backing cell");
        let clear = TileClear {
            island_id: 2,
            tile_x: 7,
            tile_y: 9,
            width: 1,
            height: 1,
            source_orientation: 1,
            source_variant: 9,
            source_map_owner_slot: 5,
            ruin_id: crate::building::NO_RUIN_ID,
            ruin_uses_strand_table: false,
            fallback_strand_cells: 0,
            source_ruin_draws: vec![],
        };
        let mut sim = Simulation::new();
        sim.source_static_map_backing_cells.push(backing);

        sim.apply_source_terminal_static_replacement(&cod, &clear);

        assert_eq!(sim.source_static_map_roots.len(), 1);
        let restored = sim.source_static_map_roots[0];
        assert_eq!(restored.kind_code, 11);
        assert_eq!(restored.source_orientation, 1);
        assert_eq!(
            (restored.source_variant, restored.source_map_owner_slot),
            (0, 5)
        );
    }

    #[test]
    fn source_kind15_figure_expires_after_its_authored_worktime() {
        let mut sim = Simulation::new();
        sim.source_kind15_combat_figures
            .push(crate::combat::SourceKind15CombatFigure {
                active: true,
                figure_definition_id: 112,
                position: (0.5, 0.0, 4.0),
                direction: 2,
                launcher_runtime_slot: 0,
                source_step_amount: crate::combat::SOURCE_KIND15_STEP_AMOUNT,
                remaining_work_time: 0.96,
                source_flags: crate::combat::SOURCE_KIND15_EXECUTOR_FLAGS,
            });

        sim.tick(960);

        assert!(sim.source_kind15_combat_figures.is_empty());
    }

    #[test]
    fn source_city_activity_gate_counts_only_foreign_kind_four_land_units() {
        let mut sim = Simulation::new();
        sim.players = vec![Player::new_human(0), Player::new_ai(1, 0)];
        let city = SourceCityRecord {
            island_id: 3,
            source_owner: 0,
            owner_slot: 0,
            phase: 0,
            tier_population: [30, 0, 0, 0, 0],
            ..Default::default()
        };
        let mut foreign_land = MilitaryUnit::new(crate::combat::UnitType::Infantry, 1, 4, 4);
        foreign_land.source_island_id = Some(3);
        sim.military_units.push(foreign_land);
        assert!(!sim.source_city_activity_allows(city));

        sim.military_units[0].source_island_id = Some(4);
        assert!(sim.source_city_activity_allows(city));

        sim.military_units[0].source_island_id = Some(3);
        sim.military_units[0].unit_type = crate::combat::UnitType::SmallWarship;
        assert!(sim.source_city_activity_allows(city));

        sim.military_units[0].unit_type = crate::combat::UnitType::Infantry;
        sim.military_units[0].source_runtime_slot = Some(42);
        sim.source_kind4_occupants.push(SourceKind4Occupant {
            runtime_slot: 42,
            figure_definition_id: 0,
            route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
            route_retry_count: 0,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            position: (0, 0),
            island_id: 3,
            owner: 1,
            direction: 0,
            animation_state: 0,
            state_selector: 0,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            idle_timestamp_ticks: 0,
            state_flags: 0,
            state_payload: [0; 8],
            active: true,
        });
        assert!(!sim.source_city_activity_allows(city));
        assert!(sim.deactivate_source_kind4_figure(42));
        assert!(!sim.source_kind4_occupants[0].active);
        assert!(!sim.military_units[0].active);
        assert!(sim.source_city_activity_allows(city));
        assert!(!sim.deactivate_source_kind4_figure(42));
    }

    #[test]
    fn source_city_kind12_gate_uses_visible_island_and_upper_tier_population() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut city = SourceCityRecord {
            island_id: 3,
            source_owner: 0,
            owner_slot: 0,
            phase: 0,
            tier_population: [30, 0, 0, 0, 0],
            ..Default::default()
        };

        assert!(!sim.source_city_kind12_dispatch_allows(city));
        sim.set_source_island_visible(3, true);
        assert!(!sim.source_city_kind12_dispatch_allows(city));

        city.tier_population = [0, 29, 0, 0, 0];
        assert!(!sim.source_city_kind12_dispatch_allows(city));
        city.tier_population = [0, 30, 0, 0, 0];
        assert!(sim.source_city_kind12_dispatch_allows(city));

        sim.set_source_island_visible(3, false);
        assert!(!sim.source_city_kind12_dispatch_allows(city));
    }

    #[test]
    fn per_step_military_death_deactivates_matching_source_kind_four_slot() {
        let mut sim = Simulation::new();
        sim.diplomacy.set(0, 6, crate::combat::Diplomacy::War);
        sim.military_units
            .push(MilitaryUnit::new(crate::combat::UnitType::Cannon, 0, 0, 0));

        let mut source_spearman =
            MilitaryUnit::new(crate::combat::UnitType::NativeSpearman, 6, 1, 0);
        source_spearman.health = 0.1;
        source_spearman.source_island_id = Some(2);
        source_spearman.source_runtime_slot = Some(91);
        source_spearman.source_figure_definition_id = Some(33);
        source_spearman.source_origin_descriptor =
            Some(SourceTargetDescriptor::from_bytes([0x33, 2, 0, 0]));
        sim.military_units.push(source_spearman);
        sim.source_kind4_occupants.push(SourceKind4Occupant {
            runtime_slot: 91,
            figure_definition_id: 33,
            route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
            route_retry_count: 0,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes([0x33, 2, 0, 0]),
            position: (1, 0),
            island_id: 2,
            owner: 6,
            direction: 0,
            animation_state: 0,
            state_selector: 0,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            idle_timestamp_ticks: 0,
            state_flags: 0,
            state_payload: [0; 8],
            active: true,
        });

        sim.military_units[0].attack_timer_ms = 2_800;
        sim.step(200);

        assert!(!sim.military_units[1].active);
        assert!(!sim.source_kind4_occupants[0].active);
    }

    #[test]
    fn source_kind_four_occupant_tracks_slot_linked_unit_motion() {
        let mut sim = Simulation::new();
        let mut map = IslandMap::new_open(2, 30, 30);
        map.source_world_origin = (220, 260);
        sim.island_maps.push(map);
        let target_descriptor = SourceTargetDescriptor::from_source_land_route_coordinate(222, 260)
            .expect("source world coordinate fits the descriptor encoding");
        let mut unit = MilitaryUnit::new(crate::combat::UnitType::Infantry, 0, 220, 260);
        unit.source_island_id = Some(2);
        unit.source_runtime_slot = Some(91);
        unit.target_x = 222;
        unit.target_y = 260;
        unit.source_target_descriptor = Some(target_descriptor);
        sim.military_units.push(unit);
        sim.source_kind4_occupants.push(SourceKind4Occupant {
            runtime_slot: 91,
            figure_definition_id: 1,
            route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
            route_retry_count: 0,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes([0x33, 2, 0, 0]),
            position: (220, 260),
            island_id: 2,
            owner: 0,
            direction: 0,
            animation_state: 0,
            state_selector: 0,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            idle_timestamp_ticks: 0,
            state_flags: 0,
            state_payload: [0; 8],
            active: true,
        });

        sim.step(400);

        let occupant = &sim.source_kind4_occupants[0];
        assert_eq!(occupant.position, (221, 260));
        assert_eq!(occupant.direction, 2);
        assert_eq!(occupant.state_descriptor, target_descriptor);
    }

    #[test]
    fn source_kind_four_slot_sync_copies_the_live_direction_program() {
        let mut sim = Simulation::new();
        let mut unit = MilitaryUnit::new(crate::combat::UnitType::Infantry, 0, 220, 260);
        unit.source_runtime_slot = Some(91);
        unit.source_route_program[0] = 0x31;
        unit.source_route_program[1] = 0x42;
        unit.source_route_program[2] = crate::combat::SOURCE_KIND4_ROUTE_PROGRAM_TERMINATOR;
        unit.source_route_program_cursor = 1;
        unit.source_idle_remaining = 1.25;
        sim.military_units.push(unit);
        sim.source_kind4_occupants.push(SourceKind4Occupant {
            runtime_slot: 91,
            figure_definition_id: 1,
            route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
            route_retry_count: 0,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes([0x33, 2, 0, 0]),
            position: (220, 260),
            island_id: 2,
            owner: 0,
            direction: 0,
            animation_state: 0,
            state_selector: 0,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            idle_timestamp_ticks: 0,
            state_flags: 0,
            state_payload: [0; 8],
            active: true,
        });

        sim.sync_source_kind4_occupants();

        assert_eq!(
            sim.source_kind4_occupants[0].route_program[..3],
            [0x31, 0x42, 0xc1]
        );
        assert_eq!(sim.source_kind4_occupants[0].route_program_cursor, 1);
        assert_eq!(
            sim.source_kind4_occupants[0].idle_remaining_bits,
            1.25_f32.to_bits()
        );
    }

    #[test]
    fn native_idle_target_uses_anchor_relative_source_rng_offsets() {
        let mut sim = Simulation::new();
        sim.seed_source_rand(1);
        let mut map = IslandMap::new_open(10, 64, 64);
        map.source_world_origin = (220, 260);
        sim.island_maps.push(map);
        let mut unit = MilitaryUnit::new(crate::combat::UnitType::NativeSpearman, 6, 268, 302);
        unit.source_island_id = Some(10);
        unit.source_runtime_slot = Some(91);
        unit.source_origin_descriptor =
            Some(SourceTargetDescriptor::from_bytes([0x33, 10, 24, 21]));
        sim.military_units.push(unit);

        sim.assign_native_idle_targets();
        assert!(sim.military_units[0].source_target_descriptor.is_none());
        sim.source_time_ticks = 27;
        sim.source_kind4_occupants.push(SourceKind4Occupant {
            runtime_slot: 91,
            figure_definition_id: 33,
            route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
            route_retry_count: 0,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes([0x33, 10, 24, 21]),
            position: (268, 302),
            island_id: 10,
            owner: 6,
            direction: 0,
            animation_state: 0,
            state_selector: 0,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            idle_timestamp_ticks: 8,
            state_flags: 0,
            state_payload: [0; 8],
            active: true,
        });

        sim.assign_native_idle_targets();
        assert!(sim.military_units[0].source_target_descriptor.is_none());
        sim.source_time_ticks = 28;

        for _ in 0..64 {
            sim.assign_native_idle_targets();
            if sim.military_units[0].source_target_descriptor.is_some() {
                break;
            }
        }

        let unit = &sim.military_units[0];
        let descriptor = unit
            .source_target_descriptor
            .expect("native idle branch selected a target");
        assert_eq!(descriptor.kind(), 0x34);
        let map = &sim.island_maps[0];
        let target = map
            .source_world_to_local((unit.target_x, unit.target_y))
            .expect("native target remains on its island");
        assert_eq!(
            descriptor.bytes(),
            [
                0x34,
                10,
                u8::try_from(target.0).unwrap(),
                u8::try_from(target.1).unwrap(),
            ]
        );
        assert!((17..=33).contains(&target.0));
        assert!((14..=30).contains(&target.1));
        assert_eq!(sim.source_kind4_occupants[0].idle_timestamp_ticks, 36);
    }

    #[test]
    fn source_clock_promotes_scaled_dispatch_time_in_100_ms_ticks() {
        let mut sim = Simulation::new();

        sim.advance_source_clock(99);
        assert_eq!(
            (sim.source_time_ticks, sim.source_time_remainder_ms),
            (0, 99)
        );
        sim.advance_source_clock(1);
        assert_eq!(
            (sim.source_time_ticks, sim.source_time_remainder_ms),
            (1, 0)
        );
        sim.advance_source_clock(250);
        assert_eq!(
            (sim.source_time_ticks, sim.source_time_remainder_ms),
            (3, 50)
        );
    }

    #[test]
    fn source_map_dispatch_phase_advances_after_each_thousand_ms_boundary() {
        let mut sim = Simulation::new();

        sim.tick_source_map_dispatch(999);
        assert_eq!(
            (
                sim.source_map_dispatch_elapsed_ms,
                sim.source_map_dispatch_phase,
            ),
            (999, 0)
        );

        sim.tick_source_map_dispatch(1);
        assert_eq!(
            (
                sim.source_map_dispatch_elapsed_ms,
                sim.source_map_dispatch_phase,
            ),
            (0, 1)
        );
    }

    #[test]
    fn production_completion_preserves_matching_source_cell_fixed_point_state() {
        use anno_formats::cod::{BuildingDef as CodBuilding, CodFile};
        use std::collections::HashMap;

        let cod_building = CodBuilding {
            kind: "HANDWERK".into(),
            source_production_amount: 16,
            source_raw_material_amount: 64,
            storage_animation_capacity: 160,
            source_scheduler_interval: 2,
            properties: HashMap::from([
                ("ProdKind".into(), "HANDWERK".into()),
                ("Ware".into(), "WERKZEUG".into()),
                ("Rohstoff".into(), "EISEN".into()),
                ("Rohmenge".into(), "2".into()),
                ("Interval".into(), "2".into()),
                ("Maxlager".into(), "5".into()),
            ]),
            ..Default::default()
        };
        let cod = CodFile {
            constants: HashMap::new(),
            buildings: vec![cod_building.clone()],
        };
        let mut sim = Simulation::new();
        sim.building_defs = crate::data_bridge::load_building_defs(&cod);
        let mut building = BuildingInstance::new(0, 2, 7, 9, 0);
        building.input_1_stock = 5;
        sim.buildings.push(building);
        sim.source_map_cell_states
            .push(SourceMapCellState::new(2, 7, 9, &cod_building, 0).unwrap());

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();

        let state = sim.source_map_cell_states[0];
        assert_eq!(state.raw_material_stock, 160);
        assert_eq!(state.storage_fill, 0);
        assert_eq!(state.progress, 0);
        assert_eq!(state.activity, 128);
        assert_eq!(sim.source_map_cell_revision, 1);
        assert_eq!(state.scheduler_cooldown, 2);

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();

        let state = sim.source_map_cell_states[0];
        assert_eq!(state.raw_material_stock, 160);
        assert_eq!(state.storage_fill, 0);
        assert_eq!(state.scheduler_cooldown, 1);

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();

        let state = sim.source_map_cell_states[0];
        assert_eq!(state.raw_material_stock, 96);
        assert_eq!(state.storage_fill, 16);
        assert_eq!(state.progress, 16);
        assert_eq!(state.activity, 128);
        assert_eq!(sim.buildings[0].input_1_stock, 3);
        assert_eq!(sim.buildings[0].output_stock, 0);
    }

    #[test]
    fn generic_transfer_requires_the_post_update_source_storage_gate() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.building_defs = vec![
            BuildingDef {
                input_good_1: Good::Iron,
                input_1_rate: 2,
                ..Default::default()
            },
            BuildingDef {
                output_good: Good::Iron,
                ..Default::default()
            },
        ];
        sim.buildings.push(BuildingInstance::new(0, 0, 0, 0, 0));
        let mut supplier = BuildingInstance::new(1, 0, 2, 0, 0);
        supplier.output_stock = 4;
        sim.buildings.push(supplier);

        let transfer_root = CodBuilding {
            kind: "HANDWERK".into(),
            storage_animation_capacity: 64,
            source_raw_material_amount: 64,
            properties: [
                ("ProdKind".into(), "HANDWERK".into()),
                ("Rohstoff".into(), "EISEN".into()),
            ]
            .into(),
            ..Default::default()
        };
        let supplier_root = CodBuilding {
            kind: "HANDWERK".into(),
            storage_animation_capacity: 320,
            ..Default::default()
        };
        let mut root = SourceMapCellState::new(0, 0, 0, &transfer_root, 0).unwrap();
        root.storage_fill = 64;
        let mut supplier_state = SourceMapCellState::new(0, 2, 0, &supplier_root, 0).unwrap();
        supplier_state.storage_fill = 128;
        sim.source_map_cell_states = vec![root, supplier_state];

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();
        assert!(sim.figures.is_empty());

        sim.source_map_cell_states[0].storage_fill = 0;
        sim.source_map_cell_states[0].scheduler_cooldown = 0;
        sim.buildings[0].output_stock = 0;
        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();

        assert_eq!(sim.figures.len(), 1);
        assert_eq!(sim.figures[0].carried_good, Good::Iron as u8);
        assert_eq!(sim.source_map_cell_states[1].reserved_storage, 128);
    }

    #[test]
    fn city_cart_supplier_view_uses_karren_path_class_without_mutating_traeger_view() {
        let supplier = carrier::CarrierSupplier {
            island: 3,
            owner: 1,
            x: 9,
            y: 12,
            good: Good::Cloth,
            available: 4,
            storage: carrier::CarrierSupplierStorage::SourceRoot,
            source_path_class: carrier::source_path_class(100),
            source_footprint: (1, 1),
        };
        let building = BuildingInstance::new(0, 3, 9, 12, 1);
        let definition = BuildingDef {
            wegspeed: [100, 100, 150, 100],
            ..Default::default()
        };

        let city_view =
            Simulation::city_cart_supplier_view(&[supplier], &[building], &[definition]);

        assert_eq!(supplier.source_path_class, 32);
        assert_eq!(city_view[0].source_path_class, 48);
    }

    #[test]
    fn source_carrier_consumes_each_ready_motion_quantum() {
        let mut sim = Simulation::new();
        let mut carrier = Figure::new();
        carrier.action = ActionType::CarryingGoods;
        carrier.speed = 4;
        carrier.source_move_speed = 220;
        carrier.target_x = 1;
        carrier.path = vec![(1, 0)];
        sim.figures.push(carrier);

        sim.tick_entities(200);

        let carrier = &sim.figures[0];
        assert_eq!(carrier.move_timer_ms, 0);
        assert_eq!((carrier.tile_x, carrier.tile_y), (0, 0));
        assert!((carrier.source_step_remaining - 0.9296).abs() < 0.000_001);
        assert!((carrier.source_position_x - 0.5704).abs() < 0.000_001);
        assert_eq!(carrier.source_position_y, 0.5);
    }

    #[test]
    fn type11_city_eligibility_uses_the_live_source_city_population() {
        let mut sim = Simulation::new();
        sim.warehouses.push(Warehouse::new(3, 4, 8, 9));
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                island_id: 3,
                source_owner: 4,
                owner_slot: 4,
                tier_population: [12, 34, 56, 78, 90],
                ..SourceCityRecord::default()
            })
        ));

        sim.sync_source_city_populations_to_warehouses();

        assert_eq!(sim.warehouses[0].city_population, [12, 34, 56, 78, 90]);
    }

    #[test]
    fn type11_city_capacity_counts_only_the_origin_map_owner_roots() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let market = CodBuilding {
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "MARKT".into())].into(),
            ..Default::default()
        };
        let mut sim = Simulation::new();
        let mut first = SourceMapCellState::new(5, 1, 1, &market, 0).unwrap();
        first.source_map_owner_slot = 2;
        let mut second = first;
        second.x = 2;
        let mut other_owner = first;
        other_owner.x = 3;
        other_owner.source_map_owner_slot = 3;
        sim.source_map_cell_states = vec![first, second, other_owner];

        assert_eq!(sim.source_city_transfer_root_count(5, 2), 2);
        assert_eq!(sim.source_city_transfer_root_count(5, 3), 1);
    }

    #[test]
    fn type11_city_owner_resolves_through_the_source_city_record() {
        let mut sim = Simulation::new();
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                island_id: 5,
                source_owner: 2,
                owner_slot: 4,
                ..SourceCityRecord::default()
            })
        ));

        assert_eq!(sim.source_city_player_owner(5, 2), Some(4));
        assert_eq!(sim.source_city_player_owner(5, 3), None);
    }

    #[test]
    fn type11_kontor_uses_its_owner_city_record_away_from_the_root_anchor() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.building_defs = vec![BuildingDef {
            output_good: Good::Cloth,
            ..Default::default()
        }];
        let mut supplier = BuildingInstance::new(0, 2, 2, 0, 0);
        supplier.output_stock = 3;
        supplier.owner = 4;
        sim.buildings.push(supplier);
        sim.warehouses.push(Warehouse::with_capacity_and_population(
            2,
            4,
            1,
            0,
            50,
            [0, 100, 0, 0, 0],
        ));
        assert!(sim.source_cities.set_record(
            0,
            Some(SourceCityRecord {
                island_id: 2,
                source_owner: 2,
                owner_slot: 4,
                tier_population: [0, 100, 0, 0, 0],
                ..SourceCityRecord::default()
            })
        ));
        sim.island_maps.push(IslandMap::new_open(2, 3, 1));

        let kontor = CodBuilding {
            kind: "HQ".into(),
            storage_animation_capacity: 160,
            source_transfer_radius: 16,
            source_transfer_figure_limit: 1,
            properties: [
                ("ProdKind".into(), "KONTOR".into()),
                ("Figurnr".into(), "KARREN".into()),
            ]
            .into(),
            ..Default::default()
        };
        let workshop = CodBuilding {
            kind: "HANDWERK".into(),
            storage_animation_capacity: 320,
            ..Default::default()
        };
        let mut root = SourceMapCellState::new(2, 0, 0, &kontor, 0).unwrap();
        root.source_map_owner_slot = 2;
        let mut supplier_state = SourceMapCellState::new(2, 2, 0, &workshop, 0).unwrap();
        supplier_state.source_map_owner_slot = 2;
        supplier_state.storage_fill = 128;
        sim.source_map_cell_states = vec![root, supplier_state];

        sim.tick_production();

        assert!(sim.figures.is_empty());
        assert_eq!(sim.source_map_cell_states[0].phase, 0);

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();

        assert_eq!(sim.figures.len(), 1);
        assert_eq!(sim.figures[0].cargo_route, CargoRoute::CityCart);
        assert_eq!(sim.figures[0].owner, 4);
        assert_eq!(sim.figures[0].origin_source_map_owner_slot, 2);
        assert_eq!(sim.source_map_cell_states[0].scheduler_cooldown, 11);

        for _ in 0..100 {
            if sim.figures.is_empty() {
                break;
            }
            sim.tick_entities(100);
        }

        assert!(sim.figures.is_empty());
        assert_eq!(sim.warehouses[0].city_stock_fixed(Good::Cloth), 65);
    }

    #[test]
    fn generic_carrier_collects_and_tops_up_at_supplier_anchor_after_reaching_footprint_edge() {
        use crate::types::Good;
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.building_defs = vec![
            BuildingDef {
                input_good_1: Good::Iron,
                input_1_rate: 2,
                input_good_2: Good::Iron,
                input_2_rate: 2,
                ..Default::default()
            },
            BuildingDef {
                output_good: Good::Iron,
                ..Default::default()
            },
        ];
        sim.buildings.push(BuildingInstance::new(0, 4, 1, 1, 0));
        let mut supplier = BuildingInstance::new(1, 4, 3, 2, 0);
        supplier.output_stock = 3;
        sim.buildings.push(supplier);
        let source_definition = CodBuilding {
            kind: "HANDWERK".into(),
            ..Default::default()
        };
        sim.source_map_cell_states
            .push(SourceMapCellState::new(4, 1, 1, &source_definition, 0).unwrap());
        let mut supplier_state = SourceMapCellState::new(4, 3, 2, &source_definition, 0).unwrap();
        supplier_state.storage_fill = 128;
        supplier_state.reserved_storage = 65;
        sim.source_map_cell_states.push(supplier_state);
        let mut carrier = Figure::new();
        carrier.action = ActionType::CarryingGoods;
        carrier.speed = 4;
        carrier.tile_x = 4;
        carrier.tile_y = 2;
        carrier.target_x = 4;
        carrier.target_y = 2;
        carrier.destination_kind = 1;
        carrier.supplier_x = 3;
        carrier.supplier_y = 2;
        carrier.building_idx = 0;
        carrier.carried_good = Good::Iron as u8;
        carrier.carried_amount = 2;
        carrier.cargo_fixed = 65;
        sim.figures.push(carrier);

        sim.tick_entities(100);
        sim.tick_entities(100);
        sim.tick_entities(100);
        sim.tick_entities(100);

        assert_eq!(sim.buildings[1].output_stock, 0);
        assert_eq!(sim.source_map_cell_states[1].storage_fill, 0);
        assert_eq!(sim.source_map_cell_states[1].reserved_storage, 0);
        assert_eq!(sim.buildings[0].input_1_stock, 0);
        assert_eq!(sim.buildings[0].input_2_stock, 4);
        assert_eq!(sim.source_map_cell_states[0].raw_material_stock, 0);
        assert_eq!(sim.source_map_cell_states[0].work_material_stock, 128);
        assert!(sim.figures.is_empty());
    }

    #[test]
    fn city_cart_collects_highest_score_supplier_and_credits_market_inventory() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.building_defs = vec![BuildingDef {
            output_good: Good::Cloth,
            ..Default::default()
        }];
        let mut supplier = BuildingInstance::new(0, 0, 2, 0, 0);
        supplier.output_stock = 3;
        sim.buildings.push(supplier);
        sim.warehouses.push(Warehouse::new(0, 0, 0, 0));
        sim.island_maps.push(IslandMap::new_open(0, 3, 1));

        let market = CodBuilding {
            kind: "GEBAEUDE".into(),
            storage_animation_capacity: 160,
            source_transfer_figure_limit: 1,
            properties: [
                ("ProdKind".into(), "MARKT".into()),
                ("Figurnr".into(), "KARREN".into()),
            ]
            .into(),
            ..Default::default()
        };
        let workshop = CodBuilding {
            kind: "HANDWERK".into(),
            storage_animation_capacity: 320,
            ..Default::default()
        };
        sim.source_map_cell_states
            .push(SourceMapCellState::new(0, 0, 0, &market, 0).unwrap());
        let mut supplier_state = SourceMapCellState::new(0, 2, 0, &workshop, 0).unwrap();
        supplier_state.storage_fill = 65;
        sim.source_map_cell_states.push(supplier_state);
        let city_capacity = sim.warehouses[0].city_storage_capacity_fixed(1);
        assert_eq!(
            sim.warehouses[0].deposit_city_good_fixed(
                Good::Cloth,
                city_capacity as u16 - 32,
                city_capacity,
            ),
            city_capacity as u16 - 32
        );

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();
        assert_eq!(sim.figures.len(), 1);
        assert_eq!(sim.figures[0].cargo_route, CargoRoute::CityCart);
        let event_slot = sim.figures[0]
            .source_event_slot
            .expect("type-11 cart owns its source event");
        assert_eq!(
            sim.source_figure_events
                .slot(event_slot)
                .unwrap()
                .route_program[..2],
            [0x32, crate::source_route::SOURCE_ROUTE_TERMINATOR]
        );
        assert_eq!(sim.source_map_cell_states[1].reserved_storage, 65);
        sim.source_map_cell_states[1].storage_fill += 64;

        for _ in 0..100 {
            if sim.figures.is_empty() {
                break;
            }
            sim.tick_entities(100);
        }

        assert!(sim.figures.is_empty());
        assert_eq!(sim.buildings[0].output_stock, 0);
        assert_eq!(sim.source_map_cell_states[1].storage_fill, 0);
        assert_eq!(sim.source_map_cell_states[1].reserved_storage, 0);
        assert_eq!(sim.warehouses[0].stock(Good::Cloth), 50);
        assert_eq!(
            sim.warehouses[0].city_stock_fixed(Good::Cloth),
            city_capacity as u16
        );
        assert_eq!(sim.source_map_cell_states[0].progress, 129);
        assert_eq!(
            sim.source_figure_events
                .slot(event_slot)
                .unwrap()
                .transfer_amount_fixed,
            129
        );
    }

    #[test]
    fn plantation_worker_reserves_harvests_and_returns_to_its_root() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(0, 5, 1));

        let plantation = CodBuilding {
            kind: "GEBAEUDE".into(),
            source_transfer_radius: 3,
            properties: [
                ("ProdKind".into(), "PLANTAGE".into()),
                ("Rohstoff".into(), "GETREIDE".into()),
                ("Figurnr".into(), "MAEHER".into()),
            ]
            .into(),
            ..Default::default()
        };
        let raw_resource = CodBuilding {
            kind: "ROHSTOFF".into(),
            gfx: 100,
            source_resource_growth_factor: 128,
            properties: [
                ("Ware".into(), "GETREIDE".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..Default::default()
        };
        let mut root = SourceMapCellState::new(0, 0, 0, &plantation, 0).unwrap();
        root.source_map_owner_slot = 0;
        root.set_footprint(3, 1);
        let mut resource = SourceMapCellState::new_static(0, 4, 0, &raw_resource, 0).unwrap();
        resource.source_map_owner_slot = 0;
        sim.source_map_cell_states.push(root);
        sim.source_static_map_roots.push(resource);

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();

        assert_eq!(sim.figures.len(), 1);
        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::Searching
        );
        assert_eq!(sim.figures[0].carried_good, 0x2d);
        assert_eq!(sim.figures[0].source_animation_state, 1);
        assert_eq!(
            (
                sim.figures[0].source_worker_home_x,
                sim.figures[0].source_worker_home_y,
            ),
            (1, 0)
        );
        assert!(!sim.source_static_map_roots[0].source_resource_reserved);
        let event_slot = sim.figures[0]
            .source_event_slot
            .expect("plantation worker owns a source event");
        assert!(!sim
            .source_figure_events
            .slot(event_slot)
            .expect("worker event slot is present")
            .is_free());
        sim.source_map_cell_states[0].scheduler_cooldown = 11;
        sim.source_map_cell_states[0].source_production_time = 48;

        sim.tick_entities(99);
        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::Searching
        );
        assert!(!sim.source_static_map_roots[0].source_resource_reserved);

        sim.tick_entities(1);
        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::ToResource
        );
        assert!(sim.source_static_map_roots[0].source_resource_reserved);
        assert_eq!(sim.figures[0].source_animation_state, 1);

        for _ in 0..100 {
            if sim.figures[0].source_worker_route == SourceWorkerRoute::Harvesting {
                break;
            }
            sim.tick_entities(100);
        }

        assert_eq!(sim.source_static_map_roots[0].kind_code, 9);
        assert!(sim.source_static_map_roots[0].source_resource_reserved);
        assert_eq!(sim.figures[0].action, ActionType::CarryingGoods);
        assert_eq!(sim.figures[0].source_animation_state, 2);
        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::Harvesting
        );
        assert_eq!(
            sim.source_figure_events
                .slot(event_slot)
                .expect("worker event slot is retained")
                .lifecycle,
            2
        );

        sim.tick_entities(100);
        assert_eq!(sim.source_static_map_roots[0].source_definition_offset, 99);
        assert_eq!(sim.source_static_map_roots[0].kind_code, 10);
        assert!(!sim.source_static_map_roots[0].source_resource_reserved);
        assert_eq!(sim.figures.len(), 1);
        assert_eq!(sim.figures[0].source_animation_state, 0);
        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::ReturningSearch
        );

        sim.tick_entities(100);
        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::Returning
        );
        assert_eq!((sim.figures[0].target_x, sim.figures[0].target_y), (1, 0));

        for _ in 0..100 {
            if sim.figures.is_empty() {
                break;
            }
            sim.tick_entities(100);
        }

        assert!(sim.figures.is_empty());
        assert_eq!(sim.source_map_cell_states[0].scheduler_cooldown, 0);
        assert_eq!(sim.source_map_cell_states[0].source_production_time, 37);
        assert!(sim
            .source_figure_events
            .slot(event_slot)
            .expect("released worker event slot is retained")
            .is_free());
    }

    #[test]
    fn plantation_worker_retries_after_ignoring_raw_resources_outside_its_authored_radius() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(0, 4, 1));
        let plantation = CodBuilding {
            kind: "GEBAEUDE".into(),
            source_transfer_radius: 2,
            properties: [
                ("ProdKind".into(), "PLANTAGE".into()),
                ("Rohstoff".into(), "GETREIDE".into()),
                ("Figurnr".into(), "MAEHER".into()),
            ]
            .into(),
            ..Default::default()
        };
        let raw_resource = CodBuilding {
            kind: "ROHSTOFF".into(),
            properties: [
                ("Ware".into(), "GETREIDE".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..Default::default()
        };
        sim.source_map_cell_states
            .push(SourceMapCellState::new(0, 0, 0, &plantation, 0).unwrap());
        sim.source_static_map_roots
            .push(SourceMapCellState::new_static(0, 3, 0, &raw_resource, 0).unwrap());

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();

        assert_eq!(sim.figures.len(), 1);
        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::Searching
        );
        assert!(!sim.source_static_map_roots[0].source_resource_reserved);
        sim.tick_entities(100);
        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::Searching
        );
        assert!(!sim.source_static_map_roots[0].source_resource_reserved);

        sim.source_static_map_roots
            .push(SourceMapCellState::new_static(0, 1, 0, &raw_resource, 0).unwrap());
        sim.tick_entities(100);

        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::ToResource
        );
        assert!(!sim.source_static_map_roots[0].source_resource_reserved);
        assert!(sim.source_static_map_roots[1].source_resource_reserved);
    }

    #[test]
    fn plantation_worker_selects_a_fixed_grass_terrain_target() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(0, 3, 1));
        let plantation = CodBuilding {
            kind: "GEBAEUDE".into(),
            source_transfer_radius: 3,
            properties: [
                ("ProdKind".into(), "PLANTAGE".into()),
                ("Rohstoff".into(), "GRAS".into()),
                ("Figurnr".into(), "PFLUECKER".into()),
            ]
            .into(),
            ..Default::default()
        };
        let grass = CodBuilding {
            kind: "BODEN".into(),
            properties: [
                ("Ware".into(), "GRAS".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..Default::default()
        };
        sim.source_map_cell_states
            .push(SourceMapCellState::new(0, 0, 0, &plantation, 0).unwrap());
        sim.source_static_map_roots
            .push(SourceMapCellState::new_static(0, 2, 0, &grass, 0).unwrap());

        sim.tick_source_map_dispatch(1_000);
        sim.tick_production();
        assert_eq!(sim.figures.len(), 1);

        sim.tick_entities(100);

        assert_eq!(
            sim.figures[0].source_worker_route,
            SourceWorkerRoute::ToResource
        );
        assert_eq!((sim.figures[0].target_x, sim.figures[0].target_y), (2, 0));
        assert!(sim.source_static_map_roots[0].source_resource_reserved);
    }

    #[test]
    fn source_resource_environment_scans_raw_cells_before_the_deadline() {
        use anno_formats::cod::BuildingDef as CodBuilding;
        use anno_formats::szs::IslandSourceResourceState;

        let mut sim = Simulation::new();
        sim.island_maps
            .push(IslandMap::new_open(0, 1, 1).with_source_resource_state(
                IslandSourceResourceState {
                    attenuation: 128,
                    transition_deadline_ticks: 1,
                    ..Default::default()
                },
            ));
        let raw = CodBuilding {
            kind: "ROHSTOFF".into(),
            gfx: 100,
            properties: [("Ware".into(), "GETREIDE".into())].into(),
            ..Default::default()
        };
        sim.source_static_map_roots
            .push(SourceMapCellState::new_static(0, 0, 0, &raw, 0).unwrap());

        sim.tick_source_resource_environment(30_000);

        let cell = sim.source_static_map_roots[0];
        assert_eq!(cell.source_definition_offset, 101);
        assert_eq!(cell.kind_code, 10);
        assert!(cell.source_resource_is_dry);
        assert_eq!(sim.island_maps[0].source_resource_attenuation(), 128);
    }

    #[test]
    fn source_resource_environment_decays_and_restores_dry_cells() {
        use anno_formats::cod::BuildingDef as CodBuilding;
        use anno_formats::szs::IslandSourceResourceState;

        let mut sim = Simulation::new();
        sim.island_maps
            .push(IslandMap::new_open(0, 1, 1).with_source_resource_state(
                IslandSourceResourceState {
                    attenuation: 64,
                    crop_flags: 1,
                    ..Default::default()
                },
            ));
        let raw = CodBuilding {
            kind: "ROHSTOFF".into(),
            gfx: 100,
            properties: [("Ware".into(), "GETREIDE".into())].into(),
            ..Default::default()
        };
        let mut dry = SourceMapCellState::new_static(0, 0, 0, &raw, 0).unwrap();
        dry.source_resource_growth_factor = 128;
        assert!(dry.replace_harvested_raw_resource(SourceResourceHarvestTransition::Drought));
        sim.source_static_map_roots.push(dry);

        sim.tick_source_resource_environment(30_000);

        let cell = sim.source_static_map_roots[0];
        assert_eq!(cell.source_definition_offset, 100);
        assert_eq!(cell.kind_code, 9);
        assert!(!cell.source_resource_is_dry);
        assert_eq!(sim.island_maps[0].source_resource_attenuation(), 32);
    }

    #[test]
    fn type17_terrain_event_replays_target_harvest_and_failed_route_cleanup() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(0, 3, 3));
        let grass = |x, y| SourceMapCellState {
            kind_code: 9,
            source_production_kind_code: 9,
            source_output_ware_slot: 0x35,
            ..SourceMapCellState::new_static(
                0,
                x,
                y,
                &CodBuilding {
                    kind: "ROHSTOFF".into(),
                    properties: [("Ware".into(), "ERDE".into())].into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        };
        let terrain_target = SourceMapCellState {
            kind_code: 1,
            source_production_kind_code: 9,
            source_output_ware_slot: 0x34,
            ..SourceMapCellState::new_static(
                0,
                2,
                1,
                &CodBuilding {
                    kind: "ROHSTOFF".into(),
                    properties: [("Ware".into(), "GRAS".into())].into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        };
        sim.source_static_map_roots = vec![
            grass(1, 1),
            grass(0, 1),
            grass(1, 0),
            grass(1, 2),
            terrain_target,
        ];
        let mut rows = [SourceTerrainEventSchedule::default(); 8];
        rows[0] = SourceTerrainEventSchedule {
            island_id: 0,
            x: 1,
            y: 1,
            due_at_ticks: 0,
        };
        sim.source_terrain_event_schedules.push(rows);
        sim.source_resource_environment_phase = 1;
        sim.source_resource_environment_last_phase = vec![0];

        sim.tick_source_resource_environment(0);
        assert_eq!(sim.figures.len(), 1);
        assert!(sim.figures[0].source_terrain_event_active);
        assert_eq!(sim.figures[0].sprite_set, 0x59);
        let slot = sim.figures[0]
            .source_event_slot
            .expect("terrain event owns a shared source slot");
        assert_eq!(sim.source_figure_events.slot(slot).unwrap().owner, 7);

        sim.tick_entities(100);
        assert_eq!(
            (
                sim.source_figure_events.slot(slot).unwrap().target_x,
                sim.source_figure_events.slot(slot).unwrap().target_y,
            ),
            (2, 1)
        );
        assert!(sim.source_static_map_roots[4].source_resource_reserved);

        let route_steps = sim.figures[0].source_event_route_steps.len();
        assert!(sim
            .source_figure_events
            .set_kind12_route_progress(slot, route_steps));
        sim.figures[0].tile_x = 2;
        sim.figures[0].tile_y = 1;
        sim.tick_entities(100);
        assert_eq!(sim.source_figure_events.slot(slot).unwrap().lifecycle, 1);
        sim.tick_entities(100);
        let harvested = sim.source_figure_events.slot(slot).unwrap();
        assert_eq!((harvested.target_x, harvested.target_y), (-1, -1));
        assert_eq!(harvested.lifecycle, 0);
        assert_eq!(harvested.resource_ware_slot, 0x34);
        assert!(!sim.source_static_map_roots[4].source_resource_reserved);

        sim.source_static_map_roots[4].kind_code = 9;
        sim.tick_entities(100);
        assert_eq!(sim.source_figure_events.slot(slot).unwrap().lifecycle, 2);
        assert!(sim.source_terrain_event_schedules[0][0].is_free());
        sim.tick_entities(100);
        assert!(sim.figures.is_empty());
        assert!(sim.source_figure_events.slot(slot).unwrap().is_free());
    }

    #[test]
    fn terrain_event_due_row_requires_two_grass_neighbours() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(0, 3, 3));
        let grass = CodBuilding {
            kind: "ROHSTOFF".into(),
            properties: [("Ware".into(), "ERDE".into())].into(),
            ..Default::default()
        };
        for (x, y) in [(1, 1), (0, 1)] {
            sim.source_static_map_roots.push(SourceMapCellState {
                kind_code: 9,
                source_production_kind_code: 9,
                source_output_ware_slot: 0x35,
                ..SourceMapCellState::new_static(0, x, y, &grass, 0).unwrap()
            });
        }
        let mut rows = [SourceTerrainEventSchedule::default(); 8];
        rows[0] = SourceTerrainEventSchedule {
            island_id: 0,
            x: 1,
            y: 1,
            due_at_ticks: 0,
        };
        sim.source_terrain_event_schedules.push(rows);
        sim.source_resource_environment_phase = 1;
        sim.source_resource_environment_last_phase = vec![0];

        sim.tick_source_resource_environment(0);

        assert!(sim.source_terrain_event_schedules[0][0].is_free());
        assert!(sim.figures.is_empty());
    }

    #[test]
    fn terrain_event_candidate_scan_emits_one_due_row_after_four_island_phases() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(0, 7, 3));
        let source_grass = CodBuilding {
            kind: "ROHSTOFF".into(),
            properties: [("Ware".into(), "ERDE".into())].into(),
            ..Default::default()
        };
        for y in 0..3 {
            for x in 0..7 {
                sim.source_static_map_roots.push(SourceMapCellState {
                    kind_code: 9,
                    source_production_kind_code: 9,
                    source_output_ware_slot: 0x35,
                    ..SourceMapCellState::new_static(0, x, y, &source_grass, 0).unwrap()
                });
            }
        }
        sim.seed_source_rand(1);
        sim.source_time_ticks = 100;
        sim.source_resource_environment_phase = 1;
        sim.source_resource_environment_last_phase = vec![0];
        sim.source_terrain_event_schedule_counters = vec![3];

        sim.tick_source_resource_environment(0);

        assert_eq!(sim.source_terrain_event_schedule_counters, vec![0]);
        assert_eq!(sim.source_terrain_event_schedules.len(), 1);
        let rows = sim.source_terrain_event_schedules[0];
        assert_eq!(rows.iter().filter(|row| !row.is_free()).count(), 1);
        let row = rows.into_iter().find(|row| !row.is_free()).unwrap();
        assert_eq!(row.island_id, 0);
        assert_eq!(row.due_at_ticks, 700);
        assert!(matches!((row.x, row.y), (1 | 3 | 5, 1)));
        assert_eq!(sim.figures.len(), 1);
        assert_eq!(
            (sim.figures[0].origin_x, sim.figures[0].origin_y),
            (u16::from(row.x), u16::from(row.y))
        );
    }

    #[test]
    fn ai_request_build_places_building() {
        use crate::ai::AiAction;
        use crate::building::BuildingDef;
        use crate::types::{Good, ProductionType};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 5_000;
        // One open island map for AI.
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.warehouses.push(Warehouse::new(0, 1, 15, 15));
        // A single buildable def: a Tools workshop.
        sim.building_defs.push(BuildingDef {
            id: 0,
            category: 0,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools,
            input_good_1: Good::Iron,
            input_good_2: Good::None,
            output_rate: 1,
            input_1_rate: 1,
            input_2_rate: 0,
            storage_capacity: 50,
            cycle_time_ms: 1000,
            cost_gold: 500,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        });
        // Drive the build path manually.
        let action = AiAction::RequestBuild {
            good: Good::Tools,
            priority: 0,
        };
        // Inline the dispatch loop (tick_ai runs the controller too, which
        // would emit its own actions; we want a focused single-action test).
        let owner = 1u8;
        let player_idx = owner as usize;
        let gold_before = sim.players[player_idx].gold;
        match action {
            AiAction::RequestBuild { good, .. } => {
                let pick = sim
                    .building_defs
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| {
                        d.output_good == good && d.cost_gold as i32 <= sim.players[player_idx].gold
                    })
                    .min_by_key(|(_, d)| d.cost_gold);
                let (def_id, def) = pick.unwrap();
                let wh = sim.warehouses.iter().find(|w| w.owner == owner).unwrap();
                let cx = wh.tile_x;
                let cy = wh.tile_y;
                let w = def.width as u16;
                let h = def.height as u16;
                let cost = def.cost_gold;
                let map_idx = sim
                    .island_maps
                    .iter()
                    .position(|m| m.island_id == wh.island_id)
                    .unwrap();
                let spot = sim.island_maps[map_idx]
                    .find_open_spot(cx, cy, w, h, 12)
                    .unwrap();
                for dy in 0..h {
                    for dx in 0..w {
                        sim.island_maps[map_idx].set_walkable(spot.0 + dx, spot.1 + dy, false);
                    }
                }
                let mut inst =
                    BuildingInstance::new(def_id as u16, wh.island_id, spot.0, spot.1, owner);
                inst.construction_ms_total = 8_000;
                inst.construction_ms_remaining = 8_000;
                sim.buildings.push(inst);
                sim.players[player_idx].gold -= cost as i32;
            }
            _ => unreachable!(),
        }
        assert_eq!(sim.buildings.len(), 1);
        assert_eq!(sim.buildings[0].owner, owner);
        assert!(sim.players[player_idx].gold < gold_before);
        assert!(!sim.buildings[0].is_built()); // construction in progress
    }

    #[test]
    fn scenario_complete_flips_on_last_objective() {
        use crate::objectives::{Objective, ObjectiveSet};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        // Single objective: reach 10 000 gold.
        sim.objectives = ObjectiveSet::new(vec![Objective::AccumulateGold { amount: 10_000 }]);

        // Below threshold — scenario_complete must stay false.
        sim.players[0].gold = 1_000;
        sim.tick_population();
        assert!(!sim.scenario_complete);
        assert!(!sim.event_log.iter().any(|l| l.starts_with("[victory]")));

        // Cross the threshold — the next tick must flip
        // scenario_complete and emit the victory line exactly
        // once.
        sim.players[0].gold = 12_000;
        sim.tick_population();
        assert!(sim.scenario_complete);
        let victory_lines: usize = sim
            .event_log
            .iter()
            .filter(|l| l.starts_with("[victory]"))
            .count();
        assert_eq!(
            victory_lines, 1,
            "victory line should fire exactly once on the flip"
        );

        // Tick again — must NOT re-emit (edge-triggered).
        sim.event_log.clear();
        sim.tick_population();
        assert!(sim.scenario_complete);
        assert!(
            !sim.event_log.iter().any(|l| l.starts_with("[victory]")),
            "victory line is edge-triggered"
        );
    }

    #[test]
    fn ai_picks_fertile_warehouse_for_fertility_bound_plantation() {
        // AI has two warehouses: island 0 (barren, listed
        // first) and island 1 (Cocoa-fertile). When asked to
        // build a Cocoa plantation, the warehouse selector
        // must skip island 0 and anchor on island 1 even
        // though it appears later.
        use crate::building::{BuildingDef, OreDeposit};
        use crate::types::{Good, ProductionType};
        use anno_formats::szs::Fertility;

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 50_000;

        // Island 0: barren. Island 1: Cocoa-fertile.
        let mut barren = IslandMap::new_open(0, 30, 30);
        barren.fertilities = [7; 8];
        sim.island_maps.push(barren);
        let mut cocoa = IslandMap::new_open(1, 30, 30);
        cocoa.fertilities = [6, 7, 7, 7, 7, 7, 7, 7];
        sim.island_maps.push(cocoa);

        // Two warehouses owned by AI 1, in the order
        // (barren, fertile) — the natural `find` would pick
        // the barren one first.
        sim.warehouses.push(Warehouse::new(0, 1, 15, 15));
        sim.warehouses.push(Warehouse::new(1, 1, 15, 15));

        // Cocoa plantation, fertility-bound.
        let cocoa_def = BuildingDef {
            id: 0,
            category: 0,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "PLANTAGE".into(),
            radius: 0,
            output_good: Good::Cocoa,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 1,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 50,
            cycle_time_ms: 1000,
            cost_gold: 200,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: true,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: Some(Fertility::Cocoa),
        };
        sim.building_defs.push(cocoa_def);

        // Replay the warehouse-selection logic the inline
        // RequestBuild handler performs.
        let owner = 1u8;
        let def = &sim.building_defs[0];
        let wh = sim.warehouses.iter().find(|w| {
            if !(w.active && w.owner == owner) {
                return false;
            }
            match def.required_fertility {
                None => true,
                Some(req) => sim
                    .island_maps
                    .iter()
                    .find(|m| m.island_id == w.island_id)
                    .map(|m| m.active_fertilities().contains(&req))
                    .unwrap_or(false),
            }
        });
        let wh = wh.expect("a fertile warehouse must be found");
        assert_eq!(
            wh.island_id, 1,
            "must pick the Cocoa-fertile island, not the first warehouse"
        );
    }

    #[test]
    fn ai_build_path_blocks_fertility_bound_plantation_on_barren_island() {
        // Direct exercise of the simulation's RequestBuild
        // handler, bypassing the priority-list early-return so we
        // focus on the fertility filter only. The handler is
        // inline in tick_ai but its filter uses the same
        // available_fertilities calculation we're testing.
        use crate::building::{BuildingDef, BuildingInstance, OreDeposit};
        use crate::types::{Good, ProductionType};
        use anno_formats::szs::Fertility;

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 50_000;
        // Barren island — no fertility bytes set.
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.warehouses.push(Warehouse::new(0, 1, 15, 15));
        // Cocoa plantation — fertility-bound to Cocoa fertility.
        sim.building_defs.push(BuildingDef {
            id: 0,
            category: 0,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "PLANTAGE".into(),
            radius: 0,
            output_good: Good::Cocoa,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 1,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 50,
            cycle_time_ms: 1000,
            cost_gold: 200,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: true,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: Some(Fertility::Cocoa),
        });

        // Replay the handler logic: build the
        // available_fertilities set, filter defs, expect None.
        let owner = 1u8;
        let owner_warehouse_islands: std::collections::HashSet<u8> = sim
            .warehouses
            .iter()
            .filter(|w| w.active && w.owner == owner)
            .map(|w| w.island_id)
            .collect();
        let mut available_fertilities: std::collections::HashSet<Fertility> =
            std::collections::HashSet::new();
        for map in &sim.island_maps {
            if owner_warehouse_islands.contains(&map.island_id) {
                for f in map.active_fertilities() {
                    available_fertilities.insert(f);
                }
            }
        }
        assert!(
            available_fertilities.is_empty(),
            "barren island contributes no fertilities"
        );
        let pick = sim
            .building_defs
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.output_good == Good::Cocoa
                    && match d.required_fertility {
                        Some(req) => available_fertilities.contains(&req),
                        None => true,
                    }
            })
            .next();
        assert!(
            pick.is_none(),
            "fertility filter must reject the Cocoa def on a barren island"
        );

        // Make the island fertile and re-run.
        sim.island_maps[0].fertilities = [6, 7, 7, 7, 7, 7, 7, 7];
        let mut available_fertilities: std::collections::HashSet<Fertility> =
            std::collections::HashSet::new();
        for map in &sim.island_maps {
            if owner_warehouse_islands.contains(&map.island_id) {
                for f in map.active_fertilities() {
                    available_fertilities.insert(f);
                }
            }
        }
        assert!(available_fertilities.contains(&Fertility::Cocoa));
        let pick = sim
            .building_defs
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.output_good == Good::Cocoa
                    && match d.required_fertility {
                        Some(req) => available_fertilities.contains(&req),
                        None => true,
                    }
            })
            .next();
        assert!(
            pick.is_some(),
            "Cocoa-fertile island should accept the Cocoa def"
        );

        // Construct a mock built building so we can verify
        // BuildingInstance flows through the rest of the sim
        // without panicking even with the fertility wiring.
        let inst = BuildingInstance::new(0, 0, 5, 5, owner);
        sim.buildings.push(inst);
        assert_eq!(sim.buildings[0].owner, owner);
    }

    #[test]
    fn ai_does_not_synthesize_defenders_when_threatened() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        sim.warehouses.push(Warehouse::new(0, 1, 30, 30));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        // Hostile unit within 8 tiles of the AI's warehouse.
        sim.military_units
            .push(MilitaryUnit::new(UnitType::Infantry, 0, 32, 32));
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Military,
            Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        let ai_units = sim
            .military_units
            .iter()
            .filter(|u| u.owner == 1 && u.is_alive())
            .count();
        assert_eq!(ai_units, 0);
        assert_eq!(sim.players[1].gold, 5_000);
    }

    #[test]
    fn exploration_reveals_around_player_warehouse() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.warehouses.push(Warehouse::new(0, 0, 15, 15));
        sim.tick_exploration();
        let m = sim.exploration.iter().find(|e| e.island_id == 0).unwrap();
        // Tile right at the warehouse must be revealed.
        assert!(m.is_explored(15, 15));
        // Tile far away must not be (default radius 5).
        assert!(!m.is_explored(0, 0));
    }

    #[test]
    fn exploration_converts_source_land_world_coordinates_to_its_island() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut island = IslandMap::new_open(7, 30, 30);
        island.source_world_origin = (220, 260);
        sim.island_maps.push(island);
        let mut unit = MilitaryUnit::new(UnitType::Infantry, 0, 224, 264);
        unit.source_island_id = Some(7);
        sim.military_units.push(unit);

        sim.tick_exploration();

        let map = sim
            .exploration
            .iter()
            .find(|map| map.island_id == 7)
            .expect("source island exploration map");
        assert!(map.is_explored(2, 2));
        assert!(!map.is_explored(15, 15));
    }

    #[test]
    fn construction_stalls_without_materials() {
        use crate::building::{BuildingDef, BuildingInstance};
        use crate::types::{Good, ProductionType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.building_defs.push(BuildingDef {
            id: 0,
            category: 0,
            width: 1,
            height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 0,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        });
        let mut b = BuildingInstance::new(0, 0, 0, 0, 0);
        b.construction_ms_total = 1_000;
        b.construction_ms_remaining = 1_000;
        b.wood_needed = 5; // demands wood we don't have
        sim.buildings.push(b);

        // No warehouse has wood. Tick entities; construction must NOT advance.
        sim.tick_entities(500);
        assert_eq!(sim.buildings[0].construction_ms_remaining, 1_000);
        assert_eq!(sim.buildings[0].wood_needed, 5);
    }

    #[test]
    fn construction_trickles_materials_from_warehouse() {
        use crate::building::BuildingInstance;
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut wh = Warehouse::new(0, 0, 0, 0);
        wh.set_capacity(Good::Wood, 100);
        wh.deposit(Good::Wood, 10);
        sim.warehouses.push(wh);
        let mut b = BuildingInstance::new(0, 0, 5, 5, 0);
        b.construction_ms_total = 1_000;
        b.construction_ms_remaining = 1_000;
        b.wood_needed = 3;
        sim.buildings.push(b);

        // 1st tick: drain 1 wood, construction stays paused (need still 2).
        sim.tick_entities(50);
        assert_eq!(sim.buildings[0].wood_needed, 2);
        assert_eq!(sim.buildings[0].construction_ms_remaining, 1_000);
        // 2nd tick: 1 more drained.
        sim.tick_entities(50);
        assert_eq!(sim.buildings[0].wood_needed, 1);
        // 3rd: drained, materials done; this tick the timer also
        // decrements by dt_ms.
        sim.tick_entities(50);
        assert_eq!(sim.buildings[0].wood_needed, 0);
        assert!(sim.buildings[0].construction_ms_remaining < 1_000);
        assert_eq!(sim.warehouses[0].stock(Good::Wood), 7);
    }

    #[test]
    fn warehouse_pay_materials_consumes_when_sufficient() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut w = Warehouse::new(0, 0, 0, 0);
        w.set_capacity(Good::Wood, 100);
        w.set_capacity(Good::Tools, 100);
        w.set_capacity(Good::Bricks, 100);
        w.deposit(Good::Wood, 50);
        w.deposit(Good::Tools, 30);
        w.deposit(Good::Bricks, 20);
        sim.warehouses.push(w);
        assert!(sim.warehouse_pay_materials(0, 0, 10, 5, 8));
        let wh = &sim.warehouses[0];
        assert_eq!(wh.stock(Good::Wood), 40);
        assert_eq!(wh.stock(Good::Tools), 25);
        assert_eq!(wh.stock(Good::Bricks), 12);
    }

    #[test]
    fn warehouse_pay_materials_refuses_when_short() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut w = Warehouse::new(0, 0, 0, 0);
        w.set_capacity(Good::Wood, 100);
        w.deposit(Good::Wood, 5);
        sim.warehouses.push(w);
        // Need 10 wood, only have 5 → no withdrawal.
        assert!(!sim.warehouse_pay_materials(0, 0, 10, 0, 0));
        assert_eq!(sim.warehouses[0].stock(Good::Wood), 5);
    }

    #[test]
    fn pirates_do_not_spawn_without_source_hideout() {
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.trade_ships.push(TradeShip::new(0, 0, 100, 100));
        // Seed 2 satisfied the removed random pirate gate; without a source
        // trigger, the event must not fabricate an origin near the target ship.
        sim.seed_source_rand(2);
        sim.tick_events();
        assert!(sim.military_units.iter().all(|u| u.owner != 6));
        assert_eq!(sim.diplomacy.get(6, 0), crate::combat::Diplomacy::Neutral);
    }

    #[test]
    fn pirates_skip_spawn_when_no_player_trade_ships() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        for _ in 0..30 {
            sim.tick_events();
        }
        assert!(sim.military_units.iter().all(|u| u.owner != 6));
    }

    #[test]
    fn war_does_not_synthesize_building_destruction() {
        use crate::building::{BuildingDef, BuildingInstance};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        use crate::types::{Good, ProductionType};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.building_defs.push(BuildingDef {
            id: 0,
            category: 0,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 1000,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: 5,
            required_fertility: None,
        });
        let b = BuildingInstance::new(0, 0, 10, 10, 0); // player 0 owns
        sim.buildings.push(b);
        // Enemy unit standing right next to the footprint.
        sim.military_units
            .push(MilitaryUnit::new(UnitType::Infantry, 1, 11, 12));
        // The former fixed damage model destroyed the building here.
        for _ in 0..30 {
            sim.tick_military();
        }
        assert_eq!(sim.buildings.len(), 1);
        assert_eq!(
            sim.buildings[0].health,
            crate::building::BUILDING_MAX_HEALTH
        );
        assert!(sim.tile_clears.is_empty());
        assert!(sim.island_maps[0].is_walkable(10, 10));
    }

    #[test]
    fn buildings_safe_from_neutral_units() {
        use crate::building::{BuildingDef, BuildingInstance};
        use crate::combat::{MilitaryUnit, UnitType};
        use crate::types::{Good, ProductionType};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.building_defs.push(BuildingDef {
            id: 0,
            category: 0,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 1000,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        });
        sim.buildings.push(BuildingInstance::new(0, 0, 10, 10, 0));
        sim.military_units
            .push(MilitaryUnit::new(UnitType::Infantry, 1, 11, 12));
        // No diplomacy edit → relation stays Neutral, no damage.
        for _ in 0..30 {
            sim.tick_military();
        }
        assert_eq!(sim.buildings.len(), 1);
        assert_eq!(
            sim.buildings[0].health,
            crate::building::BUILDING_MAX_HEALTH
        );
    }

    #[test]
    fn military_tick_does_not_synthesize_legacy_combat() {
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        sim.military_units
            .push(MilitaryUnit::new(UnitType::Musketeer, 0, 5, 5));
        sim.military_units
            .push(MilitaryUnit::new(UnitType::Infantry, 1, 6, 5));
        let health_before: Vec<f32> = sim.military_units.iter().map(|unit| unit.health).collect();

        sim.tick_military();

        assert_eq!(
            sim.military_units
                .iter()
                .map(|unit| unit.health)
                .collect::<Vec<_>>(),
            health_before
        );
        assert!(sim.military_units.iter().all(|unit| unit.combat_target < 0));
    }

    #[test]
    fn entity_tick_does_not_synthesize_legacy_unit_movement() {
        use crate::combat::{MilitaryUnit, UnitType};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut unit = MilitaryUnit::new(UnitType::Infantry, 0, 5, 5);
        unit.target_x = 12;
        unit.target_y = 9;
        sim.military_units.push(unit);

        sim.tick_entities(1_000);

        assert_eq!(
            (sim.military_units[0].tile_x, sim.military_units[0].tile_y),
            (5, 5)
        );
        assert_eq!(
            (
                sim.military_units[0].target_x,
                sim.military_units[0].target_y
            ),
            (12, 9)
        );
    }

    #[test]
    fn ai_does_not_synthesize_escort_warship_when_at_war() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::Diplomacy;
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        sim.warehouses.push(Warehouse::new(0, 1, 10, 10));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        // AI has a trade ship.
        sim.trade_ships.push(TradeShip::new(1, 0, 50, 60));
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Military,
            Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        assert!(sim.military_units.is_empty());
        assert_eq!(sim.players[1].gold, 5_000);
    }

    #[test]
    fn escort_targets_track_their_ship() {
        use crate::combat::{MilitaryUnit, UnitType};
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.trade_ships.push(TradeShip::new(0, 0, 100, 200));
        let mut warship = MilitaryUnit::new(UnitType::SmallWarship, 0, 50, 50);
        warship.escort_ship = 0;
        sim.military_units.push(warship);
        sim.tick_entities(50);
        let u = &sim.military_units[0];
        assert_eq!(u.target_x, 100);
        assert_eq!(u.target_y, 200);
    }

    #[test]
    fn escort_clears_when_ship_inactive() {
        use crate::combat::{MilitaryUnit, UnitType};
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut s = TradeShip::new(0, 0, 5, 5);
        s.active = false;
        sim.trade_ships.push(s);
        let mut warship = MilitaryUnit::new(UnitType::SmallWarship, 0, 50, 50);
        warship.escort_ship = 0;
        sim.military_units.push(warship);
        sim.tick_entities(50);
        assert_eq!(sim.military_units[0].escort_ship, -1);
    }

    #[test]
    fn ai_score_does_not_dispatch_offensive_raid() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        // Enemy warehouse at (50,50), AI's at (10,10).
        sim.warehouses.push(Warehouse::new(0, 0, 50, 50));
        sim.warehouses.push(Warehouse::new(0, 1, 10, 10));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        // 4 idle AI units sitting near their warehouse.
        for k in 0..4 {
            let mut u = MilitaryUnit::new(UnitType::Infantry, 1, 10 + k, 10);
            u.target_x = u.tile_x;
            u.target_y = u.tile_y;
            sim.military_units.push(u);
        }
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Military,
            Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        // A war state plus score superiority must not synthesize the
        // original AI offensive order/cooldown state.
        let marching = sim
            .military_units
            .iter()
            .filter(|u| u.owner == 1 && u.target_x == 50 && u.target_y == 50)
            .count();
        assert_eq!(marching, 0, "score-only raid dispatched {marching} units");
    }

    #[test]
    fn ai_does_not_defend_against_neutral() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 1_000;
        sim.warehouses.push(Warehouse::new(0, 1, 30, 30));
        // Neutral player walking near the warehouse — not a threat.
        sim.military_units
            .push(MilitaryUnit::new(UnitType::Infantry, 0, 32, 32));
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Military,
            Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        let ai_units: usize = sim
            .military_units
            .iter()
            .filter(|u| u.owner == 1 && u.is_alive())
            .count();
        assert_eq!(ai_units, 0);
    }

    #[test]
    fn ai_does_not_synthesize_buildings_from_priority_list() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::building::BuildingDef;
        use crate::types::{Good, ProductionType};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 10_000;
        sim.players[1].population[1] = 150; // make AI tick fire
        sim.players[1].total_population = 150;
        sim.island_maps.push(IslandMap::new_open(0, 60, 60));
        sim.warehouses.push(Warehouse::new(0, 1, 30, 30));

        let mk_def = |cost: u32| BuildingDef {
            id: 0,
            category: 0,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Food,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 50,
            cycle_time_ms: 1000,
            cost_gold: cost,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        };
        // Two source-known building definitions were formerly selected by
        // local cost and variety heuristics.
        sim.building_defs.push(mk_def(500)); // def 0 (cheap)
        sim.building_defs.push(mk_def(800)); // def 1 (expensive)

        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Economic,
            Difficulty::Hard,
        ));
        let gold_before = sim.players[1].gold;
        sim.tick_ai();
        sim.tick_ai();

        assert!(sim.buildings.is_empty());
        assert_eq!(sim.players[1].gold, gold_before);
    }

    #[test]
    fn dynamic_map_object_slots_follow_live_hq_buildings() {
        use crate::building::{BuildingDef, BuildingInstance, OreDeposit};
        use crate::types::{Good, ProductionType};
        use anno_formats::cod::BuildingDef as CodBuilding;
        use anno_formats::szs::{Island, IslandTile};

        let mut sim = Simulation::new();
        sim.building_defs.push(BuildingDef {
            id: 0,
            category: 0,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "HQ".into(),
            prod_kind: "KONTOR".into(),
            radius: 0,
            output_good: Good::None,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 0,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        });
        sim.buildings.push(BuildingInstance::new(0, 4, 9, 7, 2));
        sim.buildings.push(BuildingInstance::new(0, 4, 5, 6, 3));
        sim.buildings.push(BuildingInstance::new(0, 4, 11, 12, 6));

        assert_eq!(
            sim.allocate_source_dynamic_map_object_for_building(0),
            Some(SourceDynamicMapObject {
                island: 4,
                slot: 0,
                owner: 2,
                local_position: (9, 7),
            })
        );
        assert_eq!(
            sim.allocate_source_dynamic_map_object_for_building(1),
            Some(SourceDynamicMapObject {
                island: 4,
                slot: 1,
                owner: 3,
                local_position: (5, 6),
            })
        );
        assert_eq!(sim.source_dynamic_map_object_table(4).objects().count(), 2);

        assert_eq!(
            sim.release_source_dynamic_map_object_for_building(0),
            Some(SourceDynamicMapObject {
                island: 4,
                slot: 0,
                owner: 2,
                local_position: (9, 7),
            })
        );
        assert_eq!(
            sim.allocate_source_dynamic_map_object_for_building(2),
            Some(SourceDynamicMapObject {
                island: 4,
                slot: 0,
                owner: 6,
                local_position: (11, 12),
            })
        );
        assert_eq!(
            sim.source_dynamic_map_object_table(4)
                .objects()
                .collect::<Vec<_>>(),
            vec![
                SourceDynamicMapObject {
                    island: 4,
                    slot: 0,
                    owner: 6,
                    local_position: (11, 12),
                },
                SourceDynamicMapObject {
                    island: 4,
                    slot: 1,
                    owner: 3,
                    local_position: (5, 6),
                },
            ]
        );

        let island = Island {
            number: 4,
            width: 32,
            height: 32,
            x_pos: 100,
            y_pos: 200,
            fertilities: [7; 8],
            tiles: vec![IslandTile {
                building_id: 3,
                x: 11,
                y: 12,
                orientation: 1,
                anim_count: 0,
                flags: 0,
            }],
            city: None,
        };
        let definitions = [CodBuilding {
            source_id: 0x4e23,
            size: (2, 4),
            ..Default::default()
        }];
        assert_eq!(
            sim.resolve_source_dynamic_map_object_target(
                SourceTargetDescriptor::from_bytes([0x35, 4, 0, 0]),
                &[island.clone()],
                &definitions,
            ),
            Some(SourceResolvedDynamicTarget {
                target: crate::source_route::SourcePathTargetRect::new((111, 212), 4, 2).unwrap(),
                owner: 6,
            })
        );
        assert_eq!(
            sim.resolve_source_dynamic_map_object_target(
                SourceTargetDescriptor::from_bytes([0x36, 4, 0, 0]),
                &[island],
                &[],
            ),
            Some(SourceResolvedDynamicTarget {
                target: crate::source_route::SourcePathTargetRect::new((111, 212), 1, 1).unwrap(),
                owner: 6,
            })
        );
    }

    #[test]
    fn source_kind_four_dispatch_routes_to_a_live_dynamic_object_descriptor() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(7, 8, 1));
        sim.source_dynamic_map_objects.push(SourceDynamicMapObject {
            island: 7,
            slot: 3,
            owner: 2,
            local_position: (1, 0),
        });
        let mut unit = crate::combat::MilitaryUnit::new(crate::combat::UnitType::Infantry, 0, 0, 0);
        unit.source_island_id = Some(7);
        unit.source_runtime_slot = Some(0);
        unit.source_figure_definition_id = Some(1);
        unit.source_target_descriptor = Some(SourceTargetDescriptor::from_bytes([0x36, 7, 3, 0]));
        sim.military_units.push(unit);

        sim.tick_source_land_figures(1);

        assert!(sim.military_units[0].source_motion_target.is_some());
        assert_eq!(
            sim.military_units[0].source_target_descriptor,
            Some(SourceTargetDescriptor::from_bytes([0x36, 7, 3, 0]))
        );
    }

    #[test]
    fn source_kind_four_targetless_payload_cycles_two_descriptors() {
        let mut sim = Simulation::new();
        let mut unit = MilitaryUnit::new(crate::combat::UnitType::Infantry, 0, 0, 0);
        unit.source_runtime_slot = Some(9);
        unit.source_figure_kind = Some(4);
        sim.military_units.push(unit);
        sim.source_kind4_occupants.push(SourceKind4Occupant {
            runtime_slot: 9,
            figure_definition_id: 1,
            route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
            route_retry_count: 3,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            position: (0, 0),
            island_id: 0,
            owner: 0,
            direction: 0,
            animation_state: 0,
            state_selector: 0,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            idle_timestamp_ticks: 0,
            state_flags: 1,
            state_payload: [0x38, 0, 4, 0, 0x38, 0, 8, 0],
            active: true,
        });

        sim.source_time_ticks = SOURCE_KIND4_IDLE_TARGET_DELAY_TICKS;
        sim.apply_source_kind4_state_payload_targets();
        assert_eq!(
            sim.military_units[0].source_target_descriptor,
            Some(SourceTargetDescriptor::from_bytes([0x38, 0, 8, 0]))
        );
        assert_eq!(sim.military_units[0].source_route_retry_count, 0);
        assert_eq!(sim.source_kind4_occupants[0].state_selector, 1);

        sim.military_units[0].source_target_descriptor = None;
        sim.source_kind4_occupants[0].state_selector = 1;
        sim.source_kind4_occupants[0].state_payload = [0x38, 0, 4, 0, 0, 0, 0, 0];
        sim.apply_source_kind4_state_payload_targets();
        assert_eq!(
            sim.military_units[0].source_target_descriptor,
            Some(SourceTargetDescriptor::from_bytes([0x38, 0, 4, 0]))
        );
        assert_eq!(sim.source_kind4_occupants[0].state_selector, 0);

        sim.military_units[0].source_target_descriptor = None;
        sim.source_kind4_occupants[0].state_selector = 1;
        sim.source_kind4_occupants[0].state_flags = 3;
        sim.apply_source_kind4_state_payload_targets();
        assert_eq!(sim.military_units[0].source_target_descriptor, None);
        assert_eq!(sim.source_kind4_occupants[0].state_flags, 2);
    }

    #[test]
    fn source_kind_four_live_candidates_emit_category_one_terminal_control() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(7, 20, 1));

        let mut attacker = MilitaryUnit::new(crate::combat::UnitType::Infantry, 0, 0, 0);
        attacker.source_island_id = Some(7);
        attacker.source_runtime_slot = Some(0);
        attacker.source_live_runtime_slot = Some(0);
        attacker.source_candidate_list_key = Some(7);
        attacker.source_figure_kind = Some(4);
        attacker.source_figure_definition_id = Some(1);
        attacker.source_energy = anno_formats::szs::LandFigureDefinition::from_id(1)
            .expect("source land definition is known")
            .source_runtime_energy_cap();
        sim.military_units.push(attacker);
        sim.source_kind4_occupants.push(SourceKind4Occupant {
            runtime_slot: 0,
            figure_definition_id: 1,
            route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
            route_retry_count: 0,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            position: (0, 0),
            island_id: 7,
            owner: 0,
            direction: 0,
            animation_state: 0,
            state_selector: 0,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            idle_timestamp_ticks: 0,
            state_flags: 0,
            state_payload: [0; 8],
            active: true,
        });

        let mut ship = MilitaryUnit::new(crate::combat::UnitType::SmallWarship, 1, 1, 0);
        ship.source_live_runtime_slot = Some(3);
        ship.source_candidate_list_key = Some(7);
        ship.source_figure_kind = Some(1);
        ship.source_figure_definition_id = Some(0x19);
        ship.source_energy = 195;
        sim.military_units.push(ship);

        let candidates = sim.source_combat_candidates();
        sim.dispatch_source_kind4_live_candidate_actions(&candidates);

        assert_eq!(sim.military_units[0].source_target_descriptor, None);
        let action = sim.source_kind4_actions[0];
        assert_eq!(action.attacker_position, (0.0, 0.0));
        assert_eq!(action.attacker_runtime_slot, 0);
        assert_eq!(action.attacker_figure_kind, 4);
        assert_eq!(action.direction, 2);
        assert_eq!(action.flags, crate::combat::SOURCE_KIND4_ACTION_EVENT_FLAGS);
        assert_eq!(
            action.target_descriptor,
            SourceTargetDescriptor::from_bytes([1, 7, 3, 0])
        );
        assert_eq!(sim.military_units[0].combat_target, -1);
        assert_eq!(sim.source_kind4_deferred_hits.len(), 1);
        assert_eq!(
            sim.source_kind4_occupants[0].idle_timestamp_ticks,
            crate::combat::source_kind4_action_ready_at(0, &candidates[0])
                .expect("category-four action readiness")
        );

        sim.military_units[1].source_energy = action.raw_strength;
        sim.source_time_ticks = sim.source_kind4_deferred_hits[0].due_at;
        sim.tick_source_kind4_deferred_hits();
        assert_eq!(sim.military_units[1].source_energy, action.raw_strength);
        assert!(sim.military_units[1].source_terminal_pending);
        assert!(sim.military_units[1].active);
        sim.tick_source_land_figures(20);
        assert!(!sim.military_units[1].active);
        assert_eq!(
            sim.source_combat_terminal_events,
            vec![SourceCombatTerminalEvent {
                target: action.target_descriptor,
                target_figure_kind: 1,
                target_runtime_slot: 3,
                target_owner: 1,
                attacker_figure_kind: 4,
                attacker_runtime_slot: 0,
                attacker_owner: Some(0),
                control_kind: SOURCE_COMBAT_TERMINAL_CONTROL_KIND,
                kill_credit: true,
            }]
        );
        assert!(sim.source_kind4_deferred_hits.is_empty());
    }

    #[test]
    fn source_kind_four_deferred_hit_removes_category_four_slot_after_terminal_control() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(7, 20, 1));

        let mut attacker = MilitaryUnit::new(crate::combat::UnitType::Infantry, 0, 0, 0);
        attacker.source_runtime_slot = Some(0);
        attacker.source_live_runtime_slot = Some(0);
        attacker.source_candidate_list_key = Some(7);
        attacker.source_figure_kind = Some(4);
        attacker.source_figure_definition_id = Some(1);
        attacker.source_energy = 60;
        sim.military_units.push(attacker);

        let mut target = MilitaryUnit::new(crate::combat::UnitType::Infantry, 1, 1, 0);
        target.source_runtime_slot = Some(1);
        target.source_live_runtime_slot = Some(1);
        target.source_candidate_list_key = Some(7);
        target.source_figure_kind = Some(4);
        target.source_figure_definition_id = Some(1);
        target.source_energy = 1;
        target.source_island_id = Some(7);
        target.source_motion_target = Some((2, 0));
        target.source_step_remaining = 0.75;
        target.direction = 2;
        sim.military_units.push(target);
        sim.source_kind4_occupants.push(SourceKind4Occupant {
            runtime_slot: 1,
            figure_definition_id: 1,
            route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
            route_retry_count: 0,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            position: (1, 0),
            island_id: 7,
            owner: 1,
            direction: 0,
            animation_state: 0,
            state_selector: 0,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            idle_timestamp_ticks: 0,
            state_flags: 0,
            state_payload: [0; 8],
            active: true,
        });
        let action = combat::SourceKind4Action {
            attacker_position: (0.0, 0.0),
            attacker_runtime_slot: 0,
            raw_strength: 1,
            attacker_figure_kind: 4,
            direction: 2,
            flags: combat::SOURCE_KIND4_ACTION_EVENT_FLAGS,
            target_descriptor: SourceTargetDescriptor::from_bytes([4, 7, 1, 0]),
        };
        sim.source_kind4_deferred_hits
            .push(combat::SourceKind4DeferredHit { due_at: 0, action });

        sim.tick_source_kind4_deferred_hits();

        assert_eq!(sim.military_units[1].source_energy, 1);
        assert!(sim.military_units[1].source_terminal_pending);
        assert!(sim.military_units[1].active);
        assert_eq!(sim.military_units[1].source_terminal_remaining, 0.25);
        sim.tick_source_land_figures(1);
        assert!(sim.military_units[1].active);
        assert!(sim.military_units[1].source_position_x > 0.75);
        sim.tick_source_land_figures(20_000);
        assert!(!sim.military_units[1].active);
        assert!(!sim.source_kind4_occupants[0].active);
        assert_eq!(
            sim.source_combat_terminal_events,
            vec![SourceCombatTerminalEvent {
                target: action.target_descriptor,
                target_figure_kind: 4,
                target_runtime_slot: 1,
                target_owner: 1,
                attacker_figure_kind: 4,
                attacker_runtime_slot: 0,
                attacker_owner: Some(0),
                control_kind: SOURCE_COMBAT_TERMINAL_CONTROL_KIND,
                kill_credit: true,
            }]
        );
    }

    #[test]
    fn source_kind_four_terminal_hit_requires_fun_00445930_owner_gate() {
        let mut sim = Simulation::new();
        sim.source_kind4_dispatch.active_player_slot = 0;
        sim.source_kind4_dispatch.single_player = false;

        let mut attacker = MilitaryUnit::new(crate::combat::UnitType::Infantry, 1, 0, 0);
        attacker.source_runtime_slot = Some(0);
        attacker.source_live_runtime_slot = Some(0);
        attacker.source_candidate_list_key = Some(7);
        attacker.source_figure_kind = Some(4);
        attacker.source_figure_definition_id = Some(1);
        attacker.source_energy = 60;
        sim.military_units.push(attacker);

        let mut target = MilitaryUnit::new(crate::combat::UnitType::SmallWarship, 2, 1, 0);
        target.source_runtime_slot = Some(1);
        target.source_live_runtime_slot = Some(1);
        target.source_candidate_list_key = Some(7);
        target.source_figure_kind = Some(1);
        target.source_figure_definition_id = Some(0x19);
        target.source_energy = 1;
        sim.military_units.push(target);

        sim.source_kind4_deferred_hits
            .push(combat::SourceKind4DeferredHit {
                due_at: 0,
                action: combat::SourceKind4Action {
                    attacker_position: (0.0, 0.0),
                    attacker_runtime_slot: 0,
                    raw_strength: 1,
                    attacker_figure_kind: 4,
                    direction: 2,
                    flags: combat::SOURCE_KIND4_ACTION_EVENT_FLAGS,
                    target_descriptor: SourceTargetDescriptor::from_bytes([1, 7, 1, 0]),
                },
            });

        sim.tick_source_kind4_deferred_hits();

        assert_eq!(sim.military_units[1].source_energy, 1);
        assert!(sim.military_units[1].active);
        assert!(sim.source_combat_terminal_events.is_empty());
    }

    #[test]
    fn source_kind_four_terminal_dynamic_slice_waits_for_generic_removal() {
        let mut sim = Simulation::new();
        let mut attacker = MilitaryUnit::new(crate::combat::UnitType::Infantry, 0, 0, 0);
        attacker.source_runtime_slot = Some(0);
        attacker.source_live_runtime_slot = Some(0);
        attacker.source_candidate_list_key = Some(7);
        attacker.source_figure_kind = Some(4);
        attacker.source_figure_definition_id = Some(1);
        attacker.source_energy = 60;
        sim.military_units.push(attacker);
        sim.source_dynamic_combat_figures
            .push(SourceDynamicCombatFigure {
                active: true,
                figure_kind: 1,
                candidate_list_key: 7,
                figure_definition_id: 0x19,
                direction: 0,
                source_payload: 0,
                position: (1.0, 0.0),
                position_z: 0.0,
                source_energy: 1,
                source_score_state: 0,
                source_action_ready_at: 0,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
                owner: 1,
                state: 0,
                flags: 0,
                notification: 0,
                runtime_slot: 3,
                auxiliary_kind: 0,
                name_index: 0,
                source_motion: combat::SourceGenericMotion {
                    remaining_distance: 1.5,
                    scalar_speed: 0.25,
                    velocity_x: 0.25,
                    velocity_y: 0.0,
                    velocity_z: 0.0,
                    terminal_motion_locked: false,
                },
            });
        sim.source_kind4_deferred_hits
            .push(combat::SourceKind4DeferredHit {
                due_at: 0,
                action: combat::SourceKind4Action {
                    attacker_position: (0.0, 0.0),
                    attacker_runtime_slot: 0,
                    raw_strength: 1,
                    attacker_figure_kind: 4,
                    direction: 2,
                    flags: combat::SOURCE_KIND4_ACTION_EVENT_FLAGS,
                    target_descriptor: SourceTargetDescriptor::from_bytes([1, 7, 3, 0]),
                },
            });

        sim.tick_source_kind4_deferred_hits();

        assert!(sim.source_dynamic_combat_figures[0].active);
        assert_eq!(
            sim.source_combat_terminal_slices,
            vec![SourceCombatTerminalSlice {
                target: SourceCombatTerminalSliceTarget::DynamicFigure(0),
                target_figure_kind: 1,
                target_runtime_slot: 3,
                remaining_distance: 0.25,
                scalar_speed: 0.25,
                velocity_x: 0.25,
                velocity_y: 0.0,
                velocity_z: 0.0,
            }]
        );
        assert_eq!(
            sim.source_combat_candidates()
                .iter()
                .map(|candidate| candidate.entity)
                .collect::<Vec<_>>(),
            vec![combat::SourceCombatCandidateEntity::MilitaryUnit(0)]
        );

        sim.tick_source_combat_terminal_slices(19);
        assert!(sim.source_dynamic_combat_figures[0].active);
        assert!((sim.source_dynamic_combat_figures[0].position.0 - 1.2375).abs() < f32::EPSILON);
        sim.tick_source_combat_terminal_slices(1);
        assert!(!sim.source_dynamic_combat_figures[0].active);
        assert!((sim.source_dynamic_combat_figures[0].position.0 - 1.25).abs() < f32::EPSILON);
        assert!(sim.source_combat_terminal_slices.is_empty());
    }

    #[test]
    fn source_kind_four_kind_one_terminal_boards_same_owner_ship_and_removes_figure() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(7, 8, 1));
        let descriptor = SourceTargetDescriptor::from_bytes([1, 7, 0x34, 0x12]);

        let mut ship = TradeShip::new(2, 0, 0, 0);
        ship.source_figure_kind = Some(1);
        ship.source_candidate_list_key = Some(7);
        ship.source_runtime_slot = Some(0x1234);
        ship.source_figure_definition_id = Some(0x15);
        ship.source_energy = 150;
        sim.trade_ships.push(ship);

        let mut unit = MilitaryUnit::new(crate::combat::UnitType::Infantry, 2, 0, 0);
        unit.source_island_id = Some(7);
        unit.source_runtime_slot = Some(9);
        unit.source_live_runtime_slot = Some(9);
        unit.source_candidate_list_key = Some(7);
        unit.source_figure_kind = Some(4);
        unit.source_figure_definition_id = Some(1);
        unit.source_energy = anno_formats::szs::LandFigureDefinition::from_id(1)
            .expect("source land definition is known")
            .source_runtime_energy_cap();
        unit.source_target_descriptor = Some(descriptor);
        sim.military_units.push(unit);

        sim.tick_source_land_figures(1);

        assert!(!sim.military_units[0].active);
        assert_eq!(
            crate::combat::source_ship_cargo_slot_ware(sim.trade_ships[0].source_cargo_slots[0]),
            0x19
        );
        assert_eq!(
            crate::combat::source_ship_cargo_slot_quantity(
                sim.trade_ships[0].source_cargo_slots[0]
            ),
            crate::combat::SOURCE_SHIP_CARGO_SLOT_QUANTITY_CAPACITY
        );
    }

    #[test]
    fn source_kind_four_kind_one_terminal_keeps_figure_when_ship_has_no_free_slot() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(7, 8, 1));
        let descriptor = SourceTargetDescriptor::from_bytes([1, 7, 0x34, 0x12]);

        let mut ship = TradeShip::new(2, 0, 0, 0);
        ship.source_figure_kind = Some(1);
        ship.source_candidate_list_key = Some(7);
        ship.source_runtime_slot = Some(0x1234);
        ship.source_figure_definition_id = Some(0x15);
        ship.source_energy = 150;
        ship.source_cargo_slots = [0x19; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT];
        sim.trade_ships.push(ship);

        let mut unit = MilitaryUnit::new(crate::combat::UnitType::Infantry, 2, 0, 0);
        unit.source_island_id = Some(7);
        unit.source_runtime_slot = Some(9);
        unit.source_live_runtime_slot = Some(9);
        unit.source_candidate_list_key = Some(7);
        unit.source_figure_kind = Some(4);
        unit.source_figure_definition_id = Some(1);
        unit.source_energy = 10;
        unit.source_target_descriptor = Some(descriptor);
        sim.military_units.push(unit);

        sim.tick_source_land_figures(1);

        assert!(sim.military_units[0].active);
        assert_eq!(sim.military_units[0].source_target_descriptor, None);
        assert_eq!(
            sim.trade_ships[0].source_cargo_slots,
            [0x19; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT]
        );
    }

    #[test]
    fn source_kind_four_klinik_terminal_defers_category_four_relocation() {
        let mut sim = Simulation::new();
        sim.island_maps.push(IslandMap::new_open(7, 8, 2));
        sim.source_static_map_roots.push(SourceMapCellState {
            island: 7,
            x: 1,
            y: 0,
            source_production_kind_code: 0x16,
            ..Default::default()
        });
        let descriptor = SourceTargetDescriptor::from_bytes([0x32, 7, 1, 0]);
        let mut unit = MilitaryUnit::new(crate::combat::UnitType::Infantry, 2, 2, 0);
        unit.source_island_id = Some(7);
        unit.source_runtime_slot = Some(9);
        unit.source_live_runtime_slot = Some(9);
        unit.source_candidate_list_key = Some(7);
        unit.source_figure_kind = Some(4);
        unit.source_figure_definition_id = Some(1);
        unit.source_target_descriptor = Some(descriptor);
        unit.source_route_program[..3] = [
            0x71,
            0x81,
            crate::combat::SOURCE_KIND4_ROUTE_PROGRAM_TERMINATOR,
        ];
        unit.source_route_program_cursor = 2;
        sim.military_units.push(unit);

        sim.tick_source_land_figures(1);

        assert!(!sim.military_units[0].active);
        assert_eq!(sim.source_kind4_deferred_relocations.len(), 1);
        assert_eq!(
            sim.source_kind4_deferred_relocations[0],
            SourceKind4DeferredRelocation {
                due_at: 100,
                island_id: 7,
                figure_definition_id: 1,
                origin: (1.25, 0.25),
                target_descriptor: SourceTargetDescriptor::from_bytes([0x38, 0, 4, 1]),
            }
        );

        sim.source_time_ticks = 99;
        sim.tick_source_kind4_deferred_relocations();
        assert_eq!(sim.military_units.len(), 1);
        sim.source_time_ticks = 100;
        sim.tick_source_kind4_deferred_relocations();

        assert_eq!(sim.military_units.len(), 2);
        let figure = &sim.military_units[1];
        assert_eq!(figure.source_figure_kind, Some(4));
        assert_eq!(figure.source_runtime_slot, Some(0));
        assert_eq!(figure.source_figure_definition_id, Some(1));
        assert_eq!(
            (figure.source_position_x, figure.source_position_y),
            (1.25, 0.25)
        );
        assert_eq!(
            figure.source_origin_descriptor,
            Some(SourceTargetDescriptor::from_bytes([0x38, 0, 2, 0]))
        );
        assert_eq!(
            figure.source_target_descriptor,
            Some(SourceTargetDescriptor::from_bytes([0x38, 0, 4, 1]))
        );
        assert_eq!(figure.owner, 0);
        assert_eq!(sim.source_kind4_occupants.len(), 1);
    }

    #[test]
    fn source_dynamic_table_retains_scenario_hq_slots() {
        let mut sim = Simulation::new();
        sim.source_dynamic_map_objects.push(SourceDynamicMapObject {
            island: 4,
            slot: 3,
            owner: 6,
            local_position: (9, 7),
        });

        assert_eq!(
            sim.source_dynamic_map_object_table(4)
                .objects()
                .collect::<Vec<_>>(),
            vec![SourceDynamicMapObject {
                island: 4,
                slot: 3,
                owner: 6,
                local_position: (9, 7),
            }]
        );
        assert!(sim
            .source_dynamic_map_object_table(5)
            .objects()
            .next()
            .is_none());
    }

    #[test]
    fn ai_does_not_generate_trade_route_without_source_route_state() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 5_000;
        // These would previously synthesize an all-goods route and ship.
        sim.warehouses.push(Warehouse::new(0, 1, 10, 10));
        sim.warehouses.push(Warehouse::new(1, 1, 50, 50));
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Economic,
            Difficulty::Hard,
        ));
        // Drive AI tick.
        sim.tick_ai();
        assert_eq!(sim.players[1].gold, 5_000);
        assert!(sim.trade_routes.is_empty());
        assert!(sim.trade_ships.is_empty());
    }

    #[test]
    fn residence_promotes_when_tier_fully_satisfied() {
        use crate::building::{BuildingDef, BuildingInstance};
        use crate::types::{Good, ProductionType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        // Make tier 0 (Pioneer) fully satisfied so the WOHN gets
        // promoted to Settler tier on the next tick_population.
        sim.players[0].satisfaction[0] = 128;
        sim.building_defs.push(BuildingDef {
            id: 0,
            category: 0,
            width: 1,
            height: 1,
            production_type: ProductionType::Residence,
            kind: "WOHN".into(),
            prod_kind: "WOHN".into(),
            radius: 0,
            output_good: Good::None,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 0,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: true,    // residence has a door
            upgradeable: true, // and is upgradeable per Ausbauflg
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        });
        sim.buildings.push(BuildingInstance::new(0, 0, 0, 0, 0));
        assert_eq!(sim.buildings[0].house_tier, 0);
        sim.tick_population();
        assert_eq!(sim.buildings[0].house_tier, 1);
    }

    #[test]
    fn building_maintenance_aggregates_per_player() {
        use crate::building::{BuildingDef, BuildingInstance};
        use crate::types::{Good, ProductionType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        let mk_def = |maint: u16| BuildingDef {
            id: 0,
            category: 0,
            width: 1,
            height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 50,
            cycle_time_ms: 1000,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: maint,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        };
        sim.building_defs.push(mk_def(5)); // def 0 cost 5
        sim.building_defs.push(mk_def(8)); // def 1 cost 8
                                           // Player 0: 2× def0 + 1× def1 → 5+5+8 = 18
        sim.buildings.push(BuildingInstance::new(0, 0, 0, 0, 0));
        sim.buildings.push(BuildingInstance::new(0, 0, 1, 1, 0));
        sim.buildings.push(BuildingInstance::new(1, 0, 2, 2, 0));
        // Player 1: 1× def1 → 8
        sim.buildings.push(BuildingInstance::new(1, 0, 3, 3, 1));
        // Make all "built" so they count.
        for b in &mut sim.buildings {
            b.construction_ms_remaining = 0;
        }
        sim.tick_population();
        assert_eq!(sim.players[0].building_maintenance, 18);
        assert_eq!(sim.players[1].building_maintenance, 8);
    }

    #[test]
    fn unfinished_buildings_do_not_pay_maintenance() {
        use crate::building::{BuildingDef, BuildingInstance};
        use crate::types::{Good, ProductionType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.building_defs.push(BuildingDef {
            id: 0,
            category: 0,
            width: 1,
            height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 1000,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 7,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        });
        // One under construction, one finished.
        let mut bb = BuildingInstance::new(0, 0, 0, 0, 0);
        bb.construction_ms_total = 5_000;
        bb.construction_ms_remaining = 5_000;
        sim.buildings.push(bb);
        sim.buildings.push(BuildingInstance::new(0, 0, 1, 1, 0));
        sim.tick_population();
        // Only the finished building should be counted.
        assert_eq!(sim.players[0].building_maintenance, 7);
    }

    #[test]
    fn ai_score_does_not_create_war() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        // Drain the human player's default starting gold so AI(1) would have
        // dominated the removed score helper.
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        // AI(1) is Military and beefy; score alone must not synthesize war.
        for _ in 0..10 {
            sim.military_units
                .push(MilitaryUnit::new(UnitType::Infantry, 1, 0, 0));
        }
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Military,
            Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        assert_eq!(sim.diplomacy.get(1, 0), Diplomacy::Neutral);
        assert_eq!(sim.diplomacy.get(0, 1), Diplomacy::Neutral);
    }

    #[test]
    fn ai_score_does_not_end_war() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        // Start at war.
        sim.diplomacy.set(1, 0, Diplomacy::War);
        // Player 0 vastly outmuscles AI(1).
        for _ in 0..30 {
            sim.military_units
                .push(MilitaryUnit::new(UnitType::Infantry, 0, 0, 0));
        }
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Military,
            Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        assert_eq!(sim.diplomacy.get(1, 0), Diplomacy::War);
        assert_eq!(sim.diplomacy.get(0, 1), Diplomacy::War);
    }

    #[test]
    fn economic_ai_does_not_declare_war() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        for _ in 0..10 {
            sim.military_units
                .push(MilitaryUnit::new(UnitType::Infantry, 1, 0, 0));
        }
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Economic,
            Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        // Economic AI must not flip to war even with the upper hand.
        assert_eq!(sim.diplomacy.get(1, 0), Diplomacy::Neutral);
    }

    #[test]
    fn ai_does_not_synthesize_military_units() {
        let mut sim = Simulation::new();
        // Player slot 1 is the AI we're driving.
        sim.players.push(Player::new_human(0)); // slot 0 (unused for this test)
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 5_000;
        sim.players[1].total_population = 200;
        sim.players[1].population[1] = 200;
        sim.warehouses.push(Warehouse::new(0, 1, 30, 40));
        // A high-population, high-gold military AI used to manufacture units
        // beside this warehouse from local difficulty rules.
        sim.ai_controllers.push(AiController::new(
            1,
            AiPersonality::Military,
            Difficulty::Hard,
        ));
        sim.tick_ai();
        assert!(sim.military_units.is_empty());
        assert_eq!(sim.players[1].gold, 5_000);
    }

    #[test]
    fn native_barter_round_trip() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.warehouses[0].set_capacity(Good::Cloth, 100);
        sim.warehouses[0].deposit(Good::Cloth, 50);
        sim.warehouses[0].set_capacity(Good::Spices, 100);
        sim.native_villages
            .push(crate::native::NativeVillage::new(0, 60, 60));

        // Deliver Cloth → village credit accumulates.
        let ok = sim.apply_command(&crate::commands::Command::NativeDeliver {
            player: 0,
            village_idx: 0,
            good: Good::Cloth,
            qty: 30,
        });
        assert!(ok);
        assert_eq!(sim.warehouses[0].stock(Good::Cloth), 20);
        assert!(sim.native_villages[0].credit[0] > 0);

        // Withdraw a small amount of Spices — credit should cover it.
        let ok = sim.apply_command(&crate::commands::Command::NativeWithdraw {
            player: 0,
            village_idx: 0,
            good: Good::Spices,
            qty: 1,
        });
        assert!(ok);
        assert_eq!(sim.warehouses[0].stock(Good::Spices), 1);
    }

    #[test]
    fn native_deliver_rejects_unwanted_good() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.warehouses[0].set_capacity(Good::Wood, 50);
        sim.warehouses[0].deposit(Good::Wood, 50);
        sim.native_villages
            .push(crate::native::NativeVillage::new(0, 60, 60));
        // Wood is not in the default wants list (Cloth/Tools/Jewelry).
        let ok = sim.apply_command(&crate::commands::Command::NativeDeliver {
            player: 0,
            village_idx: 0,
            good: Good::Wood,
            qty: 10,
        });
        assert!(!ok);
        // Refund: warehouse stock unchanged.
        assert_eq!(sim.warehouses[0].stock(Good::Wood), 50);
    }

    #[test]
    fn patrol_cycles_waypoints() {
        use crate::combat::{tick_unit_orders, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.military_units
            .push(MilitaryUnit::new(UnitType::SmallWarship, 0, 0, 0));
        let ok = sim.apply_command(&crate::commands::Command::SetPatrol {
            player: 0,
            unit_index: 0,
            waypoints: vec![(2, 0), (2, 2), (0, 2)],
        });
        assert!(ok);
        // Tick the unit until it reaches the first waypoint.
        for _ in 0..20 {
            tick_unit_orders(&mut sim.military_units, 200);
            if sim.military_units[0].tile_x == 2 && sim.military_units[0].tile_y == 0 {
                break;
            }
        }
        // After arrival the patrol index should advance to waypoint 1
        // and the new target_x/y should reflect (2, 2).
        tick_unit_orders(&mut sim.military_units, 200);
        assert_eq!(
            (
                sim.military_units[0].target_x,
                sim.military_units[0].target_y
            ),
            (2, 2),
        );

        // Empty waypoints cancels patrol.
        sim.apply_command(&crate::commands::Command::SetPatrol {
            player: 0,
            unit_index: 0,
            waypoints: vec![],
        });
        assert!(sim.military_units[0].patrol.is_empty());
    }

    #[test]
    fn ship_cargo_load_unload_round_trip() {
        use crate::trade::TradeShip;
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.warehouses[0].set_capacity(Good::Tools, 100);
        sim.warehouses[0].deposit(Good::Tools, 50);
        // Ship docked next to warehouse.
        let mut ship = TradeShip::new(0, 0, 30, 40);
        ship.active = true;
        sim.trade_ships.push(ship);

        // Load 20 Tools.
        let ok = sim.apply_command(&crate::commands::Command::LoadShip {
            player: 0,
            ship_idx: 0,
            warehouse_idx: 0,
            good: Good::Tools,
            qty: 20,
        });
        assert!(ok);
        assert_eq!(sim.warehouses[0].stock(Good::Tools), 30);
        assert_eq!(sim.trade_ships[0].cargo_amount(Good::Tools), 20);

        // Unload 5 back.
        let ok = sim.apply_command(&crate::commands::Command::UnloadShip {
            player: 0,
            ship_idx: 0,
            warehouse_idx: 0,
            good: Good::Tools,
            qty: 5,
        });
        assert!(ok);
        assert_eq!(sim.warehouses[0].stock(Good::Tools), 35);
        assert_eq!(sim.trade_ships[0].cargo_amount(Good::Tools), 15);
    }

    #[test]
    fn ship_load_rejects_distant_ship() {
        use crate::trade::TradeShip;
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.warehouses[0].set_capacity(Good::Tools, 50);
        sim.warehouses[0].deposit(Good::Tools, 50);
        let mut ship = TradeShip::new(0, 0, 100, 100);
        ship.active = true;
        sim.trade_ships.push(ship);
        let ok = sim.apply_command(&crate::commands::Command::LoadShip {
            player: 0,
            ship_idx: 0,
            warehouse_idx: 0,
            good: Good::Tools,
            qty: 5,
        });
        assert!(!ok);
    }

    #[test]
    fn sell_ship_refunds_half_cost_and_deactivates() {
        use crate::combat::{unit_build_cost, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players[0].gold = 100;
        sim.military_units
            .push(MilitaryUnit::new(UnitType::SmallWarship, 0, 5, 5));
        let cost = unit_build_cost(UnitType::SmallWarship);
        let ok = sim.apply_command(&crate::commands::Command::SellShip {
            player: 0,
            unit_index: 0,
        });
        assert!(ok);
        assert_eq!(sim.players[0].gold, 100 + cost / 2);
        assert!(!sim.military_units[0].active);
    }

    #[test]
    fn sell_ship_rejects_land_unit() {
        use crate::combat::{MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.military_units
            .push(MilitaryUnit::new(UnitType::Infantry, 0, 0, 0));
        let ok = sim.apply_command(&crate::commands::Command::SellShip {
            player: 0,
            unit_index: 0,
        });
        assert!(!ok);
    }

    #[test]
    fn arm_ship_consumes_cannons_and_clamps_to_cap() {
        use crate::combat::{MilitaryUnit, UnitType};
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players[0].gold = 5_000;
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.warehouses[0].set_capacity(Good::Cannons, 50);
        sim.warehouses[0].deposit(Good::Cannons, 50);
        sim.military_units
            .push(MilitaryUnit::new(UnitType::SmallWarship, 0, 60, 60));
        let ok = sim.apply_command(&crate::commands::Command::ArmShip {
            player: 0,
            unit_index: 0,
            target_cannons: 99, // request beyond cap
        });
        assert!(ok);
        let cap = crate::combat::cannon_capacity(UnitType::SmallWarship);
        assert_eq!(sim.military_units[0].cannons, cap);
        assert_eq!(sim.warehouses[0].stock(Good::Cannons), 50 - cap as u16);
        assert_eq!(sim.players[0].gold, 5_000 - 200 * cap as i32);
    }

    #[test]
    fn arm_ship_rejects_land_unit() {
        use crate::combat::{MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.military_units
            .push(MilitaryUnit::new(UnitType::Infantry, 0, 0, 0));
        assert!(!sim.apply_command(&crate::commands::Command::ArmShip {
            player: 0,
            unit_index: 0,
            target_cannons: 1,
        }));
    }

    #[test]
    fn trade_agreement_lifecycle() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));

        // Propose & accept.
        assert!(sim.apply_command(&crate::commands::Command::ProposeTradeAgreement { a: 0, b: 1 }));
        assert!(sim.diplomacy.has_trade_agreement(0, 1));
        assert!(sim.diplomacy.has_trade_agreement(1, 0));

        // Break it: penalty flag set, can't re-propose immediately.
        assert!(sim.apply_command(&crate::commands::Command::BreakTradeAgreement { a: 0, b: 1 }));
        assert!(!sim.diplomacy.has_trade_agreement(0, 1));
        assert!(!sim.apply_command(&crate::commands::Command::ProposeTradeAgreement { a: 0, b: 1 }));

        // Clear penalty (e.g. after cooldown), proposal allowed.
        sim.diplomacy.clear_broken_flag(0, 1);
        assert!(sim.apply_command(&crate::commands::Command::ProposeTradeAgreement { a: 0, b: 1 }));
    }

    #[test]
    fn declaring_war_clears_trade_agreement() {
        use crate::combat::Diplomacy;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.diplomacy.propose_trade_agreement(0, 1);
        assert!(sim.diplomacy.has_trade_agreement(0, 1));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        assert!(!sim.diplomacy.has_trade_agreement(0, 1));
    }

    #[test]
    fn diplomacy_score_does_not_accept_non_war_proposals() {
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        use crate::commands::Command;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 50_000;
        sim.players[0].total_population = 5_000;
        for _ in 0..20 {
            sim.military_units
                .push(MilitaryUnit::new(UnitType::Infantry, 0, 0, 0));
        }

        assert!(!sim.apply_command(&Command::SetDiplomacy {
            a: 0,
            b: 1,
            state: Diplomacy::Allied,
        }));
        assert_eq!(sim.diplomacy.get(0, 1), Diplomacy::Neutral);

        sim.diplomacy.set(0, 1, Diplomacy::War);
        assert!(!sim.apply_command(&Command::SetDiplomacy {
            a: 0,
            b: 1,
            state: Diplomacy::Neutral,
        }));
        assert_eq!(sim.diplomacy.get(0, 1), Diplomacy::War);
        assert_eq!(sim.diplomacy.get(1, 0), Diplomacy::War);
        assert_eq!(sim.event_log.len(), 2);
    }

    #[test]
    fn source_relationship_event_command_updates_the_directed_byte_and_target_queue() {
        use crate::commands::Command;

        let mut sim = Simulation::new();
        sim.source_time_ticks = 73;
        sim.diplomacy.set_source_relationship_code(0, 1, 3);
        assert!(sim.apply_command(&Command::ApplySourceRelationshipEvent {
            source: 0,
            target: 1,
            payload: 1,
        }));
        assert_eq!(sim.diplomacy.source_relationship_code(0, 1), 2);
        assert_eq!(sim.diplomacy.source_relationship_code(1, 0), 0);
        assert_eq!(
            sim.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].event_type,
            4
        );
        assert_eq!(
            sim.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].peer,
            0
        );
        assert_eq!(
            sim.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].timestamp,
            73
        );
        assert!(sim.apply_command(&Command::ApplySourceRelationshipEvent {
            source: 0,
            target: 1,
            payload: 1,
        }));

        sim.diplomacy.set_source_relationship_code(0, 1, 3);
        sim.diplomacy.set_source_attitude_code(0, 1, 2);
        assert!(sim.apply_command(&Command::ApplySourceRelationshipEvent {
            source: 0,
            target: 1,
            payload: 0,
        }));
        let queue = sim.diplomacy.source_diplomacy_event_queue(1).unwrap();
        assert_eq!(queue[0].event_type, 1);
        assert_eq!(queue[0].peer, 0);
        assert_eq!(queue[1].event_type, 3);
        assert_eq!(queue[1].peer, 0);
    }

    #[test]
    fn source_attitude_event_command_keeps_the_source_pair_asymmetric() {
        use crate::commands::Command;

        let mut sim = Simulation::new();
        sim.source_time_ticks = 91;
        assert!(sim.apply_command(&Command::ApplySourceAttitudeEvent {
            source: 0,
            target: 1,
            payload: 1,
        }));
        assert_eq!(sim.diplomacy.source_attitude_code(0, 1), 1);
        assert_eq!(sim.diplomacy.source_attitude_code(1, 0), 2);
        assert_eq!(
            sim.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].event_type,
            2
        );
        assert_eq!(
            sim.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].peer,
            0
        );
        assert_eq!(
            sim.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].timestamp,
            91
        );
        assert!(!sim.apply_command(&Command::ApplySourceAttitudeEvent {
            source: 0,
            target: 1,
            payload: 1,
        }));
    }

    #[test]
    fn cart_transfers_clamped_to_six() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.warehouses.push(Warehouse::new(0, 0, 60, 60));
        sim.warehouses[0].deposit(Good::Tools, 25);
        let ok = sim.apply_command(&crate::commands::Command::DispatchCart {
            player: 0,
            from_warehouse: 0,
            to_warehouse: 1,
            good: Good::Tools,
            qty: 100,
        });
        assert!(ok);
        // Capped at Maxtrag = 6.
        assert_eq!(sim.warehouses[0].stock(Good::Tools), 19);
        assert_eq!(sim.warehouses[1].stock(Good::Tools), 6);
    }

    #[test]
    fn cart_rejects_cross_owner() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.warehouses.push(Warehouse::new(1, 1, 60, 60));
        sim.warehouses[0].deposit(Good::Tools, 5);
        let ok = sim.apply_command(&crate::commands::Command::DispatchCart {
            player: 0,
            from_warehouse: 0,
            to_warehouse: 1,
            good: Good::Tools,
            qty: 5,
        });
        assert!(!ok);
        assert_eq!(sim.warehouses[0].stock(Good::Tools), 5);
    }

    #[test]
    fn pirate_event_does_not_spawn_from_hideout_without_source_trigger() {
        use crate::building::BuildingInstance;
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        // Define a single PIRATWOHN building def at index 0.
        sim.building_defs.push(crate::building::BuildingDef {
            id: 0,
            category: 0,
            width: 2,
            height: 2,
            production_type: crate::types::ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "PIRATWOHN".into(),
            radius: 4,
            output_good: crate::types::Good::None,
            input_good_1: crate::types::Good::None,
            input_good_2: crate::types::Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 0,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        });
        // Place a hideout at a known tile.
        let mut h = BuildingInstance::new(0, 0, 7, 11, 6);
        h.construction_ms_remaining = 0;
        sim.buildings.push(h);
        // Need a player trade ship for pirates to want to spawn.
        sim.trade_ships.push(TradeShip::new(0, 0, 50, 50));

        // Seed 2 satisfied the removed 1-in-3 random gate. A source hideout
        // and target ship still must not fabricate a new pirate without the
        // decoded source event trigger.
        let mut expected_rng = Simulation::new();
        expected_rng.seed_source_rand(2);
        let expected_first_draw = expected_rng.next_source_rand();
        sim.seed_source_rand(2);
        sim.tick_pirate_event();
        assert!(sim.military_units.iter().all(|u| u.owner != 6));
        assert_eq!(sim.diplomacy.get(6, 0), crate::combat::Diplomacy::Neutral);
        assert_eq!(sim.next_source_rand(), expected_first_draw);
    }

    #[test]
    fn defeat_when_human_runs_out_of_kontors_and_population() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        // No warehouses, no population → instant collapse.
        sim.evaluate_outcomes();
        assert_eq!(sim.players[0].state, crate::player::PlayerState::Defeated);
        assert_eq!(sim.outcome, GameOutcome::Defeat);
    }

    #[test]
    fn victory_when_all_rivals_defeated() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        // Give the human a Kontor + population so they aren't defeated.
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.players[0].population[1] = 100;

        sim.players.push(Player::new_ai(1, 0));
        sim.players.push(Player::new_ai(2, 0));
        // Knock both rivals out.
        sim.players[1].state = crate::player::PlayerState::Defeated;
        sim.players[2].state = crate::player::PlayerState::Defeated;

        sim.evaluate_outcomes();
        assert_eq!(sim.outcome, GameOutcome::Victory);
    }

    #[test]
    fn gift_gold_transfers_and_clamps_to_balance() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 5_000;
        sim.players[1].gold = 100;
        let ok = sim.apply_command(&crate::commands::Command::GiftGold {
            from: 0,
            to: 1,
            amount: 1_500,
        });
        assert!(ok);
        assert_eq!(sim.players[0].gold, 3_500);
        assert_eq!(sim.players[1].gold, 1_600);
        // Clamp to balance.
        let _ = sim.apply_command(&crate::commands::Command::GiftGold {
            from: 0,
            to: 1,
            amount: 999_999,
        });
        assert_eq!(sim.players[0].gold, 0);
        assert_eq!(sim.players[1].gold, 5_100);
    }

    #[test]
    fn gift_goods_moves_between_warehouses() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.warehouses.push(Warehouse::new(1, 1, 60, 60));
        sim.warehouses[0].deposit(Good::Tools, 25);
        let ok = sim.apply_command(&crate::commands::Command::GiftGoods {
            from: 0,
            to: 1,
            good: Good::Tools,
            qty: 20,
        });
        assert!(ok);
        assert_eq!(sim.warehouses[0].stock(Good::Tools), 5);
        assert_eq!(sim.warehouses[1].stock(Good::Tools), 20);
    }

    #[test]
    fn gift_self_or_zero_rejected() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players[0].gold = 5_000;
        assert!(!sim.apply_command(&crate::commands::Command::GiftGold {
            from: 0,
            to: 0,
            amount: 100,
        }));
        assert!(!sim.apply_command(&crate::commands::Command::GiftGoods {
            from: 0,
            to: 0,
            good: Good::Tools,
            qty: 1,
        }));
        assert!(!sim.apply_command(&crate::commands::Command::GiftGold {
            from: 0,
            to: 1,
            amount: 0,
        }));
    }

    #[test]
    fn current_price_uses_fixed_price_table() {
        use crate::prices::price_of;
        use crate::types::Good;

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut wh = Warehouse::new(0, 0, 10, 10);
        wh.deposit(Good::Tools, 1_000);
        sim.warehouses.push(wh);

        assert_eq!(
            sim.current_price(Good::Tools).buy,
            price_of(Good::Tools).buy
        );
        assert_eq!(
            sim.current_price(Good::Tools).sell,
            price_of(Good::Tools).sell
        );
        sim.warehouses[0].withdraw(Good::Tools, 1_000);
        assert_eq!(
            sim.current_price(Good::Tools).buy,
            price_of(Good::Tools).buy
        );
        assert_eq!(
            sim.current_price(Good::Tools).sell,
            price_of(Good::Tools).sell
        );
    }

    #[test]
    fn free_trader_retarget_uses_binary_seek_gate_after_docking() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.warehouses.push(Warehouse::new(0, 0, 10, 10));
        sim.warehouses.push(Warehouse::new(1, 1, 10, 10));

        let mut trader = crate::free_trader::FreeTrader::spawn_at(10, 10);
        trader.target_warehouse = Some(0);
        trader.target_x = 10;
        trader.target_y = 10;
        trader.state = crate::free_trader::FreeTraderState::Docked;
        trader.dock_ticks_left = 1;
        sim.free_traders.push(trader);

        // Seed 1 produces first MSVC rand() value 41, so
        // `(rand() & 3) == 0` target gate fails.
        sim.seed_source_rand(1);
        sim.tick_free_traders();
        assert!(sim.free_traders[0].active);
        assert!(!sim.free_traders[0].leaving);
        assert_eq!(
            sim.free_traders[0].state,
            crate::free_trader::FreeTraderState::Sailing
        );
        assert_eq!(sim.free_traders[0].target_warehouse, None);

        // Seed 3 produces first MSVC rand() value 48, so the next
        // seek tick may pick a new port. This also proves there is no
        // lifetime visit cap.
        sim.seed_source_rand(3);
        sim.tick_free_traders();
        assert!(sim.free_traders[0].target_warehouse.is_some());
    }

    #[test]
    fn free_trader_port_distance_uses_vertical_quarter_metric() {
        let trader = crate::free_trader::FreeTrader::spawn_at(10, 20);
        let warehouse = Warehouse::new(0, 0, 14, 32);

        assert_eq!(free_trader_port_distance(&trader, &warehouse), 7);
    }

    #[test]
    fn free_trader_edge_point_uses_ocean_map_bounds() {
        let szs = anno_formats::szs::SzsFile {
            chunks: Vec::new(),
            islands: vec![anno_formats::szs::Island {
                number: 0,
                width: 100,
                height: 100,
                x_pos: 180,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: Vec::new(),
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
        };
        let ocean = OceanMap::from_scenario(&szs);

        let (x, y) = free_trader_edge_point_from_ocean(&ocean, 1, 9_192);

        assert_eq!(ocean.width, 290);
        assert_eq!(ocean.height, 110);
        assert_eq!(x, 9_192 % i32::from(ocean.width));
        assert_eq!(y, i32::from(ocean.height) - 1);
        assert!(ocean.is_navigable(x, y));
    }

    #[test]
    fn free_trader_spawns_immediately_when_manual_count_requires_ship() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 20, 20));
        sim.warehouses.push(Warehouse::new(1, 0, 25, 20));

        // Seed 1's first MSVC rand() is 41, which failed the removed
        // implementation-only `(rand() & 3) == 0` spawn gate.
        sim.seed_source_rand(1);
        sim.tick_free_traders();

        assert_eq!(sim.free_traders.len(), 1);
        assert!(sim.free_traders[0].active);
        assert_eq!(
            sim.event_log.last().map(String::as_str),
            Some("[trader] free trader sighted at the horizon")
        );
    }

    #[test]
    fn free_trader_spawn_uses_loaded_ocean_edge() {
        let szs = anno_formats::szs::SzsFile {
            chunks: Vec::new(),
            islands: vec![anno_formats::szs::Island {
                number: 0,
                width: 100,
                height: 100,
                x_pos: 180,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: Vec::new(),
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
        };
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 20, 20));
        sim.warehouses.push(Warehouse::new(1, 0, 25, 20));
        sim.ocean_map = Some(OceanMap::from_scenario(&szs));

        // With no spawn-admission rand draw, seed 14 gives side rand
        // 84 (top edge) and offset rand 27125.
        sim.seed_source_rand(14);
        sim.tick_free_traders();

        assert_eq!(sim.free_traders.len(), 1);
        let ocean = sim.ocean_map.as_ref().unwrap();
        let trader = &sim.free_traders[0];
        assert!(trader.world_x >= 0 && trader.world_x < i32::from(ocean.width));
        assert!(trader.world_y >= 0 && trader.world_y < i32::from(ocean.height));
        assert_ne!(trader.world_y, 200);
        assert!(ocean.is_navigable(trader.world_x, trader.world_y));
    }

    #[test]
    fn free_trader_assign_next_port_uses_ocean_path_to_docking_tile() {
        let szs = anno_formats::szs::SzsFile {
            chunks: Vec::new(),
            islands: vec![anno_formats::szs::Island {
                number: 0,
                width: 5,
                height: 5,
                x_pos: 5,
                y_pos: 5,
                fertilities: [7; 8],
                tiles: Vec::new(),
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
        };
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 7, 7));
        sim.ocean_map = Some(OceanMap::from_scenario(&szs));
        let mut trader = crate::free_trader::FreeTrader::spawn_at(2, 7);

        sim.assign_next_port(&mut trader);

        let ocean = sim.ocean_map.as_ref().unwrap();
        assert_eq!(trader.target_warehouse, Some(0));
        assert_ne!((trader.target_x, trader.target_y), (7, 7));
        assert!(ocean.is_navigable(trader.target_x, trader.target_y));
        assert!(trader.path_required);
        assert!(!trader.path.is_empty());
        assert_eq!(
            trader.path.last().copied(),
            Some((trader.target_x, trader.target_y))
        );
        assert!(
            trader.path.iter().all(|&(x, y)| ocean.is_navigable(x, y)),
            "free-trader ocean path must not cross land"
        );
    }

    #[test]
    fn free_trader_without_candidate_ports_leaves_by_ocean_edge() {
        let szs = anno_formats::szs::SzsFile {
            chunks: Vec::new(),
            islands: vec![anno_formats::szs::Island {
                number: 0,
                width: 5,
                height: 5,
                x_pos: 5,
                y_pos: 5,
                fertilities: [7; 8],
                tiles: Vec::new(),
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
        };
        let mut sim = Simulation::new();
        sim.ocean_map = Some(OceanMap::from_scenario(&szs));
        let mut trader = crate::free_trader::FreeTrader::spawn_at(2, 7);

        sim.assign_next_port(&mut trader);

        assert_eq!(trader.target_warehouse, None);
        assert!(trader.leaving);
        assert_eq!((trader.target_x, trader.target_y), (0, 7));
        assert!(trader.path_required);
        assert!(!trader.path.is_empty());
        assert_eq!(
            trader.path.last().copied(),
            Some((trader.target_x, trader.target_y))
        );
        let mut warehouses = Vec::new();
        let mut gold = Vec::new();
        let removed = crate::free_trader::tick_one(&mut trader, &mut warehouses, &mut gold);
        assert!(removed);
        assert!(!trader.active);
    }

    #[test]
    fn free_trader_unreachable_port_leaves_instead_of_land_cutting() {
        let szs = anno_formats::szs::SzsFile {
            chunks: Vec::new(),
            islands: vec![anno_formats::szs::Island {
                number: 0,
                width: 80,
                height: 80,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: Vec::new(),
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
        };
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 40, 40));
        sim.ocean_map = Some(OceanMap::from_scenario(&szs));
        let mut trader = crate::free_trader::FreeTrader::spawn_at(0, 85);

        sim.assign_next_port(&mut trader);

        assert_eq!(trader.target_warehouse, None);
        assert!(trader.leaving);
        assert_eq!((trader.target_x, trader.target_y), (0, 85));
        assert!(trader.path_required);
        assert!(trader.path.is_empty());
        assert!(sim
            .ocean_map
            .as_ref()
            .unwrap()
            .is_navigable(trader.target_x, trader.target_y));
    }

    #[test]
    fn free_trader_port_profit_score_uses_trade_sliders() {
        let trader = crate::free_trader::FreeTrader::spawn_at(0, 0);
        let mut buyer = Warehouse::new(0, 0, 1, 0);
        buyer.set_buy_max_stock(crate::types::Good::Tools, Some(20));

        assert!(free_trader_port_profit_score(&trader, &buyer, 20_000) > 0);
        assert_eq!(free_trader_port_profit_score(&trader, &buyer, 0), 0);

        let mut empty_trader = crate::free_trader::FreeTrader::spawn_at_with_capacity(0, 0, 60);
        empty_trader.stock.clear();
        let mut seller = Warehouse::new(0, 0, 2, 0);
        seller.deposit(crate::types::Good::Wool, 20);
        seller.set_sell_min_keep(crate::types::Good::Wool, Some(5));

        assert!(free_trader_port_profit_score(&empty_trader, &seller, 0) > 0);
        seller.set_sell_price(
            crate::types::Good::Wool,
            Some(crate::prices::price_of(crate::types::Good::Wool).sell + 1),
        );
        assert_eq!(free_trader_port_profit_score(&empty_trader, &seller, 0), 0);
    }

    #[test]
    fn free_trader_port_profit_score_ignores_non_ware_goods() {
        let mut empty_trader = crate::free_trader::FreeTrader::spawn_at_with_capacity(0, 0, 60);
        empty_trader.stock.clear();
        let mut local_only_seller = Warehouse::new(0, 0, 2, 0);
        local_only_seller.deposit(crate::types::Good::Silk, 20);
        local_only_seller.set_sell_min_keep(crate::types::Good::Silk, Some(0));

        assert_eq!(
            crate::prices::original_ware_id(crate::types::Good::Silk),
            None
        );
        assert_eq!(
            free_trader_port_profit_score(&empty_trader, &local_only_seller, 0),
            0
        );

        let mut cargo_trader = crate::free_trader::FreeTrader::spawn_at_with_capacity(0, 0, 60);
        cargo_trader.stock.clear();
        cargo_trader.stock.push((crate::types::Good::Fish, 20));
        let mut local_only_buyer = Warehouse::new(0, 0, 3, 0);
        local_only_buyer.set_buy_max_stock(crate::types::Good::Fish, Some(20));

        assert_eq!(
            crate::prices::original_ware_id(crate::types::Good::Fish),
            None
        );
        assert_eq!(
            free_trader_port_profit_score(&cargo_trader, &local_only_buyer, 20_000),
            0
        );
    }

    #[test]
    fn free_trader_targeting_uses_twelve_nearest_port_shortlist() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        for x in 1..=12 {
            sim.warehouses.push(Warehouse::new(0, 0, x, 0));
        }
        let mut far = Warehouse::new(0, 0, 190, 190);
        far.set_buy_max_stock(crate::types::Good::Tools, Some(30));
        sim.warehouses.push(far);

        let mut trader = crate::free_trader::FreeTrader::spawn_at(0, 0);
        // With the source-shaped 12-port shortlist, the distant 13th
        // warehouse is not eligible even though it now has profitable
        // demand.
        sim.seed_source_rand(12);
        sim.assign_next_port(&mut trader);

        assert_ne!(trader.target_warehouse, Some(12));
        assert!(
            trader.target_warehouse.is_some_and(|idx| idx < 12),
            "target should stay inside the 12-nearest shortlist"
        );
    }

    #[test]
    fn free_trader_targeting_prefers_profitable_port_inside_shortlist() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        for x in 1..=12 {
            sim.warehouses.push(Warehouse::new(0, 0, x, 0));
        }
        sim.warehouses[5].set_buy_max_stock(crate::types::Good::Tools, Some(30));

        let mut trader = crate::free_trader::FreeTrader::spawn_at(0, 0);
        sim.seed_source_rand(0);
        sim.assign_next_port(&mut trader);

        assert_eq!(trader.target_warehouse, Some(5));
    }

    #[test]
    fn kind12_figure_claims_and_releases_its_source_event_slot() {
        let mut sim = Simulation::new();
        let mut map = IslandMap::new_open(3, 16, 16);
        map.source_world_origin = (220, 260);
        sim.island_maps.push(map);
        sim.seed_source_rand(1);
        let location = SourceKind13Location {
            island_id: 3,
            tile_x: 5,
            tile_y: 7,
            orientation: 0,
            variant: 0,
            source_owner: 0,
            phase: 0,
            state_bits: 0,
            population_group: 0,
            amount: 0x40,
            lifecycle_flags: 0,
        };

        let figure = sim
            .allocate_source_kind12_figure(location, 0)
            .expect("source registry admits an unused anchor");
        let slot = figure.source_event_slot.expect("kind-12 event slot");
        assert_eq!(
            sim.source_figure_events.slot(slot),
            Some(crate::source_figure_event::SourceFigureEventSlot {
                route_radius: 0,
                x: 115,
                y: 137,
                target_x: -1,
                target_y: -1,
                lifecycle: 0,
                owner: 0,
                route_cursor: 0,
                state: 0xc0,
                transfer_amount_fixed: 0,
                resource_ware_slot: 0,
                route_program: [0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            })
        );
        assert!(sim.allocate_source_kind12_figure(location, 0).is_none());

        sim.figures.push(figure);
        sim.tick_entities(100);
        assert!(sim.figures.is_empty());
        assert!(sim.source_figure_events.slot(slot).unwrap().is_free());
        assert!(sim.allocate_source_kind12_figure(location, 0).is_some());
    }

    #[test]
    fn transfer_event_uses_the_oriented_source_root_center() {
        let mut sim = Simulation::new();
        let mut map = IslandMap::new_open(3, 16, 16);
        map.source_world_origin = (220, 260);
        sim.island_maps.push(map);
        let mut root = SourceMapCellState::new_static(
            3,
            5,
            7,
            &anno_formats::cod::BuildingDef {
                kind: "MARKT".into(),
                size: (3, 2),
                source_transfer_figure_limit: 2,
                source_transfer_radius: 16,
                ..Default::default()
            },
            0,
        )
        .unwrap();
        root.source_map_owner_slot = 4;

        let first = sim.prepare_source_transfer_event(root).unwrap().unwrap();
        assert_eq!((first.x, first.y, first.owner), (116, 137, 4));
        assert!(sim.source_figure_events.slot(first.slot).unwrap().is_free());
        assert!(sim.activate_source_transfer_event(first));
        assert_eq!(
            sim.source_figure_events.slot(first.slot),
            Some(crate::source_figure_event::SourceFigureEventSlot {
                route_radius: 16,
                x: 116,
                y: 137,
                target_x: -1,
                target_y: -1,
                lifecycle: 1,
                owner: 4,
                route_cursor: 0,
                state: 0xc0,
                transfer_amount_fixed: 0,
                resource_ware_slot: 0,
                route_program: [0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            })
        );
        let second = sim.prepare_source_transfer_event(root).unwrap().unwrap();
        assert!(sim.activate_source_transfer_event(second));
        assert_eq!(sim.prepare_source_transfer_event(root), Err(()));
    }

    #[test]
    fn city_cart_config_follows_the_compiled_figurnr_selector() {
        let mut sim = Simulation::new();
        sim.city_cart_config = carrier::CityCartConfig {
            max_load: 6,
            movement_speed: 300,
            sprite_base: 496,
            frame_speed_ms: 60,
            frames_per_direction: 8,
        };
        sim.city_cart_traeger2_config = carrier::CityCartConfig {
            max_load: 4,
            movement_speed: 220,
            sprite_base: 32,
            frame_speed_ms: 85,
            frames_per_direction: 8,
        };

        assert_eq!(
            sim.city_cart_config_for(SourceTransferFigure::Karren),
            Some(sim.city_cart_config)
        );
        assert_eq!(
            sim.city_cart_config_for(SourceTransferFigure::Traeger2),
            Some(sim.city_cart_traeger2_config)
        );
        assert_eq!(
            sim.city_cart_config_for(SourceTransferFigure::Unknown),
            None
        );
    }

    #[test]
    fn kind12_pool_exhaustion_leaves_its_event_candidate_coordinate_free() {
        let mut sim = Simulation::new();
        let mut map = IslandMap::new_open(3, 16, 16);
        map.source_world_origin = (220, 260);
        sim.island_maps.push(map);
        sim.seed_source_rand(1);
        let location = SourceKind13Location {
            island_id: 3,
            tile_x: 5,
            tile_y: 7,
            orientation: 0,
            variant: 0,
            source_owner: 0,
            phase: 0,
            state_bits: 0,
            population_group: 0,
            amount: 0x40,
            lifecycle_flags: 0,
        };
        let mut occupied = Figure::new();
        occupied.action = ActionType::Walking;
        sim.figures = vec![occupied; SourceFigureRecordLayout::CAPACITY];

        assert!(sim.allocate_source_kind12_figure(location, 0).is_none());
        let candidate_slot = SourceFigureEventRegistry::source_index(115, 137) as u16;
        let candidate = sim.source_figure_events.slot(candidate_slot).unwrap();
        assert!(candidate.is_free());
        assert_eq!(candidate.lifecycle, 0);
        assert_eq!(candidate.route_cursor, 0);
        assert_eq!(candidate.state, 0xc0);
        assert_eq!(candidate.route_program[0], 0xc0);

        sim.figures.clear();
        assert!(sim.allocate_source_kind12_figure(location, 0).is_some());
    }

    #[test]
    fn outcome_pending_while_rivals_alive() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.warehouses.push(Warehouse::new(0, 0, 30, 40));
        sim.players[0].population[1] = 100;
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].population[1] = 100;
        sim.warehouses.push(Warehouse::new(0, 1, 32, 42));

        sim.evaluate_outcomes();
        assert_eq!(sim.outcome, GameOutcome::Pending);
    }
}
