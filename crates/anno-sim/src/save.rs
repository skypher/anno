//! Save/load: bincode-serialized snapshot of the mutable runtime state.
//!
//! Immutable scenario data (BuildingDef, IslandMap, OceanMap) is reloaded
//! from the original COD/SZS files; only the player-mutable game state is
//! captured. CoverageMap and AI controllers are recomputed from scratch.

use crate::building::BuildingInstance;
use crate::combat::{DiplomacyMatrix, MilitaryUnit};
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
pub const SAVE_VERSION: u32 = 31;

/// Oldest save version this build can still deserialize. Anything
/// older has either a hard binary incompatibility (enum-variant
/// index shift, struct field reorder) or a known data-corruption
/// risk and is hard-rejected.
///
/// v31 baseline: source command-root records retain fixed-point production
/// inputs and selector fields, so earlier
/// payloads have a different bincode struct layout.
pub const MIN_LOADABLE_VERSION: u32 = 31;

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
    let state: SaveState = bincode::deserialize(&bytes[SAVE_MAGIC.len()..])
        .map_err(|e| SaveError::Decode(e.to_string()))?;
    // Newer versions are unsupported (this build's struct schema
    // doesn't know how to reach forward) and pre-v14 saves carry
    // a different bincode enum layout that this build can't
    // decode safely.
    if state.version > SAVE_VERSION || state.version < MIN_LOADABLE_VERSION {
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

        let snap = sim.snapshot();
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
                phase: 3,
                frame_selector: 12,
                activity: 96,
                work_material_stock: 64,
                raw_material_stock: 128,
                storage_fill: 128,
                storage_animation_capacity: 160,
                source_production_amount: 32,
                source_raw_material_amount: 64,
                source_work_material_amount: 16,
                progress: 512,
                animation_frame: 2,
                animation_count: 4,
                animation_continues: true,
                kind_code: 7,
            });
        let mut wh = Warehouse::new(1, 0, 5, 5);
        wh.set_capacity(Good::Wood, 100);
        wh.deposit(Good::Wood, 50);
        wh.deposit(Good::Tools, 7);
        sim.warehouses.push(wh);

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
                phase: 3,
                frame_selector: 12,
                activity: 96,
                work_material_stock: 64,
                raw_material_stock: 128,
                storage_fill: 128,
                storage_animation_capacity: 160,
                source_production_amount: 32,
                source_raw_material_amount: 64,
                source_work_material_amount: 16,
                progress: 512,
                animation_frame: 2,
                animation_count: 4,
                animation_continues: true,
                kind_code: 7,
            }]
        );
        assert_eq!(sim2.source_map_cell_states[0].market_frame_selector(4), 3);
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
