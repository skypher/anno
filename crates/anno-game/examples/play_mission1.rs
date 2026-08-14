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
    // The transfer waves rasterise the live map with `FUN_004704d0` /
    // `FUN_004706e0` and open only outer kinds `{1, 0xb, 0xc, 0xd, 0x12, 0x1d,
    // 0x1e}` (`1602_exe.c:79470-79477`) — road, ground, ruin, plaza, bridge,
    // beach-ruin, pier. Sea, the surf/beach ring and forest are impassable.
    // `IslandMap::is_walkable` counts `MEER` as walkable, so siting by that
    // helper alone put the whole colony out at sea, where every Karren search
    // returns `NoRoute` and nothing the colony makes can ever be collected.
    let open_ground = |k: u8| matches!(k, 1 | 11 | 12 | 13 | 18 | 29 | 30);
    let kind_at = |sim: &anno_sim::simulation::Simulation, x: i32, y: i32| {
        sim.island_maps[mi]
            .source_map_kind_and_owner(x, y)
            .map(|(k, _)| k)
            .unwrap_or(u8::MAX)
    };
    // How much ground can a Karren reach from a 2x3 Kontor rooted here? The
    // transfer wave floods the rasterised window out of the requesting root's
    // own footprint (`FUN_004710b0`, `1602_exe.c:80003`) and only over the
    // open outer kinds, taking a diagonal solely when both orthogonal cells
    // beside it are clear (`FUN_0046c7d0`). A producer outside that component
    // is stranded however much it makes, so found where the component is
    // biggest — which is what a player picking a harbour does by eye.
    let cart_component = |sim: &anno_sim::simulation::Simulation, anchor: (i32, i32)| {
        let kind = |x: i32, y: i32| {
            sim.island_maps[mi]
                .source_map_kind_and_owner(x, y)
                .map(|(k, _)| k)
                .unwrap_or(u8::MAX)
        };
        let mut seen = std::collections::HashSet::new();
        let mut queue: Vec<(i32, i32)> = (0..3)
            .flat_map(|dy| (0..2).map(move |dx| (anchor.0 + dx, anchor.1 + dy)))
            .collect();
        for cell in &queue {
            seen.insert(*cell);
        }
        while let Some((x, y)) = queue.pop() {
            for (dx, dy) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let next = (x + dx, y + dy);
                if next.0 < 0
                    || next.1 < 0
                    || next.0 >= island.width as i32
                    || next.1 >= island.height as i32
                    || seen.contains(&next)
                    || !open_ground(kind(next.0, next.1))
                {
                    continue;
                }
                if dx != 0
                    && dy != 0
                    && !(open_ground(kind(x + dx, y)) && open_ground(kind(x, y + dy)))
                {
                    continue;
                }
                seen.insert(next);
                queue.push(next);
            }
        }
        seen
    };
    // The Kontor is `Kind: HQ` (35), whose terrain arm admits only
    // `{STRASSE, WALD, BODEN, RUINE, PLATZ}` (`FUN_00464660`), and it reaches
    // the water through `Strandflg` — it stands on land and *fronts* a beach
    // rather than standing in the surf. Ask the real gate: "coastal plus one
    // open footprint cell" used to accept a footprint half in the sea.
    let kontor_def_index = cod
        .buildings
        .iter()
        .position(|b| b.source_id == 22103)
        .expect("Kontor definition");
    let anchor = (2..island.height as i32 - 5)
        .flat_map(|y| (2..island.width as i32 - 4).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            sim.island_maps[mi].is_coastal(x, y)
                && anno_game::game_commands::can_place_building(
                    &island,
                    &sim.island_maps[mi],
                    &defs[kontor_def_index],
                    x,
                    y,
                    2,
                    3,
                )
        })
        .max_by_key(|&anchor| cart_component(&sim, anchor).len())
        .expect("buildable coastal anchor for the Kontor footprint");
    // The ship has to stop on navigable water within `found_kontor`'s range.
    let dock = (-8i32..=8)
        .flat_map(|dy| (-8i32..=8).map(move |dx| (dx, dy)))
        .map(|(dx, dy)| (anchor.0 + dx, anchor.1 + dy))
        .filter(|&(x, y)| x >= 0 && y >= 0 && kind_at(&sim, x, y) == 19)
        .min_by_key(|&(x, y)| (x - anchor.0).abs() + (y - anchor.1).abs())
        .expect("open water beside the anchor");
    let world = (
        i32::from(island.x_pos) + dock.0,
        i32::from(island.y_pos) + dock.1,
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

    // Which ground can a Karren actually reach from the Kontor? The transfer
    // wave floods the rasterised window from the requesting root's own
    // footprint (`FUN_004710b0`, `1602_exe.c:80003`) across the open kinds
    // only, so a producer in a forest clearing is stranded no matter how much
    // it makes. Flood the same kinds once, from the Kontor footprint, and
    // refuse to build anywhere the cart cannot follow.
    let reachable = cart_component(&sim, anchor);
    println!("cart-reachable open ground from the Kontor: {}", reachable.len());
    // A supplier's own cell is opened by the raster as a *goal* whatever its
    // terrain (`FUN_004704d0` stamps the goal bit before the kind test), so the
    // cart only has to reach a cell adjacent to the footprint — which is what
    // lets a beach-only fishery be collected at all.
    let cart_can_reach = |x: i32, y: i32, w: u8, h: u8| {
        (-1..=i32::from(h))
            .flat_map(|dy| (-1..=i32::from(w)).map(move |dx| (dx, dy)))
            .any(|(dx, dy)| reachable.contains(&(x + dx, y + dy)))
    };

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
                (0..i32::from(h)).all(|dy| {
                    (0..i32::from(w)).all(|dx| {
                        map.is_walkable(x + dx, y + dy)
                            && open_ground(
                                map.source_map_kind_and_owner(x + dx, y + dy)
                                    .map(|(k, _)| k)
                                    .unwrap_or(u8::MAX),
                            )
                    })
                }) && cart_can_reach(x, y, w, h)
                    && !used.iter().any(|&(ux, uy, uw, uh)| {
                        x < ux + i32::from(uw)
                            && ux < x + i32::from(w)
                            && y < uy + i32::from(uh)
                            && uy < y + i32::from(h)
                    })
            })
        };
    // The Kontor already owns its 2x3 footprint.
    let mut used: Vec<(i32, i32, u8, u8)> = vec![(anchor.0, anchor.1, 2, 3)];
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
                // Ask the real terrain gate rather than a walkability guess:
                // the fishery is `Kind: STRANDHAUS` and is admitted only on
                // beach, which no walkability predicate describes.
                let free = anno_game::game_commands::can_place_building(
                    &island,
                    map,
                    def,
                    x,
                    y,
                    w,
                    h,
                ) && !used.iter().any(|&(ux, uy, uw, uh)| {
                        x < ux + i32::from(uw)
                            && ux < x + i32::from(w)
                            && y < uy + i32::from(uh)
                            && uy < y + i32::from(h)
                    });
                if !free || !cart_can_reach(x, y, w, h) {
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
                // Rank by resource count, then by nearness to the Kontor:
                // a producer the Karren wave cannot reach is stranded output.
                let reach = 64usize.saturating_sub(
                    (x - anchor.0).abs().max((y - anchor.1).abs()).clamp(0, 63) as usize,
                );
                let rank = n * 64 + reach;
                if n > 0 && best.is_none_or(|(bn, _)| rank > bn) {
                    best = Some((rank, (x, y)));
                }
            }
        }
        let Some((rank, (x, y))) = best else {
            println!("no {label} site with ware {ware} in range");
            return false;
        };
        let n = rank / 64;
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
    for minute in 11..=40u32 {
        for _ in 0..600 {
            sim.tick(100);
            sim.drain_source_kind13_replacements(&cod);
        }
        report(&sim, format!("min {minute}"));
    }

    // --- Stage 4: scale toward the goal ---
    // The colony is self-sustaining but capped by its house count: pioneer
    // huts hold 2 and settler houses 6, so 100 inhabitants needs far more
    // than the opening four. Expand the core once wood is flowing, and add a
    // second food and cloth line to carry the larger population.
    // Expansion is paced against the food stock rather than built out in one
    // burst. The first attempt placed twenty huts at once and the colony
    // starved at 46 inhabitants: a harvest cell is finite now, and a fishery's
    // sustained yield is its ripe cells divided by the 450 s a sea cell takes
    // to ripen — so houses have to wait for the food line, not the other way
    // round. A player paces it the same way.
    println!("--- stage 4: expanding the colony, paced on food ---");
    build_on_resource(&mut sim, &mut used, 402, 53, "forester1");
    build_on_resource(&mut sim, &mut used, 412, 52, "sheep2");
    build(&mut sim, &mut used, 388, "weaver1");
    let mut huts = 4;
    let mut fisheries = 1;
    for minute in 41..=120u32 {
        for _ in 0..600 {
            sim.tick(100);
            sim.drain_source_kind13_replacements(&cod);
        }
        let food = sim
            .warehouses
            .iter()
            .find(|w| w.island_id == 10 && w.owner == 0)
            .map(|w| w.stock(Good::Food))
            .unwrap_or(0);
        // Below a comfortable buffer, buy food capacity before housing.
        if food < 25 && fisheries < 6 {
            if build_on_resource(&mut sim, &mut used, 270, 57, &format!("fishery{fisheries}")) {
                fisheries += 1;
            }
        } else if food >= 25 && huts < 40 {
            if build(&mut sim, &mut used, 414, &format!("hut{huts}")) {
                huts += 1;
            }
        }
        report(&sim, format!("min {minute}"));
        if minute % 20 == 0 {
            cell_report(&sim);
        }
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
