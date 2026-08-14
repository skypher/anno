//! Report the instantiated free islands of the first mission: size, climate
//! half, rolled fertilities and ore records — the inputs the mission driver
//! needs to choose which production chains an island can actually host.
fn main() {
    let root = std::path::Path::new("extracted");
    let szs_data = std::fs::read(root.join("Szenes/New Horizons0.szs")).unwrap();
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, root, 1);
    for island in &szs.islands {
        println!(
            "island {:3} at ({:3},{:3}) {:3}x{:3} tiles={:5} fert={:?}",
            island.number,
            island.x_pos,
            island.y_pos,
            island.width,
            island.height,
            island.tiles.len(),
            island.active_fertilities(),
        );
    }
}
