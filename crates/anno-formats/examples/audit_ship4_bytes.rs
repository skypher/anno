//! Survey SHIP4 record bytes (each ship slot is 436 bytes) for
//! non-zero density and per-ship dumps, so we can identify
//! owner/type/cargo fields beyond name+x+y.
//!
//! Run with `cargo run --example audit_ship4_bytes -p anno-formats`.

use anno_formats::figuren::FiguresFile;
use anno_formats::szs::SzsFile;

const RECORD_BYTES: usize = 436;

fn main() {
    let dir = std::env::args().nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted/Szenes".into());

    // Per-offset non-zero density across every record in every
    // scenario.
    let mut nonzero = vec![0u32; RECORD_BYTES];
    let mut total_records = 0u32;
    let mut all_records: Vec<(String, usize, Vec<u8>)> = Vec::new();

    for entry in std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.extension().map(|s| s.eq_ignore_ascii_case("szs")).unwrap_or(false) { continue; }
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let chunk = match parsed.chunks.iter().find(|c| c.name == "SHIP4") { Some(c) => c, None => continue };
        for (i, slot_bytes) in chunk.data.chunks_exact(RECORD_BYTES).enumerate() {
            for (j, &b) in slot_bytes.iter().enumerate() {
                if b != 0 { nonzero[j] += 1; }
            }
            total_records += 1;
            all_records.push((stem.clone(), i, slot_bytes.to_vec()));
        }
    }

    println!("SHIP4: {total_records} ship records across all scenarios.\n");

    println!("Non-zero density per byte offset (>5%):");
    for (off, &count) in nonzero.iter().enumerate() {
        let pct = count * 100 / total_records.max(1);
        if pct > 5 {
            println!("  0x{off:03X}: {pct:3}% ({count}/{total_records})");
        }
    }
    println!();

    // Dump a handful of representative ship records by name —
    // non-name fields, focusing on fixed-position high-density
    // bytes.
    // Specific cross-ship distribution for offsets that look
    // structured: 0x46 (ship index?), 0x48 (ship type?),
    // 0x4A (sub-type?), 0x4B (owner?)
    let mut owner_distribution: std::collections::BTreeMap<u8, u32> =
        std::collections::BTreeMap::new();
    let mut type_distribution: std::collections::BTreeMap<u8, u32> =
        std::collections::BTreeMap::new();
    let mut idx_matches_byte_46 = 0u32;
    for (_name, idx, body) in &all_records {
        if body.len() < 0x4C { continue; }
        *owner_distribution.entry(body[0x4B]).or_default() += 1;
        *type_distribution.entry(body[0x48]).or_default() += 1;
        if (*idx as u8) == body[0x46] { idx_matches_byte_46 += 1; }
    }
    // Check the 100%-density constant-looking bytes 0x3D and 0x41.
    let mut byte_3d_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut byte_41_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut byte_4d_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut byte_4a_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    for (_, _, body) in &all_records {
        if body.len() >= 0x50 {
            *byte_3d_dist.entry(body[0x3D]).or_default() += 1;
            *byte_41_dist.entry(body[0x41]).or_default() += 1;
            *byte_4d_dist.entry(body[0x4D]).or_default() += 1;
            *byte_4a_dist.entry(body[0x4A]).or_default() += 1;
        }
    }
    println!("byte 0x3D distribution: {byte_3d_dist:?}");
    println!("byte 0x41 distribution: {byte_41_dist:?}");
    println!("byte 0x4A distribution: {byte_4a_dist:?}");
    println!("byte 0x4D distribution: {byte_4d_dist:?}");

    // Cross-tab byte 0x4A vs byte 0x4B (owner) — does 0x4A
    // distinguish native ships from human/AI ships?
    let mut crosstab: std::collections::BTreeMap<(u8, u8), u32> = Default::default();
    for (_, _, body) in &all_records {
        if body.len() >= 0x50 {
            *crosstab.entry((body[0x4A], body[0x4B])).or_default() += 1;
        }
    }
    let mut crosstab_4d: std::collections::BTreeMap<(u8, u8), u32> = Default::default();
    for (_, _, body) in &all_records {
        if body.len() >= 0x50 {
            let bucket = if body[0x4D] == 0xFF { 0xFFu8 } else if body[0x4D] < 32 { 1u8 } else { 2u8 };
            *crosstab_4d.entry((bucket, body[0x4B])).or_default() += 1;
        }
    }
    println!("byte 0x4D bucket × owner cross-tab:");
    for ((bucket, owner), c) in &crosstab_4d {
        let name = match bucket {
            1 => "small int",
            0xFF => "0xFF",
            _ => "other",
        };
        println!("  ({}, owner={}): {}", name, owner, c);
    }
    println!();
    println!("byte 0x4A × byte 0x4B (owner) cross-tab:");
    for ((a, b), c) in &crosstab {
        println!("  (0x4A={a}, owner={b}): {c}");
    }
    println!();

    println!("byte 0x46 == chunk-index in {idx_matches_byte_46} / {} records",
        all_records.len());
    println!("byte 0x4B (owner candidate) distribution: {owner_distribution:?}");
    println!("byte 0x48 (ship-type candidate) distribution: {type_distribution:?}");

    // Resolve byte 0x48 candidates against figuren.cod symbolic
    // names so we can name the ship types.
    let fig_path = "/home/sky/anno/extracted/figuren.cod";
    if let Ok(fig_bytes) = std::fs::read(fig_path) {
        let figs = FiguresFile::parse(&fig_bytes);
        println!("\nResolving byte 0x48 against figuren.cod by VEC INDEX:");
        for type_byte in type_distribution.keys() {
            let resolved = figs.figures.get(*type_byte as usize)
                .map(|f| f.name.clone())
                .unwrap_or_else(|| String::from("(out of range)"));
            println!("  type byte 0x{type_byte:02X} ({type_byte}) → {resolved}");
        }
        // Also list figures whose names start with HANDEL/KRIEG/PIRAT
        // so we can correlate.
        println!("  figures matching HANDEL|KRIEG|PIRAT:");
        for (i, f) in figs.figures.iter().enumerate() {
            if f.name.starts_with("HANDEL") || f.name.starts_with("KRIEG")
               || f.name.starts_with("PIRAT") {
                println!("    [{i:3}] {}", f.name);
            }
        }
        // Search constants for ship-related FIGTYP values matching
        // 21, 23, 25, 27, 31.
        println!("  constants table containing 21/23/25/27/31:");
        for (k, v) in figs.constants.iter() {
            if [21, 23, 25, 27, 31].contains(v) {
                println!("    {k} = {v}");
            }
        }
    }
    println!();

    // Audit the candidate cargo-manifest region: 7 stride-8
    // entries from 0x174..0x1AC. Decode each as (good_id: u32,
    // quantity: u32) and dump per-ship.
    println!("Cargo manifest dump (offsets 0x174..0x1AC, 7 stride-8 entries):");
    let mut shown_cargo = 0;
    for (name, idx, body) in &all_records {
        let ship_name: String = body[0..28].iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect();
        if ship_name.is_empty() { continue; }
        let entries: Vec<(u32, u32)> = (0..7)
            .map(|i| {
                let o = 0x174 + i * 8;
                let a = u32::from_le_bytes([body[o], body[o+1], body[o+2], body[o+3]]);
                let b = u32::from_le_bytes([body[o+4], body[o+5], body[o+6], body[o+7]]);
                (a, b)
            })
            .collect();
        let any_nonzero = entries.iter().any(|&(a, b)| a != 0 || b != 0);
        if !any_nonzero { continue; }
        if shown_cargo >= 12 { break; }
        let pretty: Vec<String> = entries.iter()
            .map(|(a, b)| format!("({a}, {b})"))
            .collect();
        println!("  {name}#{idx} \"{ship_name}\": {}", pretty.join("  "));
        shown_cargo += 1;
    }
    println!();

    // Distribution of the first u32 of each cargo entry — if it's
    // a good_id, values should be small (< ~30 for Anno 1602's
    // ware count) and bounded.
    let mut first_u32_of_entry0: std::collections::BTreeMap<u32, u32> =
        std::collections::BTreeMap::new();
    for (_, _, body) in &all_records {
        let v = u32::from_le_bytes([body[0x174], body[0x175], body[0x176], body[0x177]]);
        *first_u32_of_entry0.entry(v).or_default() += 1;
    }
    println!("Cargo entry[0] first-u32 distribution (top 10):");
    let mut sorted: Vec<_> = first_u32_of_entry0.iter().collect();
    sorted.sort_by_key(|&(_, c)| std::cmp::Reverse(*c));
    for (v, c) in sorted.iter().take(10) {
        println!("  0x{v:08X} ({}): {c} samples", v);
    }
    println!();

    // Distribution of the LOW 16 bits and HIGH 16 bits across
    // every non-zero entry of every ship's cargo array.
    let mut low16_dist: std::collections::BTreeMap<u16, u32> =
        std::collections::BTreeMap::new();
    let mut high16_dist: std::collections::BTreeMap<u16, u32> =
        std::collections::BTreeMap::new();
    for (_, _, body) in &all_records {
        for i in 0..7 {
            let o = 0x174 + i * 8;
            let v = u32::from_le_bytes([body[o], body[o+1], body[o+2], body[o+3]]);
            if v == 0 { continue; }
            *low16_dist.entry((v & 0xFFFF) as u16).or_default() += 1;
            *high16_dist.entry((v >> 16) as u16).or_default() += 1;
        }
    }
    println!("Cargo entry low-16-bits distinct values: {}", low16_dist.len());
    println!("  most common 10:");
    let mut sl: Vec<_> = low16_dist.iter().collect();
    sl.sort_by_key(|&(_, c)| std::cmp::Reverse(*c));
    for (v, c) in sl.iter().take(10) {
        println!("    0x{v:04X} ({:5}): {c}", v);
    }
    println!("Cargo entry high-16-bits distinct values: {}", high16_dist.len());
    println!("  most common 10:");
    let mut sh: Vec<_> = high16_dist.iter().collect();
    sh.sort_by_key(|&(_, c)| std::cmp::Reverse(*c));
    for (v, c) in sh.iter().take(10) {
        println!("    0x{v:04X} ({:5}): {c}", v);
    }
    println!();

    println!("Per-ship dump (offsets 0x1C..0x60 — past x/y):");
    let mut shown = 0;
    for (name, idx, body) in &all_records {
        // Find ship name (offset 0..28).
        let ship_name: String = body[0..28].iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect();
        if ship_name.is_empty() { continue; }
        if shown >= 12 { break; }
        let bytes_dump: Vec<String> = (0x1C..0x60).step_by(4)
            .map(|o| format!("{:02x}{:02x}{:02x}{:02x}",
                body[o+3], body[o+2], body[o+1], body[o]))
            .collect();
        println!("  {name}#{idx} \"{ship_name}\": {}", bytes_dump.join(" "));
        shown += 1;
    }
}
