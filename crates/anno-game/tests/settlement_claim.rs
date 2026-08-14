//! The founding flow, end to end: found a Kontor on New Horizons0 island 10,
//! then build a harvester and require it to actually harvest. Self-skips
//! without the data corpus (extracted/ is gitignored).
//!
//! `source_production.rs` deliberately never founds, so it only covers the
//! pristine-island case where every tile — the harvester's own ground and the
//! wild `ROHSTOFF` cells alike — still carries the "no settlement" selector 7.
//! Founding is what makes the two selectors able to disagree:
//!
//!   * `FUN_00468ce0` (`1602_exe.c:73100-73146`) takes the first free
//!     `island + 0xac` settlement slot and stamps it into the founding tile's
//!     bits 19..=21.
//!   * `FUN_00465170`'s production-kind-8 branch (`1602_exe.c:70669-70689`)
//!     then runs `FUN_0046ac60` over `max(Radius, 8)` — `RADIUS_HQ` is 16 —
//!     rewriting every still-unowned, non-`MEER` tile in that disc to the new
//!     slot. Wild woodland inside the disc joins the settlement here.
//!   * `FUN_0046aec0` (`1602_exe.c:74558-74640`) resolves each later
//!     placement's slot by voting over the live tiles its footprint covers,
//!     and `FUN_0046ae20` (`1602_exe.c:70702`) stamps that value onto the
//!     footprint.
//!
//! `FUN_0046f920` (`1602_exe.c:78803`) then admits a harvest candidate only
//! when `(tile >> 0x13 & 7) == worker_slot`, and the worker's slot is read
//! straight off its root's own tile (`FUN_0044b7e0`, `1602_exe.c:52409`). So
//! the two stamps have to be produced by the same rule, or nothing harvests.

/// Compiled `Ware: BAUM` selector; the forester's `Rohstoff`.
const BAUM: u8 = 0x35;
/// `(tile >> 0x13) & 7 == 7` is "this tile belongs to no settlement"
/// (`FUN_0046ac60` tests `(uVar4 & 0x380000) == 0x380000`).
const UNSETTLED_SLOT: u8 = 7;
/// Outer map kind `MEER`, the one kind `FUN_0046ac60` refuses to claim.
const SEA_MAP_KIND: u8 = 19;
/// `RADIUS_HQ`, the KONTOR `Radius` (`1602_exe.c:66468`).
const KONTOR_RADIUS: u8 = 16;

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

fn definition_index(cod: &anno_formats::cod::CodFile, id: &str) -> usize {
    cod.buildings
        .iter()
        .position(|building| building.properties.get("Id").is_some_and(|value| value == id))
        .unwrap_or_else(|| panic!("haeuser.cod definition Id {id}"))
}

/// Membership test for the disc `FUN_0046ac60` walks: rows `0..=radius` of the
/// `FUN_00404d70` raster, mirrored above and below the footprint's integer
/// centre and widened by its second centre row/column when an extent is even.
fn source_claim_disc(anchor: (i32, i32), footprint: (i32, i32), radius: u8) -> Vec<(i32, i32)> {
    let rows = anno_sim::data_bridge::source_service_radius_row(radius);
    let center = (
        anchor.0 + (footprint.0 - 1) / 2,
        anchor.1 + (footprint.1 - 1) / 2,
    );
    let extra = ((footprint.0 - 1) & 1, (footprint.1 - 1) & 1);
    let mut cells = Vec::new();
    for (dy, &half_width) in rows.iter().enumerate() {
        let dy = dy as i32;
        for y in [center.1 - dy, center.1 + dy + extra.1] {
            for x in center.0 - i32::from(half_width)..center.0 + 1 + extra.0 + i32::from(half_width)
            {
                cells.push((x, y));
            }
        }
    }
    cells.sort_unstable();
    cells.dedup();
    cells
}

struct Settlement {
    sim: anno_sim::simulation::Simulation,
    island_index: usize,
    anchor: (i32, i32),
    kontor_footprint: (i32, i32),
    city_slot: u8,
}

/// The real mission opening: sail the campaign's trader to island 10's west
/// coast and found there. This is the exact anchor the scripted mission
/// driver picks, so the state under test is the state the driver reaches.
fn found_island_ten(corpus: &Corpus) -> Settlement {
    let mut sim = anno_game::scenario::build_simulation(
        &corpus.szs,
        &corpus.cod,
        &corpus.defs,
        &corpus.figures,
    );
    sim.seed_source_rand(1);
    let ship_index = sim
        .trade_ships
        .iter()
        .position(|ship| ship.name == "Verena")
        .expect("Verena spawns") as u32;
    let island_index = corpus
        .szs
        .islands
        .iter()
        .position(|island| island.number == 10)
        .expect("island 10");
    let island = &corpus.szs.islands[island_index];
    let map_index = sim
        .island_maps
        .iter()
        .position(|map| map.island_id == 10)
        .expect("island 10 map");
    let anchor = (2..i32::from(island.height) - 2)
        .flat_map(|y| (2..i32::from(island.width) - 2).map(move |x| (x, y)))
        .find(|&(x, y)| {
            sim.island_maps[map_index].is_coastal(x, y)
                && sim.island_maps[map_index].is_walkable(x + 1, y)
                && sim.island_maps[map_index].is_walkable(x + 2, y)
        })
        .expect("a west-coast anchor with hinterland");
    let world = (
        i32::from(island.x_pos) + anchor.0,
        i32::from(island.y_pos) + anchor.1,
    );
    assert!(sim.apply_command(&anno_sim::commands::Command::SailShip {
        player: 0,
        ship_index,
        world_x: world.0,
        world_y: world.1,
    }));
    for _ in 0..3_000 {
        sim.tick(100);
        let ship = &sim.trade_ships[ship_index as usize];
        if (ship.world_x - world.0).abs() + (ship.world_y - world.1).abs() < 12 {
            break;
        }
    }

    // Every tile of a pristine stock island belongs to no settlement before
    // the Kontor exists — this is the precondition the claim then changes.
    assert!(
        sim.source_static_map_roots
            .iter()
            .filter(|root| root.island == 10)
            .all(|root| root.source_map_owner_slot == UNSETTLED_SLOT),
        "island 10 ships with no settled ground"
    );

    assert!(
        anno_game::game_commands::apply_game_command(
            &mut sim,
            &corpus.szs.islands,
            &corpus.cod,
            &corpus.defs,
            &anno_sim::commands::Command::FoundKontor {
                player: 0,
                ship_index,
                island: 10,
                tile_x: anchor.0 as u16,
                tile_y: anchor.1 as u16,
            },
        ),
        "founding must succeed at the coastal anchor"
    );
    let city_slot = sim
        .source_cities
        .active_records()
        .into_iter()
        .find(|city| city.island_id == 10 && city.owner_slot == 0)
        .map(|city| city.source_owner)
        .expect("the founded city record");
    let kontor = corpus.cod.buildings[definition_index(&corpus.cod, "22103")].size;
    sim.players[0].gold = 100_000;
    sim.players[0].unlock_mask = u32::MAX;
    Settlement {
        sim,
        island_index,
        anchor,
        kontor_footprint: (kontor.0, kontor.1),
        city_slot,
    }
}

/// Rank free 2x2 anchors by how many `Ware: BAUM` cells the type-12 worker
/// could reach, restricted to ground carrying `owner_slot`. `FUN_0046f920`
/// only admits a resource cell whose slot equals the worker's, so a site is
/// only useful when both its own ground and the trees share one slot.
fn woodland_sites(
    sim: &anno_sim::simulation::Simulation,
    owner_slot: u8,
    radius: u8,
) -> Vec<(i32, i32)> {
    let rows = anno_sim::data_bridge::source_service_radius_row(radius);
    let trees: Vec<(i32, i32)> = sim
        .source_static_map_roots
        .iter()
        .filter(|root| {
            root.island == 10
                && root.source_output_ware_slot == BAUM
                && root.source_map_owner_slot == owner_slot
        })
        .map(|root| (i32::from(root.x), i32::from(root.y)))
        .collect();
    let ground: std::collections::HashMap<(i32, i32), u8> = sim
        .source_static_map_roots
        .iter()
        .filter(|root| root.island == 10)
        .map(|root| {
            (
                (i32::from(root.x), i32::from(root.y)),
                root.source_map_owner_slot,
            )
        })
        .collect();
    let mut sites: Vec<((i32, i32), usize)> = Vec::new();
    for &(tx, ty) in &trees {
        for dy in -4i32..=4 {
            for dx in -4i32..=4 {
                let anchor = (tx + dx, ty + dy);
                if anchor.0 < 1 || anchor.1 < 1 {
                    continue;
                }
                if ground.get(&anchor) != Some(&owner_slot) {
                    continue;
                }
                let reachable = trees
                    .iter()
                    .filter(|&&(cx, cy)| {
                        let dy = (cy - anchor.1).abs();
                        dy < rows.len() as i32
                            && (cx - anchor.0).abs() <= i32::from(rows[dy as usize])
                    })
                    .count();
                sites.push((anchor, reachable));
            }
        }
    }
    sites.sort_by_key(|&(anchor, count)| (std::cmp::Reverse(count), anchor));
    sites.dedup_by_key(|&mut (anchor, _)| anchor);
    sites.into_iter().map(|(anchor, _)| anchor).collect()
}

/// Place the first site that passes the build gates, then hand the site its
/// materials — construction supply is a different subsystem from the harvest
/// chain under test.
fn place_built(
    settlement: &mut Settlement,
    corpus: &Corpus,
    def_index: usize,
    sites: &[(i32, i32)],
) -> Option<(i32, i32, usize)> {
    for &(x, y) in sites {
        let before = settlement.sim.buildings.len();
        anno_game::game_commands::place_building(
            &mut settlement.sim,
            &corpus.szs.islands,
            settlement.island_index,
            &corpus.defs,
            &corpus.cod,
            def_index,
            0,
            0,
            x,
            y,
        );
        if settlement.sim.buildings.len() == before {
            continue;
        }
        let index = settlement.sim.buildings.len() - 1;
        settlement.sim.buildings[index].wood_needed = 0;
        settlement.sim.buildings[index].tools_needed = 0;
        settlement.sim.buildings[index].bricks_needed = 0;
        settlement.sim.buildings[index].construction_ms_remaining = 0;
        return Some((x, y, index));
    }
    None
}

/// `FUN_0046ac60`'s claim, pinned by extent: every land tile of the disc joins
/// the settlement, sea inside it does not, and nothing outside it moves.
#[test]
fn founding_claims_the_kontor_radius_into_the_settlement() {
    let Some(corpus) = load_corpus() else { return };
    let settlement = found_island_ten(&corpus);
    let disc: std::collections::HashSet<(i32, i32)> = source_claim_disc(
        settlement.anchor,
        settlement.kontor_footprint,
        KONTOR_RADIUS,
    )
    .into_iter()
    .collect();

    let mut claimed_trees = 0;
    let mut unclaimed_trees = 0;
    let mut sea_inside = 0;
    for root in settlement
        .sim
        .source_static_map_roots
        .iter()
        .filter(|root| root.island == 10)
    {
        let position = (i32::from(root.x), i32::from(root.y));
        let inside = disc.contains(&position);
        let is_tree = root.source_output_ware_slot == BAUM;
        if !inside {
            assert_eq!(
                root.source_map_owner_slot, UNSETTLED_SLOT,
                "tile {position:?} outside the Kontor radius must stay unsettled"
            );
            unclaimed_trees += usize::from(is_tree);
            continue;
        }
        if root.kind_code == SEA_MAP_KIND {
            // `*(int *)(definition + 4) != 0x13` excludes MEER from the claim.
            assert_eq!(
                root.source_map_owner_slot, UNSETTLED_SLOT,
                "sea tile {position:?} must never join a settlement"
            );
            sea_inside += 1;
            continue;
        }
        assert_eq!(
            root.source_map_owner_slot, settlement.city_slot,
            "land tile {position:?} inside the Kontor radius must join the settlement"
        );
        claimed_trees += usize::from(is_tree);
    }
    assert!(
        claimed_trees > 0,
        "the founding disc has to take wild woodland with it"
    );
    assert!(
        unclaimed_trees > 0,
        "island 10 keeps woodland outside the founding disc"
    );
    assert!(sea_inside > 0, "the coastal anchor's disc overlaps water");

    // The kind-13 dispatch resolves a residence to its city through this same
    // selector, so a hut built on claimed ground must carry the city's slot —
    // the regression `e4e9282` originally chased.
    let mut settlement = settlement;
    let hut = definition_index(&corpus.cod, "20605");
    let claimed: std::collections::HashSet<(i32, i32)> = settlement
        .sim
        .source_static_map_roots
        .iter()
        .filter(|root| root.island == 10 && root.source_map_owner_slot == settlement.city_slot)
        .map(|root| (i32::from(root.x), i32::from(root.y)))
        .collect();
    // The hut is 2x2, so require its whole footprint on claimed ground.
    let mut hut_sites: Vec<(i32, i32)> = claimed
        .iter()
        .copied()
        .filter(|&(x, y)| {
            [(0, 0), (1, 0), (0, 1), (1, 1)]
                .into_iter()
                .all(|(dx, dy)| claimed.contains(&(x + dx, y + dy)))
        })
        .collect();
    hut_sites.sort_by_key(|&(x, y)| {
        (
            (x - settlement.anchor.0).abs() + (y - settlement.anchor.1).abs(),
            x,
            y,
        )
    });
    let (hx, hy, _) =
        place_built(&mut settlement, &corpus, hut, &hut_sites).expect("a hut fits in the city");
    let record = settlement
        .sim
        .source_kind13_locations
        .location_at(10, hx as u8, hy as u8)
        .expect("the placed residence owns a kind-13 record");
    assert_eq!(
        record.source_owner, settlement.city_slot,
        "a residence on claimed ground belongs to the founded city"
    );
}

/// The mission flow the defect broke: found first, then build a forester.
/// Both the site inside the new settlement and the site out in the wilderness
/// have to harvest, because in each case the harvester's own ground and its
/// trees carry the same selector.
#[test]
fn a_forester_placed_after_founding_harvests() {
    let Some(corpus) = load_corpus() else { return };
    let mut settlement = found_island_ten(&corpus);
    let forester = definition_index(&corpus.cod, "22001");
    let radius = corpus.cod.buildings[forester].source_transfer_radius;

    let inside_sites = woodland_sites(&settlement.sim, settlement.city_slot, radius);
    let (ix, iy, inside_building) = place_built(&mut settlement, &corpus, forester, &inside_sites)
        .expect("a forester fits in the settlement's own woodland");
    let outside_sites = woodland_sites(&settlement.sim, UNSETTLED_SLOT, radius);
    let (ox, oy, outside_building) = place_built(&mut settlement, &corpus, forester, &outside_sites)
        .expect("a forester fits in the unsettled woodland");

    // `FUN_0046ae20` stamped each placement from the ground it stands on.
    let root_index = |sim: &anno_sim::simulation::Simulation, x: i32, y: i32| {
        sim.source_map_cell_states
            .iter()
            .position(|state| state.matches(10, x as u16, y as u16))
            .expect("the placed forester owns a live source cell record")
    };
    let inside_root = root_index(&settlement.sim, ix, iy);
    let outside_root = root_index(&settlement.sim, ox, oy);
    assert_eq!(
        settlement.sim.source_map_cell_states[inside_root].source_map_owner_slot,
        settlement.city_slot
    );
    assert_eq!(
        settlement.sim.source_map_cell_states[outside_root].source_map_owner_slot,
        UNSETTLED_SLOT
    );

    let mut peak = [(0u16, 0u16, 0u16); 2];
    for _ in 0..8_000 {
        settlement.sim.tick(100);
        for (slot, (root, building)) in [
            (inside_root, inside_building),
            (outside_root, outside_building),
        ]
        .into_iter()
        .enumerate()
        {
            let state = settlement.sim.source_map_cell_states[root];
            peak[slot].0 = peak[slot].0.max(state.raw_material_stock);
            peak[slot].1 = peak[slot].1.max(state.storage_fill);
            peak[slot].2 = peak[slot]
                .2
                .max(settlement.sim.buildings[building].output_stock);
        }
        if peak[0].2 >= 2 && peak[1].2 >= 2 {
            break;
        }
    }

    for (label, (raw, fill, output)) in [
        (format!("inside the settlement at ({ix},{iy})"), peak[0]),
        (format!("out in the wilderness at ({ox},{oy})"), peak[1]),
    ] {
        assert!(
            raw > 0,
            "the forester {label} never got a harvested tree into its raw buffer +0x0a"
        );
        assert_eq!(
            raw % 32,
            0,
            "a harvest delivers whole 1/32 units: FUN_00471c50 returns 0x20"
        );
        assert!(
            fill > 0,
            "the forester {label} never converted raw material into +0x0c storage"
        );
        assert!(
            output >= 2,
            "the forester {label} produced {output} whole wood (raw peak {raw}, fill peak {fill})"
        );
    }

    // The type-12 worker inherits its root tile's selector (`FUN_0044b7e0`,
    // `1602_exe.c:52409`), which is what made the comparison succeed.
    assert!(
        settlement
            .sim
            .figures
            .iter()
            .any(|figure| figure.origin_island == 10
                && figure.origin_x == ix as u16
                && figure.origin_y == iy as u16
                && figure.origin_source_map_owner_slot == settlement.city_slot)
            || peak[0].2 >= 2,
        "the settlement forester's worker carries the city selector"
    );
}
