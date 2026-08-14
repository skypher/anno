//! Scratch probe: per-city ware-economy state on Exile (not committed).
use anno_formats::szs::SzsFile;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "extracted/Szenes/Exile.szs".into());
    let ticks: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(400);
    let data = std::fs::read(&path).expect("read scenario");
    let szs = SzsFile::parse(&data).expect("parse scenario");
    let cod_data = std::fs::read("extracted/haeuser.cod").expect("read haeuser.cod");
    let cod = anno_formats::cod::CodFile::parse(&cod_data).expect("parse haeuser.cod");
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read("extracted/figuren.cod")
        .map(|b| anno_formats::figuren::FiguresFile::parse(&b))
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let mut sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    sim.seed_source_rand(1);

    let dump = |sim: &anno_sim::simulation::Simulation, label: &str| {
        let k13: Vec<_> = sim
            .source_kind13_locations
            .active_locations()
            .into_iter()
            .filter(|l| l.island_id == 0)
            .collect();
        let total_amount: u32 = k13.iter().map(|l| u32::from(l.amount)).sum();
        println!(
            "  kind13 island0: {} locations, total amount {} (~{} residents)",
            k13.len(),
            total_amount,
            total_amount / 64,
        );
        println!("== {label} clock={}", sim.game_clock);
        for city in sim.source_cities.active_records() {
            if city.owner_slot != 0 {
                continue;
            }
            println!(
                "  city island={} owner={} pop={:?} sat={:?} food_f={} lux={:?}",
                city.island_id,
                city.owner_slot,
                city.tier_population,
                city.satisfaction_by_group,
                city.food_fulfillment,
                city.luxury_satisfaction,
            );
            println!(
                "    demand={:?} supply={:?}",
                city.ware_demand, city.ware_supply
            );
            for w in &sim.warehouses {
                if w.active && w.island_id == city.island_id && w.owner == 0 {
                    use anno_sim::types::Good;
                    println!(
                        "    wh island={} food={} cloth={} alcohol={} (fixed food={})",
                        w.island_id,
                        w.stock(Good::Food),
                        w.stock(Good::Cloth),
                        w.stock(Good::Alcohol),
                        w.city_stock_fixed(Good::Food),
                    );
                }
            }
        }
        let p = &sim.players[0];
        println!(
            "  player0 gold={} pop={:?} sat={:?}",
            p.gold, p.population, p.satisfaction
        );
    };

    dump(&sim, "t0");
    for t in 1..=ticks {
        sim.tick(1000);
        sim.drain_source_kind13_replacements(&cod);
        if t % 100 == 0 {
            dump(&sim, &format!("t{t}"));
        }
    }
}
