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
use anno_sim::data_bridge;
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
    MissingFertility {
        required: anno_formats::szs::Fertility,
    },
    NotCoastal,
    NoIslandMap,
    NoBuildingSelected,
    /// Bauinfra gate: player's highest-populated tier is below the
    /// building's `min_tier` (manual sec. 6.7.1).
    WrongTier {
        needed: u8,
        have: u8,
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

pub fn missing_required_fertility(
    def: &BuildingDef,
    island: &Island,
) -> Option<anno_formats::szs::Fertility> {
    let required = def.required_fertility?;
    (!data_bridge::island_can_host_building(def, island)).then_some(required)
}

/// Attempt to place building definition `def_index` at `(tile_x, tile_y)`
/// on `current_island` for `owner`. Mirrors the original click-place flow:
/// fertility gate, fishery coast gate, walkability, gold cost, materials
/// trickle. Side-effecting helper used by the game's click handler, its
/// drag-place loop, and command replay.
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

    let isl = &islands[current_island];
    if let Some(required) = missing_required_fertility(def, isl) {
        return PlaceOutcome::MissingFertility { required };
    }

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
    // Bauinfra gate: building requires the player to have at
    // least `min_tier` population in the matching tier or
    // higher. Manual sec. 6.7.1: civilization-level governs
    // which buildings unlock.
    if def.min_tier > 0 && owner_idx < sim.players.len() {
        let p = &sim.players[owner_idx];
        let highest = (0..p.population.len() as u8)
            .filter(|&t| p.population[t as usize] > 0)
            .max()
            .unwrap_or(0);
        if highest < def.min_tier {
            return PlaceOutcome::WrongTier {
                needed: def.min_tier,
                have: highest,
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
    let source_map_owner_slot = islands[current_island]
        .tiles
        .iter()
        .find(|tile| tile.x as i32 == tile_x && tile.y as i32 == tile_y)
        .map(|tile| tile.source_owner())
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
        _ => sim.apply_command(cmd),
    }
}
