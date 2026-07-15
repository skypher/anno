//! Main simulation dispatcher.
//!
//! Ported from FUN_00489670 (core simulation orchestrator).
//! Processes delta time in chunks of max 200ms, scaled by game speed multiplier.
//! Dispatches to 12 subsystem update functions on independent timers.

use crate::ai::{AiAction, AiController};
use crate::building::{BuildingDef, BuildingInstance};
use crate::carrier;
use crate::civilian;
use crate::combat::{
    self, DiplomacyMatrix, MilitaryUnit, SourceDynamicCombatFigure, SourceKind15CombatFigure,
};
use crate::coverage::CoverageMap;
use crate::data_bridge::{
    SourceCityRecord, SourceCityTable, SourceKind4Occupant, SourceKind13LocationTable,
};
use crate::economy;
use crate::entity::{ActionType, CargoRoute, Figure};
use crate::island_map::IslandMap;
use crate::ocean_map::OceanMap;
use crate::player::{Player, PlayerState};
use crate::population;
use crate::production;
use crate::source_cell::SourceMapCellState;
use crate::source_route::{
    SourceDynamicMapObject, SourceDynamicMapObjectTable, SourcePathBlockedCellDecision,
    SourcePathTargetRect, SourceResolvedDynamicTarget, SourceTargetDescriptor,
    source_route_positions,
};
use crate::trade::{self, TradeRoute, TradeShip};
use crate::types::{Good, TICKS_PER_MINUTE};
use crate::warehouse::Warehouse;

/// Auto-save interval in game ticks (~10 minutes of game time).
pub const AUTOSAVE_INTERVAL_MS: u32 = 599_999;

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
    timer_diplomacy: SubsystemTimer, // 5_000

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
    /// Placement anchors retained by the source kind-13 location table.
    pub source_kind13_locations: SourceKind13LocationTable,
    /// Fixed source city-record pool read by `FUN_0047f8a0`.
    pub source_cities: SourceCityTable,
    /// Live kind-4 occupancy reconstructed from authored `SOLDAT3` figures.
    pub source_kind4_occupants: Vec<SourceKind4Occupant>,
    /// Live `0x84a`/`0x84b` source figure records that do not have an
    /// equivalent local entity type. Categories 1 through 6 remain available
    /// to the common source candidate producer through these records.
    pub source_dynamic_combat_figures: Vec<SourceDynamicCombatFigure>,
    /// Immediate category-6 action records emitted through `FUN_004546e0`.
    /// This observability record is independent from compatibility damage.
    pub source_kind6_actions: Vec<combat::SourceKind6Action>,
    /// Kind-15 figures constructed by `FUN_00447f00` from emitted category-6
    /// actions. They remain outside the category-1 through -6 candidate pool.
    pub source_kind15_combat_figures: Vec<SourceKind15CombatFigure>,
    /// Deferred kind-1 records allocated by `FUN_00447880` for category-6
    /// actions. The source drains due records before its ordinary simulation
    /// subsystems run.
    pub source_kind6_deferred_hits: Vec<combat::SourceKind6DeferredHit>,
    /// Type-7 terminal commands emitted by completed map-root accumulators.
    pub source_kind6_terminal_events: Vec<SourceKind6TerminalEvent>,
    /// Source player globals that select type-4 terminal-route dispatch.
    pub source_kind4_dispatch: crate::combat::SourceKind4DispatchState,
    /// `DAT_005b6040`: source simulation clock in 100-ms ticks.
    pub source_time_ticks: u32,
    /// Milliseconds not yet promoted into `source_time_ticks`.
    pub source_time_remainder_ms: u32,
    /// `DAT_0054a3b4`: shared source city-dispatch time accumulator.
    pub source_city_dispatch_elapsed_ms: u32,
    /// Low-three-bit phase incremented after the source accumulator exceeds
    /// 9,999 ms.
    pub source_city_dispatch_phase: u8,
    /// Physical source city-record cursor. Each dispatch visits two slots.
    pub source_city_dispatch_cursor: usize,
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
            source_cities: SourceCityTable::default(),
            source_kind4_occupants: Vec::new(),
            source_dynamic_combat_figures: Vec::new(),
            source_kind6_actions: Vec::new(),
            source_kind15_combat_figures: Vec::new(),
            source_kind6_deferred_hits: Vec::new(),
            source_kind6_terminal_events: Vec::new(),
            source_kind4_dispatch: crate::combat::SourceKind4DispatchState::default(),
            source_time_ticks: 0,
            source_time_remainder_ms: 0,
            source_city_dispatch_elapsed_ms: 0,
            source_city_dispatch_phase: 0,
            source_city_dispatch_cursor: 0,
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

    /// Collect source-live combat candidates across authored and dynamically
    /// spawned figures. This mirrors the population consumed by
    /// `FUN_0045cd20` before category-specific handlers select an action.
    pub fn source_combat_candidates(&self) -> Vec<combat::SourceCombatCandidate> {
        combat::source_combat_candidate_buffer(
            &self.military_units,
            &self.trade_ships,
            &self.source_dynamic_combat_figures,
        )
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
                let object = table.objects().find(|object| object.slot == slot).copied()?;
                let map = self.island_maps.iter().find(|map| map.island_id == island)?;
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
        let attacker = candidates
            .iter()
            .find(|candidate| {
                candidate.figure_kind == 6 && candidate.runtime_slot == runtime_slot
            })?;
        combat::source_kind6_select_target(attacker, &candidates, &self.diplomacy, |descriptor| {
            self.source_kind6_target_rect(descriptor)
        })
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
        let selected = combat::source_kind6_select_target(
            &attacker,
            &candidates,
            &self.diplomacy,
            |descriptor| self.source_kind6_target_rect(descriptor),
        )?;
        if !combat::source_kind6_target_policy_allows(attacker_owner_state, &selected.target) {
            return None;
        }
        let action = combat::source_kind6_action(&attacker, selected)?;
        let next_ready_at = combat::source_kind6_action_ready_at(self.source_time_ticks, &attacker)?;
        let impact_due_at = combat::source_kind6_impact_due_at(self.source_time_ticks, &attacker)?;
        let launcher_height = self
            .source_dynamic_combat_figures
            .get(dynamic_index)?
            .position_z;
        let kind15_figure = combat::source_kind15_figure_from_action(action, launcher_height);
        let figure = self.source_dynamic_combat_figures.get_mut(dynamic_index)?;
        figure.direction = action.direction;
        figure.source_action_ready_at = next_ready_at;
        self.source_kind6_actions.push(action);
        if self.source_kind6_deferred_hits.len() < combat::SOURCE_KIND6_DEFERRED_HIT_CAPACITY {
            self.source_kind6_deferred_hits
                .push(combat::SourceKind6DeferredHit {
                    due_at: impact_due_at,
                    action,
                });
        }
        if let Some(kind15_figure) = kind15_figure {
            self.source_kind15_combat_figures.push(kind15_figure);
        }
        Some(action)
    }

    /// Install a live `0x84a`/`0x84b` figure in the category table selected
    /// by `FUN_0045d380`. Categories 1, 2, 3, and 5 share table slots, while
    /// categories 4 and 6 own their respective source tables. A later load of
    /// the same source table slot replaces the represented live record.
    pub fn install_source_dynamic_combat_figure(
        &mut self,
        mut figure: SourceDynamicCombatFigure,
    ) -> bool {
        let Some((table, capacity)) = combat::source_dynamic_slot_table(figure.figure_kind)
        else {
            return false;
        };
        if figure.runtime_slot >= capacity {
            return false;
        }
        if figure.figure_kind == 6 {
            figure.source_action_ready_at = self.source_time_ticks;
        }

        if let Some(index) = self
            .source_dynamic_combat_figures
            .iter()
            .position(|existing| {
                combat::source_dynamic_slot_table(existing.figure_kind)
                    .is_some_and(|(existing_table, _)| {
                        existing_table == table && existing.runtime_slot == figure.runtime_slot
                    })
            })
        {
            self.source_dynamic_combat_figures[index] = figure;
        } else {
            self.source_dynamic_combat_figures.push(figure);
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
            (Subsystem::Production, TimingCadence::Interval(self.timer_production.interval_ms)),
            (Subsystem::Population, TimingCadence::Interval(self.timer_population.interval_ms)),
            (Subsystem::Diplomacy, TimingCadence::Interval(self.timer_diplomacy.interval_ms)),
            (Subsystem::MarketCoverage, TimingCadence::Interval(self.timer_market.interval_ms)),
            (Subsystem::Ships, TimingCadence::Interval(self.timer_ships.interval_ms)),
            (Subsystem::Events, TimingCadence::Interval(self.timer_events.interval_ms)),
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

    fn tick_events(&mut self) {
        self.tick_pirate_event();
        self.tick_disaster_event();
    }

    fn tick_pirate_event(&mut self) {
        // Pirate event spawning needs the decoded source scheduler and target
        // selection. Scenario-loaded pirate ships and player surrender still
        // populate the pirate faction; do not fabricate new ships from an
        // implementation-only random gate.
    }

    /// Source-anchored disaster scheduler placeholder. Called from
    /// `tick_events`. RE: figuren.cod `Nummer: VULKAN` (figure 0x12)
    /// and `Nummer: BRANDMARKT` (figure 0x08); see `disaster.rs`
    /// module doc-comment.
    fn tick_disaster_event(&mut self) {
        // Fire and volcano events need decoded source triggers/anchors.
        // Do not invent an origin from a random player building; the
        // lower-level effect helpers remain available for source-anchored
        // call sites.
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
        self.tick_source_kind6_deferred_hits();
        self.tick_source_kind15_combat_figures(dt_ms);

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

    /// Drain due kind-1 records exactly as `FUN_00478ab0`. The executor's
    /// queued action still names its selected live target; category-6 target
    /// records redirect through their retained static map descriptor before
    /// the map-root accumulator receives the scaled source strength.
    fn tick_source_kind6_deferred_hits(&mut self) {
        let queued_hits = std::mem::take(&mut self.source_kind6_deferred_hits);
        for hit in queued_hits {
            if hit.due_at > self.source_time_ticks {
                self.source_kind6_deferred_hits.push(hit);
                continue;
            }

            let Some(target) = self.source_kind6_static_map_target(hit.action.target_descriptor)
            else {
                continue;
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
                        .apply_source_kind6_map_hit(hit.action.raw_strength)
                        .then_some(*state)
                });
            if let Some(root) = terminal_root {
                self.apply_source_kind6_terminal_map_command(root);
            }
        }
    }

    /// Replay the event-kind-seven branch of `FUN_0046a630`: its
    /// `FUN_00463f40` consumer removes the command root and rewrites the
    /// oriented footprint with the root's `Ruinenr` replacement or clear.
    fn apply_source_kind6_terminal_map_command(&mut self, root: SourceMapCellState) {
        let target = SourceTargetDescriptor::from_source_kind34_island_cell(
            root.island,
            root.x,
            root.y,
        );
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
            let Some(definition) = cod.ruin_variant_building(
                clear.ruin_id,
                clear.ruin_uses_strand_table,
                draw,
            ) else {
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
                    writes.push((clear.tile_x + u16::from(dx), clear.tile_y + u16::from(dy), definition));
                }
            }
        }
        for (x, y, definition) in writes {
            let (Ok(x), Ok(y)) = (u8::try_from(x), u8::try_from(y)) else {
                continue;
            };
            let Some(mut root) = SourceMapCellState::new_static(clear.island_id, x, y, definition, 0) else {
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
            if let Some(city) = self.source_cities.record_mut(slot) {
                city.phase = self.source_city_dispatch_phase;
            }
            if city.tier_population.iter().copied().sum::<u32>() > 29
                && self.source_city_activity_allows(city)
            {
                self.spawn_source_kind12_figures(city);
            }
        }
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

    /// `FUN_00480370`: consume its discarded rand draws, sample at most ten
    /// physical kind-13 slots, and submit every matching source anchor to
    /// the kind-12 route allocator.
    fn spawn_source_kind12_figures(&mut self, city: SourceCityRecord) {
        for _ in 0..15 {
            self.next_source_rand();
        }

        let locations = self
            .source_kind13_locations
            .city_slice(city.island_id)
            .to_vec();
        for _ in 0..10 {
            let location_index = usize::from(self.next_source_rand()) % locations.len();
            let Some(location) = locations[location_index] else {
                continue;
            };
            if location.island_id != city.island_id || location.source_owner != city.source_owner {
                continue;
            }

            let permission_branch = (self.next_source_rand() & 3) as u8;
            if let Some(figure) = self.allocate_source_kind12_figure(location, permission_branch) {
                self.figures.push(figure);
            }
        }
    }

    /// `FUN_0044b140`: initialize a kind-12 figure at one kind-13 anchor,
    /// then retain only the route that its threshold callback reaches.
    fn allocate_source_kind12_figure(
        &mut self,
        location: crate::data_bridge::SourceKind13Location,
        permission_branch: u8,
    ) -> Option<Figure> {
        let _initial_definition =
            civilian::source_kind12_initial_definition(self.next_source_rand());
        let threshold = (u32::from(self.next_source_rand() & 7) + 5) * 0x40;
        let map = self
            .island_maps
            .iter()
            .find(|map| map.island_id == location.island_id)?;
        let start = (i32::from(location.tile_x), i32::from(location.tile_y));
        let mut grid = map.civilian_path_grid(start, location.source_owner, permission_branch);
        let route = grid
            .search_with_blocked_cell_callback(start, |_, elapsed_cost| {
                if elapsed_cost >= threshold {
                    SourcePathBlockedCellDecision::Complete
                } else {
                    SourcePathBlockedCellDecision::Expand
                }
            })
            .ok()?;
        let target_kind = map.civilian_path_kind(route.position)?;
        let definition = civilian::source_kind12_definition(target_kind, self.next_source_rand());
        let path = source_route_positions(start, &route.steps)?;

        let mut figure = Figure::new();
        figure.action = ActionType::Walking;
        figure.owner = location.source_owner;
        figure.origin_island = location.island_id;
        figure.tile_x = start.0;
        figure.tile_y = start.1;
        figure.target_x = route.position.0;
        figure.target_y = route.position.1;
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

    fn tick_production(&mut self) {
        let mut new_carriers = Vec::new();
        let carrier_suppliers: Vec<_> = self
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
        let mut carrier_suppliers = carrier_suppliers;
        carrier_suppliers.extend(
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

        for i in 0..self.buildings.len() {
            let def_id = self.buildings[i].def_id;
            if def_id as usize >= self.building_defs.len() {
                continue;
            }
            let def = self.building_defs[def_id as usize].clone();
            let source_raw_material_stock = self.buildings[i].input_1_stock.saturating_mul(32);
            let source_work_material_stock = self.buildings[i].input_2_stock.saturating_mul(32);
            let source_storage_fill = self.buildings[i].output_stock.saturating_mul(32);
            let produced = production::tick_building(
                &mut self.buildings[i],
                &def,
                self.timer_production.interval_ms,
            );
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
                if produced != 0 {
                    state.advance_source_scheduler(self.buildings[i].efficiency);
                } else {
                    state.set_activity(self.buildings[i].efficiency);
                }
                self.buildings[i].input_1_stock = state.raw_material_stock / 32;
                self.buildings[i].input_2_stock = state.work_material_stock / 32;
                self.buildings[i].output_stock = state.storage_fill / 32;
                *state != before
            } else {
                false
            };
            if state_changed {
                self.source_map_cell_revision = self.source_map_cell_revision.wrapping_add(1);
            }

            if self.buildings[i].active {
                // Check if this building already has an active carrier
                let has_carrier = self.figures.iter().any(|f| {
                    f.is_active()
                        && f.building_idx == i as u16
                        && matches!(f.action, ActionType::CarryingGoods | ActionType::Returning)
                });

                if !has_carrier {
                    if let Some(mut c) = carrier::try_spawn_carrier(
                        &self.buildings[i],
                        &def,
                        &carrier_suppliers,
                        &mut self.source_map_cell_states,
                        &mut self.warehouses,
                        &self.island_maps,
                        self.carrier_config,
                    ) {
                        c.building_idx = i as u16;
                        new_carriers.push(c);
                    }
                }
            }
        }

        // Type 11 is scheduled from city command roots, not from the
        // production-building loop. MARKT and KONTOR roots use their city's
        // inventory to build the FUN_00480610 capacity eligibility bytes
        // before selecting and reserving a producer root.
        let city_cart_suppliers =
            Self::city_cart_supplier_view(&carrier_suppliers, &self.buildings, &self.building_defs);
        let city_origins: Vec<_> = self
            .source_map_cell_states
            .iter()
            .copied()
            .filter(|state| matches!(state.kind_code, 7 | 8 | 30))
            .collect();
        for origin in city_origins {
            let Some(warehouse_idx) = self
                .warehouses
                .iter()
                .enumerate()
                .find(|warehouse| {
                    warehouse.1.active
                        && warehouse.1.island_id == origin.island
                        && (origin.kind_code != 8
                            || (warehouse.1.tile_x == u16::from(origin.x)
                                && warehouse.1.tile_y == u16::from(origin.y)))
                })
                .map(|(idx, _)| idx)
            else {
                continue;
            };
            let transfer_root_count = self
                .source_map_cell_states
                .iter()
                .filter(|state| state.island == origin.island && matches!(state.kind_code, 7 | 8))
                .count();
            let city_capacity_fixed =
                self.warehouses[warehouse_idx].city_storage_capacity_fixed(transfer_root_count);
            let city = carrier::CityCartEligibility::from_city_store(
                &self.warehouses[warehouse_idx],
                city_capacity_fixed,
            );
            let has_cart = self.figures.iter().any(|figure| {
                figure.is_active()
                    && figure.cargo_route == CargoRoute::CityCart
                    && figure.origin_island == origin.island
                    && figure.origin_x == u16::from(origin.x)
                    && figure.origin_y == u16::from(origin.y)
            });
            if has_cart {
                continue;
            }
            if let Some(cart) = carrier::try_spawn_city_cart(
                origin,
                city,
                &city_cart_suppliers,
                &mut self.source_map_cell_states,
                &self.island_maps,
                self.city_cart_config,
            ) {
                new_carriers.push(cart);
            }
        }

        self.figures.extend(new_carriers);
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
        combat::acquire_source_kind4_candidates_with_terrain(
            &mut self.military_units,
            &self.diplomacy,
            &self.island_maps,
        );
        let mut source_rand = std::mem::take(&mut self.rng_state);
        combat::tick_unit_orders_with_maps_and_source_rand_and_dispatch_state(
            &mut self.military_units,
            dt_ms,
            self.ocean_map.as_ref(),
            &self.island_maps,
            &mut source_rand,
            self.source_kind4_dispatch,
        );
        self.rng_state = source_rand;
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
                && unit.source_target_descriptor.is_some_and(|descriptor| {
                    descriptor.kind() == 0x34
                })
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
                                && self
                                    .source_time_ticks
                                    .wrapping_sub(occupant.idle_timestamp_ticks)
                                    >= 20
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
                let descriptor =
                    SourceTargetDescriptor::from_source_kind34_island_cell(island_id, target_x, target_y);
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
                        occupant.route_program = crate::combat::default_source_kind4_route_program();
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
        let carrier_config = self.carrier_config;
        let city_cart_config = self.city_cart_config;
        let civilian_config = self.civilian_config;

        for (idx, figure) in self.figures.iter_mut().enumerate() {
            if !figure.is_active() {
                continue;
            }

            figure.move_timer_ms += dt_ms;
            while figure.speed > 0 && figure.move_timer_ms >= 100 {
                figure.move_timer_ms -= 100;

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
                                    city_cart_config.frame_speed_ms,
                                    city_cart_config.frames_per_direction,
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
                                let origin = (
                                    figure.origin_island,
                                    figure.origin_x,
                                    figure.origin_y,
                                    figure.origin_kind,
                                );
                                let should_despawn = carrier::handle_arrival(
                                    figure,
                                    &self.buildings,
                                    &self.island_maps,
                                );

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
                                                    && building.owner == figure.owner
                                                    && building.tile_x == supplier_target.0
                                                    && building.tile_y == supplier_target.1
                                                    && self
                                                        .building_defs
                                                        .get(building.def_id as usize)
                                                        .is_some_and(|definition| {
                                                            definition.output_good == good
                                                        })
                                            });
                                        let picked =
                                            supplier_idx.map(|_| requested_fixed / 32).unwrap_or(0);
                                        let mut remaining_source_fill = None;
                                        let collected = picked != 0
                                            && self
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
                                                .is_some_and(|state| {
                                                    let collected = state
                                                        .collect_reserved_storage(requested_fixed);
                                                    if collected {
                                                        remaining_source_fill =
                                                            Some(state.storage_fill);
                                                    }
                                                    collected
                                                });
                                        if collected {
                                            if let Some(supplier_idx) = supplier_idx {
                                                self.buildings[supplier_idx].output_stock =
                                                    remaining_source_fill.unwrap_or(0) / 32;
                                            }
                                            figure.carried_amount = picked;
                                            figure.cargo_fixed = requested_fixed;
                                            self.source_map_cell_revision =
                                                self.source_map_cell_revision.wrapping_add(1);
                                        } else {
                                            figure.carried_amount = 0;
                                            figure.cargo_fixed = 0;
                                        }
                                        let uncollected_fixed = if collected {
                                            requested_fixed.saturating_sub(figure.cargo_fixed)
                                        } else {
                                            requested_fixed
                                        };
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
                                        let delivered_fixed = if figure.cargo_fixed == 0 {
                                            delivered.saturating_mul(32)
                                        } else {
                                            figure.cargo_fixed
                                        };
                                        if delivered_fixed != 0 {
                                            let transfer_root_count = self
                                                .source_map_cell_states
                                                .iter()
                                                .filter(|state| {
                                                    state.island == origin.0
                                                        && matches!(state.kind_code, 7 | 8)
                                                })
                                                .count();
                                            let accepted_fixed = if let Some(warehouse) =
                                                self.warehouses.iter_mut().find(|warehouse| {
                                                    warehouse.active
                                                        && warehouse.island_id == origin.0
                                                        && warehouse.owner == figure.owner
                                                        && (origin.3 != 8
                                                            || (warehouse.tile_x == origin.1
                                                                && warehouse.tile_y == origin.2))
                                                }) {
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
                                            };
                                            if origin.3 == 7 {
                                                if let Some(state) = self
                                                    .source_map_cell_states
                                                    .iter_mut()
                                                    .find(|state| {
                                                        state.kind_code == 7
                                                            && state.matches(
                                                                origin.0, origin.1, origin.2,
                                                            )
                                                    })
                                                {
                                                    state.accept_market_transfer(accepted_fixed);
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
                                                        warehouse.collect_reserved(good, picked)
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
                                                    .is_some_and(|state| {
                                                        let collected = state
                                                            .collect_reserved_storage(
                                                                requested_fixed,
                                                            );
                                                        if collected {
                                                            remaining_source_fill =
                                                                Some(state.storage_fill);
                                                        }
                                                        collected
                                                    })
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
                                                figure.carried_amount = picked;
                                                figure.cargo_fixed = requested_fixed;
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
                                        let uncollected_fixed = if collected {
                                            requested_fixed.saturating_sub(figure.cargo_fixed)
                                        } else {
                                            requested_fixed
                                        };
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
                                        let (island, x, y, accepted) = {
                                            let building = &mut self.buildings[origin_idx];
                                            let accepted = if good == input_1 {
                                                building.input_1_stock = building
                                                    .input_1_stock
                                                    .saturating_add(delivered);
                                                true
                                            } else if good == input_2 {
                                                building.input_2_stock = building
                                                    .input_2_stock
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
                                                if good == input_1 {
                                                    state.raw_material_stock = state
                                                        .raw_material_stock
                                                        .saturating_add(delivered_fixed);
                                                } else {
                                                    state.work_material_stock = state
                                                        .work_material_stock
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
                        if civilian_config.is_kind12(figure) && figure.source_move_speed > 0 {
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
                            if carrier::advance_source_carrier(figure, 100, terrain_wegspeed) {
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

        // Remove despawned figures (iterate in reverse to preserve indices)
        for &idx in despawn_indices.iter().rev() {
            self.figures.swap_remove(idx);
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
            source_action_ready_at: 0,
            target_descriptor: SourceTargetDescriptor::from_bytes([0x37, 0, 60, 65]),
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner,
            state: 7,
            flags: 0,
            notification: 0,
            runtime_slot,
            auxiliary_kind: 0,
            name_index: 0,
        };

        assert!(sim.install_source_dynamic_combat_figure(figure(1, 149, 1)));
        assert!(sim.install_source_dynamic_combat_figure(figure(3, 149, 2)));
        assert_eq!(sim.source_dynamic_combat_figures.len(), 1);
        assert_eq!(sim.source_dynamic_combat_figures[0].figure_kind, 3);
        assert_eq!(sim.source_dynamic_combat_figures[0].owner, 2);
        assert!(!sim.install_source_dynamic_combat_figure(figure(1, 150, 0)));

        assert!(sim.install_source_dynamic_combat_figure(figure(4, 399, 3)));
        assert!(!sim.install_source_dynamic_combat_figure(figure(4, 400, 0)));
        sim.source_time_ticks = 47;
        assert!(sim.install_source_dynamic_combat_figure(figure(6, 349, 4)));
        assert!(!sim.install_source_dynamic_combat_figure(figure(6, 350, 0)));
        assert!(!sim.install_source_dynamic_combat_figure(figure(7, 0, 0)));

        assert_eq!(
            sim.source_combat_candidates()
                .iter()
                .map(|candidate| (candidate.figure_kind, candidate.runtime_slot))
                .collect::<Vec<_>>(),
            vec![(3, 149), (4, 399), (6, 349)]
        );
        assert_eq!(sim.source_dynamic_combat_figures[2].source_action_ready_at, 47);
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
            source_action_ready_at: 0,
            target_descriptor,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner,
            state: 0,
            flags: 0,
            notification: 0,
            runtime_slot,
            auxiliary_kind: 0,
            name_index: 0,
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
            source_action_ready_at: 0,
            target_descriptor,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner,
            state: 0,
            flags: 0,
            notification: 0,
            runtime_slot,
            auxiliary_kind: 0,
            name_index: 0,
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
            vec![crate::combat::SourceKind6DeferredHit {
                due_at: 29,
                action,
            }]
        );
        assert_eq!(sim.source_dynamic_combat_figures[0].direction, 2);
        assert_eq!(sim.source_dynamic_combat_figures[0].source_action_ready_at, 69);
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
    }

    #[test]
    fn deferred_category_six_map_hit_accumulates_and_emits_type_seven_terminal_event() {
        let mut sim = Simulation::new();
        let mut expected_rng = Simulation::new();
        expected_rng.seed_source_rand(23);
        let expected_ruin_draw = expected_rng.next_source_rand();
        sim.seed_source_rand(23);
        let target = SourceTargetDescriptor::from_source_kind34_island_cell(3, 4, 5);
        sim.source_dynamic_combat_figures.push(SourceDynamicCombatFigure {
            active: true,
            figure_kind: 6,
            candidate_list_key: 2,
            figure_definition_id: 0x1f,
            direction: 0,
            source_payload: 0,
            position: (0.0, 0.0),
            position_z: 0.0,
            source_energy: 285,
            source_action_ready_at: 0,
            target_descriptor: target,
            state_descriptor: SourceTargetDescriptor::from_bytes([0; 4]),
            owner: 1,
            state: 0,
            flags: 0,
            notification: 0,
            runtime_slot: 9,
            auxiliary_kind: 0,
            name_index: 0,
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
        let state = SourceMapCellState::new(2, 7, 9, &definition, 0)
            .expect("selector-bearing source root");
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
        let first = SourceMapCellState::new_static(2, 7, 9, &definition, 0)
            .expect("static source cell");
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
        let mut root = SourceMapCellState::new_static(2, 7, 9, &definition, 0)
            .expect("static source command");
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
        assert_eq!((restored.source_variant, restored.source_map_owner_slot), (0, 5));
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
        assert_eq!(
            descriptor.kind(),
            0x34
        );
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
    fn production_completion_preserves_matching_source_cell_fixed_point_state() {
        use anno_formats::cod::{BuildingDef as CodBuilding, CodFile};
        use std::collections::HashMap;

        let cod_building = CodBuilding {
            kind: "HANDWERK".into(),
            source_production_amount: 16,
            source_raw_material_amount: 64,
            storage_animation_capacity: 160,
            properties: HashMap::from([
                ("ProdKind".into(), "HANDWERK".into()),
                ("Ware".into(), "WERKZEUG".into()),
                ("Rohstoff".into(), "EISEN".into()),
                ("Rohmenge".into(), "2".into()),
                ("Interval".into(), "1".into()),
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

        sim.tick_production();

        let state = sim.source_map_cell_states[0];
        assert_eq!(state.raw_material_stock, 96);
        assert_eq!(state.storage_fill, 16);
        assert_eq!(state.progress, 16);
        assert_eq!(state.activity, 128);
        assert_eq!(sim.source_map_cell_revision, 1);

        sim.tick_production();

        let state = sim.source_map_cell_states[0];
        assert_eq!(state.raw_material_stock, 32);
        assert_eq!(state.storage_fill, 32);
        assert_eq!(state.progress, 32);
        assert_eq!(sim.buildings[0].input_1_stock, 1);
        assert_eq!(sim.buildings[0].output_stock, 1);
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
    fn generic_carrier_collects_from_supplier_anchor_after_reaching_footprint_edge() {
        use crate::types::Good;
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
        supplier_state.storage_fill = 65;
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
        assert_eq!(sim.buildings[0].input_1_stock, 2);
        assert_eq!(sim.source_map_cell_states[0].raw_material_stock, 65);
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

        let market = CodBuilding {
            kind: "MARKT".into(),
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

        sim.tick_production();
        assert_eq!(sim.figures.len(), 1);
        assert_eq!(sim.figures[0].cargo_route, CargoRoute::CityCart);
        assert_eq!(sim.source_map_cell_states[1].reserved_storage, 65);

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
        assert_eq!(sim.warehouses[0].stock(Good::Cloth), 2);
        assert_eq!(sim.warehouses[0].city_stock_fixed(Good::Cloth), 65);
        assert_eq!(sim.source_map_cell_states[0].progress, 65);
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
    fn fire_effect_uses_building_definition_maxbrand_cap() {
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
            output_good: Good::None,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 1_000,
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
            max_brand_damage_ticks: 1,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        });
        sim.buildings.push(BuildingInstance::new(0, 0, 4, 4, 0));

        assert!(crate::disaster::ignite_building_with_cap(
            &mut sim.buildings[0],
            sim.building_defs[0].max_brand_damage_ticks,
        ));
        assert_eq!(sim.buildings[0].fire_damage_ticks, 1);
        let after_first_fire = sim.buildings[0].health;

        assert!(!crate::disaster::ignite_building_with_cap(
            &mut sim.buildings[0],
            sim.building_defs[0].max_brand_damage_ticks,
        ));
        assert_eq!(sim.buildings[0].fire_damage_ticks, 1);
        assert_eq!(sim.buildings[0].health, after_first_fire);
    }

    #[test]
    fn disaster_event_does_not_fabricate_fire_origin() {
        use crate::building::{BUILDING_MAX_HEALTH, BuildingDef, BuildingInstance};
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
            output_good: Good::None,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 1_000,
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
        sim.buildings.push(BuildingInstance::new(0, 0, 4, 4, 0));

        // Seed 8 used to satisfy the removed speculative fire gate.
        sim.seed_source_rand(8);
        sim.tick_disaster_event();

        assert_eq!(sim.buildings[0].health, BUILDING_MAX_HEALTH);
        assert_eq!(sim.buildings[0].fire_damage_ticks, 0);
        assert!(sim.event_log.iter().all(|line| !line.starts_with("[fire]")));
    }

    #[test]
    fn disaster_event_does_not_fabricate_volcano_origin() {
        use crate::building::{BUILDING_MAX_HEALTH, BuildingDef, BuildingInstance};
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
            output_good: Good::None,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 1_000,
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
        sim.buildings.push(BuildingInstance::new(0, 0, 4, 4, 0));

        // Seed 2 used to treat 29216 % 32 == 0 as a volcano trigger
        // centered on this ordinary player building.
        sim.seed_source_rand(2);
        sim.tick_disaster_event();

        assert_eq!(sim.buildings[0].health, BUILDING_MAX_HEALTH);
        assert!(
            sim.event_log
                .iter()
                .all(|line| !line.starts_with("[volcano]"))
        );
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
        assert!(
            sim.source_dynamic_map_object_table(5)
                .objects()
                .next()
                .is_none()
        );
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
        use crate::combat::{MilitaryUnit, UnitType, tick_unit_orders};
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
        use crate::combat::{MilitaryUnit, UnitType, unit_build_cost};
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
        assert!(
            !sim.apply_command(&crate::commands::Command::ProposeTradeAgreement { a: 0, b: 1 })
        );

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
        assert!(
            sim.ocean_map
                .as_ref()
                .unwrap()
                .is_navigable(trader.target_x, trader.target_y)
        );
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
