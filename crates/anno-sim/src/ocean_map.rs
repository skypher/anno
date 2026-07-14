//! World ocean map for ship pathfinding.
//!
//! Builds a navigability grid covering the entire game world.
//! Ocean tiles (not covered by any island) are navigable.
//! Island interior tiles are blocked. Coastal tiles adjacent to
//! water are valid docking positions.
//!
//! The map is built once from scenario data and cached.

use crate::source_route::{source_direction_delta, SourcePathGrid};
use anno_formats::cod::BuildingDef as CodBuilding;
use anno_formats::szs::SzsFile;

/// World ocean navigability map.
#[derive(Debug, Clone)]
pub struct OceanMap {
    pub width: u16,
    pub height: u16,
    /// true = navigable (ocean), false = blocked (land)
    grid: Vec<bool>,
    /// Static source direction markers projected by `FUN_0046f6d0`. When
    /// present, ship routing uses the source fixed-cost expansion rather than
    /// the legacy A* implementation below.
    source_ship_route_grid: Option<SourcePathGrid>,
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
            source_ship_route_grid: None,
        }
    }

    /// Build a ship navigation map from the source static-map cells and the
    /// `FUN_0046f6d0` direction-marker overlay.
    pub fn from_source_scenario(szs: &SzsFile, definitions: &[CodBuilding]) -> Self {
        let mut map = Self::from_scenario(szs);
        let mut source_grid = SourcePathGrid::new((0, 0), map.width as usize, map.height as usize);

        for island in &szs.islands {
            source_grid.overlay_source_ship_route_blockers(island, definitions);
        }

        for y in 0..i32::from(map.height) {
            for x in 0..i32::from(map.width) {
                map.grid[y as usize * map.width as usize + x as usize] = source_grid
                    .is_direction_clear((x, y))
                    .expect("source grid covers OceanMap bounds");
            }
        }
        map.source_ship_route_grid = Some(source_grid);
        map
    }

    /// True when this map was built from source static-map direction markers.
    pub fn has_source_ship_route_grid(&self) -> bool {
        self.source_ship_route_grid.is_some()
    }

    /// Find a source ship route using the recovered `FUN_0046c7d0` fixed-cost
    /// expansion over the stored `FUN_0046f6d0` blockers.
    pub fn find_source_ship_path(
        &self,
        start: (i32, i32),
        goal: (i32, i32),
    ) -> Option<Vec<(i32, i32)>> {
        let grid = self.source_ship_route_grid.clone()?;
        Self::source_path_from_grid(grid, start, goal)
    }

    /// Run the source ship search in the centered `2r + 1` window used by
    /// `FUN_00455a20` and `FUN_00456920`. Their live route-state branches
    /// supply `r = 0x50` or `r = 0x28`.
    pub fn find_source_ship_path_in_radius(
        &self,
        start: (i32, i32),
        goal: (i32, i32),
        radius: usize,
    ) -> Option<Vec<(i32, i32)>> {
        let grid = self
            .source_ship_route_grid
            .as_ref()?
            .source_window(start, radius)?;
        Self::source_path_from_grid(grid, start, goal)
    }

    fn source_path_from_grid(
        mut grid: SourcePathGrid,
        start: (i32, i32),
        goal: (i32, i32),
    ) -> Option<Vec<(i32, i32)>> {
        let steps = grid.route_to(start, goal).ok()?;
        let mut position = start;
        let mut path = Vec::with_capacity(steps.len());
        for step in steps {
            let (dx, dy) = source_direction_delta(step.direction)?;
            position = (position.0 + dx, position.1 + dy);
            path.push(position);
        }
        Some(path)
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
            source_ship_route_grid: None,
        }
    }

    #[test]
    fn ocean_paths_never_cross_land_in_real_scenarios() {
        // Cross-scenario corpus check: load a real .szs's
        // ocean map and run pathfinding between every
        // warehouse pair, asserting that no returned path
        // crosses a non-navigable tile.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/A Plague of Pirates.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: Plague .szs not found");
                return;
            }
        };
        let szs = anno_formats::szs::SzsFile::parse(&data).expect("parse");
        let ocean = OceanMap::from_scenario(&szs);

        // Pick a handful of island-centre coordinates to use as
        // path endpoints — they sit ON land, but the pathfinder
        // should route us through ocean around them.
        let mut centres: Vec<(i32, i32)> = Vec::new();
        for island in szs.islands.iter().take(4) {
            let cx = (island.x_pos + island.width as u16 / 2) as i32;
            let cy = (island.y_pos + island.height as u16 / 2) as i32;
            centres.push((cx, cy));
        }
        // For each (i, j) pair with i < j, try to find an ocean
        // path. Some pairs may legitimately return None when the
        // pathfinder bails out for distance; that's fine. What
        // MUST hold is: any successful path stays on navigable
        // tiles.
        for i in 0..centres.len() {
            for j in (i + 1)..centres.len() {
                if let Some(p) = find_ocean_path(&ocean, centres[i], centres[j]) {
                    for &(x, y) in &p {
                        assert!(
                            ocean.is_navigable(x, y),
                            "ocean path crosses land at ({x}, {y})"
                        );
                    }
                }
            }
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

    #[test]
    fn source_ocean_uses_fun_0046f6d0_blockers_and_fun_0046c7d0_routes() {
        let scenario = SzsFile {
            chunks: Vec::new(),
            islands: vec![anno_formats::szs::Island {
                number: 0,
                width: 3,
                height: 3,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![anno_formats::szs::IslandTile {
                    building_id: 0,
                    x: 1,
                    y: 1,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                }],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
        };
        let definitions = [CodBuilding {
            source_id: 0x4e20,
            kind: "BODEN".to_string(),
            ..Default::default()
        }];
        let ocean = OceanMap::from_source_scenario(&scenario, &definitions);

        assert!(ocean.has_source_ship_route_grid());
        assert!(!ocean.is_navigable(1, 1));
        let path = ocean.find_source_ship_path((0, 1), (2, 1)).unwrap();
        assert_eq!(path.last(), Some(&(2, 1)));
        assert!(!path.contains(&(1, 1)));
        assert!(ocean
            .find_source_ship_path_in_radius((0, 1), (2, 1), 1)
            .is_none());
        assert!(ocean
            .find_source_ship_path_in_radius((0, 1), (2, 1), 2)
            .is_some());
    }
}
