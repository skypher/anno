//! Cross-reference plantation buildings (extracted from
//! haeuser.cod's PLANTAGE entries) against the islands they
//! appear on in shipping scenarios, to pin the fertility-byte
//! → good-name mapping empirically.
//!
//! Method: for each shipping scenario, walk every INSELHAUS
//! tile, look up the building's PLANTAGE-type and output
//! Ware, then record (fertility byte, ware) pairs from the
//! corresponding INSEL5 chunk.
//!
//! Run with `cargo run --example audit_fertility_mapping -p anno-formats`.

use anno_formats::cod::CodFile;
use anno_formats::szs::SzsFile;
use std::collections::BTreeMap;

fn main() {
    let cod_path = "/home/sky/anno/extracted/haeuser.cod";
    let cod_bytes = std::fs::read(cod_path).expect("read haeuser.cod");
    let cod = CodFile::parse(&cod_bytes).expect("parse haeuser.cod");

    // Build a map from building Nummer → (ProdKind, Ware) by
    // walking buildings whose ProdKind is PLANTAGE / ROHSTOFF
    // / HANDWERK / BERGWERK (anything that has fertility-bound
    // output).
    let mut nummer_to_ware: BTreeMap<i32, String> = BTreeMap::new();
    for b in &cod.buildings {
        let prod_kind = b.properties.get("ProdKind").cloned().unwrap_or_default();
        let ware = b.properties.get("Ware").cloned().unwrap_or_default();
        if !prod_kind.is_empty() && !ware.is_empty() {
            // Strip any ", coefficient" suffix.
            let ware_first = ware.split(',').next().unwrap_or("").trim().to_string();
            if !ware_first.is_empty() {
                nummer_to_ware.insert(b.nummer, format!("{prod_kind}:{ware_first}"));
            }
        }
    }
    println!("Loaded {} ware-producing buildings from haeuser.cod\n",
        nummer_to_ware.len());

    // Distinct wares produced by PLANTAGE buildings — the
    // climate-bound subset.
    let mut plantage_wares: BTreeMap<String, u32> = BTreeMap::new();
    for b in &cod.buildings {
        let pk = b.properties.get("ProdKind").cloned().unwrap_or_default();
        if pk != "PLANTAGE" { continue; }
        let w = b.properties.get("Ware").cloned().unwrap_or_default();
        let w0 = w.split(',').next().unwrap_or("").trim().to_string();
        if !w0.is_empty() {
            *plantage_wares.entry(w0).or_default() += 1;
        }
    }
    println!("PLANTAGE wares in haeuser.cod: {plantage_wares:?}\n");

    // Building Nummers per fertility-bound ware.
    let mut ware_to_nummers: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    for b in &cod.buildings {
        let pk = b.properties.get("ProdKind").cloned().unwrap_or_default();
        if pk != "PLANTAGE" { continue; }
        let w = b.properties.get("Ware").cloned().unwrap_or_default();
        let w0 = w.split(',').next().unwrap_or("").trim().to_string();
        if matches!(w0.as_str(), "TABAK" | "ZUCKER" | "GEWUERZE" | "KAKAO" | "WEIN" | "KORN") {
            ware_to_nummers.entry(w0).or_default().push(b.nummer);
        }
    }
    println!("Plantation Nummers per fertility-bound ware: {ware_to_nummers:?}\n");

    // Survey actual building_id values that match those Nummers
    // across all INSELHAUS tiles.
    let target_nummers: std::collections::BTreeSet<i32> = ware_to_nummers
        .values().flat_map(|v| v.iter().copied()).collect();
    let mut nummer_appearances: BTreeMap<i32, u32> = BTreeMap::new();
    let mut nummer_to_islands: BTreeMap<i32, Vec<(String, [u8; 8])>> = BTreeMap::new();
    let scan_dir = "/home/sky/anno/extracted/Szenes";
    for entry in std::fs::read_dir(scan_dir).unwrap().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.extension().map(|s| s.eq_ignore_ascii_case("szs")).unwrap_or(false) { continue; }
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        for island in &parsed.islands {
            let mut found_nummers: std::collections::BTreeSet<i32> = Default::default();
            for tile in &island.tiles {
                let n = tile.building_id as i32;
                if target_nummers.contains(&n) {
                    found_nummers.insert(n);
                    *nummer_appearances.entry(n).or_default() += 1;
                }
            }
            for n in &found_nummers {
                nummer_to_islands.entry(*n).or_default()
                    .push((stem.clone(), island.fertilities));
            }
        }
    }
    println!("Plantation Nummer appearance counts: {nummer_appearances:?}\n");
    println!("Per-plantation island fertilities (first 5 each):");
    for (n, isles) in &nummer_to_islands {
        let ware = ware_to_nummers.iter()
            .find(|(_, v)| v.contains(n))
            .map(|(k, _)| k.clone())
            .unwrap_or_default();
        println!("  Nummer {n} ({ware}):");
        for (scen, ferts) in isles.iter().take(5) {
            println!("    {scen}: {ferts:?}");
        }
        if isles.len() > 5 {
            println!("    … ({} more islands)", isles.len() - 5);
        }
    }
    println!();

    let dir = "/home/sky/anno/extracted/Szenes";
    let mut fert_to_ware: BTreeMap<u8, BTreeMap<String, u32>> = BTreeMap::new();

    for entry in std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.extension().map(|s| s.eq_ignore_ascii_case("szs")).unwrap_or(false) { continue; }
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        for island in &parsed.islands {
            // Skip islands with no fertilities.
            let actives: Vec<u8> = island.fertilities.iter()
                .copied()
                .filter(|&v| v != 7)
                .collect();
            if actives.is_empty() { continue; }
            // For each tile, if it's a plantation/raw-resource
            // for a fertility-BOUND ware, record the pairing.
            // We restrict to typical southern wares since those
            // are climate-gated (TABAK, ZUCKERROHR, GEWUERZE,
            // BAUMWOLLE, KAKAO, WEIN) plus northern climate
            // gates KORN if any. Wood/wool are universal so we
            // skip them.
            let fertility_bound = |w: &str| {
                matches!(w, "TABAK" | "ZUCKER" | "GEWUERZE" | "KAKAO" | "KORN")
            };
            for tile in &island.tiles {
                if let Some(ware_full) = nummer_to_ware.get(&(tile.building_id as i32)) {
                    let ware = ware_full.split(':').nth(1).unwrap_or("");
                    if !fertility_bound(ware) { continue; }
                    for &f in &actives {
                        *fert_to_ware.entry(f).or_default()
                            .entry(ware_full.clone()).or_default() += 1;
                    }
                }
            }
        }
    }

    println!("Fertility-byte → ware co-occurrence counts:");
    println!("(higher = more likely the byte gates that ware)\n");
    for (fert, wares) in &fert_to_ware {
        println!("  fertility 0x{fert:02X}:");
        let mut sorted: Vec<_> = wares.iter().collect();
        sorted.sort_by_key(|&(_, c)| std::cmp::Reverse(*c));
        for (ware, count) in sorted.iter().take(10) {
            println!("    {ware}: {count}");
        }
        println!();
    }
}
