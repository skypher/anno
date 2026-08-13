//! Dump PLAYER4 slot records for a scenario: state_byte, starting_gold,
//! ai_active. Usage: cargo run -p anno-formats --example dump_players -- <path.szs>
use anno_formats::szs::SzsFile;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_players <path.szs>");
    let data = std::fs::read(&path).expect("read scenario");
    let szs = SzsFile::parse(&data).expect("parse szs");
    println!("{} players in {}", szs.players.len(), path);
    for (i, p) in szs.players.iter().enumerate() {
        println!(
            "slot {i}: state_byte=0x{:02x} starting_gold={:>9} ai_active={} name={:?}",
            p.state_byte, p.starting_gold, p.ai_active, p.name
        );
    }
}
