//! Dump the compiled building definitions so the mission driver can pick
//! def indices by production chain instead of by hard-coded number.
//! Usage: `cargo run --release -p anno-game --example dump_defs -- [filter]`
fn main() {
    let root = std::path::Path::new("extracted");
    let cod_data = std::fs::read(root.join("haeuser.cod")).unwrap();
    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let filter = std::env::args().nth(1).unwrap_or_default().to_uppercase();
    // The compiled defs carry no name string; recover one by reversing the
    // constant table that `Id:` resolves through.
    let names: std::collections::HashMap<i32, &str> = cod
        .constants
        .iter()
        .map(|(name, value)| (*value, name.as_str()))
        .collect();
    for (index, def) in defs.iter().enumerate() {
        let name = cod
            .buildings
            .get(index)
            .and_then(|b| names.get(&b.source_id).copied())
            .unwrap_or("?");
        let line = format!(
            "{index:4} id={:5} {name:24} kind={:10} prod={:12} out={:?}x{} in1={:?}x{} in2={:?}x{} \
             size={}x{} infra={} gold={} wood={} tools={} bricks={} maint={} radius={}",
            def.id,
            def.kind,
            def.prod_kind,
            def.output_good,
            def.output_rate,
            def.input_good_1,
            def.input_1_rate,
            def.input_good_2,
            def.input_2_rate,
            def.width,
            def.height,
            def.bauinfra,
            def.cost_gold,
            def.cost_wood,
            def.cost_tools,
            def.cost_bricks,
            def.maintenance_cost,
            def.radius,
        );
        if filter.is_empty() || line.to_uppercase().contains(&filter) {
            println!("{line}");
        }
    }
}
