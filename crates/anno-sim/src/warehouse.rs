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

/// Per-tier warehouse storage capacity (tons per good).
/// RE: timhowgego.wordpress.com/anno_1602/gameplay/trade_diplomacy
/// — "Warehouse I: 30t, Warehouse II: 50t, Warehouse III: 75t,
/// Warehouse IV: 100t". Index by upgrade level (0..=3); the
/// default warehouse uses tier 0 (30t).
pub const WAREHOUSE_CAPACITIES: [u16; 4] = [30, 50, 75, 100];

/// A warehouse on an island, tracking inventory for one player.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Warehouse {
    pub island_id: u8,
    pub owner: u8,
    pub tile_x: u16,
    pub tile_y: u16,
    pub active: bool,

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
        Self {
            island_id,
            owner,
            tile_x,
            tile_y,
            active: true,
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
