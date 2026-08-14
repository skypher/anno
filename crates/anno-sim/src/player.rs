//! Player state and economy.
//!
//! Ported from player data at DAT_005b7680 (stride 0xA0, max 7 players)
//! and settlement data embedded within.

use crate::types::NUM_POP_TIERS;

/// Maximum number of players.
pub const MAX_PLAYERS: usize = 7;

/// Number of demand categories.
pub const NUM_DEMAND_CATEGORIES: usize = 8;

/// Per-tier tax income "per capita" table (`DAT_0061fa50`),
/// compiled from each population tier's `Steuer` field in
/// haeuser.cod at load. This is the *real* per-tier income
/// weight — it replaces the invented `TAX_TIER_MULTIPLIER_16`.
///
/// RE: `1602_exe.c:461add-461af5` (in `FUN_00460750`, the
/// haeuser.cod key/value parser). For the `Steuer` key the engine
/// loads the parsed value as a double, multiplies by the `.rdata`
/// constant at `0x496448` (= 16/3 ≈ 5.333333) and truncates toward
/// zero via `_ftol`, storing the result at `DAT_0061fa50 +
/// tier*0x48`.
///
/// The five `Objekt: BGRUPPE` tiers in haeuser.cod (Nummer 0..4)
/// carry `Steuer` = 1.4 / 1.6 / 2.1 / 2.4 / 2.6, so the stored
/// per-capita values are:
///   trunc(1.4·16/3)=7, trunc(1.6·16/3)=8, trunc(2.1·16/3)=11,
///   trunc(2.4·16/3)=12, trunc(2.6·16/3)=13.
pub const INCOME_PERCAPITA: [i32; NUM_POP_TIERS] = [7, 8, 11, 12, 13];

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

    /// 32-bit building-unlock bitmask — the runtime player field at
    /// `+0x6c` (`DAT_005b76ec`, player stride 0xA0).
    ///
    /// Bit `1 << (bauinfra - 1)` says the owner may place buildings
    /// carrying that `INFRA_*` id; `FUN_0042d530`
    /// (`1602_exe.c:33209-33265`) is the placement gate. Bits are set
    /// by the per-city sweep at the tail of `FUN_0047f8a0`
    /// (`1602_exe.c:91520-91581`) once the city's cumulative
    /// population clears the rung's `(BGruppe, Minwohn)` threshold,
    /// and are never cleared again.
    ///
    /// Seeded from the scenario's PLAYER4 slot dword at `+0x34`, which
    /// `FUN_00478160` (`1602_exe.c:85423`) copies straight into
    /// `player + 0x6c`.
    #[serde(default)]
    pub unlock_mask: u32,
}

impl Player {
    pub fn new_human(color_index: u8) -> Self {
        Self {
            state: PlayerState::HumanActive,
            gold: 20000, // standard starting gold
            color_index,
            population: [0; NUM_POP_TIERS],
            satisfaction: [128; NUM_POP_TIERS],
            // Default per-tier tax = 0x80 (128 = 100%). RE:
            // `1602_exe.c:73137` initializes the tax bytes at struct
            // `0x24d` to `0x80808080` on settlement creation, and the
            // economy tick resets any empty tier back to `0x80`
            // (`1602_exe.c:91405`).
            tax_rates: [128; NUM_POP_TIERS],
            demands: Default::default(),
            building_maintenance: 0,
            military_maintenance: 0,
            total_population: 0,
            bankruptcy_ticks: 0,
            ai_personality: 0,
            // No rung unlocked until the scenario's PLAYER4 slot
            // seeds one or a city sweep grants one.
            unlock_mask: 0,
        }
    }

    pub fn new_ai(color_index: u8, personality: u8) -> Self {
        Self {
            state: PlayerState::AiActive,
            ai_personality: personality,
            ..Self::new_human(color_index)
        }
    }

    /// Calculate gross tax income for this player.
    ///
    /// RE: `FUN_0047f740` (`1602_exe.c:91167`) sums, over the five
    /// population tiers, the per-tier income computed by
    /// `FUN_0047f370`/`FUN_0047f2f0` (`1602_exe.c:90924` / `:90886`):
    ///
    /// ```text
    ///   base        = INCOME_PERCAPITA[tier] * pop[tier] * 6 / 32   (FUN_0047f2f0)
    ///   tier_income = base * tax_rate[tier] / 128                   (FUN_0047f370)
    /// ```
    ///
    /// Both divisions truncate toward zero (the decompiled
    /// `(x + (x>>31 & mask)) >> shift` idiom — a `>>5` then a `>>7`;
    /// every operand here is non-negative, so it is a plain floor,
    /// and the two truncations are applied separately to stay
    /// bit-exact with the original). `tax_rate[tier]` is the per-tier
    /// tax byte at struct `0x24d+tier` on a 0..=128 scale (0..=100%).
    ///
    /// There is **no satisfaction term** in income: satisfaction
    /// (struct `0x248`) drives population growth, not gold. The former
    /// `× satisfaction × TAX_TIER_MULTIPLIER_16 / (128·128·16)` factors
    /// were invented approximations and have been removed.
    pub fn calculate_income(&self) -> i32 {
        let mut income = 0i32;
        for tier in 0..NUM_POP_TIERS {
            // FUN_0047f2f0: percapita * pop * 6 / 32.
            let base = INCOME_PERCAPITA[tier] * self.population[tier] as i32 * 6 / 32;
            // FUN_0047f370 / FUN_0047f740 inner: base * tax_rate / 128.
            income += base * self.tax_rates[tier] as i32 / 128;
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
    fn tax_income_matches_source_percapita_table() {
        // 100 inhabitants in a single tier at 100% tax. Values are
        // the exact source formula: INCOME_PERCAPITA[tier]*100*6/32.
        // Pioneer  : 7*100*6/32  = 4200/32 = 131
        // Settler  : 8*100*6/32  = 4800/32 = 150
        // Citizen  : 11*100*6/32 = 6600/32 = 206
        // Merchant : 12*100*6/32 = 7200/32 = 225
        // Aristocrat:13*100*6/32 = 7800/32 = 243
        let mk = |tier: usize| {
            let mut p = Player::new_human(0);
            p.population[tier] = 100;
            p.tax_rates[tier] = 128;
            p.calculate_income()
        };
        assert_eq!([mk(0), mk(1), mk(2), mk(3), mk(4)], [131, 150, 206, 225, 243]);
        // Strictly increasing with tier (the real Steuer progression).
        assert!(mk(0) < mk(1) && mk(1) < mk(2) && mk(2) < mk(3) && mk(3) < mk(4));
    }

    #[test]
    fn income_independent_of_satisfaction() {
        // Income has NO satisfaction term (FUN_0047f740): satisfaction
        // drives population growth, not gold. Vary satisfaction across
        // its whole range and income must not move.
        let base = {
            let mut p = Player::new_human(0);
            p.population[2] = 250;
            p.tax_rates[2] = 128;
            p.satisfaction[2] = 128;
            p.calculate_income()
        };
        for sat in [0u8, 32, 64, 96, 128] {
            let mut p = Player::new_human(0);
            p.population[2] = 250;
            p.tax_rates[2] = 128;
            p.satisfaction[2] = sat;
            assert_eq!(p.calculate_income(), base, "satisfaction {sat} changed income");
        }
    }

    #[test]
    fn income_zero_without_tax_or_population() {
        let mut p = Player::new_human(0);
        p.population[0] = 100;
        p.tax_rates[0] = 0; // 0% tax → no income
        assert_eq!(p.calculate_income(), 0);

        let mut p = Player::new_human(0);
        // pop all zero (default) → no income regardless of tax
        assert_eq!(p.calculate_income(), 0);
    }
}
