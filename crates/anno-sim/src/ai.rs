//! AI controller for computer players.
//!
//! The source PLAYER4 state is parsed, but its command and pacing semantics
//! are not yet decoded. Controllers therefore preserve scenario state without
//! synthesizing construction, military, tax, market, or route commands.

use crate::player::{Player, PlayerState};
use crate::types::Good;

/// AI personality type (maps to strategy selector at personality offset +2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AiPersonality {
    Economic = 0,
    Military = 1,
    Balanced = 2,
}

/// Difficulty level retained from the AI controller snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Difficulty {
    Easy = 0,
    Medium = 1,
    Hard = 2,
    Expert = 3,
}

/// Decode PLAYER4's `slot_u16_0x18` byte into an
/// `(AiPersonality, Difficulty)` pair. The raw corpus values are parsed, but
/// their binary semantics are not pinned; until they are, do not let this
/// field synthesize AI strength/personality differences.
pub fn personality_from_slot_byte(b: u16) -> (AiPersonality, Difficulty) {
    let _ = b;
    (AiPersonality::Economic, Difficulty::Medium)
}

/// AI state for a single player.
#[derive(Debug, Clone)]
pub struct AiController {
    pub player_idx: u8,
    pub personality: AiPersonality,
    pub difficulty: Difficulty,

    /// Retained cooldown fields from persisted AI state.
    pub build_cooldown: u32,
    pub military_cooldown: u32,
    pub trade_cooldown: u32,

    /// Retained build phase from persisted AI state.
    pub build_phase: u8,

    /// Retained construction count from persisted AI state.
    pub buildings_placed: u32,
}

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

    /// Main AI tick — called each economy tick (10 000 ms).
    /// Returns a list of actions the AI wants to take.
    pub fn tick(
        &mut self,
        player: &Player,
        _buildings: &[crate::building::BuildingInstance],
        _building_defs: &[crate::building::BuildingDef],
        _warehouses: &[crate::warehouse::Warehouse],
    ) -> Vec<AiAction> {
        if player.state != PlayerState::AiActive {
            return Vec::new();
        }

        Vec::new()
    }
}

/// Reserved AI actions awaiting decoded source command semantics.
#[derive(Debug, Clone)]
pub enum AiAction {
    /// Reserved for decoded source AI construction commands.
    RequestBuild { good: Good, priority: u8 },
    /// Reserved for decoded source AI tax commands.
    SetTaxRate { tier: u8, rate: u8 },
    /// Reserved for decoded source AI market commands.
    SellExcess,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn rich_ai_does_not_accelerate_cooldowns_from_gold_stockpile() {
        let mut ai = AiController::new(1, AiPersonality::Economic, Difficulty::Medium);
        ai.build_cooldown = 8;
        ai.military_cooldown = 4;
        ai.trade_cooldown = 6;
        let mut player = Player::new_ai(1, 0);
        player.gold = 20_000;

        let _ = ai.tick(&player, &[], &[], &[]);

        assert_eq!(ai.build_cooldown, 8);
        assert_eq!(ai.military_cooldown, 4);
        assert_eq!(ai.trade_cooldown, 6);
    }

    #[test]
    fn poor_ai_does_not_halve_cooldowns() {
        let mut ai = AiController::new(1, AiPersonality::Economic, Difficulty::Medium);
        ai.build_cooldown = 8;
        let mut player = Player::new_ai(1, 0);
        player.gold = 5_000;
        let _ = ai.tick(&player, &[], &[], &[]);
        assert_eq!(ai.build_cooldown, 8);
    }

    #[test]
    fn economic_ai_does_not_synthesize_build_request() {
        let mut ai = AiController::new(1, AiPersonality::Economic, Difficulty::Medium);
        let mut player = Player::new_ai(1, 0);
        player.population[0] = 50; // Some pioneers
        player.gold = 5000;
        player.total_population = 50;

        let actions = ai.tick(&player, &[], &[], &[]);

        assert!(actions.is_empty());
    }

    #[test]
    fn ai_does_not_synthesize_tax_change_from_satisfaction() {
        let mut ai = AiController::new(1, AiPersonality::Economic, Difficulty::Medium);
        let mut player = Player::new_ai(1, 0);
        player.population[0] = 100;
        player.satisfaction[0] = 40; // Below 64 threshold
        player.tax_rates[0] = 64; // Above 32 minimum

        let actions = ai.tick(&player, &[], &[], &[]);

        assert!(actions.is_empty());
    }

    #[test]
    fn personality_from_slot_byte_does_not_invent_unpinned_mapping() {
        for b in [0, 1, 2, 3, 4, 5, 6, 7, 99] {
            assert_eq!(
                personality_from_slot_byte(b),
                (AiPersonality::Economic, Difficulty::Medium),
                "slot_u16_0x18={b} must not synthesize AI behavior"
            );
        }
    }

    #[test]
    fn military_ai_does_not_synthesize_unit_request() {
        for diff in [
            Difficulty::Easy,
            Difficulty::Medium,
            Difficulty::Hard,
            Difficulty::Expert,
        ] {
            let mut ai = AiController::new(1, AiPersonality::Military, diff);
            let mut player = Player::new_ai(1, 0);
            player.population[0] = 200;
            player.gold = 5000;
            player.total_population = 200;

            let actions = ai.tick(&player, &[], &[], &[]);
            assert!(
                actions.is_empty(),
                "Military AI must not synthesize units at {:?}: {actions:?}",
                diff
            );
        }
    }
}
