//! Warehouse and marketplace inventory management.
//!
//! Ported from DAT_005a6c18 (warehouse data, 0x12 bytes × 0xa0 entries)
//! and DAT_0055eb80 (marketplace data, 0x30 bytes × 0x120 entries).
//!
//! Each island has one main warehouse (Kontor) that stores all goods.
//! Marketplaces extend the service radius but share the warehouse inventory.

use crate::types::Good;
use std::collections::BTreeMap;

fn default_capacity_fallback() -> u16 {
    30
}

const fn default_source_footprint() -> (u8, u8) {
    (1, 1)
}

const fn default_source_path_class() -> u8 {
    32
}

/// A city always owns at least the root that created it, and
/// `FUN_0047ab00` subtracts one root's worth of capacity before adding
/// `city+0x20`. Counts 0 and 1 therefore describe the same store.
const fn default_city_transfer_root_count() -> u16 {
    1
}

/// The source's storage floor. `FUN_0047aa00` reports free space below this
/// as zero (`1602_exe.c:87421-87423`) and `FUN_0047aa30` refuses to hand out
/// a whole remaining stock below it (`:87442-87444`).
pub const SOURCE_STORE_FLOOR_FIXED: u32 = 0x20;

/// Source storage capacity for the first player-built Kontor.
/// `haeuser.cod` Nr=271 (`Bauinfra: INFRA_KONTOR_1`,
/// `ProdKind: KONTOR`) carries `Maxlager: 50`.
pub const BASE_KONTOR_CAPACITY: u16 = 50;

/// All authored Kontor `Maxlager` values from `haeuser.cod`, in
/// Nummer order 271..=277. The first three are the upgrade ladder
/// (`INFRA_KONTOR_1/2/3`, source ids 22103/22105/22107); 274..=277
/// (22121..=22124) are the **destroyed** Kontor variants — they carry
/// `Destroyflg: 1`, so `FUN_00481fc0` does not count them as storage
/// roots even though they still stamp their 20 t base into `city+0x20`.
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

    /// Source city record `+0x20`, in whole goods rather than the source's
    /// 1/32 scale. `FUN_00481ee0` (`1602_exe.c:93084`) rewrites it with the
    /// placed definition's compiled `Maxlager` on **every** KONTOR
    /// installation, so a `KONTOR_2`/`KONTOR_3` upgrade raises this base;
    /// [`Warehouse::city_storage_capacity_fixed`] adds it to the per-root
    /// term. It doubles as the per-good cap for a good with no `inventory`
    /// entry yet, and old save files without the field deserialize through
    /// the legacy serde fallback.
    #[serde(default = "default_capacity_fallback")]
    pub default_capacity: u16,

    /// Source city record `+0x1fa`: the number of live MARKT/KONTOR roots
    /// this city owns that are not themselves ruins. `FUN_00481fc0`
    /// (`:93194-93196`) increments it and `FUN_004820b0` (`:93216-93222`)
    /// decrements it under the predicate
    /// `(def[0x1c] == 7 || def[0x1c] == 8) && (def[0x6a] & 0x80) == 0`.
    ///
    /// Cached here, exactly as the source caches it on the city record, so
    /// that [`Warehouse::deposit`] can apply `FUN_0047ab00`'s capacity
    /// without reaching for the island's cell table.
    /// `Simulation::refresh_source_city_storage_roots` republishes it
    /// whenever a root is installed or released. Derived state, so it stays
    /// out of the save payload and is recomputed on load.
    #[serde(skip, default = "default_city_transfer_root_count")]
    pub city_transfer_root_count: u16,

    /// Population of the city represented by this warehouse, ordered by the
    /// five source BGRUPPE tiers. STADT4 seeds this for authored settlements;
    /// player-created settlements begin at zero and update through their
    /// local city record.
    #[serde(default)]
    pub city_population: [u32; 5],

    /// Inventory: good → (current_stock, max_capacity)
    inventory: BTreeMap<Good, (u16, u16)>,

    /// Integral compatibility reservations for in-flight generic carriers.
    /// Exact source city-record reservations are retained separately below.
    #[serde(default)]
    reservations: BTreeMap<Good, u16>,

    /// Exact source city-record reservations at `ware * 0x0c + 0x00`.
    /// `FUN_0047d810` creates these while a type-8 carrier has committed a
    /// city-root load; `FUN_0047d640` releases them when that carrier collects
    /// or abandons the load. Unlike the compatibility reservation above, this
    /// map retains the source's 1/32-good amount used by `FUN_0047c080`.
    #[serde(default)]
    city_reserved_fixed: BTreeMap<Good, u16>,

    /// Exact 1/32-good city-store balances written by type-11 transfers.
    /// Ordinary warehouse stock remains integral and initializes a balance
    /// when a good has not yet been delivered by a city cart.
    #[serde(default)]
    city_fixed_inventory: BTreeMap<Good, u16>,

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
    sliders: BTreeMap<Good, TradeSlider>,
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
            city_transfer_root_count: default_city_transfer_root_count(),
            city_population: [0; 5],
            inventory: BTreeMap::new(),
            reservations: BTreeMap::new(),
            city_reserved_fixed: BTreeMap::new(),
            city_fixed_inventory: BTreeMap::new(),
            sliders: BTreeMap::new(),
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

    /// `FUN_0047ab00` against this city's own cached root count.
    pub fn city_capacity_fixed(&self) -> u32 {
        self.city_storage_capacity_fixed(usize::from(self.city_transfer_root_count))
    }

    /// Republish the cached `city+0x1fa` storage-root count.
    pub fn set_city_transfer_root_count(&mut self, transfer_root_count: usize) {
        self.city_transfer_root_count = u16::try_from(transfer_root_count).unwrap_or(u16::MAX);
    }

    /// Adopt a KONTOR definition's compiled `Maxlager` as this city's base
    /// storage, the whole body of `FUN_00481ee0`'s first statement
    /// (`1602_exe.c:93084`, `city[0x20] = def[0x30]`). The argument is the
    /// compiled `<< 5` value; the port keeps `+0x20` in whole goods.
    ///
    /// Take it from `BuildingDef::storage_animation_capacity` and not from
    /// `properties["Maxlager"]` — a definition whose `Maxlager` lives only
    /// inside a nested `HAUS_PRODTYP` block never reaches the string map.
    pub fn set_source_storage_base_fixed(&mut self, maxlager_fixed: u16) {
        self.default_capacity = maxlager_fixed / 32;
    }

    /// `FUN_0047aa00` (`1602_exe.c:87414-87424`): the room one good has left
    /// in the shared city store. Every good is measured against the *whole*
    /// city capacity — goods do not compete for space — and a remainder
    /// below `0x20` reads as no room at all.
    pub fn city_free_space_fixed(&self, good: Good, city_capacity_fixed: u32) -> u32 {
        let free = city_capacity_fixed.saturating_sub(u32::from(self.city_stock_fixed(good)));
        if free < SOURCE_STORE_FLOOR_FIXED {
            0
        } else {
            free
        }
    }

    /// `FUN_0047aa30` (`1602_exe.c:87434-87447`): how much of `good` a
    /// withdrawal asking for `requested_fixed` may actually take. A request
    /// strictly below the balance is granted in full; otherwise the caller
    /// empties the slot, and a slot holding less than `0x20` yields nothing.
    pub fn city_available_stock_fixed(&self, good: Good, requested_fixed: u16) -> u16 {
        let stock = self.city_stock_fixed(good);
        if requested_fixed < stock {
            requested_fixed
        } else if stock >= SOURCE_STORE_FLOOR_FIXED as u16 {
            stock
        } else {
            0
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

    /// Deposit whole goods into the city store. Returns the amount actually
    /// deposited.
    ///
    /// The ceiling is `FUN_0047aa00`'s — the *whole* city capacity minus this
    /// good's own balance, per `FUN_0047ab00` — not the first Kontor's
    /// `Maxlager`. A city that owns a second MARKT or KONTOR root really does
    /// take 10 t more of every good, and every ship unload, free-trader
    /// exchange and wreck salvage in the port arrives through here.
    pub fn deposit(&mut self, good: Good, amount: u16) -> u16 {
        let city_capacity_fixed = self.city_capacity_fixed();
        let space = (self.city_free_space_fixed(good, city_capacity_fixed) / 32) as u16;
        let city_capacity = (city_capacity_fixed / 32).min(u32::from(u16::MAX)) as u16;
        let entry = self.inventory.entry(good).or_insert((0, city_capacity));
        entry.1 = entry.1.max(city_capacity);
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
        // `FUN_0047aac0` asks `FUN_0047aa00` for the room, so the `0x20`
        // floor applies: a store with fewer than 32 free units is full.
        let free_fixed = self.city_free_space_fixed(good, u32::from(capacity_fixed)) as u16;
        let deposited = amount_fixed.min(free_fixed);
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

    /// `FUN_00481450` case 8 → `FUN_00481ee0` (`1602_exe.c:93084`):
    /// `city[0x20] = def[0x30]`. The compiled `Maxlager` of the *placed*
    /// definition becomes the base, so the upgrade ladder 50 → 75 → 100
    /// raises it and a KONTOR_1 placed afterwards would lower it again.
    #[test]
    fn kontor_placement_restamps_the_city_storage_base() {
        let mut wh = Warehouse::new(0, 0, 10, 10);
        assert_eq!(wh.default_capacity, 50);
        assert_eq!(wh.city_capacity_fixed(), 1_600);

        // `INFRA_KONTOR_2`, haeuser.cod Nr 272 / source id 22105.
        wh.set_source_storage_base_fixed(75 << 5);
        assert_eq!(wh.default_capacity, 75);
        assert_eq!(wh.city_capacity_fixed(), 2_400);
        assert_eq!(wh.deposit(Good::Wood, 99), 75);

        // `INFRA_KONTOR_3`, Nr 273 / source id 22107.
        wh.set_source_storage_base_fixed(100 << 5);
        assert_eq!(wh.default_capacity, 100);
        assert_eq!(wh.city_capacity_fixed(), 3_200);
        assert_eq!(wh.deposit(Good::Wood, 99), 25);
        assert_eq!(wh.stock(Good::Wood), 100);
    }

    /// `FUN_0047ab00` (`:87510-87516`) is
    /// `city[0x1fa] * 0x140 - 0x140 + city[0x20]`, and `FUN_0047aac0` stores
    /// against exactly that. A second MARKT root is 10 t more room for
    /// *every* good, and `deposit` is the port's only ship-unload, salvage
    /// and free-trader entry point, so it has to see the same ceiling.
    #[test]
    fn deposit_honours_the_whole_city_capacity_not_the_kontor_base() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, 50);
        assert_eq!(wh.city_transfer_root_count, 1);
        assert_eq!(wh.deposit(Good::Cloth, 99), 50);

        // Two more MARKT roots: 1600 + 2 * 320 = 2240 fixed = 70 t.
        wh.set_city_transfer_root_count(3);
        assert_eq!(wh.city_capacity_fixed(), 2_240);
        assert_eq!(wh.deposit(Good::Cloth, 99), 20);
        assert_eq!(wh.stock(Good::Cloth), 70);
        assert_eq!(wh.capacity(Good::Cloth), 70);
        // Every good gets the whole capacity — they do not compete.
        assert_eq!(wh.deposit(Good::Tools, 99), 70);

        // And demolishing them takes the room back.
        wh.set_city_transfer_root_count(1);
        assert_eq!(wh.city_capacity_fixed(), 1_600);
        assert_eq!(wh.deposit(Good::Bricks, 99), 50);
    }

    /// `FUN_0047aa00` (`:87421-87423`): `if (free < 0x20) free = 0`.
    #[test]
    fn free_space_below_one_whole_good_reads_as_no_room() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, 50);
        let capacity = wh.city_capacity_fixed();
        assert_eq!(capacity, 1_600);

        // 1569 leaves 31/32 of a good free, which the source calls full.
        wh.seed_city_stock_fixed(Good::Cloth, 1_569);
        assert_eq!(wh.city_free_space_fixed(Good::Cloth, capacity), 0);
        assert_eq!(wh.deposit_city_good_fixed(Good::Cloth, 31, capacity), 0);
        assert_eq!(wh.city_stock_fixed(Good::Cloth), 1_569);

        // One unit less and the whole remainder is available again.
        wh.seed_city_stock_fixed(Good::Cloth, 1_568);
        assert_eq!(wh.city_free_space_fixed(Good::Cloth, capacity), 32);
        assert_eq!(wh.deposit_city_good_fixed(Good::Cloth, 99, capacity), 32);
        assert_eq!(wh.city_stock_fixed(Good::Cloth), 1_600);
    }

    /// `FUN_0047aa30` (`:87434-87447`): a request strictly below the balance
    /// is granted in full, a request that would empty the slot is granted
    /// only when the slot holds a whole good, and nothing else.
    #[test]
    fn stock_below_one_whole_good_yields_nothing_to_a_withdrawal() {
        let mut wh = Warehouse::with_capacity(0, 0, 10, 10, 50);

        wh.seed_city_stock_fixed(Good::Tools, 31);
        assert_eq!(wh.city_available_stock_fixed(Good::Tools, 31), 0);
        assert_eq!(wh.city_available_stock_fixed(Good::Tools, 99), 0);
        // A partial request is still served out of a sub-unit balance.
        assert_eq!(wh.city_available_stock_fixed(Good::Tools, 30), 30);

        wh.seed_city_stock_fixed(Good::Tools, 32);
        assert_eq!(wh.city_available_stock_fixed(Good::Tools, 32), 32);
        assert_eq!(wh.city_available_stock_fixed(Good::Tools, 99), 32);
        assert_eq!(wh.city_available_stock_fixed(Good::Tools, 20), 20);

        wh.seed_city_stock_fixed(Good::Tools, 0);
        assert_eq!(wh.city_available_stock_fixed(Good::Tools, 99), 0);
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
