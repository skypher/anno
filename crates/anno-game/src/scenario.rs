//! Scenario → [`Simulation`] construction shared by the SDL game binary and
//! headless tools (lockstep driver, replay verification).
//!
//! Extracted verbatim from the game binary so a `Simulation` built here is
//! the same one the interactive game runs.

use anno_formats::cod::CodFile;
use anno_formats::figuren::FiguresFile;
use anno_formats::szs::SzsFile;
use anno_sim::ai::AiController;
use anno_sim::data_bridge;
use anno_sim::island_map::IslandMap;
use anno_sim::player::Player;
use anno_sim::simulation::Simulation;
use anno_sim::warehouse::Warehouse;

/// Instantiate stock islands for `INSEL5`-only records: scenario islands
/// that ship without inline tiles load a library island at scenario start.
/// The original formats `<base><climate><size><NN>.SCP` (`FUN_00469690`,
/// `1602_exe.c:73731`; climate dirs `Nord\`/`Sued\` by map half, size
/// prefixes by family) and loads its INSELHAUS (`FUN_00469770`,
/// `1602_exe.c:73766`), picking a random file each playthrough. This port
/// picks **deterministically** from `seed` and the island number so
/// scripted playthroughs replay identically.
///
/// Fertility is *not* touched here, and the original does not roll it
/// either. `FUN_00469770` only ever preserves or resets the crop mask:
///
/// ```text
/// if (*(byte *)(param_1 + 7) == param_2) { local_210 = param_1[0x17]; }  // keep island+0x5c
/// else                                    { local_210 = 0x1181; }        // bare baseline
/// ...
/// param_1[0x17] = local_210;                                             // island+0x5c
/// ```
///
/// (`1602_exe.c:73780-73788`, `:73810`). `param_2` is the climate index
/// that also picks the `Nord\`/`Sued\` directory, and it is stored at
/// runtime `island+0x1c` — the same byte the scenario authors at
/// `INSEL5[0x64]`. Since a scenario's climate byte always matches the map
/// half its island sits in, the authored crop mask survives instantiation
/// verbatim. The one place the executable synthesises a mask is the
/// editor's "drop a random island" path, which writes the bare baseline
/// (`island+0x5c = 0x1181`, `1602_exe.c:44273`) and no crops at all.
///
/// Every shipping scenario authors real crop masks in its INSEL5 records
/// (New Horizons0 spreads sugarcane, cotton, tobacco, vines, spices and
/// cocoa across its twelve free islands), so there is nothing to
/// synthesise: `SzsFile::parse` already decodes them into
/// `Island::fertilities`.
///
/// `MAP_HEIGHT` bounds the north/south split; the shipping maps are
/// 500×350 world tiles.
pub fn instantiate_stock_islands(szs: &mut SzsFile, data_dir: &std::path::Path, seed: u32) {
    const MAP_HEIGHT: u16 = 350;
    for island in &mut szs.islands {
        if !island.tiles.is_empty() {
            continue;
        }
        let prefix = match (island.width, island.height) {
            (30, 30) => "lit",
            (40, 40) => "mit",
            (50, 52) => "med",
            (70, 60) | (100, 90) => "big",
            _ => "lar",
        };
        let southern = island.y_pos + u16::from(island.height) / 2 >= MAP_HEIGHT / 2;
        let climate_dir = if southern { "SUED" } else { "NORD" };
        let dir = data_dir.join(climate_dir);
        let mut candidates: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .map(|name| {
                                let lower = name.to_ascii_lowercase();
                                lower.starts_with(prefix) && lower.ends_with(".scp")
                            })
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        candidates.sort();
        if candidates.is_empty() {
            continue;
        }
        let pick = (seed as usize)
            .wrapping_mul(31)
            .wrapping_add(usize::from(island.number))
            % candidates.len();
        // Rotate from the seeded pick until a candidate parses at the
        // expected dimensions (a few library files are mislabeled or use
        // odd sizes).
        let Some(tiles) = (0..candidates.len()).find_map(|offset| {
            let path = &candidates[(pick + offset) % candidates.len()];
            let data = std::fs::read(path).ok()?;
            let (width, height, tiles) = SzsFile::parse_stock_island(&data)?;
            (width == island.width && height == island.height).then_some(tiles)
        }) else {
            continue;
        };
        island.tiles = tiles;
    }
}

/// Build the same `Simulation` the game binary runs: [`init_simulation`]
/// plus the figuren.cod-derived carrier, city-cart, civilian, and ship-cargo
/// configuration the game's main() applies before its first tick. Headless
/// tools use this so a lockstep run starts from the exact interactive state.
pub fn build_simulation(
    szs: &SzsFile,
    cod: &CodFile,
    defs: &[anno_sim::building::BuildingDef],
    figures: &FiguresFile,
) -> Simulation {
    let ship_cargo_config = anno_sim::trade::ShipCargoConfig::from_figures(figures);
    let mut sim = init_simulation(szs, cod, defs, ship_cargo_config);
    if let Some(traeger) = figures.find("TRAEGER") {
        sim.carrier_config = anno_sim::carrier::CarrierConfig::from_figure_def(traeger);
    }
    if let Some(karren) = figures.find("KARREN") {
        sim.city_cart_config = anno_sim::carrier::CityCartConfig::from_figure_def(karren);
    }
    if let Some(traeger2) = figures.find("TRAEGER2") {
        sim.city_cart_traeger2_config =
            anno_sim::carrier::CityCartConfig::from_figure_def(traeger2);
    }
    sim.civilian_config = anno_sim::civilian::CivilianConfig::from_figures(figures);
    // The scenario's AUFTRAG4 goals are the mission's win condition; without
    // this the simulation ran with an empty objective set and could never
    // report the scenario complete.
    if let Some(mission) = szs.mission.as_ref() {
        let objectives = anno_sim::objectives::ObjectiveSet::from_mission_goals(&mission.goals());
        if !objectives.items.is_empty() {
            sim.objectives = objectives;
        }
    }
    sim
}

pub fn init_simulation(
    szs: &SzsFile,
    cod: &CodFile,
    defs: &[anno_sim::building::BuildingDef],
    ship_cargo_config: anno_sim::trade::ShipCargoConfig,
) -> Simulation {
    let instances = data_bridge::load_building_instances(szs, cod, defs);

    // Create warehouses — one per island with production
    // buildings OR a STADT4 city. The STADT4 fallback covers
    // pirate / native islands that ship as dwelling-only
    // settlements with no production tiles in INSELHAUS; the
    // original engine spawns a Kontor for those slots
    // dynamically at scenario start.
    // Only settled islands get a warehouse: a STADT4 city (or an authored
    // Kontor, handled below). Terrain production instances alone (forests,
    // ore rocks on pristine stock islands) must not conjure one — the
    // original has no island store until a Kontor exists, and a phantom
    // empty warehouse shadows the one `Command::FoundKontor` creates.
    let mut island_ids: std::collections::BTreeSet<u8> = std::collections::BTreeSet::new();
    for island in &szs.islands {
        if let Some(city) = island.city.as_ref() {
            if !city.name.is_empty() {
                island_ids.insert(island.number);
            }
        }
    }
    let island_ids: Vec<u8> = island_ids.into_iter().collect();

    // Prefer the actual KONTOR tile placement from INSELHAUS
    // when the scenario provides it. Falls back to a centroid
    // of production buildings only when no Kontor is placed
    // (Continous-Play templates with bare land).
    let mut warehouses = anno_sim::data_bridge::kontor_warehouses_from_szs(szs, cod, defs);
    let kontor_islands: std::collections::HashSet<u8> =
        warehouses.iter().map(|w| w.island_id).collect();
    for &island_id in &island_ids {
        if kontor_islands.contains(&island_id) {
            continue;
        }
        let island_buildings: Vec<_> = instances
            .iter()
            .filter(|b| b.island_id == island_id)
            .collect();
        // Pick the fallback warehouse owner from the island's
        // STADT4 city when present.
        let island = szs.islands.iter().find(|i| i.number == island_id);
        let owner = island
            .and_then(|i| i.city.as_ref())
            .map(|c| c.owner_slot)
            .unwrap_or(0);
        let city_population = island
            .and_then(|i| i.city.as_ref())
            .map(|c| c.tier_population)
            .unwrap_or([0; 5]);
        let (avg_x, avg_y) = if !island_buildings.is_empty() {
            let ax = island_buildings
                .iter()
                .map(|b| b.tile_x as u32)
                .sum::<u32>()
                / island_buildings.len() as u32;
            let ay = island_buildings
                .iter()
                .map(|b| b.tile_y as u32)
                .sum::<u32>()
                / island_buildings.len() as u32;
            (ax as u16, ay as u16)
        } else if let Some(island) = island {
            // Dwelling-only pirate/native settlements: anchor on
            // the island centre. The original engine drops a
            // Kontor here on first sight.
            (
                island.x_pos + island.width as u16 / 2,
                island.y_pos + island.height as u16 / 2,
            )
        } else {
            continue;
        };
        warehouses.push(Warehouse::with_capacity_and_population(
            island_id,
            owner,
            avg_x,
            avg_y,
            anno_sim::warehouse::BASE_KONTOR_CAPACITY,
            city_population,
        ));
    }

    // Seed authored city-store stocks from the scenario's KONTOR2
    // chunks. The source loader (`0x484230`) writes each record's
    // `+0x0c` u16 straight into the runtime city ware entry selected by
    // the record definition's compiled ware byte; without this the Rust
    // Kontors start empty and the exact `FUN_0047f8a0` demand cycle
    // correctly starves the city (live Exile check: the original's
    // human city opens with NAHRUNG 800, ALKOHOL/STOFFE 509, ...).
    for kontor in &szs.kontors {
        let Some(warehouse) = warehouses
            .iter_mut()
            .filter(|warehouse| warehouse.island_id == kontor.island_index)
            .min_by_key(|warehouse| {
                let dx = i32::from(warehouse.tile_x) - i32::from(kontor.tile_x);
                let dy = i32::from(warehouse.tile_y) - i32::from(kontor.tile_y);
                dx * dx + dy * dy
            })
        else {
            continue;
        };
        for stock in &kontor.stocks {
            if stock.definition_raw_id == 0 || stock.stock_fixed == 0 {
                continue;
            }
            let source_id = anno_formats::cod::SOURCE_DEFINITION_ID_BASE
                + i32::from(stock.definition_raw_id);
            let Some(good) = cod
                .building_by_source_id(source_id)
                .and_then(|def| def.source_ware_slot())
                .map(anno_sim::data_bridge::good_for_source_ware_slot)
            else {
                continue;
            };
            if good != anno_sim::types::Good::None {
                warehouse.seed_city_stock_fixed(good, stock.stock_fixed);
            }
        }
    }

    // Build island walkability maps
    let island_maps: Vec<IslandMap> = szs
        .islands
        .iter()
        .enumerate()
        .map(|(index, island)| {
            IslandMap::from_island(island, &cod.buildings)
                .with_source_runtime_classification(szs.island_source_runtime_classification(index))
                .with_source_resource_state(szs.island_source_resource_state(index))
        })
        .collect();

    // Build coverage maps for each island
    let coverage_maps: Vec<anno_sim::coverage::CoverageMap> = szs
        .islands
        .iter()
        .map(|island| {
            anno_sim::coverage::CoverageMap::new(
                island.number,
                island.width as u16,
                island.height as u16,
            )
        })
        .collect();

    // Build the source static-map overlay for ship pathfinding.
    let ocean_map = anno_sim::ocean_map::OceanMap::from_source_scenario(szs, &cod.buildings);
    println!(
        "Ocean map: {}x{} ({} navigable tiles)",
        ocean_map.width,
        ocean_map.height,
        (0..ocean_map.height as i32)
            .flat_map(|y| (0..ocean_map.width as i32).map(move |x| (x, y)))
            .filter(|&(x, y)| ocean_map.is_navigable(x, y))
            .count()
    );

    let mut sim = Simulation::new();
    sim.diplomacy = anno_sim::data_bridge::diplomacy_from_player4_relationships(&szs.players);
    sim.source_kind4_dispatch =
        anno_sim::data_bridge::source_kind4_dispatch_state_from_scenario(szs);
    sim.configure_source_controller_figure_capacity_limits(&szs.players);
    sim.building_defs = defs.to_vec();
    sim.source_dynamic_map_objects =
        anno_sim::data_bridge::source_dynamic_map_objects_from_scenario(szs, cod);
    sim.source_map_cell_states =
        anno_sim::data_bridge::source_map_cell_states_from_scenario(szs, cod);
    sim.source_static_map_roots =
        anno_sim::data_bridge::source_static_map_roots_from_scenario(szs, cod);
    sim.source_static_map_backing_cells =
        anno_sim::data_bridge::source_static_map_backing_cells_from_scenario(szs, cod);
    sim.source_kind13_locations =
        anno_sim::data_bridge::source_kind13_locations_from_scenario(szs, cod);
    sim.source_kind13_promotion_definitions =
        anno_sim::data_bridge::source_kind13_promotion_definitions(cod);
    sim.source_cities = anno_sim::data_bridge::source_cities_from_scenario(szs);
    sim.source_kind4_occupants = anno_sim::data_bridge::source_kind4_occupants_from_scenario(szs);
    sim.buildings = instances;
    sim.warehouses = warehouses;
    sim.island_maps = island_maps;
    sim.configure_source_controller_active_cities();
    sim.mark_loaded_source_islands_visible();
    // Initial `FUN_00482120` coverage scan: derive each residence's
    // infrastructure lifecycle bits, market state bit, and distance-class
    // variant from the actual buildings (the SIEDLER-authored values are
    // snapshots of the same computation).
    {
        let mut islands_with_houses: Vec<u8> = sim
            .source_kind13_locations
            .active_locations()
            .into_iter()
            .map(|location| location.island_id)
            .collect();
        islands_with_houses.sort_unstable();
        islands_with_houses.dedup();
        for island_id in islands_with_houses {
            sim.refresh_source_house_infrastructure(island_id);
        }
    }
    sim.coverage_maps = coverage_maps;
    sim.ocean_map = Some(ocean_map);
    sim.ship_cargo_config = ship_cargo_config;

    // Initialise the seven player slots from PLAYER4's state_byte
    // (0x00 = human, 0x0c = AI rival, 0x0d/0x0e/0x0b = reserved
    // trader / native / pirate factions). Reserved factions stay
    // PlayerState::Empty so the defeat checker skips them — the
    // trader / native / pirate subsystems address those slots
    // directly by index. Slots without a PLAYER4 record fall back
    // to PlayerState::Empty as a placeholder.
    //
    // AI rivals further gate on `ai_active`: when byte 0x0d of
    // the slot record is 0x01, `1602_exe.c::FUN_00473c50:82622`
    // skips the slot entirely, so we mirror that by promoting
    // it to PlayerState::Empty. Exile / New Horizons2 ship
    // pre-configured-but-disabled AI rosters this way.
    use anno_sim::player::PlayerState;
    for slot in 0u8..7 {
        let init = szs.players.get(slot as usize);
        let state_byte = init.map(|p| p.state_byte).unwrap_or(0xff);
        let ai_active = init.map(|p| p.ai_active).unwrap_or(true);
        let effective_state = if state_byte == 0x0c && !ai_active {
            0xff // disabled AI → treat as empty
        } else {
            state_byte
        };
        let mut p = match effective_state {
            0x00 => Player::new_human(slot),
            0x0c => Player::new_ai(slot, 0),
            _ => {
                let mut p = Player::new_ai(slot, 0);
                p.state = PlayerState::Empty;
                p
            }
        };
        if let Some(init) = init {
            p.gold = init.starting_gold;
            // Initial building-unlock bitmask: `FUN_00478160`
            // (`1602_exe.c:85423`) copies the PLAYER4 slot's `+0x34`
            // dword verbatim into the runtime player record at `+0x6c`
            // (`DAT_005b76ec`). Campaign starts author `0x0000_0003`
            // — INFRA_MARKT | INFRA_KAPELLE — and the reserved
            // trader / native / pirate slots author `0xFFFF_FFFF`
            // (everything unlocked); we copy either verbatim.
            p.unlock_mask = init.slot_u32_0x34;
        }
        sim.players.push(p);

        // Spawn an AI controller alongside every AI rival slot. The raw
        // PLAYER4 `slot_u16_0x18` byte is parsed, but its binary semantics are
        // not pinned, so `personality_from_slot_byte` currently returns the
        // conservative default instead of inventing per-scenario AI behavior.
        if effective_state == 0x0c {
            let slot_byte = init.map(|p| p.slot_u16_0x18).unwrap_or(0);
            let (personality, difficulty) = anno_sim::ai::personality_from_slot_byte(slot_byte);
            sim.ai_controllers
                .push(AiController::new(slot, personality, difficulty));
        }
    }

    // Seed populations from STADT4 city records when the
    // scenario provides them. Each city's tier_population
    // contributes to its owner_slot's player_population.
    for island in &szs.islands {
        let Some(city) = island.city.as_ref() else {
            continue;
        };
        if city.tier_population.iter().all(|&v| v == 0) {
            continue;
        }
        let slot = city.owner_slot as usize;
        if let Some(p) = sim.players.get_mut(slot) {
            for tier in 0..5 {
                p.population[tier] += city.tier_population[tier];
            }
        }
    }
    for p in &mut sim.players {
        p.total_population = p.population.iter().sum();
    }

    // Reconstruct every static type-4 land figure from SOLDAT3. These units
    // retain their source slots so combat removal can update the matching
    // source island-owner occupancy record.
    let land_units = anno_sim::data_bridge::land_units_from_scenario(szs);
    if !land_units.is_empty() {
        println!(
            "Spawning {} static land unit(s) from SOLDAT3",
            land_units.len()
        );
        sim.military_units.extend(land_units);
    }

    // Spawn warships from SHIP4: SmallWarship / LargeWarship /
    // PirateShip records become MilitaryUnit instances at the
    // exact spawn coordinates the scenario author placed them.
    let mut warships = anno_sim::data_bridge::warships_from_ships(&szs.ships);
    // Spawn trader hulls from SHIP4 too: SmallTrader / LargeTrader
    // records become TradeShip instances at their authored
    // coordinates with a sentinel route_id so the trade tick
    // leaves them inert until a route is assigned.
    let mut traders = anno_sim::data_bridge::traders_from_ships(&szs.ships, sim.ship_cargo_config);
    anno_sim::data_bridge::resolve_ship_kind6_policy_slots(cod, &mut warships, &mut traders);
    if !warships.is_empty() {
        let named: Vec<&str> = warships
            .iter()
            .filter(|u| !u.name.is_empty())
            .map(|u| u.name.as_str())
            .take(5)
            .collect();
        let suffix = if named.is_empty() {
            String::new()
        } else if warships.iter().filter(|u| !u.name.is_empty()).count() > 5 {
            format!(" — {} …", named.join(", "))
        } else {
            format!(" — {}", named.join(", "))
        };
        println!(
            "Spawning {} static warship(s) from SHIP4{suffix}",
            warships.len()
        );
        sim.military_units.extend(warships);
    }
    if !traders.is_empty() {
        println!("Spawning {} static trader(s) from SHIP4", traders.len());
        sim.trade_ships.extend(traders);
    }

    sim
}
