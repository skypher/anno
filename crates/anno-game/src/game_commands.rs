//! Game-layer command application shared by the SDL binary and headless
//! tools.
//!
//! Most player commands are pure simulation mutations and live in
//! `anno_sim::commands`. Building placement and demolition additionally
//! need the compiled COD table (to synthesize source map commands) and the
//! scenario island list, which the simulation does not own — so their
//! application lives here, and [`apply_game_command`] is the single choke
//! point that handles every `Command` a recording can contain.

use anno_formats::cod::CodFile;
use anno_formats::szs::Island;
use anno_sim::building::{BuildingDef, BuildingInstance};
use anno_sim::commands::Command;
use anno_sim::island_map::IslandMap;
use anno_sim::simulation::Simulation;
use anno_sim::types::Good;

/// Outcome of a placement attempt. The caller drives banners/sound
/// off these so the placement helper stays free of UI dependencies.
pub enum PlaceOutcome {
    Placed,
    NotEnoughGold {
        need: u32,
        have: i32,
    },
    BlockedByTerrain,
    NotCoastal,
    NoIslandMap,
    NoBuildingSelected,
    /// Bauinfra gate: the owner's 32-bit unlock mask (`player + 0x6c`)
    /// does not carry bit `1 << (infra - 1)`. RE `FUN_0042d530`
    /// (`1602_exe.c:33209-33265`). `infra` is the definition's
    /// `INFRA_*` constant id — index it into
    /// `data_bridge::INFRA_NAMES` / `BAUINFRA_LADDER` to report it.
    NotUnlocked {
        infra: u8,
    },
}

pub fn can_place_building(
    island: &Island,
    island_map: &IslandMap,
    tile_x: i32,
    tile_y: i32,
    width: u8,
    height: u8,
) -> bool {
    // Check all tiles in the footprint
    for dy in 0..height as i32 {
        for dx in 0..width as i32 {
            let tx = tile_x + dx;
            let ty = tile_y + dy;

            // Must be within island bounds
            if tx < 0 || ty < 0 || tx >= island.width as i32 || ty >= island.height as i32 {
                return false;
            }

            // Must be on walkable terrain (not water or existing building)
            if !island_map.is_walkable(tx, ty) {
                return false;
            }
        }
    }
    true
}

/// Attempt to place building definition `def_index` at `(tile_x, tile_y)`
/// on `current_island` for `owner`. Mirrors the original click-place flow:
/// fishery coast gate, walkability, gold cost, Bauinfra unlock mask,
/// materials trickle. Side-effecting helper used by the game's click
/// handler, its drag-place loop, and command replay.
///
/// Note that fertility is deliberately **not** a placement gate. The
/// original's only refusal is the Bauinfra unlock test `FUN_0042d530`
/// (`1602_exe.c:33210-33265`), which reads the player record and never
/// touches the island. Fertility instead feeds the placement-time
/// grow-vs-wither roll in the build applier (`1602_exe.c:7754-7760`):
///
/// ```text
/// if ((piVar4[7] == 10) && (FUN_004684a0(map, def+0xa9, x, y) == 0)) piVar4 = piVar4 + 0x44;
/// ```
///
/// `int*` `+0x44` is `+0x110` bytes, i.e. two definition records on — the
/// withered "DOERR" variant. So placing a cocoa plantation on a
/// cocoa-free island succeeds; the crop simply never thrives. Modelling
/// that variant swap is still outstanding.
#[allow(clippy::too_many_arguments)]
pub fn place_building(
    sim: &mut Simulation,
    islands: &[Island],
    current_island: usize,
    defs: &[BuildingDef],
    cod: &CodFile,
    def_index: usize,
    orientation: u8,
    owner: u8,
    tile_x: i32,
    tile_y: i32,
) -> PlaceOutcome {
    if def_index >= defs.len() || def_index >= cod.buildings.len() {
        return PlaceOutcome::NoBuildingSelected;
    }
    let def_idx = def_index;
    let def = &defs[def_idx];
    let island_number = islands[current_island].number;
    let bld_w = def.width;
    let bld_h = def.height;
    let cost = def.cost_gold;

    let island_map_idx = sim
        .island_maps
        .iter()
        .position(|m| m.island_id == island_number);

    // Fishery coast gate.
    if def.output_good == Good::Fish {
        let coast_ok = if let Some(idx) = island_map_idx {
            let map = &sim.island_maps[idx];
            (0..bld_h as i32)
                .any(|dy| (0..bld_w as i32).any(|dx| map.is_coastal(tile_x + dx, tile_y + dy)))
        } else {
            false
        };
        if !coast_ok {
            return PlaceOutcome::NotCoastal;
        }
    }

    let map_idx = match island_map_idx {
        Some(i) => i,
        None => return PlaceOutcome::NoIslandMap,
    };
    if !can_place_building(
        &islands[current_island],
        &sim.island_maps[map_idx],
        tile_x,
        tile_y,
        bld_w,
        bld_h,
    ) {
        return PlaceOutcome::BlockedByTerrain;
    }
    let owner_idx = owner as usize;
    if owner_idx >= sim.players.len() || sim.players[owner_idx].gold < cost as i32 {
        return PlaceOutcome::NotEnoughGold {
            need: cost,
            have: sim.players.get(owner_idx).map(|p| p.gold).unwrap_or(0),
        };
    }
    // Building-unlock gate, `FUN_0042d530` @ 0x0042d530
    // (`1602_exe.c:33209-33265`): a definition whose `Bauinfra` byte
    // is 0 (`INFRA_NIX`) is always buildable; otherwise the owner's
    // 32-bit unlock mask at `player + 0x6c` must carry bit
    // `1 << (bauinfra - 1)`. Those bits are granted by the per-city
    // sweep in `FUN_0047f8a0` as the settlement grows.
    //
    // Two source behaviours are deliberately not reproduced:
    //   * the "no military" game option (`DAT_005b706c & 0x200`) ANDs
    //     the mask with 0x5BBFFFFF before the test — we do not model
    //     game options yet;
    //   * ids 11..=14 (SCHLOSS / KATHETRALE / TRIUMPH / DENKMAL) add
    //     unique-monument side conditions on player counters at
    //     `+0x87`, `+0x88`, `+0x8a`/`+0x8c` and `+0x8e`/`+0x90` which
    //     this simulation does not model, so they fall through to the
    //     plain mask test below.
    if def.bauinfra > 0 {
        let mask = sim
            .players
            .get(owner_idx)
            .map(|p| p.unlock_mask)
            .unwrap_or(0);
        let bit = 1u32 << (u32::from(def.bauinfra) - 1);
        if mask & bit == 0 {
            return PlaceOutcome::NotUnlocked {
                infra: def.bauinfra,
            };
        }
    }

    // All gates passed — apply the placement.
    if owner_idx < sim.players.len() {
        sim.players[owner_idx].gold -= cost as i32;
    }
    let cod_b = &cod.buildings[def_idx];
    let rot_count = cod_b.rotate.max(1) as u8;
    let orient = orientation % rot_count;
    let source_definition_offset = (cod_b.source_id - anno_formats::szs::INSELHAUS_SOURCE_ID_BASE)
        .try_into()
        .ok();
    // Player construction belongs to the player's city on this island:
    // the original stamps the city selector into the claimed tiles, and
    // the kind-13 dispatch resolves a record's city through exactly this
    // selector. Scenario ground tiles on pristine stock islands carry no
    // ownership, so prefer the live city record over the tile bits.
    let source_map_owner_slot = sim
        .source_cities
        .active_records()
        .into_iter()
        .find(|city| city.island_id == island_number && city.owner_slot == owner)
        .map(|city| city.source_owner)
        .or_else(|| {
            islands[current_island]
                .tiles
                .iter()
                .find(|tile| tile.x as i32 == tile_x && tile.y as i32 == tile_y)
                .map(|tile| tile.source_owner())
        })
        .unwrap_or(7);
    let source_random_seed = (sim.next_source_rand() & 0x1f) as u8;
    for dy in 0..bld_h as u8 {
        for dx in 0..bld_w as u8 {
            let tx = tile_x as u8 + dx;
            let ty = tile_y as u8 + dy;
            sim.island_maps[map_idx].set_walkable(tx as u16, ty as u16, false);
        }
    }

    let mut instance = BuildingInstance::new(
        def_idx as u16,
        island_number,
        tile_x as u16,
        tile_y as u16,
        owner,
    );
    instance.source_placement_command = source_definition_offset.map(|definition_offset| {
        anno_sim::building::SourceBuildingCommand {
            definition_offset,
            orientation: orient,
            // `DAT_006c2f10 = *DAT_0049f790` in `FUN_0040a190`; the
            // low byte is the selected island record identifier.
            metadata: island_number,
            // The player command path explicitly clears `DAT_0049e6f8` for
            // definitions without a source-selected variant. This placement
            // path has no variant selector yet, so it carries that zero form.
            variant: 0,
            map_owner_slot: source_map_owner_slot,
            // `FUN_004631b0` itself writes `rand() & 31` into bits 17..=21.
            random_seed: source_random_seed,
            dynamic_object_owner: owner,
        }
    });
    // Mines tap a finite ore deposit (RE: haeuser.cod Erzbergnr).
    // Non-mine buildings keep the u16::MAX uncapped default.
    let cap = def.ore_deposit.capacity();
    if cap > 0 {
        instance.remaining_ore = cap;
    }
    let footprint = (def.width as u32) * (def.height as u32);
    let build_ms = (2_000u32 * footprint).max(2_000);
    instance.construction_ms_total = build_ms;
    instance.construction_ms_remaining = build_ms;
    instance.wood_needed = def.cost_wood;
    instance.tools_needed = def.cost_tools;
    instance.bricks_needed = def.cost_bricks;
    let source_cell_state = instance.source_placement_command.and_then(|command| {
        let mut state = anno_sim::source_cell::SourceMapCellState::new(
            island_number,
            tile_x as u8,
            tile_y as u8,
            cod_b,
            ((sim.game_clock / 10) & 7) as u8,
        )?;
        let (footprint_width, footprint_height) = if matches!(orient & 3, 1 | 3) {
            (cod_b.size.1, cod_b.size.0)
        } else {
            cod_b.size
        };
        state.set_footprint(footprint_width, footprint_height);
        state.set_source_command(command);
        state.configure_terminal_replacement(cod);
        Some(state)
    });
    let source_static_root = instance.source_placement_command.and_then(|command| {
        let mut state = anno_sim::source_cell::SourceMapCellState::new_static(
            island_number,
            tile_x as u8,
            tile_y as u8,
            cod_b,
            ((sim.game_clock / 10) & 7) as u8,
        )?;
        let (footprint_width, footprint_height) = if matches!(orient & 3, 1 | 3) {
            (cod_b.size.1, cod_b.size.0)
        } else {
            cod_b.size
        };
        state.set_footprint(footprint_width, footprint_height);
        state.set_source_command(command);
        state.configure_terminal_replacement(cod);
        Some(state)
    });
    sim.buildings.push(instance);
    if let Some(state) = source_cell_state {
        sim.source_map_cell_states.push(state);
    }
    if let Some(root) = source_static_root {
        sim.replace_source_static_map_footprint(root);
    }
    let building_index = sim.buildings.len() - 1;
    if def.kind == "HQ" {
        let _ = sim.allocate_source_dynamic_map_object_for_building(building_index);
    }
    // `FUN_00478b90`: installing a kind-0x0d object (WOHNUNG/PLATZ)
    // creates its runtime kind-13 record with the 0x40 initial amount
    // (one resident) and the definition's BGruppe.
    if cod_b.source_production_kind_code() == Some(13) {
        sim.source_kind13_locations
            .insert(anno_sim::data_bridge::SourceKind13Location {
                island_id: island_number,
                tile_x: tile_x as u8,
                tile_y: tile_y as u8,
                orientation: orient,
                variant: 0,
                source_owner: source_map_owner_slot,
                phase: 0,
                state_bits: 0,
                population_group: cod_b.source_population_group().unwrap_or(0),
                amount: 0x40,
                lifecycle_flags: 0,
            });
        // The created default resident joins the city ledger, the way
        // the SIEDLER loader's subtraction step implies the install
        // path added it.
        let group = usize::from(cod_b.source_population_group().unwrap_or(0)).min(4);
        let city_slot = (0..anno_sim::data_bridge::SourceCityTable::slot_count()).find(|&slot| {
            sim.source_cities
                .record(slot)
                .is_some_and(|city| city.island_id == island_number && city.owner_slot == owner)
        });
        if let Some(city) = city_slot.and_then(|slot| sim.source_cities.record_mut(slot)) {
            city.tier_population[group] = city.tier_population[group].wrapping_add(1);
        }
    }
    // The source sets the island's construction dirty flags, which the
    // next `FUN_00482120` pass consumes; rescan coverage right away.
    sim.refresh_source_house_infrastructure(island_number);
    PlaceOutcome::Placed
}

/// A successful demolition, for the caller's UI feedback.
pub struct DemolishedBuilding {
    /// Index the building occupied in `sim.buildings` before removal.
    pub building_index: usize,
    pub def_id: u16,
    pub tile_x: u16,
    pub tile_y: u16,
    pub refund: u32,
}

/// Demolish the `player`-owned building whose footprint covers
/// `(tile_x, tile_y)` on `island_id`: refund half its gold cost, restore
/// walkability, release its source records, and remove it.
pub fn demolish_building(
    sim: &mut Simulation,
    defs: &[BuildingDef],
    island_id: u8,
    tile_x: i32,
    tile_y: i32,
    player: u8,
) -> Option<DemolishedBuilding> {
    let building_idx = sim.buildings.iter().position(|b| {
        b.owner == player && b.island_id == island_id && {
            let def = &defs[b.def_id as usize];
            let bx = b.tile_x as i32;
            let by = b.tile_y as i32;
            tile_x >= bx
                && tile_x < bx + def.width as i32
                && tile_y >= by
                && tile_y < by + def.height as i32
        }
    })?;

    let b = &sim.buildings[building_idx];
    let def = &defs[b.def_id as usize];
    let def_id = b.def_id;
    let bx = b.tile_x;
    let by = b.tile_y;
    let bw = def.width;
    let bh = def.height;
    let source_orientation = b
        .source_placement_command
        .map(|command| command.orientation)
        .unwrap_or(0);
    let (source_width, source_height) = if source_orientation & 1 == 0 {
        (bw, bh)
    } else {
        (bh, bw)
    };
    let refund = def.cost_gold / 2;

    // Refund half of construction cost
    if let Some(p) = sim.players.get_mut(player as usize) {
        p.gold += refund as i32;
    }

    // Restore walkability
    let island_map_idx = sim
        .island_maps
        .iter()
        .position(|m| m.island_id == island_id);
    if let Some(map_idx) = island_map_idx {
        for dy in 0..bh {
            for dx in 0..bw {
                sim.island_maps[map_idx].set_walkable(bx + dx as u16, by + dy as u16, true);
            }
        }
    }

    // Remove building from simulation
    let _ = sim.release_source_dynamic_map_object_for_building(building_idx);
    sim.remove_source_map_footprint(island_id, bx, by, source_width, source_height);
    sim.buildings.remove(building_idx);

    Some(DemolishedBuilding {
        building_index: building_idx,
        def_id,
        tile_x: bx,
        tile_y: by,
        refund,
    })
}

/// Apply any recorded [`Command`], including the placement/demolition
/// variants `Simulation::apply_command` alone refuses. This is the replay
/// entry point: a recording captured in the game replays through here.
pub fn apply_game_command(
    sim: &mut Simulation,
    islands: &[Island],
    cod: &CodFile,
    defs: &[BuildingDef],
    cmd: &Command,
) -> bool {
    match *cmd {
        Command::PlaceBuilding {
            player,
            island,
            tile_x,
            tile_y,
            def_index,
            orientation,
        } => {
            let Some(island_index) = islands.iter().position(|i| i.number == island) else {
                return false;
            };
            matches!(
                place_building(
                    sim,
                    islands,
                    island_index,
                    defs,
                    cod,
                    def_index as usize,
                    orientation,
                    player,
                    tile_x as i32,
                    tile_y as i32,
                ),
                PlaceOutcome::Placed
            )
        }
        Command::DemolishBuilding {
            player,
            island,
            tile_x,
            tile_y,
        } => demolish_building(sim, defs, island, tile_x as i32, tile_y as i32, player).is_some(),
        Command::FoundKontor {
            player,
            ship_index,
            island,
            tile_x,
            tile_y,
        } => found_kontor(sim, islands, cod, defs, player, ship_index, island, tile_x, tile_y),
        _ => sim.apply_command(cmd),
    }
}

/// Found a settlement from a docked ship: build the island's first Kontor
/// at a coastal anchor, allocate the source city record
/// (`FUN_00468e10`), create the island `Warehouse`, and unload the ship's
/// cargo into the new city store. The original's ship-founding flow costs
/// no materials.
#[allow(clippy::too_many_arguments)]
pub fn found_kontor(
    sim: &mut Simulation,
    islands: &[Island],
    cod: &CodFile,
    defs: &[BuildingDef],
    player: u8,
    ship_index: u32,
    island_number: u8,
    tile_x: u16,
    tile_y: u16,
) -> bool {
    let Some(island) = islands.iter().find(|i| i.number == island_number) else {
        return false;
    };
    // The founding Kontor definition: the same one authored settlements
    // place (haeuser Nummer 271, source id 22103).
    let Some(def_index) = cod.buildings.iter().position(|b| b.source_id == 22103) else {
        return false;
    };
    let Some(map_idx) = sim
        .island_maps
        .iter()
        .position(|map| map.island_id == island_number)
    else {
        return false;
    };
    // Anchor must sit on the island's coastline.
    if !sim.island_maps[map_idx].is_coastal(i32::from(tile_x), i32::from(tile_y)) {
        return false;
    }
    // Ship gate: player-owned, unrouted, docked near the anchor's world
    // position (the original requires the ship alongside the beach).
    let world_x = i32::from(island.x_pos) + i32::from(tile_x);
    let world_y = i32::from(island.y_pos) + i32::from(tile_y);
    let cargo = {
        let Some(ship) = sim.trade_ships.get_mut(ship_index as usize) else {
            return false;
        };
        if !ship.active
            || ship.owner != player
            || ship.route_id != anno_sim::data_bridge::UNROUTED_TRADER_ROUTE_ID
        {
            return false;
        }
        let dist = (ship.world_x - world_x).abs() + (ship.world_y - world_y).abs();
        if dist > 12 {
            return false;
        }
        std::mem::take(&mut ship.cargo)
    };
    if sim
        .trade_ships
        .get_mut(ship_index as usize)
        .map(|ship| ship.cargo_total = 0)
        .is_none()
    {
        return false;
    }

    // City record + warehouse, then the Kontor building itself.
    let source_time = sim.source_time_ticks;
    sim.source_cities.allocate_source_city(
        island_number,
        tile_x as u8,
        tile_y as u8,
        player,
        source_time,
    );
    let mut warehouse = anno_sim::warehouse::Warehouse::with_capacity(
        island_number,
        player,
        tile_x,
        tile_y,
        anno_sim::warehouse::BASE_KONTOR_CAPACITY,
    );
    // Unload what fits (Maxlager 50 per good); overflow stays aboard.
    let mut leftover: Vec<(anno_sim::types::Good, u16)> = Vec::new();
    for (good, amount) in &cargo {
        let stored = warehouse.deposit(*good, *amount);
        if stored < *amount {
            leftover.push((*good, *amount - stored));
        }
    }
    sim.warehouses.push(warehouse);
    if let Some(ship) = sim.trade_ships.get_mut(ship_index as usize) {
        for (good, amount) in leftover {
            ship.load_unchecked(good, amount);
        }
    }

    // Place the Kontor structure: footprint blocking, instance, and the
    // source records, mirroring `place_building`'s tail without its
    // land-terrain and cost gates (the Kontor spans the beach line and
    // founding is free).
    let def = &defs[def_index];
    let cod_b = &cod.buildings[def_index];
    for dy in 0..def.height {
        for dx in 0..def.width {
            sim.island_maps[map_idx]
                .set_walkable(tile_x + u16::from(dx), tile_y + u16::from(dy), false);
        }
    }
    let source_map_owner_slot = sim
        .source_cities
        .active_records()
        .into_iter()
        .find(|city| city.island_id == island_number && city.owner_slot == player)
        .map(|city| city.source_owner)
        .unwrap_or(0);
    let source_random_seed = (sim.next_source_rand() & 0x1f) as u8;
    let command = anno_sim::building::SourceBuildingCommand {
        definition_offset: (cod_b.source_id - anno_formats::szs::INSELHAUS_SOURCE_ID_BASE)
            .try_into()
            .unwrap_or(0),
        orientation: 0,
        metadata: island_number,
        variant: 0,
        map_owner_slot: source_map_owner_slot,
        random_seed: source_random_seed,
        dynamic_object_owner: player,
    };
    let mut instance = BuildingInstance::new(
        def_index as u16,
        island_number,
        tile_x,
        tile_y,
        player,
    );
    instance.source_placement_command = Some(command);
    let phase = ((sim.game_clock / 10) & 7) as u8;
    if let Some(mut state) = anno_sim::source_cell::SourceMapCellState::new_static(
        island_number,
        tile_x as u8,
        tile_y as u8,
        cod_b,
        phase,
    ) {
        state.set_footprint(cod_b.size.0, cod_b.size.1);
        state.set_source_command(command);
        state.configure_terminal_replacement(cod);
        sim.replace_source_static_map_footprint(state);
    }
    sim.buildings.push(instance);
    let building_index = sim.buildings.len() - 1;
    if def.kind == "HQ" {
        let _ = sim.allocate_source_dynamic_map_object_for_building(building_index);
    }
    sim.refresh_source_house_infrastructure(island_number);
    sim.event_log.push(format!(
        "[found] player {player} settles island {island_number} at ({tile_x},{tile_y})"
    ));
    true
}
