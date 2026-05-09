//! Survey SZENE_MISSNR / SZENE_PLAYERMIN / SZENE_PLAYERMAX /
//! SZENE_RANKING values across the shipping corpus.

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

    let mut ranking_dist: std::collections::BTreeMap<u32, Vec<String>> = Default::default();
    let mut missnr_dist: std::collections::BTreeMap<u32, Vec<String>> = Default::default();

    println!("Per-scenario SZENE_* metadata:");
    for entry in &entries {
        let path = entry.path();
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = match std::fs::read(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match SzsFile::parse(&bytes) { Ok(p) => p, Err(_) => continue };
        let m = &parsed.scenario;
        println!("  {stem:<32} missnr={:?} pmin={:?} pmax={:?} ranking={:?}",
            m.mission_nr, m.player_min, m.player_max, m.ranking);
        if let Some(r) = m.ranking {
            ranking_dist.entry(r).or_default().push(stem.clone());
        }
        if let Some(n) = m.mission_nr {
            missnr_dist.entry(n).or_default().push(stem);
        }
    }
    println!();
    println!("RANKING distribution:");
    for (r, scens) in &ranking_dist {
        println!("  ranking={r}: {} scenarios — {}", scens.len(),
            scens.iter().take(4).cloned().collect::<Vec<_>>().join(", "));
    }
    println!();
    println!("MISSNR distribution (first 12):");
    for (n, scens) in missnr_dist.iter().take(12) {
        println!("  missnr={n}: {}", scens.iter().take(2).cloned().collect::<Vec<_>>().join(", "));
    }
}
