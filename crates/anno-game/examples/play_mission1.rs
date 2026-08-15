//! Staged scripted playthrough of the first campaign mission
//! ("Halfway there", `New Horizons0.szs`): sail, found, and grow the colony
//! past the AUFTRAG4 goal of 100 inhabitants.
//!
//! This driver is a **player**, not engine code. Every rule it needs is asked
//! of the engine rather than restated here:
//!
//! * `can_place_building` — terrain and footprint (`FUN_00464450`);
//! * `source_placement_settlement_slot` + `source_placement_area_admits` —
//!   the buildable-area gate (`1602_exe.c:7612-7616`), i.e. whose ground this
//!   footprint stands on and whether a build is allowed there at all;
//! * `source_transfer_wave_opens_ground_kind` — what a Karren walks;
//! * `source_service_radius_row` — the compiled service-disc raster
//!   (`FUN_00404d70`) that both the marketplace land claim and the
//!   house-coverage rescan measure with;
//! * the live kind-13 house records — `state_bits`, `lifecycle_flags` and
//!   `source_transition_active_for_group` say which infrastructure a house is
//!   actually missing, so the driver never has to guess.
//!
//! A previous version paraphrased the transfer wave's open-kind set locally,
//! drifted from the engine's copy, and produced a false conclusion about the
//! game. Keep asking.
use anno_sim::commands::Command;
use anno_sim::types::Good;
use std::collections::HashSet;

type Sim = anno_sim::simulation::Simulation;

/// Placement bookkeeping for the end-of-run report.
#[derive(Default)]
struct Tally {
    accepted: usize,
    refused: usize,
}

/// Compiled outer map kind `WALD`. Forest is legally buildable — the original
/// clears trees — but clearing it destroys the resource the forester lives on,
/// and island 10's woodland is the colony's only wood. Letting housing take
/// forest cells cost an earlier run its entire wood supply (wood pinned at 0,
/// population 12 against 58 when the trees were left alone), so the woodland
/// is treated as reserved rather than as free room.
const FOREST_KIND: u8 = 10;

const HUT: usize = 414;
const MARKET: usize = 468;
const CHAPEL: usize = 463;
const FORESTER: usize = 402;
const FISHERY: usize = 270;
const SHEEP: usize = 412;
const WEAVER: usize = 388;
/// Ware slots are `0x2d + crop bit`: 52 GRAS, 53 BAUM, 57 FISCHE.
const GRAS: u8 = 52;
const BAUM: u8 = 53;
const FISCHE: u8 = 57;

fn main() {
    let root = std::path::Path::new("extracted");
    let szs_data = std::fs::read(root.join("Szenes/New Horizons0.szs")).unwrap();
    let mut szs = anno_formats::szs::SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, root, 1);
    let cod_data = std::fs::read(root.join("haeuser.cod")).unwrap();
    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("figuren.cod"))
        .map(|b| anno_formats::figuren::FiguresFile::parse(&b))
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let mut sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    sim.seed_source_rand(1);

    // --- Stage 1a: sail to island 10 and found ---
    let ship = sim
        .trade_ships
        .iter()
        .position(|s| s.name == "Verena")
        .unwrap() as u32;
    let island = szs.islands.iter().find(|i| i.number == 10).unwrap().clone();
    let mi = sim
        .island_maps
        .iter()
        .position(|m| m.island_id == 10)
        .unwrap();
    // The transfer waves rasterise the live map with `FUN_004704d0` /
    // `FUN_004706e0` and open only outer kinds `{1, 0xb, 0xc, 0xd, 0x12, 0x1d,
    // 0x1e}` (`1602_exe.c:79470-79477`) — road, ground, ruin, plaza, bridge,
    // beach-ruin, pier. Sea, the surf/beach ring and forest are impassable.
    // `IslandMap::is_walkable` counts `MEER` as walkable, so siting by that
    // helper alone put the whole colony out at sea, where every Karren search
    // returns `NoRoute` and nothing the colony makes can ever be collected.
    // Ask the engine which kinds a transfer wave walks; do not paraphrase
    // it here. A local copy of this set drifted from the engine's and made
    // buildable land look scarce.
    let open_ground = anno_sim::island_map::source_transfer_wave_opens_ground_kind;
    let kind_at = |sim: &Sim, x: i32, y: i32| {
        sim.island_maps[mi]
            .source_map_kind_and_owner(x, y)
            .map(|(k, _)| k)
            .unwrap_or(u8::MAX)
    };
    // How much ground can a Karren reach from a 2x3 Kontor rooted here? The
    // transfer wave floods the rasterised window out of the requesting root's
    // own footprint (`FUN_004710b0`, `1602_exe.c:80003`) and only over the
    // open outer kinds, taking a diagonal solely when both orthogonal cells
    // beside it are clear (`FUN_0046c7d0`). A producer outside that component
    // is stranded however much it makes, so found where the component is
    // biggest — which is what a player picking a harbour does by eye.
    //
    // `walls` are the footprints the colony has already put down. The source
    // wave rasterises the **live** map, so a placed building is not open ground
    // any more: a colony that packs its houses in can cut its own producers off
    // the Kontor without a single refusal to warn it. That is exactly what
    // happened here — two foresters sat at `fill = 320 / cap = 320` with the
    // warehouse's wood pinned at 0 for the rest of the run — so the component
    // is recomputed against the live walls after every placement, and no site
    // is taken that strands something already built.
    let cart_component = |sim: &Sim, anchor: (i32, i32), walls: &HashSet<(i32, i32)>| {
        let kind = |x: i32, y: i32| {
            sim.island_maps[mi]
                .source_map_kind_and_owner(x, y)
                .map(|(k, _)| k)
                .unwrap_or(u8::MAX)
        };
        let open_cell = |x: i32, y: i32| open_ground(kind(x, y)) && !walls.contains(&(x, y));
        let mut seen = HashSet::new();
        let mut queue: Vec<(i32, i32)> = (0..3)
            .flat_map(|dy| (0..2).map(move |dx| (anchor.0 + dx, anchor.1 + dy)))
            .collect();
        for cell in &queue {
            seen.insert(*cell);
        }
        while let Some((x, y)) = queue.pop() {
            for (dx, dy) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let next = (x + dx, y + dy);
                if next.0 < 0
                    || next.1 < 0
                    || next.0 >= island.width as i32
                    || next.1 >= island.height as i32
                    || seen.contains(&next)
                    || !open_cell(next.0, next.1)
                {
                    continue;
                }
                if dx != 0 && dy != 0 && !(open_cell(x + dx, y) && open_cell(x, y + dy)) {
                    continue;
                }
                seen.insert(next);
                queue.push(next);
            }
        }
        seen
    };
    // The Kontor is `Kind: HQ` (35), whose terrain arm admits only
    // `{STRASSE, WALD, BODEN, RUINE, PLATZ}` (`FUN_00464660`), and it reaches
    // the water through `Strandflg` — it stands on land and *fronts* a beach
    // rather than standing in the surf. Ask the real gate: "coastal plus one
    // open footprint cell" used to accept a footprint half in the sea.
    let kontor_def_index = cod
        .buildings
        .iter()
        .position(|b| b.source_id == 22103)
        .expect("Kontor definition");
    let anchor = (2..island.height as i32 - 5)
        .flat_map(|y| (2..island.width as i32 - 4).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            sim.island_maps[mi].is_coastal(x, y)
                && anno_game::game_commands::can_place_building(
                    &island,
                    &sim.island_maps[mi],
                    &defs[kontor_def_index],
                    x,
                    y,
                    2,
                    3,
                )
        })
        .max_by_key(|&anchor| cart_component(&sim, anchor, &HashSet::new()).len())
        .expect("buildable coastal anchor for the Kontor footprint");
    // The ship has to stop on navigable water within `found_kontor`'s range.
    let dock = (-8i32..=8)
        .flat_map(|dy| (-8i32..=8).map(move |dx| (dx, dy)))
        .map(|(dx, dy)| (anchor.0 + dx, anchor.1 + dy))
        .filter(|&(x, y)| x >= 0 && y >= 0 && kind_at(&sim, x, y) == 19)
        .min_by_key(|&(x, y)| (x - anchor.0).abs() + (y - anchor.1).abs())
        .expect("open water beside the anchor");
    let world = (
        i32::from(island.x_pos) + dock.0,
        i32::from(island.y_pos) + dock.1,
    );
    assert!(sim.apply_command(&Command::SailShip {
        player: 0,
        ship_index: ship,
        world_x: world.0,
        world_y: world.1
    }));
    for _ in 0..3000 {
        sim.tick(100);
        let s = &sim.trade_ships[ship as usize];
        if (s.world_x - world.0).abs() + (s.world_y - world.1).abs() < 12 {
            break;
        }
    }
    let ok = anno_game::game_commands::apply_game_command(
        &mut sim,
        &szs.islands,
        &cod,
        &defs,
        &Command::FoundKontor {
            player: 0,
            ship_index: ship,
            island: 10,
            tile_x: anchor.0 as u16,
            tile_y: anchor.1 as u16,
        },
    );
    println!("founded at {anchor:?}: {ok}");
    assert!(ok);

    // Which ground can a Karren actually reach from the Kontor? Flood the
    // engine's open kinds once, from the Kontor footprint, and refuse to put a
    // producer or a market anywhere the cart cannot follow.
    // The Kontor's own 2x3 footprint is a wall for everything that follows,
    // but it is also where the wave starts, so it is seeded into the flood
    // rather than into `walls`.
    let mut walls: HashSet<(i32, i32)> = HashSet::new();
    // Every footprint whose goods a Karren has to fetch. Houses are not in it:
    // a house is served by the market disc, not by a cart.
    let mut served: Vec<(i32, i32, u8, u8)> = Vec::new();
    let mut reachable = cart_component(&sim, anchor, &walls);
    println!(
        "cart-reachable open ground from the Kontor: {}",
        reachable.len()
    );
    // A supplier's own cell is opened by the raster as a *goal* whatever its
    // terrain (`FUN_004704d0` stamps the goal bit before the kind test), so the
    // cart only has to reach a cell adjacent to the footprint — which is what
    // lets a beach-only fishery be collected at all.
    let cart_can_reach = |comp: &HashSet<(i32, i32)>, x: i32, y: i32, w: u8, h: u8| {
        (-1..=i32::from(h))
            .flat_map(|dy| (-1..=i32::from(w)).map(move |dx| (dx, dy)))
            .any(|(dx, dy)| comp.contains(&(x + dx, y + dy)))
    };
    // NOTE. The source re-reads the live map on every transfer search, so in
    // the original a building the player raises immediately blocks the wave
    // (`island_map.rs`, `source_transfer_window_grid`). This port's
    // `place_building` never rewrites that layer — it only clears `walkable`
    // and sets the runtime-occupied bit — so placed footprints do **not**
    // narrow the component here. A version of this driver that vetoed any site
    // which would have severed a producer under the source rule therefore threw
    // away most of the island's buildable ground for nothing. The wall set is
    // still tracked, because it is what a faithful engine would consult, but it
    // is not used to refuse a placement.

    // ------------------------------------------------------------------
    // Placement gates — both halves asked of the engine.
    // ------------------------------------------------------------------
    // `can_place_building` answers terrain and occupancy only; the second,
    // *territorial* half of the verdict lives in `source_placement_area_admits`
    // fed by the settlement slot the oriented footprint resolves to. On ground
    // no settlement of ours owns, a build is accepted only from a player with
    // no settlement yet on the island — which, once the Kontor stands, is
    // never us. So the claim is the colony's whole buildable world, and a
    // marketplace is how it buys more of it (`FUN_0046ac60`).
    let admitted = |sim: &Sim, def_index: usize, x: i32, y: i32| -> bool {
        let def = &defs[def_index];
        let (w, h) = (def.width, def.height);
        anno_game::game_commands::can_place_building(&island, &sim.island_maps[mi], def, x, y, w, h)
            && sim.source_placement_area_admits(
                10,
                sim.source_placement_settlement_slot(10, x, y, w, h, 0),
                0,
                def.source_kind_code().unwrap_or(u8::MAX),
            )
    };
    // Forest is a hard refusal (see `FOREST_KIND`).
    let unforested = |sim: &Sim, def_index: usize, x: i32, y: i32| -> bool {
        let def = &defs[def_index];
        (0..i32::from(def.height)).all(|dy| {
            (0..i32::from(def.width)).all(|dx| {
                sim.island_maps[mi]
                    .source_map_kind_and_owner(x + dx, y + dy)
                    .map(|(k, _)| k != FOREST_KIND)
                    .unwrap_or(false)
            })
        })
    };

    // Service discs. The half-width of each row comes from the engine's own
    // compiled raster `FUN_00404d70` — the same table `claim_source_settlement_area`
    // hands out ground with and `refresh_source_house_infrastructure` hands out
    // coverage with. The centre is the oriented footprint's integer centre
    // (`1602_exe.c:74434-74512`). This is a *prediction* used for ranking;
    // whether a tile really joined the settlement is always re-asked of
    // `source_placement_settlement_slot` afterwards.
    let centre_of = |x: i32, y: i32, w: u8, h: u8| {
        (x + (i32::from(w) - 1) / 2, y + (i32::from(h) - 1) / 2)
    };
    let in_disc = |rows: &[u8], centre: (i32, i32), p: (i32, i32)| {
        let dy = (p.1 - centre.1).unsigned_abs() as usize;
        rows.get(dy)
            .is_some_and(|&half| (p.0 - centre.0).abs() <= i32::from(half))
    };
    let market_rows = anno_sim::data_bridge::source_service_radius_row(
        cod.buildings[MARKET].source_transfer_radius,
    );
    let chapel_rows = anno_sim::data_bridge::source_service_radius_row(
        cod.buildings[CHAPEL].source_transfer_radius,
    );

    // ------------------------------------------------------------------
    // Colony state the driver keeps for itself.
    // ------------------------------------------------------------------
    // Pasture the sheep farms graze: 624 of island 10's 630 open-ground tiles
    // carry a ripe GRAS cell, and a footprint deletes the static roots it
    // covers, so housing dropped anywhere would slowly eat the wool line the
    // way an earlier run ate the wood line. These are a *soft* reservation —
    // preferred against, not refused — because there is no other ground.
    let mut pasture: HashSet<(i32, i32)> = HashSet::new();
    let mut refused_sites: HashSet<(usize, (i32, i32))> = HashSet::new();
    let mut markets: Vec<(i32, i32)> = Vec::new();
    let mut chapels: Vec<(i32, i32)> = Vec::new();
    // The next marketplace's plot, held back from everything else. A MARKT is
    // 4x3 and is the only way the colony buys ground; without a standing
    // reservation the housing it pays for fills the last plot big enough to
    // build the *next* one on, and the colony walls itself into its own claim
    // with the rest of the island unclaimed. One market plot is always kept.
    let mut plot: HashSet<(i32, i32)> = HashSet::new();
    let mut tally = Tally::default();

    let stock = |sim: &Sim, good: Good| -> u16 {
        sim.warehouses
            .iter()
            .find(|w| w.island_id == 10 && w.owner == 0)
            .map(|w| w.stock(good))
            .unwrap_or(0)
    };

    // One placement, always through `apply_game_command`. On a refusal, ask the
    // same gates again and print what each said, so a refusal is a diagnosis
    // rather than a mystery.
    #[allow(clippy::too_many_arguments)]
    let build_at = |sim: &mut Sim,
                    tally: &mut Tally,
                    refused_sites: &mut HashSet<(usize, (i32, i32))>,
                    walls: &mut HashSet<(i32, i32)>,
                    served: &mut Vec<(i32, i32, u8, u8)>,
                    reachable: &mut HashSet<(i32, i32)>,
                    needs_cart: bool,
                    def_index: usize,
                    x: i32,
                    y: i32,
                    label: &str|
     -> bool {
        let placed = anno_game::game_commands::apply_game_command(
            sim,
            &szs.islands,
            &cod,
            &defs,
            &Command::PlaceBuilding {
                player: 0,
                island: 10,
                tile_x: x as u16,
                tile_y: y as u16,
                def_index: def_index as u16,
                orientation: 0,
            },
        );
        if placed {
            tally.accepted += 1;
            let (w, h) = (defs[def_index].width, defs[def_index].height);
            for dy in 0..i32::from(h) {
                for dx in 0..i32::from(w) {
                    walls.insert((x + dx, y + dy));
                }
            }
            if needs_cart {
                served.push((x, y, w, h));
            }
            *reachable = cart_component(sim, anchor, walls);
        } else {
            tally.refused += 1;
            refused_sites.insert((def_index, (x, y)));
            let def = &defs[def_index];
            let (w, h) = (def.width, def.height);
            let terrain = anno_game::game_commands::can_place_building(
                &island,
                &sim.island_maps[mi],
                def,
                x,
                y,
                w,
                h,
            );
            let slot = sim.source_placement_settlement_slot(10, x, y, w, h, 0);
            let area = sim.source_placement_area_admits(
                10,
                slot,
                0,
                def.source_kind_code().unwrap_or(u8::MAX),
            );
            println!(
                "  refused {label} at ({x},{y}): terrain={terrain} slot={slot} area={area} \
                 gold={}/{} tools={}/{} bauinfra={} mask={:#x}",
                sim.players[0].gold,
                def.cost_gold,
                stock(sim, Good::Tools),
                def.cost_tools,
                def.bauinfra,
                sim.players[0].unlock_mask,
            );
        }
        println!("build {label} at ({x},{y}): {placed}");
        placed
    };

    // ------------------------------------------------------------------
    // Site searches.
    // ------------------------------------------------------------------
    // Every legal 2x2 house anchor on the island, split by the engine's
    // territorial verdict: `inside` is ground the colony may build on today,
    // `outside` is ground a marketplace would have to buy first.
    let survey_house_anchors = |sim: &Sim,
                                refused_sites: &HashSet<(usize, (i32, i32))>,
                                plot: &HashSet<(i32, i32)>|
     -> (Vec<(i32, i32)>, Vec<(i32, i32)>) {
        let def = &defs[HUT];
        let (w, h) = (def.width, def.height);
        let kind = def.source_kind_code().unwrap_or(u8::MAX);
        let (mut inside, mut outside) = (Vec::new(), Vec::new());
        for y in 0..i32::from(island.height) - i32::from(h) {
            for x in 0..i32::from(island.width) - i32::from(w) {
                if refused_sites.contains(&(HUT, (x, y)))
                    || !unforested(sim, HUT, x, y)
                    || (0..2).any(|dy| (0..2).any(|dx| plot.contains(&(x + dx, y + dy))))
                {
                    continue;
                }
                if !anno_game::game_commands::can_place_building(
                    &island,
                    &sim.island_maps[mi],
                    def,
                    x,
                    y,
                    w,
                    h,
                ) {
                    continue;
                }
                let slot = sim.source_placement_settlement_slot(10, x, y, w, h, 0);
                if sim.source_placement_area_admits(10, slot, 0, kind) {
                    inside.push((x, y));
                } else {
                    outside.push((x, y));
                }
            }
        }
        (inside, outside)
    };

    // Pick a house site. Coverage first — a house outside a marketplace disc
    // can never promote past pioneer (`source_transition_active_for_group(1)`
    // wants the market bit *and* a chapel), and one outside a chapel disc is
    // capped at two residents. Then pasture, then the engine's own market
    // distance class, which is literally the growth-rate input
    // (`source_kind13_variant_growth`).
    let house_site = |anchors: &[(i32, i32)],
                      markets: &[(i32, i32)],
                      chapels: &[(i32, i32)],
                      pasture: &HashSet<(i32, i32)>|
     -> Option<(i32, i32)> {
        anchors
            .iter()
            .copied()
            .min_by_key(|&(x, y)| {
                let c = centre_of(x, y, 2, 2);
                let market_class = markets
                    .iter()
                    .filter(|&&m| in_disc(&market_rows, m, c))
                    .map(|&m| {
                        anno_sim::data_bridge::source_market_distance_class(
                            (c.0 - m.0).unsigned_abs().min(255) as u8,
                            (c.1 - m.1).unsigned_abs().min(255) as u8,
                        )
                    })
                    .min();
                let chapel_covered = chapels.iter().any(|&c0| in_disc(&chapel_rows, c0, c));
                let grazed = (0..2)
                    .flat_map(|dy| (0..2).map(move |dx| (dx, dy)))
                    .filter(|&(dx, dy)| pasture.contains(&(x + dx, y + dy)))
                    .count();
                // Distance to a marketplace is the dominant term, and not by
                // a little: the engine's market distance class feeds both
                // `source_kind13_variant_growth` (0xa0 at class 0 down to 0x73
                // at class 6) *and* `source_kind13_variant_decay` (0x66 up to
                // 0xc0). A hut on the far rim of a market's disc grows two
                // thirds as fast and decays nearly twice as fast as one beside
                // it, which is how a sprawling colony peaks and then sheds
                // residents while the warehouse is full. Build compactly.
                (
                    u8::from(market_class.is_none()),
                    market_class.unwrap_or(7),
                    u8::from(!chapel_covered),
                    grazed,
                    (c.0 - anchor.0).abs() + (c.1 - anchor.1).abs(),
                )
            })
    };

    // Site a civic building deliberately: among the sites the engine admits,
    // take the one whose service disc covers the most of `targets`. For a
    // marketplace `targets` is either the houses the engine reports as having
    // no market coverage, or the house anchors that are still outside the
    // claim — the same search either way, because a MARKT both extends the
    // territory and adds a transfer root. Markets must sit inside the
    // cart-reachable component so the new root is itself served.
    #[allow(clippy::too_many_arguments)]
    let civic_site = |sim: &Sim,
                      refused_sites: &HashSet<(usize, (i32, i32))>,
                      pasture: &HashSet<(i32, i32)>,
                      reachable: &HashSet<(i32, i32)>,
                      plot: &HashSet<(i32, i32)>,
                      def_index: usize,
                      rows: &[u8],
                      targets: &[(i32, i32)],
                      need_cart: bool|
     -> Option<(i32, i32)> {
        let def = &defs[def_index];
        let (w, h) = (def.width, def.height);
        // Two passes. A marketplace wants to sit inside the cart-reachable
        // component so the new transfer root is itself served — but island 10's
        // component is a single 159-cell clearing walled in by forest, and once
        // the colony has built in it there may be no 4x3 plot left. A market
        // that is never visited by a Karren still claims ground and still
        // covers houses, which is most of what it is for here, so an unserved
        // plot beats no market at all. Never silently: the caller logs which.
        for require_cart in [need_cart, false] {
        let mut best: Option<((usize, usize, i32), (i32, i32))> = None;
        for y in 0..i32::from(island.height) - i32::from(h) {
            for x in 0..i32::from(island.width) - i32::from(w) {
                if refused_sites.contains(&(def_index, (x, y)))
                    || !unforested(sim, def_index, x, y)
                    || (require_cart && !cart_can_reach(reachable, x, y, w, h))
                    || !admitted(sim, def_index, x, y)
                    || (def_index != MARKET && plot.contains(&(x, y)))
                {
                    continue;
                }
                let c = centre_of(x, y, w, h);
                let hits = targets.iter().filter(|&&p| in_disc(rows, c, p)).count();
                if hits == 0 {
                    continue;
                }
                let grazed = (0..i32::from(h))
                    .flat_map(|dy| (0..i32::from(w)).map(move |dx| (dx, dy)))
                    .filter(|&(dx, dy)| pasture.contains(&(x + dx, y + dy)))
                    .count();
                let key = (
                    usize::MAX - hits,
                    grazed,
                    (c.0 - anchor.0).abs() + (c.1 - anchor.1).abs(),
                );
                if best.as_ref().is_none_or(|(bk, _)| key < *bk) {
                    best = Some((key, (x, y)));
                }
            }
        }
        if let Some((_, p)) = best {
            return Some(p);
        }
        }
        None
    };

    // A harvester only works if its raw resource lies inside its own Radius:
    // the worker searches the static map roots for ripe (kind 9) cells whose
    // output ware matches the building's `Rohstoff`, inside the building's
    // compiled `Radius` measured as a circle (`FUN_00404d70` rows).
    //
    // Rank sites by *sustained* yield, not by the instantaneous ripe count —
    // ranking on what happens to be ripe today is what sited two fisheries onto
    // 5 and 7 cells and starved the colony at 46 inhabitants. A harvested cell
    // leaves the ware entirely while it regrows (it reads back as an ordinary
    // ware-0 root), so the live table cannot be asked how much ground a site
    // commands; snapshot the island's authored resource layout once, before
    // anything has been harvested, and rank against that.
    let authored_resources: Vec<(u8, u8, (i32, i32))> = sim
        .source_static_map_roots
        .iter()
        .filter(|c| c.island == 10 && c.source_output_ware_slot != 0)
        .map(|c| {
            (
                c.source_output_ware_slot,
                c.kind_code,
                (i32::from(c.x), i32::from(c.y)),
            )
        })
        .collect();
    // Which settlement slot the colony's buildings carry. `FUN_0046f920`
    // compares a harvest candidate's slot against the *worker's* by exact
    // equality, so a tree standing on ground the colony has not claimed is
    // invisible to a forester built beside it.
    let colony_slot = sim
        .source_cities
        .active_records()
        .into_iter()
        .find(|c| c.island_id == 10 && c.owner_slot == 0)
        .map(|c| c.source_owner)
        .expect("the founded settlement's slot");
    #[allow(clippy::too_many_arguments)]
    let harvester_site = |sim: &Sim,
                          refused_sites: &HashSet<(usize, (i32, i32))>,
                          reachable: &HashSet<(i32, i32)>,
                          def_index: usize,
                          ware: u8,
                          focus: &[(i32, i32)]|
     -> Option<((i32, i32), usize, usize)> {
        let def = &defs[def_index];
        let (w, h) = (def.width, def.height);
        let radius = i32::from(def.radius).max(1);
        // Only cells the worker will actually accept count as yield, and the
        // engine already owns that predicate: `FUN_0046f920` takes a cell on an
        // always-walkable kind whatever its owner, and every other cell only
        // when its settlement slot equals the worker's. Ranking without that
        // test sited a forester on thirteen trees it was forbidden to touch and
        // pinned the colony's wood at zero for the rest of the run.
        //
        // A cell currently absent from the root table is one this colony
        // already harvested — it reads back as an ordinary ware-0 root while it
        // regrows — so it still counts toward the ground the site commands. A
        // cell that is present and refused is somebody else's, or nobody's, and
        // counts for nothing.
        //
        // A fishery is the exception: its worker is nested kind 6 and searches
        // the water grid, whose target predicate (`FUN_0046fb50`) matches on
        // outer kind `MEER` + nested `ROHSTOFF` + `Ware` and never reads the
        // tile's owner selector at all — which is the only reason a colony can
        // fish, since `claim_source_settlement_area` refuses to claim `MEER`.
        // The engine keeps that predicate private, so the one thing this driver
        // cannot ask it is which cells a fishery may work; it asks only whether
        // the *land* rule binds.
        let owner_bound = cod.buildings[def_index].source_production_kind_code() != Some(6);
        let mut live: std::collections::HashMap<(i32, i32), bool> =
            std::collections::HashMap::new();
        for cell in sim
            .source_static_map_roots
            .iter()
            .filter(|c| c.island == 10 && c.source_output_ware_slot == ware)
        {
            live.insert(
                (i32::from(cell.x), i32::from(cell.y)),
                (!owner_bound || cell.admits_plantation_worker_path(colony_slot, ware))
                    && cell.source_production_kind_code == 9,
            );
        }
        let cells: Vec<((i32, i32), bool)> = authored_resources
            .iter()
            .filter(|(slot, _, _)| *slot == ware)
            .filter_map(|&(_, _, p)| match live.get(&p) {
                Some(true) => Some((p, true)),
                Some(false) => None,
                None => Some((p, false)),
            })
            .collect();
        let mut best: Option<((usize, i32, usize), ((i32, i32), usize, usize))> = None;
        for y in 0..i32::from(island.height) - i32::from(h) {
            for x in 0..i32::from(island.width) - i32::from(w) {
                // A harvester is the one building allowed to stand in the
                // woods: it is sited *on* its resource, and the colony that
                // actually kept wood, food and cloth flowing at scale put its
                // foresters inside the tree line. Housing is still kept out —
                // that is what protects the woodland.
                let (dx, dy) = (x - anchor.0, y - anchor.1);
                if refused_sites.contains(&(def_index, (x, y)))
                    || dx * dx + dy * dy > 16 * 16
                    || !cart_can_reach(reachable, x, y, w, h)
                    || !admitted(sim, def_index, x, y)
                {
                    continue;
                }
                let (mut total, mut ripe) = (0usize, 0usize);
                for &((cx, cy), is_ripe) in &cells {
                    let (dx, dy) = (cx - x, cy - y);
                    if dx * dx + dy * dy <= radius * radius {
                        total += 1;
                        ripe += usize::from(is_ripe);
                    }
                }
                if ripe == 0 {
                    continue;
                }
                // Bucket the yield so near-equal grounds tie and the second
                // key decides: a grazer wants to stand next to the workshop
                // that eats its wool, because the city cart will otherwise
                // carry the wool off to the Kontor before the workshop's own
                // type-8 carrier gets there and the weaving hut starves beside
                // a full sheep farm.
                let near = focus
                    .iter()
                    .map(|&(fx, fy)| (x - fx).abs().max((y - fy).abs()))
                    .min()
                    .unwrap_or_else(|| (x - anchor.0).abs() + (y - anchor.1).abs());
                let key = (usize::MAX - total / 4, near, usize::MAX - ripe);
                if best.as_ref().is_none_or(|(bk, _)| key < *bk) {
                    best = Some((key, ((x, y), total, ripe)));
                }
            }
        }
        best.map(|(_, v)| v)
    };

    // Fence off the pasture a newly placed grazer depends on.
    let reserve_resource = |sim: &Sim,
                            pasture: &mut HashSet<(i32, i32)>,
                            def_index: usize,
                            at: (i32, i32),
                            ware: u8| {
        let radius = i32::from(defs[def_index].radius).max(1);
        for cell in sim
            .source_static_map_roots
            .iter()
            .filter(|c| c.island == 10 && c.source_output_ware_slot == ware)
        {
            let (dx, dy) = (i32::from(cell.x) - at.0, i32::from(cell.y) - at.1);
            if dx * dx + dy * dy <= radius * radius {
                pasture.insert((i32::from(cell.x), i32::from(cell.y)));
            }
        }
    };

    #[allow(clippy::too_many_arguments)]
    let build_harvester = |sim: &mut Sim,
                           tally: &mut Tally,
                           refused_sites: &mut HashSet<(usize, (i32, i32))>,
                           pasture: &mut HashSet<(i32, i32)>,
                           walls: &mut HashSet<(i32, i32)>,
                           served: &mut Vec<(i32, i32, u8, u8)>,
                           reachable: &mut HashSet<(i32, i32)>,
                           placed_at: &mut Vec<(i32, i32)>,
                           def_index: usize,
                           ware: u8,
                           focus: &[(i32, i32)],
                           label: &str|
     -> bool {
        let Some(((x, y), total, ripe)) =
            harvester_site(sim, refused_sites, reachable, def_index, ware, focus)
        else {
            println!("no {label} site with ware {ware} in range");
            return false;
        };
        println!("  {label} site ({x},{y}): {total} ware-{ware} cells in range, {ripe} ripe");
        let placed = build_at(
            sim,
            tally,
            refused_sites,
            walls,
            served,
            reachable,
            true,
            def_index,
            x,
            y,
            label,
        );
        if placed {
            placed_at.push((x, y));
            if ware == GRAS {
                reserve_resource(sim, pasture, def_index, (x, y), ware);
            }
        }
        placed
    };

    // A workshop is not sited for coverage but for its **suppliers**. The
    // type-8 workshop carrier fetches the input ware itself, and it is racing
    // the type-11 city cart, which will haul the same wool off to the Kontor;
    // put the weaving hut far from its sheep and the farms fill to their cap
    // while the weaver sits at raw = 0. Take the admitted, cart-reachable site
    // closest to the farms it eats from.
    #[allow(clippy::too_many_arguments)]
    let workshop_site = |sim: &Sim,
                         refused_sites: &HashSet<(usize, (i32, i32))>,
                         pasture: &HashSet<(i32, i32)>,
                         reachable: &HashSet<(i32, i32)>,
                         def_index: usize,
                         suppliers: &[(i32, i32)]|
     -> Option<(i32, i32)> {
        let def = &defs[def_index];
        let (w, h) = (def.width, def.height);
        let mut best: Option<((i32, i32, usize), (i32, i32))> = None;
        for y in 0..i32::from(island.height) - i32::from(h) {
            for x in 0..i32::from(island.width) - i32::from(w) {
                if refused_sites.contains(&(def_index, (x, y)))
                    || !unforested(sim, def_index, x, y)
                    || !cart_can_reach(reachable, x, y, w, h)
                    || !admitted(sim, def_index, x, y)
                {
                    continue;
                }
                let far = suppliers
                    .iter()
                    .map(|&(sx, sy)| (x - sx).abs().max((y - sy).abs()))
                    .max()
                    .unwrap_or(0);
                let sum: i32 = suppliers
                    .iter()
                    .map(|&(sx, sy)| (x - sx).abs() + (y - sy).abs())
                    .sum();
                let grazed = (0..i32::from(h))
                    .flat_map(|dy| (0..i32::from(w)).map(move |dx| (dx, dy)))
                    .filter(|&(dx, dy)| pasture.contains(&(x + dx, y + dy)))
                    .count();
                let key = (far, sum, grazed);
                if best.as_ref().is_none_or(|(bk, _)| key < *bk) {
                    best = Some((key, (x, y)));
                }
            }
        }
        best.map(|(_, p)| p)
    };

    // What does the engine say about the colony's houses? `state_bits & 0x80`
    // is marketplace coverage, `lifecycle_flags & 0x000c` is chapel-or-church,
    // and `source_transition_active_for_group(1)` is the composite that decides
    // whether a pioneer hut can ever become a settler house — 2 residents
    // against 6. Read the verdict; do not recompute it.
    let stuck_houses = |sim: &Sim| -> (Vec<(i32, i32)>, Vec<(i32, i32)>) {
        let (mut no_chapel, mut no_market) = (Vec::new(), Vec::new());
        for loc in sim.source_kind13_locations.active_locations() {
            if loc.island_id != 10 || loc.population_group != 0 {
                continue;
            }
            if loc.source_transition_active_for_group(1) {
                continue;
            }
            let p = (i32::from(loc.tile_x), i32::from(loc.tile_y));
            if loc.state_bits & 0x80 == 0 {
                no_market.push(p);
            } else {
                no_chapel.push(p);
            }
        }
        (no_chapel, no_market)
    };

    // ------------------------------------------------------------------
    // Stage 1b: the opening core.
    // ------------------------------------------------------------------
    // Bounded by the ship's 50 wood and 60 tools — the colony has no tool
    // smith at tier 0, so those 60 tools are the entire construction budget of
    // the run and every rung below is priced in them. The forester goes up
    // first: it is the only wood source and costs no wood.
    let mut forester_sites: Vec<(i32, i32)> = Vec::new();
    let mut fishery_sites: Vec<(i32, i32)> = Vec::new();
    let mut sheep_sites: Vec<(i32, i32)> = Vec::new();
    build_harvester(
        &mut sim,
        &mut tally,
        &mut refused_sites,
        &mut pasture,
        &mut walls,
        &mut served,
        &mut reachable,
        &mut forester_sites,
        FORESTER,
        BAUM,
        &[],
        "forester0",
    );
    build_harvester(
        &mut sim,
        &mut tally,
        &mut refused_sites,
        &mut pasture,
        &mut walls,
        &mut served,
        &mut reachable,
        &mut fishery_sites,
        FISHERY,
        FISCHE,
        &[],
        "fishery0",
    );
    // The first marketplace is sited for *room*, not convenience: it takes the
    // spot whose radius-16 claim buys the most buildable ground the colony does
    // not own yet. The Kontor's own claim is only 158 hut-legal tiles; the whole
    // rest of the island is out of reach until a MARKT pays for it.
    {
        let (_, outside) = survey_house_anchors(&sim, &refused_sites, &plot);
        if let Some((x, y)) = civic_site(
            &sim,
            &refused_sites,
            &pasture,
            &reachable,
            &plot,
            MARKET,
            &market_rows,
            &outside,
            true,
        ) {
            if build_at(
                &mut sim,
                &mut tally,
                &mut refused_sites,
                &mut walls,
                &mut served,
                &mut reachable,
                true,
                MARKET,
                x,
                y,
                "market0",
            ) {
                markets.push(centre_of(x, y, defs[MARKET].width, defs[MARKET].height));
            }
        }
    }
    // The chapel goes where it covers the most ground the colony can actually
    // house people on: without it every hut is capped at two residents.
    {
        let (inside, _) = survey_house_anchors(&sim, &refused_sites, &plot);
        let covered: Vec<(i32, i32)> = inside
            .iter()
            .copied()
            .filter(|&(x, y)| {
                let c = centre_of(x, y, 2, 2);
                markets.iter().any(|&m| in_disc(&market_rows, m, c))
            })
            .collect();
        if let Some((x, y)) = civic_site(
            &sim,
            &refused_sites,
            &pasture,
            &reachable,
            &plot,
            CHAPEL,
            &chapel_rows,
            &covered,
            false,
        ) {
            if build_at(
                &mut sim,
                &mut tally,
                &mut refused_sites,
                &mut walls,
                &mut served,
                &mut reachable,
                false,
                CHAPEL,
                x,
                y,
                "chapel0",
            ) {
                chapels.push(centre_of(x, y, defs[CHAPEL].width, defs[CHAPEL].height));
            }
        }
    }
    let mut huts = 0usize;
    for _ in 0..4 {
        let (inside, _) = survey_house_anchors(&sim, &refused_sites, &plot);
        let Some((x, y)) = house_site(&inside, &markets, &chapels, &pasture) else {
            println!("no house anchor for hut{huts}");
            break;
        };
        if build_at(
            &mut sim,
            &mut tally,
            &mut refused_sites,
            &mut walls,
            &mut served,
            &mut reachable,
            false,
            HUT,
            x,
            y,
            &format!("hut{huts}"),
        ) {
            huts += 1;
        }
    }
    // Cloth needs no fertility and no rung: two sheep farms feed one weaver
    // (the hut consumes 2 Wool per Cloth). The farms graze ripe GRAS cells.
    let mut sheep = 0usize;
    let mut weavers = 0usize;
    let mut weaver_sites: Vec<(i32, i32)> = Vec::new();
    for _ in 0..2 {
        let focus = sheep_sites.clone();
        if build_harvester(
            &mut sim,
            &mut tally,
            &mut refused_sites,
            &mut pasture,
            &mut walls,
            &mut served,
            &mut reachable,
            &mut sheep_sites,
            SHEEP,
            GRAS,
            &focus,
            &format!("sheep{sheep}"),
        ) {
            sheep += 1;
        }
    }
    if let Some((x, y)) = workshop_site(&sim, &refused_sites, &pasture, &reachable, WEAVER, &sheep_sites) {
        if build_at(
            &mut sim,
            &mut tally,
            &mut refused_sites,
            &mut walls,
            &mut served,
            &mut reachable,
            true,
            WEAVER,
            x,
            y,
            "weaver0",
        ) {
            weavers += 1;
            weaver_sites.push((x, y));
        }
    }

    // Hold back a plot for the next marketplace before the housing starts.
    let reserve_next_plot = |sim: &Sim,
                             refused_sites: &HashSet<(usize, (i32, i32))>,
                             pasture: &HashSet<(i32, i32)>,
                             reachable: &HashSet<(i32, i32)>,
                             plot: &mut HashSet<(i32, i32)>| {
        plot.clear();
        let (_, outside) = survey_house_anchors(sim, refused_sites, plot);
        if outside.is_empty() {
            return;
        }
        let Some((x, y)) = civic_site(
            sim,
            refused_sites,
            pasture,
            reachable,
            plot,
            MARKET,
            &market_rows,
            &outside,
            true,
        ) else {
            return;
        };
        for dy in 0..i32::from(defs[MARKET].height) {
            for dx in 0..i32::from(defs[MARKET].width) {
                plot.insert((x + dx, y + dy));
            }
        }
        println!("  next marketplace plot reserved at ({x},{y})");
    };
    reserve_next_plot(&sim, &refused_sites, &pasture, &reachable, &mut plot);

    // ------------------------------------------------------------------
    // Reporting.
    // ------------------------------------------------------------------
    let cell_report = |sim: &Sim| {
        for cell in sim
            .source_map_cell_states
            .iter()
            .filter(|c| c.island == 10 && (1..=8).contains(&c.source_production_kind_code))
        {
            println!(
                "  cell ({},{}) prodkind={} act={} raw={} work={} fill={} cap={} sched={} blocked={}",
                cell.x,
                cell.y,
                cell.source_production_kind_code,
                cell.activity,
                cell.raw_material_stock,
                cell.work_material_stock,
                cell.storage_fill,
                cell.storage_animation_capacity,
                cell.scheduler_enabled,
                cell.scheduler_blocked,
            );
        }
        let mut routes: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for figure in sim
            .figures
            .iter()
            .filter(|f| f.is_active() && f.origin_island == 10)
        {
            *routes
                .entry(format!(
                    "{:?}/good{}",
                    figure.source_worker_route, figure.carried_good
                ))
                .or_default() += 1;
        }
        println!("  worker figures on island 10: {routes:?}");
    };

    let report = |sim: &Sim, label: String| {
        let city = sim
            .source_cities
            .active_records()
            .into_iter()
            .find(|c| c.island_id == 10 && c.owner_slot == 0)
            .unwrap();
        let total: u32 = city.tier_population.iter().sum();
        println!(
            "{label}: pop={total} {:?} sat={:?} overall={} feed={} food={} cloth={} wood={} tools={} resv={:?} gold={} unlock={:#x}",
            city.tier_population,
            city.satisfaction_by_group,
            city.overall_satisfaction,
            city.food_fulfillment,
            stock(sim, Good::Food),
            stock(sim, Good::Cloth),
            stock(sim, Good::Wood),
            stock(sim, Good::Tools),
            city.promotion_reservations,
            sim.players[0].gold,
            sim.players[0].unlock_mask,
        );
        total
    };

    let run_minute = |sim: &mut Sim| {
        for _ in 0..600 {
            sim.tick(100);
            sim.drain_source_kind13_replacements(&cod);
        }
    };

    for minute in 1..=10u32 {
        run_minute(&mut sim);
        report(&sim, format!("min {minute}"));
    }
    cell_report(&sim);

    // ------------------------------------------------------------------
    // Stage 2+: paced expansion.
    // ------------------------------------------------------------------
    // One placement a minute, in a fixed priority order. Food first, then
    // cloth, then the infrastructure that lets a hut hold six people instead of
    // two, then housing. Building out in one burst starved the colony at 46
    // inhabitants: a harvest cell is finite, and a fishery's sustained yield is
    // its cells divided by the 450 s a sea cell takes to ripen, so housing has
    // to wait on the food line and not the other way round.
    println!("--- paced expansion ---");
    let mut foresters = 1usize;
    let mut fisheries = 1usize;
    // A production line takes minutes to come online, so a bare "stock is low"
    // test fires again on the very next minute and again on the one after that:
    // an earlier version of this loop answered one cloth shortage with six sheep
    // farms in six minutes, spent half the tool budget, and laid no houses at
    // all. Build a line, then wait and watch it, the way a player does.
    let (mut last_forester, mut last_fishery, mut last_cloth) = (0u32, 0u32, 0u32);
    for minute in 11..=120u32 {
        run_minute(&mut sim);
        let pop = report(&sim, format!("min {minute}"));
        let (food, cloth, wood, tools) = (
            stock(&sim, Good::Food),
            stock(&sim, Good::Cloth),
            stock(&sim, Good::Wood),
            stock(&sim, Good::Tools),
        );
        let settlers = sim
            .source_cities
            .active_records()
            .into_iter()
            .find(|c| c.island_id == 10 && c.owner_slot == 0)
            .map(|c| c.tier_population[1])
            .unwrap_or(0);

        // The ship still holds the tools that would not fit under the
        // warehouse's 50-per-good cap at founding — a sixth of the run's entire
        // construction budget. Try to land them once. `UnloadShip`'s adjacency
        // test compares the ship's *world* position against the warehouse's
        // *island-local* tile (`Simulation::apply_command`, the `dx > 2 ||
        // dy > 2` arm), so for any island not sitting at the world origin it
        // can never be satisfied: this is expected to fail, and the log line
        // records it as an engine gap rather than as a driver bug.
        if minute == 11 {
            let wh = sim
                .warehouses
                .iter()
                .position(|w| w.island_id == 10 && w.owner == 0)
                .unwrap_or(0) as u32;
            let aboard = sim.trade_ships[ship as usize].cargo_amount(Good::Tools);
            let landed = aboard > 0
                && sim.apply_command(&Command::UnloadShip {
                    player: 0,
                    ship_idx: ship,
                    warehouse_idx: wh,
                    good: Good::Tools,
                    qty: aboard,
                });
            println!(
                "  landing the Verena's remaining {aboard} tools: {landed}                  (ship at world ({},{}), warehouse tile ({},{}))",
                sim.trade_ships[ship as usize].world_x,
                sim.trade_ships[ship as usize].world_y,
                sim.warehouses[wh as usize].tile_x,
                sim.warehouses[wh as usize].tile_y,
            );
        }

        // Each rung consumes the minute only if it actually placed something.
        // A rung that wants to build but has nowhere to put it must fall
        // through, or the colony spends the rest of the run re-proposing an
        // impossible fishery and never lays another house.
        let mut placed_this_minute = false;

        if !placed_this_minute
            && wood < 8
            && foresters < 4
            && minute >= last_forester + 8
            && tools >= defs[FORESTER].cost_tools
        {
            if build_harvester(
                &mut sim,
                &mut tally,
                &mut refused_sites,
                &mut pasture,
                &mut walls,
                &mut served,
                &mut reachable,
                &mut forester_sites,
                FORESTER,
                BAUM,
                &[],
                &format!("forester{foresters}"),
            ) {
                foresters += 1;
                placed_this_minute = true;
            }
            last_forester = minute;
        }
        // Food is the one shortage the colony must not build through. If the
        // larder is thin and the last fishery is still bedding in, spend the
        // minute on nothing at all rather than on another house: adding mouths
        // to a short food line is precisely how the colony reached 46
        // inhabitants and then died.
        // The larder has to be sized against the *population*, not against a
        // fixed number. A run that used a flat-ish threshold kept one fishery
        // for ninety inhabitants: the stock read comfortable at 50 while the
        // demand cycle was quietly under-serving the houses, and the colony
        // peaked at 96 and then shed residents back to 89. Half the population
        // in store is the buffer that holds every hut at its cap.
        let hungry = food < 30 + pop as u16 / 2;
        if !placed_this_minute
            && hungry
            && fisheries < 10
            && minute >= last_fishery + 6
            && tools >= defs[FISHERY].cost_tools
        {
            if build_harvester(
                &mut sim,
                &mut tally,
                &mut refused_sites,
                &mut pasture,
                &mut walls,
                &mut served,
                &mut reachable,
                &mut fishery_sites,
                FISHERY,
                FISCHE,
                &[],
                &format!("fishery{fisheries}"),
            ) {
                fisheries += 1;
                placed_this_minute = true;
            }
            last_fishery = minute;
        }
        // Still hungry after that rung: hold off on new housing. Everything
        // that does not add a mouth — a chapel, a marketplace that buys the
        // coastline the next fishery needs — is still worth the minute.
        let hold_housing = !placed_this_minute && hungry && fisheries < 10;
        // Settlers consume cloth; a single sheep-and-weaver line stops covering
        // the settler population it created. Keep the authored 2 wool : 1 cloth
        // ratio by alternating farms and weaving huts. `TOOL_RESERVE` keeps one
        // fishery's worth of tools back: the run's whole construction budget is
        // the 60 tools the Verena carried, and food outranks everything.
        const TOOL_RESERVE: u16 = 3;
        if !placed_this_minute
            && settlers > 0
            && cloth < 10 + (settlers as u16) / 4
            && minute >= last_cloth + 12
        {
            if sheep < weavers * 2 + 2
                && sheep < 8
                && tools >= defs[SHEEP].cost_tools + TOOL_RESERVE
            {
                // A new farm goes beside the weaving huts that eat its wool.
                let focus = weaver_sites.clone();
                if build_harvester(
                    &mut sim,
                    &mut tally,
                    &mut refused_sites,
                    &mut pasture,
                    &mut walls,
                    &mut served,
                    &mut reachable,
                    &mut sheep_sites,
                    SHEEP,
                    GRAS,
                    &focus,
                    &format!("sheep{sheep}"),
                ) {
                    sheep += 1;
                    placed_this_minute = true;
                }
                last_cloth = minute;
            } else if weavers < 4 && tools >= defs[WEAVER].cost_tools + TOOL_RESERVE {
                // The new weaver serves the farms no existing weaver is next
                // to: take the two most distant from any weaving hut.
                let mut orphans = sheep_sites.clone();
                orphans.sort_by_key(|&(sx, sy)| {
                    std::cmp::Reverse(
                        weaver_sites
                            .iter()
                            .map(|&(wx, wy)| (sx - wx).abs().max((sy - wy).abs()))
                            .min()
                            .unwrap_or(0),
                    )
                });
                orphans.truncate(2);
                if let Some((x, y)) =
                    workshop_site(&sim, &refused_sites, &pasture, &reachable, WEAVER, &orphans)
                {
                    if build_at(
                        &mut sim,
                        &mut tally,
                        &mut refused_sites,
                        &mut walls,
                        &mut served,
                        &mut reachable,
                        true,
                        WEAVER,
                        x,
                        y,
                        &format!("weaver{weavers}"),
                    ) {
                        weavers += 1;
                        weaver_sites.push((x, y));
                        placed_this_minute = true;
                    }
                }
                last_cloth = minute;
            }
        }

        // Ask the engine which houses are stalled and what they are missing,
        // then buy exactly that. A chapel is the cheapest population multiplier
        // the colony can reach at this tier: two tools turn every 2-resident
        // pioneer hut in its disc into a 6-resident settler house.
        let (no_chapel, no_market) = stuck_houses(&sim);
        if !placed_this_minute
            && no_chapel.len() >= 5
            && chapels.len() < 8
            && tools >= defs[CHAPEL].cost_tools + TOOL_RESERVE
        {
            if let Some((x, y)) = civic_site(
                &sim,
                &refused_sites,
                &pasture,
                &reachable,
                &plot,
                CHAPEL,
                &chapel_rows,
                &no_chapel,
                false,
            ) {
                println!("  {} houses stalled for want of a chapel", no_chapel.len());
                if build_at(
                    &mut sim,
                    &mut tally,
                    &mut refused_sites,
                    &mut walls,
                    &mut served,
                    &mut reachable,
                    false,
                    CHAPEL,
                    x,
                    y,
                    &format!("chapel{}", chapels.len()),
                ) {
                    chapels.push(centre_of(x, y, defs[CHAPEL].width, defs[CHAPEL].height));
                    placed_this_minute = true;
                }
            }
        }
        if !placed_this_minute
            && no_market.len() >= 3
            && markets.len() < 6
            && tools >= defs[MARKET].cost_tools + TOOL_RESERVE
        {
            if let Some((x, y)) = civic_site(
                &sim,
                &refused_sites,
                &pasture,
                &reachable,
                &plot,
                MARKET,
                &market_rows,
                &no_market,
                true,
            ) {
                println!(
                    "  {} houses stalled for want of a marketplace",
                    no_market.len()
                );
                if build_at(
                    &mut sim,
                    &mut tally,
                    &mut refused_sites,
                    &mut walls,
                    &mut served,
                    &mut reachable,
                    true,
                    MARKET,
                    x,
                    y,
                    &format!("market{}", markets.len()),
                ) {
                    markets.push(centre_of(x, y, defs[MARKET].width, defs[MARKET].height));
                    placed_this_minute = true;
                    reserve_next_plot(
                        &sim,
                        &refused_sites,
                        &pasture,
                        &reachable,
                        &mut plot,
                    );
                }
            }
        }

        // Housing. Only sites the engine already admits are ever proposed, so a
        // refusal here is news rather than routine.
        // Housing is *not* gated on the wood in store. Wood sits near zero for
        // most of the run because construction draws it the moment it lands
        // (`Simulation`'s material trickle takes one unit per tick per site) —
        // the colony that reached 70 inhabitants ran at wood = 1 for forty
        // minutes straight. A version that refused to lay a hut until the
        // warehouse held its three wood simply never laid another hut. One
        // placement a minute is the pacing; the warehouse is not the brake.
        if !placed_this_minute {
            let (inside, outside) = survey_house_anchors(&sim, &refused_sites, &plot);
            if let Some((x, y)) =
                house_site(&inside, &markets, &chapels, &pasture).filter(|_| !hold_housing)
            {
                if build_at(
                    &mut sim,
                    &mut tally,
                    &mut refused_sites,
                    &mut walls,
                    &mut served,
                    &mut reachable,
                    false,
                    HUT,
                    x,
                    y,
                    &format!("hut{huts}"),
                ) {
                    huts += 1;
                    placed_this_minute = true;
                }
            }
            // Out of room inside the claim. A marketplace claims every
            // still-unowned land cell in its own radius (`FUN_0046ac60`), so it
            // is how a colony buys ground — site it where it buys the most.
            if !placed_this_minute
                && !outside.is_empty()
                && markets.len() < 6
                && tools >= defs[MARKET].cost_tools
            {
                if let Some((x, y)) = civic_site(
                    &sim,
                    &refused_sites,
                    &pasture,
                    &reachable,
                    &plot,
                    MARKET,
                    &market_rows,
                    &outside,
                    true,
                ) {
                    println!(
                        "  claim is full ({} anchors left outside it); buying ground",
                        outside.len()
                    );
                    if build_at(
                        &mut sim,
                        &mut tally,
                        &mut refused_sites,
                        &mut walls,
                        &mut served,
                        &mut reachable,
                        true,
                        MARKET,
                        x,
                        y,
                        &format!("market{}", markets.len()),
                    ) {
                        markets.push(centre_of(x, y, defs[MARKET].width, defs[MARKET].height));
                        placed_this_minute = true;
                        reserve_next_plot(&sim, &refused_sites, &pasture, &reachable, &mut plot);
                    }
                }
            }
        }
        let _ = placed_this_minute;
        if minute % 30 == 0 {
            cell_report(&sim);
        }
    }

    cell_report(&sim);
    let (no_chapel, no_market) = stuck_houses(&sim);
    println!(
        "placements: {} accepted, {} refused; {} huts, {} markets at {:?}, {} chapels at {:?}, \
         {} foresters, {} fisheries, {} sheep farms, {} weavers",
        tally.accepted,
        tally.refused,
        huts,
        markets.len(),
        markets,
        chapels.len(),
        chapels,
        foresters,
        fisheries,
        sheep,
        weavers,
    );
    println!(
        "houses still stalled: {} without a chapel, {} without a market",
        no_chapel.len(),
        no_market.len()
    );
    println!(
        "objectives={:?}",
        sim.objectives
            .items
            .iter()
            .map(|(o, done)| (o.label(), *done))
            .collect::<Vec<_>>()
    );
}
