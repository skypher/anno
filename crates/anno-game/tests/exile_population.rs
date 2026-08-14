//! End-to-end regression for the exact population chain on Exile:
//! KONTOR2 store seeding -> `FUN_0047f8a0` demand cycle -> kind-13
//! house transfers -> per-player mirror. Self-skips without the data
//! corpus (extracted/ is gitignored).

fn load_exile() -> Option<anno_sim::simulation::Simulation> {
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
    Some(sim)
}

#[test]
fn exile_population_holds_at_the_house_bound() {
    let Some(mut sim) = load_exile() else {
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
    }
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
