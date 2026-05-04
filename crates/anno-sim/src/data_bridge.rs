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
///
/// Covers all 25 player-facing goods from `text.cod [WARE]` plus the
/// raw-plantation crops the engine references in production chains
/// (BAUM = forest tree, KAKAOBAUM = cocoa tree, TABAKBAUM = tobacco
/// plant, GEWUERZBAUM = spice tree, ZUCKERROHR = sugarcane). These
/// plant tokens aren't separate economic goods; they map to the
/// finished-good enum the player sees in their warehouse so the
/// production tick can consume them as an input. ALLWARE / NOWARE /
/// GRAS are flag/terrain pseudo-goods → `Good::None`.
fn parse_good(name: &str) -> Good {
    // Mapping keys verified against the four shipping COD files
    // (haeuser.cod, figuren.cod, text.cod, editor.cod). Singular
    // forms (`SCHWERT`, `STEIN`, etc.), `RUM`, `WEIN`, `GOLDERZ`
    // and other speculative spellings were removed because the
    // strings never appear in the data — defensive mappings to
    // strings the game cannot emit are dead code.
    match name {
        "HOLZ" | "BAUM" => Good::Wood,
        "EISEN" => Good::Iron,
        "EISENERZ" | "ERZE" => Good::Ore,
        "GOLD" => Good::Gold,
        "WOLLE" => Good::Wool,
        "ZUCKER" => Good::Sugar,
        "ZUCKERROHR" => Good::SugarCane,
        "TABAK" | "TABAKBAUM" => Good::Tobacco,
        "RIND" => Good::Cattle,
        "FLEISCH" => Good::Meat,
        "GETREIDE" | "KORN" => Good::Grain,
        "MEHL" => Good::Flour,
        "WERKZEUG" => Good::Tools,
        "ZIEGEL" => Good::Bricks,
        "STEINE" => Good::Stone,
        "SCHWERTER" => Good::Swords,
        "MUSKETEN" => Good::Muskets,
        "KANONEN" => Good::Cannons,
        "NAHRUNG" => Good::Food,
        "STOFFE" => Good::Cloth,
        "ALKOHOL" => Good::Alcohol,
        "TABAKWAREN" => Good::TobaccoProducts,
        "GEWUERZE" | "GEWUERZBAUM" => Good::Spices,
        "KAKAO" | "KAKAOBAUM" => Good::Cocoa,
        "WEINTRAUBEN" => Good::Grapes,
        "WILD" => Good::WildGame,
        "BAUMWOLLE" => Good::Cotton,
        "SEIDE" => Good::Silk,
        "SCHMUCK" => Good::Jewelry,
        "KLEIDUNG" => Good::Clothing,
        "FISCHE" => Good::Fish,
        // Flags / terrain pseudo-goods that don't correspond to a
        // player-facing good slot.
        "NOWARE" | "ALLWARE" | "GRAS" | "" => Good::None,
        _ => Good::None,
    }
}

#[cfg(test)]
#[test]
fn parse_good_covers_all_haeuser_cod_tokens() {
    // Tokens enumerated from `extracted/haeuser.cod` Ware: / Rohstoff:
    // / Workstoff: lines (excluding the literal "ALLWARE" / "NOWARE"
    // wildcards which always resolve to None).
    let tokens = [
        "ALKOHOL", "BAUM", "BAUMWOLLE", "EISEN", "EISENERZ", "ERZE",
        "FISCHE", "FLEISCH", "GETREIDE", "GEWUERZBAUM", "GEWUERZE",
        "GOLD", "GRAS", "HOLZ", "KAKAO", "KAKAOBAUM", "KANONEN",
        "KLEIDUNG", "KORN", "MEHL", "MUSKETEN", "NAHRUNG",
        "SCHMUCK", "SCHWERTER", "STEINE", "STOFFE", "TABAK",
        "TABAKBAUM", "TABAKWAREN", "WEINTRAUBEN", "WERKZEUG", "WILD",
        "WOLLE", "ZIEGEL", "ZUCKER", "ZUCKERROHR",
    ];
    for tok in tokens {
        let g = parse_good(tok);
        // GRAS is the only one that legitimately maps to None.
        if tok != "GRAS" {
            assert_ne!(g, Good::None,
                "token {tok} should map to a real good");
        }
    }
}

#[cfg(test)]
#[test]
fn parse_bauinfra_matches_haeuser_cod_ladder() {
    // Aliases from haeuser.cod's `BESONDERE INFRASTRUKTUR
    // MARKPUNKTE` block, paired with the PopTier index they
    // resolve to (digit - 1 of the STUFE rung).
    let cases: &[(&str, u8)] = &[
        ("INFRA_KONTOR_1", 1), // Settler  (= INFRA_STUFE_2B)
        ("INFRA_BURG_1",   1), // Settler  (= INFRA_STUFE_2G)
        ("INFRA_WACHTURM", 1), // Settler  (= INFRA_STUFE_2G)
        ("INFRA_KONTOR_2", 2), // Citizen  (= INFRA_STUFE_3A)
        ("INFRA_KANON",    2), // Citizen  (= INFRA_STUFE_3E)
        ("INFRA_KONTOR_3", 3), // Merchant (= INFRA_STUFE_4A)
        ("INFRA_BURG_2",   3), // Merchant (= INFRA_STUFE_4B)
        ("INFRA_MUSKETE",  3), // Merchant (= INFRA_STUFE_4B)
        ("INFRA_BURG_3",   4), // Aristo   (= INFRA_STUFE_5B)
        // Direct STUFE tokens.
        ("INFRA_STUFE_1A", 0),
        ("INFRA_STUFE_5A", 4),
        ("",               0),
        // Cultural-building tags (no marker-table alias defined).
        ("INFRA_KIRCHE",   0),
        ("INFRA_SCHULE",   0),
    ];
    for (tok, want) in cases {
        let got = parse_bauinfra(tok);
        assert_eq!(got, *want, "parse_bauinfra({tok:?}) = {got}, want {want}");
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

    // Per-building maintenance cost. RE: anno-capsu.netlify.app
    // /1602/production_efficiency.html lists the per-building
    // maintenance "cost" for every production building. The COD
    // doesn't store it as a field — it's looked up by building
    // type at runtime — so we map by output_good first, then fall
    // back to a prod-kind default.
    let prod_kind_str = prop("ProdKind");
    use crate::types::Good;
    let maintenance: u16 = match output_good {
        // Food chain.
        Good::Food => match prod_kind_str {
            "FISCHEREI" | "JAGDHAUS" => 5,
            _ => 5, // Bakery / Butcher per appendix
        },
        Good::Flour => 5,                 // Windmill
        Good::Grain | Good::Cattle => 5,  // Plantation farms
        Good::Cotton | Good::Wool => 10,  // Wool/cotton farms
        // Heavy industry.
        Good::Cannons => 60,
        Good::Muskets => 45,
        Good::Swords => 30,
        Good::Iron => 25,
        Good::Tools => 25,
        Good::Ore => 60,
        Good::Gold => 60,
        Good::Jewelry => 45,
        // Cloth chain.
        Good::Cloth => 20,                // Weaving mill / weaver / tailor
        Good::Clothing => 20,
        // Plantations.
        Good::Sugar | Good::Tobacco | Good::Spices | Good::Cocoa => 5,
        Good::Wood => 5,                  // Forester
        Good::Stone | Good::Bricks => 25,
        // Drinks.
        Good::Alcohol => 15,              // Distillery
        Good::Grapes => 35,               // Winery
        Good::TobaccoProducts => 25,
        Good::WildGame => 5,
        Good::Silk => 20,
        Good::Fish => 5,
        // Non-production buildings (markets etc.) — fall through.
        _ => match prod_kind_str {
            "HANDWERK" => 5,
            "ROHSTOFF" | "PLANTAGE" | "STEINBRUCH" | "BERGWERK"
            | "JAGDHAUS" | "FISCHEREI" => 3,
            "WEIDETIER" | "ROHSTWACHS" => 2,
            _ => 0,
        },
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

    // Map COD building Kind / HAUS_PRODTYP Kind to internal category
    // and ProductionType. Kinds enumerated from haeuser.cod (top-level
    // `Kind:` values).
    use crate::types::ProductionType;
    let category: u8 = match cod_building.kind.as_str() {
        // Terrain / nature.
        "BODEN" | "WALD" | "FELS" | "STRAND" | "MEER" | "FLUSS"
        | "FLUSSECK" | "MUENDUNG" | "HANG" | "HANGECK" | "HANGQUELL"
        | "BRANDUNG" | "BRANDECK" | "STRANDECKA" | "STRANDECKI"
        | "STRANDMUND" | "STRANDRUINE" | "STRANDVARI" | "STRANDHAUS"
        | "WEIDETIER" | "MAUERSTRAND" | "TURMSTRAND" => 0,
        // Residential.
        "WOHNUNG" | "PIRATWOHN" => 1,
        // Production / industry / raw materials.
        "HANDWERK" | "PLANTAGE" | "ROHSTOFF" | "ROHSTERZ"
        | "ROHSTWACHS" | "BERGWERK" | "STEINBRUCH" | "MINE"
        | "JAGDHAUS" | "FISCHEREI" | "WMUEHLE" => 2,
        // Public services and culture.
        "KAPELLE" | "KIRCHE" | "SCHULE" | "HOCHSCHULE" | "THEATER"
        | "BADEHAUS" | "BRUNNEN" | "WIRT" | "DENKMAL" | "TRIUMPH"
        | "KLINIK" | "GALGEN" | "MARKT" | "PLATZ" => 3,
        // Trade / harbour.
        "KONTOR" | "HAFEN" | "WERFT" | "PIER" => 4,
        // Military.
        "MILITAR" | "MAUER" | "TOR" | "TURM" | "WACHTURM" | "SCHLOSS" => 5,
        // Transport.
        "STRASSE" | "BRUECKE" => 6,
        // Generic / catch-all.
        _ => 7,
    };
    let production_type = match prod_kind_str {
        "HANDWERK" | "BAECKER" => ProductionType::Craft,
        "PLANTAGE" | "ROHSTOFF" | "ROHSTERZ" | "ROHSTWACHS"
        | "JAGDHAUS" | "FISCHEREI" | "WEIDETIER" => ProductionType::Plantation,
        "BERGWERK" | "STEINBRUCH" | "MINE" => ProductionType::Mine,
        "WOHNUNG" => ProductionType::Residence,
        _ => ProductionType::Craft,
    };

    BuildingDef {
        id: cod_building.nummer as u16,
        category,
        width: cod_building.size.0 as u8,
        height: cod_building.size.1 as u8,
        production_type,
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
        // `Interval` from haeuser.cod counts production ticks. The
        // game-loop tick is exactly 1000 ms (decompiled binary uses
        // `-1000` decrement on the production-cycle accumulator at
        // `1602_exe.c:16110`), not 999.
        cycle_time_ms: interval as u32 * 1000,

        cost_gold,
        cost_tools,
        cost_wood,
        cost_bricks,
        maintenance_cost: maintenance,
        native: prop("Nativflg") == "1",
        min_tier: parse_bauinfra(prop("Bauinfra")),
        max_no_input_ticks: {
            let v = prop_int("Maxnorohst");
            if v > 0 { (v as u8).min(255) } else { 6 }
        },
        can_dry_up: prop("Doerrflg") == "1",
        wegspeed: {
            let raw = prop("Wegspeed");
            let mut quad = [100u16; 4];
            for (i, tok) in raw.split(',').map(str::trim).enumerate().take(4) {
                if let Ok(v) = tok.parse::<u16>() { quad[i] = v; }
            }
            quad
        },
        has_door: prop("Tuerflg") == "1",
        upgradeable: prop("Ausbauflg") == "1",
        max_energy: {
            let v = prop_int("Maxenergy");
            if v > 0 { v as u16 } else { 0 }
        },
        ore_deposit: match prop("Erzbergnr") {
            "ERZBERG_KLEIN" => crate::building::OreDeposit::Small,
            "ERZBERG_GROSS" => crate::building::OreDeposit::Large,
            _ => crate::building::OreDeposit::None,
        },
        pirate_owned: prop("Piratflg") == "1",
        defensive_cannons: prop_int("Kanon").max(0) as u8,
    }
}

/// Map a `Bauinfra` token from haeuser.cod to a population tier
/// requirement (0..=4, matching PopTier). Tier 0 = no requirement.
///
/// `INFRA_STUFE_NX` uses N as the tier digit (1..5 → Pioneer..
/// Aristocrat); the letter suffix groups variants within a tier.
///
/// Aliases like `INFRA_BURG_1`, `INFRA_KONTOR_1`, `INFRA_KANON`,
/// etc. are direct-substitution constants defined at the top of
/// haeuser.cod (`BESONDERE INFRASTRUKTUR MARKPUNKTE` block).
/// Their tier values come straight from that ladder, NOT from
/// general-knowledge guesses:
///
///   INFRA_BURG_1   = INFRA_STUFE_2G  (Settler)
///   INFRA_WACHTURM = INFRA_STUFE_2G  (Settler)
///   INFRA_KONTOR_1 = INFRA_STUFE_2B  (Settler)
///   INFRA_KONTOR_2 = INFRA_STUFE_3A  (Citizen)
///   INFRA_KANON    = INFRA_STUFE_3E  (Citizen)
///   INFRA_KONTOR_3 = INFRA_STUFE_4A  (Merchant)
///   INFRA_BURG_2   = INFRA_STUFE_4B  (Merchant)
///   INFRA_MUSKETE  = INFRA_STUFE_4B  (Merchant)
///   INFRA_BURG_3   = INFRA_STUFE_5B  (Aristocrat)
///
/// Cultural-building tags (INFRA_KIRCHE, INFRA_SCHULE, INFRA_ARZT,
/// INFRA_BADE, INFRA_THEATER, INFRA_TRIUMPH, INFRA_DENKMAL,
/// INFRA_HOCHSCHULE, INFRA_KATHETRALE, INFRA_SCHLOSS,
/// INFRA_GALGEN, INFRA_WIRT) are not aliased to STUFE rungs in
/// haeuser.cod's marker block — they appear directly as Bauinfra
/// values on residences, and the binary's ladder resolver maps
/// them at runtime from a separate table we haven't located.
/// Until that table is RE'd, these return 0 (no tier gate).
fn parse_bauinfra(token: &str) -> u8 {
    if token.is_empty() { return 0; }
    let resolved = resolve_infra_alias(token).unwrap_or(token);
    if let Some(rest) = resolved.strip_prefix("INFRA_STUFE_") {
        // First char is the tier digit (1..5 → Pioneer..Aristocrat).
        if let Some(c) = rest.chars().next() {
            if let Some(d) = c.to_digit(10) {
                return (d.saturating_sub(1).min(4)) as u8;
            }
        }
    }
    0
}

/// Substitute the INFRASTRUKTUR-MARKPUNKTE aliases (BURG / KONTOR /
/// WACHTURM / MUSKETE / KANON) for their STUFE rungs. Returns
/// `None` for tokens that aren't in the alias table (the caller
/// then keeps the original token, which is itself an INFRA_STUFE_*
/// rung if the parser is supposed to succeed).
fn resolve_infra_alias(token: &str) -> Option<&'static str> {
    Some(match token {
        "INFRA_BURG_1"   => "INFRA_STUFE_2G",
        "INFRA_BURG_2"   => "INFRA_STUFE_4B",
        "INFRA_BURG_3"   => "INFRA_STUFE_5B",
        "INFRA_WACHTURM" => "INFRA_STUFE_2G",
        "INFRA_MUSKETE"  => "INFRA_STUFE_4B",
        "INFRA_KONTOR_1" => "INFRA_STUFE_2B",
        "INFRA_KONTOR_2" => "INFRA_STUFE_3A",
        "INFRA_KONTOR_3" => "INFRA_STUFE_4A",
        "INFRA_KANON"    => "INFRA_STUFE_3E",
        _ => return None,
    })
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

        // Sanity-check the category/production_type mapping landed.
        let any_residence = defs.iter().any(|d|
            d.production_type == crate::types::ProductionType::Residence
        );
        let any_plantation = defs.iter().any(|d|
            d.production_type == crate::types::ProductionType::Plantation
        );
        let any_mine = defs.iter().any(|d|
            d.production_type == crate::types::ProductionType::Mine
        );
        assert!(any_residence, "expected at least one Residence");
        assert!(any_plantation, "expected at least one Plantation");
        assert!(any_mine, "expected at least one Mine");
        let cat_used: std::collections::HashSet<u8> =
            defs.iter().map(|d| d.category).collect();
        assert!(
            cat_used.len() >= 4,
            "category mapping should produce multiple categories, got {:?}",
            cat_used,
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
