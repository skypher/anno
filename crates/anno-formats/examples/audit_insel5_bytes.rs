//! Survey INSEL5 chunk bytes (116-byte island-metadata records,
//! per the memory note) for unexposed fields beyond
//! number/width/height/x_pos/y_pos.
//!
//! Run with `cargo run --example audit_insel5_bytes -p anno-formats`.

use anno_formats::szs::SzsFile;

fn main() {
    let dir = std::env::args().nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted/Szenes".into());

    let mut sizes: std::collections::BTreeMap<usize, u32> = Default::default();
    let mut all_bodies: Vec<(String, Vec<u8>)> = Vec::new();

    for entry in std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.extension().map(|s| s.eq_ignore_ascii_case("szs")).unwrap_or(false) { continue; }
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        for chunk in parsed.chunks.iter().filter(|c| c.name == "INSEL5") {
            *sizes.entry(chunk.data.len()).or_default() += 1;
            all_bodies.push((stem.clone(), chunk.data.clone()));
        }
    }

    println!("INSEL5 chunk-size distribution:");
    for (sz, count) in &sizes {
        println!("  {sz} bytes (0x{sz:X}): {count} chunks");
    }
    println!();

    let max_len = all_bodies.iter().map(|(_, b)| b.len()).max().unwrap_or(0);
    let mut nonzero = vec![0u32; max_len];
    let total = all_bodies.len() as u32;
    for (_, body) in &all_bodies {
        for (i, &b) in body.iter().enumerate() {
            if b != 0 { nonzero[i] += 1; }
        }
    }

    println!("Non-zero density per byte offset (>3% across {total} chunks):");
    for (off, &count) in nonzero.iter().enumerate() {
        let pct = count * 100 / total.max(1);
        if pct > 3 {
            println!("  0x{off:03X}: {pct:3}% ({count}/{total})");
        }
    }
    println!();

    // Dump first records of selected scenarios.
    // Probe the 100%-density region around 0x5C..0x70 across
    // the corpus.
    let mut byte_5c_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut byte_5d_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut u32_60_dist: std::collections::BTreeMap<u32, u32> = Default::default();
    for (_, body) in &all_bodies {
        if body.len() < 0x68 { continue; }
        *byte_5c_dist.entry(body[0x5C]).or_default() += 1;
        *byte_5d_dist.entry(body[0x5D]).or_default() += 1;
        let v = u32::from_le_bytes([body[0x60], body[0x61], body[0x62], body[0x63]]);
        *u32_60_dist.entry(v).or_default() += 1;
    }
    println!("\nbyte 0x5C distribution: {byte_5c_dist:?}");
    println!("byte 0x5D distribution: {byte_5d_dist:?}");
    println!("u32 at 0x60 top 10:");
    let mut sorted: Vec<_> = u32_60_dist.iter().collect();
    sorted.sort_by_key(|&(_, c)| std::cmp::Reverse(*c));
    for (v, c) in sorted.iter().take(10) {
        println!("  {v}  (0x{v:X}): {c} islands");
    }
    println!();

    println!("Per-island dumps for representative scenarios (offsets 0..=0x40):");
    let want = ["Tutorial0", "A Plague of Pirates", "Atoll", "New Horizons2"];
    for (scen, body) in &all_bodies {
        if !want.iter().any(|w| scen == w) { continue; }
        let dump: Vec<String> = (0..body.len().min(0x40)).step_by(4)
            .map(|o| format!("{:02x}{:02x}{:02x}{:02x}",
                body.get(o+3).copied().unwrap_or(0),
                body.get(o+2).copied().unwrap_or(0),
                body.get(o+1).copied().unwrap_or(0),
                body.get(o).copied().unwrap_or(0)))
            .collect();
        println!("  {scen} INSEL5 (size={}):", body.len());
        for line in dump.chunks(8) {
            println!("    {}", line.join(" "));
        }
    }
    println!();

    // Crucial cross-check: the prior parser reads bytes 0..8 only.
    // Is byte 3 stable / what does it carry?
    let mut byte3_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    for (_, body) in &all_bodies {
        if body.len() >= 4 {
            *byte3_dist.entry(body[3]).or_default() += 1;
        }
    }
    println!("byte 0x03 distribution: {byte3_dist:?}");

    // Identify the 6 outliers with byte 0x03 == 2.
    println!("byte 0x03 == 2 outliers (scenario, island number, settlement owners):");
    for (scen, body) in &all_bodies {
        if body.len() < 4 || body[3] != 2 { continue; }
        let mut owners = [7u8; 8];
        if body.len() >= 0x14 {
            owners.copy_from_slice(&body[0x0C..0x14]);
        }
        println!("  {scen}: island #{}, owner slots {:?}", body[0], owners);
    }
    println!();

    // Cross-tab bytes 0x0C..0x14: the eight per-settlement OWNER
    // slots (runtime `island+0xac`), written by `FUN_00468740`
    // (`1602_exe.c:72888-72897`) as the owning player slot, or 7
    // when the settlement pointer is null. The ~93% sevens this
    // audit reports are unsettled slots, NOT a crop distribution —
    // fertility is the u32 crop bitmask at 0x5C..0x60, cross-tabbed
    // by the 0x5C/0x5D histograms above.
    let mut owner_value_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut owner_pattern_dist: std::collections::BTreeMap<[u8; 8], u32> = Default::default();
    for (_, body) in &all_bodies {
        if body.len() < 0x14 { continue; }
        let mut row = [0u8; 8];
        row.copy_from_slice(&body[0x0C..0x14]);
        *owner_pattern_dist.entry(row).or_default() += 1;
        for &b in &row {
            *owner_value_dist.entry(b).or_default() += 1;
        }
    }
    println!("\nbytes 0x0C..0x14 (settlement owner slots, 7 = unsettled) value distribution:");
    println!("  per-byte values: {owner_value_dist:?}");
    println!("  most common 8-byte patterns:");
    let mut sorted: Vec<_> = owner_pattern_dist.iter().collect();
    sorted.sort_by_key(|&(_, c)| std::cmp::Reverse(*c));
    for (pattern, count) in sorted.iter().take(15) {
        let pretty: Vec<String> = pattern.iter().map(|b| format!("{:02X}", b)).collect();
        println!("    [{}] : {count}", pretty.join(" "));
    }
}
