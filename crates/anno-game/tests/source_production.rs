//! End-to-end production: a forester sited in stock woodland must turn
//! harvested trees into wood output. Self-skips without the data corpus.
//!
//! Every step this exercises is a `FUN_0047daf0` / `FUN_0047d940` path:
//! the type-12 worker walks to a `Ware: BAUM` map cell (`FUN_0044b7e0` /
//! `FUN_0045b200`), carries `0x20` home at figure `+0x28`, and
//! `FUN_0047d940` credits the root's raw buffer `+0x0a`, which the next
//! scheduler pass converts into `+0x0c` storage at `Prodmenge` per activity
//! step.

use std::collections::HashSet;

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

/// Compiled `Ware: BAUM` selector; the forester's `Rohstoff`.
const BAUM: u8 = 0x35;

/// Every real producer in haeuser.cod carries outer `Kind: GEBAEUDE` and puts
/// its production label in the nested `HAUS_PRODTYP Kind`. `FUN_00481450`
/// allocates the live 20-byte cell record from that nested selector
/// (`1602_exe.c:92790-92892`), so the forester must get one.
#[test]
fn stock_producers_get_a_live_source_cell_record() {
    let Some(corpus) = load_corpus() else { return };
    let forester = corpus
        .cod
        .buildings
        .iter()
        .find(|b| b.properties.get("Id").is_some_and(|id| id == "22001"))
        .expect("forester definition");
    assert_eq!(forester.kind, "GEBAEUDE");
    assert_eq!(
        forester.source_production_kind_code(),
        Some(2),
        "the forester is a nested PLANTAGE"
    );
    let state = anno_sim::source_cell::SourceMapCellState::new(0, 5, 5, forester, 0)
        .expect("forester allocates a live source record");
    assert_eq!(state.source_raw_resource_ware_slot, BAUM);
    assert_eq!(state.storage_animation_capacity, 320);
    assert_eq!(state.source_raw_material_amount, 32);

    // Testing the outer kind instead admits roads and walls while excluding
    // every producer, which is what the defect looked like.
    for id in ["20501", "20521", "22001", "21075", "22003", "21515"] {
        let building = corpus
            .cod
            .buildings
            .iter()
            .find(|b| b.properties.get("Id").is_some_and(|value| value == id))
            .unwrap_or_else(|| panic!("definition {id}"));
        assert!(
            anno_sim::source_cell::SourceMapCellState::new(0, 5, 5, building, 0).is_some(),
            "producer {id} ({}, prod {:?}) must own a live source record",
            building.kind,
            building.properties.get("ProdKind"),
        );
    }
}

/// The whole chain: place a forester in stock woodland, run it, and require
/// the harvested trees to reach the root's raw buffer and then its output.
///
/// The stall after the woodland in range is exhausted is the unported crop
/// maturation timer `FUN_0047ca80`, so this asserts that output rises, not
/// that it is sustained.
#[test]
fn a_forester_in_stock_woodland_produces_wood() {
    let Some(corpus) = load_corpus() else {
        return;
    };
    let mut sim = anno_game::scenario::build_simulation(
        &corpus.szs,
        &corpus.cod,
        &corpus.defs,
        &corpus.figures,
    );
    sim.seed_source_rand(1);

    let forester_index = corpus
        .cod
        .buildings
        .iter()
        .position(|b| b.properties.get("Id").is_some_and(|id| id == "22001"))
        .expect("forester definition");

    // Stock islands ship woodland as `Ware: BAUM` map cells; the forester
    // needs some inside its compiled `Radius: 3`.
    let (island_number, island_index) = corpus
        .szs
        .islands
        .iter()
        .enumerate()
        .map(|(index, island)| {
            let trees = sim
                .source_static_map_roots
                .iter()
                .filter(|cell| cell.island == island.number && cell.source_output_ware_slot == BAUM)
                .count();
            (island.number, index, trees)
        })
        .max_by_key(|&(_, _, trees)| trees)
        .map(|(number, index, trees)| {
            assert!(trees > 0, "no stock island ships BAUM cells");
            (number, index)
        })
        .expect("at least one island");

    let trees: HashSet<(u16, u16)> = sim
        .source_static_map_roots
        .iter()
        .filter(|cell| cell.island == island_number && cell.source_output_ware_slot == BAUM)
        .map(|cell| (u16::from(cell.x), u16::from(cell.y)))
        .collect();

    // Rank 2x2 anchors by how much woodland sits inside the search window.
    let mut anchors: Vec<((i32, i32), usize)> = trees
        .iter()
        .flat_map(|&(tx, ty)| {
            (-3i32..=3).flat_map(move |dy| (-3i32..=3).map(move |dx| (dx, dy))).map(
                move |(dx, dy)| (i32::from(tx) + dx, i32::from(ty) + dy),
            )
        })
        .filter(|&(x, y)| x >= 1 && y >= 1)
        .map(|anchor| {
            let count = (-3i32..=4)
                .flat_map(|dy| (-3i32..=4).map(move |dx| (dx, dy)))
                .filter(|&(dx, dy)| {
                    u16::try_from(anchor.0 + dx)
                        .ok()
                        .zip(u16::try_from(anchor.1 + dy).ok())
                        .is_some_and(|cell| trees.contains(&cell))
                })
                .count();
            (anchor, count)
        })
        .collect();
    anchors.sort_by_key(|&(anchor, count)| (std::cmp::Reverse(count), anchor));
    anchors.dedup_by_key(|&mut (anchor, _)| anchor);

    sim.players[0].gold = 100_000;
    sim.players[0].unlock_mask = u32::MAX;
    let placed = anchors.iter().find_map(|&((x, y), _)| {
        let before = sim.buildings.len();
        anno_game::game_commands::place_building(
            &mut sim,
            &corpus.szs.islands,
            island_index,
            &corpus.defs,
            &corpus.cod,
            forester_index,
            0,
            0,
            x,
            y,
        );
        (sim.buildings.len() > before).then_some((x, y))
    });
    let (x, y) = placed.expect("a forester fits somewhere in the woodland");

    let building_index = sim.buildings.len() - 1;
    // The scenario starts the human without a warehouse on this island, so no
    // carrier can deliver the two tools the site needs. Construction is a
    // separate subsystem from the production chain under test; hand the site
    // its materials rather than modelling a supply run.
    sim.buildings[building_index].wood_needed = 0;
    sim.buildings[building_index].tools_needed = 0;
    sim.buildings[building_index].bricks_needed = 0;
    sim.buildings[building_index].construction_ms_remaining = 0;
    assert!(sim.buildings[building_index].is_built());
    assert!(sim.buildings[building_index].active);

    let root_index = sim
        .source_map_cell_states
        .iter()
        .position(|state| state.matches(island_number, x as u16, y as u16))
        .expect("the placed forester owns a live source cell record");
    assert!(sim.source_map_cell_states[root_index].is_type12_plantation_root());

    let mut peak_raw = 0;
    let mut peak_fill = 0;
    let mut peak_output = 0;
    for _ in 0..8_000 {
        sim.tick(100);
        let state = sim.source_map_cell_states[root_index];
        peak_raw = peak_raw.max(state.raw_material_stock);
        peak_fill = peak_fill.max(state.storage_fill);
        peak_output = peak_output.max(sim.buildings[building_index].output_stock);
        if peak_output >= 2 {
            break;
        }
    }

    assert!(
        peak_raw > 0,
        "no harvested tree ever reached the root's raw buffer +0x0a"
    );
    assert_eq!(
        peak_raw % 32,
        0,
        "a harvest delivers whole 1/32 units: FUN_00471c50 returns 0x20"
    );
    assert!(
        peak_fill > 0,
        "the scheduler never converted raw material into +0x0c storage"
    );
    assert!(
        peak_output >= 2,
        "the forester produced {peak_output} whole wood (raw peak {peak_raw}, fill peak {peak_fill})"
    );
}
