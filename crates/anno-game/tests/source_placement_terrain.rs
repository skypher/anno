//! The original's build-terrain gate, against the shipped data. Self-skips
//! without the data corpus.
//!
//! `FUN_004084d0` gates every candidate tile on `FUN_00464450`
//! (`1602_exe.c:7609`), whose per-cell verdict is the kind table
//! `FUN_00464660` (`1602_exe.c:70042-70280`). Nothing player-placeable is
//! admitted on `MEER`: the harbour classes reach the water line through the
//! beach kinds and through the shore orientation resolvers `FUN_00467af0` /
//! `FUN_00467e60`, never by standing in the water.
//!
//! Until this gate existed the port's only terrain test was
//! `IslandMap::is_walkable`, which counted `MEER` as walkable, so a scripted
//! colony could put its Kontor, its marketplace and its fishery on open
//! water — everything produced, nothing could ever be carted anywhere
//! (`docs/logistics-gaps.md` §6).

use anno_sim::commands::Command;

struct Corpus {
    szs: anno_formats::szs::SzsFile,
    cod: anno_formats::cod::CodFile,
    defs: Vec<anno_sim::building::BuildingDef>,
    figures: anno_formats::figuren::FiguresFile,
}

fn load_corpus() -> Option<Corpus> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .to_path_buf();
    let (Ok(szs_data), Ok(cod_data)) = (
        std::fs::read(root.join("extracted/Szenes/New Horizons0.szs")),
        std::fs::read(root.join("extracted/haeuser.cod")),
    ) else {
        println!("Skipping test: data corpus not found");
        return None;
    };
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).expect("parse New Horizons0");
    anno_game::scenario::instantiate_stock_islands(&mut szs, &root.join("extracted"), 1);
    let cod = anno_formats::cod::CodFile::parse(&cod_data).expect("parse haeuser.cod");
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("extracted/figuren.cod"))
        .map(|bytes| anno_formats::figuren::FiguresFile::parse(&bytes))
        .expect("parse figuren.cod");
    Some(Corpus {
        szs,
        cod,
        defs,
        figures,
    })
}

/// The island the campaign leaves unsettled.
const ISLAND: u8 = 10;
/// Source outer map kind of open sea, `Kind: MEER`.
const MEER: u8 = 19;

/// The kind codes the unit test in `anno_sim::island_map` hard-codes are the
/// ones the shipped file actually authors.
#[test]
fn the_shipped_definitions_carry_the_kinds_the_gate_switches_on() {
    let Some(corpus) = load_corpus() else { return };
    let by_source_id = |id: i32| {
        corpus
            .cod
            .buildings
            .iter()
            .find(|building| building.source_id == id)
            .unwrap_or_else(|| panic!("haeuser.cod ships source id {id}"))
    };

    // `Kind: HQ`, `Strandflg: 1`, `PlaceFlg: 1`, `Size: 2, 3` — the founding
    // Kontor, `@Nummer` 271.
    let kontor = by_source_id(22103);
    assert_eq!(kontor.kind, "HQ");
    assert_eq!(kontor.source_kind_code(), Some(35));
    assert_eq!(kontor.size, (2, 3));
    assert_eq!(
        kontor.properties.get("Strandflg").map(String::as_str),
        Some("1"),
        "the Kontor finds its beach through FUN_00467af0, not by standing on it",
    );

    // `Kind: STRANDHAUS`, `ProdKind: FISCHEREI` — the fishery, `@Nummer` 269.
    let fishery = by_source_id(21075);
    assert_eq!(fishery.kind, "STRANDHAUS");
    assert_eq!(fishery.source_kind_code(), Some(28));
    assert_eq!(fishery.source_production_kind_code(), Some(6));

    // `Kind: HAFEN`, `ProdKind: WERFT` — the two shipyards, the only class
    // whose own footprint the original lets span land and beach.
    let shipyards: Vec<_> = corpus
        .cod
        .buildings
        .iter()
        .filter(|building| {
            building.properties.get("ProdKind").map(String::as_str) == Some("WERFT")
        })
        .collect();
    assert_eq!(shipyards.len(), 2, "haeuser.cod ships two shipyards");
    for shipyard in shipyards {
        assert_eq!(shipyard.kind, "HAFEN");
        assert_eq!(shipyard.source_kind_code(), Some(36));
    }

    // `Kind: GEBAEUDE` — the ordinary land building the `default:` arm covers.
    let weaver = by_source_id(20507);
    assert_eq!(weaver.kind, "GEBAEUDE");
    assert_eq!(weaver.source_kind_code(), Some(14));
}

/// A Kontor cannot be founded out at sea on New Horizons0's island 10, and
/// can be founded on its coastline.
#[test]
fn a_kontor_is_refused_at_sea_and_accepted_on_the_coastline() {
    let Some(corpus) = load_corpus() else { return };
    let mut sim = anno_game::scenario::build_simulation(
        &corpus.szs,
        &corpus.cod,
        &corpus.defs,
        &corpus.figures,
    );
    sim.seed_source_rand(1);

    let island = corpus
        .szs
        .islands
        .iter()
        .find(|island| island.number == ISLAND)
        .expect("New Horizons0 ships island 10")
        .clone();
    let map_index = sim
        .island_maps
        .iter()
        .position(|map| map.island_id == ISLAND)
        .expect("island 10 has a map");
    let kontor_index = corpus
        .cod
        .buildings
        .iter()
        .position(|building| building.source_id == 22103)
        .expect("founding Kontor definition");
    let kontor = corpus.defs[kontor_index].clone();

    let ship = sim
        .trade_ships
        .iter()
        .position(|ship| ship.name == "Verena")
        .expect("the campaign's starting ship") as u32;
    let ship_start = {
        let ship = &sim.trade_ships[ship as usize];
        (ship.world_x, ship.world_y)
    };

    // A coastline the Kontor's 2x3 footprint fits on — every cell of it has
    // to be one of the kinds `FUN_00464660` case 0x23 admits — paired with
    // the open water beside it, which is both where the ship has to dock and
    // where a sea-founded Kontor would have stood. Take the pair nearest the
    // ship's start so it can actually sail there.
    let sea_beside = |anchor: (i32, i32)| {
        (-6_i32..=6)
            .flat_map(|dy| (-6_i32..=6).map(move |dx| (anchor.0 + dx, anchor.1 + dy)))
            .filter(|&(x, y)| {
                sim.island_maps[map_index]
                    .source_map_kind_and_owner(x, y)
                    .is_some_and(|(kind, _)| kind == MEER)
            })
            .min_by_key(|&(x, y)| (x - anchor.0).abs() + (y - anchor.1).abs())
    };
    let (anchor, sea) = (2..i32::from(island.height) - i32::from(kontor.height) - 2)
        .flat_map(|y| {
            (2..i32::from(island.width) - i32::from(kontor.width) - 2).map(move |x| (x, y))
        })
        .filter(|&(x, y)| {
            sim.island_maps[map_index].is_coastal(x, y)
                && anno_game::game_commands::can_place_building(
                    &island,
                    &sim.island_maps[map_index],
                    &kontor,
                    x,
                    y,
                    kontor.width,
                    kontor.height,
                )
        })
        .filter_map(|anchor| sea_beside(anchor).map(|sea| (anchor, sea)))
        .min_by_key(|&(_, sea)| {
            (ship_start.0 - (i32::from(island.x_pos) + sea.0)).abs()
                + (ship_start.1 - (i32::from(island.y_pos) + sea.1)).abs()
        })
        .expect("island 10 has a buildable coastline with water beside it");

    // The gate itself, before any command plumbing: kind 35 takes
    // `{STRASSE, WALD, BODEN, RUINE, PLATZ}` and sea is none of them.
    assert!(
        !sim.island_maps[map_index].source_placement_terrain_admits(
            35,
            sea.0,
            sea.1,
            kontor.width,
            kontor.height
        ),
        "the Kontor's footprint must not be admitted on MEER at {sea:?}",
    );
    assert!(
        sim.island_maps[map_index].source_placement_terrain_admits(
            35,
            anchor.0,
            anchor.1,
            kontor.width,
            kontor.height
        ),
        "the coastal anchor {anchor:?} must be admitted",
    );

    // Sail the ship to the water cell, so both attempts below differ only in
    // where the Kontor would stand.
    let world = (
        i32::from(island.x_pos) + sea.0,
        i32::from(island.y_pos) + sea.1,
    );
    assert!(sim.apply_command(&Command::SailShip {
        player: 0,
        ship_index: ship,
        world_x: world.0,
        world_y: world.1,
    }));
    // `found_kontor` measures the ship against the *anchor*, not the water
    // cell it was sent to — the two differ by the width of the beach ring —
    // so approach until that distance is inside its range.
    let anchor_world = (
        i32::from(island.x_pos) + anchor.0,
        i32::from(island.y_pos) + anchor.1,
    );
    for _ in 0..3_000 {
        sim.tick(100);
        let ship = &sim.trade_ships[ship as usize];
        if (ship.world_x - anchor_world.0).abs() + (ship.world_y - anchor_world.1).abs() < 12 {
            break;
        }
    }

    assert!(
        !anno_game::game_commands::apply_game_command(
            &mut sim,
            &corpus.szs.islands,
            &corpus.cod,
            &corpus.defs,
            &Command::FoundKontor {
                player: 0,
                ship_index: ship,
                island: ISLAND,
                tile_x: sea.0 as u16,
                tile_y: sea.1 as u16,
            },
        ),
        "founding at sea {sea:?} must be refused",
    );
    assert!(
        !sim.warehouses
            .iter()
            .any(|warehouse| warehouse.island_id == ISLAND),
        "a refused founding must leave no warehouse behind",
    );

    assert!(
        anno_game::game_commands::apply_game_command(
            &mut sim,
            &corpus.szs.islands,
            &corpus.cod,
            &corpus.defs,
            &Command::FoundKontor {
                player: 0,
                ship_index: ship,
                island: ISLAND,
                tile_x: anchor.0 as u16,
                tile_y: anchor.1 as u16,
            },
        ),
        "founding on the coastline at {anchor:?} must succeed",
    );
    assert!(sim
        .warehouses
        .iter()
        .any(|warehouse| warehouse.island_id == ISLAND && warehouse.owner == 0));
    assert!(sim
        .buildings
        .iter()
        .any(|building| building.island_id == ISLAND && building.owner == 0));

    // And the footprint it now occupies is closed to a second placement.
    assert!(
        !anno_game::game_commands::can_place_building(
            &island,
            &sim.island_maps[map_index],
            &kontor,
            anchor.0,
            anchor.1,
            kontor.width,
            kontor.height,
        ),
        "the founded Kontor must occupy its own footprint",
    );
}
