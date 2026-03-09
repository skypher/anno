//! Service coverage system for marketplaces and public buildings.
//!
//! In Anno 1602, buildings must be within the service radius of a
//! marketplace or warehouse (Kontor) to function. The Kontor provides
//! base coverage; each marketplace extends the service area further.
//!
//! The coverage grid tracks which tiles on an island are "serviced" —
//! i.e., within the radius of at least one marketplace or warehouse.
//! Population happiness also depends on being covered by public
//! buildings like churches, taverns, schools, etc.
//!
//! Service building types and their effects:
//!   KONTOR (warehouse) — base coverage, goods access
//!   MARKT (marketplace) — extended coverage, goods access
//!   KAPELLE/KIRCHE — religious satisfaction
//!   WIRT — tavern satisfaction
//!   SCHULE/HOCHSCHULE — education satisfaction
//!   KLINIK — health satisfaction
//!   THEATER/BADEHAUS — entertainment satisfaction
//!
//! Coverage is recomputed periodically (on the market timer, ~1000ms).

use crate::building::{BuildingDef, BuildingInstance};

/// A per-island coverage bitmap.
/// Tracks which tiles are within service radius of a marketplace/warehouse.
#[derive(Debug, Clone)]
pub struct CoverageMap {
    pub island_id: u8,
    pub width: u16,
    pub height: u16,
    /// Bitfield: true if tile is within marketplace/warehouse service area.
    market_coverage: Vec<bool>,
    /// Count of public buildings covering each tile (for satisfaction bonus).
    public_coverage: Vec<u8>,
}

impl CoverageMap {
    pub fn new(island_id: u8, width: u16, height: u16) -> Self {
        let size = width as usize * height as usize;
        Self {
            island_id,
            width,
            height,
            market_coverage: vec![false; size],
            public_coverage: vec![0; size],
        }
    }

    /// Check if a tile position is within marketplace/warehouse coverage.
    pub fn is_covered(&self, x: u16, y: u16) -> bool {
        if x < self.width && y < self.height {
            self.market_coverage[y as usize * self.width as usize + x as usize]
        } else {
            false
        }
    }

    /// Get the public building coverage count for a tile.
    pub fn public_coverage_at(&self, x: u16, y: u16) -> u8 {
        if x < self.width && y < self.height {
            self.public_coverage[y as usize * self.width as usize + x as usize]
        } else {
            0
        }
    }

    /// Recompute coverage from building instances and definitions.
    pub fn recompute(
        &mut self,
        buildings: &[BuildingInstance],
        defs: &[BuildingDef],
        warehouses: &[(u16, u16, u16)], // (tile_x, tile_y, radius)
    ) {
        // Clear
        self.market_coverage.fill(false);
        self.public_coverage.fill(0);

        // Apply warehouse coverage
        for &(wx, wy, radius) in warehouses {
            self.apply_radius(wx, wy, radius, true, false);
        }

        // Apply building coverage
        for inst in buildings {
            if inst.island_id != self.island_id || !inst.active {
                continue;
            }
            let def_idx = inst.def_id as usize;
            if def_idx >= defs.len() {
                continue;
            }
            let def = &defs[def_idx];
            if def.radius == 0 {
                continue;
            }

            let is_market = matches!(def.prod_kind.as_str(), "MARKT" | "KONTOR");
            let is_public = matches!(
                def.prod_kind.as_str(),
                "KIRCHE" | "KAPELLE" | "WIRT" | "SCHULE" | "HOCHSCHULE"
                    | "KLINIK" | "THEATER" | "BADEHAUS" | "BRUNNEN"
                    | "GALGEN" | "DENKMAL" | "SCHLOSS" | "TRIUMPH"
            );

            if is_market || is_public {
                // Use center of building as source
                let cx = inst.tile_x + def.width as u16 / 2;
                let cy = inst.tile_y + def.height as u16 / 2;
                self.apply_radius(cx, cy, def.radius, is_market, is_public);
            }
        }
    }

    fn apply_radius(
        &mut self,
        cx: u16,
        cy: u16,
        radius: u16,
        is_market: bool,
        is_public: bool,
    ) {
        let r = radius as i32;
        let w = self.width as i32;
        let h = self.height as i32;

        for dy in -r..=r {
            for dx in -r..=r {
                // Use Manhattan distance (diamond shape) matching original game
                if dx.abs() + dy.abs() > r {
                    continue;
                }
                let tx = cx as i32 + dx;
                let ty = cy as i32 + dy;
                if tx >= 0 && tx < w && ty >= 0 && ty < h {
                    let idx = ty as usize * self.width as usize + tx as usize;
                    if is_market {
                        self.market_coverage[idx] = true;
                    }
                    if is_public {
                        self.public_coverage[idx] = self.public_coverage[idx].saturating_add(1);
                    }
                }
            }
        }
    }
}

/// Compute the fraction of residential buildings on an island that are
/// within marketplace/warehouse coverage. Returns 0-128 scale.
pub fn compute_market_coverage_ratio(
    coverage: &CoverageMap,
    buildings: &[BuildingInstance],
    defs: &[BuildingDef],
) -> u8 {
    let mut total_residential = 0u32;
    let mut covered_residential = 0u32;

    for inst in buildings {
        if inst.island_id != coverage.island_id || !inst.active {
            continue;
        }
        let def_idx = inst.def_id as usize;
        if def_idx >= defs.len() {
            continue;
        }
        let def = &defs[def_idx];

        // Count residential buildings (WOHNUNG ProdKind)
        if def.prod_kind == "WOHNUNG" {
            total_residential += 1;
            if coverage.is_covered(inst.tile_x, inst.tile_y) {
                covered_residential += 1;
            }
        }
    }

    if total_residential == 0 {
        128 // No residences = full coverage (nothing to cover)
    } else {
        ((covered_residential * 128) / total_residential).min(128) as u8
    }
}

/// Compute public building satisfaction bonus for an island.
/// Returns 0-128: how well the island is covered by public buildings.
pub fn compute_public_satisfaction(
    coverage: &CoverageMap,
    buildings: &[BuildingInstance],
    defs: &[BuildingDef],
) -> u8 {
    let mut total_residential = 0u32;
    let mut coverage_sum = 0u32;

    for inst in buildings {
        if inst.island_id != coverage.island_id || !inst.active {
            continue;
        }
        let def_idx = inst.def_id as usize;
        if def_idx >= defs.len() {
            continue;
        }
        let def = &defs[def_idx];

        if def.prod_kind == "WOHNUNG" {
            total_residential += 1;
            let pc = coverage.public_coverage_at(inst.tile_x, inst.tile_y) as u32;
            // Each public building type contributes; cap per-tile at 5
            coverage_sum += pc.min(5);
        }
    }

    if total_residential == 0 {
        128
    } else {
        // Average coverage per residential building, scaled: 5 buildings = full satisfaction
        let avg = (coverage_sum * 128) / (total_residential * 5);
        avg.min(128) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::BuildingInstance;
    use crate::types::{Good, ProductionType};

    fn make_market_def() -> BuildingDef {
        BuildingDef {
            id: 467,
            category: 0,
            width: 4,
            height: 3,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "MARKT".into(),
            radius: 30,
            output_good: Good::None,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 0,
            carrier_interval_ms: 0,
            cost_gold: 200,
            cost_tools: 4,
            cost_wood: 10,
            cost_bricks: 19,
            maintenance_cost: 0,
        }
    }

    fn make_house_def() -> BuildingDef {
        BuildingDef {
            id: 413,
            category: 0,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(),
            prod_kind: "WOHNUNG".into(),
            radius: 6,
            output_good: Good::None,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 0,
            carrier_interval_ms: 0,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
        }
    }

    #[test]
    fn warehouse_provides_base_coverage() {
        let mut map = CoverageMap::new(0, 50, 50);
        map.recompute(&[], &[], &[(25, 25, 22)]);

        // Center should be covered
        assert!(map.is_covered(25, 25));
        // Within Manhattan radius 22
        assert!(map.is_covered(25 + 10, 25 + 10)); // dist=20, covered
        assert!(!map.is_covered(25 + 15, 25 + 15)); // dist=30, not covered
    }

    #[test]
    fn marketplace_extends_coverage() {
        let defs = vec![make_market_def(), make_house_def()];
        let marketplace = BuildingInstance::new(0, 0, 40, 25, 0); // def 0 = market
        let buildings = vec![marketplace];

        let mut map = CoverageMap::new(0, 80, 50);
        // Warehouse at (10,25) with radius 22 — covers up to x=32
        map.recompute(&buildings, &defs, &[(10, 25, 22)]);

        // Warehouse covers its area
        assert!(map.is_covered(10, 25));
        // Marketplace at (40,25) with radius 30 extends far
        assert!(map.is_covered(60, 25)); // within market's radius
    }

    #[test]
    fn uncovered_houses_reduce_ratio() {
        let defs = vec![make_market_def(), make_house_def()];
        let house_near = BuildingInstance::new(1, 0, 12, 12, 0); // covered
        let house_far = BuildingInstance::new(1, 0, 45, 45, 0); // not covered
        let buildings = vec![house_near, house_far];

        let mut map = CoverageMap::new(0, 50, 50);
        map.recompute(&buildings, &defs, &[(10, 10, 10)]);

        let ratio = compute_market_coverage_ratio(&map, &buildings, &defs);
        // 1 of 2 houses covered = 64/128
        assert_eq!(ratio, 64);
    }
}
