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
const DEMAND_GOODS: [Good; NUM_DEMAND_CATEGORIES] = [
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
