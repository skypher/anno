//! Survey every shipping scenario's `Mission::flags` and dump
//! the goals_raw contents for any flag bit not already decoded.
//!
//! Run with `cargo run --example audit_mission_flags`.

use anno_formats::szs::{
    self, MISSION_FLAG_COOPERATIVE, MISSION_FLAG_PIRATE,
    MISSION_FLAG_POPULATION, MISSION_FLAG_POPULATION2,
    MISSION_FLAG_POPULATION3, MISSION_FLAG_RANKING,
};

const KNOWN: u32 = MISSION_FLAG_POPULATION
    | MISSION_FLAG_POPULATION2
    | MISSION_FLAG_POPULATION3
    | MISSION_FLAG_COOPERATIVE
    | MISSION_FLAG_RANKING
    | MISSION_FLAG_PIRATE;

fn main() {
    let dir = std::env::args().nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted/Szenes".into());

    let mut bit_users: std::collections::BTreeMap<u32, Vec<String>> =
        std::collections::BTreeMap::new();

    let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path()
            .extension().and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("szs"))
            .unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let parsed = match szs::SzsFile::parse(&bytes) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let mission = match parsed.mission {
            Some(m) => m,
            None => continue,
        };
        let flags = mission.flags;
        let unknown = flags & !KNOWN;
        if unknown != 0 {
            for bit in 0..32 {
                let mask = 1u32 << bit;
                if unknown & mask != 0 {
                    bit_users.entry(mask).or_default()
                        .push(format!("{name} (flags=0x{flags:08X})"));
                }
            }
        }
    }

    if bit_users.is_empty() {
        println!("No scenarios use unmodelled mission flag bits.");
        return;
    }

    println!("Unmodelled mission flag bits:\n");
    for (mask, users) in &bit_users {
        println!("  bit 0x{mask:08X} ({} scenarios):", users.len());
        for u in users {
            println!("    {u}");
        }
        println!();
    }

    // Dump non-zero goals_raw u32 slots for each scenario that
    // uses an unmodelled bit, so we can correlate bit ↔ slot.
    let mut seen = std::collections::BTreeSet::new();
    for users in bit_users.values() {
        for u in users {
            let name = u.split_whitespace().next().unwrap_or("");
            seen.insert(name.to_string());
        }
    }

    println!("Goals_raw slots for unmodelled scenarios:\n");
    for entry in std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()) {
        let path = entry.path();
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if !seen.iter().any(|s| stem.starts_with(s)) { continue; }
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match szs::SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        let mission = match parsed.mission { Some(m) => m, None => continue };
        println!("  {}  flags=0x{:08X}", stem, mission.flags);
        let head: String = mission.briefing.chars().take(200).collect();
        println!("    briefing: {head}");
        for i in 0..(mission.goals_raw.len() / 4).min(40) {
            let off = i * 4;
            let v = u32::from_le_bytes([
                mission.goals_raw[off], mission.goals_raw[off + 1],
                mission.goals_raw[off + 2], mission.goals_raw[off + 3],
            ]);
            if v != 0 {
                println!("    u32[{i:2}] (off 0x{off:03X}) = {v}  (0x{v:X})");
            }
        }
        println!();
    }
}
