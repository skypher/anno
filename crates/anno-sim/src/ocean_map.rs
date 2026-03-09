//! World ocean map for ship pathfinding.
//!
//! Builds a navigability grid covering the entire game world.
//! Ocean tiles (not covered by any island) are navigable.
//! Island interior tiles are blocked. Coastal tiles adjacent to
//! water are valid docking positions.
//!
//! The map is built once from scenario data and cached.

use anno_formats::szs::SzsFile;

/// World ocean navigability map.
#[derive(Debug, Clone)]
pub struct OceanMap {
    pub width: u16,
    pub height: u16,
    /// true = navigable (ocean), false = blocked (land)
    grid: Vec<bool>,
}

impl OceanMap {
    /// Build from scenario data. All tiles not occupied by islands are ocean.
    pub fn from_scenario(szs: &SzsFile) -> Self {
        // Determine world bounds
        let mut max_x: u16 = 0;
        let mut max_y: u16 = 0;
        for island in &szs.islands {
            let ex = island.x_pos + island.width as u16;
            let ey = island.y_pos + island.height as u16;
            if ex > max_x {
                max_x = ex;
            }
            if ey > max_y {
                max_y = ey;
            }
        }

        // Add margin for ships sailing around edges
        let width = (max_x + 10).min(500);
        let height = (max_y + 10).min(500);
        let size = width as usize * height as usize;

        // Start with all ocean
        let mut grid = vec![true; size];

        // Block island tiles
        for island in &szs.islands {
            // Block the entire island rectangle (conservative — island interiors are land)
            // Only tiles that have actual tile records are land; but for simplicity
            // and correctness, mark all tiles in the island bounding box as land
            // then unblock coastal tiles.
            for ly in 0..island.height as u16 {
                for lx in 0..island.width as u16 {
                    let wx = island.x_pos + lx;
                    let wy = island.y_pos + ly;
                    if wx < width && wy < height {
                        grid[wy as usize * width as usize + wx as usize] = false;
                    }
                }
            }
        }

        OceanMap {
            width,
            height,
            grid,
        }
    }

    /// Check if a world tile is navigable (ocean).
    pub fn is_navigable(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            false // Out of bounds = not navigable
        } else {
            self.grid[y as usize * self.width as usize + x as usize]
        }
    }

    /// Check if a world tile is land (blocked for ships).
    pub fn is_land(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            false
        } else {
            !self.grid[y as usize * self.width as usize + x as usize]
        }
    }

    /// Find the nearest navigable tile to a position (for ships docking near warehouses).
    /// Warehouses are on land — ships need to sail to the nearest ocean tile.
    pub fn nearest_navigable(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        if self.is_navigable(x, y) {
            return Some((x, y));
        }
        for radius in 1i32..30 {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs() != radius && dy.abs() != radius {
                        continue; // Only check perimeter
                    }
                    let nx = x + dx;
                    let ny = y + dy;
                    if self.is_navigable(nx, ny) {
                        return Some((nx, ny));
                    }
                }
            }
        }
        None
    }
}

/// 8-directional neighbor offsets.
const DIRS: [(i32, i32); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];

const COST_ORTHO: u32 = 10;
const COST_DIAG: u32 = 14;

/// Maximum nodes for ocean pathfinding (larger than land — ocean distances are bigger).
const MAX_OCEAN_ITERATIONS: u32 = 100_000;

/// A* node for ocean pathfinding.
#[derive(Clone, Eq, PartialEq)]
struct Node {
    pos: (i32, i32),
    g_cost: u32,
    f_cost: u32,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .f_cost
            .cmp(&self.f_cost)
            .then_with(|| other.g_cost.cmp(&self.g_cost))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Find an ocean path from start to goal (world coordinates).
/// Both start and goal should be navigable ocean tiles
/// (use nearest_navigable() to convert land positions first).
///
/// Returns path excluding start, including goal.
pub fn find_ocean_path(
    map: &OceanMap,
    start: (i32, i32),
    goal: (i32, i32),
) -> Option<Vec<(i32, i32)>> {
    if start == goal {
        return Some(Vec::new());
    }

    if !map.is_navigable(start.0, start.1) || !map.is_navigable(goal.0, goal.1) {
        return None;
    }

    let w = map.width as usize;
    let h = map.height as usize;
    let size = w * h;

    let mut g_costs = vec![u32::MAX; size];
    let mut came_from = vec![0xFFu8; size];

    let start_idx = start.1 as usize * w + start.0 as usize;
    g_costs[start_idx] = 0;

    let mut open = std::collections::BinaryHeap::new();
    open.push(Node {
        pos: start,
        g_cost: 0,
        f_cost: heuristic(start, goal),
    });

    let mut iterations = 0u32;

    while let Some(current) = open.pop() {
        iterations += 1;
        if iterations > MAX_OCEAN_ITERATIONS {
            return None;
        }

        if current.pos == goal {
            return Some(reconstruct_path(&came_from, w, start, goal));
        }

        let cur_idx = current.pos.1 as usize * w + current.pos.0 as usize;
        if current.g_cost > g_costs[cur_idx] {
            continue;
        }

        for (dir_idx, &(dx, dy)) in DIRS.iter().enumerate() {
            let nx = current.pos.0 + dx;
            let ny = current.pos.1 + dy;

            if !map.is_navigable(nx, ny) {
                continue;
            }

            // Diagonal corner-cutting prevention
            if dx != 0 && dy != 0 {
                if !map.is_navigable(current.pos.0 + dx, current.pos.1)
                    || !map.is_navigable(current.pos.0, current.pos.1 + dy)
                {
                    continue;
                }
            }

            let move_cost = if dx != 0 && dy != 0 {
                COST_DIAG
            } else {
                COST_ORTHO
            };
            let new_g = current.g_cost + move_cost;

            let n_idx = ny as usize * w + nx as usize;
            if new_g < g_costs[n_idx] {
                g_costs[n_idx] = new_g;
                came_from[n_idx] = dir_idx as u8;
                open.push(Node {
                    pos: (nx, ny),
                    g_cost: new_g,
                    f_cost: new_g + heuristic((nx, ny), goal),
                });
            }
        }
    }

    None
}

fn heuristic(a: (i32, i32), b: (i32, i32)) -> u32 {
    let dx = (a.0 - b.0).unsigned_abs();
    let dy = (a.1 - b.1).unsigned_abs();
    let (min, max) = if dx < dy { (dx, dy) } else { (dy, dx) };
    min * COST_DIAG + (max - min) * COST_ORTHO
}

fn reconstruct_path(
    came_from: &[u8],
    width: usize,
    start: (i32, i32),
    goal: (i32, i32),
) -> Vec<(i32, i32)> {
    let mut path = Vec::new();
    let mut pos = goal;

    while pos != start {
        path.push(pos);
        let idx = pos.1 as usize * width + pos.0 as usize;
        let dir = came_from[idx];
        if dir >= 8 {
            break;
        }
        let (dx, dy) = DIRS[dir as usize];
        pos = (pos.0 - dx, pos.1 - dy);
    }

    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_simple_ocean() -> OceanMap {
        // 20x20 ocean with a 5x5 island at (5,5)
        let mut grid = vec![true; 20 * 20];
        for y in 5..10 {
            for x in 5..10 {
                grid[y * 20 + x] = false;
            }
        }
        OceanMap {
            width: 20,
            height: 20,
            grid,
        }
    }

    #[test]
    fn ocean_path_around_island() {
        let map = make_simple_ocean();

        // Path from left of island to right of island
        let path = find_ocean_path(&map, (2, 7), (12, 7));
        assert!(path.is_some(), "Should find ocean path around island");
        let path = path.unwrap();

        // Path should avoid island interior
        for &(x, y) in &path {
            assert!(
                map.is_navigable(x, y),
                "Path goes through land at ({}, {})",
                x,
                y
            );
        }

        // Should reach the goal
        assert_eq!(*path.last().unwrap(), (12, 7));
    }

    #[test]
    fn ocean_direct_path() {
        let map = make_simple_ocean();

        // Path through open ocean (no obstacles)
        let path = find_ocean_path(&map, (0, 0), (3, 0));
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 3);
    }

    #[test]
    fn nearest_navigable_from_land() {
        let map = make_simple_ocean();

        // Island center at (7,7) is land — nearest ocean should be at edge
        let nav = map.nearest_navigable(7, 7);
        assert!(nav.is_some());
        let (nx, ny) = nav.unwrap();
        assert!(map.is_navigable(nx, ny));
    }

    #[test]
    fn ocean_already_navigable() {
        let map = make_simple_ocean();
        let nav = map.nearest_navigable(0, 0);
        assert_eq!(nav, Some((0, 0)));
    }
}
