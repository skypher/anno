//! Player state and economy.
//!
//! Ported from player data at DAT_005b7680 (stride 0xA0, max 7 players)
//! and settlement data embedded within.

use crate::types::{PopTier, NUM_POP_TIERS};

/// Maximum number of players.
pub const MAX_PLAYERS: usize = 7;

/// Number of demand categories.
pub const NUM_DEMAND_CATEGORIES: usize = 8;

/// Bankruptcy threshold (gold balance).
pub const BANKRUPTCY_THRESHOLD: i32 = -1001;

/// Consecutive bankruptcy ticks before game over.
pub const BANKRUPTCY_GAME_OVER_TICKS: u32 = 40;

/// Player state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Default)]
pub struct DemandSlot {
    pub demand: u32,
    pub supply: u32,
    /// Rolling history of fulfillment ratios (4 samples, 0-128 each).
    pub fulfillment_history: [u8; 4],
}

/// Player data.
#[derive(Debug, Clone)]
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
    /// Formula: sum over tiers of (population[tier] * tax_rate[tier] * satisfaction[tier] / 128)
    pub fn calculate_income(&self) -> i32 {
        let mut income = 0i32;
        for tier in 0..NUM_POP_TIERS {
            income += (self.population[tier] as i32
                * self.tax_rates[tier] as i32
                * self.satisfaction[tier] as i32)
                / (128 * 128);
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
    pub fn is_bankrupt(&self) -> bool {
        self.gold < BANKRUPTCY_THRESHOLD
    }

    /// Check if game over due to sustained bankruptcy.
    pub fn is_game_over(&self) -> bool {
        self.bankruptcy_ticks >= BANKRUPTCY_GAME_OVER_TICKS
    }

    pub fn total_population(&self) -> u32 {
        self.population.iter().sum()
    }
}
