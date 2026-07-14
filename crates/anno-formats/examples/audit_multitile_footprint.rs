//! Survey multi-tile INSELHAUS commands in the source-definition namespace.
//!
//! INSELHAUS `building_id` is an offset from `0x4e20`, not a STADTFLD GFX
//! index. The executable loads each command through `FUN_00465170` and
//! `FUN_004653a0`, which reconstruct a rotated live-map footprint. This audit
//! measures the authored command records and the coordinate overlap that the
//! loader must resolve; it makes no GFX-based footprint claim.

use anno_formats::cod::CodFile;
use anno_formats::szs::SzsFile;
use std::collections::HashMap;

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/sky/anno/extracted".to_owned());
    let cod = CodFile::parse(&std::fs::read(format!("{root}/haeuser.cod")).expect("read COD"))
        .expect("parse COD");
    let scenes = format!("{root}/Szenes");

    let mut records = 0_u64;
    let mut multi_tile_records = 0_u64;
    let mut occupied_footprint_cells = 0_u64;
    let mut duplicate_coordinate_records = 0_u64;

    for entry in std::fs::read_dir(scenes)
        .expect("read scenarios")
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("szs"))
        {
            continue;
        }
        let szs = SzsFile::parse(&std::fs::read(&path).expect("read scenario"))
            .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
        for island in &szs.islands {
            let mut records_at = HashMap::<(u8, u8), usize>::new();
            for tile in &island.tiles {
                *records_at.entry((tile.x, tile.y)).or_default() += 1;
            }
            duplicate_coordinate_records += records_at
                .values()
                .map(|count| count.saturating_sub(1) as u64)
                .sum::<u64>();

            for tile in &island.tiles {
                records += 1;
                let definition = cod
                    .building_by_source_id(tile.source_id())
                    .unwrap_or_else(|| {
                        panic!(
                            "unresolved source definition {} in {} island {} at ({}, {})",
                            tile.source_id(),
                            path.display(),
                            island.number,
                            tile.x,
                            tile.y,
                        )
                    });
                let (width, height) = if matches!(tile.orientation & 3, 1 | 3) {
                    (definition.size.1, definition.size.0)
                } else {
                    definition.size
                };
                if width <= 1 && height <= 1 {
                    continue;
                }
                multi_tile_records += 1;
                for dy in 0..height {
                    for dx in 0..width {
                        let Some(x) = tile.x.checked_add(dx as u8) else {
                            continue;
                        };
                        let Some(y) = tile.y.checked_add(dy as u8) else {
                            continue;
                        };
                        if records_at.contains_key(&(x, y)) {
                            occupied_footprint_cells += 1;
                        }
                    }
                }
            }
        }
    }

    println!("INSELHAUS command records: {records}");
    println!("multi-tile source commands: {multi_tile_records}");
    println!(
        "authored records lying in their oriented footprint cells: {occupied_footprint_cells}"
    );
    println!("duplicate records at one island coordinate: {duplicate_coordinate_records}");
}
