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

/// In-process recorder buffer. Captures the starting snapshot at
/// construction so the recording replays from the state the commands were
/// actually issued against.
#[derive(Debug, Clone)]
pub struct Recorder {
    initial: SaveState,
    pub entries: Vec<(u32, Command)>,
}

impl Recorder {
    /// Begin recording from `sim`'s current state.
    pub fn start(sim: &Simulation) -> Self {
        Self {
            initial: sim.snapshot(),
            entries: Vec::new(),
        }
    }
    pub fn record(&mut self, game_clock: u32, cmd: Command) {
        self.entries.push((game_clock, cmd));
    }
    pub fn finish(self) -> Recording {
        Recording {
            version: REPLAY_VERSION,
            initial: self.initial,
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

/// Apply a recording's initial snapshot, then re-run it in real ticks:
/// the sim advances with a fixed `dt_ms` timestep and each command fires
/// once `game_clock` reaches its recorded timestamp. Because the snapshot
/// restores the RNG stream and subsystem timer phases, the replayed sim
/// retraces the recorded run tick for tick.
pub fn replay_advancing(rec: &Recording, sim: &mut Simulation, dt_ms: u32) {
    sim.apply_snapshot(rec.initial.clone());
    let mut cursor = 0;
    while cursor < rec.entries.len() {
        while cursor < rec.entries.len() && rec.entries[cursor].0 <= sim.game_clock {
            sim.apply_command(&rec.entries[cursor].1);
            cursor += 1;
        }
        if cursor < rec.entries.len() {
            if sim.paused {
                // A paused sim never advances its clock; fire the remaining
                // commands immediately instead of spinning forever.
                for (_clock, cmd) in &rec.entries[cursor..] {
                    sim.apply_command(cmd);
                }
                return;
            }
            sim.tick(dt_ms);
        }
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
        let mut rec = Recorder::start(&sim);
        let cmd = Command::SetTaxRate {
            player: 0,
            tier: 0,
            rate: 100,
        };
        rec.record(10, cmd.clone());
        let recording = rec.finish();

        let mut sim2 = Simulation::new();
        replay_into(&recording, &mut sim2);
        assert_eq!(sim2.players[0].tax_rates[0], 100);
    }

    #[test]
    fn advancing_replay_retraces_a_recorded_run() {
        // Record: run a sim with fixed dt, issuing commands mid-run.
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.seed_source_rand(99);
        let mut rec = Recorder::start(&sim);
        for tick in 0..40u32 {
            if tick == 10 {
                let cmd = Command::SetTaxRate {
                    player: 0,
                    tier: 0,
                    rate: 77,
                };
                rec.record(sim.game_clock, cmd.clone());
                sim.apply_command(&cmd);
            }
            if tick == 25 {
                let cmd = Command::SetTaxRate {
                    player: 0,
                    tier: 1,
                    rate: 33,
                };
                rec.record(sim.game_clock, cmd.clone());
                sim.apply_command(&cmd);
            }
            sim.tick(130);
        }
        let final_hash = sim.state_hash();
        let recording = rec.finish();

        // Replay with time advancement, then continue to the same tick
        // count; the replayed sim must land on the identical state.
        let mut replayed = Simulation::new();
        replay_advancing(&recording, &mut replayed, 130);
        while replayed.game_clock < sim.game_clock {
            replayed.tick(130);
        }
        assert_eq!(replayed.state_hash(), final_hash);
    }

    #[test]
    fn file_round_trip() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut rec = Recorder::start(&sim);
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
        let recording = rec.finish();

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
