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
    // The human's mission needs southern crops reachable: at least one
    // free island must roll tobacco/spices/cocoa territory.
    let southern_crops = szs
        .islands
        .iter()
        .filter(|island| island.city.is_none())
        .flat_map(|island| island.fertilities)
        .filter(|f| matches!(f, 1 | 2 | 6))
        .count();
    assert!(southern_crops > 0, "no southern crops rolled");
}
