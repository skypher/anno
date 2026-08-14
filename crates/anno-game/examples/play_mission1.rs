//! Staged scripted playthrough of the first campaign mission
//! ("Halfway there", New Horizons0.szs). Stage 1: sail, found, build the
//! pioneer core (market + chapel + huts), and watch the settlement grow.
use anno_sim::commands::Command;
use anno_sim::types::Good;

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
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let mut sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    sim.seed_source_rand(1);

    // --- Stage 1a: sail to island 10 and found ---
    let ship = sim.trade_ships.iter().position(|s| s.name == "Verena").unwrap() as u32;
    let island = szs.islands.iter().find(|i| i.number == 10).unwrap().clone();
    let mi = sim.island_maps.iter().position(|m| m.island_id == 10).unwrap();
    // Prefer a west-coast anchor with land around it (skip the corner).
    let anchor = (2..island.height as i32 - 2)
        .flat_map(|y| (2..island.width as i32 - 2).map(move |x| (x, y)))
        .find(|&(x, y)| {
            sim.island_maps[mi].is_coastal(x, y)
                && sim.island_maps[mi].is_walkable(x + 1, y)
                && sim.island_maps[mi].is_walkable(x + 2, y)
        })
        .expect("coastal anchor with hinterland");
    let world = (
        i32::from(island.x_pos) + anchor.0,
        i32::from(island.y_pos) + anchor.1,
    );
    assert!(sim.apply_command(&Command::SailShip {
        player: 0,
        ship_index: ship,
        world_x: world.0,
        world_y: world.1
    }));
    for _ in 0..3000 {
        sim.tick(100);
        let s = &sim.trade_ships[ship as usize];
        if (s.world_x - world.0).abs() + (s.world_y - world.1).abs() < 12 {
            break;
        }
    }
    let ok = anno_game::game_commands::apply_game_command(
        &mut sim,
        &szs.islands,
        &cod,
        &defs,
        &Command::FoundKontor {
            player: 0,
            ship_index: ship,
            island: 10,
            tile_x: anchor.0 as u16,
            tile_y: anchor.1 as u16,
        },
    );
    println!("founded at {anchor:?}: {ok}");
    assert!(ok);

    // --- Stage 1b: place market, chapel, huts on free land ---
    let find_spot = |sim: &anno_sim::simulation::Simulation, w: u8, h: u8, used: &[(i32, i32, u8, u8)]| {
        let map = &sim.island_maps[mi];
        // Stay inside the Kontor/market service radius (16) so houses
        // keep coverage: order candidates by distance from the anchor.
        let mut candidates: Vec<(i32, i32)> = (2..island.height as i32 - i32::from(h) - 2)
            .flat_map(|y| (2..island.width as i32 - i32::from(w) - 2).map(move |x| (x, y)))
            .collect();
        candidates.sort_by_key(|&(x, y)| (x - anchor.0).abs() + (y - anchor.1).abs());
        candidates
            .into_iter()
            .find(|&(x, y)| {
                (0..i32::from(h)).all(|dy| (0..i32::from(w)).all(|dx| map.is_walkable(x + dx, y + dy)))
                    && !used.iter().any(|&(ux, uy, uw, uh)| {
                        x < ux + i32::from(uw) && ux < x + i32::from(w)
                            && y < uy + i32::from(uh) && uy < y + i32::from(h)
                    })
            })
    };
    let mut used: Vec<(i32, i32, u8, u8)> = Vec::new();
    let mut build = |sim: &mut anno_sim::simulation::Simulation, used: &mut Vec<(i32, i32, u8, u8)>, def_index: u16, label: &str| {
        let (w, h) = (defs[def_index as usize].width, defs[def_index as usize].height);
        let Some((x, y)) = find_spot(sim, w, h, used) else {
            println!("no spot for {label}");
            return false;
        };
        let placed = anno_game::game_commands::apply_game_command(
            sim,
            &szs.islands,
            &cod,
            &defs,
            &Command::PlaceBuilding {
                player: 0,
                island: 10,
                tile_x: x as u16,
                tile_y: y as u16,
                def_index,
                orientation: 0,
            },
        );
        println!("build {label} at ({x},{y}): {placed}");
        if placed {
            used.push((x, y, w, h));
        }
        placed
    };
    build(&mut sim, &mut used, 468, "market");
    build(&mut sim, &mut used, 463, "chapel");
    for n in 0..10 {
        build(&mut sim, &mut used, 414, &format!("hut{n}"));
    }

    // --- Stage 1c: run 10 minutes of sim and report ---
    for minute in 1..=10u32 {
        for _ in 0..600 {
            sim.tick(100);
            sim.drain_source_kind13_replacements(&cod);
        }
        let city = sim
            .source_cities
            .active_records()
            .into_iter()
            .find(|c| c.island_id == 10 && c.owner_slot == 0)
            .unwrap();
        let wh = sim
            .warehouses
            .iter()
            .find(|w| w.island_id == 10 && w.owner == 0)
            .unwrap();
        let p = &sim.players[0];
        println!(
            "min {minute}: pop={:?} sat={:?} food={} wood={} gold={} objectives={:?}",
            city.tier_population,
            city.satisfaction_by_group,
            wh.stock(Good::Food),
            wh.stock(Good::Wood),
            p.gold,
            sim.objectives
                .items
                .iter()
                .map(|(o, done)| (o.label(), *done))
                .collect::<Vec<_>>(),
        );
    }
}
