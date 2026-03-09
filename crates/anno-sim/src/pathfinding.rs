//! A* pathfinding with 8-directional movement.
//!
//! Ported from FUN_0046c7d0 (core A* pathfinding in original binary).
//! Uses priority queue with 8-direction neighbor expansion.
//! Diagonal moves cost ~1.41× orthogonal (approximated as 14 vs 10).

use crate::island_map::IslandMap;
use std::collections::BinaryHeap;
use std::cmp::Ordering;

/// Cost of orthogonal movement (×10 for integer arithmetic).
const COST_ORTHO: u32 = 10;
/// Cost of diagonal movement (≈√2 × 10).
const COST_DIAG: u32 = 14;

/// Maximum nodes to explore before giving up.
const MAX_ITERATIONS: u32 = 10_000;

/// 8-directional neighbor offsets: N, NE, E, SE, S, SW, W, NW
const DIRS: [(i32, i32); 8] = [
    (0, -1),  // N
    (1, -1),  // NE
    (1, 0),   // E
    (1, 1),   // SE
    (0, 1),   // S
    (-1, 1),  // SW
    (-1, 0),  // W
    (-1, -1), // NW
];

/// A* search node.
#[derive(Clone, Eq, PartialEq)]
struct Node {
    pos: (i32, i32),
    g_cost: u32,
    f_cost: u32,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap
        other.f_cost.cmp(&self.f_cost)
            .then_with(|| other.g_cost.cmp(&self.g_cost))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Find a path from start to goal on the island map.
/// Returns a list of tile positions (excluding start, including goal),
/// or None if no path exists.
pub fn find_path(
    map: &IslandMap,
    start: (i32, i32),
    goal: (i32, i32),
) -> Option<Vec<(i32, i32)>> {
    if start == goal {
        return Some(Vec::new());
    }

    // Goal must be walkable (or we find the nearest walkable tile)
    if !map.is_walkable(goal.0, goal.1) {
        // Try to find nearest walkable tile to goal
        if let Some(alt_goal) = find_nearest_walkable(map, goal) {
            return find_path_inner(map, start, alt_goal);
        }
        return None;
    }

    find_path_inner(map, start, goal)
}

fn find_path_inner(
    map: &IslandMap,
    start: (i32, i32),
    goal: (i32, i32),
) -> Option<Vec<(i32, i32)>> {
    let w = map.width as usize;
    let h = map.height as usize;
    let size = w * h;

    // Cost grid (u32::MAX = unvisited)
    let mut g_costs = vec![u32::MAX; size];
    // Parent direction (which direction we came FROM, 0xFF = no parent)
    let mut came_from = vec![0xFFu8; size];

    let start_idx = start.1 as usize * w + start.0 as usize;
    g_costs[start_idx] = 0;

    let mut open = BinaryHeap::new();
    open.push(Node {
        pos: start,
        g_cost: 0,
        f_cost: heuristic(start, goal),
    });

    let mut iterations = 0u32;

    while let Some(current) = open.pop() {
        iterations += 1;
        if iterations > MAX_ITERATIONS {
            return None; // Give up — path too complex
        }

        if current.pos == goal {
            return Some(reconstruct_path(&came_from, w, start, goal));
        }

        let cur_idx = current.pos.1 as usize * w + current.pos.0 as usize;

        // Skip if we already found a better path to this node
        if current.g_cost > g_costs[cur_idx] {
            continue;
        }

        // Explore 8 neighbors
        for (dir_idx, &(dx, dy)) in DIRS.iter().enumerate() {
            let nx = current.pos.0 + dx;
            let ny = current.pos.1 + dy;

            if !map.is_walkable(nx, ny) {
                continue;
            }

            // For diagonal moves, check that both adjacent orthogonal tiles are walkable
            // (prevents cutting through corners)
            if dx != 0 && dy != 0 {
                if !map.is_walkable(current.pos.0 + dx, current.pos.1)
                    || !map.is_walkable(current.pos.0, current.pos.1 + dy)
                {
                    continue;
                }
            }

            let move_cost = if dx != 0 && dy != 0 { COST_DIAG } else { COST_ORTHO };
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

    None // No path found
}

/// Octile distance heuristic (admissible for 8-direction movement).
fn heuristic(a: (i32, i32), b: (i32, i32)) -> u32 {
    let dx = (a.0 - b.0).unsigned_abs();
    let dy = (a.1 - b.1).unsigned_abs();
    let (min, max) = if dx < dy { (dx, dy) } else { (dy, dx) };
    // min diagonal moves + (max - min) orthogonal moves
    min * COST_DIAG + (max - min) * COST_ORTHO
}

/// Reconstruct path by walking backwards from goal to start using came_from directions.
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
            break; // Should not happen if path was found
        }
        // Reverse the direction to get where we came from
        let (dx, dy) = DIRS[dir as usize];
        pos = (pos.0 - dx, pos.1 - dy);
    }

    path.reverse();
    path
}

/// Find the nearest walkable tile to a given position (spiral search).
fn find_nearest_walkable(map: &IslandMap, pos: (i32, i32)) -> Option<(i32, i32)> {
    for radius in 1i32..20 {
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                if dx.abs() != radius && dy.abs() != radius {
                    continue; // Only check perimeter
                }
                let nx = pos.0 + dx;
                let ny = pos.1 + dy;
                if map.is_walkable(nx, ny) {
                    return Some((nx, ny));
                }
            }
        }
    }
    None
}

/// Convert a direction index (0-7) to the compass direction byte used by Figure.
pub fn dir_index_to_compass(dir_idx: usize) -> u8 {
    // DIRS order: N=0, NE=1, E=2, SE=3, S=4, SW=5, W=6, NW=7
    // This matches the Figure direction encoding
    dir_idx as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line_path() {
        let map = IslandMap::new_open(0, 20, 20);
        let path = find_path(&map, (0, 0), (5, 0)).unwrap();
        assert_eq!(path.len(), 5);
        assert_eq!(path[0], (1, 0));
        assert_eq!(path[4], (5, 0));
    }

    #[test]
    fn diagonal_path() {
        let map = IslandMap::new_open(0, 20, 20);
        let path = find_path(&map, (0, 0), (5, 5)).unwrap();
        // Diagonal should be direct: 5 steps
        assert_eq!(path.len(), 5);
        assert_eq!(path[4], (5, 5));
    }

    #[test]
    fn path_around_obstacle() {
        let mut map = IslandMap::new_open(0, 10, 10);
        // Block a wall at x=5, y=0..8
        for y in 0..8 {
            map.set_walkable(5, y, false);
        }
        let path = find_path(&map, (3, 4), (7, 4));
        assert!(path.is_some());
        let path = path.unwrap();
        // Path should go around the wall (through y=8 gap)
        assert!(path.len() > 4); // Longer than straight line
        assert_eq!(*path.last().unwrap(), (7, 4));
        // Verify no step goes through the wall
        for &(x, y) in &path {
            assert!(map.is_walkable(x, y), "Path goes through blocked tile ({}, {})", x, y);
        }
    }

    #[test]
    fn no_path_exists() {
        let mut map = IslandMap::new_open(0, 10, 10);
        // Completely surround the goal
        for x in 4..=6 {
            for y in 4..=6 {
                if x != 5 || y != 5 {
                    map.set_walkable(x as u16, y as u16, false);
                }
            }
        }
        let path = find_path(&map, (0, 0), (5, 5));
        // Should find nearest walkable or return None
        // Since goal is walkable but surrounded, path should be None
        assert!(path.is_none());
    }

    #[test]
    fn same_start_and_goal() {
        let map = IslandMap::new_open(0, 10, 10);
        let path = find_path(&map, (5, 5), (5, 5)).unwrap();
        assert!(path.is_empty());
    }
}
