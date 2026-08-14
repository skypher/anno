//! Building placement against the per-player unlock bitmask
//! (`player + 0x6c`, `DAT_005b76ec`) the shipped New Horizons0
//! scenario authors. Self-skips without the data corpus.

use anno_formats::szs::{Island, SzsFile};
use anno_game::game_commands::{PlaceOutcome, can_place_building, missing_required_fertility};
use anno_sim::building::BuildingDef;
use anno_sim::data_bridge::{BAUINFRA_LADDER, INFRA_NAMES, SourceCityRecord, SourceCityTable};
use anno_sim::simulation::Simulation;
use anno_sim::types::Good;

const MARKT: u32 = 1 << 0; // INFRA_MARKT
const KAPELLE: u32 = 1 << 1; // INFRA_KAPELLE
const STUFE_1A: u32 = 1 << 14; // INFRA_STUFE_1A (id 15)

struct Loaded {
    szs: SzsFile,
    cod: anno_formats::cod::CodFile,
    defs: Vec<BuildingDef>,
    sim: Simulation,
}

fn load() -> Option<Loaded> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let (Ok(szs_data), Ok(cod_data)) = (
        std::fs::read(root.join("extracted/Szenes/New Horizons0.szs")),
        std::fs::read(root.join("extracted/haeuser.cod")),
    ) else {
        println!("Skipping test: data corpus not found");
        return None;
    };
    let mut szs = SzsFile::parse(&szs_data).unwrap();
    anno_game::scenario::instantiate_stock_islands(&mut szs, &root.join("extracted"), 1);
    let cod = anno_formats::cod::CodFile::parse(&cod_data).unwrap();
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = std::fs::read(root.join("extracted/figuren.cod"))
        .map(|bytes| anno_formats::figuren::FiguresFile::parse(&bytes))
        .unwrap_or_else(|_| anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: Vec::new(),
        });
    let sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    Some(Loaded {
        szs,
        cod,
        defs,
        sim,
    })
}

/// Locate a definition by the haeuser.cod fields that identify it,
/// so the test does not hard-code a table index.
fn def_index(defs: &[BuildingDef], pick: impl Fn(&BuildingDef) -> bool) -> usize {
    defs.iter()
        .position(pick)
        .expect("definition present in haeuser.cod")
}

/// First tile on `island` where the non-unlock placement gates
/// (footprint walkability, required fertility, fishery coastline) all
/// pass, so the only thing `place_building` can still object to is the
/// unlock mask.
fn free_tile(
    sim: &Simulation,
    island: &Island,
    def: &BuildingDef,
    skip: &[(i32, i32)],
) -> Option<(i32, i32)> {
    if missing_required_fertility(def, island).is_some() {
        return None;
    }
    let map = sim
        .island_maps
        .iter()
        .find(|m| m.island_id == island.number)?;
    let needs_coast = def.output_good == Good::Fish;
    for y in 0..island.height as i32 {
        for x in 0..island.width as i32 {
            if skip.iter().any(|&(sx, sy)| {
                (sx - x).abs() < def.width as i32 + 2 && (sy - y).abs() < def.height as i32 + 2
            }) {
                continue;
            }
            if !can_place_building(island, map, x, y, def.width, def.height) {
                continue;
            }
            if needs_coast
                && !(0..def.height as i32)
                    .any(|dy| (0..def.width as i32).any(|dx| map.is_coastal(x + dx, y + dy)))
            {
                continue;
            }
            return Some((x, y));
        }
    }
    None
}

#[test]
fn new_horizons0_authors_the_campaign_start_unlock_mask() {
    let Some(loaded) = load() else { return };
    // `FUN_00478160` (`1602_exe.c:85423`) copies PLAYER4 `+0x34`
    // straight into the runtime player record at `+0x6c`.
    let authored = loaded.szs.players[0].slot_u32_0x34;
    assert_eq!(
        loaded.sim.players[0].unlock_mask, authored,
        "human's unlock mask must be the PLAYER4-authored value",
    );
    assert_eq!(
        authored,
        MARKT | KAPELLE,
        "campaign start authors INFRA_MARKT | INFRA_KAPELLE",
    );
}

#[test]
fn campaign_start_leaves_the_starter_buildings_placeable() {
    let Some(mut loaded) = load() else { return };
    // Every starter building carries `Bauinfra: INFRA_NIX` in
    // haeuser.cod, so `FUN_0042d530` returns 1 before it even looks at
    // the mask. Under the removed `min_tier` gate the road was
    // wrongly blocked because "highest tier with population > 0" is 0
    // for a settlement that has not been founded yet.
    let starters: &[(&str, Box<dyn Fn(&BuildingDef) -> bool>)] = &[
        (
            "pioneer house",
            Box::new(|d: &BuildingDef| d.prod_kind == "WOHNUNG"),
        ),
        (
            "marketplace",
            Box::new(|d: &BuildingDef| d.prod_kind == "MARKT"),
        ),
        (
            "chapel",
            Box::new(|d: &BuildingDef| d.prod_kind == "KAPELLE"),
        ),
        (
            "fisher's hut",
            Box::new(|d: &BuildingDef| d.prod_kind == "FISCHEREI"),
        ),
        (
            "forester",
            Box::new(|d: &BuildingDef| d.prod_kind == "PLANTAGE" && d.output_good == Good::Wood),
        ),
        (
            "dirt road",
            Box::new(|d: &BuildingDef| d.kind == "STRASSE" && d.cost_bricks == 0),
        ),
    ];

    // A pristine stock island the scenario left unclaimed.
    let island_idx = loaded
        .szs
        .islands
        .iter()
        .position(|i| i.city.is_none() && !i.tiles.is_empty())
        .expect("New Horizons0 has free islands");
    let islands = loaded.szs.islands.clone();
    loaded.sim.players[0].gold = 100_000;

    let mut used: Vec<(i32, i32)> = Vec::new();
    for (name, pick) in starters {
        let idx = def_index(&loaded.defs, pick.as_ref());
        assert_eq!(
            loaded.defs[idx].bauinfra, 0,
            "{name} should be INFRA_NIX in haeuser.cod",
        );
        let Some((x, y)) = free_tile(&loaded.sim, &islands[island_idx], &loaded.defs[idx], &used)
        else {
            panic!("no free tile for {name}");
        };
        let outcome = anno_game::game_commands::place_building(
            &mut loaded.sim,
            &islands,
            island_idx,
            &loaded.defs,
            &loaded.cod,
            idx,
            0,
            0,
            x,
            y,
        );
        assert!(
            matches!(outcome, PlaceOutcome::Placed),
            "{name} must be placeable at t0",
        );
        used.push((x, y));
    }
}

#[test]
fn rinderfarm_unlocks_once_the_city_reaches_thirty_inhabitants() {
    let Some(mut loaded) = load() else { return };
    // The Rinderfarm (`Kind: WEIDETIER`) carries
    // `Bauinfra: INFRA_STUFE_1A`, id 15 -> `(BGruppe 0, Minwohn 30)`.
    let idx = def_index(&loaded.defs, |d| d.prod_kind == "WEIDETIER");
    assert_eq!(loaded.defs[idx].bauinfra, 15);
    assert_eq!(INFRA_NAMES[15], "INFRA_STUFE_1A");
    assert_eq!(BAUINFRA_LADDER[15], (0, 30));

    let island_idx = loaded
        .szs
        .islands
        .iter()
        .position(|i| i.city.is_none() && !i.tiles.is_empty())
        .expect("New Horizons0 has free islands");
    let islands = loaded.szs.islands.clone();
    loaded.sim.players[0].gold = 100_000;

    let tile = free_tile(&loaded.sim, &islands[island_idx], &loaded.defs[idx], &[])
        .expect("free tile for the Rinderfarm");
    let outcome = anno_game::game_commands::place_building(
        &mut loaded.sim,
        &islands,
        island_idx,
        &loaded.defs,
        &loaded.cod,
        idx,
        0,
        0,
        tile.0,
        tile.1,
    );
    assert!(
        matches!(outcome, PlaceOutcome::NotUnlocked { infra: 15 }),
        "the campaign start's 0x3 mask must not carry INFRA_STUFE_1A",
    );

    // Found a 30-pioneer settlement for the human and let the running
    // simulation's `FUN_0047f8a0` city sweep notice it.
    let slot = (0..SourceCityTable::slot_count())
        .find(|&s| loaded.sim.source_cities.record(s).is_none())
        .expect("a free city slot");
    assert!(loaded.sim.source_cities.set_record(
        slot,
        Some(SourceCityRecord {
            island_id: islands[island_idx].number,
            source_owner: 0,
            owner_slot: 0,
            tier_population: [30, 0, 0, 0, 0],
            ..SourceCityRecord::default()
        })
    ));
    assert_eq!(
        loaded.sim.source_kind4_dispatch.active_player_slot, 0,
        "the human drives the city dispatcher in this scenario",
    );
    // The dispatcher only revisits a record after its 10 s phase
    // counter rolls over and the round-robin cursor comes back around.
    for _ in 0..1_500 {
        loaded.sim.tick(200);
        if loaded.sim.players[0].unlock_mask & STUFE_1A != 0 {
            break;
        }
    }
    assert_eq!(
        loaded.sim.players[0].unlock_mask & STUFE_1A,
        STUFE_1A,
        "30 pioneers must grant INFRA_STUFE_1A",
    );
    // The sweep only ORs, so the campaign-start bits survive.
    assert_eq!(
        loaded.sim.players[0].unlock_mask & (MARKT | KAPELLE),
        MARKT | KAPELLE,
    );

    loaded.sim.players[0].gold = 100_000;
    let tile = free_tile(&loaded.sim, &islands[island_idx], &loaded.defs[idx], &[])
        .expect("free tile for the Rinderfarm");
    let outcome = anno_game::game_commands::place_building(
        &mut loaded.sim,
        &islands,
        island_idx,
        &loaded.defs,
        &loaded.cod,
        idx,
        0,
        0,
        tile.0,
        tile.1,
    );
    assert!(
        matches!(outcome, PlaceOutcome::Placed),
        "the Rinderfarm becomes placeable once the rung is unlocked",
    );
}
