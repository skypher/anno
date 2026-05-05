//! Survey haeuser.cod for every building carrying a Bauinfra
//! tag, sorted by Bauinfra → so we can see which tags are used
//! by which buildings and at what min-Bewohner level. The goal
//! is to derive the tier mapping for the cultural tags
//! (INFRA_KIRCHE, INFRA_SCHULE, INFRA_ARZT, …) that aren't
//! aliased to INFRA_STUFE_* rungs.
//!
//! Run with `cargo run --example audit_bauinfra_tags -p anno-formats`.

use anno_formats::cod::CodFile;

fn main() {
    let path = std::env::args().nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted/haeuser.cod".into());
    let bytes = std::fs::read(&path).expect("read haeuser.cod");
    let cod = CodFile::parse(&bytes).expect("parse haeuser.cod");

    // Group every building by its Bauinfra value.
    let mut by_bauinfra: std::collections::BTreeMap<String, Vec<&anno_formats::cod::BuildingDef>> =
        std::collections::BTreeMap::new();
    for b in &cod.buildings {
        let bi = match b.properties.get("Bauinfra") {
            Some(s) => s.clone(),
            None => continue,
        };
        if bi.is_empty() { continue; }
        by_bauinfra.entry(bi).or_default().push(b);
    }

    println!("Bauinfra tag → buildings using it:\n");
    for (tag, builds) in &by_bauinfra {
        println!("  {tag} ({} buildings):", builds.len());
        for b in builds.iter().take(8) {
            let bewohner = b.properties.get("Bewohner").cloned().unwrap_or_default();
            let max_b = b.properties.get("MaxBewohner").cloned().unwrap_or_default();
            println!("    Nr={:5} (Kind={})  Bewohner={bewohner}  MaxBewohner={max_b}",
                b.nummer, b.kind);
        }
        if builds.len() > 8 {
            println!("    … (+{})", builds.len() - 8);
        }
        println!();
    }

    // Specifically look at residences (Kind=WOHN) and see what
    // Bauinfra tags they carry — that's what determines the
    // tier ladder for the player.
    // Distribution of `Kind` values to understand the schema.
    let mut kind_counts: std::collections::BTreeMap<String, u32> =
        std::collections::BTreeMap::new();
    for b in &cod.buildings {
        *kind_counts.entry(b.kind.clone()).or_default() += 1;
    }
    println!("All `Kind` values and counts:");
    for (k, n) in &kind_counts {
        println!("  {k}: {n}");
    }
    println!();

    println!("Residence-like buildings (Kind=HQ) by Bauinfra tag:\n");
    let mut wohn_by_bauinfra: std::collections::BTreeMap<String, Vec<&anno_formats::cod::BuildingDef>> =
        std::collections::BTreeMap::new();
    for b in &cod.buildings {
        if b.kind != "HQ" { continue; }
        let bi = b.properties.get("Bauinfra").cloned().unwrap_or_default();
        wohn_by_bauinfra.entry(bi).or_default().push(b);
    }
    for (tag, builds) in &wohn_by_bauinfra {
        println!("  Bauinfra={tag:<24}  ({} residences)", builds.len());
        for b in builds.iter().take(5) {
            let bewohner = b.properties.get("Bewohner").cloned().unwrap_or_default();
            let max_b = b.properties.get("MaxBewohner").cloned().unwrap_or_default();
            println!("    Nr={:5}  Bewohner={bewohner}  MaxBewohner={max_b}",
                b.nummer);
        }
        if builds.len() > 5 {
            println!("    … (+{})", builds.len() - 5);
        }
        println!();
    }
}
