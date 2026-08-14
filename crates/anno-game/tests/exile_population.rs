//! End-to-end regression for the exact population chain on Exile:
//! KONTOR2 store seeding -> `FUN_0047f8a0` demand cycle -> kind-13
//! house transfers -> per-player mirror. Self-skips without the data
//! corpus (extracted/ is gitignored).

fn load_exile() -> Option<(anno_sim::simulation::Simulation, anno_formats::cod::CodFile)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .to_path_buf();
    let szs_data = std::fs::read(root.join("extracted/Szenes/Exile.szs")).ok()?;
    let cod_data = std::fs::read(root.join("extracted/haeuser.cod")).ok()?;
    let szs = anno_formats::szs::SzsFile::parse(&szs_data).ok()?;
    let cod = anno_formats::cod::CodFile::parse(&cod_data).ok()?;
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("extracted/figuren.cod"))
        .map(|bytes| anno_formats::figuren::FiguresFile::parse(&bytes))
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let mut sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    sim.seed_source_rand(1);
    Some((sim, cod))
}

#[test]
fn exile_operating_costs_match_the_live_city_accumulator() {
    // The live original's human-city maintenance accumulator (`+0x1d8`,
    // read via winedbg at +30 s) is exactly 210 and the native city 10;
    // the AI cities read 160/195/255/250 but their live AI had already
    // constructed additional buildings, so only a lower bound holds.
    let Some((mut sim, _cod)) = load_exile() else {
        println!("Skipping test: data corpus not found");
        return;
    };
    for _ in 0..200 {
        sim.tick(100);
    }
    assert_eq!(sim.players[0].building_maintenance, 210);
    assert_eq!(sim.players[5].building_maintenance, 10, "native city");
    assert!(sim.players[1].building_maintenance >= 400); // live 450 after AI builds
    assert!(sim.players[3].building_maintenance >= 350); // live 410 after AI builds
}

#[test]
fn exile_house_coverage_scan_reproduces_authored_flags() {
    // The scenario's SIEDLER records are frozen output of the editor's
    // own `FUN_00482120` coverage scan (predating the authored well, so
    // the BRUNNEN bit 0x1000 is ignored below). After the load-time
    // rescan, every residence must carry identical state bits,
    // infrastructure lifecycle bits (including the 3×3 Kontor at the
    // executable's RADIUS_HQ = 16), and marketplace distance-class
    // variants.
    let Some((sim, _cod)) = load_exile() else {
        println!("Skipping test: data corpus not found");
        return;
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let szs_data = std::fs::read(root.join("extracted/Szenes/Exile.szs")).unwrap();
    let szs = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    let authored: std::collections::HashMap<(u8, u8), (u8, u16, u8)> = szs
        .settler_houses
        .iter()
        .filter(|house| house.island_id == 0)
        .map(|house| {
            (
                (house.tile_x, house.tile_y),
                (
                    house.state_bits,
                    house.lifecycle_flags,
                    house.variant & 0x0f,
                ),
            )
        })
        .collect();
    let mut compared = 0;
    for location in sim
        .source_kind13_locations
        .active_locations()
        .into_iter()
        .filter(|location| location.island_id == 0)
    {
        let Some(&(state, lifecycle, variant)) =
            authored.get(&(location.tile_x, location.tile_y))
        else {
            continue;
        };
        assert_eq!(
            (
                location.state_bits,
                location.lifecycle_flags & !0x1000,
                location.variant & 0x0f,
            ),
            (state, lifecycle, variant),
            "coverage mismatch at ({}, {})",
            location.tile_x,
            location.tile_y
        );
        compared += 1;
    }
    assert_eq!(compared, 26, "all authored residences compared");
}

#[test]
fn exile_population_holds_at_the_house_bound() {
    let Some((mut sim, cod)) = load_exile() else {
        println!("Skipping test: data corpus not found");
        return;
    };

    // Authored state: 26 settler houses at capacity = the STADT4 156.
    let residents: u32 = sim
        .source_kind13_locations
        .active_locations()
        .into_iter()
        .filter(|location| location.island_id == 0)
        .map(|location| u32::from(location.amount) >> 6)
        .sum();
    assert_eq!(residents, 156);
    assert_eq!(sim.players[0].population, [0, 156, 0, 0, 0]);

    // The seeded city store matches the original's live t0 read.
    let warehouse = sim
        .warehouses
        .iter()
        .find(|warehouse| warehouse.active && warehouse.island_id == 0 && warehouse.owner == 0)
        .expect("human island Kontor");
    assert_eq!(
        warehouse.city_stock_fixed(anno_sim::types::Good::Food),
        800
    );
    assert_eq!(
        warehouse.city_stock_fixed(anno_sim::types::Good::Alcohol),
        509
    );

    // Twenty source economy cycles (10 s each): the stocked city keeps
    // full satisfaction and the house-bounded population stays put —
    // the live original holds 156 under the same conditions, where the
    // former invented growth exploded past 400.
    for _ in 0..2_000 {
        sim.tick(100);
        sim.drain_source_kind13_replacements(&cod);
    }
    // The coverage scan grants chapel + tavern (+ Kontor/market) flags,
    // but Exile ships no school or college, so the settler->citizen
    // transition (`FUN_0047bfa0` requires lifecycle 0x20 or 0x100) stays
    // gated and the reservation pends — exactly the live original's
    // observed state. The total holds at the authored roster.
    assert_eq!(
        sim.players[0].population,
        [0, 156, 0, 0, 0],
        "house-bounded population must hold at the authored 156"
    );
    assert_eq!(sim.players[0].satisfaction[1], 128, "settlers stay satisfied");
    let food_after = sim
        .warehouses
        .iter()
        .find(|warehouse| warehouse.active && warehouse.island_id == 0 && warehouse.owner == 0)
        .map(|warehouse| warehouse.city_stock_fixed(anno_sim::types::Good::Food))
        .unwrap();
    assert!(
        food_after < 800,
        "the exact demand cycle must consume food from the seeded store"
    );
}
