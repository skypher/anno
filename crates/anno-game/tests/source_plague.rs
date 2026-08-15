//! The plague half of the city hazard block, end to end on the campaign
//! corpus. Self-skips without the data corpus (`extracted/` is gitignored).
//!
//! `FUN_0047f8a0`'s plague roll (`1602_exe.c:91456-91467`) opens at 200
//! settlers-and-above and picks its band from citizens-and-above, so unlike
//! the fire it is unreachable on a pioneer colony — which is exactly why it
//! was Stage 2. From there the chain is: `FUN_0047b850` picks a tier-2+
//! residence that is at least half full and rolls `DAT_0049aed8` on its
//! doctor and bathhouse coverage; `FUN_00479ca0` marks the house and stops
//! **all** growth in that city through `city[0x1fe]`; `FUN_0047a020`'s
//! type-1 branch steps it every 10 s phase, spreading through the radius-4
//! `FUN_004724d0` scan; and after 20 phases the expiry simply heals the
//! house — no deferred action, no ruin, no `rand()`.
//!
//! This drives that whole chain against the real New Horizons0 island, the
//! real haeuser.cod `Id: 20631` citizen residence, and the real dispatcher
//! order.

use anno_sim::data_bridge::{
    SourceAfflictionEntry, SourceKind13Location, SOURCE_AFFLICTION_KIND_PLAGUE,
    SOURCE_AFFLICTION_SPREAD_DURATION, SOURCE_KIND13_AMOUNT_CAPACITIES,
    SOURCE_PLAGUE_SCAN_RADIUS,
};
use anno_sim::simulation::Simulation;
use anno_sim::source_cell::SourceMapCellState;

/// The island the campaign leaves unsettled, and the one mission 1 colonises.
const ISLAND: u8 = 10;
/// `BGruppe 2` — the first tier `FUN_0047b850` will look at, since its filter
/// is `1 < BGruppe` where the fire's is `BGruppe < 2`.
const CITIZEN_GROUP: u8 = 2;
/// `SOURCE_KIND13_AMOUNT_CAPACITIES[2]`, the citizen tier's `Maxwohn << 6`.
const CITIZEN_CAPACITY: u16 = SOURCE_KIND13_AMOUNT_CAPACITIES[CITIZEN_GROUP as usize];

/// Four independent settlements, each four residences wide.
///
/// The block runs **once per 10 s dispatcher phase per city**, and the roll
/// is 13/180 = 7.2 % — so one settlement waits about fifteen phases for its
/// first outbreak and four settlements wait about four. Each `tick(200)`
/// costs ~0.15 s on this corpus, so that difference is minutes. The four
/// also give the city-slot filter in `FUN_004724d0` something real to reject:
/// each colony's neighbours across the row divide belong to a *different*
/// settlement slot and are neither candidates nor traversable.
const COLONIES: usize = 4;
const HOUSES_PER_COLONY: usize = 4;

struct Colony {
    city_slot: usize,
    anchors: Vec<(u8, u8)>,
}

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

/// Seed `COLONIES` citizen settlements on island 10, one per row of a solid
/// block of the shipped BGruppe-2 residence, each with its own city record
/// and its own settlement slot. Every house starts at full occupancy, which
/// is the state `FUN_00478b90` plus a fed economy leaves behind.
fn seed_citizen_colonies(corpus: &mut Corpus) -> Vec<Colony> {
    let definition = corpus
        .cod
        .source_population_group_building(CITIZEN_GROUP)
        .expect("a shipped BGruppe-2 residence")
        .clone();
    assert_eq!(definition.source_kind_code(), Some(0x0e), "Kind: GEBAEUDE");
    assert_eq!(
        definition.source_production_kind_code(),
        Some(0x0d),
        "HAUS_PRODTYP Kind: WOHNUNG",
    );
    assert_eq!(
        definition.source_population_group(),
        Some(CITIZEN_GROUP),
        "the plague only ever picks BGruppe 2 and above",
    );
    let (footprint_w, footprint_h) = definition.size;

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

    let span_x = HOUSES_PER_COLONY as i32 * i32::from(footprint_w);
    let span_y = COLONIES as i32 * i32::from(footprint_h);
    let (block_x, block_y) = 'search: {
        for origin_y in 0..height {
            for origin_x in 0..width {
                if origin_x + span_x >= width || origin_y + span_y >= height {
                    continue;
                }
                let clear = (0..span_y).all(|dy| {
                    (0..span_x).all(|dx| is_ground(&corpus.sim, origin_x + dx, origin_y + dy))
                });
                if clear {
                    break 'search (origin_x, origin_y);
                }
            }
        }
        panic!("island 10 has room for the colonies");
    };

    let mut colonies = Vec::new();
    for row in 0..COLONIES as i32 {
        let anchors: Vec<(u8, u8)> = (0..HOUSES_PER_COLONY as i32)
            .map(|column| {
                (
                    (block_x + column * i32::from(footprint_w)) as u8,
                    (block_y + row * i32::from(footprint_h)) as u8,
                )
            })
            .collect();
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
                    population_group: CITIZEN_GROUP,
                    amount: CITIZEN_CAPACITY,
                    lifecycle_flags: 0,
                }));
        }
        colonies.push(Colony { city_slot, anchors });
    }

    // Distinct settlement slots is what makes the `FUN_004724d0` city-slot
    // filter observable at all.
    let slots: std::collections::BTreeSet<u8> = colonies
        .iter()
        .map(|colony| {
            corpus
                .sim
                .source_cities
                .record(colony.city_slot)
                .unwrap()
                .source_owner
        })
        .collect();
    assert_eq!(slots.len(), COLONIES, "each colony owns its own map slot");
    colonies
}

/// Hold the colonies where the hazard block is the only thing under test.
///
/// Three pins, all of them about *reaching* the block rather than about what
/// it does:
///
/// - `tier_population` is pinned at 400 citizens. The plague gate reads
///   settlers-and-above and its band reads citizens-and-above, and 400 puts
///   it in the `mod = 180` band. It also opens the fire chain's
///   `pop[1] >= 250` arm, but there is no BGruppe-0/1 residence anywhere
///   here, so `FUN_0047b540` finds no candidate and no fire can ever start.
/// - `amount` is pinned at full capacity. Both plague occupancy gates read
///   it, and a colony with no supply chain would otherwise be starved below
///   half capacity by `FUN_0047b410` long before a 7 % roll landed.
/// - `ready_at_ticks` is pinned at zero so the block opens on every visit —
///   once per 10 s dispatcher phase — instead of once per 60.0-67.0 s re-arm.
///   The re-arm band itself is pinned by `source_hazards.rs`.
fn hold_colonies(sim: &mut Simulation, colonies: &[Colony]) {
    for colony in colonies {
        if let Some(city) = sim.source_cities.record_mut(colony.city_slot) {
            city.tier_population = [0, 0, 400, 0, 0];
            city.ready_at_ticks = 0;
        }
        for &(x, y) in &colony.anchors {
            if let Some(location) = sim.source_kind13_locations.location_at_mut(ISLAND, x, y) {
                location.amount = CITIZEN_CAPACITY;
                location.population_group = CITIZEN_GROUP;
            }
        }
    }
}

/// Live afflictions on one colony's residences. The corpus carries other
/// settlements, so nothing here may look at the affliction table as a whole.
fn colony_afflictions(sim: &Simulation, colony: &Colony) -> Vec<SourceAfflictionEntry> {
    sim.source_afflictions
        .active_entries()
        .into_iter()
        .filter(|entry| {
            entry.island_id == ISLAND && colony.anchors.contains(&(entry.tile_x, entry.tile_y))
        })
        .collect()
}

/// The whole plague chain in one pass over the corpus: outbreak, the marked
/// residence and the growth stop, the radius-4 spread to a neighbour, and an
/// expiry that heals instead of demolishing.
#[test]
fn a_citizen_colony_contracts_a_plague_that_spreads_and_then_heals() {
    let Some(mut corpus) = load_corpus() else {
        return;
    };
    let colonies = seed_citizen_colonies(&mut corpus);
    corpus.sim.seed_source_rand(3);

    let houses_before = corpus.sim.source_kind13_locations.active_locations().len();
    let clears_before = corpus.sim.tile_clears.len();

    // --- outbreak. Four settlements at 7.2 % a phase: about four phases,
    // and 1_500 slices is thirty of them.
    let mut infected = None;
    for _ in 0..1_500 {
        hold_colonies(&mut corpus.sim, &colonies);
        // `tick(200)` is one whole `FUN_00489670` slice, the largest the
        // dispatcher takes.
        corpus.sim.tick(200);
        infected = colonies
            .iter()
            .find_map(|colony| Some((colony, *colony_afflictions(&corpus.sim, colony).first()?)));
        if infected.is_some() {
            break;
        }
    }
    let (colony, outbreak) = infected.expect("a plague must break out");

    assert_eq!(outbreak.kind, SOURCE_AFFLICTION_KIND_PLAGUE);
    // Both plague paths push a literal `0x14`, unlike the fire's `0x19` on
    // ignition.
    assert_eq!(outbreak.duration_phases, SOURCE_AFFLICTION_SPREAD_DURATION);

    let marked = corpus
        .sim
        .source_kind13_locations
        .location_at(ISLAND, outbreak.tile_x, outbreak.tile_y)
        .expect("the infected residence keeps its kind-13 record");
    assert_eq!(marked.lifecycle_flags & 3, 1, "lifecycle state 1 = plague");
    // `FUN_0047b850` only ever picks a house at or above half capacity.
    assert!(marked.amount >= CITIZEN_CAPACITY / 2);

    let city = corpus.sim.source_cities.record(colony.city_slot).unwrap();
    assert!(city.active_afflictions >= 1);
    assert!(
        city.growth_blocked(),
        "`FUN_0047b410` refuses to grow anything in a city with `city[0x1fe] != 0`",
    );

    // --- spread. `FUN_0047a020` type 1 gates on the *infected* house's
    // occupancy against `DAT_005a7758`, picks over the radius-4
    // `FUN_004724d0` results, and rolls `DAT_0049aed8` on the target.
    //
    // Two simultaneous afflictions in one city can only have come from the
    // spread: `FUN_0047b850` is unreachable unless `city[0x1fe] == 0`, so
    // the *second* one is never an independent outbreak. The older of the
    // pair is the origin and the younger arrived by spreading from it.
    //
    // Any of the four settlements will do. Pinning this to the one that
    // happened to break out first throws away three quarters of the
    // opportunities, because an outbreak lives only twenty phases and the
    // next one is as likely to land in any other colony.
    let mut pair = None;
    for _ in 0..2_000 {
        hold_colonies(&mut corpus.sim, &colonies);
        corpus.sim.tick(200);
        pair = colonies.iter().find_map(|colony| {
            let mut live = colony_afflictions(&corpus.sim, colony);
            if live.len() < 2 {
                return None;
            }
            live.sort_by_key(|entry| std::cmp::Reverse(entry.elapsed_phases));
            Some((colony, live[0], live[1]))
        });
        if pair.is_some() {
            break;
        }
    }
    let (colony, origin, spread) = pair.expect("the plague must reach a second residence");

    assert_eq!(spread.kind, SOURCE_AFFLICTION_KIND_PLAGUE);
    assert_eq!(spread.duration_phases, SOURCE_AFFLICTION_SPREAD_DURATION);
    assert!(colony.anchors.contains(&(spread.tile_x, spread.tile_y)));
    assert!(colony.anchors.contains(&(origin.tile_x, origin.tile_y)));
    assert_ne!((origin.tile_x, origin.tile_y), (spread.tile_x, spread.tile_y));

    // Radius 4 off the infected root: the scan's bounding box is
    // `centre +/- radius` per axis, with the centre offset by half the
    // oriented footprint, before the flood ever runs.
    let reach = SOURCE_PLAGUE_SCAN_RADIUS as i32 + 2;
    let dx = i32::from(spread.tile_x) - i32::from(origin.tile_x);
    let dy = i32::from(spread.tile_y) - i32::from(origin.tile_y);
    assert!(
        dx.abs() <= reach && dy.abs() <= reach,
        "spread from {:?} to {:?} is outside the radius-4 box",
        (origin.tile_x, origin.tile_y),
        (spread.tile_x, spread.tile_y),
    );

    let target = corpus
        .sim
        .source_kind13_locations
        .location_at(ISLAND, spread.tile_x, spread.tile_y)
        .expect("the newly infected residence keeps its record");
    assert_eq!(target.lifecycle_flags & 3, 1);
    assert!(
        target.amount >= CITIZEN_CAPACITY / 2,
        "the spread refuses a target under half capacity",
    );
    assert!(
        corpus
            .sim
            .source_cities
            .record(colony.city_slot)
            .unwrap()
            .active_afflictions
            >= 2,
    );

    // --- expiry. The older entry runs out its 20 phases — 200 s of scaled
    // sim time, 1_000 slices — and heals its house.
    let mut healed = false;
    for _ in 0..1_500 {
        hold_colonies(&mut corpus.sim, &colonies);
        corpus.sim.tick(200);
        let entry = corpus
            .sim
            .source_afflictions
            .entry_at(ISLAND, origin.tile_x, origin.tile_y);
        let record = corpus
            .sim
            .source_kind13_locations
            .location_at(ISLAND, origin.tile_x, origin.tile_y);
        if entry.is_none() && record.is_some_and(|record| record.lifecycle_flags & 3 == 0) {
            healed = true;
            break;
        }
    }
    assert!(
        healed,
        "`flags &= 0xfffc` must clear the residence's lifecycle bits at expiry",
    );
    assert!(
        corpus
            .sim
            .source_kind13_locations
            .location_at(ISLAND, origin.tile_x, origin.tile_y)
            .is_some(),
        "a plague never removes the house it infected",
    );

    // Across the whole run nothing was demolished and no ruin was posted:
    // a plague is not a fire.
    assert_eq!(
        corpus.sim.source_kind13_locations.active_locations().len(),
        houses_before,
    );
    assert!(
        corpus.sim.tile_clears[clears_before..]
            .iter()
            .all(|clear| clear.island_id != ISLAND),
        "a plague posts no type-7 action, so no ruin conversion ever runs",
    );
}
