//! The city hazard block, end to end on the campaign corpus. Self-skips
//! without the data corpus (`extracted/` is gitignored).
//!
//! `FUN_0047f8a0`'s event block (`1602_exe.c:91441-91519`) is the only place
//! a peaceful colony loses buildings. Its fire branch goes live at 80
//! pioneers — `mod = 200`, threshold 31, so 15.5 % per 60-67 s cycle — which
//! is squarely inside what a first-mission colony reaches. From there the
//! chain is: `FUN_0047b540` picks a tier-0/1 residence and rolls
//! `DAT_0049af08`; `FUN_00479ca0` registers the affliction and stops all
//! growth in that city through `city[0x1fe]`; `FUN_0047a020` steps it every
//! 10 s phase, spreading through the `FUN_004722f0` area scan; and after 25
//! phases the expiry posts a type-7 action whose `FUN_00463f40` consumer
//! turns the house into a ruin.
//!
//! This drives that whole chain against the real New Horizons0 island, the
//! real haeuser.cod residence definitions, and the real dispatcher order.

use anno_sim::data_bridge::{SourceCityRecord, SourceKind13Location};
use anno_sim::simulation::Simulation;
use anno_sim::source_cell::SourceMapCellState;

/// haeuser.cod `IDWOHN+4` — `Wohnhäuser I`, `BGruppe 0`, `Kind: GEBAEUDE`
/// with nested `HAUS_PRODTYP Kind: WOHNUNG` and a wood-only `HAUS_BAUKOST`.
const PIONEER_HOUSE_SOURCE_ID: i32 = 20605;
/// The island the campaign leaves unsettled, and the one mission 1 colonises.
const ISLAND: u8 = 10;

struct Corpus {
    sim: Simulation,
    cod: anno_formats::cod::CodFile,
}

fn load_corpus() -> Option<Corpus> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .to_path_buf();
    let (Ok(szs_data), Ok(cod_data)) = (
        std::fs::read(root.join("extracted/Szenes/New Horizons0.szs")),
        std::fs::read(root.join("extracted/haeuser.cod")),
    ) else {
        println!("Skipping test: data corpus not found");
        return None;
    };
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).expect("parse New Horizons0");
    anno_game::scenario::instantiate_stock_islands(&mut szs, &root.join("extracted"), 1);
    let cod = anno_formats::cod::CodFile::parse(&cod_data).expect("parse haeuser.cod");
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("extracted/figuren.cod"))
        .map(|bytes| anno_formats::figuren::FiguresFile::parse(&bytes))
        .expect("parse figuren.cod");
    let sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    Some(Corpus { sim, cod })
}

/// Seed a pioneer colony on island 10: one city record and a contiguous
/// block of `Wohnhäuser I` roots with their kind-13 records, exactly the
/// state `FUN_00478b90` leaves behind once the houses are up.
///
/// Returns the physical city slot and the anchors of the seeded residences.
fn seed_pioneer_colony(
    corpus: &mut Corpus,
    houses: usize,
    pioneers: u32,
) -> (usize, Vec<(u8, u8)>) {
    let definition = corpus
        .cod
        .building_by_source_id(PIONEER_HOUSE_SOURCE_ID)
        .expect("Wohnhäuser I definition")
        .clone();
    assert_eq!(definition.size, (2, 2));
    assert_eq!(definition.source_kind_code(), Some(0x0e), "Kind: GEBAEUDE");
    assert_eq!(
        definition.source_production_kind_code(),
        Some(0x0d),
        "HAUS_PRODTYP Kind: WOHNUNG",
    );
    assert_eq!(definition.source_population_group(), Some(0));

    // A run of plain ground (`Kind: BODEN`) wide enough for the block.
    let map_index = corpus
        .sim
        .island_maps
        .iter()
        .position(|map| map.island_id == ISLAND)
        .expect("island 10 is loaded");
    let (width, height) = {
        let map = &corpus.sim.island_maps[map_index];
        (i32::from(map.width), i32::from(map.height))
    };
    let is_ground = |sim: &Simulation, x: i32, y: i32| {
        sim.island_maps[map_index]
            .source_map_kind_and_owner(x, y)
            .is_some_and(|(kind, _)| kind == 11)
    };

    // A compact block of touching 2x2 residences, so the area scan has
    // somewhere for the fire to walk.
    let columns = (houses as f64).sqrt().ceil() as i32;
    let rows = (houses as i32 + columns - 1) / columns;
    let mut anchors = Vec::new();
    'search: for origin_y in 0..height {
        for origin_x in 0..width {
            if origin_x + columns * 2 >= width || origin_y + rows * 2 >= height {
                continue;
            }
            let clear = (0..rows * 2).all(|dy| {
                (0..columns * 2).all(|dx| is_ground(&corpus.sim, origin_x + dx, origin_y + dy))
            });
            if !clear {
                continue;
            }
            for index in 0..houses as i32 {
                anchors.push((
                    (origin_x + (index % columns) * 2) as u8,
                    (origin_y + (index / columns) * 2) as u8,
                ));
            }
            break 'search;
        }
    }
    assert_eq!(anchors.len(), houses, "island 10 has room for the colony");

    let city_slot = corpus
        .sim
        .source_cities
        .allocate_source_city(ISLAND, anchors[0].0, anchors[0].1, 0, 0)
        .expect("a free city slot");
    let source_owner = corpus
        .sim
        .source_cities
        .record(city_slot)
        .unwrap()
        .source_owner;

    for &(x, y) in &anchors {
        let mut root = SourceMapCellState::new_static(ISLAND, x, y, &definition, 0)
            .expect("static root for the residence");
        root.configure_terminal_replacement(&corpus.cod);
        root.source_map_owner_slot = source_owner;
        corpus.sim.replace_source_static_map_footprint(root);
        assert!(corpus
            .sim
            .source_kind13_locations
            .insert(SourceKind13Location {
                island_id: ISLAND,
                tile_x: x,
                tile_y: y,
                orientation: 0,
                variant: 0,
                source_owner,
                phase: 0,
                state_bits: 0,
                population_group: 0,
                amount: 0x40,
                lifecycle_flags: 0,
            }));
    }

    let record = corpus.sim.source_cities.record_mut(city_slot).unwrap();
    record.tier_population = [pioneers, 0, 0, 0, 0];
    (city_slot, anchors)
}

/// Below the 80-pioneer boundary, with fewer than 250 pioneers-plus-settlers
/// and fewer than 250 settlers-and-above, the whole fire section is
/// unreachable: the event cycle still opens on its 60.0-67.0 s re-arm and
/// still spends its two unconditional draws, but nothing ever ignites.
#[test]
fn a_colony_below_the_pioneer_boundary_never_catches_fire() {
    let Some(mut corpus) = load_corpus() else {
        return;
    };
    let (city_slot, _) = seed_pioneer_colony(&mut corpus, 9, 79);
    corpus.sim.seed_source_rand(11);

    // `tick(200)` is one whole `FUN_00489670` slice per call, the largest the
    // dispatcher takes, so this covers 160 s of scaled sim time — two event
    // cycles at the 60.0-67.0 s re-arm.
    let mut previous = corpus.sim.source_cities.record(city_slot).unwrap();
    let mut cycles = 0;
    for _ in 0..800 {
        corpus.sim.tick(200);
        let now = corpus.sim.source_cities.record(city_slot).unwrap();
        if now.ready_at_ticks != previous.ready_at_ticks {
            // `now + (60 + (rand() & 7)) * 10` in 100 ms ticks. The city is
            // only visited when the dispatcher's phase has moved on, so the
            // observed remainder can sit a little under the nominal band.
            let delta = now.ready_at_ticks - corpus.sim.source_time_ticks;
            assert!(
                (560..=670).contains(&delta),
                "re-arm {delta} deciseconds outside the 600..=670 band",
            );
            cycles += 1;
        }
        previous = now;
    }
    assert!(cycles >= 2, "the event must have opened repeatedly");
    assert!(
        corpus.sim.source_afflictions.active_entries().is_empty(),
        "no fire below the 80-pioneer boundary",
    );
    let city = corpus.sim.source_cities.record(city_slot).unwrap();
    assert_eq!(city.active_afflictions, 0);
    assert!(!city.growth_blocked());
}

/// Past 80 pioneers the fire branch is live: a fire ignites, `FUN_0047a020`
/// steps it, and its expiry turns the residence into a ruin while the city's
/// affliction count comes back down.
#[test]
fn a_pioneer_colony_catches_fire_and_burns_down_to_a_ruin() {
    let Some(mut corpus) = load_corpus() else {
        return;
    };
    // 160 pioneers is the `mod = 100`, threshold 31 band — 31 % per cycle.
    let (city_slot, anchors) = seed_pioneer_colony(&mut corpus, 16, 160);
    corpus.sim.seed_source_rand(3);

    // Ignition.
    // The first event cycle opens at the city record's `now + 600` arming,
    // 60 s in, and fires with probability 0.31 * (1 - (5/8)^3) = 23 %; this
    // budget covers roughly ten cycles.
    let mut ignition = None;
    for _ in 0..1_200 {
        corpus.sim.tick(200);
        if let Some(entry) = corpus.sim.source_afflictions.active_entries().first() {
            ignition = Some(*entry);
            break;
        }
    }
    let ignition = ignition.expect("a fire must ignite within four minutes of sim time");
    assert_eq!(
        ignition.kind,
        anno_sim::data_bridge::SOURCE_AFFLICTION_KIND_FIRE,
    );
    assert!(anchors.contains(&(ignition.tile_x, ignition.tile_y)));

    // `FUN_00479ca0` marks the residence and blocks all growth in the city.
    let burning = corpus
        .sim
        .source_kind13_locations
        .location_at(ISLAND, ignition.tile_x, ignition.tile_y)
        .expect("the burning residence still has its kind-13 record");
    assert_eq!(burning.lifecycle_flags & 3, 2);
    let city = corpus.sim.source_cities.record(city_slot).unwrap();
    assert!(city.active_afflictions >= 1);
    assert!(city.growth_blocked());

    let houses_before = corpus
        .sim
        .source_kind13_locations
        .active_locations()
        .iter()
        .filter(|location| location.island_id == ISLAND)
        .count();
    let clears_before = corpus.sim.tile_clears.len();

    // Spread and expiry. The ignited entry lives 25 phases (250 s of scaled
    // sim time) and its expiry posts a deferred type-7 action, so give the
    // colony room for the conversion to land as well.
    let mut peak_afflictions = city.active_afflictions;
    // 25 phases of 10 s, plus a slice for the deferred action to drain.
    let mut burnt_out = false;
    for _ in 0..1_800 {
        corpus.sim.tick(200);
        peak_afflictions = peak_afflictions.max(
            corpus
                .sim
                .source_cities
                .record(city_slot)
                .unwrap()
                .active_afflictions,
        );
        if corpus
            .sim
            .source_kind13_locations
            .location_at(ISLAND, ignition.tile_x, ignition.tile_y)
            .is_none()
        {
            burnt_out = true;
            break;
        }
    }
    assert!(burnt_out, "the burning residence must eventually be removed");

    // `FUN_00463f40` rewrote the footprint as a ruin.
    assert!(corpus.sim.tile_clears.len() > clears_before);
    let clear = corpus
        .sim
        .tile_clears
        .iter()
        .find(|clear| {
            clear.island_id == ISLAND
                && clear.tile_x == u16::from(ignition.tile_x)
                && clear.tile_y == u16::from(ignition.tile_y)
        })
        .expect("the burnt residence produced a ruin conversion");
    assert_ne!(
        clear.ruin_id,
        anno_sim::building::NO_RUIN_ID,
        "Wohnhäuser I is authored `Ruinenr: RUINE_HOLZ`",
    );
    assert!(
        !clear.source_ruin_draws.is_empty(),
        "the ruin conversion draws at least one `rand()`",
    );

    // The removal released the record, its residents, and the affliction.
    let houses_after = corpus
        .sim
        .source_kind13_locations
        .active_locations()
        .iter()
        .filter(|location| location.island_id == ISLAND)
        .count();
    assert!(houses_after < houses_before);
    assert!(corpus
        .sim
        .source_afflictions
        .entry_at(ISLAND, ignition.tile_x, ignition.tile_y)
        .is_none());
    let city = corpus.sim.source_cities.record(city_slot).unwrap();
    assert!(
        city.active_afflictions < peak_afflictions,
        "expiry must decrement the city's affliction count \
         (peak {peak_afflictions}, now {})",
        city.active_afflictions,
    );
    assert!(city.active_afflictions >= 0);
}
