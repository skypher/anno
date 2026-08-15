//! The buildable-area gate: `FUN_004084d0` (`1602_exe.c:7612-7616` and
//! `:7662-7665`), the half of the per-tile build verdict the terrain table
//! does not answer. The corpus half self-skips without `extracted/`.
//!
//! ```text
//! if (slot == 7) {                                  // unowned ground
//!     if (((player_kind != 0 && player_kind != 0x0c)
//!          || FUN_0046b120(island, player) == 0)
//!         && FUN_0046b100(island) < 7)
//!         goto ACCEPT;
//!     buildable = 0;                                // REJECT
//! } else if (foreign == 0 && def[4] == 0x23) {
//!     buildable = 0;                                // no second HQ in your claim
//! }
//! ```
//!
//! Until this existed the port refused no placement on ownership grounds at
//! all: a player could build anywhere on any island, and the scripted mission
//! driver had to guess where a colony "should" expand instead of asking the
//! game. The rule is what makes a settlement a *place*: you found on ground
//! nobody owns, and from then on you may only build on ground your own
//! settlement has claimed — which is why the marketplace, whose
//! `FUN_00465170` kind-7 branch runs `FUN_0046ac60` over its own `Radius`, is
//! the mechanism a colony grows by.
//!
//! `docs/logistics-gaps.md` §6.

use anno_game::game_commands::PlaceOutcome;
use anno_sim::island_map::IslandMap;
use anno_sim::player::{Player, PlayerState};
use anno_sim::simulation::Simulation;

/// `(tile >> 0x13) & 7 == 7`: this ground belongs to no settlement — and the
/// same value `FUN_004084d0` stamps when the resolved settlement is somebody
/// else's (`LAB_00408622`).
const UNSETTLED_SLOT: u8 = 7;
/// Outer `HAUS Kind: GEBAEUDE`, the ordinary workshop/residence kind.
const GEBAEUDE: u8 = 14;
/// Outer `HAUS Kind: HQ` — `def[4] == 0x23`, the Kontor.
const HQ: u8 = 0x23;

// --------------------------------------------------------------------------
// The rule itself, on a synthetic island. No data corpus needed.
// --------------------------------------------------------------------------

const UNIT_ISLAND: u8 = 3;
const UNIT_EXTENT: u16 = 48;

/// One 48x48 island of plain `BODEN` whose every tile carries the unsettled
/// selector, plus `players` player slots. [`IslandMap::new_open`] leaves the
/// selector at 0, which would read as "settlement slot 0 owns the whole
/// island" the moment a record for that slot exists, so it is cleared to 7
/// here — the state a stock island actually ships in.
fn unit_sim(players: &[PlayerState]) -> Simulation {
    let mut sim = Simulation::new();
    for (index, state) in players.iter().enumerate() {
        let mut player = Player::new_human(index as u8);
        player.state = *state;
        sim.players.push(player);
    }
    let mut map = IslandMap::new_open(UNIT_ISLAND, UNIT_EXTENT, UNIT_EXTENT);
    for y in 0..i32::from(UNIT_EXTENT) {
        for x in 0..i32::from(UNIT_EXTENT) {
            map.set_source_map_owner(x, y, UNSETTLED_SLOT);
        }
    }
    sim.island_maps.push(map);
    sim
}

/// Found `player` a settlement at `(x, y)` and let it claim `radius` around
/// itself, the way `FUN_00468ce0` + `FUN_00465170`'s kind-8 branch do.
/// Returns its island-local slot.
fn settle(sim: &mut Simulation, x: u8, y: u8, player: u8, radius: u8) -> u8 {
    let slot = sim
        .source_cities
        .allocate_source_city(UNIT_ISLAND, x, y, player, 0)
        .and_then(|physical| sim.source_cities.record(physical))
        .expect("a free city slot")
        .source_owner;
    if radius > 0 {
        sim.claim_source_settlement_area(
            UNIT_ISLAND,
            i32::from(x),
            i32::from(y),
            1,
            1,
            radius,
            slot,
        );
    }
    slot
}

fn slot_at(sim: &Simulation, x: i32, y: i32, player: u8) -> u8 {
    sim.source_placement_settlement_slot(UNIT_ISLAND, x, y, 1, 1, player)
}

/// The whole point of the gate. A player who holds no settlement on the
/// island may build on ground nobody owns — that is how a colony is founded.
/// Once they hold one, that same ground is closed to them and only their own
/// claim is open.
#[test]
fn unowned_ground_is_open_until_you_settle_and_closed_afterwards() {
    let mut sim = unit_sim(&[PlayerState::HumanActive]);

    // Before founding: every tile reports the unsettled selector, and
    // `FUN_0046b120` reports zero settlements, so `:7613-7615` accepts.
    assert_eq!(slot_at(&sim, 10, 10, 0), UNSETTLED_SLOT);
    assert!(sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 0, GEBAEUDE));

    let slot = settle(&mut sim, 10, 10, 0, 8);
    assert_ne!(slot, UNSETTLED_SLOT);

    // Inside the claim the footprint resolves to the settlement's own slot,
    // and `:7612` is not entered at all.
    assert_eq!(slot_at(&sim, 10, 10, 0), slot);
    assert!(sim.source_placement_area_admits(UNIT_ISLAND, slot, 0, GEBAEUDE));

    // Outside it the selector is still 7 — but `FUN_0046b120` now returns 1,
    // so the first disjunct of `:7613` fails and the tile is refused.
    assert_eq!(slot_at(&sim, 40, 40, 0), UNSETTLED_SLOT);
    assert_eq!(sim.source_island_settlement_count_for_player(UNIT_ISLAND, 0), 1);
    assert!(!sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 0, GEBAEUDE));
}

/// The disjunct is per *player*, not per island: a second player who has not
/// settled here yet is still free to found, on ground the first one has not
/// claimed. This is how two colonies end up sharing an island.
#[test]
fn another_players_settlement_does_not_close_the_island() {
    let mut sim = unit_sim(&[PlayerState::HumanActive, PlayerState::AiActive]);
    settle(&mut sim, 10, 10, 0, 8);

    // Player 1's footprint on player 0's claimed ground short-circuits
    // `FUN_0046aec0` to a foreign settlement, which `1602_exe.c:7529-7531`
    // collapses to selector 7 — and player 1 holds nothing here, so it is
    // admitted, exactly as unclaimed ground is.
    assert_eq!(slot_at(&sim, 10, 10, 1), UNSETTLED_SLOT);
    assert!(sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 1, GEBAEUDE));
    assert!(!sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 0, GEBAEUDE));

    settle(&mut sim, 40, 40, 1, 8);
    assert!(!sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 1, GEBAEUDE));
}

/// `FUN_0046b100(island) < 7`. The island's eight settlement pointers are
/// scanned by `FUN_00468ce0`, which returns 7 both as a valid allocation and
/// as its failure code, while 7 is simultaneously the "unowned" sentinel in
/// the map word — so this cap is what keeps slot 7 from ever being handed
/// out, and with it the port's `source_owner == 7` hazard closed.
#[test]
fn the_island_is_full_at_seven_settlements() {
    let mut sim = unit_sim(&[
        PlayerState::HumanActive,
        PlayerState::AiActive,
        PlayerState::AiActive,
    ]);
    // Six settlements, none of them player 0's or player 2's, leave both
    // admitted.
    for index in 0..6u8 {
        settle(&mut sim, 4 + index * 6, 4, 1, 0);
    }
    assert_eq!(sim.source_island_settlement_count(UNIT_ISLAND), 6);
    assert_eq!(sim.source_island_settlement_count_for_player(UNIT_ISLAND, 0), 0);
    assert!(sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 0, GEBAEUDE));
    assert!(sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 2, GEBAEUDE));

    // Player 0 takes the seventh and last. `allocate_source_city` scans slots
    // `0..8`, so this is the last allocation before the sentinel: the six
    // above hold 0..=5 and this one takes 6. That is exactly what the cap is
    // for — the eighth would be handed slot 7, which is simultaneously the
    // map word's "unowned" selector and `FUN_00468ce0`'s failure code.
    let slot = settle(&mut sim, 20, 20, 0, 8);
    assert_eq!(slot, 6);
    assert_eq!(sim.source_island_settlement_count(UNIT_ISLAND), 7);

    // The island is now closed to everyone who is not already on it.
    assert!(!sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 2, GEBAEUDE));
    // But the cap only guards the unowned-ground arm: player 0 keeps building
    // on the ground their own settlement claimed.
    assert!(sim.source_placement_area_admits(UNIT_ISLAND, slot, 0, GEBAEUDE));
    assert_eq!(slot_at(&sim, 20, 20, 0), slot);
    // And unowned ground is closed to them too, on both counts.
    assert!(!sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 0, GEBAEUDE));
}

/// `1602_exe.c:7662-7665`: inside your own claim, `def[4] == 0x23` is refused
/// and nothing else is. One Kontor per settlement.
#[test]
fn a_settlement_admits_no_second_hq() {
    let mut sim = unit_sim(&[PlayerState::HumanActive]);
    let slot = settle(&mut sim, 10, 10, 0, 8);

    assert!(!sim.source_placement_area_admits(UNIT_ISLAND, slot, 0, HQ));
    assert!(sim.source_placement_area_admits(UNIT_ISLAND, slot, 0, GEBAEUDE));

    // And the unowned-ground arm closes the other half of the same rule: a
    // player already settled here cannot plant a second Kontor beyond the
    // claim either. Together that is one Kontor per player per island.
    assert!(!sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 0, HQ));
}

/// `(player_kind != 0 && player_kind != 0x0c)` — the escape the pirates, the
/// free trader and the natives take. Their settlements are placed by the
/// engine, not by a build order, and the gate never stands in their way.
///
/// `PlayerState`'s discriminants are the source kind bytes, so this is a
/// value test: `AiAllied` is 0x0d, the free trader. The port additionally
/// parks the reserved factions on `Empty` (7) when a scenario authors them
/// (`anno_game::scenario`), which is not 0 or 0x0c either — same verdict.
#[test]
fn only_human_and_ai_players_are_held_to_the_settlement_rule() {
    for kind in [PlayerState::AiAllied, PlayerState::Empty, PlayerState::AiDefending] {
        let mut sim = unit_sim(&[PlayerState::HumanActive, kind]);
        settle(&mut sim, 10, 10, 1, 8);
        assert_eq!(sim.source_island_settlement_count_for_player(UNIT_ISLAND, 1), 1);
        assert!(
            sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 1, GEBAEUDE),
            "{kind:?} must not be held to the one-settlement rule",
        );
    }

    // The contrast: the same setup with a settling kind is refused.
    for kind in [PlayerState::HumanActive, PlayerState::AiActive] {
        let mut sim = unit_sim(&[PlayerState::HumanActive, kind]);
        settle(&mut sim, 10, 10, 1, 8);
        assert!(
            !sim.source_placement_area_admits(UNIT_ISLAND, UNSETTLED_SLOT, 1, GEBAEUDE),
            "{kind:?} is a settling faction and must be refused",
        );
    }
}

/// `FUN_0046b120`'s `param_2 == 7` sentinel counts every settlement held by a
/// *settling* faction, whatever its player slot.
#[test]
fn the_seven_sentinel_counts_every_settling_factions_settlement() {
    let mut sim = unit_sim(&[
        PlayerState::HumanActive,
        PlayerState::AiActive,
        PlayerState::AiAllied,
    ]);
    settle(&mut sim, 10, 10, 0, 0);
    settle(&mut sim, 20, 10, 1, 0);
    settle(&mut sim, 30, 10, 2, 0);

    assert_eq!(sim.source_island_settlement_count(UNIT_ISLAND), 3);
    assert_eq!(
        sim.source_island_settlement_count_for_player(UNIT_ISLAND, UNSETTLED_SLOT),
        2,
        "the free trader's settlement is not a settling faction's",
    );
}

// --------------------------------------------------------------------------
// The gate against the shipped campaign island.
// --------------------------------------------------------------------------

/// The island the campaign leaves unsettled, and the one mission 1 colonises.
const ISLAND: u8 = 10;
/// haeuser.cod `IDWOHN+4` — `Wohnhäuser I`, 2x2, `Kind: GEBAEUDE`.
const PIONEER_HOUSE_SOURCE_ID: i32 = 20605;
/// The founding Kontor, haeuser `@Nummer 271`.
const KONTOR_SOURCE_ID: i32 = 22103;

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

fn def_index(corpus: &Corpus, source_id: i32) -> usize {
    corpus
        .cod
        .buildings
        .iter()
        .position(|building| building.source_id == source_id)
        .unwrap_or_else(|| panic!("haeuser.cod ships source id {source_id}"))
}

struct Colony {
    sim: Simulation,
    island_index: usize,
    anchor: (i32, i32),
    /// The founded settlement's island-local slot.
    slot: u8,
    /// The campaign's starting ship, still unrouted after the founding.
    ship: u32,
}

/// Sail `colony`'s ship to `sea` and run the simulation until it is inside
/// `found_kontor`'s 12-unit range of `anchor`, which the original measures
/// against the anchor and not against the water cell.
fn dock_at(
    colony: &mut Colony,
    island: &anno_formats::szs::Island,
    sea: (i32, i32),
    anchor: (i32, i32),
) {
    assert!(
        colony
            .sim
            .apply_command(&anno_sim::commands::Command::SailShip {
                player: 0,
                ship_index: colony.ship,
                world_x: i32::from(island.x_pos) + sea.0,
                world_y: i32::from(island.y_pos) + sea.1,
            })
    );
    let anchor_world = (
        i32::from(island.x_pos) + anchor.0,
        i32::from(island.y_pos) + anchor.1,
    );
    let mut distance = i32::MAX;
    for _ in 0..3_000 {
        colony.sim.tick(100);
        let ship = &colony.sim.trade_ships[colony.ship as usize];
        distance = (ship.world_x - anchor_world.0).abs() + (ship.world_y - anchor_world.1).abs();
        if distance < 12 {
            break;
        }
    }
    assert!(
        distance < 12,
        "the ship must reach {anchor:?}'s dock range, or a refusal below would \
         be the ship gate and not the gate under test (distance {distance})",
    );
}

/// Found the campaign's starting ship on island 10, the same way
/// `source_placement_terrain.rs` does: a coastal anchor whose whole 2x3
/// footprint the Kontor's own terrain table admits, with open water beside it
/// for the ship to dock in.
fn found_island_ten(corpus: &Corpus) -> Colony {
    let sim = anno_game::scenario::build_simulation(
        &corpus.szs,
        &corpus.cod,
        &corpus.defs,
        &corpus.figures,
    );
    let island_index = corpus
        .szs
        .islands
        .iter()
        .position(|island| island.number == ISLAND)
        .expect("New Horizons0 ships island 10");
    let island = corpus.szs.islands[island_index].clone();
    let map_index = sim
        .island_maps
        .iter()
        .position(|map| map.island_id == ISLAND)
        .expect("island 10 has a map");
    let kontor = corpus.defs[def_index(corpus, KONTOR_SOURCE_ID)].clone();

    let ship = sim
        .trade_ships
        .iter()
        .position(|ship| ship.name == "Verena")
        .expect("the campaign's starting ship") as u32;
    let ship_start = {
        let ship = &sim.trade_ships[ship as usize];
        (ship.world_x, ship.world_y)
    };
    let sea_beside = |sim: &Simulation, anchor: (i32, i32)| {
        (-6_i32..=6)
            .flat_map(|dy| (-6_i32..=6).map(move |dx| (anchor.0 + dx, anchor.1 + dy)))
            .filter(|&(x, y)| {
                sim.island_maps[map_index]
                    .source_map_kind_and_owner(x, y)
                    .is_some_and(|(kind, _)| kind == 19)
            })
            .min_by_key(|&(x, y)| (x - anchor.0).abs() + (y - anchor.1).abs())
    };
    let (anchor, sea) = (2..i32::from(island.height) - i32::from(kontor.height) - 2)
        .flat_map(|y| {
            (2..i32::from(island.width) - i32::from(kontor.width) - 2).map(move |x| (x, y))
        })
        .filter(|&(x, y)| {
            sim.island_maps[map_index].is_coastal(x, y)
                && anno_game::game_commands::can_place_building(
                    &island,
                    &sim.island_maps[map_index],
                    &kontor,
                    x,
                    y,
                    kontor.width,
                    kontor.height,
                )
        })
        .filter_map(|anchor| sea_beside(&sim, anchor).map(|sea| (anchor, sea)))
        .min_by_key(|&(_, sea)| {
            (ship_start.0 - (i32::from(island.x_pos) + sea.0)).abs()
                + (ship_start.1 - (i32::from(island.y_pos) + sea.1)).abs()
        })
        .expect("island 10 has a buildable coastline with water beside it");

    let mut colony = Colony {
        sim,
        island_index,
        anchor,
        slot: UNSETTLED_SLOT,
        ship,
    };
    dock_at(&mut colony, &island, sea, anchor);
    let mut sim = colony.sim;
    assert!(
        anno_game::game_commands::apply_game_command(
            &mut sim,
            &corpus.szs.islands,
            &corpus.cod,
            &corpus.defs,
            &anno_sim::commands::Command::FoundKontor {
                player: 0,
                ship_index: ship,
                island: ISLAND,
                tile_x: anchor.0 as u16,
                tile_y: anchor.1 as u16,
            },
        ),
        "founding at {anchor:?} must succeed",
    );
    let slot = sim
        .source_cities
        .active_records()
        .into_iter()
        .find(|city| city.island_id == ISLAND && city.owner_slot == 0)
        .expect("the founded city record")
        .source_owner;
    // The build gates under test are not the gold gate or the Bauinfra gate.
    sim.players[0].gold = 1_000_000;
    sim.players[0].unlock_mask = u32::MAX;
    Colony {
        sim,
        island_index,
        anchor,
        slot,
        ship,
    }
}

/// Every tile of island 10 the settlement's claim currently covers.
fn claimed(colony: &Colony) -> std::collections::HashSet<(i32, i32)> {
    let map = colony
        .sim
        .island_maps
        .iter()
        .find(|map| map.island_id == ISLAND)
        .expect("island 10 map");
    (0..i32::from(map.height))
        .flat_map(|y| (0..i32::from(map.width)).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            map.source_map_kind_and_owner(x, y)
                .is_some_and(|(_, owner)| owner == colony.slot)
        })
        .collect()
}

/// Anchors where `def`'s footprint passes the terrain gate, split by whether
/// the claim covers the whole footprint or none of it. A footprint straddling
/// the boundary is deliberately left out of both: `FUN_0046aec0` votes, so one
/// claimed cell out of four is enough to carry the placement into the
/// settlement, and neither list would then say anything clean.
fn sites(
    colony: &Colony,
    corpus: &Corpus,
    def: &anno_sim::building::BuildingDef,
    claim: &std::collections::HashSet<(i32, i32)>,
) -> (Vec<(i32, i32)>, Vec<(i32, i32)>) {
    let island = &corpus.szs.islands[colony.island_index];
    let map = colony
        .sim
        .island_maps
        .iter()
        .find(|map| map.island_id == ISLAND)
        .expect("island 10 map");
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for y in 0..i32::from(island.height) {
        for x in 0..i32::from(island.width) {
            if !anno_game::game_commands::can_place_building(
                island, map, def, x, y, def.width, def.height,
            ) {
                continue;
            }
            let cells = (0..i32::from(def.height))
                .flat_map(|dy| (0..i32::from(def.width)).map(move |dx| (dx, dy)))
                .filter(|&(dx, dy)| claim.contains(&(x + dx, y + dy)))
                .count();
            if cells == usize::from(def.width) * usize::from(def.height) {
                inside.push((x, y));
            } else if cells == 0 {
                outside.push((x, y));
            }
        }
    }
    (inside, outside)
}

fn place(colony: &mut Colony, corpus: &Corpus, def_idx: usize, at: (i32, i32)) -> PlaceOutcome {
    anno_game::game_commands::place_building(
        &mut colony.sim,
        &corpus.szs.islands,
        colony.island_index,
        &corpus.defs,
        &corpus.cod,
        def_idx,
        0,
        0,
        at.0,
        at.1,
    )
}

/// The gate end to end on the shipped island: found, and from then on only
/// the claim is buildable — until a marketplace extends it.
#[test]
fn the_claim_is_the_buildable_area_and_a_marketplace_grows_it() {
    let Some(corpus) = load_corpus() else { return };
    let mut colony = found_island_ten(&corpus);
    let house_idx = def_index(&corpus, PIONEER_HOUSE_SOURCE_ID);
    let house = corpus.defs[house_idx].clone();
    assert_eq!(house.source_kind_code(), Some(GEBAEUDE));
    assert_eq!((house.width, house.height), (2, 2));

    let claim = claimed(&colony);
    assert!(
        !claim.is_empty(),
        "the founded Kontor claims its `max(Radius, 8)` disc",
    );
    let (inside, outside) = sites(&colony, &corpus, &house, &claim);
    assert!(!inside.is_empty() && !outside.is_empty());

    // Far outside the claim — the far end of the island — is refused, and
    // refused without cost or trace: `FUN_004084d0` skips the tile.
    let far = *outside
        .iter()
        .max_by_key(|&&(x, y)| (x - colony.anchor.0).abs() + (y - colony.anchor.1).abs())
        .expect("island 10 is larger than the Kontor's claim");
    let buildings_before = colony.sim.buildings.len();
    let gold_before = colony.sim.players[0].gold;
    assert!(
        matches!(
            place(&mut colony, &corpus, house_idx, far),
            PlaceOutcome::OutsideSettlement
        ),
        "a house at {far:?}, outside the claim, must be refused",
    );
    assert_eq!(colony.sim.buildings.len(), buildings_before);
    assert_eq!(colony.sim.players[0].gold, gold_before);

    // The same house inside the claim is accepted.
    let within = *inside
        .iter()
        .min_by_key(|&&(x, y)| (x - colony.anchor.0).abs() + (y - colony.anchor.1).abs())
        .expect("open ground inside the claim");
    assert!(
        matches!(
            place(&mut colony, &corpus, house_idx, within),
            PlaceOutcome::Placed
        ),
        "a house at {within:?}, inside the claim, must be accepted",
    );

    // Now the growth mechanism. Pick the nearest refused tile to the claim,
    // confirm it is refused, then put a marketplace inside the claim beside
    // it: `FUN_00465170`'s kind-7 branch (`1602_exe.c:70652-70666`) runs
    // `FUN_0046ac60` over the market's own `Radius`, which converts every
    // still-unowned land cell in the disc to the settlement.
    let claim = claimed(&colony);
    let (_, outside) = sites(&colony, &corpus, &house, &claim);
    let target = *outside
        .iter()
        .min_by_key(|&&(x, y)| {
            claim
                .iter()
                .map(|&(cx, cy)| (cx - x).abs().max((cy - y).abs()))
                .min()
                .unwrap_or(i32::MAX)
        })
        .expect("open ground just past the claim boundary");
    assert!(
        matches!(
            place(&mut colony, &corpus, house_idx, target),
            PlaceOutcome::OutsideSettlement
        ),
        "the house at {target:?} must be refused before the market exists",
    );

    let market_idx = corpus
        .defs
        .iter()
        .position(|def| def.prod_kind == "MARKT")
        .expect("haeuser.cod ships a marketplace");
    let market = corpus.defs[market_idx].clone();
    assert_eq!(
        corpus.cod.buildings[market_idx].source_production_kind_code(),
        Some(7),
        "the market's nested `HAUS_PRODTYP Kind` is what `FUN_00465170` claims on",
    );
    let (market_sites, _) = sites(&colony, &corpus, &market, &claim);
    let market_at = *market_sites
        .iter()
        .min_by_key(|&&(x, y)| (x - target.0).abs().max((y - target.1).abs()))
        .expect("room for a marketplace inside the claim");
    assert!(
        matches!(
            place(&mut colony, &corpus, market_idx, market_at),
            PlaceOutcome::Placed
        ),
        "the marketplace at {market_at:?} is inside the claim and must be accepted",
    );

    let grown = claimed(&colony);
    assert!(
        grown.len() > claim.len(),
        "the marketplace must extend the settlement's claim",
    );
    assert!(
        grown.contains(&target),
        "the market at {market_at:?} must claim {target:?}",
    );
    assert!(
        matches!(
            place(&mut colony, &corpus, house_idx, target),
            PlaceOutcome::Placed
        ),
        "the house at {target:?} must be accepted once the market claims it",
    );
}

/// `1602_exe.c:7662-7665` on the shipped Kontor definition: once a player has
/// founded here, no second Kontor. There is no separate found-Kontor command
/// in the original — founding runs the same `FUN_004084d0` gate — so the
/// port's `found_kontor` is held to it too.
#[test]
fn a_founded_island_refuses_the_same_players_second_kontor() {
    let Some(corpus) = load_corpus() else { return };
    let mut colony = found_island_ten(&corpus);
    let kontor_idx = def_index(&corpus, KONTOR_SOURCE_ID);
    let kontor = corpus.defs[kontor_idx].clone();
    assert_eq!(kontor.source_kind_code(), Some(HQ));

    let island = corpus.szs.islands[colony.island_index].clone();
    let map_index = colony
        .sim
        .island_maps
        .iter()
        .position(|map| map.island_id == ISLAND)
        .expect("island 10 map");
    let ship = colony.ship;

    // A second coastal anchor whose whole footprint the Kontor's own terrain
    // table still admits, with water beside it for the ship — so that when
    // the founding below is refused, the only gate left to have refused it is
    // the buildable-area one.
    let (second, sea) = (2..i32::from(island.height) - i32::from(kontor.height) - 2)
        .flat_map(|y| {
            (2..i32::from(island.width) - i32::from(kontor.width) - 2).map(move |x| (x, y))
        })
        .filter(|&(x, y)| {
            colony.sim.island_maps[map_index].is_coastal(x, y)
                && anno_game::game_commands::can_place_building(
                    &island,
                    &colony.sim.island_maps[map_index],
                    &kontor,
                    x,
                    y,
                    kontor.width,
                    kontor.height,
                )
        })
        .filter_map(|anchor| {
            (-6_i32..=6)
                .flat_map(|dy| (-6_i32..=6).map(move |dx| (anchor.0 + dx, anchor.1 + dy)))
                .filter(|&(x, y)| {
                    colony.sim.island_maps[map_index]
                        .source_map_kind_and_owner(x, y)
                        .is_some_and(|(kind, _)| kind == 19)
                })
                .min_by_key(|&(x, y)| (x - anchor.0).abs() + (y - anchor.1).abs())
                .map(|sea| (anchor, sea))
        })
        .min_by_key(|&(anchor, _)| {
            (anchor.0 - colony.anchor.0).abs() + (anchor.1 - colony.anchor.1).abs()
        })
        .expect("a second coastal anchor on island 10 with water beside it");
    dock_at(&mut colony, &island, sea, second);

    assert!(
        !anno_game::game_commands::placement_area_admits(
            &colony.sim,
            ISLAND,
            &kontor,
            second.0,
            second.1,
            kontor.width,
            kontor.height,
            0,
        ),
        "a second Kontor at {second:?} must be refused",
    );
    let cities_before = colony.sim.source_cities.active_records().len();
    assert!(
        !anno_game::game_commands::apply_game_command(
            &mut colony.sim,
            &corpus.szs.islands,
            &corpus.cod,
            &corpus.defs,
            &anno_sim::commands::Command::FoundKontor {
                player: 0,
                ship_index: ship,
                island: ISLAND,
                tile_x: second.0 as u16,
                tile_y: second.1 as u16,
            },
        ),
        "founding a second colony at {second:?} must be refused",
    );
    assert_eq!(
        colony.sim.source_cities.active_records().len(),
        cities_before,
        "a refused founding must allocate no settlement",
    );

    // A rival, who holds nothing here, is still free to found on the same
    // island — the rule is per player, not per island.
    colony.sim.players[1].state = PlayerState::AiActive;
    assert!(
        anno_game::game_commands::placement_area_admits(
            &colony.sim,
            ISLAND,
            &kontor,
            second.0,
            second.1,
            kontor.width,
            kontor.height,
            1,
        ),
        "player 1 holds no settlement on island 10 and must be admitted",
    );
}
