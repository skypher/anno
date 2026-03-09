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

/// Building kinds that are explicitly blocked (buildings, resources, etc.)
const BLOCKED_KINDS: &[&str] = &[
    "HANDWERK",   // Production building
    "ROHSTOFF",   // Raw resource
    "PLANTAGE",   // Plantation
    "BERGWERK",   // Mine
    "WOHN",       // Residence
    "KONTOR",     // Trading post
    "MARKT",      // Market
    "TURM",       // Tower
    "BURG",       // Castle
    "KIRCHE",     // Church
    "HAFEN",      // Harbor
    "MILITAR",    // Military
    "STEINBRUCH", // Quarry
    "FISCHEREI",  // Fishery (building itself blocks)
];

impl IslandMap {
    /// Build a walkability map from island tile data and building definitions.
    pub fn from_island(island: &Island, cod_buildings: &[CodBuilding]) -> Self {
        let width = island.width as u16;
        let height = island.height as u16;
        let size = width as usize * height as usize;

        // Start with all tiles as non-walkable (water/empty)
        let mut walkable = vec![false; size];

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
        }
    }

    /// Check if a tile is walkable.
    #[inline]
    pub fn is_walkable(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return false;
        }
        self.walkable[y as usize * self.width as usize + x as usize]
    }

    /// Mark a tile as walkable (e.g., for warehouse placement after map creation).
    pub fn set_walkable(&mut self, x: u16, y: u16, val: bool) {
        if x < self.width && y < self.height {
            self.walkable[y as usize * self.width as usize + x as usize] = val;
        }
    }

    /// Create an empty map (all walkable) for testing or when no tile data is available.
    pub fn new_open(island_id: u8, width: u16, height: u16) -> Self {
        let size = width as usize * height as usize;
        Self {
            island_id,
            width,
            height,
            walkable: vec![true; size],
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
}
