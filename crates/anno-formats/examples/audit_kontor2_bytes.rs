//! Hexdump KONTOR2 chunks from a scenario for layout auditing.
//!
//! Layout so far (loader at `0x484230`, in a decompiled-dump gap —
//! recovered by direct disassembly): 4-byte header (island, tile_x,
//! tile_y, pad), then 50 records of 0x14 bytes each (4 + 50*0x14 =
//! 1004 = chunk size). Per record: `+0x00` u32 bitfields merged into
//! the runtime ware entry's `+8` dword (10-bit trade-slider fields),
//! `+0x08` u32 split into the entry's `+2`/`+4` u16s, `+0x0c` u16
//! initial stock written to the entry's `+6` (city record
//! `+0x24 + ware*0x0c + 6`), `+0x10` u16 def number (+20000) whose
//! def byte `+0x21` selects the ware slot. Zero def numbers are
//! skipped. This is where authored initial city-store stocks live —
//! e.g. Exile's human city starts with NAHRUNG 800/32 t.
//!
//! Usage: cargo run --example audit_kontor2_bytes <scenario.szs>
fn main() {
    let path = std::env::args().nth(1).expect("usage: audit_kontor2_bytes <scenario.szs>");
    let data = std::fs::read(&path).unwrap();
    let szs = anno_formats::szs::SzsFile::parse(&data).unwrap();
    for (i, chunk) in szs
        .chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name == "KONTOR2")
    {
        let head = &chunk.data[..4.min(chunk.data.len())];
        println!(
            "== KONTOR2 #{i} ({} bytes) island={} tile=({}, {})",
            chunk.data.len(),
            head.first().copied().unwrap_or(0xff),
            head.get(1).copied().unwrap_or(0),
            head.get(2).copied().unwrap_or(0),
        );
        for (n, rec) in chunk.data[4..].chunks(0x14).enumerate() {
            if rec.len() < 0x14 {
                continue;
            }
            let u16at = |o: usize| u16::from_le_bytes([rec[o], rec[o + 1]]);
            let u32at = |o: usize| u32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
            let def = u16at(0x10);
            if def == 0 && u32at(0) == 0 && u32at(8) == 0 && u16at(0xc) == 0 {
                continue;
            }
            println!(
                "  rec {n:2}: flags={:08x} f4={:08x} f8={:08x} stock={} def={}",
                u32at(0),
                u32at(4),
                u32at(8),
                u16at(0xc),
                def,
            );
        }
    }
}
