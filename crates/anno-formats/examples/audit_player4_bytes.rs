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
    let mut size_distribution: std::collections::BTreeMap<usize, usize> =
        std::collections::BTreeMap::new();
    for entry in &entries {
        let bytes = match std::fs::read(entry.path()) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        let player4 = parsed.chunks.iter().find(|c| c.name == "PLAYER4");
        if let Some(c) = player4 {
            let stem = entry.path().file_stem().unwrap().to_string_lossy().into_owned();
            *size_distribution.entry(c.data.len()).or_default() += 1;
            player4_bodies.push((stem, c.data.clone()));
        }
    }

    println!("PLAYER4 chunk-size distribution:");
    for (sz, count) in &size_distribution {
        let leading = sz / 0x430;
        let trailing = sz % 0x430;
        println!("  {sz} bytes (0x{sz:X}) — {count} scenarios; / 0x430 = {leading}, % = 0x{trailing:X}");
    }
    println!();

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

    // Raw slot 0x400..0x440 dump for one scenario to confirm
    // whether the binary's second strcpy source is non-empty.
    if let Some((name, body)) = player4_bodies.iter().find(|(n, _)| n == "Tutorial0") {
        println!("Tutorial0 raw slot 0x400..0x440 hex dump:");
        for slot in 0..SLOTS {
            let base = slot * SLOT_STRIDE;
            if base + 0x440 > body.len() { continue; }
            let bytes = &body[base + 0x400..base + 0x440];
            println!("  slot {slot}: {bytes:02x?}");
        }
        println!();
        let _ = name;
    }
    if let Some((_, body)) = player4_bodies.iter().find(|(n, _)| n == "A Plague of Pirates") {
        println!("A Plague of Pirates raw slot 0x400..0x440 hex dump:");
        for slot in 0..SLOTS {
            let base = slot * SLOT_STRIDE;
            if base + 0x440 > body.len() { continue; }
            let bytes = &body[base + 0x400..base + 0x440];
            println!("  slot {slot}: {bytes:02x?}");
        }
        println!();
    }
    // Find the highest-density non-zero region across all
    // PLAYER4 slots — gives a list of fields the parser hasn't
    // inspected yet.
    let mut nonzero_count = vec![0u32; SLOT_STRIDE];
    for (_, body) in &player4_bodies {
        for slot in 0..SLOTS {
            let base = slot * SLOT_STRIDE;
            for i in 0..SLOT_STRIDE {
                if base + i < body.len() && body[base + i] != 0 {
                    nonzero_count[i] += 1;
                }
            }
        }
    }
    let total = (player4_bodies.len() * SLOTS) as u32;
    println!("Non-zero density per slot offset (>5% across {total} samples):");
    for i in 0..SLOT_STRIDE {
        let pct = nonzero_count[i] * 100 / total.max(1);
        if pct > 5 {
            println!("  0x{i:03X}: {pct:3}% ({}/{total})", nonzero_count[i]);
        }
    }
    println!();

    // Per-slot u32 dumps for offsets identified as field-shaped:
    //   0x03C — universally non-zero
    //   0x140..0x178 — 7-element 8-byte-stride array (per-rival
    //                  relationship table?)
    //   0x0C0..0x0E8 — 4-element 8-byte-stride block
    let probe_scenarios = ["Tutorial0", "A Plague of Pirates",
                           "Atoll", "The Magnate0", "Cooperation"];
    for probe in &probe_scenarios {
        if let Some((_, body)) = player4_bodies.iter().find(|(n, _)| n == probe) {
            println!("{probe}:");
            for slot in 0..SLOTS {
                let base = slot * SLOT_STRIDE;
                if base + 0x180 > body.len() { continue; }
                let read_u32 = |o: usize| u32::from_le_bytes([
                    body[base + o], body[base + o + 1],
                    body[base + o + 2], body[base + o + 3],
                ]);
                let s_0x3c = read_u32(0x3C);
                let block_c0: Vec<u32> = (0..4).map(|i| read_u32(0xC0 + i * 8)).collect();
                let arr_140: Vec<u32> = (0..7).map(|i| read_u32(0x140 + i * 8)).collect();
                println!("  slot {slot}: 0x3C={s_0x3c:#010x}  0xC0/8={block_c0:08x?}  0x140/8={arr_140:08x?}");
            }
            println!();
        }
    }
    // Dump the null-terminated string at slot offset 0x400.
    // FUN_00473cc9 in 1602_exe.c copies this into the runtime
    // player struct alongside the name at 0x3C0 — likely an AI
    // personality / strategy id.
    println!("Slot+0x400 strings (per-scenario per-slot):\n");
    for (name, body) in &player4_bodies {
        let mut row = format!("  {name}:");
        let mut any = false;
        for slot in 0..SLOTS {
            let base = slot * SLOT_STRIDE;
            if base + 0x440 > body.len() { continue; }
            let s_bytes = &body[base + 0x400..base + 0x440];
            let end = s_bytes.iter().position(|&b| b == 0).unwrap_or(s_bytes.len());
            if end == 0 { continue; }
            let s: String = s_bytes[..end].iter().map(|&b| char::from(b)).collect();
            row.push_str(&format!(" [{slot}]\"{s}\""));
            any = true;
        }
        if any {
            println!("{row}");
        }
    }
    println!();

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
