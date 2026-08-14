//! Save/load: bincode-serialized snapshot of the mutable runtime state.
//!
//! Immutable scenario data (BuildingDef, IslandMap, OceanMap) is reloaded
//! from the original COD/SZS files; only the player-mutable game state is
//! captured. CoverageMap and AI controllers are recomputed from scratch.

use crate::building::BuildingInstance;
use crate::combat::{DiplomacyMatrix, MilitaryUnit, SourceDynamicCombatFigure};
use crate::data_bridge::{
    SourceCityTable, SourceKind13DispatchState, SourceKind13LocationTable, SourceKind4Occupant,
};
use crate::entity::Figure;
use crate::player::Player;
use crate::simulation::Simulation;
use crate::source_cell::{SourceMapCellState, SOURCE_GROWTH_BUCKET_COUNT};
use crate::source_route::SourceDynamicMapObject;
use crate::trade::{TradeRoute, TradeShip};
use crate::warehouse::Warehouse;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// On-disk save format version. Bump on incompatible changes.
///
/// v13: FREE_TRADER_SLOT / NATIVE_SLOT corrected from 5 / 4 to
///      4 / 5 (binary-confirmed). Pre-v13 saves have building /
///      unit owner values for those factions swapped — refuse
///      to load them rather than produce a corrupt world.
/// v14: `Objective` gained a `ReachTotalPopulation` variant for
///      AUFTRAG4 triples whose `total` is independent of any
///      tier sub-goal. Bincode enum-variant indices shift, so
///      pre-v14 saves cannot decode the new variant.
/// v15: `Warehouse` gained `default_capacity: u16` carrying the
///      Kontor's `Maxlager` (50/75/100/20-small variants).
///      `#[serde(default)]`
///      makes pre-v15 saves loadable with the legacy 30 cap,
///      but `Warehouse` field order shifts so we bump anyway.
/// v16: `MilitaryUnit` gained `name: String` for SHIP4-spawned
///      warships. `#[serde(default)]` keeps pre-v16 saves
///      loadable with empty names.
/// v17: Removed graph-only `EconomyHistory` from save state after the
///      economy graph / sparkline UI was audited out as non-original.
/// v18: Removed non-original construction priority from
///      `BuildingInstance`.
/// v19: `TradeShip` gained `cargo_capacity` so saved ships retain
///      their figuren.cod `Maxware × 10` class capacity.
/// v20: `TradeShip` gained `class` so HANDEL1/HANDEL2 scenario
///      hull identity survives into source-derived sprite rendering.
/// v21: `BuildingInstance` gained `fire_damage_ticks` so fire damage
///      honours haeuser.cod `Maxbrand: 4`.
/// v22: `TradeShip` gained `name: String` so SHIP4-authored trade-ship
///      names survive into save files and the manual ships list.
/// v23: `TradeShip` gained `source_route_window`, preserving the source
///      caller's normal versus short target-retry search radius.
/// v24: `TradeShip` gained `source_target_approach_radius`, preserving the
///      source figure's direct-target `Shotradius >> 3` argument.
/// v25: `TradeShip` gained `source_target_descriptor`, preserving the live
///      four-byte target selected for source ship routing.
/// v26: `BuildingInstance` gained `source_dynamic_object_slot`, preserving
///      a live HQ's source island map-object-table entry across save/load.
/// v27: scenario-created source dynamic-map objects are serialized separately
///      from live player-built HQ instances.
/// v28: player-created `BuildingInstance`s retain their source placement
///      command identity instead of only their local definition index.
/// v29: `SourceBuildingCommand` gained every packed `FUN_004631b0` field.
/// v30: source command-root animation records persist their live frame state.
/// v31: source command-root records retain their fixed-point production inputs.
/// v32: `Figure` gained `destination_kind`, preserving the selected
/// source command-root kind for an in-flight carrier supplier.
/// v33: source command-root records retain outstanding type-8 supplier
/// reservations.
/// v34: warehouse city-good reservations persist across in-flight carriers.
/// v35: `Figure` gained type-11 city-cart route identity and its source-root
/// origin, preserving in-flight KARREN transfers.
/// v36: `Warehouse` gained source-city BGRUPPE population, preserving the
/// type-11 cart selector's city-local demand priorities.
/// v37: `Figure` gained source supplier-root coordinates, separating a
/// type-11 cart's reached footprint cell from its selected root record.
/// v38: `Warehouse` gained its oriented source KONTOR footprint for type-8
/// source-grid supplier selection.
/// v39: `Warehouse` gained its compiled type-8 TRAEGER source path class.
/// v40: `Figure` gained exact type-8 1/32-good in-flight cargo.
/// v41: `Warehouse` gained exact type-11 city-store balances.
/// v42: `Figure` gained source route traversal speed and remaining cell
/// distance, preserving in-flight TRAEGER/KARREN movement timing.
/// v43: `Figure` gained continuous source-grid coordinates for in-flight
/// TRAEGER/KARREN rendering and route-state replay.
/// v44: `Figure` gained its source-grid Z coordinate, seeded from the
/// replayed map-cell `Posoffs` terrain elevation.
/// v45: `Figure` gained its source animation-time accumulator, preserving
/// per-figure `ANIM` phase across save/load.
/// v46: the source animation accumulator changed from a presentation elapsed
/// clock to the executable's per-frame remainder.
/// v47: source kind-13 map-location anchors persist for the kind-`0x12`
/// civilian supplier.
/// v48: kind-13 map locations use the source's fixed 4,160-slot hash table.
/// v49: fixed source city records and the kind-12 dispatch phase/cursor are
///      persisted so the next two-city source update remains reproducible.
/// v50: military units retain the source island occupied by kind-4 land
///      figures, preserving source city-activity gates across save/load.
/// v51: authored `SOLDAT3` kind-4 occupancy persists independently of the
///      partially reconstructed local combat entity model.
/// v52: source kind-4 records retain their runtime slots, authored state,
///      position, and selected animation for lifecycle replay.
/// v53: source-backed military units retain their `SOLDAT3` runtime slots
///      and compiled figure definitions, linking combat lifecycle to source
///      island-owner occupancy.
/// v54: source kind-4 occupancy retains the four-byte `SOLDAT3` descriptor
///      consumed by the type-4 dispatcher.
/// v55: source-backed military units retain their non-null type-4 target
///      descriptor alongside source occupancy.
/// v56: source-backed military units and type-4 occupancy retain the
///      `SOLDAT3` idle-anchor descriptor.
/// v57: the MSVC-compatible source `rand()` state persists across saves.
/// v58: the source 100-ms clock and kind-4 idle-gate timestamps persist.
/// v59: native kind-4 timestamps begin after the source `Worktime` cadence.
/// v60: type-4 land figures retain continuous source-engine motion state.
/// v61: type-4 figures and runtime slots retain the `FUN_004581f0`
/// failed-route retry counter that controls shifted search windows.
/// v62: type-4 figures and runtime slots retain the packed `+0x130` direction
/// program and its `+0x02` cursor between dispatcher updates.
/// v63: type-4 figures retain the `FUN_00456d00` terminal route residual.
/// v64: source type-4 dispatch retains source player/session state.
/// v65: dynamic `0x84a`/`0x84b` source combat figures retain their
/// category-local live records independently of local unit types.
/// v66: source figures retain SHIP4 byte `0x42` as the category-1/2/3
/// `FUN_00454250` score-state tier.
/// v67: SHIP4 policy-slot inputs and category-6 source action timestamps
/// persist across save/load.
/// v68: SHIP4 policy slots retain their executable-resolved Ware bytes.
/// v69: source category-record target-descriptor bytes persist across saves.
/// v70: immediate category-6 action records persist across saves.
/// v71: executor-created kind-15 figure records persist across saves.
/// v72: kind-15 figure records retain their source `Worktime:` remainder.
/// v73: source map-cell records retain their compiled hit threshold.
/// v74: building instances omit the non-source production-fatigue counter.
/// v75: source category-6 deferred hits, map-root accumulators, and terminal
///      type-7 events persist across save/load.
/// v76: source map-cell records retain type-7 replacement footprint and
///      `Ruinenr` metadata.
/// v77: source map-cell records retain type-7 ruin table selection and
///      replacement dimensions for synchronous source-random replay.
/// v78: pending source map clear/ruin events persist until the renderer has
///      consumed their frozen terminal-command replacement draws.
/// v79: source map-cell records distinguish raw definition dimensions from
///      oriented map footprints for type-7 replacement draw selection.
/// v80: source map-cell records and pending terminal writes retain source
///      orientation for the selected ruin command.
/// v81: terminal replacement commands retain the source frame selector and
///      map-owner selector forwarded by `FUN_00463f40`.
/// v82: terminal fallback replacements retain per-cell strand-table
///      selection in source write order.
/// v83: all static map roots retain category-6 terminal accumulators outside
///      the renderer's selector-state subset.
/// v84: static backing map cells persist for `Ruinenr = 0xff` terminal replay.
/// v85: source map-cell records retain the backing command definition offset
///      needed to reconstruct visible NORUINE terminal replay.
/// v86: kind-13 source records retain their initialized lifecycle state, and
///      the 70-record phase dispatcher retains its clocks and physical cursor.
/// v87: source city records retain luxury satisfaction operands, group
///      weights, pressure, and the resulting kind-13 transfer inputs.
/// v88: source city records retain pending kind-13 BGruppe promotion
///      reservations, their origin coordinates, and the promotion block bit.
/// v89: warehouses retain the source city's 1/32-good carrier commitments
///      that BGruppe promotion subtracts from available construction stock.
/// v90: completed kind-13 BGruppe replacements persist until the map-command
///      consumer has replayed their INSELHAUS writes.
/// v91: source figures retain their shared event slots and the corresponding
///      `DAT_00505e38` coordinate registry survives save/load.
/// v92: source roots retain type-11 `Figurnr`, and in-flight city figures
///      retain the selected authored animation layout.
/// v93: in-flight type-11 figures retain the exact source-grid predecessor
///      steps used to reconstruct their shared-event route program.
/// v94: in-flight type-11 figures retain the nested source production kind
///      needed to distinguish MARKT and KONTOR arrival behavior.
/// v95: source figure events retain the type-11 fixed-point transfer amount
///      passed from supplier collection to terminal delivery.
/// v96: in-flight type-11 figures retain the source map-owner selector used
///      to address the origin city's inventory and root count.
/// v97: in-flight type-11 figures retain their authored `Maxtrag`, needed for
///      the source supplier-arrival top-up in `FUN_0047d640`.
/// v98: source roots retain the `FUN_0047daf0` enable/cooldown gate and
///      authored interval, and the global source map-dispatch phase and
///      accumulator survive save/load.
/// v99: source roots retain the `FUN_0047daf0` byte-`+0x0f` blocked flag.
/// v100: source roots retain the compiled `Ware` selector used by scheduler
///       activity.
/// v101: source roots retain the compiled `Maxnorohst` threshold and their
///       saturated no-raw-material counter.
/// v102: source roots retain plantation worker selectors, raw-resource
///       reservation/path state, and the source `+0x12` production-time
///       accumulator; figures retain type-12 worker route/home and animation
///       selectors.
/// v103: static source cells retain the `ROHSTWACHS` `+0xa9` resource
///       selector used by plantation-worker traversal after regrowth.
/// v104: source resource-environment phase/cursor state and dry-cell markers
///       retain the `FUN_0046b3e0` / `FUN_0047c920` transition schedule.
/// v105: source terrain-event scheduler rows and type-17 event-slot fields
///       retain the `FUN_0044bd00` / `FUN_0045bfc0` lifecycle state.
/// v106: source terrain-event four-phase scheduling counters retain the
///       `FUN_0046b920` candidate-emission cadence.
/// v107: Klinik terminal relocations retain their shared deferred-event rows.
/// v108: type-4 alternate-state selectors retain `SOLDAT3 +0x1c` across
///       scenario snapshots and replay.
/// v109: category-4 direct attacks retain their `FUN_004546e0` actions and
///       deferred type-one rows across save/load.
/// v110: terminal category-1 through -4 figure-control events retain the
///       `FUN_00443bf0` / `FUN_0045e1f0` terminal-state transition.
/// v111: terminal military figures retain the `FUN_00446120` motion slice
///       until `FUN_00451890` reaches its removal boundary.
/// v112: stationary terminal category-one through -three records retain
///       their `FUN_0045d380` motion slice until generic removal.
/// v113: dynamic category-one through -three records retain the directional
///       `FUN_0044a690` state used by their terminal motion slice.
/// v114: dynamic terminal slices retain the generic record's vertical and
///       bit-two locked-motion state inspected by `FUN_00446120`.
/// v115: dynamic category-one through -three records retain the dedicated
///       `FUN_0045e1f0` opcode-`0x19` score-state byte, action payloads,
///       and deferred type-one records.
/// v116: category-one through -three `Shotfignr:` launches retain live
///       `FUN_00447e90` kind-14 records.
/// v117: source player state retains `DAT_005bafc8`, which gates remote
///       category-six controller actions.
/// v118: diplomacy retains directed `DAT_005b7770` relationship codes for
///       source combat candidate filtering.
/// v119: diplomacy retains directed `DAT_005b77b0` attitude codes for the
///       source `FUN_00476130` event path.
/// v120: diplomacy retains each player's 32-slot `DAT_005b77f0` notification
///       queue plus the `DAT_005b7750` activity counters and queue-state byte.
/// v121: source cities retain the live `+0x218` resident amount used by
///       `FUN_0047f790` and diplomacy score aggregation.
/// v122: diplomacy retains PLAYER4's `DAT_005b7730`, `DAT_005b7740`, and
///       `DAT_005b7750` score-input arrays.
/// v123: diplomacy retains PLAYER4's `FUN_00475c60` policy flags, thresholds,
///       city-strength targets, and source player-state bytes.
/// v124: source figure purchases retain the shared category-1/2/3/5 control
///       bytes that gate `0x84d` ownership transfers.
/// v125: source player-controller timers, action stacks, city gates, and
///       category-1/2/3 roster state retain `FUN_0042b4b0` scheduling.
/// v126: the `FUN_00423710` global controller difficulty mode persists.
/// v127: controller city-management profile retains source offset `+0x10638`.
/// v128: removes the synthetic controller `+0x106f8` profile field after an
///       executable instruction census found its sole read and no writes.
/// v129: controller `+0x3e7c` retains its physical source-city slot.
/// v130: controller `+0x08` initialization ticks retain the strict
///       `FUN_00429070` 36,000-tick reset cadence.
/// v131: controller city-management profiles retain their physical city slot,
///       arrival figure, target island, and action budget.
/// v132: source cities retain their allocation tile and `+0x1e0` readiness
///       tick; controller arrivals retain the state-three target tile.
/// v133: scenario city `+0x19` stores its island-local city-pointer slot,
///       separately from city owner `+0x1a`.
/// v134: source controller state-two island cursor, capability selector,
///       selected island, and area thresholds persist across saves.
/// v135: shared dynamic category-1/2/3 figures retain their live
///       `FUN_00455a20` route program and cursor.
/// v136: player controllers retain `FUN_00417aa0` state-seven construction
///       work queues.
/// v137: player controllers retain the `+0x1d14` construction-consumer
///       cursor and its live scan state.
/// v138: subsystem timer accumulators and the fractional game-clock
///       accumulator persist, so save→load is phase-exact (required for
///       tick-lockstep replay and state-hash comparison).
/// v139: source city records retain the `FUN_0047f8a0` ware
///       demand/supply accumulators, per-slot fulfillment bytes and
///       histories, and the worst-slot byte — the exact per-city
///       consumption cycle's state.
/// v140: static map roots retain the compiled active `Kosten` operating
///       cost; per-player maintenance accrues from roots (city `+0x1d8`
///       semantics) instead of production instances.
/// v141: raw-resource growth timer. The two 32-entry bucket clocks
///       `DAT_0054a2f4` / `DAT_00562da8` persist — the original saves them
///       in its `"TIMERS"` chunk (`1602_exe.c:94964-94984`) — and static map
///       roots carry their compiled record group plus the per-cell
///       `DAT_00562dc8` entry (bucket, counter snapshot, growth phase,
///       placement jitter). The table itself is not saved by the original
///       either; it is rebuilt from tile state on load.
/// v142: the city hazard block. Source city records carry `+0x1fe` as the
///       signed affliction **count** it really is rather than a boolean, and
///       the `DAT_005a5100` affliction table and undrained `FUN_0046a8c0`
///       type-7 actions persist. The original saves neither the table nor
///       the kind-13 records' two affliction bits, so an original save/load
///       cures every plague and puts out every fire; keeping them here is a
///       deliberate improvement, not a reproduction.
pub const SAVE_VERSION: u32 = 142;

/// Oldest save version this build can still deserialize. Anything
/// older has either a hard binary incompatibility (enum-variant
/// index shift, struct field reorder) or a known data-corruption
/// risk and is hard-rejected.
///
/// v52 baseline: carrier figures retain type-8 supplier identity, exact
/// 1/32-good cargo, continuous source position, and route timing; city stores retain fractional balances; type-11 carts
/// retain origin identity; source command-root records retain fixed-point
/// production inputs, selector fields, and in-flight supplier reservations;
/// warehouse records retain city population, source-root footprint, and
/// type-8 path-class data; figures retain independent source animation
/// accumulators and the kind-13 source slot table in a distinct bincode layout.
///
/// v142 baseline: bincode is not self-describing, so neither the city
/// record's `growth_blocked: bool` -> `active_afflictions: i16` widening nor
/// the four fields appended to `SourceMapCellState` can be read out of an
/// older payload — `#[serde(default)]` cannot fill a field the encoding
/// never delimits. Everything before v142 is a hard binary incompatibility.
pub const MIN_LOADABLE_VERSION: u32 = 142;

/// Magic bytes prefixing every save file.
pub const SAVE_MAGIC: [u8; 4] = *b"ASV1";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SaveState {
    pub version: u32,
    pub game_clock: u32,
    /// Milliseconds not yet promoted into `game_clock`.
    pub clock_frac_ms: u32,
    pub speed_multiplier: u32,
    pub paused: bool,
    pub autosave_timer_ms: u32,
    /// Subsystem cadence accumulators, in `step()` dispatch order. Restored
    /// on load so the next fire of each subsystem lands on the same tick it
    /// would have without the save/load roundtrip.
    pub timer_production_accumulator_ms: u32,
    pub timer_population_accumulator_ms: u32,
    pub timer_diplomacy_accumulator_ms: u32,
    pub timer_market_accumulator_ms: u32,
    pub timer_ships_accumulator_ms: u32,
    pub timer_events_accumulator_ms: u32,
    pub players: Vec<Player>,
    pub buildings: Vec<BuildingInstance>,
    pub source_dynamic_map_objects: Vec<SourceDynamicMapObject>,
    pub source_map_cell_states: Vec<SourceMapCellState>,
    pub source_static_map_roots: Vec<SourceMapCellState>,
    pub source_static_map_backing_cells: Vec<SourceMapCellState>,
    pub source_kind13_locations: SourceKind13LocationTable,
    pub source_kind13_dispatch: SourceKind13DispatchState,
    /// The source affliction table `DAT_005a5100`. The original persists
    /// none of it — no save chunk touches the table and `FUN_00485530`
    /// drops the kind-13 records' two affliction bits — so an original
    /// save/reload cures every plague and extinguishes every fire. This
    /// port keeps it, which is strictly better behaviour.
    #[serde(default)]
    pub source_afflictions: crate::data_bridge::SourceAfflictionTable,
    /// Posted-but-undrained `FUN_0046a8c0` type-7 map actions.
    #[serde(default)]
    pub source_deferred_map_demolitions:
        Vec<crate::simulation::SourceDeferredMapDemolition>,
    pub source_kind13_replacement_commands: Vec<crate::simulation::SourceKind13ReplacementCommand>,
    pub source_cities: SourceCityTable,
    pub source_figure_events: crate::source_figure_event::SourceFigureEventRegistry,
    pub source_kind4_occupants: Vec<SourceKind4Occupant>,
    pub source_dynamic_combat_figures: Vec<SourceDynamicCombatFigure>,
    pub source_dynamic_route_programs: Vec<crate::simulation::SourceDynamicRouteProgram>,
    pub source_kind6_actions: Vec<crate::combat::SourceKind6Action>,
    pub source_kind4_actions: Vec<crate::combat::SourceKind4Action>,
    pub source_kind13_actions: Vec<crate::combat::SourceKind13Action>,
    pub source_kind14_combat_figures: Vec<crate::combat::SourceKind14CombatFigure>,
    pub source_kind15_combat_figures: Vec<crate::combat::SourceKind15CombatFigure>,
    pub source_kind6_deferred_hits: Vec<crate::combat::SourceKind6DeferredHit>,
    pub source_kind4_deferred_hits: Vec<crate::combat::SourceKind4DeferredHit>,
    pub source_kind13_deferred_hits: Vec<crate::combat::SourceKind13DeferredHit>,
    pub source_kind4_deferred_relocations: Vec<crate::simulation::SourceKind4DeferredRelocation>,
    pub source_kind6_terminal_events: Vec<crate::simulation::SourceKind6TerminalEvent>,
    pub source_combat_terminal_events: Vec<crate::simulation::SourceCombatTerminalEvent>,
    pub source_combat_terminal_slices: Vec<crate::simulation::SourceCombatTerminalSlice>,
    pub tile_clears: Vec<crate::simulation::TileClear>,
    pub source_kind4_dispatch: crate::combat::SourceKind4DispatchState,
    #[serde(with = "crate::serde_util::byte_array")]
    pub source_shared_figure_control_flags:
        [u8; crate::combat::SOURCE_DYNAMIC_SHARED_SLOT_CAPACITY as usize],
    pub source_player_controllers:
        [crate::simulation::SourcePlayerController; crate::combat::SOURCE_KIND4_PLAYER_SLOT_COUNT],
    pub source_player_controller_cursor: u8,
    pub source_controller_difficulty_mode: u8,
    pub source_time_ticks: u32,
    pub source_time_remainder_ms: u32,
    pub source_resource_environment_elapsed_ms: u32,
    pub source_resource_environment_phase: u8,
    pub source_resource_environment_cursor: usize,
    pub source_resource_environment_last_phase: Vec<u8>,
    pub source_terrain_event_schedules: Vec<[crate::simulation::SourceTerrainEventSchedule; 8]>,
    pub source_terrain_event_schedule_counters: Vec<u8>,
    pub source_map_dispatch_elapsed_ms: u32,
    pub source_map_dispatch_phase: u8,
    pub source_city_dispatch_elapsed_ms: u32,
    pub source_city_dispatch_phase: u8,
    pub source_city_dispatch_cursor: usize,
    pub source_rng_state: crate::source_rand::SourceRand,
    pub warehouses: Vec<Warehouse>,
    pub figures: Vec<Figure>,
    pub military_units: Vec<MilitaryUnit>,
    pub diplomacy: DiplomacyMatrix,
    pub trade_routes: Vec<TradeRoute>,
    pub trade_ships: Vec<TradeShip>,
    #[serde(default)]
    pub objectives: crate::objectives::ObjectiveSet,
    /// `DAT_0054a2f4` and `DAT_00562da8`, the growth-timer bucket clocks. The
    /// original saves both in its 0x298-byte `"TIMERS"` chunk and rebuilds the
    /// entry table itself from tile state, so only these two arrays are state.
    #[serde(default = "default_source_growth_bucket_elapsed_ms")]
    pub source_growth_bucket_elapsed_ms: [u32; SOURCE_GROWTH_BUCKET_COUNT],
    #[serde(default = "default_source_growth_bucket_phase")]
    pub source_growth_bucket_phase: [u8; SOURCE_GROWTH_BUCKET_COUNT],
}

fn default_source_growth_bucket_elapsed_ms() -> [u32; SOURCE_GROWTH_BUCKET_COUNT] {
    [0; SOURCE_GROWTH_BUCKET_COUNT]
}

fn default_source_growth_bucket_phase() -> [u8; SOURCE_GROWTH_BUCKET_COUNT] {
    [0; SOURCE_GROWTH_BUCKET_COUNT]
}

#[derive(Debug)]
pub enum SaveError {
    Io(std::io::Error),
    BadMagic,
    VersionMismatch { found: u32, expected: u32 },
    Decode(String),
    Encode(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "io: {e}"),
            SaveError::BadMagic => write!(f, "not a valid save file"),
            SaveError::VersionMismatch { found, expected } => {
                write!(f, "save version {found}, expected {expected}")
            }
            SaveError::Decode(e) => write!(f, "decode: {e}"),
            SaveError::Encode(e) => write!(f, "encode: {e}"),
        }
    }
}

impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e)
    }
}

impl Simulation {
    /// Build a save snapshot of the current mutable game state.
    pub fn snapshot(&self) -> SaveState {
        SaveState {
            version: SAVE_VERSION,
            game_clock: self.game_clock,
            clock_frac_ms: self.clock_frac_ms,
            speed_multiplier: self.speed_multiplier,
            paused: self.paused,
            autosave_timer_ms: self.autosave_timer_ms,
            timer_production_accumulator_ms: self.timer_production.accumulator_ms,
            timer_population_accumulator_ms: self.timer_population.accumulator_ms,
            timer_diplomacy_accumulator_ms: self.timer_diplomacy.accumulator_ms,
            timer_market_accumulator_ms: self.timer_market.accumulator_ms,
            timer_ships_accumulator_ms: self.timer_ships.accumulator_ms,
            timer_events_accumulator_ms: self.timer_events.accumulator_ms,
            players: self.players.clone(),
            buildings: self.buildings.clone(),
            source_dynamic_map_objects: self.source_dynamic_map_objects.clone(),
            source_map_cell_states: self.source_map_cell_states.clone(),
            source_static_map_roots: self.source_static_map_roots.clone(),
            source_static_map_backing_cells: self.source_static_map_backing_cells.clone(),
            source_kind13_locations: self.source_kind13_locations.clone(),
            source_kind13_dispatch: self.source_kind13_dispatch.clone(),
            source_afflictions: self.source_afflictions.clone(),
            source_deferred_map_demolitions: self.source_deferred_map_demolitions.clone(),
            source_kind13_replacement_commands: self.source_kind13_replacement_commands.clone(),
            source_cities: self.source_cities.clone(),
            source_figure_events: self.source_figure_events.clone(),
            source_kind4_occupants: self.source_kind4_occupants.clone(),
            source_dynamic_combat_figures: self.source_dynamic_combat_figures.clone(),
            source_dynamic_route_programs: self.source_dynamic_route_programs.clone(),
            source_kind6_actions: self.source_kind6_actions.clone(),
            source_kind4_actions: self.source_kind4_actions.clone(),
            source_kind13_actions: self.source_kind13_actions.clone(),
            source_kind14_combat_figures: self.source_kind14_combat_figures.clone(),
            source_kind15_combat_figures: self.source_kind15_combat_figures.clone(),
            source_kind6_deferred_hits: self.source_kind6_deferred_hits.clone(),
            source_kind4_deferred_hits: self.source_kind4_deferred_hits.clone(),
            source_kind13_deferred_hits: self.source_kind13_deferred_hits.clone(),
            source_kind4_deferred_relocations: self.source_kind4_deferred_relocations.clone(),
            source_kind6_terminal_events: self.source_kind6_terminal_events.clone(),
            source_combat_terminal_events: self.source_combat_terminal_events.clone(),
            source_combat_terminal_slices: self.source_combat_terminal_slices.clone(),
            tile_clears: self.tile_clears.clone(),
            source_kind4_dispatch: self.source_kind4_dispatch,
            source_shared_figure_control_flags: self.source_shared_figure_control_flags,
            source_player_controllers: self.source_player_controllers.clone(),
            source_player_controller_cursor: self.source_player_controller_cursor,
            source_controller_difficulty_mode: self.source_controller_difficulty_mode,
            source_time_ticks: self.source_time_ticks,
            source_time_remainder_ms: self.source_time_remainder_ms,
            source_resource_environment_elapsed_ms: self.source_resource_environment_elapsed_ms,
            source_resource_environment_phase: self.source_resource_environment_phase,
            source_resource_environment_cursor: self.source_resource_environment_cursor,
            source_resource_environment_last_phase: self
                .source_resource_environment_last_phase
                .clone(),
            source_terrain_event_schedules: self.source_terrain_event_schedules.clone(),
            source_terrain_event_schedule_counters: self
                .source_terrain_event_schedule_counters
                .clone(),
            source_map_dispatch_elapsed_ms: self.source_map_dispatch_elapsed_ms,
            source_map_dispatch_phase: self.source_map_dispatch_phase,
            source_city_dispatch_elapsed_ms: self.source_city_dispatch_elapsed_ms,
            source_city_dispatch_phase: self.source_city_dispatch_phase,
            source_city_dispatch_cursor: self.source_city_dispatch_cursor,
            source_rng_state: self.rng_state,
            warehouses: self.warehouses.clone(),
            figures: self.figures.clone(),
            military_units: self.military_units.clone(),
            diplomacy: self.diplomacy.clone(),
            trade_routes: self.trade_routes.clone(),
            trade_ships: self.trade_ships.clone(),
            objectives: self.objectives.clone(),
            source_growth_bucket_elapsed_ms: self.source_growth_bucket_elapsed_ms,
            source_growth_bucket_phase: self.source_growth_bucket_phase,
        }
    }

    /// Deterministic digest of the current mutable game state. Two sims
    /// seeded identically and driven with the same fixed timestep and
    /// command stream produce the same hash on every tick; the first
    /// differing tick localizes a divergence.
    pub fn state_hash(&self) -> u64 {
        state_hash(&self.snapshot())
    }

    /// Apply a previously captured snapshot. Preserves immutable scenario
    /// data (building_defs, island_maps, ocean_map). Coverage maps are
    /// reset and will be recomputed by the next tick; subsystem timer
    /// accumulators are restored, so the tick phase survives the roundtrip.
    pub fn apply_snapshot(&mut self, s: SaveState) {
        self.game_clock = s.game_clock;
        self.clock_frac_ms = s.clock_frac_ms;
        self.speed_multiplier = s.speed_multiplier;
        self.paused = s.paused;
        self.autosave_timer_ms = s.autosave_timer_ms;
        self.timer_production.accumulator_ms = s.timer_production_accumulator_ms;
        self.timer_population.accumulator_ms = s.timer_population_accumulator_ms;
        self.timer_diplomacy.accumulator_ms = s.timer_diplomacy_accumulator_ms;
        self.timer_market.accumulator_ms = s.timer_market_accumulator_ms;
        self.timer_ships.accumulator_ms = s.timer_ships_accumulator_ms;
        self.timer_events.accumulator_ms = s.timer_events_accumulator_ms;
        self.players = s.players;
        self.buildings = s.buildings;
        self.source_dynamic_map_objects = s.source_dynamic_map_objects;
        self.source_map_cell_states = s.source_map_cell_states;
        self.source_static_map_roots = s.source_static_map_roots;
        self.source_static_map_backing_cells = s.source_static_map_backing_cells;
        self.source_kind13_locations = s.source_kind13_locations;
        self.source_kind13_dispatch = s.source_kind13_dispatch;
        self.source_afflictions = s.source_afflictions;
        self.source_deferred_map_demolitions = s.source_deferred_map_demolitions;
        self.source_kind13_replacement_commands = s.source_kind13_replacement_commands;
        self.source_cities = s.source_cities;
        self.source_figure_events = s.source_figure_events;
        self.source_kind4_occupants = s.source_kind4_occupants;
        self.source_dynamic_combat_figures = s.source_dynamic_combat_figures;
        self.source_dynamic_route_programs = s.source_dynamic_route_programs;
        self.source_kind6_actions = s.source_kind6_actions;
        self.source_kind4_actions = s.source_kind4_actions;
        self.source_kind13_actions = s.source_kind13_actions;
        self.source_kind14_combat_figures = s.source_kind14_combat_figures;
        self.source_kind15_combat_figures = s.source_kind15_combat_figures;
        self.source_kind6_deferred_hits = s.source_kind6_deferred_hits;
        self.source_kind4_deferred_hits = s.source_kind4_deferred_hits;
        self.source_kind13_deferred_hits = s.source_kind13_deferred_hits;
        self.source_kind4_deferred_relocations = s.source_kind4_deferred_relocations;
        self.source_kind6_terminal_events = s.source_kind6_terminal_events;
        self.source_combat_terminal_events = s.source_combat_terminal_events;
        self.source_combat_terminal_slices = s.source_combat_terminal_slices;
        self.tile_clears = s.tile_clears;
        self.source_kind4_dispatch = s.source_kind4_dispatch;
        self.source_shared_figure_control_flags = s.source_shared_figure_control_flags;
        self.source_player_controllers = s.source_player_controllers;
        self.source_player_controller_cursor = s.source_player_controller_cursor;
        self.source_controller_difficulty_mode = s.source_controller_difficulty_mode;
        self.source_time_ticks = s.source_time_ticks;
        self.source_time_remainder_ms = s.source_time_remainder_ms;
        self.source_resource_environment_elapsed_ms = s.source_resource_environment_elapsed_ms;
        self.source_resource_environment_phase = s.source_resource_environment_phase;
        self.source_resource_environment_cursor = s.source_resource_environment_cursor;
        self.source_resource_environment_last_phase = s.source_resource_environment_last_phase;
        self.source_terrain_event_schedules = s.source_terrain_event_schedules;
        self.source_terrain_event_schedule_counters = s.source_terrain_event_schedule_counters;
        self.source_map_dispatch_elapsed_ms = s.source_map_dispatch_elapsed_ms;
        self.source_map_dispatch_phase = s.source_map_dispatch_phase;
        self.source_city_dispatch_elapsed_ms = s.source_city_dispatch_elapsed_ms;
        self.source_city_dispatch_phase = s.source_city_dispatch_phase;
        self.source_city_dispatch_cursor = s.source_city_dispatch_cursor;
        self.rng_state = s.source_rng_state;
        self.warehouses = s.warehouses;
        self.figures = s.figures;
        self.military_units = s.military_units;
        self.diplomacy = s.diplomacy;
        self.trade_routes = s.trade_routes;
        self.trade_ships = s.trade_ships;
        self.objectives = s.objectives;
        self.source_growth_bucket_elapsed_ms = s.source_growth_bucket_elapsed_ms;
        self.source_growth_bucket_phase = s.source_growth_bucket_phase;
    }
}

/// FNV-1a 64-bit hash of the bincode encoding of a save snapshot.
///
/// Stable across processes and platforms (bincode encodes fixed-width
/// little-endian integers), so two simulations driven with the same seed,
/// timestep, and commands can be compared tick by tick by hash alone.
pub fn state_hash(state: &SaveState) -> u64 {
    let payload = bincode::serialize(state).expect("SaveState must serialize");
    fnv1a_64(&payload)
}

const FNV1A_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = FNV1A_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV1A_PRIME);
    }
    hash
}

pub fn save_to_file(path: &Path, state: &SaveState) -> Result<(), SaveError> {
    let payload = bincode::serialize(state).map_err(|e| SaveError::Encode(e.to_string()))?;
    let mut buf = Vec::with_capacity(SAVE_MAGIC.len() + payload.len());
    buf.extend_from_slice(&SAVE_MAGIC);
    buf.extend_from_slice(&payload);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &buf)?;
    Ok(())
}

pub fn load_from_file(path: &Path) -> Result<SaveState, SaveError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < SAVE_MAGIC.len() || bytes[..SAVE_MAGIC.len()] != SAVE_MAGIC {
        return Err(SaveError::BadMagic);
    }
    // `bincode::serialize` uses fixed-width little-endian integers, and
    // `SaveState::version` is its first field. Reject incompatible layouts
    // before deserializing fields that may have shifted since that version.
    let version_offset = SAVE_MAGIC.len();
    let version_end = version_offset + std::mem::size_of::<u32>();
    if bytes.len() < version_end {
        return Err(SaveError::Decode(
            "save payload is missing its version".into(),
        ));
    }
    let found_version = u32::from_le_bytes([
        bytes[version_offset],
        bytes[version_offset + 1],
        bytes[version_offset + 2],
        bytes[version_offset + 3],
    ]);
    if found_version > SAVE_VERSION || found_version < MIN_LOADABLE_VERSION {
        return Err(SaveError::VersionMismatch {
            found: found_version,
            expected: SAVE_VERSION,
        });
    }
    let state: SaveState = bincode::deserialize(&bytes[SAVE_MAGIC.len()..])
        .map_err(|e| SaveError::Decode(e.to_string()))?;
    if state.version != found_version {
        return Err(SaveError::VersionMismatch {
            found: state.version,
            expected: SAVE_VERSION,
        });
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn load_from_file_accepts_intermediate_versions() {
        // Build a save payload with version = MIN_LOADABLE_VERSION,
        // verify the loader accepts it instead of hard-rejecting.
        let tmp = std::env::temp_dir().join("anno_save_intermediate.bin");
        let mut state = SaveState {
            version: MIN_LOADABLE_VERSION,
            game_clock: 0,
            clock_frac_ms: 0,
            speed_multiplier: 1,
            paused: false,
            autosave_timer_ms: 0,
            timer_production_accumulator_ms: 0,
            timer_population_accumulator_ms: 0,
            timer_diplomacy_accumulator_ms: 0,
            timer_market_accumulator_ms: 0,
            timer_ships_accumulator_ms: 0,
            timer_events_accumulator_ms: 0,
            players: vec![Player::new_human(0)],
            buildings: vec![],
            source_dynamic_map_objects: vec![],
            source_map_cell_states: vec![],
            source_static_map_roots: vec![],
            source_static_map_backing_cells: vec![],
            source_kind13_locations: SourceKind13LocationTable::default(),
            source_kind13_dispatch: SourceKind13DispatchState::default(),
            source_afflictions: crate::data_bridge::SourceAfflictionTable::default(),
            source_deferred_map_demolitions: vec![],
            source_kind13_replacement_commands: vec![],
            source_cities: SourceCityTable::default(),
            source_figure_events: crate::source_figure_event::SourceFigureEventRegistry::default(),
            source_kind4_occupants: vec![],
            source_dynamic_combat_figures: vec![],
            source_dynamic_route_programs: vec![],
            source_kind6_actions: vec![],
            source_kind4_actions: vec![],
            source_kind13_actions: vec![],
            source_kind14_combat_figures: vec![],
            source_kind15_combat_figures: vec![],
            source_kind6_deferred_hits: vec![],
            source_kind4_deferred_hits: vec![],
            source_kind13_deferred_hits: vec![],
            source_kind4_deferred_relocations: vec![],
            source_kind6_terminal_events: vec![],
            source_combat_terminal_events: vec![],
            source_combat_terminal_slices: vec![],
            tile_clears: vec![],
            source_kind4_dispatch: crate::combat::SourceKind4DispatchState::default(),
            source_shared_figure_control_flags: [0;
                crate::combat::SOURCE_DYNAMIC_SHARED_SLOT_CAPACITY as usize],
            source_player_controllers: std::array::from_fn(|_| {
                crate::simulation::SourcePlayerController::default()
            }),
            source_player_controller_cursor: 0,
            source_controller_difficulty_mode: 2,
            source_time_ticks: 0,
            source_time_remainder_ms: 0,
            source_resource_environment_elapsed_ms: 0,
            source_resource_environment_phase: 0,
            source_resource_environment_cursor: 0,
            source_resource_environment_last_phase: vec![],
            source_terrain_event_schedules: vec![],
            source_terrain_event_schedule_counters: vec![],
            source_map_dispatch_elapsed_ms: 0,
            source_map_dispatch_phase: 0,
            source_city_dispatch_elapsed_ms: 0,
            source_city_dispatch_phase: 0,
            source_city_dispatch_cursor: 0,
            source_rng_state: crate::source_rand::SourceRand::default(),
            warehouses: vec![],
            figures: vec![],
            military_units: vec![],
            diplomacy: crate::combat::DiplomacyMatrix::new(),
            trade_routes: vec![],
            trade_ships: vec![],
            objectives: Default::default(),
            source_growth_bucket_elapsed_ms: [0; SOURCE_GROWTH_BUCKET_COUNT],
            source_growth_bucket_phase: [0; SOURCE_GROWTH_BUCKET_COUNT],
        };
        let payload = bincode::serialize(&state).unwrap();
        let mut buf = Vec::with_capacity(SAVE_MAGIC.len() + payload.len());
        buf.extend_from_slice(&SAVE_MAGIC);
        buf.extend_from_slice(&payload);
        std::fs::write(&tmp, &buf).unwrap();
        let loaded = load_from_file(&tmp).expect("intermediate version should load");
        assert_eq!(loaded.version, MIN_LOADABLE_VERSION);

        // A version below MIN_LOADABLE_VERSION must hard-reject.
        state.version = MIN_LOADABLE_VERSION - 1;
        let payload = bincode::serialize(&state).unwrap();
        let mut buf = Vec::with_capacity(SAVE_MAGIC.len() + payload.len());
        buf.extend_from_slice(&SAVE_MAGIC);
        buf.extend_from_slice(&payload);
        std::fs::write(&tmp, &buf).unwrap();
        match load_from_file(&tmp) {
            Err(SaveError::VersionMismatch { .. }) => {}
            other => panic!("expected VersionMismatch, got {other:?}"),
        }

        let mut legacy_layout = Vec::from(SAVE_MAGIC);
        legacy_layout.extend_from_slice(&(MIN_LOADABLE_VERSION - 1).to_le_bytes());
        legacy_layout.push(0);
        std::fs::write(&tmp, legacy_layout).unwrap();
        match load_from_file(&tmp) {
            Err(SaveError::VersionMismatch { .. }) => {}
            other => panic!("expected pre-decode VersionMismatch, got {other:?}"),
        }
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn round_trip_minimal_simulation() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players[0].gold = 12345;
        sim.players[0].population[0] = 100;
        sim.players[0].population[1] = 42;
        sim.players[0].satisfaction[0] = 96;
        sim.game_clock = 7777;
        sim.paused = true;
        sim.diplomacy.set_source_relationship_code(0, 1, 1);
        sim.diplomacy.set_source_relationship_code(1, 0, 3);
        sim.diplomacy
            .set_source_diplomacy_score_inputs(0, 1, 72, 0x1234, 0x5678);
        sim.source_time_ticks = 321;
        assert!(
            sim.apply_command(&crate::commands::Command::ApplySourceAttitudeEvent {
                source: 0,
                target: 1,
                payload: 1,
            })
        );
        sim.seed_source_rand(1);
        let _ = sim.next_source_rand();

        let snap = sim.snapshot();
        let expected_next_rand = sim.next_source_rand();
        let bytes = bincode::serialize(&snap).unwrap();
        let restored: SaveState = bincode::deserialize(&bytes).unwrap();

        let mut sim2 = Simulation::new();
        sim2.apply_snapshot(restored);
        assert_eq!(sim2.players.len(), 1);
        assert_eq!(sim2.players[0].gold, 12345);
        assert_eq!(sim2.players[0].population[0], 100);
        assert_eq!(sim2.players[0].population[1], 42);
        assert_eq!(sim2.players[0].satisfaction[0], 96);
        assert_eq!(sim2.game_clock, 7777);
        assert!(sim2.paused);
        assert_eq!(sim2.diplomacy.source_relationship_code(0, 1), 1);
        assert_eq!(sim2.diplomacy.source_relationship_code(1, 0), 3);
        assert_eq!(sim2.diplomacy.source_attitude_code(0, 1), 1);
        assert_eq!(sim2.diplomacy.source_attitude_code(1, 0), 2);
        assert_eq!(sim2.diplomacy.source_diplomacy_activity(0, 1), 72);
        assert_eq!(
            sim2.diplomacy.source_diplomacy_pair_weights(0, 1),
            (0x1234, 0x5678)
        );
        assert_eq!(
            sim2.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].event_type,
            2
        );
        assert_eq!(
            sim2.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].peer,
            0
        );
        assert_eq!(
            sim2.diplomacy.source_diplomacy_event_queue(1).unwrap()[0].timestamp,
            321
        );
        assert_eq!(sim2.next_source_rand(), expected_next_rand);
    }

    #[test]
    fn save_load_is_phase_exact() {
        // A sim saved mid-interval and restored into a fresh instance must
        // fire every subsystem on the same future tick as an uninterrupted
        // run: the timer accumulators and clock_frac_ms must survive the
        // roundtrip, not reset to zero.
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.seed_source_rand(42);
        // 7 × 130 ms leaves every accumulator mid-interval (910 ms elapsed).
        for _ in 0..7 {
            sim.tick(130);
        }

        let snap = sim.snapshot();
        assert_eq!(snap.clock_frac_ms, sim.clock_frac_ms);
        assert_ne!(
            snap.timer_production_accumulator_ms, 0,
            "test precondition: production timer should be mid-interval"
        );

        let mut restored = Simulation::new();
        restored.apply_snapshot(snap);
        assert_eq!(
            restored.timer_production.accumulator_ms,
            sim.timer_production.accumulator_ms
        );
        assert_eq!(
            restored.timer_population.accumulator_ms,
            sim.timer_population.accumulator_ms
        );
        assert_eq!(
            restored.timer_diplomacy.accumulator_ms,
            sim.timer_diplomacy.accumulator_ms
        );
        assert_eq!(
            restored.timer_market.accumulator_ms,
            sim.timer_market.accumulator_ms
        );
        assert_eq!(
            restored.timer_ships.accumulator_ms,
            sim.timer_ships.accumulator_ms
        );
        assert_eq!(
            restored.timer_events.accumulator_ms,
            sim.timer_events.accumulator_ms
        );
        assert_eq!(restored.clock_frac_ms, sim.clock_frac_ms);

        // Continue both sims in lockstep; their states must stay identical.
        for _ in 0..40 {
            sim.tick(130);
            restored.tick(130);
        }
        assert_eq!(sim.game_clock, restored.game_clock);
        assert_eq!(
            bincode::serialize(&sim.snapshot()).unwrap(),
            bincode::serialize(&restored.snapshot()).unwrap()
        );
    }

    #[test]
    fn state_hash_matches_lockstep_twins_and_flags_divergence() {
        let build = || {
            let mut sim = Simulation::new();
            sim.players.push(Player::new_human(0));
            sim.players[0].gold = 500;
            sim.seed_source_rand(1234);
            sim
        };
        let mut a = build();
        let mut b = build();
        for _ in 0..50 {
            a.tick(130);
            b.tick(130);
            assert_eq!(a.state_hash(), b.state_hash());
        }

        // A single extra RNG draw must change the digest.
        let before = a.state_hash();
        let _ = a.next_source_rand();
        assert_ne!(a.state_hash(), before);
    }

    #[test]
    fn round_trip_with_buildings_and_warehouses() {
        use crate::building::BuildingInstance;
        use crate::types::Good;
        use crate::warehouse::Warehouse;

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.buildings.push(BuildingInstance::new(7, 1, 10, 20, 0));
        sim.buildings[0].output_stock = 12;
        sim.buildings[0].source_dynamic_object_slot = Some(3);
        sim.buildings[0].source_placement_command = Some(crate::building::SourceBuildingCommand {
            definition_offset: 21,
            orientation: 3,
            variant: 8,
            metadata: 1,
            map_owner_slot: 6,
            random_seed: 17,
            dynamic_object_owner: 4,
        });
        sim.source_dynamic_map_objects.push(SourceDynamicMapObject {
            island: 1,
            slot: 0,
            owner: 4,
            local_position: (7, 8),
        });
        sim.source_map_cell_states
            .push(crate::source_cell::SourceMapCellState {
                island: 1,
                x: 10,
                y: 20,
                source_definition_offset: 21,
                source_command_anchor_x: 10,
                source_command_anchor_y: 20,
                footprint_width: 2,
                footprint_height: 3,
                source_definition_width: 2,
                source_definition_height: 3,
                source_orientation: 0,
                source_variant: 8,
                source_map_owner_slot: 6,
                source_transfer_figure_limit: 2,
                source_operating_cost: 0,
                source_transfer_radius: 16,
                source_transfer_figure: crate::source_cell::SourceTransferFigure::Karren,
                ruin_id: 4,
                ruin_footprint_width: 2,
                ruin_footprint_height: 3,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                phase: 3,
                scheduler_enabled: false,
                scheduler_cooldown: 2,
                scheduler_blocked: true,
                frame_selector: 12,
                activity: 96,
                work_material_stock: 64,
                raw_material_stock: 128,
                storage_fill: 128,
                reserved_storage: 32,
                storage_animation_capacity: 160,
                source_production_amount: 32,
                source_raw_material_amount: 64,
                source_work_material_amount: 16,
                source_max_no_raw_material_count: 9,
                source_scheduler_interval: 7,
                source_no_raw_material_count: 6,
                source_output_ware_slot: 0x16,
                source_raw_resource_ware_slot: 0x2d,
                source_work_material_ware_slot: 0,
                source_growth_resource_ware_slot: 0,
                source_resource_is_dry: false,
                source_plantation_worker_definition: 0x60,
                source_resource_reserved: true,
                source_path_class: 46,
                source_resource_growth_factor: 96,
                source_damage_threshold: 640,
                source_damage_accumulator: 192,
                progress: 512,
                source_production_time: 384,
                animation_frame: 2,
                animation_count: 4,
                animation_continues: true,
                kind_code: 7,
                source_production_kind_code: 7,
                source_definition_record: 51,
                source_resource_records: None,
                source_growth_enrolled: false,
                source_growth_bucket: 0,
                source_growth_phase_seen: 0,
                source_growth_phase: 0,
                source_placement_variant: 17,
                source_destroy_flag: true,
                source_wood_cost_fixed: 320,
                source_bricks_cost_fixed: 96,
                source_path_class_loaded_road: 32,
            });
        let mut static_root = sim.source_map_cell_states[0];
        static_root.x = 14;
        static_root.y = 16;
        static_root.source_command_anchor_x = 14;
        static_root.source_command_anchor_y = 16;
        static_root.source_variant = 3;
        static_root.source_map_owner_slot = 5;
        static_root.fallback_strand_cells = 0b10_101;
        static_root.source_damage_accumulator = 511;
        static_root.kind_code = 35;
        sim.source_static_map_roots.push(static_root);
        let mut backing_cell = static_root;
        backing_cell.x = 12;
        backing_cell.y = 18;
        backing_cell.source_command_anchor_x = 12;
        backing_cell.source_command_anchor_y = 18;
        sim.source_static_map_backing_cells.push(backing_cell);
        assert!(sim
            .source_kind13_locations
            .insert(crate::data_bridge::SourceKind13Location {
                island_id: 1,
                tile_x: 9,
                tile_y: 11,
                orientation: 3,
                variant: 9,
                source_owner: 4,
                phase: 5,
                state_bits: 0xa0,
                population_group: 2,
                amount: 192,
                lifecycle_flags: 0x14,
            }));
        assert!(sim.source_cities.set_record(
            3,
            Some(crate::data_bridge::SourceCityRecord {
                island_id: 1,
                source_owner: 4,
                owner_slot: 4,
                phase: 6,
                tier_population: [10, 20, 30, 40, 50],
                resident_amount: 77,
                ..Default::default()
            }),
        ));
        assert_eq!(
            sim.source_kind13_dispatch
                .advance(&mut sim.source_kind13_locations, 15_640),
            0
        );
        sim.source_city_dispatch_elapsed_ms = 9_800;
        sim.source_city_dispatch_phase = 6;
        sim.source_city_dispatch_cursor = 17;
        sim.source_time_ticks = 19;
        sim.source_time_remainder_ms = 99;
        sim.source_resource_environment_elapsed_ms = 17_000;
        sim.source_resource_environment_phase = 3;
        sim.source_resource_environment_cursor = 1;
        sim.source_resource_environment_last_phase = vec![2, 3];
        sim.source_terrain_event_schedules = vec![[
            crate::simulation::SourceTerrainEventSchedule {
                island_id: 3,
                x: 7,
                y: 9,
                due_at_ticks: 619,
            },
            crate::simulation::SourceTerrainEventSchedule::default(),
            crate::simulation::SourceTerrainEventSchedule::default(),
            crate::simulation::SourceTerrainEventSchedule::default(),
            crate::simulation::SourceTerrainEventSchedule::default(),
            crate::simulation::SourceTerrainEventSchedule::default(),
            crate::simulation::SourceTerrainEventSchedule::default(),
            crate::simulation::SourceTerrainEventSchedule::default(),
        ]];
        sim.source_terrain_event_schedule_counters = vec![3, 1];
        sim.source_map_dispatch_elapsed_ms = 800;
        sim.source_map_dispatch_phase = 5;
        sim.source_kind4_dispatch = crate::combat::SourceKind4DispatchState {
            active_player_slot: 2,
            single_player: false,
            remote_owner_dispatch_enabled: true,
            faction_states: [0x0c, 0x0c, 0, 0x0c, 0x0d, 0x0e, 0x0b],
        };
        sim.source_shared_figure_control_flags[9] = 0x40;
        sim.source_player_controller_cursor = 4;
        sim.source_controller_difficulty_mode = 3;
        sim.source_player_controllers[2] = crate::simulation::SourcePlayerController {
            initialized: true,
            initialized_at_ticks: 0,
            action_timer_ms: 47,
            city_management_timer_ms: 9_999,
            maintenance_timer_ms: 2_998,
            desired_figure_count: 3,
            figure_roster_ratio: 1,
            figure_capacity: 4,
            figure_capacity_limit: Some(12),
            city_management_profile: Some(crate::simulation::SourceCityManagementProfile {
                city_slot: 3,
                initialized_at_ticks: 0,
            }),
            action_figure_handle: Some(4),
            action_target_island_id: Some(1),
            action_target_tile: Some((12, 13)),
            action_source_candidate_tile: Some((21, -6)),
            action_target_direction: Some(2),
            action_arrival_retries: 3,
            island_search_cursor: 17,
            island_search_requirement: Some(0x2d),
            island_search_selected_island_id: Some(3),
            island_search_area_threshold: 1_450,
            island_search_minimum_area: 300,
            island_search_deferred_requirement: Some(0x31),
            island_search_retry_at_ticks: Some(6_000),
            source_city_construction_cursor:
                crate::simulation::SourceControllerCityConstructionCursor {
                    work_index: 3,
                    scan_x: -4,
                    scan_y: 7,
                    remaining: 5,
                    baseline_x: -4,
                    baseline_y: 7,
                },
            active_city_owner: Some(2),
            active_city_slot: Some(3),
            selected_city_active: false,
            city_management_disabled: false,
            action_stack: vec![8, 9],
            action_budget: 14,
            purchase_predecessor_issued: false,
            owned_figure_handles: vec![4, 7],
            figure_roster_dirty: true,
            source_city_rectangles: Vec::new(),
            source_city_construction_queue: Default::default(),
        };
        let mut source_route_program = crate::combat::default_source_kind4_route_program();
        source_route_program[0] = 0x31;
        source_route_program[1] = 0x42;
        source_route_program[2] = crate::combat::SOURCE_KIND4_ROUTE_PROGRAM_TERMINATOR;
        sim.source_kind4_occupants
            .push(crate::data_bridge::SourceKind4Occupant {
                runtime_slot: 0,
                figure_definition_id: 0,
                route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
                route_retry_count: 3,
                route_program: source_route_program,
                route_program_cursor: 1,
                idle_remaining_bits: 1.25_f32.to_bits(),
                origin_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x33, 1, 2, 3,
                ]),
                position: (0, 0),
                island_id: 1,
                owner: 4,
                direction: 0,
                animation_state: 0,
                state_selector: 1,
                state_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x38, 0, 16, 32,
                ]),
                idle_timestamp_ticks: 0,
                state_flags: 0,
                state_payload: [0; 8],
                active: true,
            });
        sim.source_dynamic_combat_figures
            .push(crate::combat::SourceDynamicCombatFigure {
                active: true,
                figure_kind: 6,
                candidate_list_key: 1,
                figure_definition_id: 31,
                direction: 7,
                source_payload: 0x1234_5678,
                position: (18.5, 21.25),
                position_z: 0.0,
                source_energy: 320,
                source_score_state: 0,
                source_action_ready_at: 53,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x37, 0, 9, 10,
                ]),
                state_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x38, 0, 18, 21,
                ]),
                owner: 4,
                state: 7,
                flags: 1,
                notification: 0,
                runtime_slot: 12,
                auxiliary_kind: 2,
                name_index: 6,
                source_motion: crate::combat::SourceGenericMotion::stationary_from_loader_flags(1),
            });
        sim.source_kind6_actions
            .push(crate::combat::SourceKind6Action {
                attacker_position: (18.5, 21.25),
                attacker_runtime_slot: 12,
                raw_strength: 6,
                attacker_figure_kind: 6,
                direction: 7,
                flags: crate::combat::SOURCE_KIND6_ACTION_EVENT_FLAGS,
                target_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    6, 1, 9, 0,
                ]),
                kind15_figure_definition_id: Some(112),
            });
        sim.source_kind4_actions
            .push(crate::combat::SourceKind4Action {
                attacker_position: (12.25, 8.25),
                attacker_runtime_slot: 4,
                raw_strength: 9,
                attacker_figure_kind: 4,
                direction: 2,
                flags: crate::combat::SOURCE_KIND4_ACTION_EVENT_FLAGS,
                target_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    1, 1, 9, 0,
                ]),
            });
        sim.source_kind15_combat_figures
            .push(crate::combat::SourceKind15CombatFigure {
                active: true,
                figure_definition_id: 112,
                position: (18.5, 20.75, 4.0),
                direction: 0,
                launcher_runtime_slot: 12,
                source_step_amount: crate::combat::SOURCE_KIND15_STEP_AMOUNT,
                remaining_work_time: 0.96,
                source_flags: crate::combat::SOURCE_KIND15_EXECUTOR_FLAGS,
            });
        sim.source_kind6_deferred_hits
            .push(crate::combat::SourceKind6DeferredHit {
                due_at: 63,
                action: sim.source_kind6_actions[0],
            });
        sim.source_kind4_deferred_hits
            .push(crate::combat::SourceKind4DeferredHit {
                due_at: 67,
                action: sim.source_kind4_actions[0],
            });
        sim.source_kind4_deferred_relocations.push(
            crate::simulation::SourceKind4DeferredRelocation {
                due_at: 100,
                island_id: 1,
                figure_definition_id: 0x1f,
                origin: (18.5, 21.25),
                target_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x38, 0, 40, 42,
                ]),
            },
        );
        sim.source_kind6_terminal_events
            .push(crate::simulation::SourceKind6TerminalEvent {
                target: crate::source_route::SourceTargetDescriptor::from_bytes([0x34, 1, 10, 20]),
                event_kind: 7,
            });
        sim.source_combat_terminal_events
            .push(crate::simulation::SourceCombatTerminalEvent {
                target: crate::source_route::SourceTargetDescriptor::from_bytes([1, 7, 3, 0]),
                target_figure_kind: 1,
                target_runtime_slot: 3,
                target_owner: 1,
                attacker_figure_kind: 4,
                attacker_runtime_slot: 0,
                attacker_owner: Some(0),
                control_kind: crate::simulation::SOURCE_COMBAT_TERMINAL_CONTROL_KIND,
                kill_credit: true,
            });
        sim.source_combat_terminal_slices
            .push(crate::simulation::SourceCombatTerminalSlice {
                target: crate::simulation::SourceCombatTerminalSliceTarget::TradeShip(0),
                target_figure_kind: 1,
                target_runtime_slot: 12,
                remaining_distance: 0.015,
                scalar_speed: crate::combat::SOURCE_TERMINAL_STATIONARY_SPEED,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
            });
        sim.tile_clears.push(crate::simulation::TileClear {
            island_id: 1,
            tile_x: 10,
            tile_y: 20,
            width: 2,
            height: 3,
            source_orientation: 0,
            source_variant: 8,
            source_map_owner_slot: 6,
            ruin_id: 4,
            ruin_uses_strand_table: false,
            fallback_strand_cells: 0,
            source_ruin_draws: vec![12],
        });
        let mut source_spearman =
            crate::combat::MilitaryUnit::new(crate::combat::UnitType::NativeSpearman, 6, 12, 14);
        source_spearman.source_island_id = Some(1);
        source_spearman.source_runtime_slot = Some(0);
        source_spearman.source_figure_definition_id = Some(33);
        source_spearman.source_origin_descriptor =
            Some(crate::source_route::SourceTargetDescriptor::from_bytes([
                0x33, 1, 2, 3,
            ]));
        source_spearman.source_target_descriptor =
            Some(crate::source_route::SourceTargetDescriptor::from_bytes([
                0x38, 0, 16, 32,
            ]));
        source_spearman.source_route_retry_count = 3;
        source_spearman.source_route_program = source_route_program;
        source_spearman.source_route_program_cursor = 1;
        source_spearman.source_step_remaining = 0.375;
        source_spearman.source_terminal_pending = true;
        source_spearman.source_terminal_remaining = 0.125;
        source_spearman.source_idle_remaining = 1.25;
        source_spearman.source_motion_target = Some((14, 16));
        source_spearman.source_position_x = 6.25;
        source_spearman.source_position_y = 7.75;
        source_spearman.source_position_initialized = true;
        sim.military_units.push(source_spearman);
        let mut wh = Warehouse::new(1, 0, 5, 5);
        wh.city_population = [10, 20, 30, 40, 50];
        wh.set_source_path_class(55);
        wh.set_capacity(Good::Wood, 100);
        wh.deposit(Good::Wood, 50);
        wh.deposit(Good::Tools, 7);
        assert_eq!(wh.deposit_city_good_fixed(Good::Cloth, 65, 1_600), 65);
        assert!(wh.reserve(Good::Wood, 4));
        sim.warehouses.push(wh);
        let mut carrier = crate::entity::Figure::new();
        carrier.action = crate::entity::ActionType::CarryingGoods;
        carrier.target_x = 5;
        carrier.target_y = 5;
        carrier.destination_kind = 8;
        carrier.supplier_x = 6;
        carrier.supplier_y = 7;
        carrier.cargo_route = crate::entity::CargoRoute::CityCart;
        carrier.origin_island = 1;
        carrier.origin_x = 10;
        carrier.origin_y = 20;
        carrier.origin_kind = 7;
        carrier.origin_source_map_owner_slot = 3;
        carrier.origin_production_kind = 7;
        carrier.carried_good = Good::Wood as u8;
        carrier.carried_amount = 7;
        carrier.cargo_fixed = 231;
        carrier.source_transfer_max_load_fixed = 320;
        carrier.source_move_speed = 300;
        carrier.source_step_remaining = 0.75;
        carrier.source_position_x = 10.25;
        carrier.source_position_y = 20.5;
        carrier.source_position_z = 0.56;
        carrier.source_position_initialized = true;
        carrier.source_animation_elapsed_ms = 75;
        sim.figures.push(carrier);

        let snap = sim.snapshot();
        let bytes = bincode::serialize(&snap).unwrap();
        let restored: SaveState = bincode::deserialize(&bytes).unwrap();

        let mut sim2 = Simulation::new();
        sim2.apply_snapshot(restored);
        assert_eq!(sim2.buildings.len(), 1);
        assert_eq!(sim2.buildings[0].def_id, 7);
        assert_eq!(sim2.buildings[0].output_stock, 12);
        assert_eq!(sim2.buildings[0].source_dynamic_object_slot, Some(3));
        assert_eq!(
            sim2.buildings[0].source_placement_command,
            Some(crate::building::SourceBuildingCommand {
                definition_offset: 21,
                orientation: 3,
                variant: 8,
                metadata: 1,
                map_owner_slot: 6,
                random_seed: 17,
                dynamic_object_owner: 4,
            })
        );
        assert_eq!(sim2.source_dynamic_map_objects.len(), 1);
        assert_eq!(sim2.source_dynamic_map_objects[0].owner, 4);
        assert_eq!(
            sim2.source_map_cell_states,
            vec![crate::source_cell::SourceMapCellState {
                island: 1,
                x: 10,
                y: 20,
                source_definition_offset: 21,
                source_command_anchor_x: 10,
                source_command_anchor_y: 20,
                footprint_width: 2,
                footprint_height: 3,
                source_definition_width: 2,
                source_definition_height: 3,
                source_orientation: 0,
                source_variant: 8,
                source_map_owner_slot: 6,
                source_transfer_figure_limit: 2,
                source_operating_cost: 0,
                source_transfer_radius: 16,
                source_transfer_figure: crate::source_cell::SourceTransferFigure::Karren,
                ruin_id: 4,
                ruin_footprint_width: 2,
                ruin_footprint_height: 3,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                phase: 3,
                scheduler_enabled: false,
                scheduler_cooldown: 2,
                scheduler_blocked: true,
                frame_selector: 12,
                activity: 96,
                work_material_stock: 64,
                raw_material_stock: 128,
                storage_fill: 128,
                reserved_storage: 32,
                storage_animation_capacity: 160,
                source_production_amount: 32,
                source_raw_material_amount: 64,
                source_work_material_amount: 16,
                source_max_no_raw_material_count: 9,
                source_scheduler_interval: 7,
                source_no_raw_material_count: 6,
                source_output_ware_slot: 0x16,
                source_raw_resource_ware_slot: 0x2d,
                source_work_material_ware_slot: 0,
                source_growth_resource_ware_slot: 0,
                source_resource_is_dry: false,
                source_plantation_worker_definition: 0x60,
                source_resource_reserved: true,
                source_path_class: 46,
                source_resource_growth_factor: 96,
                source_damage_threshold: 640,
                source_damage_accumulator: 192,
                progress: 512,
                source_production_time: 384,
                animation_frame: 2,
                animation_count: 4,
                animation_continues: true,
                kind_code: 7,
                source_production_kind_code: 7,
                source_definition_record: 51,
                source_resource_records: None,
                source_growth_enrolled: false,
                source_growth_bucket: 0,
                source_growth_phase_seen: 0,
                source_growth_phase: 0,
                source_placement_variant: 17,
                source_destroy_flag: true,
                source_wood_cost_fixed: 320,
                source_bricks_cost_fixed: 96,
                source_path_class_loaded_road: 32,
            }]
        );
        assert_eq!(sim2.source_map_cell_states[0].market_frame_selector(4), 3);
        assert_eq!(sim2.source_static_map_roots.len(), 1);
        assert!(sim2.source_static_map_roots[0].matches(1, 14, 16));
        assert_eq!(sim2.source_static_map_roots[0].kind_code, 35);
        assert_eq!(sim2.source_static_map_roots[0].source_variant, 3);
        assert_eq!(sim2.source_static_map_roots[0].source_map_owner_slot, 5);
        assert_eq!(sim2.source_static_map_roots[0].source_definition_offset, 21);
        assert_eq!(
            sim2.source_static_map_roots[0].fallback_strand_cells,
            0b10_101
        );
        assert_eq!(
            sim2.source_static_map_roots[0].source_damage_accumulator,
            511
        );
        assert_eq!(sim2.source_static_map_backing_cells.len(), 1);
        assert!(sim2.source_static_map_backing_cells[0].matches(1, 12, 18));
        assert_eq!(
            (
                sim2.source_static_map_backing_cells[0].source_command_anchor_x,
                sim2.source_static_map_backing_cells[0].source_command_anchor_y,
            ),
            (12, 18)
        );
        assert_eq!(
            sim2.tile_clears,
            vec![crate::simulation::TileClear {
                island_id: 1,
                tile_x: 10,
                tile_y: 20,
                width: 2,
                height: 3,
                source_orientation: 0,
                source_variant: 8,
                source_map_owner_slot: 6,
                ruin_id: 4,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                source_ruin_draws: vec![12],
            }]
        );
        assert_eq!(
            sim2.source_kind13_locations.active_locations(),
            vec![crate::data_bridge::SourceKind13Location {
                island_id: 1,
                tile_x: 9,
                tile_y: 11,
                orientation: 3,
                variant: 9,
                source_owner: 4,
                phase: 5,
                state_bits: 0xa0,
                population_group: 2,
                amount: 192,
                lifecycle_flags: 0x14,
            }]
        );
        assert_eq!(sim2.source_kind13_dispatch, sim.source_kind13_dispatch);
        assert_eq!(
            sim2.source_cities.record(3),
            Some(crate::data_bridge::SourceCityRecord {
                island_id: 1,
                source_owner: 4,
                owner_slot: 4,
                phase: 6,
                tier_population: [10, 20, 30, 40, 50],
                resident_amount: 77,
                ..Default::default()
            })
        );
        assert_eq!(sim2.source_city_dispatch_elapsed_ms, 9_800);
        assert_eq!(sim2.source_city_dispatch_phase, 6);
        assert_eq!(sim2.source_city_dispatch_cursor, 17);
        assert_eq!(sim2.source_time_ticks, 19);
        assert_eq!(sim2.source_time_remainder_ms, 99);
        assert_eq!(sim2.source_resource_environment_elapsed_ms, 17_000);
        assert_eq!(sim2.source_resource_environment_phase, 3);
        assert_eq!(sim2.source_resource_environment_cursor, 1);
        assert_eq!(sim2.source_resource_environment_last_phase, vec![2, 3]);
        assert_eq!(
            sim2.source_terrain_event_schedules[0][0],
            crate::simulation::SourceTerrainEventSchedule {
                island_id: 3,
                x: 7,
                y: 9,
                due_at_ticks: 619,
            }
        );
        assert_eq!(sim2.source_terrain_event_schedule_counters, vec![3, 1]);
        assert_eq!(sim2.source_map_dispatch_elapsed_ms, 800);
        assert_eq!(sim2.source_map_dispatch_phase, 5);
        assert_eq!(
            sim2.source_dynamic_combat_figures,
            vec![crate::combat::SourceDynamicCombatFigure {
                active: true,
                figure_kind: 6,
                candidate_list_key: 1,
                figure_definition_id: 31,
                direction: 7,
                source_payload: 0x1234_5678,
                position: (18.5, 21.25),
                position_z: 0.0,
                source_energy: 320,
                source_score_state: 0,
                source_action_ready_at: 53,
                source_cargo_slots: [0; crate::combat::SOURCE_SHIP_CARGO_SLOT_COUNT],
                target_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x37, 0, 9, 10,
                ]),
                state_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x38, 0, 18, 21,
                ]),
                owner: 4,
                state: 7,
                flags: 1,
                notification: 0,
                runtime_slot: 12,
                auxiliary_kind: 2,
                name_index: 6,
                source_motion: crate::combat::SourceGenericMotion::stationary_from_loader_flags(1),
            }]
        );
        assert_eq!(
            sim2.source_kind6_actions,
            vec![crate::combat::SourceKind6Action {
                attacker_position: (18.5, 21.25),
                attacker_runtime_slot: 12,
                raw_strength: 6,
                attacker_figure_kind: 6,
                direction: 7,
                flags: crate::combat::SOURCE_KIND6_ACTION_EVENT_FLAGS,
                target_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    6, 1, 9, 0,
                ]),
                kind15_figure_definition_id: Some(112),
            }]
        );
        assert_eq!(
            sim2.source_kind4_actions,
            vec![crate::combat::SourceKind4Action {
                attacker_position: (12.25, 8.25),
                attacker_runtime_slot: 4,
                raw_strength: 9,
                attacker_figure_kind: 4,
                direction: 2,
                flags: crate::combat::SOURCE_KIND4_ACTION_EVENT_FLAGS,
                target_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    1, 1, 9, 0,
                ]),
            }]
        );
        assert_eq!(
            sim2.source_kind15_combat_figures,
            vec![crate::combat::SourceKind15CombatFigure {
                active: true,
                figure_definition_id: 112,
                position: (18.5, 20.75, 4.0),
                direction: 0,
                launcher_runtime_slot: 12,
                source_step_amount: crate::combat::SOURCE_KIND15_STEP_AMOUNT,
                remaining_work_time: 0.96,
                source_flags: crate::combat::SOURCE_KIND15_EXECUTOR_FLAGS,
            }]
        );
        assert_eq!(
            sim2.source_kind6_deferred_hits,
            vec![crate::combat::SourceKind6DeferredHit {
                due_at: 63,
                action: sim2.source_kind6_actions[0],
            }]
        );
        assert_eq!(
            sim2.source_kind4_deferred_hits,
            vec![crate::combat::SourceKind4DeferredHit {
                due_at: 67,
                action: sim2.source_kind4_actions[0],
            }]
        );
        assert_eq!(
            sim2.source_kind4_deferred_relocations,
            vec![crate::simulation::SourceKind4DeferredRelocation {
                due_at: 100,
                island_id: 1,
                figure_definition_id: 0x1f,
                origin: (18.5, 21.25),
                target_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x38, 0, 40, 42,
                ]),
            }]
        );
        assert_eq!(
            sim2.source_kind6_terminal_events,
            vec![crate::simulation::SourceKind6TerminalEvent {
                target: crate::source_route::SourceTargetDescriptor::from_bytes([0x34, 1, 10, 20]),
                event_kind: 7,
            }]
        );
        assert_eq!(
            sim2.source_combat_terminal_events,
            vec![crate::simulation::SourceCombatTerminalEvent {
                target: crate::source_route::SourceTargetDescriptor::from_bytes([1, 7, 3, 0]),
                target_figure_kind: 1,
                target_runtime_slot: 3,
                target_owner: 1,
                attacker_figure_kind: 4,
                attacker_runtime_slot: 0,
                attacker_owner: Some(0),
                control_kind: crate::simulation::SOURCE_COMBAT_TERMINAL_CONTROL_KIND,
                kill_credit: true,
            }]
        );
        assert_eq!(
            sim2.source_combat_terminal_slices,
            vec![crate::simulation::SourceCombatTerminalSlice {
                target: crate::simulation::SourceCombatTerminalSliceTarget::TradeShip(0),
                target_figure_kind: 1,
                target_runtime_slot: 12,
                remaining_distance: 0.015,
                scalar_speed: crate::combat::SOURCE_TERMINAL_STATIONARY_SPEED,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
            }]
        );
        assert_eq!(
            sim2.source_combat_candidates(),
            vec![crate::combat::SourceCombatCandidate {
                entity: crate::combat::SourceCombatCandidateEntity::DynamicFigure(0),
                figure_kind: 6,
                runtime_slot: 12,
                figure_definition_id: Some(31),
                source_energy: 320,
                source_score_state: 0,
                source_kind6_policy_raw_slots: [0; 8],
                source_kind6_policy_ware_slots: [0; 8],
                source_kind6_target_descriptor_payload: None,
                kind6_target_descriptor: Some(
                    crate::source_route::SourceTargetDescriptor::from_bytes([0x37, 0, 9, 10]),
                ),
                candidate_list_key: Some(1),
                owner: 4,
                position: (18.5, 21.25),
                direction: 7,
            }]
        );
        assert_eq!(
            sim2.source_kind4_dispatch,
            crate::combat::SourceKind4DispatchState {
                active_player_slot: 2,
                single_player: false,
                remote_owner_dispatch_enabled: true,
                faction_states: [0x0c, 0x0c, 0, 0x0c, 0x0d, 0x0e, 0x0b],
            }
        );
        assert_eq!(sim2.source_shared_figure_control_flags[9], 0x40);
        assert_eq!(sim2.source_player_controller_cursor, 4);
        assert_eq!(sim2.source_controller_difficulty_mode, 3);
        assert_eq!(
            sim2.source_player_controllers[2],
            crate::simulation::SourcePlayerController {
                initialized: true,
                initialized_at_ticks: 0,
                action_timer_ms: 47,
                city_management_timer_ms: 9_999,
                maintenance_timer_ms: 2_998,
                desired_figure_count: 3,
                figure_roster_ratio: 1,
                figure_capacity: 4,
                figure_capacity_limit: Some(12),
                city_management_profile: Some(crate::simulation::SourceCityManagementProfile {
                    city_slot: 3,
                    initialized_at_ticks: 0,
                }),
                action_figure_handle: Some(4),
                action_target_island_id: Some(1),
                action_target_tile: Some((12, 13)),
                action_source_candidate_tile: Some((21, -6)),
                action_target_direction: Some(2),
                action_arrival_retries: 3,
                island_search_cursor: 17,
                island_search_requirement: Some(0x2d),
                island_search_selected_island_id: Some(3),
                island_search_area_threshold: 1_450,
                island_search_minimum_area: 300,
                island_search_deferred_requirement: Some(0x31),
                island_search_retry_at_ticks: Some(6_000),
                source_city_construction_cursor:
                    crate::simulation::SourceControllerCityConstructionCursor {
                        work_index: 3,
                        scan_x: -4,
                        scan_y: 7,
                        remaining: 5,
                        baseline_x: -4,
                        baseline_y: 7,
                    },
                active_city_owner: Some(2),
                active_city_slot: Some(3),
                selected_city_active: false,
                city_management_disabled: false,
                action_stack: vec![8, 9],
                action_budget: 14,
                purchase_predecessor_issued: false,
                owned_figure_handles: vec![4, 7],
                figure_roster_dirty: true,
                source_city_rectangles: Vec::new(),
                source_city_construction_queue: Default::default(),
            }
        );
        assert_eq!(
            sim2.source_kind4_occupants,
            vec![crate::data_bridge::SourceKind4Occupant {
                runtime_slot: 0,
                figure_definition_id: 0,
                route_radius: crate::combat::SOURCE_KIND4_DEFAULT_ROUTE_RADIUS,
                route_retry_count: 3,
                route_program: source_route_program,
                route_program_cursor: 1,
                idle_remaining_bits: 1.25_f32.to_bits(),
                origin_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x33, 1, 2, 3,
                ]),
                position: (0, 0),
                island_id: 1,
                owner: 4,
                direction: 0,
                animation_state: 0,
                state_selector: 1,
                state_descriptor: crate::source_route::SourceTargetDescriptor::from_bytes([
                    0x38, 0, 16, 32,
                ]),
                idle_timestamp_ticks: 0,
                state_flags: 0,
                state_payload: [0; 8],
                active: true,
            }]
        );
        assert_eq!(sim2.military_units.len(), 1);
        assert_eq!(
            sim2.military_units[0].unit_type,
            crate::combat::UnitType::NativeSpearman
        );
        assert_eq!(sim2.military_units[0].source_island_id, Some(1));
        assert_eq!(sim2.military_units[0].source_runtime_slot, Some(0));
        assert_eq!(sim2.military_units[0].source_figure_definition_id, Some(33));
        assert_eq!(sim2.military_units[0].source_route_retry_count, 3);
        assert_eq!(
            sim2.military_units[0].source_route_program[..3],
            [0x31, 0x42, 0xc1]
        );
        assert_eq!(sim2.military_units[0].source_route_program_cursor, 1);
        assert_eq!(
            sim2.source_kind4_occupants[0].route_program[..3],
            [0x31, 0x42, 0xc1]
        );
        assert_eq!(sim2.source_kind4_occupants[0].route_program_cursor, 1);
        assert_eq!(
            sim2.source_kind4_occupants[0].idle_remaining_bits,
            1.25_f32.to_bits()
        );
        assert_eq!(sim2.military_units[0].source_step_remaining, 0.375);
        assert!(sim2.military_units[0].source_terminal_pending);
        assert_eq!(sim2.military_units[0].source_terminal_remaining, 0.125);
        assert_eq!(sim2.military_units[0].source_idle_remaining, 1.25);
        assert_eq!(sim2.military_units[0].source_motion_target, Some((14, 16)));
        assert_eq!(sim2.military_units[0].source_position_x, 6.25);
        assert_eq!(sim2.military_units[0].source_position_y, 7.75);
        assert!(sim2.military_units[0].source_position_initialized);
        assert_eq!(
            sim2.military_units[0]
                .source_origin_descriptor
                .map(crate::source_route::SourceTargetDescriptor::bytes),
            Some([0x33, 1, 2, 3])
        );
        assert_eq!(
            sim2.military_units[0]
                .source_target_descriptor
                .map(crate::source_route::SourceTargetDescriptor::bytes),
            Some([0x38, 0, 16, 32])
        );
        assert_eq!(
            sim2.source_kind4_occupants[0].state_descriptor.bytes(),
            sim2.military_units[0]
                .source_target_descriptor
                .expect("source unit target descriptor")
                .bytes()
        );
        assert_eq!(sim2.warehouses[0].city_population, [10, 20, 30, 40, 50]);
        assert_eq!(
            sim2.source_dynamic_map_object_table(1).object(0),
            Some(SourceDynamicMapObject {
                island: 1,
                slot: 0,
                owner: 4,
                local_position: (7, 8),
            })
        );
        let island = anno_formats::szs::Island {
            number: 1,
            width: 32,
            height: 32,
            x_pos: 100,
            y_pos: 200,
            fertilities: [7; 8],
            tiles: vec![anno_formats::szs::IslandTile {
                building_id: 3,
                x: 7,
                y: 8,
                orientation: 1,
                anim_count: 0,
                flags: 0,
            }],
            city: None,
        };
        let definitions = [anno_formats::cod::BuildingDef {
            source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
            size: (2, 4),
            ..Default::default()
        }];
        assert_eq!(
            sim2.resolve_source_dynamic_map_object_target(
                crate::source_route::SourceTargetDescriptor::from_bytes([0x35, 1, 0, 0]),
                &[island],
                &definitions,
            ),
            Some(crate::source_route::SourceResolvedDynamicTarget {
                target: crate::source_route::SourcePathTargetRect::new((107, 208), 4, 2).unwrap(),
                owner: 4,
            })
        );
        assert_eq!(sim2.warehouses.len(), 1);
        assert_eq!(sim2.warehouses[0].stock(Good::Wood), 50);
        assert_eq!(sim2.warehouses[0].stock(Good::Tools), 7);
        assert_eq!(sim2.warehouses[0].city_stock_fixed(Good::Cloth), 65);
        assert_eq!(sim2.warehouses[0].reserved(Good::Wood), 4);
        assert_eq!(sim2.warehouses[0].source_path_class, 55);
        assert_eq!(sim2.figures.len(), 1);
        assert_eq!(
            sim2.figures[0].action,
            crate::entity::ActionType::CarryingGoods
        );
        assert_eq!(sim2.figures[0].target_x, 5);
        assert_eq!(sim2.figures[0].target_y, 5);
        assert_eq!(sim2.figures[0].destination_kind, 8);
        assert_eq!(sim2.figures[0].supplier_x, 6);
        assert_eq!(sim2.figures[0].supplier_y, 7);
        assert_eq!(
            sim2.figures[0].cargo_route,
            crate::entity::CargoRoute::CityCart
        );
        assert_eq!(sim2.figures[0].origin_island, 1);
        assert_eq!(sim2.figures[0].origin_x, 10);
        assert_eq!(sim2.figures[0].origin_y, 20);
        assert_eq!(sim2.figures[0].origin_kind, 7);
        assert_eq!(sim2.figures[0].origin_source_map_owner_slot, 3);
        assert_eq!(sim2.figures[0].origin_production_kind, 7);
        assert_eq!(sim2.figures[0].carried_good, Good::Wood as u8);
        assert_eq!(sim2.figures[0].carried_amount, 7);
        assert_eq!(sim2.figures[0].cargo_fixed, 231);
        assert_eq!(sim2.figures[0].source_transfer_max_load_fixed, 320);
        assert_eq!(sim2.figures[0].source_move_speed, 300);
        assert_eq!(sim2.figures[0].source_step_remaining, 0.75);
        assert_eq!(sim2.figures[0].source_position_x, 10.25);
        assert_eq!(sim2.figures[0].source_position_y, 20.5);
        assert_eq!(sim2.figures[0].source_position_z, 0.56);
        assert!(sim2.figures[0].source_position_initialized);
        assert_eq!(sim2.figures[0].source_animation_elapsed_ms, 75);
    }

    #[test]
    fn file_round_trip() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players[0].gold = 999;
        sim.trade_ships
            .push(TradeShip::new(0, 0, 12, 13).with_name("Seehind".into()));
        let snap = sim.snapshot();
        let path = std::env::temp_dir().join("anno_sim_save_test.bin");
        save_to_file(&path, &snap).unwrap();
        let loaded = load_from_file(&path).unwrap();
        assert_eq!(loaded.players[0].gold, 999);
        assert_eq!(loaded.trade_ships[0].name, "Seehind");
        assert_eq!(loaded.version, SAVE_VERSION);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_bad_magic() {
        let path = std::env::temp_dir().join("anno_sim_save_bad.bin");
        std::fs::write(&path, b"NOPE\x00\x00").unwrap();
        let err = load_from_file(&path).unwrap_err();
        assert!(matches!(err, SaveError::BadMagic));
        std::fs::remove_file(&path).ok();
    }
}
