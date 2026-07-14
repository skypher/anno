//! Bridge between parsed game files (anno-formats) and simulation data structures.
//!
//! Converts COD building definitions and SZS scenario data into the types
//! used by the simulation engine.

use std::collections::HashMap;

use anno_formats::cod::{BuildingDef as CodBuilding, CodFile};
use anno_formats::szs::SzsFile;

use crate::building::{BuildingDef, BuildingInstance};
use crate::source_cell::SourceMapCellState;
use crate::source_route::{SourceDynamicMapObject, SourceDynamicMapObjectTable};
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
        "ALKOHOL",
        "BAUM",
        "BAUMWOLLE",
        "EISEN",
        "EISENERZ",
        "ERZE",
        "FISCHE",
        "FLEISCH",
        "GETREIDE",
        "GEWUERZBAUM",
        "GEWUERZE",
        "GOLD",
        "GRAS",
        "HOLZ",
        "KAKAO",
        "KAKAOBAUM",
        "KANONEN",
        "KLEIDUNG",
        "KORN",
        "MEHL",
        "MUSKETEN",
        "NAHRUNG",
        "SCHMUCK",
        "SCHWERTER",
        "STEINE",
        "STOFFE",
        "TABAK",
        "TABAKBAUM",
        "TABAKWAREN",
        "WEINTRAUBEN",
        "WERKZEUG",
        "WILD",
        "WOLLE",
        "ZIEGEL",
        "ZUCKER",
        "ZUCKERROHR",
    ];
    for tok in tokens {
        let g = parse_good(tok);
        // GRAS is the only one that legitimately maps to None.
        if tok != "GRAS" {
            assert_ne!(g, Good::None, "token {tok} should map to a real good");
        }
    }
}

#[cfg(test)]
#[test]
fn rohstoff_to_fertility_matches_audit_pairs() {
    use anno_formats::szs::Fertility;
    // Pairs derived from `cargo run --example audit_fertility_mapping`
    // — every PLANTAGE entry's Rohstoff field paired with the
    // fertility-gated crop it grows.
    let pairs: &[(&str, Option<Fertility>)] = &[
        ("GETREIDE", Some(Fertility::Grain)),
        ("TABAKBAUM", Some(Fertility::Tobacco)),
        ("GEWUERZBAUM", Some(Fertility::Spices)),
        ("ZUCKERROHR", Some(Fertility::Sugarcane)),
        ("BAUMWOLLE", Some(Fertility::Cotton)),
        ("WEINTRAUBEN", Some(Fertility::Vines)),
        ("KAKAOBAUM", Some(Fertility::Cocoa)),
        // Universal raw materials should NOT bind a fertility.
        ("BAUM", None),
        ("STEINE", None),
        ("ERZE", None),
        ("", None),
    ];
    for (rohstoff, want) in pairs {
        assert_eq!(
            rohstoff_to_fertility(rohstoff),
            *want,
            "rohstoff_to_fertility({rohstoff:?})"
        );
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
        ("INFRA_BURG_1", 1),   // Settler  (= INFRA_STUFE_2G)
        ("INFRA_WACHTURM", 1), // Settler  (= INFRA_STUFE_2G)
        ("INFRA_KONTOR_2", 2), // Citizen  (= INFRA_STUFE_3A)
        ("INFRA_KANON", 2),    // Citizen  (= INFRA_STUFE_3E)
        ("INFRA_KONTOR_3", 3), // Merchant (= INFRA_STUFE_4A)
        ("INFRA_BURG_2", 3),   // Merchant (= INFRA_STUFE_4B)
        ("INFRA_MUSKETE", 3),  // Merchant (= INFRA_STUFE_4B)
        ("INFRA_BURG_3", 4),   // Aristo   (= INFRA_STUFE_5B)
        // Direct STUFE tokens.
        ("INFRA_STUFE_1A", 0),
        ("INFRA_STUFE_5A", 4),
        ("", 0),
        // Cultural-building tags (no marker-table alias defined).
        ("INFRA_KIRCHE", 0),
        ("INFRA_SCHULE", 0),
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
        Good::Flour => 5,                // Windmill
        Good::Grain | Good::Cattle => 5, // Plantation farms
        Good::Cotton | Good::Wool => 10, // Wool/cotton farms
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
        Good::Cloth => 20, // Weaving mill / weaver / tailor
        Good::Clothing => 20,
        // Plantations.
        Good::Sugar | Good::Tobacco | Good::Spices | Good::Cocoa => 5,
        Good::Wood => 5, // Forester
        Good::Stone | Good::Bricks => 25,
        // Drinks.
        Good::Alcohol => 15, // Distillery
        Good::Grapes => 35,  // Winery
        Good::TobaccoProducts => 25,
        Good::WildGame => 5,
        Good::Silk => 20,
        Good::Fish => 5,
        // Non-production buildings (markets etc.) — fall through.
        _ => match prod_kind_str {
            "HANDWERK" => 5,
            "ROHSTOFF" | "PLANTAGE" | "STEINBRUCH" | "BERGWERK" | "JAGDHAUS" | "FISCHEREI" => 3,
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
        "BODEN" | "WALD" | "FELS" | "STRAND" | "MEER" | "FLUSS" | "FLUSSECK" | "MUENDUNG"
        | "HANG" | "HANGECK" | "HANGQUELL" | "BRANDUNG" | "BRANDECK" | "STRANDECKA"
        | "STRANDECKI" | "STRANDMUND" | "STRANDRUINE" | "STRANDVARI" | "STRANDHAUS"
        | "WEIDETIER" | "MAUERSTRAND" | "TURMSTRAND" => 0,
        // Residential.
        "WOHNUNG" | "PIRATWOHN" => 1,
        // Production / industry / raw materials.
        "HANDWERK" | "PLANTAGE" | "ROHSTOFF" | "ROHSTERZ" | "ROHSTWACHS" | "BERGWERK"
        | "STEINBRUCH" | "MINE" | "JAGDHAUS" | "FISCHEREI" | "WMUEHLE" => 2,
        // Public services and culture.
        "KAPELLE" | "KIRCHE" | "SCHULE" | "HOCHSCHULE" | "THEATER" | "BADEHAUS" | "BRUNNEN"
        | "WIRT" | "DENKMAL" | "TRIUMPH" | "KLINIK" | "GALGEN" | "MARKT" | "PLATZ" => 3,
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
        "PLANTAGE" | "ROHSTOFF" | "ROHSTERZ" | "ROHSTWACHS" | "JAGDHAUS" | "FISCHEREI"
        | "WEIDETIER" => ProductionType::Plantation,
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
            if v > 0 {
                (v as u8).min(255)
            } else {
                6
            }
        },
        can_dry_up: prop("Doerrflg") == "1",
        wegspeed: {
            let raw = prop("Wegspeed");
            let mut quad = [100u16; 4];
            for (i, tok) in raw.split(',').map(str::trim).enumerate().take(4) {
                if let Ok(v) = tok.parse::<u16>() {
                    quad[i] = v;
                }
            }
            quad
        },
        has_door: prop("Tuerflg") == "1",
        upgradeable: prop("Ausbauflg") == "1",
        max_energy: {
            let v = prop_int("Maxenergy");
            if v > 0 {
                v as u16
            } else {
                0
            }
        },
        ore_deposit: match prop("Erzbergnr") {
            "ERZBERG_KLEIN" => crate::building::OreDeposit::Small,
            "ERZBERG_GROSS" => crate::building::OreDeposit::Large,
            _ => crate::building::OreDeposit::None,
        },
        pirate_owned: prop("Piratflg") == "1",
        defensive_cannons: prop_int("Kanon").max(0) as u8,
        max_brand_damage_ticks: {
            let v = prop_int("Maxbrand");
            if v > 0 {
                v as u16
            } else {
                crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS
            }
        },
        ruin_id: cod_building.ruinenr.clamp(0, 255) as u8,
        required_fertility: rohstoff_to_fertility(good_name("Rohstoff")),
    }
}

/// Map haeuser.cod's `Rohstoff` raw-material name to the typed
/// fertility the host island must carry. Audit-derived from
/// `cargo run --example audit_fertility_mapping`:
///
///   TABAKBAUM    → Tobacco
///   KAKAOBAUM    → Cocoa
///   ZUCKERROHR   → Sugarcane
///   WEINTRAUBEN  → Vines      (Nummer 408 → Alkohol/Wine)
///   BAUMWOLLE    → Cotton     (Nummer 404 → Wolle/Cotton)
///   GEWUERZBAUM  → Spices
///   GETREIDE     → Grain
///   (BAUM, STEINE, …) — universal, no fertility gate
fn rohstoff_to_fertility(name: &str) -> Option<anno_formats::szs::Fertility> {
    use anno_formats::szs::Fertility::*;
    Some(match name {
        "GETREIDE" => Grain,
        "TABAKBAUM" => Tobacco,
        "GEWUERZBAUM" => Spices,
        "ZUCKERROHR" => Sugarcane,
        "BAUMWOLLE" => Cotton,
        "WEINTRAUBEN" => Vines,
        "KAKAOBAUM" => Cocoa,
        _ => return None,
    })
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
/// INFRA_GALGEN, INFRA_WIRT) appear on the cultural BUILDINGS
/// themselves (church, school, doctor, etc.) — not as Bauinfra
/// requirements on residences. Audit confirmed by
/// `cargo run --example audit_bauinfra_tags`: every Kind=HQ
/// (residence-like) entry uses INFRA_STUFE_* or INFRA_KONTOR_*
/// tags, never a cultural tag. The cultural tags identify which
/// building IS the cultural one, and the runtime tier
/// progression checks for an active such building on the island
/// (a check we haven't traced to a specific binary function).
/// As a Bauinfra-on-residence tier gate, these return 0.
fn parse_bauinfra(token: &str) -> u8 {
    if token.is_empty() {
        return 0;
    }
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
        "INFRA_BURG_1" => "INFRA_STUFE_2G",
        "INFRA_BURG_2" => "INFRA_STUFE_4B",
        "INFRA_BURG_3" => "INFRA_STUFE_5B",
        "INFRA_WACHTURM" => "INFRA_STUFE_2G",
        "INFRA_MUSKETE" => "INFRA_STUFE_4B",
        "INFRA_KONTOR_1" => "INFRA_STUFE_2B",
        "INFRA_KONTOR_2" => "INFRA_STUFE_3A",
        "INFRA_KONTOR_3" => "INFRA_STUFE_4A",
        "INFRA_KANON" => "INFRA_STUFE_3E",
        _ => return None,
    })
}

/// Load all building definitions from a parsed COD file.
pub fn load_building_defs(cod: &CodFile) -> Vec<BuildingDef> {
    cod.buildings
        .iter()
        .map(|b| convert_building_def(b))
        .collect()
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
/// (via source definition ID) into a BuildingInstance.
/// Only creates instances for production buildings (those with production ProdKind).
pub fn load_building_instances(
    szs: &SzsFile,
    cod: &CodFile,
    building_defs: &[BuildingDef],
) -> Vec<BuildingInstance> {
    let source_id_map: HashMap<i32, usize> = cod
        .buildings
        .iter()
        .enumerate()
        .map(|(index, building)| (building.source_id, index))
        .collect();
    let mut instances = Vec::new();

    for island in &szs.islands {
        for tile in &island.tiles {
            if let Some(&def_idx) = source_id_map.get(&tile.source_id()) {
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

                // Each island's STADT4 chunk carries the slot
                // number that owns its city — that's the closest
                // proxy to per-tile ownership, since INSELHAUS
                // tiles don't carry an explicit owner byte.
                // Islands without a city default to slot 0 (the
                // player) which matches the original engine's
                // behaviour for player-built tiles on uncolonised
                // land.
                let owner = island.city.as_ref().map(|c| c.owner_slot).unwrap_or(0);
                let instance = BuildingInstance::new(
                    def_idx as u16,
                    island.number,
                    tile.x as u16,
                    tile.y as u16,
                    owner,
                );
                instances.push(instance);
            }
        }
    }

    instances
}

/// Replay INSELHAUS overwrite order into the renderer-relevant subset of the
/// source map-cell records. `FUN_00481450` removes records whose command
/// roots are overwritten before `FUN_00481fc0` creates the new root record;
/// this bridge retains the selector-bearing source kinds 1 through 7.
pub fn source_map_cell_states_from_scenario(
    szs: &SzsFile,
    cod: &CodFile,
) -> Vec<SourceMapCellState> {
    let mut states = Vec::new();

    for island in &szs.islands {
        for tile in &island.tiles {
            let Some(definition) = cod.building_by_source_id(tile.source_id()) else {
                continue;
            };
            let (width, height) = if matches!(tile.orientation & 3, 1 | 3) {
                (definition.size.1, definition.size.0)
            } else {
                definition.size
            };
            if width <= 0 || height <= 0 {
                continue;
            }
            let right = i32::from(tile.x) + width;
            let bottom = i32::from(tile.y) + height;
            states.retain(|state: &SourceMapCellState| {
                state.island != island.number
                    || i32::from(state.x) < i32::from(tile.x)
                    || i32::from(state.x) >= right
                    || i32::from(state.y) < i32::from(tile.y)
                    || i32::from(state.y) >= bottom
            });

            if !matches!(definition.source_kind_code(), Some(1..=7)) {
                continue;
            }
            if let Some(state) =
                SourceMapCellState::new(island.number, tile.x, tile.y, definition, 0)
            {
                states.push(state);
            }
        }
    }

    states
}

/// Reconstruct the source island map-object tables that the INSELHAUS loader
/// creates for `Kind=HQ` definitions.
///
/// `FUN_00465170` allocates the first free slot when the tile's current
/// three-bit map-owner value does not already name a live object. It then
/// writes that slot across the definition's oriented footprint via
/// `FUN_0046ae20`. INSELHAUS stores the definition offset, so the lookup here
/// must use `IslandTile::source_id`, not the definition's GFX value.
pub fn source_dynamic_map_objects_from_scenario(
    szs: &SzsFile,
    cod: &CodFile,
) -> Vec<SourceDynamicMapObject> {
    let mut objects = Vec::new();

    for island in &szs.islands {
        let mut table = SourceDynamicMapObjectTable::new(island.number);
        let mut slot_overlay = HashMap::<(u8, u8), u8>::new();

        for tile in &island.tiles {
            let Some(definition) = cod.building_by_source_id(tile.source_id()) else {
                continue;
            };
            if definition.source_kind_code() != Some(0x23) {
                continue;
            }

            let current_slot = slot_overlay
                .get(&(tile.x, tile.y))
                .copied()
                .unwrap_or_else(|| tile.source_owner());
            if table.object(current_slot).is_some() {
                continue;
            }

            let Some(object) = table.allocate(tile.source_dynamic_object_owner(), (tile.x, tile.y))
            else {
                continue;
            };

            let (width, height) = if matches!(tile.orientation, 1 | 3) {
                (definition.size.1, definition.size.0)
            } else {
                definition.size
            };
            if width <= 0 || height <= 0 {
                continue;
            }

            for y in i32::from(tile.y)..i32::from(tile.y) + height {
                for x in i32::from(tile.x)..i32::from(tile.x) + width {
                    if x < i32::from(island.width) && y < i32::from(island.height) {
                        slot_overlay.insert((x as u8, y as u8), object.slot);
                    }
                }
            }
        }

        objects.extend(table.objects());
    }

    objects
}

/// Locate KONTOR (warehouse) tiles in INSELHAUS data and
/// emit a `Warehouse` per occurrence, anchored on the actual
/// tile position rather than an averaged centroid. This is
/// faithful to where the scenario author placed the Kontor.
///
/// Caller pairs this with each island's `city.owner_slot`
/// when present so the warehouse inherits the right slot;
/// uncolonised islands default to slot 0.
///
/// Capacity comes from haeuser.cod's `Maxlager` field on the
/// matching building def — KONTOR_1 = 50, KONTOR_2 = 75,
/// KONTOR_3 = 100.
pub fn kontor_warehouses_from_szs(
    szs: &anno_formats::szs::SzsFile,
    cod: &CodFile,
    building_defs: &[BuildingDef],
) -> Vec<crate::warehouse::Warehouse> {
    use crate::warehouse::Warehouse;
    let source_id_map: HashMap<i32, usize> = cod
        .buildings
        .iter()
        .enumerate()
        .map(|(index, building)| (building.source_id, index))
        .collect();
    let mut out = Vec::new();
    for island in &szs.islands {
        let owner = island.city.as_ref().map(|c| c.owner_slot).unwrap_or(0);
        for tile in &island.tiles {
            let Some(&def_idx) = source_id_map.get(&tile.source_id()) else {
                continue;
            };
            let Some(def) = building_defs.get(def_idx) else {
                continue;
            };
            // ProdKind=KONTOR identifies warehouse tiles.
            if def.prod_kind != "KONTOR" {
                continue;
            }
            // Carry the Kontor's authored storage capacity (50/
            // 75/100 tons across KONTOR_1/2/3, 20 t for the
            // small variants) into the warehouse so deposits
            // hit the right ceiling instead of the legacy 30 t
            // default.
            let cap = if def.storage_capacity > 0 {
                def.storage_capacity
            } else {
                30
            };
            out.push(Warehouse::with_capacity(
                island.number,
                owner,
                tile.x as u16,
                tile.y as u16,
                cap,
            ));
        }
    }
    out
}

/// Seed an initial `DiplomacyMatrix` from PLAYER4's per-slot
/// `relationships` arrays. The audit-derived value mapping is
/// HEURISTIC (binary semantics not yet pinned — see TaskList
/// #126):
///
///   0 → Neutral (no explicit relationship; default state)
///   1 → Neutral (rare, possibly cooldown — treat as neutral)
///   2 → Neutral (rare, possibly treaty-pending — treat as neutral)
///   3 → Neutral (peace/pact)
///
/// Currently all four PLAYER4 codes resolve to `Neutral`,
/// because we don't have a confident "this code means war"
/// reading. Callers should still apply additional War-edge
/// seeding (e.g. native-ship spawn → War with slot 5) on top
/// of this matrix.
pub fn diplomacy_from_player4_relationships(
    players: &[anno_formats::szs::PlayerSlotInit],
) -> crate::combat::DiplomacyMatrix {
    let mut dm = crate::combat::DiplomacyMatrix::new();
    let n = players.len().min(7);
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let code = players[i].relationships[j];
            let state = code_to_diplomacy(code);
            // Only downgrade default Neutral to War / upgrade
            // to Allied; never overwrite an already-set War.
            if dm.get(i as u8, j as u8) != crate::combat::Diplomacy::War {
                dm.set(i as u8, j as u8, state);
            }
        }
    }
    dm
}

/// Map a raw PLAYER4 relationship code to the sim's
/// `Diplomacy` enum. Heuristic-only; see
/// `diplomacy_from_player4_relationships` doc-comment.
pub fn code_to_diplomacy(code: u32) -> crate::combat::Diplomacy {
    use crate::combat::Diplomacy;
    match code {
        0..=3 => Diplomacy::Neutral,
        _ => Diplomacy::Neutral,
    }
}

/// Building Nummer references for native + pirate dwellings,
/// derived from haeuser.cod's `Nativflg=1` / `Piratflg=1`
/// tagged entries (`cargo run --example
/// probe_native_pirate_buildings`).
///
/// Native village (slot 5):
///   442 = Chief's hut / Kontor (variant A)
///   443 = Warrior's hut (MILITAR)
///   444 = Native dwelling (PIRATWOHN)
///   445 = Spice plantation (GEWUERZE)
///   446–447 = Tobacco plantations (TABAKWAREN)
///   448 = Chief's hut / Kontor (variant B)
///   449–450 = Additional native dwellings + warrior hut
///   451–454 = More native plantations (incl. cliff-side TABAK)
///
/// Pirate stronghold (slot 6):
///   455 = Pirate Kontor (also Nativflg=1 in COD — the same
///         building doubles as both faction's hub)
///   456–458 = Pirate dwellings (PIRATWOHN)
///   459–460 = Pirate watchtowers (WACHTURM)
pub const NATIVE_KONTOR_A: i32 = 442;
pub const NATIVE_KONTOR_B: i32 = 448;
pub const PIRATE_KONTOR: i32 = 455;

/// All native-faction building Nummers (442..=458 inclusive).
pub const NATIVE_BUILDING_NUMMERS: std::ops::RangeInclusive<i32> = 442..=458;

/// All pirate-faction building Nummers (455..=460 inclusive,
/// overlapping the native range at 455-458).
pub const PIRATE_BUILDING_NUMMERS: std::ops::RangeInclusive<i32> = 455..=460;

/// `route_id` value used for SHIP4 traders that have no
/// configured route. Picked so it can never collide with a
/// real `TradeRoute::id` (those start at 0 and grow
/// monotonically). `tick_trade_ship` skips ships whose
/// route_id doesn't match any active route, so these
/// "stranded" traders sit at their spawn coordinates until
/// the player or AI assigns them to a route.
pub const UNROUTED_TRADER_ROUTE_ID: u16 = u16::MAX;

/// Convert SHIP4 records whose `ShipClass::is_warship()` is
/// true into `MilitaryUnit` instances for the simulation's
/// naval combat path. Trader ships are skipped — those need a
/// `TradeShip` with a route, which the scenario doesn't seed
/// directly. Returns the new units in the same order as the
/// underlying SHIP4 records, so callers can correlate by
/// index when annotating ship names later.
pub fn warships_from_ships(ships: &[anno_formats::szs::Ship]) -> Vec<crate::combat::MilitaryUnit> {
    use crate::combat::{MilitaryUnit, UnitType};
    use anno_formats::szs::ShipClass;
    ships
        .iter()
        .filter_map(|s| {
            let class = s.class()?;
            let unit_type = match class {
                ShipClass::SmallWarship => UnitType::SmallWarship,
                ShipClass::LargeWarship => UnitType::LargeWarship,
                ShipClass::PirateShip => UnitType::PirateShip,
                _ => return None,
            };
            Some(MilitaryUnit::with_name(
                unit_type,
                s.owner,
                s.x as i32,
                s.y as i32,
                s.name.clone(),
            ))
        })
        .collect()
}

/// Convert SHIP4 records whose class is `SmallTrader` or
/// `LargeTrader` into `TradeShip` instances. The resulting
/// ships have `route_id = UNROUTED_TRADER_ROUTE_ID` (a
/// sentinel that never matches a real route), so the trade
/// tick leaves them inert until a route is assigned. They
/// still spawn at their authored coordinates so the player
/// sees them in the world.
pub fn traders_from_ships(
    ships: &[anno_formats::szs::Ship],
    cargo_config: crate::trade::ShipCargoConfig,
) -> Vec<crate::trade::TradeShip> {
    use crate::trade::{TradeShip, TradeShipClass};
    use anno_formats::szs::ShipClass;
    ships
        .iter()
        .filter(|s| {
            matches!(
                s.class(),
                Some(ShipClass::SmallTrader | ShipClass::LargeTrader)
            )
        })
        .map(|s| {
            let class = match s.class().expect("filtered trader ship class") {
                ShipClass::SmallTrader => TradeShipClass::SmallTrader,
                ShipClass::LargeTrader => TradeShipClass::LargeTrader,
                _ => unreachable!("filtered to trader classes"),
            };
            let mut t = TradeShip::new_with_class(
                s.owner,
                UNROUTED_TRADER_ROUTE_ID,
                s.x as i32,
                s.y as i32,
                class,
                cargo_config.capacity_for(class),
            )
            .with_name(s.name.clone());
            // Carry the authored heading so the renderer
            // shows the ship facing the right direction.
            t.heading = s.heading();
            t.source_target_approach_radius = cargo_config.target_approach_radius_for(class);
            t
        })
        .collect()
}

/// Whether a plantation/farm building can be placed on the
/// given island. The check is purely a fertility lookup —
/// ownership, infrastructure tier, and tile-level placement
/// rules are validated by other passes.
///
/// Universal buildings (`required_fertility = None`, e.g.
/// foresters, brick kilns) always pass. Fertility-bound
/// plantations (`Some(Fertility::Tobacco)`, etc.) require
/// the corresponding non-sentinel byte in the island's
/// 8-slot fertility map.
///
/// Pre-placed scenario buildings are NOT subject to this
/// check — `load_building_instances` honours the scenario
/// author's decisions verbatim. The check applies to the
/// player/AI build-action path, where the original engine
/// rejects placements that violate the fertility gate.
pub fn island_can_host_building(def: &BuildingDef, island: &anno_formats::szs::Island) -> bool {
    let Some(req) = def.required_fertility else {
        return true;
    };
    island.active_fertilities().contains(&req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warships_from_ships_routes_warships_only() {
        use crate::combat::UnitType;
        use anno_formats::szs::Ship;
        let mk = |owner: u8, class: u8, x: u16, y: u16| Ship {
            raw_record: [0; anno_formats::szs::SHIP4_RECORD_BYTES],
            name: "test".into(),
            x,
            y,
            owner,
            figure_definition_id: class.into(),
            ship_class: class,
            stored_energy: 0,
            runtime_slot: 0,
            figure_kind: 0,
            animation_state: 0,
            heading_byte: 0,
            cargo_slots: [0; 7],
        };
        let ships = vec![
            mk(0, 0x15, 10, 10), // SmallTrader  → skip
            mk(1, 0x19, 20, 20), // SmallWarship → keep
            mk(2, 0x1B, 30, 30), // LargeWarship → keep
            mk(0, 0x17, 40, 40), // LargeTrader  → skip
            mk(6, 0x1F, 50, 50), // PirateShip   → keep
            mk(0, 0xFE, 0, 0),   // unknown     → skip
        ];
        let units = warships_from_ships(&ships);
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].unit_type, UnitType::SmallWarship);
        assert_eq!(units[0].owner, 1);
        assert_eq!(units[1].unit_type, UnitType::LargeWarship);
        assert_eq!(units[2].unit_type, UnitType::PirateShip);
        assert_eq!(units[2].owner, 6);
        // Position should round-trip from u16 → i32.
        assert_eq!(units[0].tile_x, 20);
        assert_eq!(units[0].tile_y, 20);
    }

    #[test]
    fn traders_from_ships_routes_traders_only_with_sentinel_id() {
        use anno_formats::szs::Ship;
        let mk = |owner: u8, class: u8, x: u16, y: u16| Ship {
            raw_record: [0; anno_formats::szs::SHIP4_RECORD_BYTES],
            name: "test".into(),
            x,
            y,
            owner,
            figure_definition_id: class.into(),
            ship_class: class,
            stored_energy: 0,
            runtime_slot: 0,
            figure_kind: 0,
            animation_state: 0,
            heading_byte: 0,
            cargo_slots: [0; 7],
        };
        let ships = vec![
            mk(0, 0x15, 10, 10), // SmallTrader  → keep
            mk(1, 0x19, 20, 20), // SmallWarship → skip
            mk(2, 0x17, 30, 30), // LargeTrader  → keep
            mk(0, 0x1B, 40, 40), // LargeWarship → skip
            mk(0, 0x1F, 50, 50), // PirateShip   → skip
            mk(0, 0xFE, 0, 0),   // unknown     → skip
        ];
        let mut cargo_config = crate::trade::ShipCargoConfig::default();
        cargo_config.small_trader_target_approach_radius = 4;
        cargo_config.large_trader_target_approach_radius = 2;
        let traders = traders_from_ships(&ships, cargo_config);
        assert_eq!(traders.len(), 2);
        for t in &traders {
            assert_eq!(
                t.route_id, UNROUTED_TRADER_ROUTE_ID,
                "spawn ships use the sentinel route id"
            );
            assert!(t.active, "spawn ships start active");
        }
        assert_eq!(traders[0].owner, 0);
        assert_eq!(traders[0].name, "test");
        assert_eq!(traders[0].world_x, 10);
        assert_eq!(traders[0].class, crate::trade::TradeShipClass::SmallTrader);
        assert_eq!(traders[0].cargo_capacity(), 40);
        assert_eq!(traders[0].source_target_approach_radius, 4);
        assert_eq!(traders[1].owner, 2);
        assert_eq!(traders[1].world_x, 30);
        assert_eq!(traders[1].class, crate::trade::TradeShipClass::LargeTrader);
        assert_eq!(traders[1].cargo_capacity(), 60);
        assert_eq!(traders[1].source_target_approach_radius, 2);
    }

    #[test]
    fn scenario_hq_tiles_reconstruct_one_oriented_dynamic_map_object() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let source_id = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![CodBuilding {
                source_id,
                kind: "HQ".into(),
                size: (2, 4),
                ..Default::default()
            }],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 4,
                width: 16,
                height: 16,
                x_pos: 100,
                y_pos: 200,
                fertilities: [7; 8],
                tiles: vec![
                    IslandTile {
                        building_id: 3,
                        x: 2,
                        y: 3,
                        orientation: 1,
                        anim_count: 0,
                        flags: 6 << 6,
                    },
                    // `FUN_0046ae20` has already overlaid slot 0 at this
                    // cell of the rotated 4 x 2 footprint, so the second
                    // HQ record must not allocate another object.
                    IslandTile {
                        building_id: 3,
                        x: 3,
                        y: 3,
                        orientation: 1,
                        anim_count: 0,
                        flags: 0,
                    },
                ],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
        };

        assert_eq!(
            source_dynamic_map_objects_from_scenario(&szs, &cod),
            vec![SourceDynamicMapObject {
                island: 4,
                slot: 0,
                owner: 6,
                local_position: (2, 3),
            }]
        );
    }

    #[test]
    fn source_cell_seeder_replays_oriented_command_overwrites() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let base = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                CodBuilding {
                    source_id: base + 1,
                    kind: "HANDWERK".into(),
                    size: (1, 1),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 2,
                    kind: "MARKT".into(),
                    size: (1, 1),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 3,
                    kind: "GEBAEUDE".into(),
                    // Rotating this 1 x 2 command makes it cover (3, 4),
                    // the earlier market root, while creating no cell state.
                    size: (1, 2),
                    ..Default::default()
                },
            ],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 6,
                width: 16,
                height: 16,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![
                    IslandTile {
                        building_id: 1,
                        x: 1,
                        y: 1,
                        orientation: 0,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 2,
                        x: 3,
                        y: 4,
                        orientation: 0,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 3,
                        x: 2,
                        y: 4,
                        orientation: 1,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 1,
                        x: 8,
                        y: 9,
                        orientation: 0,
                        anim_count: 0,
                        flags: 0,
                    },
                ],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
        };

        let states = source_map_cell_states_from_scenario(&szs, &cod);
        assert_eq!(states.len(), 2);
        assert!(states.iter().any(|state| state.matches(6, 1, 1)));
        assert!(states.iter().any(|state| state.matches(6, 8, 9)));
        assert!(!states.iter().any(|state| state.matches(6, 3, 4)));
    }

    #[test]
    fn production_loader_resolves_inselhaus_source_ids_not_gfx() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![CodBuilding {
                source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
                gfx: 9000,
                kind: "GEBAEUDE".into(),
                properties: HashMap::from([
                    ("ProdKind".into(), "HANDWERK".into()),
                    ("Ware".into(), "HOLZ".into()),
                ]),
                ..Default::default()
            }],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 2,
                width: 1,
                height: 1,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![IslandTile {
                    building_id: 3,
                    x: 0,
                    y: 0,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                }],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
        };

        let defs = load_building_defs(&cod);
        assert_eq!(load_building_instances(&szs, &cod, &defs).len(), 1);
    }

    #[test]
    fn load_building_instances_picks_owner_from_stadt4() {
        // New Horizons2 has cities owned by multiple slots
        // (player on island 0, AI rivals on later islands,
        // pirates on island 21). Building instances on those
        // islands should inherit the city's owner_slot.
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
        let szs_data = match std::fs::read(base.join("extracted/Szenes/New Horizons2.szs")) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: New Horizons2.szs not found");
                return;
            }
        };
        let cod = CodFile::parse(&cod_data).unwrap();
        let defs = load_building_defs(&cod);
        let szs = SzsFile::parse(&szs_data).unwrap();
        let instances = load_building_instances(&szs, &cod, &defs);

        // Map island_number → expected owner_slot from STADT4.
        let mut expected_owner: std::collections::HashMap<u8, u8> =
            std::collections::HashMap::new();
        for island in &szs.islands {
            if let Some(city) = island.city.as_ref() {
                expected_owner.insert(island.number, city.owner_slot);
            }
        }
        // Every instance's owner should match its island's
        // STADT4 owner_slot.
        for inst in &instances {
            if let Some(want) = expected_owner.get(&inst.island_id) {
                assert_eq!(
                    inst.owner, *want,
                    "building on island {} should be owned by slot {}, got {}",
                    inst.island_id, want, inst.owner
                );
            }
        }
        // Cross-slot diversity: at least 2 distinct owners
        // across the building set, otherwise the wiring is
        // probably broken (everything would be slot 0).
        let owners: std::collections::HashSet<u8> = instances.iter().map(|b| b.owner).collect();
        assert!(
            owners.len() >= 2,
            "expected ≥2 distinct owners across New Horizons2's buildings, got {owners:?}"
        );
    }

    #[test]
    fn shipping_scenarios_expose_authored_kontors_by_source_id() {
        // INSELHAUS records are source-definition offsets, not GFX values.
        // The source-ID lookup exposes authored Kontors that the former GFX
        // lookup missed, including the native and pirate settlements in the
        // shipping corpus.
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
        let cod = CodFile::parse(&cod_data).unwrap();
        let defs = load_building_defs(&cod);
        let szenes = base.join("extracted/Szenes");
        if !szenes.exists() {
            println!("Skipping: scenes dir not found");
            return;
        }
        let mut scenarios = 0;
        let mut kontors = 0;
        for entry in std::fs::read_dir(&szenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let szs = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let whs = kontor_warehouses_from_szs(&szs, &cod, &defs);
            for warehouse in &whs {
                assert!(
                    szs.islands
                        .iter()
                        .any(|island| island.number == warehouse.island_id),
                    "{:?} yielded a Kontor on an unknown island {}",
                    path.file_stem().unwrap(),
                    warehouse.island_id
                );
            }
            kontors += whs.len();
            scenarios += 1;
        }
        assert!(scenarios > 0, "audit must cover at least one scenario");
        assert!(kontors > 0, "source-ID audit must recover authored Kontors");
    }

    #[test]
    fn native_pirate_kontor_constants_match_haeuser_cod() {
        // Pin the canonical Kontor Nummers for native + pirate
        // settlements against haeuser.cod.
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
        let cod = CodFile::parse(&cod_data).unwrap();
        for &nr in &[NATIVE_KONTOR_A, NATIVE_KONTOR_B, PIRATE_KONTOR] {
            let b = cod
                .buildings
                .iter()
                .find(|b| b.nummer == nr)
                .unwrap_or_else(|| panic!("Nr={nr} not in haeuser.cod"));
            assert_eq!(b.kind, "HQ", "Nr={nr} should be Kind=HQ");
            assert_eq!(
                b.properties.get("ProdKind").map(|s| s.as_str()),
                Some("KONTOR"),
                "Nr={nr} should be ProdKind=KONTOR"
            );
        }
        // Pirate Kontor must carry both flags.
        let pirat = cod
            .buildings
            .iter()
            .find(|b| b.nummer == PIRATE_KONTOR)
            .unwrap();
        assert_eq!(
            pirat.properties.get("Piratflg").map(|s| s.as_str()),
            Some("1")
        );
    }

    #[test]
    fn diplomacy_from_relationships_defaults_neutral_for_now() {
        use crate::combat::Diplomacy;
        use anno_formats::szs::PlayerSlotInit;
        // Synthesise three slots with the typical pattern:
        // slots 0..=2 with relationships [0, 0, 0, 3, 3, 3, 3].
        let mk = |relationships: [u32; 7]| PlayerSlotInit {
            relationships,
            ..Default::default()
        };
        let players = vec![
            mk([0, 0, 0, 3, 3, 3, 3]),
            mk([0, 0, 0, 3, 3, 3, 3]),
            mk([0, 0, 0, 3, 3, 3, 3]),
        ];
        let dm = diplomacy_from_player4_relationships(&players);
        // Heuristic mapping currently produces Neutral for
        // every pair — until the code semantics are pinned,
        // we don't synthesise War from these arrays alone.
        for i in 0..3u8 {
            for j in 0..3u8 {
                if i == j {
                    continue;
                }
                assert_eq!(
                    dm.get(i, j),
                    Diplomacy::Neutral,
                    "default mapping should be Neutral for ({i}, {j})"
                );
            }
        }
    }

    #[test]
    fn island_can_host_building_gates_fertility_bound_plantations() {
        use crate::types::ProductionType;
        use anno_formats::szs::{Fertility, Island};
        let mk_island = |ferts: [u8; 8]| Island {
            number: 0,
            width: 10,
            height: 10,
            x_pos: 0,
            y_pos: 0,
            fertilities: ferts,
            tiles: Vec::new(),
            city: None,
        };
        let mk_def = |req: Option<Fertility>| {
            let mut d = BuildingDef {
                id: 0,
                category: 0,
                width: 1,
                height: 1,
                production_type: ProductionType::Craft,
                kind: "PLANTAGE".into(),
                prod_kind: "PLANTAGE".into(),
                radius: 0,
                output_good: Good::None,
                input_good_1: Good::None,
                input_good_2: Good::None,
                output_rate: 1,
                input_1_rate: 0,
                input_2_rate: 0,
                storage_capacity: 0,
                cycle_time_ms: 1000,
                cost_gold: 0,
                cost_tools: 0,
                cost_wood: 0,
                cost_bricks: 0,
                maintenance_cost: 0,
                native: false,
                min_tier: 0,
                max_no_input_ticks: 6,
                can_dry_up: false,
                wegspeed: [100; 4],
                has_door: false,
                upgradeable: false,
                max_energy: 0,
                ore_deposit: crate::building::OreDeposit::None,
                pirate_owned: false,
                defensive_cannons: 0,
                max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
                ruin_id: crate::building::NO_RUIN_ID,
                required_fertility: req,
            };
            d.required_fertility = req;
            d
        };

        // Universal building (no fertility requirement) passes
        // even on a barren island.
        let universal = mk_def(None);
        let barren = mk_island([7; 8]);
        assert!(island_can_host_building(&universal, &barren));

        // Tobacco plantation requires byte 1 in the map.
        let tobacco = mk_def(Some(Fertility::Tobacco));
        assert!(
            !island_can_host_building(&tobacco, &barren),
            "barren island should reject tobacco"
        );
        let tobacco_isle = mk_island([1, 7, 7, 7, 7, 7, 7, 7]);
        assert!(
            island_can_host_building(&tobacco, &tobacco_isle),
            "byte=1 island should accept tobacco"
        );

        // Multi-fertility island accepts every matching crop.
        let multi = mk_island([3, 6, 7, 7, 7, 7, 7, 7]);
        let sugarcane = mk_def(Some(Fertility::Sugarcane));
        let cocoa = mk_def(Some(Fertility::Cocoa));
        let cotton = mk_def(Some(Fertility::Cotton));
        assert!(island_can_host_building(&sugarcane, &multi));
        assert!(island_can_host_building(&cocoa, &multi));
        assert!(
            !island_can_host_building(&cotton, &multi),
            "cotton missing from {{Sugarcane, Cocoa}} island"
        );
    }

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
        let any_residence = defs
            .iter()
            .any(|d| d.production_type == crate::types::ProductionType::Residence);
        let any_plantation = defs
            .iter()
            .any(|d| d.production_type == crate::types::ProductionType::Plantation);
        let any_mine = defs
            .iter()
            .any(|d| d.production_type == crate::types::ProductionType::Mine);
        assert!(any_residence, "expected at least one Residence");
        assert!(any_plantation, "expected at least one Plantation");
        assert!(any_mine, "expected at least one Mine");
        let cat_used: std::collections::HashSet<u8> = defs.iter().map(|d| d.category).collect();
        assert!(
            cat_used.len() >= 4,
            "category mapping should produce multiple categories, got {:?}",
            cat_used,
        );

        let source_maxbrand_values: std::collections::HashSet<_> = cod
            .buildings
            .iter()
            .filter_map(|b| b.properties.get("Maxbrand"))
            .map(|v| v.as_str())
            .collect();
        assert_eq!(
            source_maxbrand_values,
            std::collections::HashSet::from(["4"])
        );
        assert!(
            defs.iter().all(
                |d| d.max_brand_damage_ticks == crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS
            ),
            "converted definitions should inherit haeuser.cod Maxbrand: 4",
        );
        let ruin_cases = [
            (270, 8),  // RUINE_KONTOR_1
            (271, 9),  // ObjFill: BASE, then @Ruinenr: +1
            (272, 10), // next @Ruinenr: +1 directive value
            (273, 11), // next @Ruinenr: +1 directive value
            (274, 0),  // RUINE_HOLZ
            (275, 0),  // RUINE_HOLZ
            (276, 2),  // RUINE_STEIN
            (277, 2),  // RUINE_STEIN
            (359, crate::building::NO_RUIN_ID),
        ];
        for (nummer, ruin_id) in ruin_cases {
            let def = defs
                .iter()
                .find(|d| d.id == nummer)
                .unwrap_or_else(|| panic!("missing converted building Nr={nummer}"));
            assert_eq!(def.ruin_id, ruin_id, "Nr={nummer} ruin_id");
        }
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
                .find(|e| e.file_name().to_string_lossy().ends_with(".szs"))
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
