//! Compare INSELHAUS raw IDs against the haeuser.cod definition-ID and Gfx
//! namespaces across the shipping scenario corpus.
//!
//! Run with `cargo run --example audit_inselhaus_gfx -p anno-formats`.

use anno_formats::cod::CodFile;
use anno_formats::szs::SzsFile;
use std::collections::{BTreeMap, HashSet};

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted".to_owned());
    let cod_path = format!("{root}/haeuser.cod");
    let cod = CodFile::parse(&std::fs::read(&cod_path).expect("read haeuser.cod"))
        .expect("parse haeuser.cod");
    let known_source_ids: HashSet<i32> = cod
        .buildings
        .iter()
        .map(|building| building.source_id)
        .collect();
    let known_gfx: HashSet<i32> = cod.buildings.iter().map(|building| building.gfx).collect();

    let mut total = 0_u64;
    let mut source_id_matched = 0_u64;
    let mut gfx_matched = 0_u64;
    let mut both_matched = 0_u64;
    let mut source_id_uses = BTreeMap::<i32, u64>::new();
    let mut unmatched = BTreeMap::<u16, u64>::new();
    let scenes = format!("{root}/Szenes");

    for entry in std::fs::read_dir(scenes)
        .expect("read Szenes")
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("szs"))
        {
            continue;
        }
        let scenario = match SzsFile::parse(&std::fs::read(&path).expect("read scenario")) {
            Ok(scenario) => scenario,
            Err(error) => {
                eprintln!("skip {}: {error}", path.display());
                continue;
            }
        };
        for tile in scenario.islands.iter().flat_map(|island| &island.tiles) {
            total += 1;
            let source_id_match = known_source_ids.contains(&tile.source_id());
            let gfx_match = known_gfx.contains(&i32::from(tile.building_id));
            if source_id_match {
                source_id_matched += 1;
                *source_id_uses.entry(tile.source_id()).or_default() += 1;
            }
            if gfx_match {
                gfx_matched += 1;
            }
            if source_id_match && gfx_match {
                both_matched += 1;
            }
            if !source_id_match {
                *unmatched.entry(tile.building_id).or_default() += 1;
            }
        }
    }

    println!("INSELHAUS tile records: {total}");
    println!("raw + 20000 source-ID matches: {source_id_matched}");
    println!("raw Gfx matches: {gfx_matched}");
    println!("matches in both namespaces: {both_matched}");
    println!("unmatched source IDs: {}", total - source_id_matched);
    println!("distinct unmatched raw IDs: {}", unmatched.len());

    let mut by_source_id = BTreeMap::<i32, Vec<_>>::new();
    for building in &cod.buildings {
        by_source_id
            .entry(building.source_id)
            .or_default()
            .push(building);
    }
    let duplicate_ids: Vec<_> = by_source_id
        .iter()
        .filter(|(_, definitions)| definitions.len() > 1)
        .collect();
    let route_ambiguous_ids: Vec<_> = duplicate_ids
        .iter()
        .filter(|(_, definitions)| {
            let first = definitions[0];
            definitions.iter().skip(1).any(|definition| {
                definition.kind != first.kind
                    || definition.size != first.size
                    || definition.source_path_classes() != first.source_path_classes()
            })
        })
        .collect();
    println!("duplicate source IDs: {}", duplicate_ids.len());
    println!("duplicates with route-relevant differences: {}", route_ambiguous_ids.len());
    for (source_id, definitions) in route_ambiguous_ids.iter().take(20) {
        let details = definitions
            .iter()
            .map(|definition| {
                format!(
                    "Nr={} kind={} size={:?} paths={:?}",
                    definition.nummer,
                    definition.kind,
                    definition.size,
                    definition.source_path_classes(),
                )
            })
            .collect::<Vec<_>>();
        println!(
            "  {source_id}: uses={}; {}",
            source_id_uses.get(source_id).copied().unwrap_or(0),
            details.join("; "),
        );
    }

    println!("most frequent unmatched raw IDs:");
    let mut frequent: Vec<_> = unmatched.into_iter().collect();
    frequent.sort_unstable_by_key(|&(_, count)| std::cmp::Reverse(count));
    for (gfx, count) in frequent.into_iter().take(20) {
        let predecessor = cod
            .buildings
            .iter()
            .filter(|building| building.source_id <= 0x4e20 + i32::from(gfx))
            .max_by_key(|building| building.source_id);
        if let Some(building) = predecessor {
            println!(
                "  raw {gfx}: {count}; preceding Nr={} source_id={} kind={} size={:?} rotate={} anim=({}, {}) rand=({}, {})",
                building.nummer,
                building.source_id,
                building.kind,
                building.size,
                building.rotate,
                building.anim_anz,
                building.anim_add,
                building.rand_anz,
                building.rand_add,
            );
        } else {
            println!("  Gfx {gfx}: {count}; no preceding COD definition");
        }
    }

    println!("compiled-ID candidates 21200..=21220:");
    for source_id in 21_200..=21_220 {
        let candidates: Vec<_> = cod
            .buildings
            .iter()
            .filter(|building| building.source_id == source_id)
            .map(|building| {
                format!(
                    "Nr={} Gfx={} {}",
                    building.nummer, building.gfx, building.kind
                )
            })
            .collect();
        if !candidates.is_empty() {
            println!("  {source_id}: {}", candidates.join(", "));
        }
    }
    println!("parsed records 175..=190:");
    for building in cod
        .buildings
        .iter()
        .filter(|building| (175..=190).contains(&building.nummer))
    {
        println!(
            "  Nr={} source_id={} Gfx={} {}",
            building.nummer, building.source_id, building.gfx, building.kind
        );
    }
}
