//! Rolling economy / population history for the human player.
//!
//! Sampled on each population tick (~10s game time). Bounded ring buffer
//! so memory stays flat regardless of run length.

use crate::player::Player;
use crate::types::Good;
use crate::warehouse::Warehouse;

const SAMPLES: usize = 120; // ~20 minutes at 1 sample / 10s
/// Number of `Good` enum slots; the discriminant fits in u8 so we index by it.
const N_GOODS: usize = 31;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EconomyHistory {
    /// Latest sample index modulo `SAMPLES`. `len` is total writes (capped).
    head: usize,
    len: usize,
    gold: Vec<i32>,
    population: Vec<u32>,
    /// Population-weighted average satisfaction, 0..=128.
    satisfaction: Vec<u8>,
    /// Income (gold/tick) and costs (gold/tick) snapshots.
    income: Vec<i32>,
    costs: Vec<i32>,
    /// Per-good warehouse stock totals across all of the player's
    /// active warehouses, indexed by `Good as u8` then by ring-buffer slot.
    /// Lazily allocated on first `record_stocks` call.
    #[serde(default)]
    stocks: Vec<Vec<u32>>,
}

impl EconomyHistory {
    pub fn new() -> Self {
        Self {
            head: 0,
            len: 0,
            gold: vec![0; SAMPLES],
            population: vec![0; SAMPLES],
            satisfaction: vec![0; SAMPLES],
            income: vec![0; SAMPLES],
            costs: vec![0; SAMPLES],
            stocks: vec![vec![0; SAMPLES]; N_GOODS],
        }
    }

    pub fn capacity() -> usize { SAMPLES }
    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Single-player snapshot — gold/population/satisfaction/income/costs.
    /// Use [`record_full`] when warehouses are available so per-good stocks
    /// land in the same time slot.
    pub fn record(&mut self, p: &Player) {
        self.record_full(p, &[], 0);
    }

    /// Record a full sample (player + warehouse stocks).
    pub fn record_full(&mut self, p: &Player, warehouses: &[Warehouse], owner: u8) {
        let total_pop: u32 = p.population.iter().sum();
        let weighted_sat: u32 = p
            .population
            .iter()
            .zip(p.satisfaction.iter())
            .map(|(&pop, &s)| pop * s as u32)
            .sum();
        let avg_sat = if total_pop > 0 {
            (weighted_sat / total_pop).min(128) as u8
        } else {
            0
        };
        let i = self.head;
        self.gold[i] = p.gold;
        self.population[i] = total_pop;
        self.satisfaction[i] = avg_sat;
        self.income[i] = p.calculate_income();
        self.costs[i] = p.calculate_costs();

        // Backfill stocks vec if a save predates the field.
        if self.stocks.len() < N_GOODS {
            self.stocks = vec![vec![0; SAMPLES]; N_GOODS];
        }
        for slot in self.stocks.iter_mut() {
            if slot.len() < SAMPLES { slot.resize(SAMPLES, 0); }
            slot[i] = 0;
        }
        for w in warehouses.iter().filter(|w| w.active && w.owner == owner) {
            for (g, qty, _cap) in w.all_stock() {
                let gi = g as u8 as usize;
                if gi < N_GOODS {
                    self.stocks[gi][i] = self.stocks[gi][i].saturating_add(qty as u32);
                }
            }
        }

        self.head = (self.head + 1) % SAMPLES;
        self.len = (self.len + 1).min(SAMPLES);
    }

    /// Return the samples in chronological order (oldest → newest).
    fn ordered<T: Copy>(&self, src: &[T]) -> Vec<T> {
        if self.len < SAMPLES {
            src[..self.len].to_vec()
        } else {
            // Ring buffer: head points just past newest; oldest is at head.
            let mut out = Vec::with_capacity(SAMPLES);
            out.extend_from_slice(&src[self.head..]);
            out.extend_from_slice(&src[..self.head]);
            out
        }
    }

    pub fn gold_series(&self) -> Vec<i32> { self.ordered(&self.gold) }
    pub fn population_series(&self) -> Vec<u32> { self.ordered(&self.population) }
    pub fn satisfaction_series(&self) -> Vec<u8> { self.ordered(&self.satisfaction) }
    pub fn income_series(&self) -> Vec<i32> { self.ordered(&self.income) }
    pub fn stock_series(&self, good: Good) -> Vec<u32> {
        let gi = good as u8 as usize;
        if gi >= self.stocks.len() { return Vec::new(); }
        self.ordered(&self.stocks[gi])
    }
    pub fn costs_series(&self) -> Vec<i32> { self.ordered(&self.costs) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(gold: i32, pop: u32, sat: u8) -> Player {
        let mut pl = Player::new_human(0);
        pl.gold = gold;
        pl.population[1] = pop;
        pl.satisfaction[1] = sat;
        pl
    }

    #[test]
    fn records_in_order_until_full() {
        let mut h = EconomyHistory::new();
        for g in 0..5 {
            h.record(&p(g, 10, 100));
        }
        let s = h.gold_series();
        assert_eq!(s, vec![0, 1, 2, 3, 4]);
        assert_eq!(h.len(), 5);
    }

    #[test]
    fn ring_buffer_drops_oldest() {
        let mut h = EconomyHistory::new();
        // Fill past capacity by 5 samples so the oldest 5 are dropped.
        for g in 0..(EconomyHistory::capacity() as i32 + 5) {
            h.record(&p(g, 1, 100));
        }
        let s = h.gold_series();
        assert_eq!(s.len(), EconomyHistory::capacity());
        assert_eq!(s.first().copied(), Some(5));
        assert_eq!(s.last().copied(), Some(EconomyHistory::capacity() as i32 + 4));
    }

    #[test]
    fn record_full_captures_warehouse_stocks() {
        let mut h = EconomyHistory::new();
        let pl = Player::new_human(0);
        let mut w = Warehouse::new(0, 0, 0, 0);
        w.set_capacity(Good::Wood, 100);
        w.deposit(Good::Wood, 50);
        w.deposit(Good::Iron, 12);
        h.record_full(&pl, &[w], 0);
        let wood = h.stock_series(Good::Wood);
        let iron = h.stock_series(Good::Iron);
        assert_eq!(wood, vec![50]);
        assert_eq!(iron, vec![12]);
        assert!(h.stock_series(Good::Sugar).iter().all(|&v| v == 0));
    }

    #[test]
    fn record_full_sums_across_warehouses() {
        let mut h = EconomyHistory::new();
        let pl = Player::new_human(0);
        let mut w1 = Warehouse::new(0, 0, 0, 0);
        let mut w2 = Warehouse::new(0, 0, 5, 5);
        w1.deposit(Good::Wood, 20);
        w2.deposit(Good::Wood, 25);
        h.record_full(&pl, &[w1, w2], 0);
        assert_eq!(h.stock_series(Good::Wood), vec![45]);
    }

    #[test]
    fn weighted_satisfaction() {
        let mut h = EconomyHistory::new();
        let mut pl = Player::new_human(0);
        pl.population = [100, 100, 0, 0, 0];
        pl.satisfaction = [80, 40, 0, 0, 0];
        h.record(&pl);
        // (100*80 + 100*40) / 200 = 60
        assert_eq!(h.satisfaction_series(), vec![60]);
    }
}
