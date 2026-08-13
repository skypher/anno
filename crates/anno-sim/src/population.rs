//! Population demand model.
//!
//! Connects population tiers to warehouse goods supply.
//! Each tier has specific good requirements; satisfaction depends on
//! how well these demands are met from warehouse inventory.
//!
//! Anno 1602 population tiers and their demands:
//!   Pioneer:     Food
//!   Settler:     Food, Cloth
//!   Citizen:     Food, Cloth, Alcohol, TobaccoProducts
//!   Merchant:    Food, Cloth, Alcohol, TobaccoProducts, Spices
//!   Aristocrat:  Food, Cloth, Alcohol, TobaccoProducts, Spices,
//!                Cocoa, Jewelry, Clothing

use crate::player::{Player, NUM_DEMAND_CATEGORIES};
use crate::types::{Good, PopTier, NUM_POP_TIERS};
use crate::warehouse::Warehouse;

/// Goods demanded by each population tier.
/// Each tier demands all goods of its level plus all lower-tier goods.
pub const TIER_DEMANDS: &[&[Good]] = &[
    // Pioneer
    &[Good::Food],
    // Settler
    &[Good::Food, Good::Cloth],
    // Citizen
    &[
        Good::Food,
        Good::Cloth,
        Good::Alcohol,
        Good::TobaccoProducts,
    ],
    // Merchant
    &[
        Good::Food,
        Good::Cloth,
        Good::Alcohol,
        Good::TobaccoProducts,
        Good::Spices,
    ],
    // Aristocrat — full eight-good roster (manual sec. 7.3
    // "Aristokraten").  The DEMAND_GOODS table already reserves
    // slots 6/7 for Jewelry and Clothing; this is the only tier
    // that consumes them.
    &[
        Good::Food,
        Good::Cloth,
        Good::Alcohol,
        Good::TobaccoProducts,
        Good::Spices,
        Good::Cocoa,
        Good::Jewelry,
        Good::Clothing,
    ],
];

/// Per-capita consumption rate per economy tick (per 100 population).
/// Higher tiers consume more per capita because they demand more
/// distinct goods.
///
/// REPORT-GAP (consumption mechanism is config-driven and only
/// partially reproduced here). The original does NOT use a single
/// per-tier consumption scalar. Instead (`FUN_0047f8a0`,
/// `1602_exe.c:91379-91434`):
///   * Per tier, per good, a smoothed *demand accumulator* at struct
///     `0x150` (food) / `0x15c+` (goods 1..7) is grown by
///     `weight * population_measure` each tick, then decayed by
///     `×15/16` (`:91430`).
///   * The per-good `weight` values come from the `BGRUPPE_WARE`
///     blocks in haeuser.cod (recovered — the `Ware: NAME, f` floats),
///     scaled at load (`:66651`, `/600`); the food rate is
///     `DAT_0049af2c = ftol(Nahrung)/600` (`:66678`).
///   * Goods are then pulled from the warehouse as WHOLE units via
///     `FUN_0047a9b0`, sized by `((demand-supply) >> 8) + 1` and
///     capped by stock (`:91330-91347`).
///
/// The recovered `BGRUPPE_WARE` demand weights (per tier, per good)
/// are, from haeuser.cod:
///   Settler   : STOFFE 0.6, ALKOHOL 0.5
///   Citizen   : STOFFE 0.7, ALKOHOL 0.6, TABAKWAREN 0.5, GEWUERZE 0.5
///   Merchant  : STOFFE 0.8, ALKOHOL 0.7, KAKAO 0.7, TABAKWAREN 0.6, GEWUERZE 0.6
///   Aristocrat: ALKOHOL 0.8, KAKAO 0.6, TABAKWAREN 0.6, GEWUERZE 0.6, KLEIDUNG 0.5, SCHMUCK 0.2
/// (Pioneer consumes food only, via the `Nahrung` rate.)
///
/// Faithfully reproducing the accumulator EMA + whole-unit pull
/// dynamics is a larger, higher-risk change that would ripple through
/// warehouse withdrawal and every economy/population test, so it is
/// intentionally left as a gap. The scalar below is the prior
/// empirical approximation (community population-per-industry table,
/// scaled to per-100-pop-per-economy-tick), retained pending a full
/// port of the accumulator model.
const CONSUMPTION_PER_100: [u16; NUM_POP_TIERS] = [
    2, // Pioneer
    4, // Settler
    5, // Citizen
    7, // Merchant
    7, // Aristocrat
];

/// Map demand slot indices to goods.
pub const DEMAND_GOODS: [Good; NUM_DEMAND_CATEGORIES] = [
    Good::Food,
    Good::Cloth,
    Good::Alcohol,
    Good::TobaccoProducts,
    Good::Spices,
    Good::Cocoa,
    Good::Jewelry,
    Good::Clothing,
];

/// Update population demands and satisfaction for a player based on warehouse supply.
///
/// This is called each economy tick (10 000 ms) to:
/// 1. Calculate total demand from population
/// 2. Consume goods from warehouses
/// 3. Update demand/supply ratios
/// 4. Adjust satisfaction based on fulfillment
pub fn update_population_demands(player: &mut Player, warehouses: &mut [Warehouse], player_id: u8) {
    // Reset demands
    for slot in &mut player.demands {
        slot.demand = 0;
        slot.supply = 0;
    }

    // Calculate demand from each population tier
    for tier in 0..NUM_POP_TIERS {
        let pop = player.population[tier];
        if pop == 0 {
            continue;
        }

        let demands = if tier < TIER_DEMANDS.len() {
            TIER_DEMANDS[tier]
        } else {
            continue;
        };

        let consumption = (pop as u32 * CONSUMPTION_PER_100[tier] as u32) / 100;
        let consumption = consumption.max(1); // At least 1 unit if any population

        for &good in demands {
            if let Some(slot_idx) = demand_slot_for_good(good) {
                player.demands[slot_idx].demand += consumption;
            }
        }
    }

    // Try to supply demands from warehouses
    let player_warehouses: Vec<usize> = warehouses
        .iter()
        .enumerate()
        .filter(|(_, w)| w.active && w.owner == player_id)
        .map(|(i, _)| i)
        .collect();

    for slot_idx in 0..NUM_DEMAND_CATEGORIES {
        let demand = player.demands[slot_idx].demand;
        if demand == 0 {
            continue;
        }

        let good = DEMAND_GOODS[slot_idx];
        let mut remaining_demand = demand;
        let mut total_supplied = 0u32;

        // Withdraw from each warehouse proportionally
        for &wh_idx in &player_warehouses {
            if remaining_demand == 0 {
                break;
            }

            let available = warehouses[wh_idx].stock(good) as u32;
            let take = remaining_demand.min(available);
            if take > 0 {
                warehouses[wh_idx].withdraw(good, take as u16);
                total_supplied += take;
                remaining_demand -= take;
            }
        }

        player.demands[slot_idx].supply = total_supplied;
    }

    // Update per-tier satisfaction based on how well their specific demands are met
    for tier in 0..NUM_POP_TIERS {
        if player.population[tier] == 0 {
            player.satisfaction[tier] = 128; // Full satisfaction when no one to complain
            continue;
        }

        let demands = if tier < TIER_DEMANDS.len() {
            TIER_DEMANDS[tier]
        } else {
            continue;
        };

        // Average fulfillment across this tier's demanded goods
        let mut total_fulfillment = 0u32;
        let mut num_goods = 0u32;

        for &good in demands {
            if let Some(slot_idx) = demand_slot_for_good(good) {
                let slot = &player.demands[slot_idx];
                if slot.demand > 0 {
                    let fulfillment =
                        ((slot.supply as u64 * 128) / slot.demand as u64).min(128) as u32;
                    total_fulfillment += fulfillment;
                    num_goods += 1;
                }
            }
        }

        if num_goods > 0 {
            // Satisfaction is recomputed FRESH each economy tick from
            // this tick's goods fulfillment — it is NOT blended with
            // the previous value and NOT decayed. RE: `1602_exe.c:91396`
            // stores the freshly computed satisfaction at struct
            // `0x248+tier`; the `×15/16` decay nearby (`:91430`) applies
            // to the demand/supply accumulators, not to satisfaction.
            // Each per-good fulfillment is already clamped to 0x80, so
            // the average stays within 0..=128.
            player.satisfaction[tier] = (total_fulfillment / num_goods) as u8;
        }
    }
}

/// Map a Good to its demand slot index.
fn demand_slot_for_good(good: Good) -> Option<usize> {
    DEMAND_GOODS.iter().position(|&g| g == good)
}

/// Net per-tier population growth per economy tick, expressed in
/// thousandths of current population, before promotion/demotion.
///
/// Tuning:
///   sat = 128 (full)   → +5.0% per tick
///   sat = 96  (high)   → +2.5% per tick
///   sat = 64  (par)    → 0
///   sat = 32  (low)    → -2.5% per tick
///   sat = 0   (none)   → -5.0% per tick
/// Each step bounds the absolute change at most 50 people per tier.
fn growth_delta(pop: u32, sat: u8) -> i32 {
    if pop == 0 {
        return 0;
    }
    let s = sat as i32;
    // (s - 64) / 16 percent  →  per-mille = (s - 64) * 1000 / 1600
    let permille = (s - 64) * 50 / 64;
    let delta = (pop as i64 * permille as i64 / 1000) as i32;
    delta.clamp(-50, 50)
}

/// Apply per-tier population growth/decay and tier promotion.
/// Runs every economy tick after `update_population_demands`.
///
///   - Each tier's population grows or shrinks based on `growth_delta`.
///   - When a tier is fully satisfied (sat ≥ 96) AND the next tier exists,
///     2% of that tier promotes to the next (Pioneer → Settler → … → Aristocrat).
///   - When a tier is starving (sat < 32) and pop > 0, 1% emigrates entirely.
///   - Total population is capped at `housing_cap` (0 means uncapped). When
///     the cap is exceeded, every tier is scaled proportionally so the
///     surplus simply doesn't materialise this tick.
pub fn update_population_growth(player: &mut Player, housing_cap: u32) {
    let mut new_pop = player.population;

    // 1. Growth / decay
    for tier in 0..NUM_POP_TIERS {
        let pop = new_pop[tier];
        let delta = growth_delta(pop, player.satisfaction[tier]);
        new_pop[tier] = (pop as i64 + delta as i64).max(0) as u32;
    }

    // 2. Promotion (lower → higher).
    //
    // Anno 1602 manual section 6.7.1 ("The relationships between
    // population, the level of civilization, and demand"): citizens
    // upgrade when their existing demands are fully met AND the
    // additional goods that the next tier requires are also being
    // supplied. We honour that: promote only when (a) current tier
    // satisfaction is full, and (b) every good the next tier
    // *additionally* demands has > 0 supply this tick.
    for tier in 0..NUM_POP_TIERS - 1 {
        if player.satisfaction[tier] < 96 || new_pop[tier] == 0 {
            continue;
        }
        let cur_demands: &[Good] = TIER_DEMANDS[tier];
        let next_demands: &[Good] = TIER_DEMANDS[tier + 1];
        let extra_goods: Vec<Good> = next_demands
            .iter()
            .copied()
            .filter(|g| !cur_demands.contains(g))
            .collect();
        let next_tier_supplied = extra_goods.iter().all(|g| {
            // The good must be in the demand-slot grid AND have a
            // non-zero most-recent fulfilment sample. If we don't
            // track this good as a demand category at all, we can't
            // gate on it — fall through to the satisfaction rule.
            match DEMAND_GOODS.iter().position(|d| d == g) {
                Some(idx) => player.demands[idx].fulfillment_history[0] > 0,
                None => true,
            }
        });
        if !next_tier_supplied {
            continue;
        }
        let promoted = (new_pop[tier] / 50).max(1); // 2% (min 1)
        let promoted = promoted.min(new_pop[tier]);
        new_pop[tier] -= promoted;
        new_pop[tier + 1] = new_pop[tier + 1].saturating_add(promoted);
    }

    // 3. Emigration on severe shortage.
    for tier in 0..NUM_POP_TIERS {
        if player.satisfaction[tier] < 32 && new_pop[tier] > 0 {
            let leaving = (new_pop[tier] / 100).max(1);
            new_pop[tier] = new_pop[tier].saturating_sub(leaving);
        }
    }

    // 4. Housing cap: scale every tier down proportionally if we'd exceed
    //    the player's total housing capacity. 0 means uncapped.
    let total: u32 = new_pop.iter().sum();
    if housing_cap > 0 && total > housing_cap {
        for tier in 0..NUM_POP_TIERS {
            new_pop[tier] = ((new_pop[tier] as u64 * housing_cap as u64) / total as u64) as u32;
        }
    }

    player.population = new_pop;
    player.total_population = new_pop.iter().sum();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn growth_delta_par_satisfaction_is_zero() {
        assert_eq!(growth_delta(1000, 64), 0);
    }

    #[test]
    fn growth_delta_full_satisfaction_grows() {
        let d = growth_delta(1000, 128);
        assert!(d > 0);
        assert!(d <= 50, "growth capped at 50, got {d}");
    }

    #[test]
    fn growth_delta_starving_shrinks() {
        let d = growth_delta(1000, 0);
        assert!(d < 0);
        assert!(d >= -50);
    }

    #[test]
    fn fully_satisfied_population_grows_and_promotes() {
        let mut p = Player::new_human(0);
        p.population[0] = 1000;
        p.satisfaction[0] = 128;
        let before_pioneer = p.population[0];
        update_population_growth(&mut p, 0);
        // Pioneers' next-tier extra demand is Cloth. Without Cloth
        // supply the gate stops promotion entirely.
        assert_eq!(
            p.population[1], 0,
            "promotion should be blocked without next-tier supply"
        );
        let total_after = p.population.iter().sum::<u32>();
        assert!(total_after >= before_pioneer);

        // Now wire Cloth supply and re-run: promotion fires.
        let cloth_idx = DEMAND_GOODS.iter().position(|g| *g == Good::Cloth).unwrap();
        p.demands[cloth_idx].fulfillment_history[0] = 128;
        update_population_growth(&mut p, 0);
        assert!(p.population[1] > 0, "supplied Cloth → settlers exist");
    }

    #[test]
    fn starving_tier_emigrates() {
        let mut p = Player::new_human(0);
        p.population[2] = 200; // citizens
        p.satisfaction[2] = 0;
        update_population_growth(&mut p, 0);
        assert!(p.population[2] < 200, "starving citizens should leave");
    }

    #[test]
    fn housing_cap_clamps_total() {
        let mut p = Player::new_human(0);
        p.population = [200, 200, 200, 0, 0]; // total 600
        for s in &mut p.satisfaction {
            *s = 64;
        } // par; no growth/decay
        update_population_growth(&mut p, 300);
        let total: u32 = p.population.iter().sum();
        assert!(total <= 300, "got {total}");
        // Proportions roughly preserved (each tier scaled by 0.5).
        assert!(p.population[0] < 200);
        assert!(p.population[1] < 200);
        assert!(p.population[2] < 200);
    }

    #[test]
    fn empty_tier_stays_empty() {
        let mut p = Player::new_human(0);
        // sat is 128 by default but pop is 0
        update_population_growth(&mut p, 0);
        assert_eq!(p.population.iter().sum::<u32>(), 0);
    }
}

/// Get the population tier that demands a specific good.
pub fn tier_for_good(good: Good) -> Option<PopTier> {
    for (tier_idx, demands) in TIER_DEMANDS.iter().enumerate() {
        if demands.contains(&good) {
            return match tier_idx {
                0 => Some(PopTier::Pioneer),
                1 => Some(PopTier::Settler),
                2 => Some(PopTier::Citizen),
                3 => Some(PopTier::Merchant),
                4 => Some(PopTier::Aristocrat),
                _ => None,
            };
        }
    }
    None
}
