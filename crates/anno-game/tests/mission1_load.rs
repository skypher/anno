//! First-mission loading: every New Horizons0 island must have terrain
//! after stock-island instantiation. Self-skips without the data corpus.

#[test]
fn new_horizons0_instantiates_all_stock_islands() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let Ok(szs_data) = std::fs::read(root.join("extracted/Szenes/New Horizons0.szs")) else {
        println!("Skipping test: data corpus not found");
        return;
    };
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).expect("parse New Horizons0");
    anno_game::scenario::instantiate_stock_islands(&mut szs, &root.join("extracted"), 1);
    assert_eq!(szs.islands.len(), 14);
    for island in &szs.islands {
        assert!(
            !island.tiles.is_empty(),
            "island {} ({}x{}) has no terrain after instantiation",
            island.number,
            island.width,
            island.height
        );
    }
    // Deterministic: the same seed must pick the same islands.
    let mut again = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut again, &root.join("extracted"), 1);
    for (a, b) in szs.islands.iter().zip(again.islands.iter()) {
        assert_eq!(a.tiles.len(), b.tiles.len());
        assert_eq!(a.fertilities, b.fertilities);
    }

    // Fertility is authored in each island's `INSEL5[0x5C]` crop mask
    // and must survive instantiation untouched — `FUN_00469770`
    // (`1602_exe.c:73780-73810`) preserves `island+0x5c` whenever the
    // climate byte matches, and never randomises it. These are the
    // real masks New Horizons0 ships.
    let authored: Vec<(u8, u32)> = szs
        .islands
        .iter()
        .map(|island| (island.number, island.crop_flags()))
        .collect();
    assert_eq!(
        authored,
        vec![
            (0, 0x55),  // grain, spices, cotton, cocoa (STADT4, owner slot 1)
            (1, 0x23),  // grain, tobacco, vines (STADT4, owner slot 1)
            (2, 0x09),  // grain, sugarcane
            (3, 0x01),  // grain only
            (4, 0x11),  // grain, cotton
            (5, 0x03),  // grain, tobacco
            (6, 0x03),  // grain, tobacco
            (7, 0x11),  // grain, cotton
            (8, 0x21),  // grain, vines
            (9, 0x05),  // grain, spices
            (10, 0x41), // grain, cocoa
            (11, 0x01), // grain only
            (13, 0x01), // grain only
            (14, 0x01), // grain only
        ],
        "New Horizons0's authored crop masks"
    );

    // The mission needs the southern chains reachable on a free
    // island: tobacco, spices and cocoa are all authored.
    use anno_formats::szs::Fertility;
    for crop in [Fertility::Tobacco, Fertility::Spices, Fertility::Cocoa] {
        assert!(
            szs.islands
                .iter()
                .any(|island| island.city.is_none() && island.has_fertility(crop)),
            "no free island carries {crop:?}"
        );
    }
}

#[test]
fn verena_spawns_with_the_authored_starting_cargo() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let (Ok(szs_data), Ok(cod_data)) = (
        std::fs::read(root.join("extracted/Szenes/New Horizons0.szs")),
        std::fs::read(root.join("extracted/haeuser.cod")),
    ) else {
        println!("Skipping test: data corpus not found");
        return;
    };
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, &root.join("extracted"), 1);
    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("extracted/figuren.cod"))
        .map(|bytes| anno_formats::figuren::FiguresFile::parse(&bytes))
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    let verena = sim
        .trade_ships
        .iter()
        .find(|ship| ship.name == "Verena")
        .expect("player trader spawns");
    assert_eq!(verena.owner, 0);
    // Classic mission-1 loadout: 50 + 10 tools, 50 wood, 20 food
    // (ship-cargo ids 2/7/4, live-verified against the hold UI).
    use anno_sim::types::Good;
    assert_eq!(verena.cargo_amount(Good::Tools), 60);
    assert_eq!(verena.cargo_amount(Good::Wood), 50);
    assert_eq!(verena.cargo_amount(Good::Food), 20);
}

#[test]
fn verena_sails_to_a_free_island_on_command() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let (Ok(szs_data), Ok(cod_data)) = (
        std::fs::read(root.join("extracted/Szenes/New Horizons0.szs")),
        std::fs::read(root.join("extracted/haeuser.cod")),
    ) else {
        println!("Skipping test: data corpus not found");
        return;
    };
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, &root.join("extracted"), 1);
    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("extracted/figuren.cod"))
        .map(|bytes| anno_formats::figuren::FiguresFile::parse(&bytes))
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let mut sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    sim.seed_source_rand(1);
    let ship_index = sim
        .trade_ships
        .iter()
        .position(|ship| ship.name == "Verena")
        .expect("Verena spawns") as u32;
    let start = (
        sim.trade_ships[ship_index as usize].world_x,
        sim.trade_ships[ship_index as usize].world_y,
    );
    // Island 10 sits at (201,231) 50x52; aim just off its west coast.
    let target = (198i32, 255i32);
    assert!(sim.apply_command(&anno_sim::commands::Command::SailShip {
        player: 0,
        ship_index,
        world_x: target.0,
        world_y: target.1,
    }));
    let mut dist = i32::MAX;
    for _ in 0..3_000 {
        sim.tick(100);
        let ship = &sim.trade_ships[ship_index as usize];
        dist = (ship.world_x - target.0).abs() + (ship.world_y - target.1).abs();
        if dist < 12 {
            break;
        }
    }
    let ship = &sim.trade_ships[ship_index as usize];
    assert!(
        dist < 12,
        "Verena should approach the target: start {start:?}, now ({}, {}), dist {dist}",
        ship.world_x,
        ship.world_y
    );
}

#[test]
fn verena_founds_a_settlement_on_the_free_island() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let (Ok(szs_data), Ok(cod_data)) = (
        std::fs::read(root.join("extracted/Szenes/New Horizons0.szs")),
        std::fs::read(root.join("extracted/haeuser.cod")),
    ) else {
        println!("Skipping test: data corpus not found");
        return;
    };
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, &root.join("extracted"), 1);
    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("extracted/figuren.cod"))
        .map(|bytes| anno_formats::figuren::FiguresFile::parse(&bytes))
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let mut sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    sim.seed_source_rand(1);
    let ship_index = sim
        .trade_ships
        .iter()
        .position(|ship| ship.name == "Verena")
        .expect("Verena spawns") as u32;

    // Island 10 at (201,231): find a coastal tile on its western side.
    let island = szs.islands.iter().find(|i| i.number == 10).unwrap();
    let map_idx = sim
        .island_maps
        .iter()
        .position(|m| m.island_id == 10)
        .unwrap();
    let anchor = (0..island.height as i32)
        .flat_map(|y| (0..island.width as i32).map(move |x| (x, y)))
        .find(|&(x, y)| sim.island_maps[map_idx].is_coastal(x, y))
        .expect("island 10 has a coastline");
    let world = (
        i32::from(island.x_pos) + anchor.0,
        i32::from(island.y_pos) + anchor.1,
    );

    // Sail alongside, then found.
    assert!(sim.apply_command(&anno_sim::commands::Command::SailShip {
        player: 0,
        ship_index,
        world_x: world.0,
        world_y: world.1,
    }));
    for _ in 0..3_000 {
        sim.tick(100);
        let ship = &sim.trade_ships[ship_index as usize];
        if (ship.world_x - world.0).abs() + (ship.world_y - world.1).abs() < 12 {
            break;
        }
    }
    let found = anno_game::game_commands::apply_game_command(
        &mut sim,
        &szs.islands,
        &cod,
        &defs,
        &anno_sim::commands::Command::FoundKontor {
            player: 0,
            ship_index,
            island: 10,
            tile_x: anchor.0 as u16,
            tile_y: anchor.1 as u16,
        },
    );
    assert!(found, "founding must succeed at the coastal anchor");

    // The settlement exists: city record, warehouse with the ship cargo,
    // and the Kontor building.
    assert!(sim
        .source_cities
        .active_records()
        .iter()
        .any(|city| city.island_id == 10 && city.owner_slot == 0));
    let warehouse = sim
        .warehouses
        .iter()
        .find(|w| w.island_id == 10 && w.owner == 0)
        .expect("island warehouse created");
    use anno_sim::types::Good;
    assert_eq!(warehouse.stock(Good::Tools), 50, "capacity-clamped tools");
    assert_eq!(warehouse.stock(Good::Wood), 50);
    assert_eq!(warehouse.stock(Good::Food), 20);
    assert!(sim
        .buildings
        .iter()
        .any(|b| b.island_id == 10 && b.owner == 0));
    let ship = &sim.trade_ships[ship_index as usize];
    assert_eq!(
        ship.cargo_amount(Good::Tools),
        10,
        "store overflow stays aboard"
    );
}

/// The scenario's AUFTRAG4 goals must reach the simulation. New Horizons0
/// authors flag bit 0 (population) with the triple [100, 4, 100]: one
/// hundred inhabitants of whom one hundred are Aristocrats. A simulation
/// built with an empty objective set can never report the mission won.
#[test]
fn new_horizons0_loads_its_mission_objectives() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let (Ok(szs_data), Ok(cod_data)) = (
        std::fs::read(root.join("extracted/Szenes/New Horizons0.szs")),
        std::fs::read(root.join("extracted/haeuser.cod")),
    ) else {
        println!("Skipping test: data corpus not found");
        return;
    };
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, &root.join("extracted"), 1);
    let goals = szs.mission.as_ref().expect("New Horizons0 has AUFTRAG4").goals();
    let primary = goals.primary.expect("primary population goal");
    assert_eq!(primary.total, 100);
    assert_eq!(primary.tier, Some(4));
    assert_eq!(primary.at_tier, 100);

    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("extracted/figuren.cod"))
        .map(|bytes| anno_formats::figuren::FiguresFile::parse(&bytes))
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    let labels: Vec<String> = sim
        .objectives
        .items
        .iter()
        .map(|(objective, _)| objective.label())
        .collect();
    assert_eq!(
        labels,
        vec![
            "Reach 100 total inhabitants".to_string(),
            "Reach 100 Aristocrats".to_string(),
        ],
        "the mission's own goals must replace the starter set"
    );
    assert!(sim.objectives.items.iter().all(|(_, done)| !done));
}
