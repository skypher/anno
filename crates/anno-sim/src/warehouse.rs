//! Warehouse and marketplace inventory management.
//!
//! Ported from DAT_005a6c18 (warehouse data, 0x12 bytes × 0xa0 entries)
//! and DAT_0055eb80 (marketplace data, 0x30 bytes × 0x120 entries).
//!
//! Each island has one main warehouse (Kontor) that stores all goods.
//! Marketplaces extend the service radius but share the warehouse inventory.

use crate::types::Good;
use std::collections::HashMap;

fn default_capacity_fallback() -> u16 {
    30
}

/// Source storage capacity for the first player-built Kontor.
/// `haeuser.cod` Nr=271 (`Bauinfra: INFRA_KONTOR_1`,
/// `ProdKind: KONTOR`) carries `Maxlager: 50`.
pub const BASE_KONTOR_CAPACITY: u16 = 50;

/// All authored Kontor `Maxlager` values from `haeuser.cod`, in
/// Nummer order 271..=277. The first three are the upgrade ladder;
/// 274..=277 are small/special variants with 20 t per good.
pub const KONTOR_MAXLAGER_VALUES: [u16; 7] = [50, 75, 100, 20, 20, 20, 20];

/// A warehouse on an island, tracking inventory for one player.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Warehouse {
    pub island_id: u8,
    pub owner: u8,
    pub tile_x: u16,
    pub tile_y: u16,
    pub active: bool,

    /// Default per-good cap when a good has no entry yet in
    /// `inventory`. New warehouses use the base Kontor `Maxlager`;
    /// loaded scenario Kontors can override this with their authored
    /// definition, and old save files without the field deserialize
    /// through the legacy serde fallback.
    #[serde(default = "default_capacity_fallback")]
    pub default_capacity: u16,

    /// Inventory: good → (current_stock, max_capacity)
    inventory: HashMap<Good, (u16, u16)>,

    /// Per-good buy/sell sliders (Anno 1602 manual section 8.1).
    /// `Sell` = "everything left of the mark stays, everything right
    /// of it gets sold to the free trader" → `min_keep` (we sell down
    /// to this floor).
    /// `Buy` = "the trader keeps selling you the chosen product up to
    /// the desired amount" → `max_buy` (we buy up to this ceiling).
    /// Defaults: no slider configured = no trade with the free trader
    /// for that good (matching the original — players have to set
    /// each slider explicitly).
    #[serde(default)]
    sliders: HashMap<Good, TradeSlider>,
}

/// Per-good free-trader sliders. See `Warehouse::sliders` doc-comment.
///
/// Manual section 8.1 lets the player drag *two* sliders per
/// storeroom: one for the price and one for the quantity. We model
/// both: the quantity sliders gate which goods the trader trades and
/// at what threshold; the price overrides nudge the player's
/// effective buy/sell rates above or below the standard
/// `prices::price_of` values.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct TradeSlider {
    /// Sell-to-trader floor: keep at least this many in stock.
    /// `None` = don't sell this good.
    pub sell_min_keep: Option<u16>,
    /// Buy-from-trader ceiling: top up to at most this many in stock.
    /// `None` = don't buy this good.
    pub buy_max_stock: Option<u16>,
    /// Optional override for the price the player asks when selling
    /// this good to the trader. `None` = use `prices::price_of`. The
    /// trader will only buy when this is at-or-below its standard
    /// sell price (otherwise it walks away).
    pub sell_price: Option<i32>,
    /// Optional override for the price the player offers when buying
    /// this good from the trader. `None` = use `prices::price_of`.
    /// The trader will only sell when this is at-or-above its
    /// standard buy price.
    pub buy_price: Option<i32>,
}

impl Warehouse {
    pub fn new(island_id: u8, owner: u8, tile_x: u16, tile_y: u16) -> Self {
        Self::with_capacity(island_id, owner, tile_x, tile_y, BASE_KONTOR_CAPACITY)
    }

    /// Construct a warehouse with an explicit per-good
    /// default capacity (sourced from the matching Kontor's
    /// `Maxlager` in haeuser.cod when loading a scenario).
    pub fn with_capacity(
        island_id: u8,
        owner: u8,
        tile_x: u16,
        tile_y: u16,
        default_capacity: u16,
    ) -> Self {
        Self {
            island_id,
            owner,
            tile_x,
            tile_y,
            active: true,
            default_capacity,
            inventory: HashMap::new(),
            sliders: HashMap::new(),
        }
    }

    /// Slider configuration for a good (default = no trade).
    pub fn slider(&self, good: Good) -> TradeSlider {
        self.sliders.get(&good).copied().unwrap_or_default()
    }

    /// Set the sell-to-trader floor. Pass `None` to disable selling.
    pub fn set_sell_min_keep(&mut self, good: Good, min_keep: Option<u16>) {
        let s = self.sliders.entry(good).or_default();
        s.sell_min_keep = min_keep;
    }

    /// Set the buy-from-trader ceiling. Pass `None` to disable buying.
    pub fn set_buy_max_stock(&mut self, good: Good, max_stock: Option<u16>) {
        let s = self.sliders.entry(good).or_default();
        s.buy_max_stock = max_stock;
    }

    /// Player's asking price when selling `good` to the trader.
    /// `None` clears the override.
    pub fn set_sell_price(&mut self, good: Good, price: Option<i32>) {
        let s = self.sliders.entry(good).or_default();
        s.sell_price = price;
    }

    /// Player's offered price when buying `good` from the trader.
    /// `None` clears the override.
    pub fn set_buy_price(&mut self, good: Good, price: Option<i32>) {
        let s = self.sliders.entry(good).or_default();
        s.buy_price = price;
    }

    /// How much of `good` we are willing to sell to the trader right
    /// now (current stock minus the slider floor; 0 if no slider set).
    pub fn sell_offer(&self, good: Good) -> u16 {
        let floor = match self.slider(good).sell_min_keep {
            Some(f) => f,
            None => return 0,
        };
        self.stock(good).saturating_sub(floor)
    }

    /// How much of `good` we want to buy from the trader right now
    /// (slider ceiling minus current stock; 0 if no slider set).
    pub fn buy_demand(&self, good: Good) -> u16 {
        let ceiling = match self.slider(good).buy_max_stock {
            Some(c) => c,
            None => return 0,
        };
        ceiling.saturating_sub(self.stock(good))
    }

    /// Get current stock of a good.
    pub fn stock(&self, good: Good) -> u16 {
        self.inventory.get(&good).map(|&(s, _)| s).unwrap_or(0)
    }

    /// Get maximum capacity for a good.
    pub fn capacity(&self, good: Good) -> u16 {
        self.inventory
            .get(&good)
            .map(|&(_, c)| c)
            .unwrap_or(self.default_capacity)
    }

    /// Deposit goods into the warehouse. Returns amount actually deposited.
    pub fn deposit(&mut self, good: Good, amount: u16) -> u16 {
        let cap = self.default_capacity;
        let entry = self.inventory.entry(good).or_insert((0, cap));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kontor_capacities_match_haeuser_cod_audit() {
        assert_eq!(BASE_KONTOR_CAPACITY, 50);
        assert_eq!(KONTOR_MAXLAGER_VALUES, [50, 75, 100, 20, 20, 20, 20]);
    }

    #[test]
    fn default_constructor_uses_base_kontor_capacity() {
        let mut wh = Warehouse::new(0, 0, 10, 10);

        assert_eq!(wh.default_capacity, BASE_KONTOR_CAPACITY);
        assert_eq!(wh.capacity(Good::Wood), BASE_KONTOR_CAPACITY);
        assert_eq!(wh.deposit(Good::Wood, 99), BASE_KONTOR_CAPACITY);
        assert_eq!(wh.stock(Good::Wood), BASE_KONTOR_CAPACITY);
    }

    #[test]
    fn explicit_default_capacity_clamps_new_goods() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, BASE_KONTOR_CAPACITY);

        assert_eq!(wh.capacity(Good::Wood), BASE_KONTOR_CAPACITY);
        assert_eq!(wh.deposit(Good::Wood, 99), BASE_KONTOR_CAPACITY);
        assert_eq!(wh.stock(Good::Wood), BASE_KONTOR_CAPACITY);
        assert_eq!(wh.capacity(Good::Wood), BASE_KONTOR_CAPACITY);
    }
}
