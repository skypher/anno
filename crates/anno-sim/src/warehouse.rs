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

const fn default_source_footprint() -> (u8, u8) {
    (1, 1)
}

const fn default_source_path_class() -> u8 {
    32
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

    /// Oriented source-map footprint of this warehouse root. Scenario
    /// loading derives it from the KONTOR definition and INSELHAUS command;
    /// player-created roots retain the unit fallback until their command
    /// definition is attached.
    #[serde(default = "default_source_footprint")]
    pub source_footprint: (u8, u8),

    /// Low-seven-bit `Wegspeed[Speedtyp: 0]` class of this KONTOR root.
    /// The type-8 overlay writes this class below its callback bit.
    #[serde(default = "default_source_path_class")]
    pub source_path_class: u8,

    /// Default per-good cap when a good has no entry yet in
    /// `inventory`. New warehouses use the base Kontor `Maxlager`;
    /// loaded scenario Kontors can override this with their authored
    /// definition, and old save files without the field deserialize
    /// through the legacy serde fallback.
    #[serde(default = "default_capacity_fallback")]
    pub default_capacity: u16,

    /// Population of the city represented by this warehouse, ordered by the
    /// five source BGRUPPE tiers. STADT4 seeds this for authored settlements;
    /// player-created settlements begin at zero and update through their
    /// local city record.
    #[serde(default)]
    pub city_population: [u32; 5],

    /// Inventory: good → (current_stock, max_capacity)
    inventory: HashMap<Good, (u16, u16)>,

    /// Integral compatibility reservations for in-flight generic carriers.
    /// Exact source city-record reservations are retained separately below.
    #[serde(default)]
    reservations: HashMap<Good, u16>,

    /// Exact source city-record reservations at `ware * 0x0c + 0x00`.
    /// `FUN_0047d810` creates these while a type-8 carrier has committed a
    /// city-root load; `FUN_0047d640` releases them when that carrier collects
    /// or abandons the load. Unlike the compatibility reservation above, this
    /// map retains the source's 1/32-good amount used by `FUN_0047c080`.
    #[serde(default)]
    city_reserved_fixed: HashMap<Good, u16>,

    /// Exact 1/32-good city-store balances written by type-11 transfers.
    /// Ordinary warehouse stock remains integral and initializes a balance
    /// when a good has not yet been delivered by a city cart.
    #[serde(default)]
    city_fixed_inventory: HashMap<Good, u16>,

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
            source_footprint: default_source_footprint(),
            source_path_class: default_source_path_class(),
            default_capacity,
            city_population: [0; 5],
            inventory: HashMap::new(),
            reservations: HashMap::new(),
            city_reserved_fixed: HashMap::new(),
            city_fixed_inventory: HashMap::new(),
            sliders: HashMap::new(),
        }
    }

    /// Construct a source city store with its authored STADT4 population.
    pub fn with_capacity_and_population(
        island_id: u8,
        owner: u8,
        tile_x: u16,
        tile_y: u16,
        default_capacity: u16,
        city_population: [u32; 5],
    ) -> Self {
        let mut warehouse = Self::with_capacity(island_id, owner, tile_x, tile_y, default_capacity);
        warehouse.city_population = city_population;
        warehouse
    }

    /// Preserve the source command's oriented KONTOR footprint.
    pub fn set_source_footprint(&mut self, footprint: (u8, u8)) {
        self.source_footprint = (footprint.0.max(1), footprint.1.max(1));
    }

    /// Preserve the source command's compiled TRAEGER path class.
    pub fn set_source_path_class(&mut self, path_class: u8) {
        self.source_path_class = path_class & 0x7f;
    }

    /// Source-city shared storage capacity in the fixed 1/32-good scale.
    /// FUN_0047ab00 starts from the city's Kontor capacity and adds 320 for
    /// every additional active MARKT/KONTOR root after the first.
    pub fn city_storage_capacity_fixed(&self, transfer_root_count: usize) -> u32 {
        let additional_roots = transfer_root_count.saturating_sub(1) as u32;
        u32::from(self.default_capacity)
            .saturating_mul(32)
            .saturating_add(additional_roots.saturating_mul(320))
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

    /// Exact city-store balance for one good, in 1/32-good units.
    pub fn city_stock_fixed(&self, good: Good) -> u16 {
        self.city_fixed_inventory
            .get(&good)
            .copied()
            .unwrap_or_else(|| self.stock(good).saturating_mul(32))
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
        if let Some(fixed) = self.city_fixed_inventory.get_mut(&good) {
            *fixed = fixed.saturating_add(deposited.saturating_mul(32));
        }
        deposited
    }

    /// Store a city-cart delivery using FUN_0047aac0's shared city capacity.
    /// The source limit is common to every good slot rather than the
    /// warehouse UI's independent per-good slider ceiling.
    pub fn deposit_city_good(&mut self, good: Good, amount: u16, city_capacity_fixed: u32) -> u16 {
        (self.deposit_city_good_fixed(good, amount.saturating_mul(32), city_capacity_fixed) / 32)
            as u16
    }

    /// Store an exact type-11 cart delivery. The public stock value remains
    /// the floor of this source fixed-point balance.
    pub fn deposit_city_good_fixed(
        &mut self,
        good: Good,
        amount_fixed: u16,
        city_capacity_fixed: u32,
    ) -> u16 {
        let capacity_fixed = city_capacity_fixed.min(u32::from(u16::MAX)) as u16;
        let current_fixed = self.city_stock_fixed(good);
        let deposited = amount_fixed.min(capacity_fixed.saturating_sub(current_fixed));
        let updated_fixed = current_fixed.saturating_add(deposited);
        self.city_fixed_inventory.insert(good, updated_fixed);

        let city_capacity = capacity_fixed / 32;
        let entry = self.inventory.entry(good).or_insert((0, city_capacity));
        entry.1 = entry.1.max(city_capacity);
        entry.0 = updated_fixed / 32;
        deposited
    }

    /// Withdraw goods from the warehouse. Returns amount actually withdrawn.
    pub fn withdraw(&mut self, good: Good, amount: u16) -> u16 {
        if let Some(entry) = self.inventory.get_mut(&good) {
            let withdrawn = amount.min(entry.0);
            entry.0 -= withdrawn;
            if let Some(fixed) = self.city_fixed_inventory.get_mut(&good) {
                *fixed = fixed.saturating_sub(withdrawn.saturating_mul(32));
            }
            withdrawn
        } else {
            0
        }
    }

    /// Seed an authored scenario stock in 1/32-good units. The `KONTOR2`
    /// loader (`0x484230`) writes each record's `+0x0c` u16 directly into
    /// the runtime city ware entry's stock (`+6`) with no capacity clamp;
    /// the integral view keeps its floor. Raises the integral capacity to
    /// hold the seeded amount so display/deposit logic stays consistent.
    pub fn seed_city_stock_fixed(&mut self, good: Good, stock_fixed: u16) {
        self.city_fixed_inventory.insert(good, stock_fixed);
        let integral = stock_fixed / 32;
        let capacity = self.capacity(good).max(integral);
        let entry = self.inventory.entry(good).or_insert((0, capacity));
        entry.1 = entry.1.max(capacity);
        entry.0 = integral;
    }

    /// Withdraw an exact source city-store amount in 1/32-good units.
    /// `FUN_0047b160` uses this scale for kind-13 housing replacements after
    /// its caller has checked the city record's stock-minus-reserved balance.
    pub fn withdraw_city_good_fixed(&mut self, good: Good, amount_fixed: u16) -> u16 {
        let current_fixed = self.city_stock_fixed(good);
        let withdrawn = current_fixed.min(amount_fixed);
        let updated_fixed = current_fixed - withdrawn;
        self.city_fixed_inventory.insert(good, updated_fixed);

        let capacity = self.capacity(good);
        let entry = self.inventory.entry(good).or_insert((0, capacity));
        entry.0 = updated_fixed / 32;
        withdrawn
    }

    /// Amount currently committed out of a source city store, in 1/32-good
    /// units. Housing promotion subtracts this before testing its material
    /// requirements.
    pub fn city_reserved_fixed(&self, good: Good) -> u16 {
        self.city_reserved_fixed.get(&good).copied().unwrap_or(0)
    }

    /// Reserve an exact city-store amount for a source type-8 carrier.
    pub fn reserve_city_good_fixed(&mut self, good: Good, amount_fixed: u16) -> bool {
        let reserved = self.city_reserved_fixed(good);
        if amount_fixed == 0
            || self
                .city_stock_fixed(good)
                .saturating_sub(reserved)
                < amount_fixed
        {
            return false;
        }
        *self.city_reserved_fixed.entry(good).or_default() = reserved.saturating_add(amount_fixed);
        true
    }

    /// Release a source type-8 city-store commitment without changing stock.
    pub fn release_city_good_reservation_fixed(&mut self, good: Good, amount_fixed: u16) {
        if let Some(reserved) = self.city_reserved_fixed.get_mut(&good) {
            *reserved = reserved.saturating_sub(amount_fixed);
        }
    }

    /// Reserve available stock for an in-flight carrier without withdrawing it.
    pub fn reserve(&mut self, good: Good, amount: u16) -> bool {
        if amount == 0 || self.stock(good).saturating_sub(self.reserved(good)) < amount {
            return false;
        }
        *self.reservations.entry(good).or_default() += amount;
        true
    }

    /// Withdraw a previously reserved quantity at the supplier arrival.
    pub fn collect_reserved(&mut self, good: Good, amount: u16) -> bool {
        if amount == 0 || self.reserved(good) < amount || self.stock(good) < amount {
            return false;
        }
        *self.reservations.entry(good).or_default() -= amount;
        self.withdraw(good, amount) == amount
    }

    /// Release a reservation when the outbound leg cannot return to its root.
    pub fn release_reservation(&mut self, good: Good, amount: u16) {
        if let Some(reserved) = self.reservations.get_mut(&good) {
            *reserved = reserved.saturating_sub(amount);
        }
    }

    /// Current reservation for one good.
    pub fn reserved(&self, good: Good) -> u16 {
        self.reservations.get(&good).copied().unwrap_or(0)
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
        assert_eq!(wh.source_footprint, (1, 1));
        assert_eq!(wh.source_path_class, 32);
    }

    #[test]
    fn source_footprint_preserves_oriented_nonzero_dimensions() {
        let mut wh = Warehouse::new(0, 0, 10, 10);
        wh.set_source_footprint((3, 2));
        assert_eq!(wh.source_footprint, (3, 2));
        wh.set_source_footprint((0, 0));
        assert_eq!(wh.source_footprint, (1, 1));
    }

    #[test]
    fn source_path_class_retains_only_the_source_grid_cost_bits() {
        let mut wh = Warehouse::new(0, 0, 10, 10);
        wh.set_source_path_class(0xff);
        assert_eq!(wh.source_path_class, 0x7f);
    }

    #[test]
    fn explicit_default_capacity_clamps_new_goods() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, BASE_KONTOR_CAPACITY);

        assert_eq!(wh.capacity(Good::Wood), BASE_KONTOR_CAPACITY);
        assert_eq!(wh.deposit(Good::Wood, 99), BASE_KONTOR_CAPACITY);
        assert_eq!(wh.stock(Good::Wood), BASE_KONTOR_CAPACITY);
        assert_eq!(wh.capacity(Good::Wood), BASE_KONTOR_CAPACITY);
    }

    #[test]
    fn city_delivery_uses_shared_city_capacity_not_ui_per_good_cap() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, 50);
        let city_capacity = wh.city_storage_capacity_fixed(2);

        assert_eq!(wh.deposit_city_good(Good::Cloth, 60, city_capacity), 60);
        assert_eq!(wh.stock(Good::Cloth), 60);
        assert_eq!(wh.capacity(Good::Cloth), 60);
    }

    #[test]
    fn city_delivery_retains_fractional_source_balance() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, 50);
        let city_capacity = wh.city_storage_capacity_fixed(1);

        assert_eq!(
            wh.deposit_city_good_fixed(Good::Cloth, 65, city_capacity),
            65
        );
        assert_eq!(wh.city_stock_fixed(Good::Cloth), 65);
        assert_eq!(wh.stock(Good::Cloth), 2);
        assert_eq!(wh.deposit(Good::Cloth, 1), 1);
        assert_eq!(wh.city_stock_fixed(Good::Cloth), 97);
    }

    #[test]
    fn city_fixed_withdrawal_preserves_the_source_fractional_balance() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, 50);
        let capacity = wh.city_storage_capacity_fixed(1);
        assert_eq!(wh.deposit_city_good_fixed(Good::Tools, 97, capacity), 97);

        assert_eq!(wh.withdraw_city_good_fixed(Good::Tools, 64), 64);
        assert_eq!(wh.city_stock_fixed(Good::Tools), 33);
        assert_eq!(wh.stock(Good::Tools), 1);
    }

    #[test]
    fn city_fixed_reservation_tracks_the_promotion_debit_field() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, 50);
        let capacity = wh.city_storage_capacity_fixed(1);
        assert_eq!(wh.deposit_city_good_fixed(Good::Bricks, 96, capacity), 96);

        assert!(wh.reserve_city_good_fixed(Good::Bricks, 64));
        assert_eq!(wh.city_reserved_fixed(Good::Bricks), 64);
        assert!(!wh.reserve_city_good_fixed(Good::Bricks, 33));
        wh.release_city_good_reservation_fixed(Good::Bricks, 32);
        assert_eq!(wh.city_reserved_fixed(Good::Bricks), 32);
    }

    #[test]
    fn city_reservations_are_per_good_and_withdraw_only_on_collection() {
        let mut wh = Warehouse::new(0, 0, 10, 10);
        wh.deposit(Good::Wood, 4);
        wh.deposit(Good::Iron, 4);

        assert!(wh.reserve(Good::Wood, 4));
        assert!(wh.reserve(Good::Iron, 4));
        assert_eq!(wh.stock(Good::Wood), 4);
        assert_eq!(wh.reserved(Good::Wood), 4);
        assert!(wh.collect_reserved(Good::Wood, 4));
        assert_eq!(wh.stock(Good::Wood), 0);
        assert_eq!(wh.stock(Good::Iron), 4);
        assert_eq!(wh.reserved(Good::Iron), 4);
    }
}
