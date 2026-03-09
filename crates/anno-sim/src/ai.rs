//! AI controller for computer players.
//!
//! Ported from FUN_0042b4b0 (AI main tick) and related functions.
//! Three personality types control decision-making:
//!   - Economic (Strategy 0): prioritizes building placement and production chains
//!   - Military (Strategy 1): prioritizes unit production and attacks
//!   - Balanced (Strategy 2): hybrid of economic and military
//!
//! AI operates on cooldown timers to avoid excessive computation per tick.

use crate::building::{BuildingDef, BuildingInstance};
use crate::player::{Player, PlayerState};
use crate::types::Good;
use crate::warehouse::Warehouse;

/// AI personality type (maps to strategy selector at personality offset +2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AiPersonality {
    Economic = 0,
    Military = 1,
    Balanced = 2,
}

/// Difficulty level (scales AI unit counts and aggression).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Difficulty {
    Easy = 0,
    Medium = 1,
    Hard = 2,
    Expert = 3,
}

/// Building construction priority for AI decision-making.
/// Ordered by importance within each phase.
#[derive(Debug, Clone)]
pub struct BuildPriority {
    pub good: Good,
    pub min_population: u32,
    pub max_count: u16,
}

/// AI state for a single player.
#[derive(Debug, Clone)]
pub struct AiController {
    pub player_idx: u8,
    pub personality: AiPersonality,
    pub difficulty: Difficulty,

    /// Cooldown timers (in economy ticks).
    pub build_cooldown: u32,
    pub military_cooldown: u32,
    pub trade_cooldown: u32,

    /// Current build phase (advances as population grows).
    pub build_phase: u8,

    /// Total buildings constructed by AI.
    pub buildings_placed: u32,
}

/// Standard building priority for economic personality.
/// Mirrors the original AI's construction order from FUN_00429aa0.
const ECONOMIC_PRIORITIES: &[BuildPriority] = &[
    // Phase 0: Pioneer basics
    BuildPriority { good: Good::Food, min_population: 0, max_count: 2 },
    BuildPriority { good: Good::Cloth, min_population: 0, max_count: 1 },
    // Phase 1: Settler needs
    BuildPriority { good: Good::Food, min_population: 100, max_count: 4 },
    BuildPriority { good: Good::Cloth, min_population: 100, max_count: 2 },
    BuildPriority { good: Good::Alcohol, min_population: 200, max_count: 2 },
    // Phase 2: Citizen needs
    BuildPriority { good: Good::TobaccoProducts, min_population: 300, max_count: 1 },
    BuildPriority { good: Good::Spices, min_population: 300, max_count: 1 },
    BuildPriority { good: Good::Tools, min_population: 200, max_count: 1 },
    BuildPriority { good: Good::Bricks, min_population: 200, max_count: 1 },
    // Phase 3: Merchant needs
    BuildPriority { good: Good::Cocoa, min_population: 500, max_count: 1 },
    BuildPriority { good: Good::Jewelry, min_population: 500, max_count: 1 },
];

impl AiController {
    pub fn new(player_idx: u8, personality: AiPersonality, difficulty: Difficulty) -> Self {
        Self {
            player_idx,
            personality,
            difficulty,
            build_cooldown: 0,
            military_cooldown: 0,
            trade_cooldown: 0,
            build_phase: 0,
            buildings_placed: 0,
        }
    }

    /// Main AI tick — called each economy tick (9999ms).
    /// Returns a list of actions the AI wants to take.
    pub fn tick(
        &mut self,
        player: &Player,
        buildings: &[BuildingInstance],
        building_defs: &[BuildingDef],
        warehouses: &[Warehouse],
    ) -> Vec<AiAction> {
        if player.state != PlayerState::AiActive {
            return Vec::new();
        }

        let mut actions = Vec::new();

        // Decrement cooldowns
        if self.build_cooldown > 0 {
            self.build_cooldown -= 1;
        }
        if self.military_cooldown > 0 {
            self.military_cooldown -= 1;
        }
        if self.trade_cooldown > 0 {
            self.trade_cooldown -= 1;
        }

        match self.personality {
            AiPersonality::Economic => {
                self.tick_economic(player, buildings, building_defs, warehouses, &mut actions);
            }
            AiPersonality::Military => {
                self.tick_economic(player, buildings, building_defs, warehouses, &mut actions);
                self.tick_military(player, &mut actions);
            }
            AiPersonality::Balanced => {
                self.tick_economic(player, buildings, building_defs, warehouses, &mut actions);
                if player.total_population > 200 {
                    self.tick_military(player, &mut actions);
                }
            }
        }

        // Tax rate adjustment
        self.adjust_taxes(player, &mut actions);

        // Gold management
        self.manage_gold(player, &mut actions);

        actions
    }

    /// Economic strategy: decide what to build.
    fn tick_economic(
        &mut self,
        player: &Player,
        buildings: &[BuildingInstance],
        building_defs: &[BuildingDef],
        warehouses: &[Warehouse],
        actions: &mut Vec<AiAction>,
    ) {
        if self.build_cooldown > 0 {
            return;
        }

        let total_pop = player.total_population;

        // Count existing production buildings by output good
        let mut good_counts: std::collections::HashMap<Good, u16> = std::collections::HashMap::new();
        for b in buildings {
            if b.owner == self.player_idx && b.active {
                if (b.def_id as usize) < building_defs.len() {
                    let def = &building_defs[b.def_id as usize];
                    if def.output_good != Good::None {
                        *good_counts.entry(def.output_good).or_default() += 1;
                    }
                }
            }
        }

        // Check priority list for unmet needs
        for priority in ECONOMIC_PRIORITIES {
            if total_pop < priority.min_population {
                continue;
            }

            let current = good_counts.get(&priority.good).copied().unwrap_or(0);
            if current < priority.max_count {
                // Check if we can afford it
                if player.gold > 500 {
                    actions.push(AiAction::RequestBuild {
                        good: priority.good,
                        priority: (priority.max_count - current) as u8,
                    });
                    self.build_cooldown = self.build_interval();
                    break; // One build decision per tick
                }
            }
        }

        // Check for supply shortages — if a demand is unmet, build more of that good
        for slot in &player.demands {
            if slot.demand > 0 && slot.supply < slot.demand / 2 {
                // Severe shortage — try to address it
                // (The RequestBuild above handles this via priority list)
            }
        }
    }

    /// Military strategy: decide about unit production.
    fn tick_military(&mut self, player: &Player, actions: &mut Vec<AiAction>) {
        if self.military_cooldown > 0 {
            return;
        }

        // Scale military by difficulty
        let unit_target = match self.difficulty {
            Difficulty::Easy => 1,
            Difficulty::Medium => 3,
            Difficulty::Hard => 6,
            Difficulty::Expert => 12,
        };

        // Only build military if economy is stable
        if player.gold > 2000 && player.total_population > 100 {
            actions.push(AiAction::RequestMilitary {
                unit_count: unit_target,
            });
            self.military_cooldown = 10; // 10 economy ticks (~100 seconds)
        }
    }

    /// Adjust tax rates based on satisfaction and gold.
    fn adjust_taxes(&self, player: &Player, actions: &mut Vec<AiAction>) {
        for tier in 0..5 {
            if player.population[tier] == 0 {
                continue;
            }

            let sat = player.satisfaction[tier];
            let current_tax = player.tax_rates[tier];

            // If satisfaction is high and we need money, raise taxes
            let new_tax = if sat > 96 && player.gold < 3000 {
                (current_tax + 8).min(96)
            } else if sat < 64 && current_tax > 32 {
                // Satisfaction low — lower taxes to prevent citizens leaving
                current_tax - 8
            } else {
                continue; // No change needed
            };

            if new_tax != current_tax {
                actions.push(AiAction::SetTaxRate {
                    tier: tier as u8,
                    rate: new_tax,
                });
            }
        }
    }

    /// Manage gold — sell excess resources if gold is low.
    fn manage_gold(&self, player: &Player, actions: &mut Vec<AiAction>) {
        if player.gold < 1000 {
            actions.push(AiAction::SellExcess);
        }
    }

    /// Build interval depends on difficulty (faster on higher difficulty).
    fn build_interval(&self) -> u32 {
        match self.difficulty {
            Difficulty::Easy => 8,    // ~80 seconds
            Difficulty::Medium => 5,  // ~50 seconds
            Difficulty::Hard => 3,    // ~30 seconds
            Difficulty::Expert => 2,  // ~20 seconds
        }
    }
}

/// Actions the AI wants to take. Applied by the simulation dispatcher.
#[derive(Debug, Clone)]
pub enum AiAction {
    /// Request construction of a building producing this good.
    RequestBuild {
        good: Good,
        priority: u8,
    },
    /// Request military unit production.
    RequestMilitary {
        unit_count: u32,
    },
    /// Adjust tax rate for a population tier.
    SetTaxRate {
        tier: u8,
        rate: u8,
    },
    /// Sell excess warehouse goods for gold.
    SellExcess,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn economic_ai_requests_food_first() {
        let mut ai = AiController::new(1, AiPersonality::Economic, Difficulty::Medium);
        let mut player = Player::new_ai(1, 0);
        player.population[0] = 50; // Some pioneers
        player.gold = 5000;
        player.total_population = 50;

        let actions = ai.tick(&player, &[], &[], &[]);

        // Should request Food production first (highest priority for pioneers)
        let build_actions: Vec<_> = actions.iter().filter(|a| matches!(a, AiAction::RequestBuild { .. })).collect();
        assert!(!build_actions.is_empty(), "AI should request a build");
        if let AiAction::RequestBuild { good, .. } = &build_actions[0] {
            assert_eq!(*good, Good::Food, "First build should be Food");
        }
    }

    #[test]
    fn ai_lowers_taxes_when_satisfaction_low() {
        let ai = AiController::new(1, AiPersonality::Economic, Difficulty::Medium);
        let mut player = Player::new_ai(1, 0);
        player.population[0] = 100;
        player.satisfaction[0] = 40; // Below 64 threshold
        player.tax_rates[0] = 64;   // Above 32 minimum

        let mut actions = Vec::new();
        ai.adjust_taxes(&player, &mut actions);

        let tax_actions: Vec<_> = actions.iter().filter(|a| matches!(a, AiAction::SetTaxRate { .. })).collect();
        assert!(!tax_actions.is_empty(), "Should adjust taxes");
        if let AiAction::SetTaxRate { rate, .. } = tax_actions[0] {
            assert!(*rate < 64, "Should lower tax rate");
        }
    }

    #[test]
    fn military_scales_with_difficulty() {
        for (diff, expected_min) in [
            (Difficulty::Easy, 1),
            (Difficulty::Medium, 3),
            (Difficulty::Hard, 6),
            (Difficulty::Expert, 12),
        ] {
            let mut ai = AiController::new(1, AiPersonality::Military, diff);
            let mut player = Player::new_ai(1, 0);
            player.population[0] = 200;
            player.gold = 5000;
            player.total_population = 200;

            let actions = ai.tick(&player, &[], &[], &[]);
            let mil: Vec<_> = actions.iter().filter(|a| matches!(a, AiAction::RequestMilitary { .. })).collect();
            assert!(!mil.is_empty(), "Military AI should request units at {:?}", diff);
            if let AiAction::RequestMilitary { unit_count } = mil[0] {
                assert_eq!(*unit_count, expected_min);
            }
        }
    }
}
