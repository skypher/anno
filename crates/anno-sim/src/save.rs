//! Save/load: bincode-serialized snapshot of the mutable runtime state.
//!
//! Immutable scenario data (BuildingDef, IslandMap, OceanMap) is reloaded
//! from the original COD/SZS files; only the player-mutable game state is
//! captured. CoverageMap and AI controllers are recomputed from scratch.

use crate::building::BuildingInstance;
use crate::combat::{DiplomacyMatrix, MilitaryUnit, SourceDynamicCombatFigure};
use crate::data_bridge::{
    SourceCityTable, SourceKind4Occupant, SourceKind13DispatchState, SourceKind13LocationTable,
};
use crate::entity::Figure;
use crate::player::Player;
use crate::simulation::Simulation;
use crate::source_cell::SourceMapCellState;
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
pub const SAVE_VERSION: u32 = 91;

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
pub const MIN_LOADABLE_VERSION: u32 = 90;

/// Magic bytes prefixing every save file.
pub const SAVE_MAGIC: [u8; 4] = *b"ASV1";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SaveState {
    pub version: u32,
    pub game_clock: u32,
    pub speed_multiplier: u32,
    pub paused: bool,
    pub autosave_timer_ms: u32,
    pub players: Vec<Player>,
    pub buildings: Vec<BuildingInstance>,
    pub source_dynamic_map_objects: Vec<SourceDynamicMapObject>,
    pub source_map_cell_states: Vec<SourceMapCellState>,
    pub source_static_map_roots: Vec<SourceMapCellState>,
    pub source_static_map_backing_cells: Vec<SourceMapCellState>,
    pub source_kind13_locations: SourceKind13LocationTable,
    pub source_kind13_dispatch: SourceKind13DispatchState,
    pub source_kind13_replacement_commands: Vec<crate::simulation::SourceKind13ReplacementCommand>,
    pub source_cities: SourceCityTable,
    pub source_figure_events: crate::source_figure_event::SourceFigureEventRegistry,
    pub source_kind4_occupants: Vec<SourceKind4Occupant>,
    pub source_dynamic_combat_figures: Vec<SourceDynamicCombatFigure>,
    pub source_kind6_actions: Vec<crate::combat::SourceKind6Action>,
    pub source_kind15_combat_figures: Vec<crate::combat::SourceKind15CombatFigure>,
    pub source_kind6_deferred_hits: Vec<crate::combat::SourceKind6DeferredHit>,
    pub source_kind6_terminal_events: Vec<crate::simulation::SourceKind6TerminalEvent>,
    pub tile_clears: Vec<crate::simulation::TileClear>,
    pub source_kind4_dispatch: crate::combat::SourceKind4DispatchState,
    pub source_time_ticks: u32,
    pub source_time_remainder_ms: u32,
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
            speed_multiplier: self.speed_multiplier,
            paused: self.paused,
            autosave_timer_ms: self.autosave_timer_ms,
            players: self.players.clone(),
            buildings: self.buildings.clone(),
            source_dynamic_map_objects: self.source_dynamic_map_objects.clone(),
            source_map_cell_states: self.source_map_cell_states.clone(),
            source_static_map_roots: self.source_static_map_roots.clone(),
            source_static_map_backing_cells: self.source_static_map_backing_cells.clone(),
            source_kind13_locations: self.source_kind13_locations.clone(),
            source_kind13_dispatch: self.source_kind13_dispatch.clone(),
            source_kind13_replacement_commands: self.source_kind13_replacement_commands.clone(),
            source_cities: self.source_cities.clone(),
            source_figure_events: self.source_figure_events.clone(),
            source_kind4_occupants: self.source_kind4_occupants.clone(),
            source_dynamic_combat_figures: self.source_dynamic_combat_figures.clone(),
            source_kind6_actions: self.source_kind6_actions.clone(),
            source_kind15_combat_figures: self.source_kind15_combat_figures.clone(),
            source_kind6_deferred_hits: self.source_kind6_deferred_hits.clone(),
            source_kind6_terminal_events: self.source_kind6_terminal_events.clone(),
            tile_clears: self.tile_clears.clone(),
            source_kind4_dispatch: self.source_kind4_dispatch,
            source_time_ticks: self.source_time_ticks,
            source_time_remainder_ms: self.source_time_remainder_ms,
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
        }
    }

    /// Apply a previously captured snapshot. Preserves immutable scenario
    /// data (building_defs, island_maps, ocean_map). Coverage maps and
    /// timers are reset and will be recomputed by the next tick.
    pub fn apply_snapshot(&mut self, s: SaveState) {
        self.game_clock = s.game_clock;
        self.speed_multiplier = s.speed_multiplier;
        self.paused = s.paused;
        self.autosave_timer_ms = s.autosave_timer_ms;
        self.players = s.players;
        self.buildings = s.buildings;
        self.source_dynamic_map_objects = s.source_dynamic_map_objects;
        self.source_map_cell_states = s.source_map_cell_states;
        self.source_static_map_roots = s.source_static_map_roots;
        self.source_static_map_backing_cells = s.source_static_map_backing_cells;
        self.source_kind13_locations = s.source_kind13_locations;
        self.source_kind13_dispatch = s.source_kind13_dispatch;
        self.source_kind13_replacement_commands = s.source_kind13_replacement_commands;
        self.source_cities = s.source_cities;
        self.source_figure_events = s.source_figure_events;
        self.source_kind4_occupants = s.source_kind4_occupants;
        self.source_dynamic_combat_figures = s.source_dynamic_combat_figures;
        self.source_kind6_actions = s.source_kind6_actions;
        self.source_kind15_combat_figures = s.source_kind15_combat_figures;
        self.source_kind6_deferred_hits = s.source_kind6_deferred_hits;
        self.source_kind6_terminal_events = s.source_kind6_terminal_events;
        self.tile_clears = s.tile_clears;
        self.source_kind4_dispatch = s.source_kind4_dispatch;
        self.source_time_ticks = s.source_time_ticks;
        self.source_time_remainder_ms = s.source_time_remainder_ms;
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
    }
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
        return Err(SaveError::Decode("save payload is missing its version".into()));
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
            speed_multiplier: 1,
            paused: false,
            autosave_timer_ms: 0,
            players: vec![Player::new_human(0)],
            buildings: vec![],
            source_dynamic_map_objects: vec![],
            source_map_cell_states: vec![],
            source_static_map_roots: vec![],
            source_static_map_backing_cells: vec![],
            source_kind13_locations: SourceKind13LocationTable::default(),
            source_kind13_dispatch: SourceKind13DispatchState::default(),
            source_kind13_replacement_commands: vec![],
            source_cities: SourceCityTable::default(),
            source_figure_events: crate::source_figure_event::SourceFigureEventRegistry::default(),
            source_kind4_occupants: vec![],
            source_dynamic_combat_figures: vec![],
            source_kind6_actions: vec![],
            source_kind15_combat_figures: vec![],
            source_kind6_deferred_hits: vec![],
            source_kind6_terminal_events: vec![],
            tile_clears: vec![],
            source_kind4_dispatch: crate::combat::SourceKind4DispatchState::default(),
            source_time_ticks: 0,
            source_time_remainder_ms: 0,
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
        assert_eq!(sim2.next_source_rand(), expected_next_rand);
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
                ruin_id: 4,
                ruin_footprint_width: 2,
                ruin_footprint_height: 3,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                phase: 3,
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
                source_damage_threshold: 640,
                source_damage_accumulator: 192,
                progress: 512,
                animation_frame: 2,
                animation_count: 4,
                animation_continues: true,
                kind_code: 7,
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
        assert!(
            sim.source_kind13_locations
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
                })
        );
        assert!(sim.source_cities.set_record(
            3,
            Some(crate::data_bridge::SourceCityRecord {
                island_id: 1,
                source_owner: 4,
                owner_slot: 4,
                phase: 6,
                tier_population: [10, 20, 30, 40, 50],
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
        sim.source_kind4_dispatch = crate::combat::SourceKind4DispatchState {
            active_player_slot: 2,
            single_player: false,
            faction_states: [0x0c, 0x0c, 0, 0x0c, 0x0d, 0x0e, 0x0b],
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
                source_action_ready_at: 53,
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
        sim.source_kind6_terminal_events
            .push(crate::simulation::SourceKind6TerminalEvent {
                target: crate::source_route::SourceTargetDescriptor::from_bytes([0x34, 1, 10, 20]),
                event_kind: 7,
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
        carrier.carried_good = Good::Wood as u8;
        carrier.carried_amount = 7;
        carrier.cargo_fixed = 231;
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
                ruin_id: 4,
                ruin_footprint_width: 2,
                ruin_footprint_height: 3,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                phase: 3,
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
                source_damage_threshold: 640,
                source_damage_accumulator: 192,
                progress: 512,
                animation_frame: 2,
                animation_count: 4,
                animation_continues: true,
                kind_code: 7,
            }]
        );
        assert_eq!(sim2.source_map_cell_states[0].market_frame_selector(4), 3);
        assert_eq!(sim2.source_static_map_roots.len(), 1);
        assert!(sim2.source_static_map_roots[0].matches(1, 14, 16));
        assert_eq!(sim2.source_static_map_roots[0].kind_code, 35);
        assert_eq!(sim2.source_static_map_roots[0].source_variant, 3);
        assert_eq!(sim2.source_static_map_roots[0].source_map_owner_slot, 5);
        assert_eq!(sim2.source_static_map_roots[0].source_definition_offset, 21);
        assert_eq!(sim2.source_static_map_roots[0].fallback_strand_cells, 0b10_101);
        assert_eq!(sim2.source_static_map_roots[0].source_damage_accumulator, 511);
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
                ..Default::default()
            })
        );
        assert_eq!(sim2.source_city_dispatch_elapsed_ms, 9_800);
        assert_eq!(sim2.source_city_dispatch_phase, 6);
        assert_eq!(sim2.source_city_dispatch_cursor, 17);
        assert_eq!(sim2.source_time_ticks, 19);
        assert_eq!(sim2.source_time_remainder_ms, 99);
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
                source_action_ready_at: 53,
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
            sim2.source_kind6_terminal_events,
            vec![crate::simulation::SourceKind6TerminalEvent {
                target: crate::source_route::SourceTargetDescriptor::from_bytes([0x34, 1, 10, 20]),
                event_kind: 7,
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
                kind6_target_descriptor: None,
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
                faction_states: [0x0c, 0x0c, 0, 0x0c, 0x0d, 0x0e, 0x0b],
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
        assert_eq!(sim2.source_kind4_occupants[0].idle_remaining_bits, 1.25_f32.to_bits());
        assert_eq!(sim2.military_units[0].source_step_remaining, 0.375);
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
        assert_eq!(sim2.figures[0].carried_good, Good::Wood as u8);
        assert_eq!(sim2.figures[0].carried_amount, 7);
        assert_eq!(sim2.figures[0].cargo_fixed, 231);
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
