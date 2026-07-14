//! Replay recorder + replayer.
//!
//! A recording is a sequence of `(game_clock, Command)` entries plus a
//! starting `SaveState`. To play it back: deserialize, apply the snapshot
//! to a fresh sim, then replay each command at its recorded clock — the
//! deterministic sim produces identical state so a recording is a sound
//! way to share crashes, debug AI, or post-mortem a multiplayer match.

use crate::commands::Command;
use crate::save::SaveState;
use crate::simulation::Simulation;
use std::path::Path;

pub const REPLAY_MAGIC: [u8; 4] = *b"REPL";
pub const REPLAY_VERSION: u32 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Recording {
    pub version: u32,
    pub initial: SaveState,
    pub entries: Vec<(u32, Command)>,
}

#[derive(Debug)]
pub enum ReplayError {
    Io(std::io::Error),
    BadMagic,
    VersionMismatch { found: u32, expected: u32 },
    Codec(String),
}

impl From<std::io::Error> for ReplayError {
    fn from(e: std::io::Error) -> Self {
        ReplayError::Io(e)
    }
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::Io(e) => write!(f, "io: {e}"),
            ReplayError::BadMagic => write!(f, "not a replay file"),
            ReplayError::VersionMismatch { found, expected } => {
                write!(f, "replay version {found}, expected {expected}")
            }
            ReplayError::Codec(e) => write!(f, "codec: {e}"),
        }
    }
}

pub fn save_recording(path: &Path, rec: &Recording) -> Result<(), ReplayError> {
    let body = bincode::serialize(rec).map_err(|e| ReplayError::Codec(e.to_string()))?;
    let mut buf = Vec::with_capacity(REPLAY_MAGIC.len() + body.len());
    buf.extend_from_slice(&REPLAY_MAGIC);
    buf.extend_from_slice(&body);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &buf)?;
    Ok(())
}

pub fn load_recording(path: &Path) -> Result<Recording, ReplayError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < REPLAY_MAGIC.len() || bytes[..REPLAY_MAGIC.len()] != REPLAY_MAGIC {
        return Err(ReplayError::BadMagic);
    }
    let rec: Recording = bincode::deserialize(&bytes[REPLAY_MAGIC.len()..])
        .map_err(|e| ReplayError::Codec(e.to_string()))?;
    if rec.version != REPLAY_VERSION {
        return Err(ReplayError::VersionMismatch {
            found: rec.version,
            expected: REPLAY_VERSION,
        });
    }
    Ok(rec)
}

/// In-process recorder buffer.
#[derive(Debug, Default, Clone)]
pub struct Recorder {
    pub entries: Vec<(u32, Command)>,
}

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn record(&mut self, game_clock: u32, cmd: Command) {
        self.entries.push((game_clock, cmd));
    }
    pub fn finish(self, sim: &Simulation) -> Recording {
        Recording {
            version: REPLAY_VERSION,
            initial: sim.snapshot(),
            entries: self.entries,
        }
    }
}

/// Apply a recording's initial snapshot + every command in order.
/// Doesn't advance time between commands; meant for offline verification.
pub fn replay_into(rec: &Recording, sim: &mut Simulation) {
    sim.apply_snapshot(rec.initial.clone());
    for (_clock, cmd) in &rec.entries {
        sim.apply_command(cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn record_and_replay_changes_taxes() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut rec = Recorder::new();
        let cmd = Command::SetTaxRate {
            player: 0,
            tier: 0,
            rate: 100,
        };
        rec.record(10, cmd.clone());
        let recording = rec.finish(&sim);

        let mut sim2 = Simulation::new();
        replay_into(&recording, &mut sim2);
        assert_eq!(sim2.players[0].tax_rates[0], 100);
    }

    #[test]
    fn file_round_trip() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut rec = Recorder::new();
        rec.record(
            7,
            Command::SetTaxRate {
                player: 0,
                tier: 1,
                rate: 80,
            },
        );
        rec.record(
            15,
            Command::SetTaxRate {
                player: 0,
                tier: 2,
                rate: 32,
            },
        );
        let recording = rec.finish(&sim);

        let path = std::env::temp_dir().join("anno_replay_test.bin");
        save_recording(&path, &recording).unwrap();
        let loaded = load_recording(&path).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].0, 7);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_bad_magic() {
        let path = std::env::temp_dir().join("anno_replay_bad.bin");
        std::fs::write(&path, b"NOPE----").unwrap();
        assert!(matches!(load_recording(&path), Err(ReplayError::BadMagic)));
        std::fs::remove_file(&path).ok();
    }
}
