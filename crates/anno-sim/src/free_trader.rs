//! Free trader: an NPC trading ship that roams between player
//! warehouses, exchanging goods at standard prices. Replaces the
//! teleport-deposit free trader removed in the authenticity audit.
//!
//! RE references for the trader ship:
//! - `decompiled/1602_exe.c:83179` — `sprintf(..., s_Trader_d_0049ae30, 4)`,
//!   confirming the free trader occupies a fixed player slot and starts
//!   with `_DAT_005b8084 = 1000000` gold and ID `0xd`.
//! - `decompiled/1602_exe.c:11248` — `s_HANDLER_004982c8`, the figure-
//!   name string. Free-trader ship is object-type tag `0x35`
//!   ("HANDLER") in the object-reference system (`FUN_00443e60`
//!   case `0x35` at line 47208 returns `(undefined4 *)
//!   (&DAT_005e6bcc)[island * 0x2c0 + tile]` — i.e. the trader ship
//!   is a building-tile-resident object).
//! - `decompiled/1602_exe.c:50190 FUN_004487e0` — the per-ship
//!   per-tick efficiency / cargo-load update for trader ships
//!   (stride `0x86` = 134-byte ship records in `&DAT_004cf358`).
//! - `decompiled/1602_exe.c:50245 FUN_004488d0` — TARGET selector:
//!   iterates `&DAT_005b7680` (player slots, stride `0xa0`) testing
//!   `state == 0` or `state == 0xc`, calls `FUN_00475c60` to score
//!   the slot, picks the lowest-scoring suitable slot via a list of
//!   tiles `aiStack_258[150]`, returns one chosen by `rand() % count`.
//! - `decompiled/1602_exe.c:57709-57714` — periodic re-target gate:
//!   `if (*param_1 == '\x03') { uVar18 = rand(); if (((uVar18 & 3)
//!   == 0) && (iVar21 = FUN_004488d0(puVar2), ...))` — when the
//!   ship is in state 3 ("seeking next target"), it has a **1-in-4
//!   chance per ship tick** to invoke the target selector. New
//!   sail-target assigned via `FUN_00445350(&local_3ec, 0x37,
//!   target_x, target_y)`.
//! - `figuren.cod` `Nummer: HANDEL1` — the ship sprite/animation set
//!   the free trader reuses in render.
//!
//! The exact dock-duration, stock-list, and visit-count constants
//! sit elsewhere (the trader's at-Kontor dialog handler isn't yet
//! located) and remain SPECULATIVE; values below are conservative
//! defaults pending that RE.

use crate::trade::compass_heading;
use crate::types::Good;
use crate::warehouse::Warehouse;

/// Diplomacy / player slot reserved for the free-trader faction.
/// SPECULATIVE — `1602_exe.c:83179` shows player slot 4 is the trader,
/// but our slot layout uses 0-3 = humans/AI, 6 = pirate, leaving 5
/// for the trader by convention rather than direct mapping.
pub const FREE_TRADER_SLOT: u8 = 5;

// Visit count and dock duration. The Anno 1602 manual (section 8.1
// "Free traders") describes traders as roaming around buying surplus
// and selling needed goods at every player-set buy/sell slider.
// Traders don't have a fixed "visits before leaving" — in the
// original they keep circulating between active Kontors as long as
// there are at least two warehouses on the island chain (manual
// section 11.4.3). To match that behaviour we treat
// `VISITS_BEFORE_LEAVING` as a soft per-spawn cap so the same ship
// doesn't loop forever (the binary recreates trader ships
// dynamically — see `1602_exe.c:50289` returning `&DAT_004cf358 +
// iVar3 * 0x218` after `FUN_004488d0` picks a new target).
//
// `DOCK_TICKS` and `TRADE_BATCH` control how long a trader stays at
// each Kontor and how much it exchanges per dock-tick. The original
// uses player-set slider amounts (manual 8.1: "Use one slider to
// set the selling price and the other to set the maximum amount you
// want to sell."), which we simplify into a fixed batch since our
// warehouse model doesn't yet track per-good buy/sell sliders.
//
// Tick rate is 10 Hz / 600 per minute (decoded from `:98053`); 5
// dock-ticks = 0.5 s in our model.
const VISITS_BEFORE_LEAVING: u8 = 3;
const DOCK_TICKS: u8 = 5;
const TRADE_BATCH: u16 = 5;

/// Initial inventory of a freshly-spawned free trader.
///
/// Source: original Anno 1602 manual, section 8.1 "Free traders":
/// > "Tools and raw materials, such as iron ore, may be scarce at the
/// > beginning, and the traders carry a supply of such items."
///
/// And confirmed by Tim Howgego's gameplay notes
/// (timhowgego.wordpress.com/anno_1602/gameplay/trade_diplomacy/):
/// > "Tools and ore (after the first player started mining it) are
/// > always for sale. All other goods will be sold only if someone
/// > sells them to the Free Traders."
///
/// So the trader's default stock is `Tools` and `Ore` — every other
/// good is added dynamically when a player sells some to the trader,
/// which we don't yet model (would require a persistent trader-stock
/// table tracked across visits). Per-good inventory amounts aren't
/// quoted in the manual.
pub fn default_stock() -> Vec<(Good, u16)> {
    vec![
        (Good::Tools, 30),
        (Good::Ore, 40),
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
