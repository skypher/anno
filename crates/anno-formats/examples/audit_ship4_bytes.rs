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
