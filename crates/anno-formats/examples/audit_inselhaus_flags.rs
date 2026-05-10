//! Survey the 16-bit `flags` field on each INSELHAUS tile
//! across the shipping corpus to identify per-tile metadata
//! beyond building_id / x / y / orientation / anim_count.

use anno_formats::szs::SzsFile;

fn main() {
    let dir = "/home/sky/anno/extracted/Szenes";
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path()
            .extension().and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("szs"))
            .unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut anim_count_dist: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut flag_dist: std::collections::BTreeMap<u16, u32> = Default::default();
    let mut bit_freq = [0u32; 16];
    let mut total = 0;
    for entry in &entries {
        let path = entry.path();
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        for island in &parsed.islands {
            for tile in &island.tiles {
                total += 1;
                *anim_count_dist.entry(tile.anim_count).or_default() += 1;
                *flag_dist.entry(tile.flags).or_default() += 1;
                for bit in 0..16 {
                    if tile.flags & (1 << bit) != 0 {
                        bit_freq[bit] += 1;
                    }
                }
            }
        }
    }

    println!("INSELHAUS tile.anim_count distribution:");
    let mut total_anim = 0u32;
    for (v, c) in &anim_count_dist {
        let pct = *c * 100 / total.max(1);
        if pct > 0 {
            println!("  {v}: {pct}% ({c})");
        }
        total_anim += c;
    }
    println!("  ({} tiles)\n", total_anim);

    println!("INSELHAUS tile.flags: {} tiles total", total);
    println!("Distinct values: {}", flag_dist.len());
    println!("Most common 15 values:");
    let mut sorted: Vec<_> = flag_dist.iter().collect();
    sorted.sort_by_key(|&(_, c)| std::cmp::Reverse(*c));
    for (v, c) in sorted.iter().take(15) {
        println!("  0x{v:04X} ({}): {} tiles", v, c);
    }
    println!();
    println!("Per-bit frequency (% of tiles with bit set):");
    for bit in 0..16 {
        let pct = bit_freq[bit] * 100 / total.max(1);
        if bit_freq[bit] > 0 {
            println!("  bit {bit:2} (0x{:04X}): {pct:3}% ({} tiles)",
                1u32 << bit, bit_freq[bit]);
        }
    }
}
