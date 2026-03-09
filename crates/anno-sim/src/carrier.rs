//! Carrier dispatching and movement.
//!
//! Carriers transport goods between production buildings and warehouses.
//! When a building's output stock exceeds half capacity, a carrier is
//! spawned to pick up goods and deliver them to the nearest warehouse.
//!
//! Carrier lifecycle:
//!   1. Spawned at production building (CarryingGoods action)
//!   2. Walks A* path to nearest warehouse on same island
//!   3. Deposits goods at warehouse
//!   4. Walks A* path back to production building (Returning action)
//!   5. Despawns when back at building

use crate::building::{BuildingDef, BuildingInstance};
use crate::entity::{ActionType, Figure};
use crate::island_map::IslandMap;
use crate::pathfinding;
use crate::types::Good;
use crate::warehouse::{self, Warehouse};

/// Carrier walking speed in sub-tiles per movement tick (100ms).
const CARRIER_SPEED: u16 = 4;

/// Try to spawn a carrier for a production building.
/// Returns Some(figure) if a carrier was created.
pub fn try_spawn_carrier(
    building: &mut BuildingInstance,
    def: &BuildingDef,
    warehouses: &[Warehouse],
    island_maps: &[IslandMap],
) -> Option<Figure> {
    if def.output_good == Good::None || def.storage_capacity == 0 {
        return None;
    }

    // Only spawn when output exceeds half capacity
    if building.output_stock <= def.storage_capacity / 2 {
        return None;
    }

    // Find nearest warehouse on same island
    let wh_idx = warehouse::find_nearest_warehouse(
        warehouses,
        building.island_id,
        building.owner,
        building.tile_x,
        building.tile_y,
    )?;

    let wh = &warehouses[wh_idx];

    // Compute A* path if island map is available
    let start = (building.tile_x as i32, building.tile_y as i32);
    let goal = (wh.tile_x as i32, wh.tile_y as i32);

    let path = if let Some(map) = island_maps.iter().find(|m| m.island_id == building.island_id) {
        pathfinding::find_path(map, start, goal).unwrap_or_else(|| direct_path(start, goal))
    } else {
        direct_path(start, goal)
    };

    // Pick up all output from the building
    let amount = building.output_stock;
    building.output_stock = 0;

    let mut carrier = Figure::new();
    carrier.action = ActionType::CarryingGoods;
    carrier.owner = building.owner;
    carrier.tile_x = building.tile_x as i32;
    carrier.tile_y = building.tile_y as i32;
    carrier.target_x = wh.tile_x as i32;
    carrier.target_y = wh.tile_y as i32;
    carrier.building_idx = 0; // Will be set by caller
    carrier.carried_good = def.output_good as u8;
    carrier.carried_amount = amount;
    carrier.speed = CARRIER_SPEED;
    carrier.path = path;
    carrier.path_idx = 0;

    Some(carrier)
}

/// Move a carrier one step along its path.
/// Returns true if the carrier reached its target.
pub fn step_carrier(figure: &mut Figure) -> bool {
    if figure.speed == 0 {
        return false;
    }

    // Follow pre-computed path
    if figure.path_idx < figure.path.len() {
        let (nx, ny) = figure.path[figure.path_idx];
        let dx = nx - figure.tile_x;
        let dy = ny - figure.tile_y;
        figure.direction = direction_from_delta(dx, dy);
        figure.tile_x = nx;
        figure.tile_y = ny;
        figure.path_idx += 1;

        figure.path_idx >= figure.path.len()
    } else {
        // Fallback: direct movement if no path
        let dx = figure.target_x - figure.tile_x;
        let dy = figure.target_y - figure.tile_y;

        if dx == 0 && dy == 0 {
            return true;
        }

        let step = 1i32;
        if dx.abs() >= dy.abs() {
            figure.tile_x += dx.signum() * step;
        } else {
            figure.tile_y += dy.signum() * step;
        }

        figure.direction = direction_from_delta(dx, dy);
        figure.tile_x == figure.target_x && figure.tile_y == figure.target_y
    }
}

/// Compute compass direction from delta.
fn direction_from_delta(dx: i32, dy: i32) -> u8 {
    match (dx.signum(), dy.signum()) {
        (0, -1) => 0,  // N
        (1, -1) => 1,  // NE
        (1, 0) => 2,   // E
        (1, 1) => 3,   // SE
        (0, 1) => 4,   // S
        (-1, 1) => 5,  // SW
        (-1, 0) => 6,  // W
        (-1, -1) => 7, // NW
        _ => 0,
    }
}

/// Process a carrier that has arrived at its destination.
/// Returns true if the carrier should be despawned.
pub fn handle_arrival(
    figure: &mut Figure,
    warehouses: &mut [Warehouse],
    buildings: &[BuildingInstance],
    island_maps: &[IslandMap],
) -> bool {
    match figure.action {
        ActionType::CarryingGoods => {
            // Find the warehouse at the target location
            if let Some(wh) = warehouses.iter_mut().find(|w| {
                w.tile_x == figure.target_x as u16 && w.tile_y == figure.target_y as u16
            }) {
                // Deposit goods
                let good = good_from_u8(figure.carried_good);
                let deposited = wh.deposit(good, figure.carried_amount);
                figure.carried_amount -= deposited;
            }

            // Return to source building
            if figure.building_idx < buildings.len() as u16 {
                let building = &buildings[figure.building_idx as usize];
                let start = (figure.tile_x, figure.tile_y);
                let goal = (building.tile_x as i32, building.tile_y as i32);

                // Compute return path
                let path = if let Some(map) = island_maps.iter().find(|m| m.island_id == building.island_id) {
                    pathfinding::find_path(map, start, goal).unwrap_or_else(|| direct_path(start, goal))
                } else {
                    direct_path(start, goal)
                };

                figure.target_x = building.tile_x as i32;
                figure.target_y = building.tile_y as i32;
                figure.action = ActionType::Returning;
                figure.carried_good = 0;
                figure.carried_amount = 0;
                figure.path = path;
                figure.path_idx = 0;
                false
            } else {
                true // No building to return to
            }
        }
        ActionType::Returning => {
            // Back at source building — despawn
            true
        }
        _ => true,
    }
}

/// Generate a direct path (no obstacles) from start to goal.
fn direct_path(start: (i32, i32), goal: (i32, i32)) -> Vec<(i32, i32)> {
    let mut path = Vec::new();
    let mut pos = start;

    while pos != goal {
        let dx = goal.0 - pos.0;
        let dy = goal.1 - pos.1;

        // Move diagonally when possible, otherwise axis-aligned
        let sx = dx.signum();
        let sy = dy.signum();

        if dx != 0 && dy != 0 {
            pos = (pos.0 + sx, pos.1 + sy);
        } else if dx != 0 {
            pos = (pos.0 + sx, pos.1);
        } else {
            pos = (pos.0, pos.1 + sy);
        }

        path.push(pos);
    }

    path
}

/// Convert Good u8 repr back to Good enum.
fn good_from_u8(val: u8) -> Good {
    match val {
        1 => Good::Wood,
        2 => Good::Iron,
        3 => Good::Gold,
        4 => Good::Wool,
        5 => Good::Sugar,
        6 => Good::Tobacco,
        7 => Good::Cattle,
        8 => Good::Grain,
        9 => Good::Flour,
        10 => Good::Tools,
        11 => Good::Bricks,
        12 => Good::Swords,
        13 => Good::Muskets,
        14 => Good::Cannons,
        15 => Good::Food,
        16 => Good::Cloth,
        17 => Good::Alcohol,
        18 => Good::TobaccoProducts,
        19 => Good::Spices,
        20 => Good::Cocoa,
        21 => Good::Grapes,
        22 => Good::Stone,
        23 => Good::Ore,
        24 => Good::GoldOre,
        25 => Good::Hides,
        26 => Good::Cotton,
        27 => Good::Silk,
        28 => Good::Jewelry,
        29 => Good::Clothing,
        30 => Good::Fish,
        _ => Good::None,
    }
}
