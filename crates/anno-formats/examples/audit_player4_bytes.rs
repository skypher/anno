//! Survey PLAYER4 byte offsets that the parser doesn't yet
//! interpret — primarily 0x18 and 0x34..=0x37. Cross-scenario
//! patterns guide the next round of RE on those fields.
//!
//! Run with `cargo run --example audit_player4_bytes`.

use anno_formats::szs::SzsFile;

const SLOT_STRIDE: usize = 1072;
const SLOTS: usize = 7;

fn main() {
    let dir = std::env::args().nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted/Szenes".into());

    let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("szs"))
            .unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    // For each candidate offset, collect (scenario, slot, value)
    // and report a cross-tab.
    let candidate_offsets: &[usize] = &[
        0x18, 0x34, 0x35, 0x36, 0x37,
        0x05, 0x06, 0x08, 0x09, 0x0a, 0x0b,
        0x0d, 0x0e, 0x0f, 0x10, 0x11,
    ];

    let mut samples: std::collections::BTreeMap<usize,
        std::collections::BTreeMap<u8, Vec<String>>> =
        std::collections::BTreeMap::new();

    let mut player4_bodies: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in &entries {
        let bytes = match std::fs::read(entry.path()) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        let player4 = parsed.chunks.iter().find(|c| c.name == "PLAYER4");
        if let Some(c) = player4 {
            let stem = entry.path().file_stem().unwrap().to_string_lossy().into_owned();
            player4_bodies.push((stem, c.data.clone()));
        }
    }

    for off in candidate_offsets {
        for (name, body) in &player4_bodies {
            for slot in 0..SLOTS {
                let pos = slot * SLOT_STRIDE + off;
                if pos < body.len() {
                    let v = body[pos];
                    samples.entry(*off).or_default()
                        .entry(v).or_default()
                        .push(format!("{name}#{slot}"));
                }
            }
        }
    }

    println!("PLAYER4 byte-value distribution across {} scenarios × 7 slots:\n",
             player4_bodies.len());
    for off in candidate_offsets {
        let entry = match samples.get(off) { Some(e) => e, None => continue };
        let total: usize = entry.values().map(|v| v.len()).sum();
        println!("  offset 0x{off:02X}: {} distinct values across {total} samples",
                 entry.len());
        // List up to 5 most common values with sample scenarios.
        let mut by_count: Vec<_> = entry.iter().collect();
        by_count.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        for (val, users) in by_count.iter().take(8) {
            let head: Vec<_> = users.iter().take(3).cloned().collect();
            let suffix = if users.len() > 3 {
                format!(" … (+{})", users.len() - 3)
            } else { String::new() };
            println!("    0x{val:02X} ({} samples): {}{suffix}",
                     users.len(), head.join(", "));
        }
        println!();
    }

    // For 0x18 and 0x34..=0x37 specifically, dump per-slot values
    // for a handful of representative scenarios so we can eyeball
    // which slot the byte tracks.
    let representatives = [
        "Atoll", "A Plague of Pirates", "Cooperation", "Good Neighbors",
        "Tutorial0", "Continous Play00", "The Magnate0",
    ];
    println!("Per-slot dumps for 0x18 / 0x34..0x37:\n");
    for (name, body) in &player4_bodies {
        if !representatives.iter().any(|r| name == r) { continue; }
        println!("  {name}");
        for slot in 0..SLOTS {
            let base = slot * SLOT_STRIDE;
            if base + 0x38 > body.len() { continue; }
            let b18 = body[base + 0x18];
            let b34 = body[base + 0x34];
            let b35 = body[base + 0x35];
            let b36 = body[base + 0x36];
            let b37 = body[base + 0x37];
            println!("    slot {slot}: 0x18={b18:#04x}  0x34..0x37 = {b34:#04x} {b35:#04x} {b36:#04x} {b37:#04x}");
        }
        println!();
    }
}
