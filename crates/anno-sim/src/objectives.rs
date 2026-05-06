//! Scenario objectives: small, declarative goals the player can pursue,
//! evaluated against `Simulation` state each economy tick.
//!
//! Objectives complete in-place (a `bool` flag flips to true) and never
//! revert. The game binary draws them in a panel and posts a chat-log
//! event on each completion.

use crate::types::{Good, PopTier};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Objective {
    /// Player owns at least `count` people in `tier`.
    ReachPopulation { tier: u8, count: u32 },
    /// Player's total inhabitants (summed across all tiers) reach
    /// at least `count`. Mirrors the AUFTRAG4 triple's `total`
    /// component, which is independent of any tier sub-goal — see
    /// `MissionGoals` in anno-formats.
    ReachTotalPopulation { count: u32 },
    /// Player has at least `count` built buildings whose def `prod_kind`
    /// matches the value (case-insensitive).
    Build { prod_kind: String, count: u32 },
    /// Player's gold balance reaches at least `amount`.
    AccumulateGold { amount: i32 },
    /// Player has at least `qty` of `good` summed across active warehouses.
    StockGood { good: Good, qty: u32 },
    /// Player holds a simultaneous monopoly on every listed good. A
    /// player has a monopoly on a good when they are the only player
    /// with at least one production building producing it. Manual
    /// section 11.7.2.4: "You may also set the attainment of two
    /// monopolies simultaneously as the assignment goal."
    Monopolies { goods: Vec<Good> },
    /// "Support a fellow player" — manual sec. 11.7.2.5: scenario
    /// completes when the named other player slot reaches at least
    /// `target_population` total inhabitants. Used for cooperative
    /// missions where the human is asked to bootstrap an AI ally.
    SupportFellowPlayer { who: u8, target_population: u32 },
}

impl Objective {
    pub fn label(&self) -> String {
        match self {
            Objective::ReachPopulation { tier, count } => {
                let name = match *tier {
                    0 => "Pioneers", 1 => "Settlers", 2 => "Citizens",
                    3 => "Merchants", _ => "Aristocrats",
                };
                format!("Reach {count} {name}")
            }
            Objective::ReachTotalPopulation { count } => {
                format!("Reach {count} total inhabitants")
            }
            Objective::Build { prod_kind, count } => {
                format!("Build {count} × {prod_kind}")
            }
            Objective::AccumulateGold { amount } => {
                format!("Accumulate {amount} gold")
            }
            Objective::StockGood { good, qty } => {
                format!("Stockpile {qty} {:?}", good)
            }
            Objective::Monopolies { goods } => {
                let names: Vec<String> = goods.iter()
                    .map(|g| format!("{g:?}")).collect();
                format!("Monopoly: {}", names.join(" + "))
            }
            Objective::SupportFellowPlayer { who, target_population } => {
                format!("Help player {} reach {} pop", who, target_population)
            }
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ObjectiveSet {
    pub items: Vec<(Objective, bool)>,
}

impl ObjectiveSet {
    pub fn new(items: Vec<Objective>) -> Self {
        Self {
            items: items.into_iter().map(|o| (o, false)).collect(),
        }
    }

    /// Evaluate every unfulfilled objective against the given player view.
    /// Returns the indices that flipped from false → true this call.
    /// `all_players` is needed for cross-player checks like
    /// `SupportFellowPlayer`; pass `&[player.clone()]` if only the
    /// owner's slot matters.
    pub fn evaluate(
        &mut self,
        player: &crate::player::Player,
        buildings: &[crate::building::BuildingInstance],
        defs: &[crate::building::BuildingDef],
        warehouses: &[crate::warehouse::Warehouse],
        all_players: &[crate::player::Player],
        owner: u8,
    ) -> Vec<usize> {
        let mut newly_done = Vec::new();
        for (idx, (obj, done)) in self.items.iter_mut().enumerate() {
            if *done { continue; }
            let met = match obj {
                Objective::ReachPopulation { tier, count } => {
                    let t = (*tier as usize).min(crate::types::NUM_POP_TIERS - 1);
                    player.population[t] >= *count
                }
                Objective::ReachTotalPopulation { count } => {
                    player.total_population() >= *count
                }
                Objective::Build { prod_kind, count } => {
                    let needle = prod_kind.to_ascii_uppercase();
                    let n = buildings.iter()
                        .filter(|b| b.owner == owner && b.active && b.is_built()
                            && (b.def_id as usize) < defs.len()
                            && defs[b.def_id as usize]
                                .prod_kind.to_ascii_uppercase() == needle)
                        .count();
                    n as u32 >= *count
                }
                Objective::AccumulateGold { amount } => player.gold >= *amount,
                Objective::StockGood { good, qty } => {
                    let total: u32 = warehouses.iter()
                        .filter(|w| w.active && w.owner == owner)
                        .map(|w| w.stock(*good) as u32)
                        .sum();
                    total >= *qty
                }
                Objective::SupportFellowPlayer { who, target_population } => {
                    let who_idx = *who as usize;
                    if who_idx >= all_players.len() { false }
                    else {
                        all_players[who_idx].total_population >= *target_population
                    }
                }
                Objective::Monopolies { goods } => {
                    // For every listed good: the owner produces it
                    // somewhere, and no other player does.
                    goods.iter().all(|g| {
                        let mut me = false;
                        let mut other = false;
                        for b in buildings {
                            if !b.active || !b.is_built() { continue; }
                            let def_id = b.def_id as usize;
                            if def_id >= defs.len() { continue; }
                            if defs[def_id].output_good != *g { continue; }
                            if b.owner == owner { me = true; }
                            else { other = true; }
                        }
                        me && !other
                    })
                }
            };
            if met {
                *done = true;
                newly_done.push(idx);
            }
        }
        newly_done
    }

    /// `(completed, total)` count for the HUD.
    pub fn progress(&self) -> (usize, usize) {
        let done = self.items.iter().filter(|(_, d)| *d).count();
        (done, self.items.len())
    }

    /// Default starter objectives — used when no scenario specifies its own.
    pub fn default_starter() -> Self {
        Self::new(vec![
            Objective::ReachPopulation { tier: PopTier::Settler as u8, count: 50 },
            Objective::Build { prod_kind: "MARKT".into(), count: 1 },
            Objective::AccumulateGold { amount: 10_000 },
            Objective::StockGood { good: Good::Tools, qty: 50 },
        ])
    }

    /// Build an objective set from a parsed AUFTRAG4 scenario
    /// briefing. Each `(total, tier, at_tier)` triple decodes to:
    ///
    ///   • `ReachTotalPopulation { count: total }` — always.
    ///   • `ReachPopulation { tier, count: at_tier }` — only when
    ///     `tier` is set and `at_tier > 0`. Cooperation's
    ///     `[2000, 4, 1300]` therefore yields TWO objectives:
    ///     "Reach 2000 total inhabitants" + "Reach 1300
    ///     Aristocrats", both of which must clear before the
    ///     scenario is won.
    ///
    /// `MISSION_FLAG_COOPERATIVE` adds a
    /// `SupportFellowPlayer { who: 1, .. }` (the conventional
    /// cooperative neighbour slot is the first AI rival, slot 1).
    ///
    /// Returns an empty set when the mission carries no flagged
    /// goals — caller can fall back to `default_starter`.
    pub fn from_mission_goals(
        goals: &anno_formats::szs::MissionGoals,
    ) -> Self {
        let mut items = Vec::new();
        for slot in [goals.primary, goals.secondary, goals.tertiary] {
            if let Some(p) = slot {
                items.push(Objective::ReachTotalPopulation {
                    count: p.total,
                });
                if let Some(tier) = p.tier {
                    if p.at_tier > 0 {
                        items.push(Objective::ReachPopulation {
                            tier,
                            count: p.at_tier,
                        });
                    }
                }
            }
        }
        if let Some(target) = goals.cooperative_population {
            items.push(Objective::SupportFellowPlayer {
                who: 1,
                target_population: target,
            });
        }
        Self::new(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn from_mission_goals_translates_triples_and_cooperative() {
        use anno_formats::szs::{MissionGoals, PopulationGoal};

        // Plague-of-Pirates style: just primary total, no tier
        // sub-goal. One ReachTotalPopulation only.
        let s = ObjectiveSet::from_mission_goals(&MissionGoals {
            primary: Some(PopulationGoal { total: 5_000, tier: None, at_tier: 0 }),
            ..Default::default()
        });
        assert_eq!(s.items.len(), 1);
        assert!(matches!(s.items[0].0,
            Objective::ReachTotalPopulation { count: 5_000 }));

        // Cooperation [2000, 4, 1300]: total + tier sub-goal,
        // both of which must clear.
        let s = ObjectiveSet::from_mission_goals(&MissionGoals {
            primary: Some(PopulationGoal { total: 2_000, tier: Some(4), at_tier: 1_300 }),
            ..Default::default()
        });
        assert_eq!(s.items.len(), 2);
        assert!(matches!(s.items[0].0,
            Objective::ReachTotalPopulation { count: 2_000 }));
        assert!(matches!(s.items[1].0,
            Objective::ReachPopulation { tier: 4, count: 1_300 }));

        // Good-Neighbors: primary tier sub-goal + cooperative
        // neighbour. Three objectives: total, at-tier, support.
        let s = ObjectiveSet::from_mission_goals(&MissionGoals {
            primary: Some(PopulationGoal { total: 1_000, tier: Some(3), at_tier: 500 }),
            cooperative_population: Some(1_000),
            ..Default::default()
        });
        assert_eq!(s.items.len(), 3);
        assert!(matches!(s.items[0].0,
            Objective::ReachTotalPopulation { count: 1_000 }));
        assert!(matches!(s.items[1].0,
            Objective::ReachPopulation { tier: 3, count: 500 }));
        assert!(matches!(s.items[2].0,
            Objective::SupportFellowPlayer { who: 1, target_population: 1_000 }));

        // The Continent: three city goals, each a triple with
        // tier + at_tier — six objectives total (3 × total +
        // 3 × at-tier).
        let triple = PopulationGoal { total: 5_000, tier: Some(4), at_tier: 5_000 };
        let s = ObjectiveSet::from_mission_goals(&MissionGoals {
            primary: Some(triple),
            secondary: Some(triple),
            tertiary: Some(triple),
            ..Default::default()
        });
        assert_eq!(s.items.len(), 6);

        // Empty MissionGoals → empty set, caller falls back to
        // default_starter.
        let s = ObjectiveSet::from_mission_goals(&MissionGoals::default());
        assert!(s.items.is_empty());
    }

    #[test]
    fn total_population_objective_completes_on_sum() {
        // ReachTotalPopulation sums every tier rather than
        // looking at one slot — Plague's 5000 should clear when
        // the player has 3000 Settlers + 2500 Citizens.
        let mut set = ObjectiveSet::new(vec![
            Objective::ReachTotalPopulation { count: 5_000 },
        ]);
        let mut p = Player::new_human(0);
        p.population[1] = 3_000;
        p.population[2] = 1_000;
        let done = set.evaluate(&p, &[], &[], &[], &[p.clone()], 0);
        assert!(done.is_empty(), "4_000 < 5_000");
        p.population[2] = 2_500;
        let done = set.evaluate(&p, &[], &[], &[], &[p.clone()], 0);
        assert_eq!(done, vec![0]);
    }

    #[test]
    fn population_objective_completes() {
        let mut set = ObjectiveSet::new(vec![
            Objective::ReachPopulation { tier: 1, count: 100 },
        ]);
        let mut p = Player::new_human(0);
        p.population[1] = 50;
        let done = set.evaluate(&p, &[], &[], &[], &[p.clone()], 0);
        assert!(done.is_empty());
        assert_eq!(set.progress(), (0, 1));

        p.population[1] = 150;
        let done = set.evaluate(&p, &[], &[], &[], &[p.clone()], 0);
        assert_eq!(done, vec![0]);
        assert_eq!(set.progress(), (1, 1));
    }

    #[test]
    fn monopoly_completes_when_only_owner_produces() {
        use crate::building::{BuildingDef, BuildingInstance};
        use crate::types::ProductionType;
        let mk_def = |out: Good| BuildingDef {
            id: 0, category: 2, width: 1, height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: out, input_good_1: Good::None, input_good_2: Good::None,
            output_rate: 1, input_1_rate: 0, input_2_rate: 0,
            storage_capacity: 0, cycle_time_ms: 0,
            cost_gold: 0, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            required_fertility: None,
        };
        let defs = vec![mk_def(Good::Tools), mk_def(Good::Cloth)];
        let mk_b = |def_id: u16, owner: u8| {
            let mut b = BuildingInstance::new(def_id, 0, 1, 1, owner);
            b.construction_ms_remaining = 0;
            b
        };
        let buildings = vec![
            mk_b(0, 0), // owner 0: Tools
            mk_b(1, 0), // owner 0: Cloth
        ];
        let p = Player::new_human(0);
        let mut set = ObjectiveSet::new(vec![
            Objective::Monopolies {
                goods: vec![Good::Tools, Good::Cloth],
            },
        ]);
        let done = set.evaluate(&p, &buildings, &defs, &[], &[p.clone()], 0);
        assert_eq!(done, vec![0]);

        // If a rival also builds Tools, the monopoly is broken — but
        // the objective doesn't revert (objectives are sticky).
        let mut set2 = ObjectiveSet::new(vec![
            Objective::Monopolies { goods: vec![Good::Tools] },
        ]);
        let buildings2 = vec![mk_b(0, 0), mk_b(0, 1)];
        let done2 = set2.evaluate(&p, &buildings2, &defs, &[], &[p.clone()], 0);
        assert!(done2.is_empty(), "rival production blocks monopoly");
    }

    #[test]
    fn support_fellow_player_completes_when_target_pop_reached() {
        let mut set = ObjectiveSet::new(vec![
            Objective::SupportFellowPlayer { who: 1, target_population: 100 },
        ]);
        let mut p = Player::new_human(0);
        let mut ally = Player::new_ai(1, 0);
        ally.total_population = 50;
        let players = vec![p.clone(), ally.clone()];
        let done = set.evaluate(&p, &[], &[], &[], &players, 0);
        assert!(done.is_empty());
        ally.total_population = 150;
        p.gold = 1; // make this a real edit
        let players = vec![p.clone(), ally.clone()];
        let done = set.evaluate(&p, &[], &[], &[], &players, 0);
        assert_eq!(done, vec![0]);
    }

    #[test]
    fn gold_objective_does_not_revert() {
        let mut set = ObjectiveSet::new(vec![
            Objective::AccumulateGold { amount: 1000 },
        ]);
        let mut p = Player::new_human(0);
        p.gold = 1500;
        set.evaluate(&p, &[], &[], &[], &[p.clone()], 0);
        // Spend down — the flag stays set.
        p.gold = 0;
        set.evaluate(&p, &[], &[], &[], &[p.clone()], 0);
        assert_eq!(set.progress(), (1, 1));
    }
}
