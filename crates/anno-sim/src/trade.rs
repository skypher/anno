//! Trade route and ship trading system.
//!
//! Ported from the ship timer tick (1000ms) and trade route data structures.
//!
//! Trade routes define a sequence of stops (warehouses on different islands).
//! At each stop, the ship buys/sells specific goods according to the route config.
//! Ships navigate between stops automatically.
//!
//! Trade route lifecycle:
//!   1. Ship departs from current stop
//!   2. Travels toward next stop (1 tile per ship tick)
//!   3. Arrives at warehouse, loads/unloads goods
//!   4. Advances to next stop in route (loops back to first)

use crate::types::Good;
use crate::warehouse::Warehouse;

/// Maximum stops per trade route.
pub const MAX_ROUTE_STOPS: usize = 8;

/// Cargo units represented by one figuren.cod `Maxware` slot.
pub const CARGO_TONS_PER_MAXWARE: u16 = 10;

/// Source fallback for HANDEL1 (`Maxware: 4`).
pub const DEFAULT_SMALL_TRADER_CARGO_CAPACITY: u16 = 40;

/// Source fallback for HANDEL2 / HANDLER (`Maxware: 6`).
pub const DEFAULT_LARGE_TRADER_CARGO_CAPACITY: u16 = 60;

fn default_ship_cargo_capacity() -> u16 {
    DEFAULT_SMALL_TRADER_CARGO_CAPACITY
}

/// Trade-ship classes represented by `TradeShip`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TradeShipClass {
    SmallTrader,
    LargeTrader,
}

impl Default for TradeShipClass {
    fn default() -> Self {
        Self::SmallTrader
    }
}

/// Source-derived trade-ship cargo capacities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipCargoConfig {
    pub small_trader_capacity: u16,
    pub large_trader_capacity: u16,
    pub free_trader_capacity: u16,
}

impl Default for ShipCargoConfig {
    fn default() -> Self {
        Self {
            small_trader_capacity: DEFAULT_SMALL_TRADER_CARGO_CAPACITY,
            large_trader_capacity: DEFAULT_LARGE_TRADER_CARGO_CAPACITY,
            free_trader_capacity: DEFAULT_LARGE_TRADER_CARGO_CAPACITY,
        }
    }
}

impl ShipCargoConfig {
    pub fn from_figures(figures: &anno_formats::figuren::FiguresFile) -> Self {
        let default = Self::default();
        Self {
            small_trader_capacity: capacity_from_figure(
                figures,
                "HANDEL1",
                default.small_trader_capacity,
            ),
            large_trader_capacity: capacity_from_figure(
                figures,
                "HANDEL2",
                default.large_trader_capacity,
            ),
            free_trader_capacity: capacity_from_figure(
                figures,
                "HANDLER",
                default.free_trader_capacity,
            ),
        }
    }

    pub fn capacity_for(self, class: TradeShipClass) -> u16 {
        match class {
            TradeShipClass::SmallTrader => self.small_trader_capacity,
            TradeShipClass::LargeTrader => self.large_trader_capacity,
        }
    }
}

fn capacity_from_figure(
    figures: &anno_formats::figuren::FiguresFile,
    name: &str,
    fallback: u16,
) -> u16 {
    figures
        .find(name)
        .and_then(|def| u16::try_from(def.max_ware()).ok())
        .filter(|&maxware| maxware > 0)
        .and_then(|maxware| maxware.checked_mul(CARGO_TONS_PER_MAXWARE))
        .unwrap_or(fallback)
}

// Per-good buy/sell prices live in `crate::prices`. The flat constants that
// used to live here (8 sell / 6 buy) have been replaced with `price_of`.

/// A single stop in a trade route.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteStop {
    /// Target island ID.
    pub island_id: u8,
    /// Warehouse position on island.
    pub warehouse_x: u16,
    pub warehouse_y: u16,
    /// Goods to load (buy from warehouse) at this stop.
    pub load_goods: Vec<(Good, u16)>,
    /// Goods to unload (sell to warehouse) at this stop.
    pub unload_goods: Vec<Good>,
}

/// A trade route connecting multiple warehouses.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TradeRoute {
    pub id: u16,
    pub owner: u8,
    pub stops: Vec<RouteStop>,
    pub active: bool,
}

impl TradeRoute {
    pub fn new(id: u16, owner: u8) -> Self {
        Self {
            id,
            owner,
            stops: Vec::new(),
            active: false,
        }
    }

    pub fn add_stop(&mut self, stop: RouteStop) {
        if self.stops.len() < MAX_ROUTE_STOPS {
            self.stops.push(stop);
        }
    }

    pub fn activate(&mut self) {
        if self.stops.len() >= 2 {
            self.active = true;
        }
    }
}

/// A trading ship executing a route.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TradeShip {
    pub owner: u8,
    /// Authored ship name from SHIP4 when available.
    #[serde(default)]
    pub name: String,
    pub route_id: u16,
    /// Current position (world coordinates).
    pub world_x: i32,
    pub world_y: i32,
    /// Compass heading 0..8 (N, NE, E, SE, S, SW, W, NW), updated as the
    /// ship moves; used to pick the right rotation sprite at render time.
    #[serde(default)]
    pub heading: u8,
    /// Scenario-authored trader hull class. This is needed for source
    /// sprite identity (HANDEL1 vs HANDEL2), while cargo capacity stores
    /// the class's `Maxware × 10` hold size.
    #[serde(default)]
    pub class: TradeShipClass,
    /// Movement speed in tiles per ship tick.
    pub speed: u16,
    /// Current stop index in route.
    pub current_stop: usize,
    /// Ship state.
    pub state: ShipState,
    /// Cargo hold: (good, amount) pairs.
    pub cargo: Vec<(Good, u16)>,
    /// Total cargo currently loaded.
    pub cargo_total: u16,
    /// Per-instance cargo capacity in tons. Source: figuren.cod
    /// `Maxware × 10` for the ship's class.
    #[serde(default = "default_ship_cargo_capacity")]
    pub cargo_capacity: u16,
    /// Gold earned from trading.
    pub profit: i32,
    pub active: bool,
    /// Pre-computed ocean path (world coordinates).
    /// Empty means either the ship is docked, has no ocean map, or is
    /// waiting for a valid navigable route.
    pub path: Vec<(i32, i32)>,
    /// Current index into path.
    pub path_idx: usize,
}

/// Ship operating states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ShipState {
    /// Traveling toward next stop.
    Sailing,
    /// Loading/unloading at warehouse.
    Trading,
    /// Waiting (e.g., warehouse full).
    Waiting,
    /// Docked/idle.
    Idle,
}

impl TradeShip {
    pub fn new(owner: u8, route_id: u16, world_x: i32, world_y: i32) -> Self {
        Self::new_with_capacity(
            owner,
            route_id,
            world_x,
            world_y,
            DEFAULT_SMALL_TRADER_CARGO_CAPACITY,
        )
    }

    pub fn new_with_capacity(
        owner: u8,
        route_id: u16,
        world_x: i32,
        world_y: i32,
        cargo_capacity: u16,
    ) -> Self {
        Self::new_with_class(
            owner,
            route_id,
            world_x,
            world_y,
            TradeShipClass::SmallTrader,
            cargo_capacity,
        )
    }

    pub fn new_with_class(
        owner: u8,
        route_id: u16,
        world_x: i32,
        world_y: i32,
        class: TradeShipClass,
        cargo_capacity: u16,
    ) -> Self {
        Self {
            owner,
            name: String::new(),
            route_id,
            world_x,
            world_y,
            heading: 0,
            class,
            speed: 2,
            current_stop: 0,
            state: ShipState::Idle,
            cargo: Vec::new(),
            cargo_total: 0,
            cargo_capacity,
            profit: 0,
            active: true,
            path: Vec::new(),
            path_idx: 0,
        }
    }

    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }

    pub fn cargo_capacity(&self) -> u16 {
        if self.cargo_capacity == 0 {
            DEFAULT_SMALL_TRADER_CARGO_CAPACITY
        } else {
            self.cargo_capacity
        }
    }

    /// Get amount of a specific good in cargo.
    pub fn cargo_amount(&self, good: Good) -> u16 {
        self.cargo
            .iter()
            .filter(|(g, _)| *g == good)
            .map(|(_, a)| *a)
            .sum()
    }

    /// Add goods to cargo. Returns amount actually loaded.
    pub fn load(&mut self, good: Good, amount: u16) -> u16 {
        let space = self.cargo_capacity().saturating_sub(self.cargo_total);
        let loaded = amount.min(space);
        if loaded > 0 {
            if let Some(entry) = self.cargo.iter_mut().find(|(g, _)| *g == good) {
                entry.1 += loaded;
            } else {
                self.cargo.push((good, loaded));
            }
            self.cargo_total += loaded;
        }
        loaded
    }

    /// Remove goods from cargo. Returns amount actually unloaded.
    pub fn unload(&mut self, good: Good, amount: u16) -> u16 {
        if let Some(entry) = self.cargo.iter_mut().find(|(g, _)| *g == good) {
            let unloaded = amount.min(entry.1);
            entry.1 -= unloaded;
            self.cargo_total -= unloaded;
            unloaded
        } else {
            0
        }
    }

    /// Remove empty cargo entries.
    pub fn compact_cargo(&mut self) {
        self.cargo.retain(|(_, a)| *a > 0);
    }
}

/// Process one ship tick for a trade ship.
/// Returns gold earned/spent this tick.
/// ocean_map is optional — if provided, ships use A* ocean pathfinding.
pub fn tick_trade_ship(
    ship: &mut TradeShip,
    route: &TradeRoute,
    warehouses: &mut [Warehouse],
    ocean_map: Option<&crate::ocean_map::OceanMap>,
) -> i32 {
    if !ship.active || !route.active || route.stops.is_empty() {
        return 0;
    }

    let mut gold_delta = 0i32;

    match ship.state {
        ShipState::Idle => {
            // Start the route
            ship.current_stop = 0;
            ship.state = match compute_path_to_stop(ship, route, ocean_map) {
                RoutePathStatus::AlreadyAtStop => ShipState::Trading,
                _ => ShipState::Sailing,
            };
        }
        ShipState::Sailing => {
            if !ship.path.is_empty() && ship.path_idx < ship.path.len() {
                // Follow pre-computed ocean path
                let steps = ship.speed as usize;
                let prev_x = ship.world_x;
                let prev_y = ship.world_y;
                for _ in 0..steps {
                    if ship.path_idx >= ship.path.len() {
                        break;
                    }
                    let (nx, ny) = ship.path[ship.path_idx];
                    ship.world_x = nx;
                    ship.world_y = ny;
                    ship.path_idx += 1;
                }
                ship.heading =
                    compass_heading(ship.world_x - prev_x, ship.world_y - prev_y, ship.heading);

                // Check if we reached end of path (near destination)
                if ship.path_idx >= ship.path.len() {
                    ship.path.clear();
                    ship.path_idx = 0;
                    ship.state = ShipState::Trading;
                }
            } else {
                if ocean_map.is_some() {
                    match compute_path_to_stop(ship, route, ocean_map) {
                        RoutePathStatus::AlreadyAtStop => ship.state = ShipState::Trading,
                        RoutePathStatus::PathReady
                        | RoutePathStatus::Blocked
                        | RoutePathStatus::LegacyDirect => {}
                    }
                } else {
                    sail_direct_to_stop(ship, route);
                }
            }
        }
        ShipState::Trading => {
            let stop = &route.stops[ship.current_stop];

            // Find the warehouse at this stop
            if let Some(wh) = warehouses
                .iter_mut()
                .find(|w| w.island_id == stop.island_id && w.owner == ship.owner && w.active)
            {
                // Unload goods (sell to warehouse)
                for &good in &stop.unload_goods {
                    let amount = ship.cargo_amount(good);
                    if amount > 0 {
                        let deposited = wh.deposit(good, amount);
                        ship.unload(good, deposited);
                        gold_delta += deposited as i32 * crate::prices::price_of(good).sell;
                    }
                }

                // Load goods (buy from warehouse)
                for &(good, max_amount) in &stop.load_goods {
                    let available = wh.stock(good);
                    let to_load = max_amount.min(available);
                    if to_load > 0 {
                        let withdrawn = wh.withdraw(good, to_load);
                        let loaded = ship.load(good, withdrawn);
                        // Return excess to warehouse if ship is full
                        if loaded < withdrawn {
                            wh.deposit(good, withdrawn - loaded);
                        }
                        gold_delta -= loaded as i32 * crate::prices::price_of(good).buy;
                    }
                }
            }

            ship.compact_cargo();

            // Advance to next stop and compute ocean path
            ship.current_stop = (ship.current_stop + 1) % route.stops.len();
            ship.state = match compute_path_to_stop(ship, route, ocean_map) {
                RoutePathStatus::AlreadyAtStop => ShipState::Trading,
                _ => ShipState::Sailing,
            };
        }
        ShipState::Waiting => {
            // Re-check if we can trade
            ship.state = ShipState::Trading;
        }
    }

    ship.profit += gold_delta;
    gold_delta
}

fn sail_direct_to_stop(ship: &mut TradeShip, route: &TradeRoute) {
    let Some(stop) = route.stops.get(ship.current_stop) else {
        return;
    };
    let target_x = stop.warehouse_x as i32;
    let target_y = stop.warehouse_y as i32;

    let dx = target_x - ship.world_x;
    let dy = target_y - ship.world_y;

    if dx == 0 && dy == 0 {
        ship.state = ShipState::Trading;
    } else {
        let steps = ship.speed as i32;
        if dx.abs() > dy.abs() {
            ship.world_x += dx.signum() * steps.min(dx.abs());
        } else {
            ship.world_y += dy.signum() * steps.min(dy.abs());
        }
        ship.heading = compass_heading(dx, dy, ship.heading);
    }
}

/// Map a (dx, dy) movement vector to an 8-direction compass heading
/// (0=N, 1=NE, 2=E, 3=SE, 4=S, 5=SW, 6=W, 7=NW). Returns `prev` when
/// the delta is zero so a stalled ship keeps facing the same way.
pub(crate) fn compass_heading(dx: i32, dy: i32, prev: u8) -> u8 {
    if dx == 0 && dy == 0 {
        return prev;
    }
    let sx = dx.signum();
    let sy = dy.signum();
    match (sx, sy) {
        (0, -1) => 0,
        (1, -1) => 1,
        (1, 0) => 2,
        (1, 1) => 3,
        (0, 1) => 4,
        (-1, 1) => 5,
        (-1, 0) => 6,
        (-1, -1) => 7,
        _ => prev,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoutePathStatus {
    /// No ocean map was supplied; use legacy direct sailing.
    LegacyDirect,
    /// The ship is already at the navigable tile for the route stop.
    AlreadyAtStop,
    /// A non-empty ocean path was stored on the ship.
    PathReady,
    /// An ocean map exists, but no navigable route could be found.
    Blocked,
}

/// Compute an ocean A* path from ship's current position to the next route stop.
fn compute_path_to_stop(
    ship: &mut TradeShip,
    route: &TradeRoute,
    ocean_map: Option<&crate::ocean_map::OceanMap>,
) -> RoutePathStatus {
    ship.path.clear();
    ship.path_idx = 0;

    let ocean = match ocean_map {
        Some(m) => m,
        None => return RoutePathStatus::LegacyDirect,
    };

    if ship.current_stop >= route.stops.len() {
        return RoutePathStatus::Blocked;
    }
    let stop = &route.stops[ship.current_stop];
    let target_x = stop.warehouse_x as i32;
    let target_y = stop.warehouse_y as i32;

    // Find nearest navigable tiles to start and end
    let start = match ocean.nearest_navigable(ship.world_x, ship.world_y) {
        Some(p) => p,
        None => return RoutePathStatus::Blocked,
    };
    let goal = match ocean.nearest_navigable(target_x, target_y) {
        Some(p) => p,
        None => return RoutePathStatus::Blocked,
    };

    if start == goal {
        return RoutePathStatus::AlreadyAtStop;
    }

    let path = if ocean.has_source_ship_route_grid() {
        ocean.find_source_ship_path(start, goal)
    } else {
        crate::ocean_map::find_ocean_path(ocean, start, goal)
    };
    if let Some(path) = path {
        if path.is_empty() {
            RoutePathStatus::AlreadyAtStop
        } else {
            ship.path = path;
            ship.path_idx = 0;
            RoutePathStatus::PathReady
        }
    } else {
        RoutePathStatus::Blocked
    }
}

/// Free trader AI: finds profitable trades between warehouses.
/// Returns a trade action if one is found.
pub fn free_trader_find_trade(
    warehouses: &[Warehouse],
    ship_owner: u8,
    cargo_capacity: u16,
) -> Option<(usize, usize, Good, u16)> {
    // Find pairs of warehouses where one has excess and the other has deficit
    let owner_whs: Vec<(usize, &Warehouse)> = warehouses
        .iter()
        .enumerate()
        .filter(|(_, w)| w.owner == ship_owner && w.active)
        .collect();

    let mut best_trade: Option<(usize, usize, Good, u16, u16)> = None; // (from, to, good, amount, surplus)

    for &(i, wh_a) in &owner_whs {
        for &(j, wh_b) in &owner_whs {
            if i == j {
                continue;
            }

            // Check each good
            let stock_a = wh_a.all_stock();
            for (good, amount_a, _cap_a) in &stock_a {
                if *amount_a < 10 {
                    continue; // Not enough to trade
                }

                let amount_b = wh_b.stock(*good);
                if amount_b < 5 {
                    // Warehouse B needs this good
                    let surplus = amount_a - 5; // Keep 5 in source
                    let transfer = surplus.min(cargo_capacity);
                    if let Some(ref best) = best_trade {
                        if surplus > best.4 {
                            best_trade = Some((i, j, *good, transfer, surplus));
                        }
                    } else {
                        best_trade = Some((i, j, *good, transfer, surplus));
                    }
                }
            }
        }
    }

    best_trade.map(|(from, to, good, amount, _)| (from, to, good, amount))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_warehouse(island: u8, owner: u8, x: u16, y: u16) -> Warehouse {
        Warehouse::new(island, owner, x, y)
    }

    #[test]
    fn ship_cargo_config_reads_figuren_maxware() {
        let mut small = anno_formats::figuren::FigureDef::default();
        small.name = "HANDEL1".into();
        small.properties.insert("Maxware".into(), "4".into());
        let mut large = anno_formats::figuren::FigureDef::default();
        large.name = "HANDEL2".into();
        large.properties.insert("Maxware".into(), "6".into());
        let mut handler = anno_formats::figuren::FigureDef::default();
        handler.name = "HANDLER".into();
        handler.properties.insert("Maxware".into(), "6".into());
        let figures = anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: vec![small, large, handler],
        };

        let config = ShipCargoConfig::from_figures(&figures);

        assert_eq!(config.small_trader_capacity, 40);
        assert_eq!(config.large_trader_capacity, 60);
        assert_eq!(config.free_trader_capacity, 60);
    }

    #[test]
    fn compass_heading_8way() {
        assert_eq!(compass_heading(0, -1, 0), 0); // N
        assert_eq!(compass_heading(1, -1, 0), 1); // NE
        assert_eq!(compass_heading(1, 0, 0), 2); // E
        assert_eq!(compass_heading(1, 1, 0), 3); // SE
        assert_eq!(compass_heading(0, 1, 0), 4); // S
        assert_eq!(compass_heading(-1, 1, 0), 5); // SW
        assert_eq!(compass_heading(-1, 0, 0), 6); // W
        assert_eq!(compass_heading(-1, -1, 0), 7); // NW
                                                   // Stalled ships keep their previous heading
        assert_eq!(compass_heading(0, 0, 5), 5);
    }

    #[test]
    fn ship_cargo_load_unload() {
        let mut ship = TradeShip::new(0, 0, 0, 0);
        assert_eq!(ship.load(Good::Food, 10), 10);
        assert_eq!(ship.cargo_total, 10);
        assert_eq!(ship.cargo_amount(Good::Food), 10);

        assert_eq!(ship.unload(Good::Food, 5), 5);
        assert_eq!(ship.cargo_total, 5);

        // Load up to capacity
        assert_eq!(
            ship.load(Good::Cloth, ship.cargo_capacity()),
            ship.cargo_capacity() - 5
        );
    }

    #[test]
    fn trade_route_execution() {
        let mut wh_a = make_warehouse(0, 0, 10, 10);
        let mut wh_b = make_warehouse(1, 0, 20, 20);

        // Stock warehouse A with spices
        wh_a.deposit(Good::Spices, 20);
        // Stock warehouse B with food
        wh_b.deposit(Good::Food, 15);

        let mut route = TradeRoute::new(0, 0);
        route.add_stop(RouteStop {
            island_id: 0,
            warehouse_x: 10,
            warehouse_y: 10,
            load_goods: vec![(Good::Spices, 10)],
            unload_goods: vec![Good::Food],
        });
        route.add_stop(RouteStop {
            island_id: 1,
            warehouse_x: 20,
            warehouse_y: 20,
            load_goods: vec![(Good::Food, 10)],
            unload_goods: vec![Good::Spices],
        });
        route.activate();

        let mut ship = TradeShip::new(0, 0, 10, 10);
        let mut warehouses = vec![wh_a, wh_b];

        // Tick until ship arrives and trades at first stop
        let mut total_gold = 0i32;
        for _ in 0..100 {
            total_gold += tick_trade_ship(&mut ship, &route, &mut warehouses, None);
        }

        // Ship should have executed at least one complete trade cycle
        assert!(
            ship.profit != 0 || ship.cargo_total > 0 || total_gold != 0,
            "Ship should have traded something"
        );
    }

    #[test]
    fn trade_uses_per_good_prices() {
        // Round-trip 10 Wood through a route and verify the gold delta
        // matches `price_of(Wood).sell - price_of(Wood).buy` per unit, not
        // the old flat 8-vs-6 numbers.
        use crate::prices::price_of;
        let mut wh_a = make_warehouse(0, 0, 10, 10);
        wh_a.set_capacity(Good::Wood, 100);
        wh_a.deposit(Good::Wood, 50);

        let mut wh_b = make_warehouse(1, 0, 50, 50);
        wh_b.set_capacity(Good::Wood, 100);

        let mut warehouses = vec![wh_a, wh_b];
        let mut route = TradeRoute::new(0, 0);
        route.add_stop(RouteStop {
            island_id: 0,
            warehouse_x: 10,
            warehouse_y: 10,
            load_goods: vec![(Good::Wood, 10)],
            unload_goods: vec![],
        });
        route.add_stop(RouteStop {
            island_id: 1,
            warehouse_x: 50,
            warehouse_y: 50,
            load_goods: vec![],
            unload_goods: vec![Good::Wood],
        });
        route.activate();

        let mut ship = TradeShip::new(0, 0, 10, 10);
        // Run enough ticks for ship to reach stop 0, load, then stop 1, unload.
        let mut total_gold = 0i32;
        for _ in 0..200 {
            total_gold += tick_trade_ship(&mut ship, &route, &mut warehouses, None);
            if ship.profit != 0 && warehouses[1].stock(Good::Wood) > 0 && ship.cargo_total == 0 {
                break;
            }
        }
        let p = price_of(Good::Wood);
        // Bought 10 (-10*buy), sold 10 (+10*sell).
        let expected = 10 * (p.sell - p.buy);
        assert_eq!(
            total_gold, expected,
            "want {expected} gold, got {total_gold}"
        );
    }

    #[test]
    fn ocean_mapped_ship_stalls_when_stop_has_no_navigable_approach() {
        let szs = anno_formats::szs::SzsFile {
            chunks: Vec::new(),
            islands: vec![anno_formats::szs::Island {
                number: 0,
                width: 100,
                height: 100,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: Vec::new(),
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
        };
        let ocean = crate::ocean_map::OceanMap::from_scenario(&szs);

        let mut route = TradeRoute::new(0, 0);
        route.add_stop(RouteStop {
            island_id: 0,
            warehouse_x: 50,
            warehouse_y: 50,
            load_goods: vec![],
            unload_goods: vec![],
        });
        route.add_stop(RouteStop {
            island_id: 0,
            warehouse_x: 105,
            warehouse_y: 50,
            load_goods: vec![],
            unload_goods: vec![],
        });
        route.activate();

        let mut ship = TradeShip::new(0, 0, 105, 50);
        let mut warehouses = vec![make_warehouse(0, 0, 50, 50)];

        tick_trade_ship(&mut ship, &route, &mut warehouses, Some(&ocean));
        assert_eq!(ship.state, ShipState::Sailing);
        assert_eq!((ship.world_x, ship.world_y), (105, 50));

        for _ in 0..5 {
            tick_trade_ship(&mut ship, &route, &mut warehouses, Some(&ocean));
        }
        assert_eq!((ship.world_x, ship.world_y), (105, 50));
        assert!(ship.path.is_empty());
    }

    #[test]
    fn source_ocean_trade_path_uses_source_direction_blockers() {
        let scenario = anno_formats::szs::SzsFile {
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
        let definitions = [anno_formats::cod::BuildingDef {
            source_id: 0x4e20,
            kind: "BODEN".to_string(),
            ..Default::default()
        }];
        let ocean = crate::ocean_map::OceanMap::from_source_scenario(&scenario, &definitions);
        let mut route = TradeRoute::new(0, 0);
        route.add_stop(RouteStop {
            island_id: 0,
            warehouse_x: 2,
            warehouse_y: 1,
            load_goods: vec![],
            unload_goods: vec![],
        });
        route.add_stop(RouteStop {
            island_id: 0,
            warehouse_x: 0,
            warehouse_y: 1,
            load_goods: vec![],
            unload_goods: vec![],
        });
        route.activate();
        let mut ship = TradeShip::new(0, 0, 0, 1);

        assert_eq!(
            compute_path_to_stop(&mut ship, &route, Some(&ocean)),
            RoutePathStatus::PathReady
        );
        assert_eq!(ship.path.last(), Some(&(2, 1)));
        assert!(!ship.path.contains(&(1, 1)));
    }

    #[test]
    fn trade_route_needs_two_stops() {
        let mut route = TradeRoute::new(0, 0);
        route.add_stop(RouteStop {
            island_id: 0,
            warehouse_x: 0,
            warehouse_y: 0,
            load_goods: vec![],
            unload_goods: vec![],
        });
        route.activate();
        assert!(!route.active, "Route with 1 stop should not activate");

        route.add_stop(RouteStop {
            island_id: 1,
            warehouse_x: 10,
            warehouse_y: 10,
            load_goods: vec![],
            unload_goods: vec![],
        });
        route.activate();
        assert!(route.active, "Route with 2 stops should activate");
    }

    #[test]
    fn free_trader_finds_surplus() {
        let mut wh_a = make_warehouse(0, 0, 10, 10);
        let wh_b = make_warehouse(1, 0, 20, 20);

        wh_a.deposit(Good::Spices, 25); // Surplus
                                        // wh_b has no spices — needs some

        let warehouses = vec![wh_a, wh_b];
        let trade = free_trader_find_trade(&warehouses, 0, DEFAULT_SMALL_TRADER_CARGO_CAPACITY);

        assert!(trade.is_some(), "Should find a trade opportunity");
        let (from, to, good, _amount) = trade.unwrap();
        assert_eq!(from, 0);
        assert_eq!(to, 1);
        assert_eq!(good, Good::Spices);
    }
}
