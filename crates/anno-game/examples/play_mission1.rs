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
    let ship = sim
        .trade_ships
        .iter()
        .position(|s| s.name == "Verena")
        .unwrap() as u32;
    let island = szs.islands.iter().find(|i| i.number == 10).unwrap().clone();
    let mi = sim
        .island_maps
        .iter()
        .position(|m| m.island_id == 10)
        .unwrap();
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
    let find_spot =
        |sim: &anno_sim::simulation::Simulation, w: u8, h: u8, used: &[(i32, i32, u8, u8)]| {
            let map = &sim.island_maps[mi];
            // Stay inside the Kontor/market service radius (16) so houses
            // keep coverage: order candidates by distance from the anchor.
            let mut candidates: Vec<(i32, i32)> = (2..island.height as i32 - i32::from(h) - 2)
                .flat_map(|y| (2..island.width as i32 - i32::from(w) - 2).map(move |x| (x, y)))
                .collect();
            candidates.sort_by_key(|&(x, y)| (x - anchor.0).abs() + (y - anchor.1).abs());
            candidates.into_iter().find(|&(x, y)| {
                (0..i32::from(h))
                    .all(|dy| (0..i32::from(w)).all(|dx| map.is_walkable(x + dx, y + dy)))
                    && !used.iter().any(|&(ux, uy, uw, uh)| {
                        x < ux + i32::from(uw)
                            && ux < x + i32::from(w)
                            && y < uy + i32::from(uh)
                            && uy < y + i32::from(h)
                    })
            })
        };
    let mut used: Vec<(i32, i32, u8, u8)> = Vec::new();
    let build = |sim: &mut anno_sim::simulation::Simulation,
                 used: &mut Vec<(i32, i32, u8, u8)>,
                 def_index: u16,
                 label: &str| {
        let (w, h) = (
            defs[def_index as usize].width,
            defs[def_index as usize].height,
        );
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
    // A harvester only works if its raw resource lies inside its own Radius:
    // the worker searches the static map roots for ripe (kind 9) cells whose
    // output ware matches the building's `Rohstoff`. Island 10 carries 559
    // BAUM (ware 53), 627 GRAS (52) and 585 FISCHE (57) cells, but none of
    // them next to the founding beach — siting by "nearest free tile" gives a
    // forester with no trees. Pick the spot that maximises matching cells in
    // range while staying inside the Kontor's radius-16 coverage.
    let build_on_resource = |sim: &mut anno_sim::simulation::Simulation,
                             used: &mut Vec<(i32, i32, u8, u8)>,
                             def_index: u16,
                             ware: u8,
                             label: &str| {
        let def = &defs[def_index as usize];
        let (w, h) = (def.width, def.height);
        let radius = i32::from(def.radius).max(1);
        let cells: Vec<(i32, i32)> = sim
            .source_static_map_roots
            .iter()
            .filter(|c| {
                c.island == 10
                    && c.source_production_kind_code == 9
                    && c.source_output_ware_slot == ware
            })
            .map(|c| (i32::from(c.x), i32::from(c.y)))
            .collect();
        let map = &sim.island_maps[mi];
        let mut best: Option<(usize, (i32, i32))> = None;
        for y in 2..island.height as i32 - i32::from(h) - 2 {
            for x in 2..island.width as i32 - i32::from(w) - 2 {
                // Stay inside the Kontor's radius-16 claim. A producer on
                // unclaimed ground still harvests, but the city cart filters
                // suppliers by settlement slot, so its output is stranded —
                // and the original would refuse the placement outright.
                let (dx, dy) = (x - anchor.0, y - anchor.1);
                if dx * dx + dy * dy > 16 * 16 {
                    continue;
                }
                let free = (0..i32::from(h))
                    .all(|dy| (0..i32::from(w)).all(|dx| map.is_walkable(x + dx, y + dy)))
                    && !used.iter().any(|&(ux, uy, uw, uh)| {
                        x < ux + i32::from(uw)
                            && ux < x + i32::from(w)
                            && y < uy + i32::from(uh)
                            && uy < y + i32::from(h)
                    });
                if !free {
                    continue;
                }
                // The worker's search grid is a circle of the building's
                // compiled `Radius` (`FUN_00404d70` rows), not a box, so rank
                // sites by cells actually inside that circle.
                let n = cells
                    .iter()
                    .filter(|&&(cx, cy)| {
                        let (dx, dy) = (cx - x, cy - y);
                        dx * dx + dy * dy <= radius * radius
                    })
                    .count();
                if n > 0 && best.is_none_or(|(bn, _)| n > bn) {
                    best = Some((n, (x, y)));
                }
            }
        }
        let Some((n, (x, y))) = best else {
            println!("no {label} site with ware {ware} in range");
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
        println!("build {label} at ({x},{y}) with {n} ware-{ware} cells in range: {placed}");
        if placed {
            used.push((x, y, w, h));
        }
        placed
    };

    // Build order is bounded by the ship's 50 wood: the forester is the only
    // wood source and costs none, so it goes up first, then food, then the
    // civic core, then houses, then the cloth chain. Every def here carries
    // `Bauinfra: INFRA_NIX` and is placeable under the campaign's `0x3` mask.
    // Ware slots are `0x2d + bit`: 52 GRAS, 53 BAUM, 57 FISCHE.
    build_on_resource(&mut sim, &mut used, 402, 53, "forester"); //  0 wood, 2 tools
    build_on_resource(&mut sim, &mut used, 270, 57, "fishery"); //   5 wood, 3 tools
    build(&mut sim, &mut used, 468, "market"); // 10 wood,  4 tools
    build(&mut sim, &mut used, 463, "chapel"); //  5 wood,  2 tools
    for n in 0..4 {
        build(&mut sim, &mut used, 414, &format!("hut{n}")); // 3 wood each
    }
    // Cloth needs no fertility and no rung: two sheep farms feed one weaver
    // (the hut consumes 2 Wool per Cloth). The farms graze ripe GRAS cells.
    build_on_resource(&mut sim, &mut used, 412, 52, "sheep0"); //  4 wood, 2 tools
    build_on_resource(&mut sim, &mut used, 412, 52, "sheep1"); //  4 wood, 2 tools
    build(&mut sim, &mut used, 388, "weaver"); //  6 wood,  3 tools

    // Per-producer live cell state: is it scheduled, is raw material arriving,
    // is finished stock piling up locally instead of reaching the warehouse?
    let cell_report = |sim: &anno_sim::simulation::Simulation| {
        for cell in sim
            .source_map_cell_states
            .iter()
            .filter(|c| c.island == 10 && (1..=8).contains(&c.source_production_kind_code))
        {
            println!(
                "  cell ({},{}) prodkind={} act={} raw={} work={} fill={} cap={} sched={} blocked={}",
                cell.x,
                cell.y,
                cell.source_production_kind_code,
                cell.activity,
                cell.raw_material_stock,
                cell.work_material_stock,
                cell.storage_fill,
                cell.storage_animation_capacity,
                cell.scheduler_enabled,
                cell.scheduler_blocked,
            );
        }
        let mut routes: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for figure in sim
            .figures
            .iter()
            .filter(|f| f.is_active() && f.origin_island == 10)
        {
            *routes
                .entry(format!(
                    "{:?}/good{}",
                    figure.source_worker_route, figure.carried_good
                ))
                .or_default() += 1;
        }
        println!("  worker figures on island 10: {routes:?}");
        let carts = sim
            .figures
            .iter()
            .filter(|f| {
                f.is_active()
                    && f.origin_island == 10
                    && f.source_worker_route == anno_sim::entity::SourceWorkerRoute::None
            })
            .count();
        println!("  non-harvest figures from island 10: {carts}");
    };

    // Which buildings are still owed materials / still under construction?
    let site_report = |sim: &anno_sim::simulation::Simulation| {
        for b in sim
            .buildings
            .iter()
            .filter(|b| b.island_id == 10 && b.owner == 0)
        {
            if b.construction_ms_remaining > 0 || b.wood_needed > 0 || b.tools_needed > 0 {
                println!(
                    "  site def={} at ({},{}) build_ms={} owes wood={} tools={} bricks={}",
                    b.def_id,
                    b.tile_x,
                    b.tile_y,
                    b.construction_ms_remaining,
                    b.wood_needed,
                    b.tools_needed,
                    b.bricks_needed,
                );
            }
        }
    };

    // --- Stage 1c: run 10 minutes of sim and report ---
    let report = |sim: &anno_sim::simulation::Simulation, label: String| {
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
        println!(
            "{label}: pop={:?} sat={:?} food={} wool={} cloth={} wood={} resv={:?} gold={} unlock={:#x}",
            city.tier_population,
            city.satisfaction_by_group,
            wh.stock(Good::Food),
            wh.stock(Good::Wool),
            wh.stock(Good::Cloth),
            wh.stock(Good::Wood),
            city.promotion_reservations,
            sim.players[0].gold,
            sim.players[0].unlock_mask,
        );
    };
    for minute in 1..=10u32 {
        for _ in 0..600 {
            sim.tick(100);
            sim.drain_source_kind13_replacements(&cod);
        }
        report(&sim, format!("min {minute}"));
        if minute == 1 || minute == 10 {
            site_report(&sim);
            cell_report(&sim);
        }
    }

    // --- Stage 3: run the real chains ---
    // Nothing is injected any more. Stage 2 proved the promotion gate is
    // supply-driven by feeding the warehouse directly; this stage makes the
    // colony earn its own cloth through sheep farm -> weaving hut, which is
    // enough on its own because group-1 satisfaction saturates on cloth.
    println!("--- stage 3: colony produces its own cloth ---");
    for minute in 11..=60u32 {
        for _ in 0..600 {
            sim.tick(100);
            sim.drain_source_kind13_replacements(&cod);
        }
        report(&sim, format!("min {minute}"));
    }
    println!(
        "objectives={:?}",
        sim.objectives
            .items
            .iter()
            .map(|(o, done)| (o.label(), *done))
            .collect::<Vec<_>>()
    );
}
