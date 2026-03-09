//! Simulation test — loads a scenario, runs the full simulation loop, and prints output.
//!
//! Demonstrates: production → carrier dispatch → warehouse delivery pipeline.
//!
//! Usage: cargo run --bin sim-test [scenario.szs]

use anno_formats::cod::CodFile;
use anno_formats::szs::SzsFile;
use anno_sim::ai::{AiController, AiPersonality, Difficulty};
use anno_sim::combat::{DiplomacyMatrix, Diplomacy, MilitaryUnit, UnitType};
use anno_sim::data_bridge;
use anno_sim::island_map::IslandMap;
use anno_sim::player::Player;
use anno_sim::simulation::Simulation;
use anno_sim::trade::{TradeRoute, TradeShip, RouteStop};
use anno_sim::types::Good;
use anno_sim::warehouse::Warehouse;

fn main() {
    let base_dir = find_data_dir();

    // Load building definitions from COD
    let cod_data =
        std::fs::read(base_dir.join("haeuser.cod")).expect("Failed to read haeuser.cod");
    let cod = CodFile::parse(&cod_data).expect("Failed to parse COD");
    let defs = data_bridge::load_building_defs(&cod);
    println!("Loaded {} building definitions", defs.len());

    // Load scenario
    let scenario_path = std::env::args().nth(1).unwrap_or_else(|| {
        let szenes = base_dir.join("Szenes");
        let mut entries: Vec<_> = std::fs::read_dir(&szenes)
            .expect("Failed to read Szenes/")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".szs"))
            .collect();
        entries.sort_by_key(|e| e.file_name());
        entries
            .first()
            .map(|e| e.path().to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                eprintln!("No .szs files found");
                std::process::exit(1);
            })
    });

    let szs_data = std::fs::read(&scenario_path).expect("Failed to read scenario");
    let szs = SzsFile::parse(&szs_data).expect("Failed to parse scenario");
    let scenario_name = std::path::Path::new(&scenario_path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let mut instances = data_bridge::load_building_instances(&szs, &cod, &defs);
    println!(
        "Scenario '{}': {} production buildings",
        scenario_name,
        instances.len()
    );

    // Seed processing buildings with input materials
    for inst in &mut instances {
        let def = &defs[inst.def_id as usize];
        if def.input_good_1 != Good::None {
            inst.input_1_stock = def.storage_capacity;
        }
        if def.input_good_2 != Good::None {
            inst.input_2_stock = def.storage_capacity;
        }
    }

    // Create warehouses — one per island that has production buildings
    let mut island_ids: Vec<u8> = instances.iter().map(|i| i.island_id).collect();
    island_ids.sort();
    island_ids.dedup();

    let mut warehouses = Vec::new();
    for &island_id in &island_ids {
        // Place warehouse near the center of the island's buildings
        let island_buildings: Vec<_> = instances
            .iter()
            .filter(|b| b.island_id == island_id)
            .collect();
        if island_buildings.is_empty() {
            continue;
        }
        let avg_x = island_buildings.iter().map(|b| b.tile_x as u32).sum::<u32>()
            / island_buildings.len() as u32;
        let avg_y = island_buildings.iter().map(|b| b.tile_y as u32).sum::<u32>()
            / island_buildings.len() as u32;

        let wh = Warehouse::new(island_id, 0, avg_x as u16, avg_y as u16);
        warehouses.push(wh);
    }
    println!(
        "Created {} warehouses (one per island with buildings)",
        warehouses.len()
    );

    // Summarize buildings by output good
    let mut inst_by_good: std::collections::HashMap<Good, usize> =
        std::collections::HashMap::new();
    for inst in &instances {
        let def = &defs[inst.def_id as usize];
        if def.output_good != Good::None {
            *inst_by_good.entry(def.output_good).or_default() += 1;
        }
    }
    println!("\nProduction buildings by output:");
    let mut sorted: Vec<_> = inst_by_good.iter().collect();
    sorted.sort_by_key(|&(_, &c)| std::cmp::Reverse(c));
    for (good, count) in &sorted {
        println!("  {:?}: {}", good, count);
    }

    // Build island walkability maps for A* pathfinding
    let island_maps: Vec<IslandMap> = szs
        .islands
        .iter()
        .map(|island| IslandMap::from_island(island, &cod.buildings))
        .collect();
    println!(
        "Built {} island walkability maps for A* pathfinding",
        island_maps.len()
    );

    // Build coverage maps for each island
    let coverage_maps: Vec<anno_sim::coverage::CoverageMap> = szs
        .islands
        .iter()
        .map(|island| {
            anno_sim::coverage::CoverageMap::new(island.number, island.width as u16, island.height as u16)
        })
        .collect();

    // Build ocean map for ship pathfinding
    let ocean_map = anno_sim::ocean_map::OceanMap::from_scenario(&szs);

    // Set up simulation with a player
    let mut sim = Simulation::new();
    sim.building_defs = defs;
    sim.buildings = instances;
    sim.warehouses = warehouses;
    sim.island_maps = island_maps;
    sim.coverage_maps = coverage_maps;
    sim.ocean_map = Some(ocean_map);

    // Create a human player with some population
    let mut player = Player::new_human(0);
    player.population[0] = 200; // 200 Pioneers
    player.population[1] = 100; // 100 Settlers
    player.population[2] = 50;  // 50 Citizens
    player.gold = 10000;
    sim.players.push(player);

    // Create an AI player (economic personality, medium difficulty)
    let mut ai_player = Player::new_ai(1, 0);
    ai_player.population[0] = 150; // 150 Pioneers
    ai_player.population[1] = 50;  // 50 Settlers
    ai_player.gold = 8000;
    sim.players.push(ai_player);
    sim.ai_controllers.push(AiController::new(1, AiPersonality::Economic, Difficulty::Medium));

    println!(
        "Human: {} Pioneers, {} Settlers, {} Citizens, {} gold",
        sim.players[0].population[0],
        sim.players[0].population[1],
        sim.players[0].population[2],
        sim.players[0].gold
    );
    println!(
        "AI:    {} Pioneers, {} Settlers, {} gold (Economic/Medium)",
        sim.players[1].population[0],
        sim.players[1].population[1],
        sim.players[1].gold
    );

    // Set up military units for a combat demonstration
    sim.diplomacy.set(0, 1, Diplomacy::War);
    // Human player: 3 swordsmen + 1 cannon at island center
    sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 0, 20, 20));
    sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 0, 21, 20));
    sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 0, 20, 21));
    sim.military_units.push(MilitaryUnit::new(UnitType::Cannon, 0, 18, 20));
    // AI player: 4 pikemen + 1 musketeer approaching
    sim.military_units.push(MilitaryUnit::new(UnitType::Pikeman, 1, 25, 20));
    sim.military_units.push(MilitaryUnit::new(UnitType::Pikeman, 1, 25, 21));
    sim.military_units.push(MilitaryUnit::new(UnitType::Pikeman, 1, 26, 20));
    sim.military_units.push(MilitaryUnit::new(UnitType::Pikeman, 1, 26, 21));
    sim.military_units.push(MilitaryUnit::new(UnitType::Musketeer, 1, 27, 20));
    println!(
        "Combat: {} human units vs {} AI units (at war)",
        sim.military_units.iter().filter(|u| u.owner == 0).count(),
        sim.military_units.iter().filter(|u| u.owner == 1).count(),
    );

    // Set up a trade route between islands with warehouses
    // Find two islands that have warehouses with goods
    let wh_islands: Vec<(u8, u16, u16)> = sim.warehouses
        .iter()
        .map(|w| (w.island_id, w.tile_x, w.tile_y))
        .collect();
    if wh_islands.len() >= 2 {
        let mut route = TradeRoute::new(0, 0);
        route.add_stop(RouteStop {
            island_id: wh_islands[0].0,
            warehouse_x: wh_islands[0].1,
            warehouse_y: wh_islands[0].2,
            load_goods: vec![(Good::Spices, 10)],
            unload_goods: vec![Good::Grain],
        });
        route.add_stop(RouteStop {
            island_id: wh_islands[1].0,
            warehouse_x: wh_islands[1].1,
            warehouse_y: wh_islands[1].2,
            load_goods: vec![(Good::Grain, 10)],
            unload_goods: vec![Good::Spices],
        });
        route.activate();
        println!(
            "Trade route: Island {} ({},{}) <-> Island {} ({},{}) [Spices/Grain]",
            wh_islands[0].0, wh_islands[0].1, wh_islands[0].2,
            wh_islands[1].0, wh_islands[1].1, wh_islands[1].2,
        );

        let ship = TradeShip::new(0, 0, wh_islands[0].1 as i32, wh_islands[0].2 as i32);
        sim.trade_routes.push(route);
        sim.trade_ships.push(ship);
    }

    // Run simulation: 10 game minutes in 100ms real-time steps
    println!("\n--- Running 10 game minutes ---\n");
    let steps_per_minute = 600; // 100ms × 600 = 60s
    let total_steps = steps_per_minute * 10;
    let step_ms = 100;

    for step in 0..total_steps {
        sim.tick(step_ms);

        // Print status every game minute
        if (step + 1) % steps_per_minute == 0 {
            let (minutes, _seconds) = sim.display_time();
            let active_carriers = sim
                .figures
                .iter()
                .filter(|f| f.is_active())
                .count();

            let human_units = sim.military_units.iter().filter(|u| u.owner == 0 && u.is_alive()).count();
            let ai_units = sim.military_units.iter().filter(|u| u.owner == 1 && u.is_alive()).count();
            let ships_trading = sim.trade_ships.iter().filter(|s| s.active).count();
            let total_cargo: u16 = sim.trade_ships.iter().map(|s| s.cargo_total).sum();
            print!("  t={}min: carriers={} mil={}v{} ships={} cargo={}", minutes, active_carriers, human_units, ai_units, ships_trading, total_cargo);

            // Show warehouse totals
            let mut wh_totals: std::collections::HashMap<Good, u16> =
                std::collections::HashMap::new();
            for wh in &sim.warehouses {
                for (good, stock, _cap) in wh.all_stock() {
                    *wh_totals.entry(good).or_default() += stock;
                }
            }
            if !wh_totals.is_empty() {
                print!(" warehouse:");
                let mut items: Vec<_> = wh_totals.iter().collect();
                items.sort_by_key(|&(_, &s)| std::cmp::Reverse(s));
                for (good, stock) in items.iter().take(6) {
                    print!(" {:?}={}", good, stock);
                }
            }

            // Show player satisfaction and gold
            for (pi, p) in sim.players.iter().enumerate() {
                let label = if pi == 0 { "H" } else { "AI" };
                print!(
                    " | {}:sat=[{},{},{},{},{}] g={}",
                    label,
                    p.satisfaction[0],
                    p.satisfaction[1],
                    p.satisfaction[2],
                    p.satisfaction[3],
                    p.satisfaction[4],
                    p.gold
                );
            }
            println!();
        }
    }

    // Final summary
    println!("\n--- Final warehouse inventory ---\n");
    for wh in &sim.warehouses {
        let stock = wh.all_stock();
        if stock.is_empty() {
            println!(
                "  Island {} warehouse at ({},{}): empty",
                wh.island_id, wh.tile_x, wh.tile_y
            );
        } else {
            println!(
                "  Island {} warehouse at ({},{}):",
                wh.island_id, wh.tile_x, wh.tile_y
            );
            for (good, stock, cap) in &stock {
                println!("    {:?}: {}/{}", good, stock, cap);
            }
        }
    }

    // Show some building states
    println!("\n--- Sample building states ---\n");
    let mut shown = std::collections::HashSet::new();
    for b in &sim.buildings {
        let def = &sim.building_defs[b.def_id as usize];
        if def.output_good != Good::None && shown.insert(def.output_good) {
            println!(
                "  {:?}: eff={}/128 in1={} in2={} out={}/{} cycles={}",
                def.output_good,
                b.efficiency,
                b.input_1_stock,
                b.input_2_stock,
                b.output_stock,
                def.storage_capacity,
                b.total_work,
            );
            if shown.len() >= 8 {
                break;
            }
        }
    }

    println!(
        "\nActive figures: {}",
        sim.figures.iter().filter(|f| f.is_active()).count()
    );

    // Trade results
    if !sim.trade_ships.is_empty() {
        println!("\n--- Trade results ---\n");
        for (i, ship) in sim.trade_ships.iter().enumerate() {
            println!(
                "  Ship {}: {:?} at ({},{}) cargo={}/{} profit={}g",
                i, ship.state, ship.world_x, ship.world_y,
                ship.cargo_total, anno_sim::trade::SHIP_CARGO_CAPACITY,
                ship.profit
            );
            for (good, amount) in &ship.cargo {
                if *amount > 0 {
                    println!("    {:?}: {}", good, amount);
                }
            }
        }
    }

    // Combat results
    if !sim.military_units.is_empty() || sim.diplomacy.get(0, 1) == Diplomacy::War {
        println!("\n--- Combat results ---\n");
        let human_alive: Vec<_> = sim.military_units.iter().filter(|u| u.owner == 0 && u.is_alive()).collect();
        let ai_alive: Vec<_> = sim.military_units.iter().filter(|u| u.owner == 1 && u.is_alive()).collect();
        println!("  Human survivors: {}", human_alive.len());
        for u in &human_alive {
            println!("    {:?}: hp={:.2}/{:.2} at ({},{})", u.unit_type, u.health, u.unit_type.stats().max_health, u.tile_x, u.tile_y);
        }
        println!("  AI survivors: {}", ai_alive.len());
        for u in &ai_alive {
            println!("    {:?}: hp={:.2}/{:.2} at ({},{})", u.unit_type, u.health, u.unit_type.stats().max_health, u.tile_x, u.tile_y);
        }
    }

    // Player economy summary
    for (pi, p) in sim.players.iter().enumerate() {
        let label = if pi == 0 { "Human" } else { "AI" };
        println!("\n--- {} player economy ---\n", label);
        println!("  Gold: {}", p.gold);
        println!("  Income: {}/tick", p.calculate_income());
        println!("  Costs: {}/tick", p.calculate_costs());
        println!("  Net: {}/tick", p.net_balance());
        for tier in 0..5 {
            let name = ["Pioneer", "Settler", "Citizen", "Merchant", "Aristocrat"][tier];
            if p.population[tier] > 0 {
                println!(
                    "  {}: pop={} satisfaction={}/128 tax={}/128",
                    name, p.population[tier], p.satisfaction[tier], p.tax_rates[tier]
                );
            }
        }
        let goods = ["Food", "Cloth", "Alcohol", "TobaccoP", "Spices", "Cocoa", "Jewelry", "Clothing"];
        for (i, slot) in p.demands.iter().enumerate() {
            if slot.demand > 0 {
                println!(
                    "  Demand {}: {}/{} ({:.0}%)",
                    goods[i],
                    slot.supply,
                    slot.demand,
                    if slot.demand > 0 {
                        slot.supply as f64 / slot.demand as f64 * 100.0
                    } else {
                        100.0
                    }
                );
            }
        }
    }
}

fn find_data_dir() -> std::path::PathBuf {
    for candidate in &["extracted", "../extracted", "../../extracted"] {
        let p = std::path::Path::new(candidate);
        if p.join("haeuser.cod").exists() {
            return p.to_path_buf();
        }
    }
    eprintln!("Could not find game data directory.");
    std::process::exit(1);
}
