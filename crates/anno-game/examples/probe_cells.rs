//! Histogram the live source map cells of one island so the mission driver
//! can site harvesters (forester, fishery) where their raw resource actually
//! is. Usage: `cargo run --release -p anno-game --example probe_cells`
fn main() {
    let root = std::path::Path::new("extracted");
    let szs_data = std::fs::read(root.join("Szenes/New Horizons0.szs")).unwrap();
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, root, 1);
    let cod_data = std::fs::read(root.join("haeuser.cod")).unwrap();
    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("figuren.cod"))
        .map(|b| anno_formats::figuren::FiguresFile::parse(&b))
        .unwrap();
    let sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);

    let mut by_kind: std::collections::BTreeMap<u8, usize> = std::collections::BTreeMap::new();
    for cell in sim.source_map_cell_states.iter().filter(|c| c.island == 10) {
        *by_kind.entry(cell.source_production_kind_code).or_default() += 1;
    }
    println!("island 10 live cells by production kind: {by_kind:?}");

    // Harvest targets are looked up in the STATIC root table, not the live
    // cell states (`try_assign_source_plantation_worker_target`).
    let mut static_by_kind: std::collections::BTreeMap<u8, usize> =
        std::collections::BTreeMap::new();
    for cell in sim.source_static_map_roots.iter().filter(|c| c.island == 10) {
        *static_by_kind
            .entry(cell.source_production_kind_code)
            .or_default() += 1;
    }
    println!("island 10 static roots by production kind: {static_by_kind:?}");
    let mut owners: std::collections::BTreeMap<u8, usize> = std::collections::BTreeMap::new();
    for cell in sim
        .source_static_map_roots
        .iter()
        .filter(|c| c.island == 10 && c.source_production_kind_code == 9)
    {
        *owners.entry(cell.source_map_owner_slot).or_default() += 1;
    }
    println!("ripe (kind 9) roots by owner slot: {owners:?}");
    let mut wares: std::collections::BTreeMap<u8, usize> = std::collections::BTreeMap::new();
    for cell in sim
        .source_static_map_roots
        .iter()
        .filter(|c| c.island == 10 && c.source_production_kind_code == 9)
    {
        *wares.entry(cell.source_output_ware_slot).or_default() += 1;
    }
    println!("ripe (kind 9) roots by output ware slot: {wares:?}");
    // What raw ware does the forester actually request?
    let forester = &cod.buildings[402];
    println!(
        "forester Rohstoff slot = {:?}, Ware slot = {:?}, radius = {}",
        forester.source_raw_resource_ware_slot(),
        forester.source_ware_slot(),
        defs[402].radius,
    );
    // And does an unowned (slot 7) resource cell admit a player-0 worker?
    if let Some(cell) = sim
        .source_static_map_roots
        .iter()
        .find(|c| c.island == 10 && c.source_production_kind_code == 9)
    {
        for owner in [0u8, 7] {
            println!(
                "  cell({},{}) ware={} admits owner {}: {}",
                cell.x,
                cell.y,
                cell.source_output_ware_slot,
                owner,
                cell.is_plantation_worker_target(owner, cell.source_output_ware_slot),
            );
        }
    }

    // Where are the harvestable raw-resource cells? Print the bounding box and
    // a coarse density map so a harvester can be sited in the thick of them.
    let harvestable: Vec<(u8, u8, u8)> = sim
        .source_map_cell_states
        .iter()
        .filter(|c| c.island == 10 && matches!(c.source_production_kind_code, 9 | 10))
        .map(|c| (c.x, c.y, c.source_production_kind_code))
        .collect();
    println!("harvestable-ish cells: {}", harvestable.len());
    if let (Some(minx), Some(maxx), Some(miny), Some(maxy)) = (
        harvestable.iter().map(|c| c.0).min(),
        harvestable.iter().map(|c| c.0).max(),
        harvestable.iter().map(|c| c.1).min(),
        harvestable.iter().map(|c| c.1).max(),
    ) {
        println!("bbox x {minx}..{maxx}  y {miny}..{maxy}");
    }
    for kind in [9u8, 10] {
        let n = harvestable.iter().filter(|c| c.2 == kind).count();
        println!("  kind {kind}: {n}");
    }
    // Best 2x2 site by count of harvestable cells within radius 3.
    let mut best = (0usize, (0i32, 0i32));
    for y in 0..52i32 {
        for x in 0..50i32 {
            let n = harvestable
                .iter()
                .filter(|c| {
                    (i32::from(c.0) - x).abs() <= 3 && (i32::from(c.1) - y).abs() <= 3
                })
                .count();
            if n > best.0 {
                best = (n, (x, y));
            }
        }
    }
    println!("densest radius-3 site: {:?} with {} cells", best.1, best.0);

    // Replicate the passing corpus test, but pinned to island 10 and with no
    // Kontor founded, to separate "this island" from "the founding flow".
    let mut sim = sim;
    sim.seed_source_rand(1);
    sim.players[0].gold = 100_000;
    sim.players[0].unlock_mask = u32::MAX;
    let island_index = szs.islands.iter().position(|i| i.number == 10).unwrap();
    let trees: Vec<(i32, i32)> = sim
        .source_static_map_roots
        .iter()
        .filter(|c| c.island == 10 && c.source_output_ware_slot == 53)
        .map(|c| (i32::from(c.x), i32::from(c.y)))
        .collect();
    let mut sites: Vec<((i32, i32), usize)> = Vec::new();
    for y in 2..50i32 {
        for x in 2..48i32 {
            let n = trees
                .iter()
                .filter(|&&(cx, cy)| {
                    let (dx, dy) = (cx - x, cy - y);
                    dx * dx + dy * dy <= 9
                })
                .count();
            if n > 0 {
                sites.push(((x, y), n));
            }
        }
    }
    sites.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let placed = sites.iter().take(60).find_map(|&((x, y), n)| {
        let before = sim.buildings.len();
        anno_game::game_commands::place_building(
            &mut sim,
            &szs.islands,
            island_index,
            &defs,
            &cod,
            402,
            0,
            0,
            x,
            y,
        );
        (sim.buildings.len() > before).then_some((x, y, n))
    });
    let Some((x, y, n)) = placed else {
        println!("could not place a forester anywhere in island 10 woodland");
        return;
    };
    let bi = sim.buildings.len() - 1;
    sim.buildings[bi].wood_needed = 0;
    sim.buildings[bi].tools_needed = 0;
    sim.buildings[bi].bricks_needed = 0;
    sim.buildings[bi].construction_ms_remaining = 0;
    println!("forester at ({x},{y}) with {n} trees in circle");
    let root = sim
        .source_map_cell_states
        .iter()
        .position(|s| s.matches(10, x as u16, y as u16))
        .expect("live cell record");
    let mut peak_raw = 0;
    let mut peak_fill = 0;
    for step in 0..8_000 {
        sim.tick(100);
        let s = sim.source_map_cell_states[root];
        peak_raw = peak_raw.max(s.raw_material_stock);
        peak_fill = peak_fill.max(s.storage_fill);
        if step % 2000 == 0 || (peak_raw > 0 && step % 500 == 0) {
            let routes: Vec<String> = sim
                .figures
                .iter()
                .filter(|f| f.is_active() && f.origin_island == 10)
                .map(|f| format!("{:?}", f.source_worker_route))
                .collect();
            println!(
                "  step {step}: raw={} fill={} out={} figures={routes:?}",
                s.raw_material_stock, s.storage_fill, sim.buildings[bi].output_stock
            );
        }
    }
    println!("peak raw={peak_raw} fill={peak_fill} out={}", sim.buildings[bi].output_stock);
}
