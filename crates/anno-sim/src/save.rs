//! Save/load: bincode-serialized snapshot of the mutable runtime state.
//!
//! Immutable scenario data (BuildingDef, IslandMap, OceanMap) is reloaded
//! from the original COD/SZS files; only the player-mutable game state is
//! captured. CoverageMap and AI controllers are recomputed from scratch.

use crate::building::BuildingInstance;
use crate::combat::{DiplomacyMatrix, MilitaryUnit};
use crate::entity::Figure;
use crate::history::EconomyHistory;
use crate::player::Player;
use crate::simulation::Simulation;
use crate::trade::{TradeRoute, TradeShip};
use crate::warehouse::Warehouse;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// On-disk save format version. Bump on incompatible changes.
pub const SAVE_VERSION: u32 = 12;

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
    pub warehouses: Vec<Warehouse>,
    pub figures: Vec<Figure>,
    pub military_units: Vec<MilitaryUnit>,
    pub diplomacy: DiplomacyMatrix,
    pub trade_routes: Vec<TradeRoute>,
    pub trade_ships: Vec<TradeShip>,
    #[serde(default)]
    pub history: EconomyHistory,
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
    fn from(e: std::io::Error) -> Self { SaveError::Io(e) }
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
            warehouses: self.warehouses.clone(),
            figures: self.figures.clone(),
            military_units: self.military_units.clone(),
            diplomacy: self.diplomacy.clone(),
            trade_routes: self.trade_routes.clone(),
            trade_ships: self.trade_ships.clone(),
            history: self.history.clone(),
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
        self.warehouses = s.warehouses;
        self.figures = s.figures;
        self.military_units = s.military_units;
        self.diplomacy = s.diplomacy;
        self.trade_routes = s.trade_routes;
        self.trade_ships = s.trade_ships;
        self.history = s.history;
        self.objectives = s.objectives;
    }
}

pub fn save_to_file(path: &Path, state: &SaveState) -> Result<(), SaveError> {
    let payload = bincode::serialize(state)
        .map_err(|e| SaveError::Encode(e.to_string()))?;
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
    if state.version != SAVE_VERSION {
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
        assert_eq!(sim2.warehouses.len(), 1);
        assert_eq!(sim2.warehouses[0].stock(Good::Wood), 50);
        assert_eq!(sim2.warehouses[0].stock(Good::Tools), 7);
    }

    #[test]
    fn file_round_trip() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players[0].gold = 999;
        let snap = sim.snapshot();
        let path = std::env::temp_dir().join("anno_sim_save_test.bin");
        save_to_file(&path, &snap).unwrap();
        let loaded = load_from_file(&path).unwrap();
        assert_eq!(loaded.players[0].gold, 999);
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
