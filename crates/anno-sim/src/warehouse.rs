//! Warehouse and marketplace inventory management.
//!
//! Ported from DAT_005a6c18 (warehouse data, 0x12 bytes × 0xa0 entries)
//! and DAT_0055eb80 (marketplace data, 0x30 bytes × 0x120 entries).
//!
//! Each island has one main warehouse (Kontor) that stores all goods.
//! Marketplaces extend the service radius but share the warehouse inventory.

use crate::types::Good;
use std::collections::HashMap;

/// Maximum goods slots per warehouse.
pub const MAX_GOOD_TYPES: usize = 32;

/// A warehouse on an island, tracking inventory for one player.
#[derive(Debug, Clone)]
pub struct Warehouse {
    pub island_id: u8,
    pub owner: u8,
    pub tile_x: u16,
    pub tile_y: u16,
    pub active: bool,

    /// Inventory: good → (current_stock, max_capacity)
    inventory: HashMap<Good, (u16, u16)>,
}

impl Warehouse {
    pub fn new(island_id: u8, owner: u8, tile_x: u16, tile_y: u16) -> Self {
        Self {
            island_id,
            owner,
            tile_x,
            tile_y,
            active: true,
            inventory: HashMap::new(),
        }
    }

    /// Get current stock of a good.
    pub fn stock(&self, good: Good) -> u16 {
        self.inventory.get(&good).map(|&(s, _)| s).unwrap_or(0)
    }

    /// Get maximum capacity for a good.
    pub fn capacity(&self, good: Good) -> u16 {
        self.inventory.get(&good).map(|&(_, c)| c).unwrap_or(30)
    }

    /// Deposit goods into the warehouse. Returns amount actually deposited.
    pub fn deposit(&mut self, good: Good, amount: u16) -> u16 {
        let entry = self.inventory.entry(good).or_insert((0, 30));
        let space = entry.1.saturating_sub(entry.0);
        let deposited = amount.min(space);
        entry.0 += deposited;
        deposited
    }

    /// Withdraw goods from the warehouse. Returns amount actually withdrawn.
    pub fn withdraw(&mut self, good: Good, amount: u16) -> u16 {
        if let Some(entry) = self.inventory.get_mut(&good) {
            let withdrawn = amount.min(entry.0);
            entry.0 -= withdrawn;
            withdrawn
        } else {
            0
        }
    }

    /// Set the capacity for a specific good.
    pub fn set_capacity(&mut self, good: Good, capacity: u16) {
        let entry = self.inventory.entry(good).or_insert((0, capacity));
        entry.1 = capacity;
    }

    /// Get all goods with non-zero stock.
    pub fn all_stock(&self) -> Vec<(Good, u16, u16)> {
        let mut result: Vec<_> = self
            .inventory
            .iter()
            .filter(|&(_, &(stock, _))| stock > 0)
            .map(|(&good, &(stock, cap))| (good, stock, cap))
            .collect();
        result.sort_by_key(|(g, _, _)| *g as u8);
        result
    }

    /// Squared tile distance to a position.
    pub fn distance_sq(&self, x: u16, y: u16) -> u32 {
        let dx = self.tile_x as i32 - x as i32;
        let dy = self.tile_y as i32 - y as i32;
        (dx * dx + dy * dy) as u32
    }
}

/// Find the nearest warehouse on the same island for a given player.
pub fn find_nearest_warehouse(
    warehouses: &[Warehouse],
    island_id: u8,
    owner: u8,
    tile_x: u16,
    tile_y: u16,
) -> Option<usize> {
    warehouses
        .iter()
        .enumerate()
        .filter(|(_, w)| w.active && w.island_id == island_id && w.owner == owner)
        .min_by_key(|(_, w)| w.distance_sq(tile_x, tile_y))
        .map(|(i, _)| i)
}
