//! End-to-end logistics: what a colony produces has to reach the colony's
//! store. Self-skips without the data corpus.
//!
//! Every other production test in this repo stops at the producer's own
//! `+0x0c` storage. That is one hop short of the thing the player sees: a
//! forester whose store fills to `Maxlager` and then stalls at activity zero
//! looks identical, from the root record, to a healthy one. The missing hop is
//! the type-11 city cart — `FUN_0047daf0` case 7/8 (`1602_exe.c:90075-90080`)
//! dispatches `FUN_0044ad50`, whose search `FUN_004596b0`
//! (`1602_exe.c:61822-61905`) rasterises a window, reopens the requesting
//! root's own footprint through `FUN_004710b0`, floods with `FUN_004717b0`,
//! and reserves the winner's storage through `FUN_0047d810`.

use anno_sim::commands::Command;
use anno_sim::types::Good;

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

/// Compiled `Ware: BAUM`, the forester's `Rohstoff`.
const BAUM: u8 = 0x35;
/// haeuser.cod `Id: 22001`, FOERSTEREI.
const FORESTER_ID: &str = "22001";
/// The island the campaign leaves unsettled.
const ISLAND: u8 = 10;

/// The outer map kinds `FUN_004704d0` opens for a transfer wave
/// (`1602_exe.c:79470-79477`): `STRASSE`, `BODEN`, `RUINE`, `PLATZ`,
/// `BRUECKE`, `STRANDRUINE`, `PIER`. Everything else — sea, the surf and
/// beach ring, forest — is impassable unless it is a goal.
fn source_open_path_kind(kind_code: u8) -> bool {
    matches!(kind_code, 1 | 11 | 12 | 13 | 18 | 29 | 30)
}

/// Found a Kontor on island 10 at a coastline the cart wave can actually
/// leave, put a forester in the claimed woodland next to it, and run the
/// colony.
///
/// The site rules here are the ones the original enforces through its build
/// gate, which this port does not have yet (`docs/logistics-gaps.md` §6): the
/// Kontor's oriented footprint has to reach open ground, and the forester has
/// to stand on open ground too. `IslandMap::is_walkable` counts `MEER` as
/// walkable, so a naive "first coastal tile with walkable neighbours" search
/// puts the whole colony out at sea, where no wave can reach it.
#[test]
fn a_founded_colony_carts_its_forester_wood_into_the_warehouse() {
    let Some(corpus) = load_corpus() else { return };
    let mut sim = anno_game::scenario::build_simulation(
        &corpus.szs,
        &corpus.cod,
        &corpus.defs,
        &corpus.figures,
    );
    sim.seed_source_rand(1);

    let island = corpus
        .szs
        .islands
        .iter()
        .find(|island| island.number == ISLAND)
        .expect("New Horizons0 ships island 10")
        .clone();
    let map_index = sim
        .island_maps
        .iter()
        .position(|map| map.island_id == ISLAND)
        .expect("island 10 has a map");
    let kind_at = |sim: &anno_sim::simulation::Simulation, x: i32, y: i32| -> u8 {
        sim.island_maps[map_index]
            .source_map_kind_and_owner(x, y)
            .map(|(kind, _)| kind)
            .unwrap_or(u8::MAX)
    };

    // The founding Kontor is `Id: 22103`, 2x3 unrotated.
    let kontor = corpus
        .cod
        .buildings
        .iter()
        .find(|building| building.source_id == 22103)
        .expect("founding Kontor definition");
    let (kontor_w, kontor_h) = kontor.size;
    // The anchor has to be near the ship's start (`found_kontor` requires the
    // ship within 12 world tiles) and its 2x3 footprint has to reach open
    // ground, or the cart wave has nowhere to step out to.
    let anchor = (2..i32::from(island.height) - kontor_h - 2)
        .flat_map(|y| (2..i32::from(island.width) - kontor_w - 2).map(move |x| (x, y)))
        .find(|&(x, y)| {
            sim.island_maps[map_index].is_coastal(x, y)
                && (0..kontor_h).any(|dy| {
                    (0..kontor_w).any(|dx| source_open_path_kind(kind_at(&sim, x + dx, y + dy)))
                })
        })
        .expect("a coastline whose Kontor footprint reaches open ground");

    // The ship has to stop on navigable water inside `found_kontor`'s range.
    let dock = (-8_i32..=8)
        .flat_map(|dy| (-8_i32..=8).map(move |dx| (dx, dy)))
        .map(|(dx, dy)| (anchor.0 + dx, anchor.1 + dy))
        .filter(|&(x, y)| x >= 0 && y >= 0 && kind_at(&sim, x, y) == 19)
        .min_by_key(|&(x, y)| (x - anchor.0).abs() + (y - anchor.1).abs())
        .expect("open water beside the anchor");
    let ship = sim
        .trade_ships
        .iter()
        .position(|ship| ship.name == "Verena")
        .expect("the campaign's starting ship") as u32;
    let world = (
        i32::from(island.x_pos) + dock.0,
        i32::from(island.y_pos) + dock.1,
    );
    assert!(sim.apply_command(&Command::SailShip {
        player: 0,
        ship_index: ship,
        world_x: world.0,
        world_y: world.1,
    }));
    for _ in 0..3_000 {
        sim.tick(100);
        let ship = &sim.trade_ships[ship as usize];
        if (ship.world_x - world.0).abs() + (ship.world_y - world.1).abs() < 12 {
            break;
        }
    }
    assert!(
        anno_game::game_commands::apply_game_command(
            &mut sim,
            &corpus.szs.islands,
            &corpus.cod,
            &corpus.defs,
            &Command::FoundKontor {
                player: 0,
                ship_index: ship,
                island: ISLAND,
                tile_x: anchor.0 as u16,
                tile_y: anchor.1 as u16,
            },
        ),
        "founding at {anchor:?} was refused"
    );

    // Site the forester on open ground, as close to the Kontor as a site with
    // woodland in its compiled `Radius: 3` allows.
    let forester_index = corpus
        .cod
        .buildings
        .iter()
        .position(|building| {
            building
                .properties
                .get("Id")
                .is_some_and(|id| id == FORESTER_ID)
        })
        .expect("forester definition");
    let forester = &corpus.defs[forester_index];
    let (forester_w, forester_h) = (i32::from(forester.width), i32::from(forester.height));
    let radius = i32::from(forester.radius).max(1);
    let trees: Vec<(i32, i32)> = sim
        .source_static_map_roots
        .iter()
        .filter(|cell| {
            cell.island == ISLAND
                && cell.source_production_kind_code == 9
                && cell.source_output_ware_slot == BAUM
        })
        .map(|cell| (i32::from(cell.x), i32::from(cell.y)))
        .collect();
    let site = (2..i32::from(island.height) - forester_h - 2)
        .flat_map(|y| (2..i32::from(island.width) - forester_w - 2).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (dx, dy) = (x - anchor.0, y - anchor.1);
            // Inside the Kontor's radius-16 claim, on open ground, with the
            // Kontor's own footprint left free.
            dx * dx + dy * dy <= 16 * 16
                && (0..forester_h).all(|dy| {
                    (0..forester_w).all(|dx| source_open_path_kind(kind_at(&sim, x + dx, y + dy)))
                })
                && (x >= anchor.0 + kontor_w
                    || anchor.0 >= x + forester_w
                    || y >= anchor.1 + kontor_h
                    || anchor.1 >= y + forester_h)
        })
        .filter(|&(x, y)| {
            trees
                .iter()
                .any(|&(tx, ty)| (tx - x).pow(2) + (ty - y).pow(2) <= radius * radius)
        })
        .min_by_key(|&(x, y)| (x - anchor.0).abs().max((y - anchor.1).abs()))
        .expect("open ground with woodland in range inside the claim");
    assert!(
        anno_game::game_commands::apply_game_command(
            &mut sim,
            &corpus.szs.islands,
            &corpus.cod,
            &corpus.defs,
            &Command::PlaceBuilding {
                player: 0,
                island: ISLAND,
                tile_x: site.0 as u16,
                tile_y: site.1 as u16,
                def_index: forester_index as u16,
                orientation: 0,
            },
        ),
        "forester placement at {site:?} was refused"
    );

    let warehouse_index = sim
        .warehouses
        .iter()
        .position(|warehouse| warehouse.island_id == ISLAND && warehouse.owner == 0)
        .expect("founding creates the island warehouse");

    // `FUN_00480610` (`1602_exe.c:91902-91926`) zeroes a ware's eligibility
    // byte whenever `FUN_0047aa00` reports no free space for it, and
    // `FUN_004717b0` refuses every candidate whose byte is zero
    // (`1602_exe.c:80578-80583`). The ship lands its whole hold into a
    // `Maxlager: 50` store, so Wood starts at exactly capacity and the colony
    // cannot collect a single log until some is spent. Spend it here — the
    // subject of this test is the cart, not the founding cargo.
    let landed_wood = sim.warehouses[warehouse_index].stock(Good::Wood);
    sim.warehouses[warehouse_index].withdraw(Good::Wood, landed_wood);
    assert_eq!(sim.warehouses[warehouse_index].stock(Good::Wood), 0);

    let mut carts_seen = 0;
    let mut delivered = 0;
    for _ in 0..6_000 {
        sim.tick(100);
        carts_seen = carts_seen.max(
            sim.figures
                .iter()
                .filter(|figure| {
                    figure.is_active()
                        && figure.origin_island == ISLAND
                        && figure.cargo_route == anno_sim::entity::CargoRoute::CityCart
                })
                .count(),
        );
        delivered = sim.warehouses[warehouse_index].stock(Good::Wood);
        if delivered >= 2 {
            break;
        }
    }

    let root = sim
        .source_map_cell_states
        .iter()
        .find(|state| state.matches(ISLAND, site.0 as u16, site.1 as u16))
        .copied()
        .expect("the forester owns a live source record");
    assert!(
        carts_seen > 0,
        "no type-11 city cart was ever dispatched from the Kontor at {anchor:?} \
         (forester at {site:?}, root fill {} / {})",
        root.storage_fill,
        root.storage_animation_capacity,
    );
    assert!(
        delivered > 0,
        "the forester at {site:?} produced but the island warehouse never \
         received any Wood (root fill {} / {}, raw {})",
        root.storage_fill,
        root.storage_animation_capacity,
        root.raw_material_stock,
    );
}
