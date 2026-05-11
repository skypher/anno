//! Survey STADT4 chunk bytes (168-byte city records) across
//! every shipping scenario, looking for population/gold/
//! satisfaction-shaped fields beyond the existing owner+name.
//!
//! Run with `cargo run --example audit_stadt4_bytes -p anno-formats`.

use anno_formats::szs::SzsFile;

const RECORD_BYTES: usize = 168;

fn main() {
    let dir = std::env::args().nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted/Szenes".into());

    let mut nonzero = vec![0u32; RECORD_BYTES];
    let mut total_records = 0u32;
    let mut all_records: Vec<(String, u8, String, Vec<u8>)> = Vec::new();

    for entry in std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.extension().map(|s| s.eq_ignore_ascii_case("szs")).unwrap_or(false) { continue; }
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        for chunk in parsed.chunks.iter().filter(|c| c.name == "STADT4") {
            if chunk.data.len() != RECORD_BYTES { continue; }
            for (j, &b) in chunk.data.iter().enumerate() {
                if b != 0 { nonzero[j] += 1; }
            }
            total_records += 1;
            // Owner = byte 0; name = at 0x87..
            let owner = chunk.data[0];
            let name_end = chunk.data[0x87..]
                .iter()
                .position(|&b| b == 0)
                .map(|n| 0x87 + n)
                .unwrap_or(chunk.data.len());
            let name: String = chunk.data[0x87..name_end]
                .iter().map(|&b| char::from(b)).collect();
            all_records.push((stem.clone(), owner, name, chunk.data.clone()));
        }
    }

    println!("STADT4: {total_records} city records.\n");

    println!("Non-zero density per byte offset (>5%):");
    for (off, &count) in nonzero.iter().enumerate() {
        let pct = count * 100 / total_records.max(1);
        if pct > 5 {
            println!("  0x{off:03X}: {pct:3}% ({count}/{total_records})");
        }
    }
    println!();

    // Try to interpret byte 0x60 as u32 across cities and dump
    // distribution.
    let mut byte_60_dist: std::collections::BTreeMap<u32, u32> = Default::default();
    for (_, _, _, body) in &all_records {
        if body.len() >= 0x64 {
            let v = u32::from_le_bytes([body[0x60], body[0x61], body[0x62], body[0x63]]);
            *byte_60_dist.entry(v).or_default() += 1;
        }
    }
    println!("u32 at offset 0x60 distribution (top 10):");
    let mut sorted: Vec<_> = byte_60_dist.iter().collect();
    sorted.sort_by_key(|&(_, c)| std::cmp::Reverse(*c));
    for (v, c) in sorted.iter().take(10) {
        println!("  {v}  (0x{v:X}): {c} samples", v=v);
    }
    println!();

    // Cities by owner — only owners 0..=6 should appear.
    let mut owner_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    for (_, owner, _, _) in &all_records {
        *owner_dist.entry(*owner).or_default() += 1;
    }
    println!("STADT4 owner distribution: {owner_dist:?}\n");

    // Show records for known scenarios with active cities.
    println!("Scenarios with non-zero STADT4 fields (offset 0x00..0x70):");
    let mut shown = 0;
    for (scen, owner, name, body) in &all_records {
        // Skip placeholder cities (all-zero or only sentinel).
        let head_nonzero = body[0..0x70].iter().any(|&b| b != 0);
        if !head_nonzero { continue; }
        if shown >= 8 { break; }
        let bytes_dump: Vec<String> = (0..0x70).step_by(4)
            .map(|o| format!("{:02x}{:02x}{:02x}{:02x}",
                body[o+3], body[o+2], body[o+1], body[o]))
            .collect();
        println!("  {scen} owner={owner} \"{name}\":");
        for chunk_line in bytes_dump.chunks(8) {
            println!("    {}", chunk_line.join(" "));
        }
        shown += 1;
    }
    println!();

    // Probe the 0x60..0x78 region as 5 u32s — candidate for
    // per-tier population in cities that ship pre-populated.
    // Probe the sparse 0x18..0x28 region for non-zero cities.
    println!("0x18..0x28 region — cities with non-zero values:");
    let mut shown_sparse = 0;
    for (scen, _owner, name, body) in &all_records {
        let has = (0x18..0x28).any(|i| body.get(i).copied().unwrap_or(0) != 0);
        if !has { continue; }
        if shown_sparse >= 12 { break; }
        let r = |o: usize| u32::from_le_bytes([
            body[o], body[o+1], body[o+2], body[o+3],
        ]);
        println!("  {scen} \"{name}\": [0x18]={} [0x1C]={} [0x20]={} [0x24]={}",
            r(0x18), r(0x1C), r(0x20), r(0x24));
        shown_sparse += 1;
    }
    println!();

    println!("Per-tier population candidate (5 u32 at 0x60..0x74):");
    let mut shown = 0;
    for (scen, owner, name, body) in &all_records {
        // Only show cities with at least one non-zero value here.
        let any = (0x60..0x78).any(|i| body.get(i).copied().unwrap_or(0) != 0);
        if !any { continue; }
        if shown >= 12 { break; }
        let read_u32 = |o: usize| u32::from_le_bytes([
            body[o], body[o+1], body[o+2], body[o+3],
        ]);
        let tiers = [
            read_u32(0x60), read_u32(0x64), read_u32(0x68),
            read_u32(0x6C), read_u32(0x70),
        ];
        let total: u64 = tiers.iter().map(|&v| v as u64).sum();
        println!("  {scen} \"{name}\" island_index={owner}: {tiers:?} (sum={total})");
        shown += 1;
    }
    println!();

    // Correlate byte 0x05 with highest populated tier index
    // (Pioneer=0, Settler=1, Citizen=2, Merchant=3, Aristocrat=4).
    println!("byte 0x05 vs highest populated tier:");
    let mut hits: std::collections::BTreeMap<(u8, usize), u32> = Default::default();
    for (_, _, _, body) in &all_records {
        let read_u32 = |o: usize| u32::from_le_bytes([
            body[o], body[o+1], body[o+2], body[o+3],
        ]);
        let tiers = [
            read_u32(0x60), read_u32(0x64), read_u32(0x68),
            read_u32(0x6C), read_u32(0x70),
        ];
        if tiers.iter().all(|&v| v == 0) { continue; }
        let highest = tiers.iter().enumerate()
            .filter(|&(_, &v)| v > 0)
            .map(|(i, _)| i)
            .max().unwrap_or(0);
        let b5 = body.get(0x05).copied().unwrap_or(0);
        *hits.entry((b5, highest)).or_default() += 1;
    }
    for ((b5, tier), c) in &hits {
        println!("  byte 0x05 = 0x{b5:02X}, highest tier = {tier} : {c} cities");
    }
    println!();

    // Probe the head region 0x04..0x14 for a treasury-shaped
    // u32 — the Chamnitz dump showed 0x6d9f = 28063 at 0x08.
    println!("u32 at 0x08 (treasury candidate) for cities with population:");
    let mut shown = 0;
    for (scen, _owner, name, body) in &all_records {
        let pop_any = (0x60..0x78).any(|i| body.get(i).copied().unwrap_or(0) != 0);
        if !pop_any { continue; }
        if shown >= 15 { break; }
        let read_u32 = |o: usize| u32::from_le_bytes([
            body[o], body[o+1], body[o+2], body[o+3],
        ]);
        let h_u32_4  = read_u32(0x04);
        let h_u32_8  = read_u32(0x08);
        let h_u32_c  = read_u32(0x0C);
        let h_u32_10 = read_u32(0x10);
        println!("  {scen} \"{name}\": [0x04]={h_u32_4} [0x08]={h_u32_8} [0x0C]={h_u32_c} [0x10]={h_u32_10}");
        shown += 1;
    }
    println!();

    println!("First-12 records dump (offsets 0x00..0x88, before name):");
    for (scen, owner, name, body) in all_records.iter().take(12) {
        let bytes_dump: Vec<String> = (0..0x88).step_by(4)
            .map(|o| format!("{:02x}{:02x}{:02x}{:02x}",
                body[o+3], body[o+2], body[o+1], body[o]))
            .collect();
        println!("  {scen} owner={owner} \"{name}\":");
        for chunk_line in bytes_dump.chunks(8) {
            println!("    {}", chunk_line.join(" "));
        }
    }
}
