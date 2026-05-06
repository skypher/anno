//! Island walkability grid.
//!
//! Builds a per-island tile grid marking which tiles are walkable.
//! Used by the A* pathfinder for carrier routing.
//!
//! Walkability rules (from original game analysis):
//! - Terrain tiles (grass, sand, etc.) are walkable
//! - Roads are walkable
//! - Buildings block movement (their full tile footprint)
//! - Water/coast tiles are not walkable
//! - Warehouse tiles are walkable (carriers need to reach them)

use anno_formats::cod::BuildingDef as CodBuilding;
use anno_formats::szs::Island;
use std::collections::HashSet;

/// Walkability grid for a single island.
#[derive(Debug, Clone)]
pub struct IslandMap {
    pub island_id: u8,
    pub width: u16,
    pub height: u16,
    /// Flat grid: true = walkable. Index = y * width + x.
    walkable: Vec<bool>,
    /// Per-tile road flag. Carriers crossing a road tile pay a
    /// reduced path cost so the A* pathfinder prefers roads when
    /// they're available — RE: haeuser.cod `Wegspeed: 145, 120,
    /// 170, 100` (plain ground vs road quad).
    road: Vec<bool>,
    /// Eight raw INSEL5 fertility bytes carried over from the
    /// scenario file. Use `active_fertilities()` to iterate the
    /// non-sentinel typed values.
    pub fertilities: [u8; 8],
}

/// Building kinds that represent walkable terrain or roads.
const WALKABLE_KINDS: &[&str] = &[
    "BODEN",      // Ground/terrain
    "STRASSE",    // Road
    "STRANDMUND", // Beach mouth
    "STRAND",     // Beach
    "MEER",       // Sea (for coastal)
    "FLUSS",      // River
    "MAUER",      // Wall sections can be walked on
    "MAUERSTRAND",
    "PLATZ",      // Plaza/square
    "TOR",        // Gate
];

impl IslandMap {
    /// Build a walkability map from island tile data and building definitions.
    pub fn from_island(island: &Island, cod_buildings: &[CodBuilding]) -> Self {
        let width = island.width as u16;
        let height = island.height as u16;
        let size = width as usize * height as usize;

        // Start with all tiles as non-walkable (water/empty)
        let mut walkable = vec![false; size];
        let mut road = vec![false; size];

        // Warehouse positions — always walkable
        let mut warehouse_tiles: HashSet<(u8, u8)> = HashSet::new();

        // Process each tile record
        for tile in &island.tiles {
            let x = tile.x as u16;
            let y = tile.y as u16;
            if x >= width || y >= height {
                continue;
            }

            let idx = y as usize * width as usize + x as usize;

            // Look up building definition
            let building_id = tile.building_id as usize;
            if building_id < cod_buildings.len() {
                let def = &cod_buildings[building_id];
                let kind = def.kind.as_str();

                if is_walkable_kind(kind) {
                    walkable[idx] = true;
                    if kind == "STRASSE" {
                        road[idx] = true;
                    }
                } else if kind == "KONTOR" {
                    // Warehouses: mark walkable so carriers can reach them
                    walkable[idx] = true;
                    warehouse_tiles.insert((tile.x, tile.y));
                }
                // Everything else (buildings, resources) stays blocked
            } else {
                // Unknown building_id — assume terrain is walkable if it has a tile record
                // (islands only have records for land tiles, not water)
                walkable[idx] = true;
            }
        }

        Self {
            island_id: island.number,
            width,
            height,
            walkable,
            road,
            fertilities: island.fertilities,
        }
    }

    /// Active (non-sentinel) fertilities decoded into the typed
    /// `Fertility` enum. Mirrors `Island::active_fertilities`
    /// but on the simulation-side map so callers don't need to
    /// keep the original `Island` around.
    pub fn active_fertilities(&self) -> Vec<anno_formats::szs::Fertility> {
        self.fertilities.iter()
            .filter_map(|&b| anno_formats::szs::Fertility::from_byte(b))
            .collect()
    }

    /// Check if a tile is walkable.
    #[inline]
    pub fn is_walkable(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return false;
        }
        self.walkable[y as usize * self.width as usize + x as usize]
    }

    /// True if `(x, y)` is a road tile. Used by the pathfinder to
    /// favour road-following routes (RE: haeuser.cod `Wegspeed`
    /// quads — plain-ground 145/120 off-road vs 170/100 on-road).
    #[inline]
    pub fn is_road(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return false;
        }
        self.road[y as usize * self.width as usize + x as usize]
    }

    /// Mark a tile as walkable (e.g., for warehouse placement after map creation).
    pub fn set_walkable(&mut self, x: u16, y: u16, val: bool) {
        if x < self.width && y < self.height {
            self.walkable[y as usize * self.width as usize + x as usize] = val;
        }
    }

    /// A tile is "coastal" if it's walkable AND at least one of its
    /// 4-neighbours is out of bounds — i.e. it sits on the perimeter of
    /// the island's walkable region. Fisheries can only go on coastal tiles.
    pub fn is_coastal(&self, x: i32, y: i32) -> bool {
        if !self.is_walkable(x, y) { return false; }
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            if nx < 0 || ny < 0
                || nx >= self.width as i32 || ny >= self.height as i32
            {
                return true;
            }
        }
        false
    }

    /// True iff the entire `w × h` footprint anchored at `(x, y)` is walkable.
    pub fn can_fit(&self, x: i32, y: i32, w: u16, h: u16) -> bool {
        if x < 0 || y < 0 { return false; }
        if (x as u32) + w as u32 > self.width as u32 { return false; }
        if (y as u32) + h as u32 > self.height as u32 { return false; }
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                if !self.is_walkable(x + dx, y + dy) {
                    return false;
                }
            }
        }
        true
    }

    /// Spiral search around `(cx, cy)` (warehouse, typically) for the first
    /// position where `w × h` fits. Returns top-left tile or None.
    pub fn find_open_spot(
        &self, cx: u16, cy: u16, w: u16, h: u16, max_radius: u16,
    ) -> Option<(u16, u16)> {
        for r in 0..=max_radius as i32 {
            for dy in -r..=r {
                for dx in -r..=r {
                    // Only the ring at the current radius (skip interior).
                    if dx.abs() != r && dy.abs() != r { continue; }
                    let x = cx as i32 + dx;
                    let y = cy as i32 + dy;
                    if self.can_fit(x, y, w, h) {
                        return Some((x as u16, y as u16));
                    }
                }
            }
        }
        None
    }

    /// Like `find_open_spot`, but additionally rejects candidates that
    /// aren't reachable from `anchor` via the walkability graph (BFS).
    /// `anchor` is normally the warehouse tile that will service the new
    /// building. Use this when placement requires a carrier route home.
    pub fn find_reachable_spot(
        &self, cx: u16, cy: u16, w: u16, h: u16, max_radius: u16,
        anchor: (u16, u16),
    ) -> Option<(u16, u16)> {
        for r in 0..=max_radius as i32 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r { continue; }
                    let x = cx as i32 + dx;
                    let y = cy as i32 + dy;
                    if !self.can_fit(x, y, w, h) { continue; }
                    if self.reachable(
                        (anchor.0 as i32, anchor.1 as i32),
                        (x, y),
                    ) {
                        return Some((x as u16, y as u16));
                    }
                }
            }
        }
        None
    }

    /// 4-directional BFS reachability check; cap node visits so a giant
    /// island doesn't make this dominate the AI tick. Returns true when
    /// `goal` is connected to `start` via walkable tiles. If `start`
    /// itself isn't walkable (e.g. it's a warehouse anchor whose footprint
    /// is non-walkable), falls back to the nearest walkable 4-neighbour.
    pub fn reachable(&self, start: (i32, i32), goal: (i32, i32)) -> bool {
        if start == goal { return true; }
        let start = if self.is_walkable(start.0, start.1) {
            start
        } else {
            let mut alt = None;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = (start.0 + dx, start.1 + dy);
                if self.is_walkable(n.0, n.1) { alt = Some(n); break; }
            }
            match alt { Some(p) => p, None => return false }
        };
        if !self.is_walkable(goal.0, goal.1) { return false; }
        const NODE_BUDGET: usize = 4_096;
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        visited.insert(start);
        queue.push_back(start);
        while let Some((x, y)) = queue.pop_front() {
            if visited.len() > NODE_BUDGET { return false; }
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let n = (x + dx, y + dy);
                if !self.is_walkable(n.0, n.1) { continue; }
                if !visited.insert(n) { continue; }
                if n == goal { return true; }
                queue.push_back(n);
            }
        }
        false
    }

    /// Create an empty map (all walkable) for testing or when no tile data is available.
    pub fn new_open(island_id: u8, width: u16, height: u16) -> Self {
        let size = width as usize * height as usize;
        Self {
            island_id,
            width,
            height,
            walkable: vec![true; size],
            road: vec![false; size],
            fertilities: [7; 8],
        }
    }

    /// Mark a tile as a road (test helper / explicit road overlay).
    pub fn set_road(&mut self, x: u16, y: u16, val: bool) {
        if x < self.width && y < self.height {
            self.road[y as usize * self.width as usize + x as usize] = val;
        }
    }
}

fn is_walkable_kind(kind: &str) -> bool {
    WALKABLE_KINDS.iter().any(|&k| kind == k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_map_all_walkable() {
        let map = IslandMap::new_open(0, 10, 10);
        assert!(map.is_walkable(0, 0));
        assert!(map.is_walkable(9, 9));
        assert!(!map.is_walkable(-1, 0));
        assert!(!map.is_walkable(10, 0));
    }

    #[test]
    fn set_walkable() {
        let mut map = IslandMap::new_open(0, 10, 10);
        assert!(map.is_walkable(5, 5));
        map.set_walkable(5, 5, false);
        assert!(!map.is_walkable(5, 5));
    }

    #[test]
    fn can_fit_respects_bounds_and_blocks() {
        let mut map = IslandMap::new_open(0, 10, 10);
        assert!(map.can_fit(0, 0, 3, 3));
        assert!(!map.can_fit(8, 8, 3, 3)); // would overflow
        map.set_walkable(2, 2, false);
        assert!(!map.can_fit(0, 0, 3, 3)); // blocker hits the footprint
        assert!(map.can_fit(3, 3, 3, 3));  // clear away from blocker
    }

    #[test]
    fn coastal_tiles_are_on_the_perimeter() {
        let map = IslandMap::new_open(0, 5, 5);
        // Corners and edges are coastal.
        assert!(map.is_coastal(0, 0));
        assert!(map.is_coastal(2, 0));
        assert!(map.is_coastal(4, 4));
        // Interior tile is not.
        assert!(!map.is_coastal(2, 2));
    }

    #[test]
    fn reachable_via_walkable_path() {
        let map = IslandMap::new_open(0, 10, 10);
        assert!(map.reachable((0, 0), (9, 9)));
    }

    #[test]
    fn reachable_blocked_by_wall() {
        let mut map = IslandMap::new_open(0, 10, 10);
        // Vertical wall at x=5 cuts the map in two.
        for y in 0..10u16 {
            map.set_walkable(5, y, false);
        }
        assert!(!map.reachable((0, 0), (9, 9)));
        assert!(map.reachable((0, 0), (4, 4)));
    }

    #[test]
    fn find_reachable_spot_avoids_isolated_island() {
        let mut map = IslandMap::new_open(0, 10, 10);
        // Cut off the right half so reachable spots only exist on the left.
        for y in 0..10u16 { map.set_walkable(5, y, false); }
        // Anchor at (1,1). Tile (8,8) is walkable but unreachable; the
        // reachable variant should pick a left-side spot instead.
        let spot = map.find_reachable_spot(8, 8, 1, 1, 9, (1, 1)).unwrap();
        assert!((spot.0 as i32) < 5, "got unreachable spot {:?}", spot);
    }

    #[test]
    fn find_open_spot_spirals_around_blocked_center() {
        let mut map = IslandMap::new_open(0, 10, 10);
        // Wall the 2x2 around (5,5).
        for x in 4..=6 {
            for y in 4..=6 {
                map.set_walkable(x, y, false);
            }
        }
        let pos = map.find_open_spot(5, 5, 2, 2, 5).expect("found spot");
        // Picked footprint must be walkable.
        assert!(map.can_fit(pos.0 as i32, pos.1 as i32, 2, 2));
        // And must lie outside the blocked block.
        let in_blocked = (pos.0 as i32) >= 3 && (pos.0 as i32) <= 6
            && (pos.1 as i32) >= 3 && (pos.1 as i32) <= 6;
        assert!(!in_blocked, "got {:?} which overlaps the 2x2 wall", pos);
    }
}
