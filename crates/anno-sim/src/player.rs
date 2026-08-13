//! Player state and economy.
//!
//! Ported from player data at DAT_005b7680 (stride 0xA0, max 7 players)
//! and settlement data embedded within.

use crate::types::NUM_POP_TIERS;

/// Maximum number of players.
pub const MAX_PLAYERS: usize = 7;

/// Number of demand categories.
pub const NUM_DEMAND_CATEGORIES: usize = 8;

/// Per-tier tax multiplier in 16-fixed-point. The canonical
/// Anno 1602 progression has higher tiers paying more gold
/// per inhabitant than lower tiers — the manual quotes ratios
/// of roughly 1 (Pioneer) → 2 → 3 → 5 → 8 (Aristocrat). We
/// keep the Citizen tier near 16 (= 1.0×) so existing
/// economy tuning stays close to its legacy curve, scaling
/// Pioneer down and Aristocrat up around that anchor.
pub const TAX_TIER_MULTIPLIER_16: [u8; NUM_POP_TIERS] = [
    4,  // Pioneer    (0.25×)
    8,  // Settler    (0.5×)
    16, // Citizen    (1.0× = legacy baseline)
    24, // Merchant   (1.5×)
    32, // Aristocrat (2.0×)
];

/// Bankruptcy threshold (gold balance).
///
/// RE: `1602_exe.c:84682` (settlement collapse in `FUN_00476...`),
/// where the "still solvent" test is `-0x3e9 < gold`
/// (`-0x3e9 == -1001`). Gold at or below -1001 makes the player
/// eligible for the bankruptcy game-over counter, so -1001 is the
/// highest balance that already counts as bankrupt (hence the `<=`
/// comparison in `is_bankrupt`).
pub const BANKRUPTCY_THRESHOLD: i32 = -1001;

/// Consecutive bankruptcy ticks before game over.
///
/// RE: `1602_exe.c:84685` — the per-player counter at struct
/// offset 0x9e is pre-incremented and compared `< 0x28`, so the
/// game-over path is taken once it reaches 0x28 == 40.
pub const BANKRUPTCY_GAME_OVER_TICKS: u32 = 40;

/// Player state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum PlayerState {
    HumanActive = 0,
    Empty = 7,
    AiDefending = 11,
    AiActive = 12,
    AiAllied = 13,
    Defeated = 14,
}

/// Per-demand-category tracking.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DemandSlot {
    pub demand: u32,
    pub supply: u32,
    /// Rolling history of fulfillment ratios (4 samples, 0-128 each).
    pub fulfillment_history: [u8; 4],
}

/// Player data.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Player {
    pub state: PlayerState,
    pub gold: i32,
    pub color_index: u8,

    /// Per-tier population counts.
    pub population: [u32; NUM_POP_TIERS],
    /// Per-tier satisfaction ratings (0-128 scale).
    pub satisfaction: [u8; NUM_POP_TIERS],
    /// Per-tier tax rates (0-128 scale).
    pub tax_rates: [u8; NUM_POP_TIERS],

    /// Resource demand/supply tracking.
    pub demands: [DemandSlot; NUM_DEMAND_CATEGORIES],

    /// Building maintenance costs (per tick).
    pub building_maintenance: u32,
    /// Military maintenance costs (per tick).
    pub military_maintenance: u32,

    /// Total population count.
    pub total_population: u32,

    /// Consecutive bankruptcy ticks.
    pub bankruptcy_ticks: u32,

    /// AI personality index.
    pub ai_personality: u8,
}

impl Player {
    pub fn new_human(color_index: u8) -> Self {
        Self {
            state: PlayerState::HumanActive,
            gold: 20000, // standard starting gold
            color_index,
            population: [0; NUM_POP_TIERS],
            satisfaction: [128; NUM_POP_TIERS],
            tax_rates: [64; NUM_POP_TIERS], // 50% default
            demands: Default::default(),
            building_maintenance: 0,
            military_maintenance: 0,
            total_population: 0,
            bankruptcy_ticks: 0,
            ai_personality: 0,
        }
    }

    pub fn new_ai(color_index: u8, personality: u8) -> Self {
        Self {
            state: PlayerState::AiActive,
            ai_personality: personality,
            ..Self::new_human(color_index)
        }
    }

    /// Calculate tax income for this player.
    ///
    /// Per-tier formula:
    ///   `pop × tax_rate × satisfaction × tier_multiplier
    ///    / (128 × 128 × 16)`
    ///
    /// where `tax_rate` and `satisfaction` are both in
    /// 0..=128 scale (=0..=100%) and `tier_multiplier` comes
    /// from `TAX_TIER_MULTIPLIER_16` — a 16-fixed-point per-
    /// tier scale that rises with population tier (the
    /// canonical Anno 1602 progression has Aristocrats paying
    /// roughly 8× Pioneers per inhabitant).
    ///
    /// The /16 denominator keeps the integer-math output near
    /// the legacy single-tier value at the Citizen baseline,
    /// so existing scenario tuning stays close to its prior
    /// gold curve.
    pub fn calculate_income(&self) -> i32 {
        let mut income = 0i32;
        for tier in 0..NUM_POP_TIERS {
            let mult = TAX_TIER_MULTIPLIER_16[tier] as i32;
            income += (self.population[tier] as i32
                * self.tax_rates[tier] as i32
                * self.satisfaction[tier] as i32
                * mult)
                / (128 * 128 * 16);
        }
        income
    }

    /// Calculate total running costs.
    pub fn calculate_costs(&self) -> i32 {
        (self.building_maintenance + self.military_maintenance) as i32
    }

    /// Net balance applied per economy tick.
    /// Original formula: (income - costs) / 6
    pub fn net_balance(&self) -> i32 {
        (self.calculate_income() - self.calculate_costs()) / 6
    }

    /// Check if player is bankrupt.
    ///
    /// Source (`1602_exe.c:84682`) treats the player as solvent
    /// while `-1001 < gold`; the bankruptcy counter advances once
    /// `gold <= -1001`. Use `<=` so the boundary value -1001 counts
    /// as bankrupt, matching the decompiled comparison.
    pub fn is_bankrupt(&self) -> bool {
        self.gold <= BANKRUPTCY_THRESHOLD
    }

    /// Check if game over due to sustained bankruptcy.
    pub fn is_game_over(&self) -> bool {
        self.bankruptcy_ticks >= BANKRUPTCY_GAME_OVER_TICKS
    }

    pub fn total_population(&self) -> u32 {
        self.population.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tax_income_scales_per_tier() {
        // At full satisfaction + tax_rate, an Aristocrat
        // should pay ~8× a Pioneer per inhabitant (the
        // canonical Anno 1602 manual ratio). Citizen sits at
        // the 1.0× legacy baseline.
        let mk = |tier: usize| {
            let mut p = Player::new_human(0);
            // 100 inhabitants in just this tier, 100% tax / sat.
            p.population[tier] = 100;
            p.tax_rates[tier] = 128;
            p.satisfaction[tier] = 128;
            p.calculate_income()
        };
        let pioneer = mk(0);
        let citizen = mk(2);
        let aristo = mk(4);
        assert!(citizen > 0);
        // Aristocrat should out-earn Pioneer by ≥4× per 100 pop.
        assert!(
            aristo >= pioneer * 4,
            "Aristocrat income {aristo} should be ≥4× Pioneer {pioneer}"
        );
        // Citizen ≈ legacy baseline (100 pop × full settings).
        // The TAX_TIER_MULTIPLIER_16 for Citizen is 16, so
        // the /16 division cancels and we recover the legacy
        // pop × rate × sat / 128² formula → 100.
        assert_eq!(citizen, 100);
    }

    #[test]
    fn calculate_income_zero_at_zero_satisfaction() {
        let mut p = Player::new_human(0);
        p.population[0] = 100;
        p.tax_rates[0] = 128;
        p.satisfaction[0] = 0;
        assert_eq!(p.calculate_income(), 0);
    }
}
