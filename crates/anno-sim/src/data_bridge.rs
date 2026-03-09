//! Bridge between parsed game files (anno-formats) and simulation data structures.
//!
//! Converts COD building definitions and SZS scenario data into the types
//! used by the simulation engine.

use std::collections::HashMap;

use anno_formats::cod::{BuildingDef as CodBuilding, CodFile};
use anno_formats::szs::SzsFile;

use crate::building::{BuildingDef, BuildingInstance};
use crate::types::Good;

/// Map COD good names to simulation Good enum.
fn parse_good(name: &str) -> Good {
    match name {
        "HOLZ" => Good::Wood,
        "EISEN" => Good::Iron,
        "EISENERZ" | "ERZE" => Good::Ore,
        "GOLD" => Good::Gold,
        "GOLDERZ" => Good::GoldOre,
        "WOLLE" => Good::Wool,
        "ZUCKER" | "ZUCKERROHR" => Good::Sugar,
        "TABAK" => Good::Tobacco,
        "RIND" | "FLEISCH" | "VIEH" => Good::Cattle,
        "GETREIDE" | "KORN" => Good::Grain,
        "MEHL" => Good::Flour,
        "WERKZEUG" => Good::Tools,
        "ZIEGEL" => Good::Bricks,
        "STEINE" | "STEIN" => Good::Stone,
        "SCHWERT" | "SCHWERTER" => Good::Swords,
        "MUSKETE" | "MUSKETEN" => Good::Muskets,
        "KANONE" | "KANONEN" => Good::Cannons,
        "NAHRUNG" => Good::Food,
        "STOFFE" | "STOFF" | "TUCH" => Good::Cloth,
        "ALKOHOL" | "RUM" => Good::Alcohol,
        "TABAKWAREN" | "TABAKWARE" => Good::TobaccoProducts,
        "GEWUERZ" | "GEWUERZE" => Good::Spices,
        "KAKAO" => Good::Cocoa,
        "WEINTRAUBEN" | "WEIN" => Good::Grapes,
        "HAEUTE" => Good::Hides,
        "BAUMWOLLE" => Good::Cotton,
        "SEIDE" => Good::Silk,
        "SCHMUCK" => Good::Jewelry,
        "KLEIDUNG" => Good::Clothing,
        "FISCHE" | "FISCH" => Good::Fish,
        _ => Good::None,
    }
}

/// Convert a COD building definition into a simulation BuildingDef.
fn convert_building_def(cod_building: &CodBuilding) -> BuildingDef {
    let prop = |key: &str| -> &str {
        cod_building
            .properties
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or("")
    };

    let prop_int = |key: &str| -> i32 {
        let s = prop(key);
        s.parse::<i32>().unwrap_or(0)
    };

    // Ware/Rohstoff values may have comma-separated coefficients: "ALKOHOL, 0.5"
    // Extract just the good name (first token)
    let good_name = |key: &str| -> &str {
        let val = prop(key);
        val.split(',').next().unwrap_or(val).trim()
    };

    let output_good = parse_good(good_name("Ware"));
    let input_good_1 = parse_good(good_name("Rohstoff"));
    let input_good_2 = parse_good(good_name("Workstoff"));

    let interval = prop_int("Interval").max(1) as u16;
    let maxlager = prop_int("Maxlager").max(0) as u16;
    // Only set input rates if the corresponding input good exists
    let rohmenge = if input_good_1 != Good::None {
        prop_int("Rohmenge").max(0) as u16
    } else {
        0
    };
    let workmenge = if input_good_2 != Good::None {
        prop_int("Workmenge").max(0) as u16
    } else {
        0
    };

    // Construction costs from HAUS_BAUKOST sub-object
    let cost_gold = prop_int("Money").max(0) as u32;
    let cost_tools = prop_int("Werkzeug").max(0) as u16;
    let cost_wood = prop_int("Holz").max(0) as u16;
    let cost_bricks = prop_int("Ziegel").max(0) as u16;

    // Maintenance from Kosten property (comma-separated, first value is the cost constant)
    // For now use a flat estimate based on production kind
    let prod_kind_str = prop("ProdKind");
    let maintenance = match prod_kind_str {
        "HANDWERK" => 5,
        "ROHSTOFF" | "PLANTAGE" | "STEINBRUCH" | "BERGWERK" | "JAGDHAUS" | "FISCHEREI" => 3,
        "WEIDETIER" | "ROHSTWACHS" => 2,
        _ => 0,
    };

    // Resolve Radius property (may be a number or a constant name)
    let radius_raw = prop("Radius");
    let radius = if let Ok(n) = radius_raw.parse::<i32>() {
        n.max(0) as u16
    } else {
        // Hardcoded constants from original binary (not defined in COD file)
        match radius_raw {
            "RADIUS_MARKT" => 30,
            "RADIUS_HQ" => 22,
            _ => 0,
        }
    };

    BuildingDef {
        id: cod_building.nummer as u16,
        category: 0, // TODO: map from Kind
        width: cod_building.size.0 as u8,
        height: cod_building.size.1 as u8,
        production_type: crate::types::ProductionType::Craft, // TODO: map from Kind
        kind: cod_building.kind.clone(),
        prod_kind: prod_kind_str.to_string(),
        radius,
        output_good,
        input_good_1,
        input_good_2,
        output_rate: 1, // Each cycle produces 1 unit of output
        input_1_rate: rohmenge,
        input_2_rate: workmenge,
        storage_capacity: maxlager,
        cycle_time_ms: interval as u32 * 999, // Interval is in production ticks (each ~999ms)
        carrier_interval_ms: 5000,
        cost_gold,
        cost_tools,
        cost_wood,
        cost_bricks,
        maintenance_cost: maintenance,
    }
}

/// Load all building definitions from a parsed COD file.
pub fn load_building_defs(cod: &CodFile) -> Vec<BuildingDef> {
    cod.buildings.iter().map(|b| convert_building_def(b)).collect()
}

/// Build a lookup from COD Nummer → index into building_defs vec.
pub fn nummer_to_def_index(cod: &CodFile) -> HashMap<i32, usize> {
    let mut map = HashMap::new();
    for (i, b) in cod.buildings.iter().enumerate() {
        map.entry(b.nummer).or_insert(i);
    }
    map
}

/// Build a lookup from COD Gfx (sprite index) → index into building_defs vec.
pub fn gfx_to_def_index(cod: &CodFile) -> HashMap<i32, usize> {
    cod.gfx_to_building_map()
}

/// Production kind strings that indicate a building can produce goods.
const PRODUCTION_KINDS: &[&str] = &[
    "HANDWERK",
    "ROHSTOFF",
    "PLANTAGE",
    "BERGWERK",
    "STEINBRUCH",
    "JAGDHAUS",
    "FISCHEREI",
    "WEIDETIER",
    "ROHSTWACHS",
    "ROHSTERZ",
];

/// Check if a COD building definition is a production building.
fn is_production_building(cod_building: &CodBuilding) -> bool {
    if let Some(prod_kind) = cod_building.properties.get("ProdKind") {
        PRODUCTION_KINDS.iter().any(|&k| prod_kind == k)
    } else {
        false
    }
}

/// Load building instances from a parsed SZS scenario file.
///
/// Maps each INSELHAUS tile that has a matching building definition
/// (via sprite index → COD gfx lookup) into a BuildingInstance.
/// Only creates instances for production buildings (those with production ProdKind).
pub fn load_building_instances(
    szs: &SzsFile,
    cod: &CodFile,
    building_defs: &[BuildingDef],
) -> Vec<BuildingInstance> {
    let gfx_map = gfx_to_def_index(cod);
    let mut instances = Vec::new();

    for island in &szs.islands {
        for tile in &island.tiles {
            let sprite_idx = tile.building_id as i32;

            // Look up which building def this sprite belongs to
            if let Some(&def_idx) = gfx_map.get(&sprite_idx) {
                let cod_building = &cod.buildings[def_idx];

                // Only create instances for actual production buildings
                if !is_production_building(cod_building) {
                    continue;
                }

                let def = &building_defs[def_idx];
                // Skip terrain/decoration tiles (GRAS, NOWARE, BAUM, etc.)
                if def.output_good == Good::None {
                    continue;
                }

                // Skip duplicate tiles for multi-tile buildings
                // (only the base tile at the building's gfx index creates an instance)
                if sprite_idx != cod_building.gfx {
                    continue;
                }

                let instance = BuildingInstance::new(
                    def_idx as u16,
                    island.number,
                    tile.x as u16,
                    tile.y as u16,
                    0, // owner unknown from SZS alone
                );
                instances.push(instance);
            }
        }
    }

    instances
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_defs_from_cod() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/haeuser.cod");

        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: haeuser.cod not found");
                return;
            }
        };

        let cod = CodFile::parse(&data).unwrap();
        let defs = load_building_defs(&cod);

        assert_eq!(defs.len(), cod.buildings.len());

        // Find production buildings (those with actual output goods)
        let production: Vec<_> = defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.output_good != Good::None)
            .collect();

        println!("Total defs: {}", defs.len());
        println!("Production buildings: {}", production.len());

        // Print some production buildings
        for (i, d) in production.iter().take(10) {
            let cod_b = &cod.buildings[*i];
            println!(
                "  #{} (cod #{}) {:?} → {:?} (input: {:?} x{}, {:?} x{}) interval={}ms storage={}",
                i,
                cod_b.nummer,
                d.output_good,
                cod_b.properties.get("Ware").unwrap_or(&"?".into()),
                d.input_good_1,
                d.input_1_rate,
                d.input_good_2,
                d.input_2_rate,
                d.cycle_time_ms,
                d.storage_capacity,
            );
        }

        assert!(
            production.len() >= 20,
            "expected >= 20 production buildings"
        );
    }

    #[test]
    fn load_scenario_buildings() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();

        let cod_data = match std::fs::read(base.join("extracted/haeuser.cod")) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: haeuser.cod not found");
                return;
            }
        };

        // Find any .szs file
        let szenes_dir = base.join("extracted/Szenes");
        let szs_path = match std::fs::read_dir(&szenes_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .find(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .ends_with(".szs")
                })
                .map(|e| e.path()),
            Err(_) => None,
        };

        let szs_path = match szs_path {
            Some(p) => p,
            None => {
                println!("Skipping: no .szs files found");
                return;
            }
        };

        let cod = CodFile::parse(&cod_data).unwrap();
        let defs = load_building_defs(&cod);
        let szs_data = std::fs::read(&szs_path).unwrap();
        let szs = SzsFile::parse(&szs_data).unwrap();

        let instances = load_building_instances(&szs, &cod, &defs);
        println!(
            "Scenario '{}': {} production building instances",
            szs_path.file_stem().unwrap().to_string_lossy(),
            instances.len()
        );

        for inst in instances.iter().take(10) {
            let def = &defs[inst.def_id as usize];
            println!(
                "  island={} pos=({},{}) output={:?} storage={}",
                inst.island_id, inst.tile_x, inst.tile_y, def.output_good, def.storage_capacity,
            );
        }
    }
}
