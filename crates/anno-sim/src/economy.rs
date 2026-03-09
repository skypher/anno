//! Population happiness and economy model.
//!
//! Ported from FUN_0047f8a0 (population/player settlement tick).
//! Timer: 9999ms intervals.
//!
//! Happiness model:
//! - For each demand category: fulfillment = (supply << 7) / demand (0-128 scale)
//! - Per-tier satisfaction is a weighted sum of fulfilled demands
//! - Satisfaction decays by 15/16 per tick
//! - Unrest decays by 255/256 per tick
//! - Citizens leave when satisfaction drops below 60% (0x4C)

use crate::player::{Player, NUM_DEMAND_CATEGORIES};
use crate::types::NUM_POP_TIERS;

/// Economy tick interval in milliseconds.
pub const ECONOMY_TICK_MS: u32 = 9999;

/// Satisfaction threshold below which citizens leave (60%).
pub const LEAVE_THRESHOLD: u8 = 0x4C;

/// Satisfaction decay factor: multiply by 15/16 each tick.
const SATISFACTION_DECAY_NUM: u32 = 15;
const SATISFACTION_DECAY_DEN: u32 = 16;

/// Update the player's economy for one tick.
pub fn tick_economy(player: &mut Player) {
    // 1. Calculate demand fulfillment ratios
    for slot in &mut player.demands {
        if slot.demand > 0 {
            let fulfillment = ((slot.supply as u64 * 128) / slot.demand as u64).min(128) as u8;

            // Shift history and add new sample
            slot.fulfillment_history[3] = slot.fulfillment_history[2];
            slot.fulfillment_history[2] = slot.fulfillment_history[1];
            slot.fulfillment_history[1] = slot.fulfillment_history[0];
            slot.fulfillment_history[0] = fulfillment;
        }
    }

    // 2. Apply satisfaction decay (only for tiers with population)
    for tier in 0..NUM_POP_TIERS {
        if player.population[tier] > 0 {
            let sat = player.satisfaction[tier] as u32;
            player.satisfaction[tier] =
                ((sat * SATISFACTION_DECAY_NUM) / SATISFACTION_DECAY_DEN) as u8;
        }
    }

    // 3. Apply economy balance
    let balance = player.net_balance();
    player.gold += balance;

    // 4. Track bankruptcy
    if player.is_bankrupt() {
        player.bankruptcy_ticks += 1;
    } else {
        player.bankruptcy_ticks = 0;
    }

    // 5. Update total population
    player.total_population = player.total_population();
}

/// Check if a population tier should grow (upgrade houses).
/// Returns true if satisfaction is high enough for growth.
pub fn can_grow(player: &Player, tier: usize) -> bool {
    if tier >= NUM_POP_TIERS {
        return false;
    }
    // Growth requires high satisfaction (>= 75% = 96/128)
    player.satisfaction[tier] >= 96
}

/// Check if citizens should leave (satisfaction too low).
pub fn should_citizens_leave(player: &Player, tier: usize) -> bool {
    if tier >= NUM_POP_TIERS {
        return false;
    }
    player.satisfaction[tier] < LEAVE_THRESHOLD
}
