//! Census MARKT2 scenario chunks before decoding their per-market state.

use anno_formats::cod::CodFile;
use anno_formats::szs::SzsFile;
use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    let scenes = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("extracted/Szenes");
    let entries = std::fs::read_dir(&scenes).expect("read scenario directory");
    let cod_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("extracted/haeuser.cod");
    let cod_bytes = std::fs::read(cod_path).expect("read haeuser.cod");
    let cod = CodFile::parse(&cod_bytes).expect("parse haeuser.cod");
    let mut lengths = BTreeMap::<usize, usize>::new();
    let mut predecessor_names = BTreeMap::<String, usize>::new();
    let mut record_count_by_market_roots = BTreeMap::<(usize, usize), usize>::new();
    let mut record_zero_bytes = [0usize; 20];
    let mut record_count = 0usize;
    let mut nonempty_samples = Vec::<(String, usize, [u8; 20])>::new();
    let mut chunk_count = 0usize;

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "szs") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read scenario");
        let szs = SzsFile::parse(&bytes).expect("parse scenario");
        let mut island_index = None;
        for (index, chunk) in szs.chunks.iter().enumerate() {
            if chunk.name == "INSEL5" {
                island_index = Some(island_index.map_or(0, |prior| prior + 1));
            }
            if chunk.name != "MARKT2" {
                continue;
            }
            chunk_count += 1;
            *lengths.entry(chunk.data.len()).or_default() += 1;
            let predecessor = index
                .checked_sub(1)
                .and_then(|prior| szs.chunks.get(prior))
                .map(|prior| prior.name.clone())
                .unwrap_or_else(|| "<start>".to_owned());
            *predecessor_names.entry(predecessor).or_default() += 1;
            let market_roots = island_index
                .and_then(|index| szs.islands.get(index))
                .map(|island| {
                    island
                        .tiles
                        .iter()
                        .filter(|tile| {
                            cod.building_by_source_id(tile.source_id())
                                .is_some_and(|definition| definition.source_kind_code() == Some(7))
                        })
                        .count()
                })
                .unwrap_or(0);
            *record_count_by_market_roots
                .entry((chunk.data.len() / 20, market_roots))
                .or_default() += 1;
            for (record_index, record) in chunk.data.chunks_exact(20).enumerate() {
                record_count += 1;
                for (offset, byte) in record.iter().enumerate() {
                    if *byte == 0 {
                        record_zero_bytes[offset] += 1;
                    }
                }
                let bytes: [u8; 20] = record.try_into().expect("MARKT2 record width");
                if bytes.iter().any(|byte| *byte != 0) && nonempty_samples.len() < 16 {
                    let scene = path
                        .file_name()
                        .expect("scenario file name")
                        .to_string_lossy()
                        .into_owned();
                    nonempty_samples.push((scene, record_index, bytes));
                }
            }
        }
    }

    println!("MARKT2 chunks: {chunk_count}");
    println!("length distribution:");
    for (length, count) in lengths {
        println!("  {length} bytes: {count}");
    }
    println!("immediate predecessor chunks:");
    for (name, count) in predecessor_names {
        println!("  {name}: {count}");
    }
    println!("20-byte record count by authored kind-7 root count:");
    for ((records, market_roots), count) in record_count_by_market_roots {
        println!("  records={records}, kind7_roots={market_roots}: {count}");
    }
    println!("zero count by byte offset across {record_count} records:");
    for (offset, zero_count) in record_zero_bytes.into_iter().enumerate() {
        println!("  {offset:02x}: {zero_count}");
    }
    println!("nonempty record samples:");
    for (scene, record_index, record) in nonempty_samples {
        let bytes = record
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!("  {scene} record {record_index}: {bytes}");
    }
}
