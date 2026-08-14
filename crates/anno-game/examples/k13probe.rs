fn main() {
    let data = std::fs::read("extracted/Szenes/New Horizons0.szs").unwrap();
    let mut szs = anno_formats::szs::SzsFile::parse(&data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, std::path::Path::new("extracted"), 1);
    let cod_data = std::fs::read("extracted/haeuser.cod").unwrap();
    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let locs = anno_sim::data_bridge::source_kind13_locations_from_scenario(&szs, &cod);
    let mut by: std::collections::BTreeMap<(u8, u8), (usize, u32)> = Default::default();
    for l in locs.active_locations() {
        let e = by.entry((l.island_id, l.population_group)).or_default();
        e.0 += 1; e.1 += u32::from(l.amount);
    }
    for ((isl, group), (n, amt)) in by {
        println!("island {isl} group {group}: {n} locs, total amount {amt} (~{} res)", amt >> 6);
    }
}
