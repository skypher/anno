//! End-to-end coverage for the nested-production worker arms that
//! `FUN_0047daf0` reaches through its jump table at `0x47e258`.
//!
//! The table is indexed by a byte table at `0x47e27c` addressed with
//! `Kind - 1` (`mov eax, [ebx+0x1c]; dec eax; cmp eax, 0x1d; ja 0x47e234;
//! movzx edx, byte [eax + 0x47e27c]; jmp [edx*4 + 0x47e258]`, `0x0047def2`).
//! Decoded straight from `extracted/1602.exe`:
//!
//! | nested kind | index byte | arm | allocator |
//! | --- | --- | --- | --- |
//! | 2 `PLANTAGE` | `01` | `0x47df1f` | `FUN_0044b7e0` |
//! | 4 `WEIDETIER` | `03` | `0x47df5f` | `FUN_0044bb40` |
//! | 5 `JAGDHAUS` | `08` | `0x47e234` | none — the default arm |
//! | 6 `FISCHEREI` | `04` | `0x47df97` | `FUN_0044b9a0` |
//!
//! Self-skips without the data corpus.

use std::collections::HashSet;

struct Corpus {
    szs: anno_formats::szs::SzsFile,
    cod: anno_formats::cod::CodFile,
    defs: Vec<anno_sim::building::BuildingDef>,
    figures: anno_formats::figuren::FiguresFile,
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
    Some(Corpus {
        szs,
        cod,
        defs,
        figures,
    })
}

/// The island `New Horizons0` ships with 627 ripe `GRAS`, 585 `FISCHE` and
/// 559 `BAUM` cells — enough of each for one farm's compiled radius.
const ISLAND: u8 = 10;
/// Compiled `Ware: GRAS`, the `WEIDETIER` farms' `Rohstoff`.
const GRAS: u8 = 0x34;
/// Compiled `Ware: FISCHE`, the fishery's `Rohstoff`.
const FISCHE: u8 = 0x39;
/// Outer `Kind: MEER`, the only kind `FUN_0046fb50` accepts as a target.
const MEER_KIND: u8 = 19;

fn definition_index(cod: &anno_formats::cod::CodFile, id: &str) -> usize {
    cod.buildings
        .iter()
        .position(|b| b.properties.get("Id").is_some_and(|value| value == id))
        .unwrap_or_else(|| panic!("definition {id}"))
}

fn ready(sim: &mut anno_sim::simulation::Simulation, index: usize) {
    // Construction supply is a separate subsystem: the scenario leaves the
    // human without a warehouse on this island, so nothing can deliver the
    // site's materials. Hand them over rather than modelling a supply run.
    sim.buildings[index].wood_needed = 0;
    sim.buildings[index].tools_needed = 0;
    sim.buildings[index].bricks_needed = 0;
    sim.buildings[index].construction_ms_remaining = 0;
    assert!(sim.buildings[index].is_built());
    assert!(sim.buildings[index].active);
}

fn resource_cells(
    sim: &anno_sim::simulation::Simulation,
    ware: u8,
) -> HashSet<(i32, i32)> {
    sim.source_static_map_roots
        .iter()
        .filter(|cell| cell.island == ISLAND && cell.source_output_ware_slot == ware)
        .map(|cell| (i32::from(cell.x), i32::from(cell.y)))
        .collect()
}

/// Rank every candidate anchor by how many matching resource cells sit inside
/// the compiled search window.
fn ranked_anchors(cells: &HashSet<(i32, i32)>, radius: i32) -> Vec<(i32, i32)> {
    let mut anchors: Vec<((i32, i32), usize)> = cells
        .iter()
        .flat_map(|&(cx, cy)| {
            (-radius..=radius)
                .flat_map(move |dy| (-radius..=radius).map(move |dx| (cx + dx, cy + dy)))
        })
        .filter(|&(x, y)| x >= 1 && y >= 1)
        .collect::<HashSet<_>>()
        .into_iter()
        .map(|anchor| {
            let count = (-radius..=radius)
                .flat_map(|dy| (-radius..=radius).map(move |dx| (dx, dy)))
                .filter(|&(dx, dy)| cells.contains(&(anchor.0 + dx, anchor.1 + dy)))
                .count();
            (anchor, count)
        })
        .collect();
    anchors.sort_by_key(|&(anchor, count)| (std::cmp::Reverse(count), anchor));
    anchors.into_iter().map(|(anchor, _)| anchor).collect()
}

/// Production kind 5's index-table byte at `0x47e27c + (5 - 1)` is `0x08`,
/// which selects jump-table arm 8 — the loop's own `0x47e234` continuation,
/// not an allocator. The hunting lodge therefore never reaches
/// `FUN_00446ca0` and spawns no figure of any kind.
///
/// haeuser.cod agrees independently: `WILD` occurs exactly once in the whole
/// file, as this building's own `Rohstoff`, so no map definition is ever
/// authored `Ware: WILD` for a hunter to walk to. The lodge is limited to the
/// `Maxnorohst: 8` production cycles it may run with an empty raw buffer.
#[test]
fn the_hunting_lodge_dispatches_no_worker() {
    let Some(corpus) = load_corpus() else { return };
    let lodge = &corpus.cod.buildings[definition_index(&corpus.cod, "22003")];
    assert_eq!(lodge.source_production_kind_code(), Some(5));
    assert_eq!(lodge.source_raw_resource_ware_slot(), Some(0x38), "WILD");
    assert_eq!(
        anno_sim::simulation::SOURCE_HUNTING_LODGE_DISPATCH_INDEX_BYTE, 0x08,
        "0x47e27c[5 - 1] selects the default arm at 0x47e234"
    );
    assert!(
        corpus
            .cod
            .buildings
            .iter()
            .all(|b| b.source_ware_slot() != Some(0x38)),
        "no map definition is authored Ware: WILD, so a hunter would have no target"
    );

    let mut sim = anno_game::scenario::build_simulation(
        &corpus.szs,
        &corpus.cod,
        &corpus.defs,
        &corpus.figures,
    );
    sim.seed_source_rand(1);
    let island_index = corpus
        .szs
        .islands
        .iter()
        .position(|island| island.number == ISLAND)
        .expect("island 10");
    sim.players[0].gold = 100_000;
    sim.players[0].unlock_mask = u32::MAX;

    // Site it on plain ground, exactly as a producing kind-2 root would be.
    let lodge_index = definition_index(&corpus.cod, "22003");
    let placed = ranked_anchors(&resource_cells(&sim, GRAS), 3)
        .into_iter()
        .take(64)
        .find_map(|(x, y)| {
            let before = sim.buildings.len();
            anno_game::game_commands::place_building(
                &mut sim,
                &corpus.szs.islands,
                island_index,
                &corpus.defs,
                &corpus.cod,
                lodge_index,
                0,
                0,
                x,
                y,
            );
            (sim.buildings.len() > before).then_some((x, y))
        });
    let (x, y) = placed.expect("a 1x1 hunting lodge fits somewhere on the grassland");
    let building_index = sim.buildings.len() - 1;
    ready(&mut sim, building_index);

    let root = sim
        .source_map_cell_states
        .iter()
        .find(|state| state.matches(ISLAND, x as u16, y as u16))
        .copied()
        .expect("the lodge still owns a live source cell record");
    assert_eq!(root.source_production_kind_code, 5);
    assert_eq!(root.source_plantation_worker_definition, 0x65, "JAEGER");

    for _ in 0..2_000 {
        sim.tick(100);
        assert!(
            !sim.figures.iter().any(|figure| {
                figure.is_active()
                    && figure.source_worker_route
                        != anno_sim::entity::SourceWorkerRoute::None
                    && figure.origin_island == ISLAND
                    && figure.origin_x == x as u16
                    && figure.origin_y == y as u16
            }),
            "production kind 5 has no dispatch arm and must never allocate a worker"
        );
    }
}

/// `FUN_0044b9a0` / `FUN_0045b490` / `FUN_0045b730`: a fishery must turn
/// `FISCHE` map cells into `NAHRUNG` output.
///
/// `FUN_0046fb50` never reads a tile's settlement-slot selector, unlike the
/// land grid `FUN_0046f920` (`if ((param_6 == param_8) && ((*param_3 >> 0x13
/// & 7) == param_2))`, `1602_exe.c:78803`). This test therefore founds no
/// Kontor at all: the fishery has to work on unclaimed coast.
#[test]
fn a_fishery_harvests_unclaimed_water_without_a_kontor() {
    let Some(corpus) = load_corpus() else { return };
    let fishery_index = definition_index(&corpus.cod, "21075");
    let fishery = &corpus.cod.buildings[fishery_index];
    assert_eq!(fishery.source_production_kind_code(), Some(6));
    assert_eq!(fishery.source_raw_resource_ware_slot(), Some(FISCHE));
    assert_eq!(
        fishery.source_plantation_worker_definition(),
        Some(0x66),
        "Figurnr: FISCHER"
    );

    let mut sim = anno_game::scenario::build_simulation(
        &corpus.szs,
        &corpus.cod,
        &corpus.defs,
        &corpus.figures,
    );
    sim.seed_source_rand(1);
    let island_index = corpus
        .szs
        .islands
        .iter()
        .position(|island| island.number == ISLAND)
        .expect("island 10");
    sim.players[0].gold = 100_000;
    sim.players[0].unlock_mask = u32::MAX;

    let fish = resource_cells(&sim, FISCHE);
    assert!(fish.len() > 500, "island {ISLAND} ships {} fish", fish.len());
    let map_kind = |sim: &anno_sim::simulation::Simulation, x: i32, y: i32| {
        sim.island_maps
            .iter()
            .find(|map| map.island_id == ISLAND)
            .and_then(|map| map.source_map_kind_and_owner(x, y))
            .map(|(kind, _)| kind)
    };

    // `FUN_0045b730` opens exactly one water cell beside the pier — the one
    // its compiled orientation faces (`1602_exe.c:63200-63222`) — so the
    // orientation has to point at the sea, and the sea it points at has to
    // reach a `FISCHE` cell. Ask the ported `FUN_0046fb50` overlay directly
    // rather than guessing from a fish count in a square window: an anchor
    // can sit beside plenty of fish and still be walled off from all of them.
    let viable: Vec<(i32, i32, u8)> = {
        let map = sim
            .island_maps
            .iter()
            .find(|map| map.island_id == ISLAND)
            .expect("island 10 map");
        let statics = &sim.source_static_map_roots;
        ranked_anchors(&fish, 5)
            .into_iter()
            .filter(|&(x, y)| map.is_coastal(x, y))
            .flat_map(|(x, y)| (0..4u8).map(move |orientation| (x, y, orientation)))
            .filter(|&(x, y, orientation)| {
                let faced = match orientation & 3 {
                    0 => (x, y + 1),
                    1 => (x - 1, y),
                    2 => (x, y - 1),
                    _ => (x + 1, y),
                };
                map.source_map_kind_and_owner(faced.0, faced.1)
                    .map(|(kind, _)| kind)
                    == Some(MEER_KIND)
            })
            .filter_map(|(x, y, orientation)| {
                let mut grid = map.fishery_worker_path_grid(
                    (x, y),
                    (x, y),
                    (1, 1),
                    5,
                    orientation,
                    FISCHE,
                    statics,
                );
                // Count the cells `FUN_0046fb50` actually flagged inside the
                // clipped `Radius: 5` window, then confirm the wave search
                // reaches one of them.
                let targets = (-5..=5)
                    .flat_map(|dy| (-5..=5).map(move |dx| (dx, dy)))
                    .filter(|&(dx, dy)| {
                        grid.metadata((x + dx, y + dy)) == Some(0xa0)
                    })
                    .count();
                grid.search_source_high_metadata_target((x, y), 0)
                    .is_ok()
                    .then_some((x, y, orientation, targets))
            })
            .filter(|&(_, _, _, targets)| targets >= 12)
            .map(|(x, y, orientation, _)| (x, y, orientation))
            .take(32)
            .collect()
    };
    assert!(
        !viable.is_empty(),
        "no shore tile on island {ISLAND} reaches a FISCHE cell through the water grid"
    );
    let _ = map_kind;
    let mut placed = None;
    for (x, y, orientation) in viable {
        let before = sim.buildings.len();
        anno_game::game_commands::place_building(
            &mut sim,
            &corpus.szs.islands,
            island_index,
            &corpus.defs,
            &corpus.cod,
            fishery_index,
            orientation,
            0,
            x,
            y,
        );
        if sim.buildings.len() > before {
            placed = Some((x, y));
            break;
        }
    }
    let (x, y) = placed.expect("a fishery fits on a shore facing reachable fish");
    let building_index = sim.buildings.len() - 1;
    ready(&mut sim, building_index);

    assert!(
        !sim.warehouses.iter().any(|w| w.active && w.island_id == ISLAND),
        "the settlement rule must not be what makes this work"
    );
    let root_index = sim
        .source_map_cell_states
        .iter()
        .position(|state| state.matches(ISLAND, x as u16, y as u16))
        .expect("the placed fishery owns a live source cell record");

    let mut spawned_worker = false;
    let mut peak_raw = 0;
    let mut peak_output = 0;
    for _ in 0..8_000 {
        sim.tick(100);
        spawned_worker |= sim
            .figures
            .iter()
            .any(|figure| figure.is_active() && figure.origin_production_kind == 6);
        peak_raw = peak_raw.max(sim.source_map_cell_states[root_index].raw_material_stock);
        peak_output = peak_output.max(sim.buildings[building_index].output_stock);
        if peak_output >= 1 {
            break;
        }
    }

    println!("fishery at {x},{y}: raw {peak_raw}, food {peak_output}");
    assert!(spawned_worker, "FUN_0044b9a0 never allocated a FISCHER");
    assert!(
        peak_raw > 0,
        "no harvested fish reached the fishery's raw buffer +0x0a"
    );
    assert_eq!(peak_raw % 32, 0, "a harvest delivers whole 1/32 units");
    assert!(
        peak_output >= 1,
        "the fishery produced {peak_output} whole food (raw peak {peak_raw})"
    );
}

/// `FUN_0044bb40` / `FUN_0045ba60` / `FUN_0045bcc0`: a sheep farm sited on
/// ripe `GRAS` must turn it into `WOLLE`.
///
/// The sheep farm authors `Figuranz: 3`, and arm 3 of the dispatch table
/// carries no idle gate, so up to three `SCHAF` graze concurrently.
#[test]
fn a_sheep_farm_on_stock_grassland_produces_wool() {
    let Some(corpus) = load_corpus() else { return };
    let farm_index = definition_index(&corpus.cod, "21515");
    let farm = &corpus.cod.buildings[farm_index];
    assert_eq!(farm.source_production_kind_code(), Some(4));
    assert_eq!(farm.source_raw_resource_ware_slot(), Some(GRAS));
    assert_eq!(farm.source_ware_slot(), Some(0x04), "Ware: WOLLE");
    assert_eq!(
        farm.source_plantation_worker_definition(),
        Some(0x68),
        "Figurnr: SCHAF"
    );
    assert_eq!(farm.source_transfer_figure_limit, 3, "Figuranz: 3");

    let mut sim = anno_game::scenario::build_simulation(
        &corpus.szs,
        &corpus.cod,
        &corpus.defs,
        &corpus.figures,
    );
    sim.seed_source_rand(1);
    let island_index = corpus
        .szs
        .islands
        .iter()
        .position(|island| island.number == ISLAND)
        .expect("island 10");
    sim.players[0].gold = 100_000;
    sim.players[0].unlock_mask = u32::MAX;

    let grass = resource_cells(&sim, GRAS);
    assert!(grass.len() > 500, "island {ISLAND} ships {} grass", grass.len());
    let placed = ranked_anchors(&grass, 3)
        .into_iter()
        .take(96)
        .find_map(|(x, y)| {
            let before = sim.buildings.len();
            anno_game::game_commands::place_building(
                &mut sim,
                &corpus.szs.islands,
                island_index,
                &corpus.defs,
                &corpus.cod,
                farm_index,
                0,
                0,
                x,
                y,
            );
            (sim.buildings.len() > before).then_some((x, y))
        });
    let (x, y) = placed.expect("a 2x2 sheep farm fits somewhere in the grassland");
    let building_index = sim.buildings.len() - 1;
    ready(&mut sim, building_index);

    let root_index = sim
        .source_map_cell_states
        .iter()
        .position(|state| state.matches(ISLAND, x as u16, y as u16))
        .expect("the placed sheep farm owns a live source cell record");

    let mut peak_grazers = 0;
    let mut peak_raw = 0;
    let mut peak_output = 0;
    for _ in 0..8_000 {
        sim.tick(100);
        // `FUN_0044af10` keys its census on the home cell *and* settlement
        // selector, so count only this farm's own herd — the AI colonies run
        // pastures of their own.
        peak_grazers = peak_grazers.max(
            sim.figures
                .iter()
                .filter(|figure| {
                    figure.is_active()
                        && figure.origin_production_kind == 4
                        && figure.origin_island == ISLAND
                        && figure.origin_x == x as u16
                        && figure.origin_y == y as u16
                })
                .count(),
        );
        peak_raw = peak_raw.max(sim.source_map_cell_states[root_index].raw_material_stock);
        peak_output = peak_output.max(sim.buildings[building_index].output_stock);
        if peak_output >= 1 && peak_grazers > 1 {
            break;
        }
    }
    println!("sheep farm at {x},{y}: grazers {peak_grazers}, raw {peak_raw}, wool {peak_output}");

    assert!(peak_grazers > 0, "FUN_0044bb40 never allocated a SCHAF");
    assert!(
        peak_grazers > 1,
        "FUN_0044af10 admits Figuranz grazers at once, saw {peak_grazers}"
    );
    assert!(peak_grazers <= 3, "the herd cap is Figuranz: 3, saw {peak_grazers}");
    assert!(
        peak_raw > 0,
        "no grazed grass reached the farm's raw buffer +0x0a"
    );
    assert!(
        peak_output >= 1,
        "the sheep farm produced {peak_output} whole wool (raw peak {peak_raw}, grazers {peak_grazers})"
    );
}
