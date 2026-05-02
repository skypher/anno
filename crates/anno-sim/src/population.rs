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
//!   Aristocrat:  Food, Cloth, Alcohol, TobaccoProducts, Spices, Cocoa

use crate::player::{DemandSlot, Player, NUM_DEMAND_CATEGORIES};
use crate::types::{Good, PopTier, NUM_POP_TIERS};
use crate::warehouse::Warehouse;

/// Goods demanded by each population tier.
/// Each tier demands all goods of its level plus all lower-tier goods.
const TIER_DEMANDS: &[&[Good]] = &[
    // Pioneer
    &[Good::Food],
    // Settler
    &[Good::Food, Good::Cloth],
    // Citizen
    &[Good::Food, Good::Cloth, Good::Alcohol, Good::TobaccoProducts],
    // Merchant
    &[
        Good::Food,
        Good::Cloth,
        Good::Alcohol,
        Good::TobaccoProducts,
        Good::Spices,
    ],
    // Aristocrat
    &[
        Good::Food,
        Good::Cloth,
        Good::Alcohol,
        Good::TobaccoProducts,
        Good::Spices,
        Good::Cocoa,
    ],
];

/// Per-capita consumption rate per economy tick (per 100 population).
/// Higher tiers consume more per capita.
const CONSUMPTION_PER_100: [u16; NUM_POP_TIERS] = [
    2, // Pioneer: 2 units per 100 pop per tick
    2, // Settler
    3, // Citizen
    3, // Merchant
    4, // Aristocrat
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
/// This is called each economy tick (9999ms) to:
/// 1. Calculate total demand from population
/// 2. Consume goods from warehouses
/// 3. Update demand/supply ratios
/// 4. Adjust satisfaction based on fulfillment
pub fn update_population_demands(
    player: &mut Player,
    warehouses: &mut [Warehouse],
    player_id: u8,
) {
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
            let avg = (total_fulfillment / num_goods) as u8;
            // Blend with current satisfaction (weighted average: 3/4 new + 1/4 old)
            let old = player.satisfaction[tier] as u32;
            let new = avg as u32;
            player.satisfaction[tier] = ((new * 3 + old) / 4) as u8;
        }
    }
}

/// Map a Good to its demand slot index.
fn demand_slot_for_good(good: Good) -> Option<usize> {
    DEMAND_GOODS.iter().position(|&g| g == good)
}

/// Return the goods the player is failing to supply at a "severe" level.
/// A severe shortage is `supply < demand / 2`. Used by the HUD to nudge
/// the player toward the missing production chain.
pub fn severe_shortages(player: &crate::player::Player) -> Vec<Good> {
    let mut out = Vec::new();
    for (slot_idx, slot) in player.demands.iter().enumerate() {
        if slot.demand > 0 && slot.supply * 2 < slot.demand {
            if slot_idx < DEMAND_GOODS.len() {
                out.push(DEMAND_GOODS[slot_idx]);
            }
        }
    }
    out
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
    if pop == 0 { return 0; }
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
pub fn update_population_growth(player: &mut Player) {
    let mut new_pop = player.population;

    // 1. Growth / decay
    for tier in 0..NUM_POP_TIERS {
        let pop = new_pop[tier];
        let delta = growth_delta(pop, player.satisfaction[tier]);
        new_pop[tier] = (pop as i64 + delta as i64).max(0) as u32;
    }

    // 2. Promotion (lower → higher)
    for tier in 0..NUM_POP_TIERS - 1 {
        if player.satisfaction[tier] >= 96 && new_pop[tier] > 0 {
            let promoted = (new_pop[tier] / 50).max(1); // 2% (min 1)
            let promoted = promoted.min(new_pop[tier]);
            new_pop[tier] -= promoted;
            new_pop[tier + 1] = new_pop[tier + 1].saturating_add(promoted);
        }
    }

    // 3. Emigration on severe shortage.
    for tier in 0..NUM_POP_TIERS {
        if player.satisfaction[tier] < 32 && new_pop[tier] > 0 {
            let leaving = (new_pop[tier] / 100).max(1);
            new_pop[tier] = new_pop[tier].saturating_sub(leaving);
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
        update_population_growth(&mut p);
        // Some pioneers promoted to settlers.
        assert!(p.population[1] > 0, "should have promoted some to settlers");
        // And the pioneer count moved (either net up from growth or down from promotion).
        let total_after = p.population.iter().sum::<u32>();
        // Total can't have lost people on positive sat; promotion is internal.
        assert!(total_after >= before_pioneer);
    }

    #[test]
    fn starving_tier_emigrates() {
        let mut p = Player::new_human(0);
        p.population[2] = 200; // citizens
        p.satisfaction[2] = 0;
        update_population_growth(&mut p);
        assert!(p.population[2] < 200, "starving citizens should leave");
    }

    #[test]
    fn severe_shortages_lists_only_starved_goods() {
        let mut p = Player::new_human(0);
        // Slot 0 (Food): demand 100, supply 30 → severe
        p.demands[0].demand = 100;
        p.demands[0].supply = 30;
        // Slot 1 (Cloth): demand 100, supply 60 → not severe
        p.demands[1].demand = 100;
        p.demands[1].supply = 60;
        // Slot 2 (Alcohol): demand 0, supply 0 → ignored
        let s = severe_shortages(&p);
        assert_eq!(s, vec![Good::Food]);
    }

    #[test]
    fn empty_tier_stays_empty() {
        let mut p = Player::new_human(0);
        // sat is 128 by default but pop is 0
        update_population_growth(&mut p);
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
