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
    /// Player has at least `count` built buildings whose def `prod_kind`
    /// matches the value (case-insensitive).
    Build { prod_kind: String, count: u32 },
    /// Player's gold balance reaches at least `amount`.
    AccumulateGold { amount: i32 },
    /// Player has at least `qty` of `good` summed across active warehouses.
    StockGood { good: Good, qty: u32 },
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
            Objective::Build { prod_kind, count } => {
                format!("Build {count} × {prod_kind}")
            }
            Objective::AccumulateGold { amount } => {
                format!("Accumulate {amount} gold")
            }
            Objective::StockGood { good, qty } => {
                format!("Stockpile {qty} {:?}", good)
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
    pub fn evaluate(
        &mut self,
        player: &crate::player::Player,
        buildings: &[crate::building::BuildingInstance],
        defs: &[crate::building::BuildingDef],
        warehouses: &[crate::warehouse::Warehouse],
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn population_objective_completes() {
        let mut set = ObjectiveSet::new(vec![
            Objective::ReachPopulation { tier: 1, count: 100 },
        ]);
        let mut p = Player::new_human(0);
        p.population[1] = 50;
        let done = set.evaluate(&p, &[], &[], &[], 0);
        assert!(done.is_empty());
        assert_eq!(set.progress(), (0, 1));

        p.population[1] = 150;
        let done = set.evaluate(&p, &[], &[], &[], 0);
        assert_eq!(done, vec![0]);
        assert_eq!(set.progress(), (1, 1));
    }

    #[test]
    fn gold_objective_does_not_revert() {
        let mut set = ObjectiveSet::new(vec![
            Objective::AccumulateGold { amount: 1000 },
        ]);
        let mut p = Player::new_human(0);
        p.gold = 1500;
        set.evaluate(&p, &[], &[], &[], 0);
        // Spend down — the flag stays set.
        p.gold = 0;
        set.evaluate(&p, &[], &[], &[], 0);
        assert_eq!(set.progress(), (1, 1));
    }
}
