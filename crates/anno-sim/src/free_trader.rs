//! Free trader: an NPC trading ship that roams between player
//! warehouses, exchanging goods at standard prices. Replaces the
//! teleport-deposit free trader removed in the authenticity audit.
//!
//! RE references:
//! - `decompiled/1602_exe.c:83179` — `sprintf(..., s_Trader_d_0049ae30, 4)`,
//!   confirming the free trader occupies a fixed player slot and starts
//!   with `_DAT_005b8084 = 1000000` gold.
//! - `figuren.cod` `Nummer: HANDEL1` — the ship sprite/animation set
//!   used for trade ships, which the free trader reuses in render.
//! - General gameplay observation: the original ship sails between
//!   player Kontors offering high-end wares at standard prices.
//!
//! The detailed numeric constants below (`VISITS_BEFORE_LEAVING`,
//! `DOCK_TICKS`, `TRADE_BATCH`, default stock list) are SPECULATIVE
//! placeholders — the corresponding RE sites have not yet been
//! identified. Conservative defaults; replace once the dispatcher
//! function is located in the binary.

use crate::trade::compass_heading;
use crate::types::Good;
use crate::warehouse::Warehouse;

/// Diplomacy / player slot reserved for the free-trader faction.
/// SPECULATIVE — `1602_exe.c:83179` shows player slot 4 is the trader,
/// but our slot layout uses 0-3 = humans/AI, 6 = pirate, leaving 5
/// for the trader by convention rather than direct mapping.
pub const FREE_TRADER_SLOT: u8 = 5;

// SPECULATIVE — no RE reference yet for these cadences. The original
// trader's visit count and dock duration will be located in the
// dispatcher function (TBD).
const VISITS_BEFORE_LEAVING: u8 = 3;
const DOCK_TICKS: u8 = 5;
const TRADE_BATCH: u16 = 5;

/// Initial inventory of a freshly-spawned free trader.
/// SPECULATIVE — the original game's per-trader stock list and amounts
/// are in a data table not yet located. Goods chosen for plausibility
/// (high-tier wares the player typically wants to import).
pub fn default_stock() -> Vec<(Good, u16)> {
    vec![
        (Good::Tools, 30),
        (Good::Cloth, 30),
        (Good::Wood, 40),
        (Good::Bricks, 30),
        (Good::Cannons, 8),
        (Good::Spices, 20),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FreeTraderState {
    #[default]
    Sailing,
    Docked,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct FreeTrader {
    pub world_x: i32,
    pub world_y: i32,
    pub heading: u8,
    pub speed: u16,
    pub target_x: i32,
    pub target_y: i32,
    /// Index into `Simulation::warehouses` of the current dock target,
    /// or `None` while sailing toward the world edge to despawn.
    pub target_warehouse: Option<usize>,
    pub state: FreeTraderState,
    pub dock_ticks_left: u8,
    pub visits_remaining: u8,
    pub stock: Vec<(Good, u16)>,
    /// Once true, the ship is heading off-edge and should be removed
    /// when it reaches its target.
    pub leaving: bool,
    pub active: bool,
    pub path: Vec<(i32, i32)>,
    pub path_idx: usize,
}

impl FreeTrader {
    pub fn spawn_at(x: i32, y: i32) -> Self {
        Self {
            world_x: x,
            world_y: y,
            heading: 0,
            speed: 2,
            target_x: x,
            target_y: y,
            target_warehouse: None,
            state: FreeTraderState::Sailing,
            dock_ticks_left: 0,
            visits_remaining: VISITS_BEFORE_LEAVING,
            stock: default_stock(),
            leaving: false,
            active: true,
            path: Vec::new(),
            path_idx: 0,
        }
    }

    pub fn stock_amount(&self, good: Good) -> u16 {
        self.stock.iter().filter(|(g, _)| *g == good).map(|(_, a)| *a).sum()
    }

    fn withdraw_stock(&mut self, good: Good, amount: u16) -> u16 {
        let mut remaining = amount;
        for (g, a) in self.stock.iter_mut() {
            if *g == good && remaining > 0 {
                let take = (*a).min(remaining);
                *a -= take;
                remaining -= take;
            }
        }
        self.stock.retain(|(_, a)| *a > 0);
        amount - remaining
    }

    fn deposit_stock(&mut self, good: Good, amount: u16) {
        if amount == 0 { return; }
        if let Some(slot) = self.stock.iter_mut().find(|(g, _)| *g == good) {
            slot.1 = slot.1.saturating_add(amount);
        } else {
            self.stock.push((good, amount));
        }
    }
}

/// Trade outcome at a single warehouse visit.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TradeResult {
    /// Net gold delta for the warehouse owner (negative = paid for goods,
    /// positive = received gold for surplus).
    pub gold_delta: i32,
}

/// Run a single docked trade interaction with the warehouse pointed at
/// by `trader.target_warehouse`. Sells what the warehouse can afford
/// and has room for; buys high-stock goods the trader still has space
/// for. Standard prices; gold settled with the warehouse owner.
pub fn dock_trade(
    trader: &mut FreeTrader,
    warehouses: &mut [Warehouse],
    player_gold: &mut i32,
) -> TradeResult {
    let mut result = TradeResult::default();
    let wh_idx = match trader.target_warehouse {
        Some(i) if i < warehouses.len() => i,
        _ => return result,
    };
    let wh = &mut warehouses[wh_idx];
    if !wh.active {
        return result;
    }

    // 1. Sell from trader stock → warehouse.
    let stock_snapshot: Vec<(Good, u16)> = trader.stock.clone();
    for (good, available) in stock_snapshot {
        if available == 0 { continue; }
        let want = TRADE_BATCH.min(available);
        let price = crate::prices::price_of(good).buy as i32;
        if *player_gold < price {
            continue;
        }
        let max_aff = (*player_gold / price.max(1)) as u16;
        let qty = want.min(max_aff);
        let deposited = wh.deposit(good, qty);
        if deposited > 0 {
            trader.withdraw_stock(good, deposited);
            *player_gold -= deposited as i32 * price;
            result.gold_delta -= deposited as i32 * price;
        }
    }

    // 2. Buy surplus from warehouse → trader. Pick goods the warehouse
    //    has more than 30 of so we don't drain its working stock.
    let surplus: Vec<(Good, u16)> = wh
        .all_stock()
        .into_iter()
        .filter(|(_, qty, _)| *qty > 30)
        .map(|(g, qty, _)| (g, qty.saturating_sub(30).min(TRADE_BATCH)))
        .collect();
    for (good, qty) in surplus {
        let price = crate::prices::price_of(good).sell as i32;
        let withdrawn = wh.withdraw(good, qty);
        if withdrawn > 0 {
            trader.deposit_stock(good, withdrawn);
            *player_gold += withdrawn as i32 * price;
            result.gold_delta += withdrawn as i32 * price;
        }
    }

    result
}

/// Step the free trader by one ship-tick: move toward target, dock on
/// arrival, run dock trades over multiple ticks, pick the next port
/// when done. Returns `true` if the trader has finished and should be
/// removed by the caller.
pub fn tick_one(
    trader: &mut FreeTrader,
    warehouses: &mut [Warehouse],
    players_gold: &mut [i32],
) -> bool {
    if !trader.active {
        return true;
    }

    match trader.state {
        FreeTraderState::Sailing => {
            // Direct movement; ocean-A* path overrides if present.
            let prev_x = trader.world_x;
            let prev_y = trader.world_y;
            if !trader.path.is_empty() && trader.path_idx < trader.path.len() {
                for _ in 0..trader.speed {
                    if trader.path_idx >= trader.path.len() { break; }
                    let (nx, ny) = trader.path[trader.path_idx];
                    trader.world_x = nx;
                    trader.world_y = ny;
                    trader.path_idx += 1;
                }
                if trader.path_idx >= trader.path.len() {
                    trader.path.clear();
                    trader.path_idx = 0;
                }
            } else {
                let dx = trader.target_x - trader.world_x;
                let dy = trader.target_y - trader.world_y;
                let steps = trader.speed as i32;
                if dx.abs() > dy.abs() {
                    trader.world_x += dx.signum() * steps.min(dx.abs());
                } else if dy != 0 {
                    trader.world_y += dy.signum() * steps.min(dy.abs());
                } else if dx != 0 {
                    trader.world_x += dx.signum() * steps.min(dx.abs());
                }
            }
            trader.heading = compass_heading(
                trader.world_x - prev_x,
                trader.world_y - prev_y,
                trader.heading,
            );

            let arrived = trader.world_x == trader.target_x
                && trader.world_y == trader.target_y;
            if arrived {
                if trader.leaving {
                    trader.active = false;
                    return true;
                }
                trader.state = FreeTraderState::Docked;
                trader.dock_ticks_left = DOCK_TICKS;
            }
        }
        FreeTraderState::Docked => {
            let wh_idx = trader.target_warehouse;
            if let Some(idx) = wh_idx {
                if idx < warehouses.len() {
                    let owner = warehouses[idx].owner as usize;
                    if owner < players_gold.len() {
                        let mut gold = players_gold[owner];
                        dock_trade(trader, warehouses, &mut gold);
                        players_gold[owner] = gold;
                    }
                }
            }
            if trader.dock_ticks_left > 0 {
                trader.dock_ticks_left -= 1;
            }
            if trader.dock_ticks_left == 0 {
                // Done at this port; caller chooses next destination.
                trader.target_warehouse = None;
                trader.state = FreeTraderState::Sailing;
                if trader.visits_remaining > 0 {
                    trader.visits_remaining -= 1;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_wh(island: u8, owner: u8, x: u16, y: u16) -> Warehouse {
        Warehouse::new(island, owner, x, y)
    }

    #[test]
    fn dock_trade_sells_to_warehouse() {
        let mut wh = mk_wh(0, 0, 5, 5);
        let mut whs = vec![wh.clone()];
        let mut trader = FreeTrader::spawn_at(5, 5);
        trader.target_warehouse = Some(0);
        let mut gold = 5000;
        let _ = dock_trade(&mut trader, &mut whs, &mut gold);
        assert!(whs[0].stock(Good::Tools) > 0);
        assert!(gold < 5000);
        let _ = wh; // suppress unused
    }

    #[test]
    fn dock_trade_buys_warehouse_surplus() {
        let mut whs = vec![mk_wh(0, 0, 5, 5)];
        whs[0].set_capacity(Good::Wool, 200);
        whs[0].deposit(Good::Wool, 200);
        let mut trader = FreeTrader::spawn_at(5, 5);
        trader.target_warehouse = Some(0);
        trader.stock.clear();
        let mut gold = 0;
        let _ = dock_trade(&mut trader, &mut whs, &mut gold);
        assert!(trader.stock_amount(Good::Wool) > 0);
        assert!(gold > 0);
    }

    #[test]
    fn dock_trade_skipped_when_player_broke() {
        let mut whs = vec![mk_wh(0, 0, 5, 5)];
        let mut trader = FreeTrader::spawn_at(5, 5);
        trader.target_warehouse = Some(0);
        let mut gold = 0;
        let _ = dock_trade(&mut trader, &mut whs, &mut gold);
        assert_eq!(whs[0].stock(Good::Tools), 0);
    }

    #[test]
    fn sailing_reaches_target_and_docks() {
        let mut whs = vec![mk_wh(0, 0, 10, 10)];
        let mut trader = FreeTrader::spawn_at(0, 10);
        trader.target_warehouse = Some(0);
        trader.target_x = 10;
        trader.target_y = 10;
        let mut gold = vec![5000i32];
        for _ in 0..30 {
            tick_one(&mut trader, &mut whs, &mut gold);
            if trader.state == FreeTraderState::Docked { break; }
        }
        assert_eq!(trader.state, FreeTraderState::Docked);
        assert_eq!(trader.world_x, 10);
        assert_eq!(trader.world_y, 10);
    }

    #[test]
    fn leaving_trader_despawns_on_arrival() {
        let mut whs = vec![mk_wh(0, 0, 10, 10)];
        let mut trader = FreeTrader::spawn_at(5, 5);
        trader.target_x = 5;
        trader.target_y = 5;
        trader.leaving = true;
        let mut gold = vec![0i32];
        let removed = tick_one(&mut trader, &mut whs, &mut gold);
        assert!(removed);
        assert!(!trader.active);
    }
}
