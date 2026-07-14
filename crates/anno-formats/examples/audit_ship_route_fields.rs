//! Inspect haeuser.cod fields used by `FUN_0046f6d0` ship-route blockers.
//!
//! Run with `cargo run --example audit_ship_route_fields -p anno-formats`.

use anno_formats::cod::CodFile;
use std::collections::{BTreeMap, BTreeSet};

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted".to_owned());
    let cod_path = format!("{root}/haeuser.cod");
    let cod = CodFile::parse(&std::fs::read(&cod_path).expect("read haeuser.cod"))
        .expect("parse haeuser.cod");

    let keys: BTreeSet<_> = cod
        .buildings
        .iter()
        .flat_map(|building| building.properties.keys())
        .filter(|key| {
            let key = key.to_ascii_lowercase();
            [
                "strand", "flag", "wasser", "meer", "schiff", "hafen", "shot", "place", "destroy",
                "grund",
            ]
            .iter()
            .any(|needle| key.contains(needle))
        })
        .cloned()
        .collect();
    println!("route-related property keys: {keys:?}");

    for key in keys {
        let mut values = BTreeMap::<String, Vec<(i32, String, i32)>>::new();
        for building in &cod.buildings {
            if let Some(value) = building.properties.get(&key) {
                values.entry(value.clone()).or_default().push((
                    building.nummer,
                    building.kind.clone(),
                    building.source_id,
                ));
            }
        }
        println!("{key}: {} distinct values", values.len());
        for (value, definitions) in values {
            let sample = definitions
                .iter()
                .take(12)
                .map(|(nummer, kind, source_id)| format!("Nr{nummer}:{kind}@{source_id}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  {value:?}: {} [{sample}]", definitions.len());
        }
    }

    let mut flagged_by_prod_kind = BTreeMap::<String, Vec<(i32, String, i32)>>::new();
    for building in &cod.buildings {
        if building.properties.get("Strandflg").is_some() {
            flagged_by_prod_kind
                .entry(
                    building
                        .properties
                        .get("ProdKind")
                        .cloned()
                        .unwrap_or_else(|| "<none>".to_owned()),
                )
                .or_default()
                .push((building.nummer, building.kind.clone(), building.source_id));
        }
    }
    println!("Strandflg definitions by ProdKind:");
    for (prod_kind, definitions) in flagged_by_prod_kind {
        let sample = definitions
            .iter()
            .take(12)
            .map(|(nummer, kind, source_id)| format!("Nr{nummer}:{kind}@{source_id}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {prod_kind}: {} [{sample}]", definitions.len());
    }

    let mut no_shot_by_route_fields = BTreeMap::<(i32, u8), Vec<(i32, String, i32)>>::new();
    for building in &cod.buildings {
        if building.no_shot {
            no_shot_by_route_fields
                .entry((
                    building.anim_anz,
                    building.source_kind_code().unwrap_or(u8::MAX),
                ))
                .or_default()
                .push((building.nummer, building.kind.clone(), building.source_id));
        }
    }
    println!("NoShotFlg definitions by (AnimAnz, source kind):");
    for ((anim_anz, source_kind), definitions) in no_shot_by_route_fields {
        let sample = definitions
            .iter()
            .take(12)
            .map(|(nummer, kind, source_id)| format!("Nr{nummer}:{kind}@{source_id}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  ({anim_anz}, {source_kind}): {} [{sample}]",
            definitions.len()
        );
    }
}
