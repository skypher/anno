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

/// Maximum cargo capacity per ship.
pub const SHIP_CARGO_CAPACITY: u16 = 50;

/// Gold earned per unit of goods sold.
const SELL_PRICE_PER_UNIT: i32 = 8;
/// Gold spent per unit of goods bought.
const BUY_PRICE_PER_UNIT: i32 = 6;

/// A single stop in a trade route.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct TradeShip {
    pub owner: u8,
    pub route_id: u16,
    /// Current position (world coordinates).
    pub world_x: i32,
    pub world_y: i32,
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
    /// Gold earned from trading.
    pub profit: i32,
    pub active: bool,
    /// Pre-computed ocean path (world coordinates). Empty = direct movement fallback.
    pub path: Vec<(i32, i32)>,
    /// Current index into path.
    pub path_idx: usize,
}

/// Ship operating states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        Self {
            owner,
            route_id,
            world_x,
            world_y,
            speed: 2,
            current_stop: 0,
            state: ShipState::Idle,
            cargo: Vec::new(),
            cargo_total: 0,
            profit: 0,
            active: true,
            path: Vec::new(),
            path_idx: 0,
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
        let space = SHIP_CARGO_CAPACITY.saturating_sub(self.cargo_total);
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
            compute_path_to_stop(ship, route, ocean_map);
            ship.state = ShipState::Sailing;
        }
        ShipState::Sailing => {
            if !ship.path.is_empty() && ship.path_idx < ship.path.len() {
                // Follow pre-computed ocean path
                let steps = ship.speed as usize;
                for _ in 0..steps {
                    if ship.path_idx >= ship.path.len() {
                        break;
                    }
                    let (nx, ny) = ship.path[ship.path_idx];
                    ship.world_x = nx;
                    ship.world_y = ny;
                    ship.path_idx += 1;
                }

                // Check if we reached end of path (near destination)
                if ship.path_idx >= ship.path.len() {
                    ship.path.clear();
                    ship.path_idx = 0;
                    ship.state = ShipState::Trading;
                }
            } else {
                // Fallback: direct movement (no path computed or path exhausted)
                let stop = &route.stops[ship.current_stop];
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
                }
            }
        }
        ShipState::Trading => {
            let stop = &route.stops[ship.current_stop];

            // Find the warehouse at this stop
            if let Some(wh) = warehouses.iter_mut().find(|w| {
                w.island_id == stop.island_id
                    && w.owner == ship.owner
                    && w.active
            }) {
                // Unload goods (sell to warehouse)
                for &good in &stop.unload_goods {
                    let amount = ship.cargo_amount(good);
                    if amount > 0 {
                        let deposited = wh.deposit(good, amount);
                        ship.unload(good, deposited);
                        gold_delta += deposited as i32 * SELL_PRICE_PER_UNIT;
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
                        gold_delta -= loaded as i32 * BUY_PRICE_PER_UNIT;
                    }
                }
            }

            ship.compact_cargo();

            // Advance to next stop and compute ocean path
            ship.current_stop = (ship.current_stop + 1) % route.stops.len();
            compute_path_to_stop(ship, route, ocean_map);
            ship.state = ShipState::Sailing;
        }
        ShipState::Waiting => {
            // Re-check if we can trade
            ship.state = ShipState::Trading;
        }
    }

    ship.profit += gold_delta;
    gold_delta
}

/// Compute an ocean A* path from ship's current position to the next route stop.
fn compute_path_to_stop(
    ship: &mut TradeShip,
    route: &TradeRoute,
    ocean_map: Option<&crate::ocean_map::OceanMap>,
) {
    ship.path.clear();
    ship.path_idx = 0;

    let ocean = match ocean_map {
        Some(m) => m,
        None => return, // No ocean map — use direct movement fallback
    };

    if ship.current_stop >= route.stops.len() {
        return;
    }
    let stop = &route.stops[ship.current_stop];
    let target_x = stop.warehouse_x as i32;
    let target_y = stop.warehouse_y as i32;

    // Find nearest navigable tiles to start and end
    let start = match ocean.nearest_navigable(ship.world_x, ship.world_y) {
        Some(p) => p,
        None => return,
    };
    let goal = match ocean.nearest_navigable(target_x, target_y) {
        Some(p) => p,
        None => return,
    };

    if let Some(path) = crate::ocean_map::find_ocean_path(ocean, start, goal) {
        ship.path = path;
        ship.path_idx = 0;
    }
    // If pathfinding fails, path stays empty and ship uses direct movement
}

/// Free trader AI: finds profitable trades between warehouses.
/// Returns a trade action if one is found.
pub fn free_trader_find_trade(
    warehouses: &[Warehouse],
    ship_owner: u8,
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
                    let transfer = surplus.min(SHIP_CARGO_CAPACITY);
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
    fn ship_cargo_load_unload() {
        let mut ship = TradeShip::new(0, 0, 0, 0);
        assert_eq!(ship.load(Good::Food, 10), 10);
        assert_eq!(ship.cargo_total, 10);
        assert_eq!(ship.cargo_amount(Good::Food), 10);

        assert_eq!(ship.unload(Good::Food, 5), 5);
        assert_eq!(ship.cargo_total, 5);

        // Load up to capacity
        assert_eq!(ship.load(Good::Cloth, SHIP_CARGO_CAPACITY), SHIP_CARGO_CAPACITY - 5);
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
        assert!(ship.profit != 0 || ship.cargo_total > 0 || total_gold != 0,
            "Ship should have traded something");
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
        let trade = free_trader_find_trade(&warehouses, 0);

        assert!(trade.is_some(), "Should find a trade opportunity");
        let (from, to, good, _amount) = trade.unwrap();
        assert_eq!(from, 0);
        assert_eq!(to, 1);
        assert_eq!(good, Good::Spices);
    }
}
