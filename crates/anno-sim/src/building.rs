//! Building definitions and instances.
//!
//! Ported from the building definition table at DAT_00619b60 (136 bytes each)
//! and the active building array at PTR_DAT_0049aebc (20-byte stride, 1037 entries).

use crate::types::{Good, ProductionType};

/// Maximum active building instances.
pub const MAX_BUILDINGS: usize = 1037;

/// Building definition (loaded from haeuser.cod).
#[derive(Debug, Clone)]
pub struct BuildingDef {
    pub id: u16,
    pub category: u8,
    pub width: u8,
    pub height: u8,
    pub production_type: ProductionType,
    /// Building kind from COD (BODEN, GEBAEUDE, HQ, etc.)
    pub kind: String,
    /// Production kind from COD (HANDWERK, MARKT, KONTOR, KIRCHE, etc.)
    pub prod_kind: String,
    /// Service radius in tiles (0 = no service area).
    /// Used by marketplaces (extend warehouse access), churches, taverns, etc.
    pub radius: u16,
    pub output_good: Good,
    pub input_good_1: Good,
    pub input_good_2: Good,
    pub output_rate: u16,
    pub input_1_rate: u16,
    pub input_2_rate: u16,
    pub storage_capacity: u16,
    pub cycle_time_ms: u32,
    pub carrier_interval_ms: u32,
    pub cost_gold: u32,
    pub cost_tools: u16,
    pub cost_wood: u16,
    pub cost_bricks: u16,
    pub maintenance_cost: u16,
}

/// An active building instance in the world.
#[derive(Debug, Clone)]
pub struct BuildingInstance {
    pub def_id: u16,
    pub island_id: u8,
    pub tile_x: u16,
    pub tile_y: u16,
    pub owner: u8,
    pub active: bool,

    /// Production efficiency (0-128 scale, 128 = 100%).
    pub efficiency: u8,

    /// Current stock levels.
    pub input_1_stock: u16,
    pub input_2_stock: u16,
    pub output_stock: u16,

    /// Production timer (counts down from cycle_time).
    pub production_timer_ms: u32,

    /// Accumulated production work.
    pub total_work: u16,
}

impl BuildingInstance {
    pub fn new(def_id: u16, island_id: u8, tile_x: u16, tile_y: u16, owner: u8) -> Self {
        Self {
            def_id,
            island_id,
            tile_x,
            tile_y,
            owner,
            active: true,
            efficiency: 0,
            input_1_stock: 0,
            input_2_stock: 0,
            output_stock: 0,
            production_timer_ms: 0,
            total_work: 0,
        }
    }
}
