//! Verify multi-tile buildings have the right INSELHAUS tile
//! count. Each W×H building should appear as W*H tiles in
//! INSELHAUS — base tile at (x,y) carries the building's gfx,
//! adjacent tiles carry gfx+1, gfx+2, etc.

use anno_formats::szs::SzsFile;
use anno_formats::cod::CodFile;
use std::collections::BTreeMap;

fn main() {
    let cod_bytes = std::fs::read("/home/sky/anno/extracted/haeuser.cod").unwrap();
    let cod = CodFile::parse(&cod_bytes).unwrap();

    // Build per-Nummer (size, gfx) lookup.
    let mut sizes: BTreeMap<i32, ((i32, i32), i32)> = BTreeMap::new();
    for b in &cod.buildings {
        sizes.insert(b.nummer, (b.size, b.gfx));
    }

    // Audit each scenario for footprint anomalies.
    let dir = "/home/sky/anno/extracted/Szenes";
    let mut total_tiles = 0;
    let mut multi_tile_anomalies = 0;
    let mut all_correct = 0;

    for entry in std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.extension().map(|s| s.eq_ignore_ascii_case("szs")).unwrap_or(false) { continue; }
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        for island in &parsed.islands {
            // Map (x, y) → tile gfx for fast lookup.
            let mut grid: std::collections::HashMap<(u8, u8), i32> = Default::default();
            for tile in &island.tiles {
                grid.insert((tile.x, tile.y), tile.building_id as i32);
                total_tiles += 1;
            }
            // Find each building base tile (where building_id ==
            // its def's gfx) and verify the W*H footprint.
            for tile in &island.tiles {
                let building_gfx = tile.building_id as i32;
                // Find def with matching gfx.
                let Some((nummer, &(size, _))) = sizes.iter()
                    .find(|(_, (_, g))| *g == building_gfx)
                    .map(|(n, info)| (*n, info))
                else { continue };
                let (w, h) = size;
                if w <= 1 && h <= 1 { continue; }
                // Verify W*H tile slots starting at (x, y) all
                // have *some* tile. The exact gfx-per-quadrant
                // depends on rotation + animation variants
                // (haeuser.cod's Rotate × AnimAdd × AnimAnz)
                // and isn't trivially `base + offset`, so we
                // just check tile presence.
                let mut footprint_ok = true;
                for dy in 0..h {
                    for dx in 0..w {
                        let cx = tile.x.wrapping_add(dx as u8);
                        let cy = tile.y.wrapping_add(dy as u8);
                        if !grid.contains_key(&(cx, cy)) {
                            footprint_ok = false;
                            break;
                        }
                    }
                    if !footprint_ok { break; }
                }
                if footprint_ok {
                    all_correct += 1;
                } else {
                    multi_tile_anomalies += 1;
                    if multi_tile_anomalies <= 5 {
                        println!("Anomaly: {:?} island {} Nr={} (W×H={}×{}) at ({}, {})",
                            path.file_stem().unwrap(), island.number, nummer, w, h,
                            tile.x, tile.y);
                    }
                }
            }
        }
    }

    // Probe specific anomalies — show actual gfx in footprint.
    println!();
    println!("Sample footprint dump for first anomaly (New Horizons2 island 0 Nr=409 at 41,7):");
    let path = std::path::Path::new("/home/sky/anno/extracted/Szenes/New Horizons2.szs");
    if let Ok(data) = std::fs::read(path) {
        if let Ok(parsed) = SzsFile::parse(&data) {
            if let Some(island) = parsed.islands.iter().find(|i| i.number == 0) {
                let mut grid: std::collections::HashMap<(u8, u8), i32> = Default::default();
                for t in &island.tiles {
                    grid.insert((t.x, t.y), t.building_id as i32);
                }
                for &(cx, cy) in &[(41, 7), (42, 7), (41, 8), (42, 8)] {
                    let g = grid.get(&(cx, cy)).copied().unwrap_or(-1);
                    println!("  ({cx},{cy}) gfx = {g}");
                }
            }
        }
    }

    println!();
    println!("Total tiles: {total_tiles}");
    println!("Multi-tile bases checked: {}",
        all_correct + multi_tile_anomalies);
    println!("All-correct footprints: {all_correct}");
    println!("Anomalies: {multi_tile_anomalies}");
}
