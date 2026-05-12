//! Fidelity harness primitives for comparing implementation behaviour
//! against decoded Anno 1602 behaviour.
//!
//! This module records named timing and probability claims, marks whether
//! each one is binary-backed or still provisional, and provides a
//! deterministic scheduler trace that can be compared against captures
//! from the original executable.

/// Maximum delta time processed by one simulation step.
pub const MAX_STEP_MS: u32 = 200;

/// Scaled time above this is treated as a runaway frame.
pub const MAX_TOTAL_MS: u32 = 2_999;

pub const POPULATION_TICK_MS: u32 = 10_000;
pub const EVENT_TICK_MS: u32 = 10_000;
pub const SHIP_TICK_MS: u32 = 1_000;
pub const MARKET_TICK_MS: u32 = 1_000;
pub const MILITARY_TICK_MS: u32 = 10_000;
pub const DIPLOMACY_TICK_MS: u32 = 5_000;

pub const PIRATE_EVENT_GATE: u64 = 3;
pub const FREE_TRADER_TARGET_GATE_MASK: u64 = 3;
pub const FREE_TRADER_TARGET_GATE_DENOMINATOR: u64 = 4;
pub const FREE_TRADER_RESPAWN_COOLDOWN_TICKS: u32 = 60;

/// How strongly a fidelity claim is tied to original evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FidelityStatus {
    BinaryVerified,
    ManualVerified,
    CorpusVerified,
    Heuristic,
    Speculative,
    StandIn,
}

impl FidelityStatus {
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::BinaryVerified | Self::ManualVerified | Self::CorpusVerified
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRef {
    pub source: &'static str,
    pub location: &'static str,
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subsystem {
    Production,
    Population,
    Diplomacy,
    MarketCoverage,
    Ships,
    Military,
    Events,
}

impl Subsystem {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Population => "population",
            Self::Diplomacy => "diplomacy",
            Self::MarketCoverage => "market_coverage",
            Self::Ships => "ships",
            Self::Military => "military",
            Self::Events => "events",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingSpec {
    pub subsystem: Subsystem,
    pub interval_ms: u32,
    pub status: FidelityStatus,
    pub source: SourceRef,
}

pub const TIMING_SPECS: [TimingSpec; 7] = [
    TimingSpec {
        subsystem: Subsystem::Production,
        interval_ms: crate::production::PRODUCTION_TICK_MS,
        status: FidelityStatus::BinaryVerified,
        source: SourceRef {
            source: "decompiled/1602_exe.c",
            location: "16110",
            note: "production accumulator decremented by 1000 ms",
        },
    },
    TimingSpec {
        subsystem: Subsystem::Population,
        interval_ms: POPULATION_TICK_MS,
        status: FidelityStatus::BinaryVerified,
        source: SourceRef {
            source: "decompiled/1602_exe.c",
            location: "FUN_0047f8a0 / dispatcher",
            note: "population/economy tick aligned to 10 game ticks",
        },
    },
    TimingSpec {
        subsystem: Subsystem::Diplomacy,
        interval_ms: DIPLOMACY_TICK_MS,
        status: FidelityStatus::BinaryVerified,
        source: SourceRef {
            source: "decompiled/1602_exe.c",
            location: "FUN_00476350",
            note: "diplomacy/economy dispatcher tick",
        },
    },
    TimingSpec {
        subsystem: Subsystem::MarketCoverage,
        interval_ms: MARKET_TICK_MS,
        status: FidelityStatus::BinaryVerified,
        source: SourceRef {
            source: "decompiled/1602_exe.c",
            location: "FUN_004798e0 / 1602_exe.c:16110",
            note: "market/building service work runs on 1000 ms game tick",
        },
    },
    TimingSpec {
        subsystem: Subsystem::Ships,
        interval_ms: SHIP_TICK_MS,
        status: FidelityStatus::BinaryVerified,
        source: SourceRef {
            source: "decompiled/1602_exe.c",
            location: "FUN_004791b0",
            note: "ship movement ticker",
        },
    },
    TimingSpec {
        subsystem: Subsystem::Military,
        interval_ms: MILITARY_TICK_MS,
        status: FidelityStatus::BinaryVerified,
        source: SourceRef {
            source: "decompiled/1602_exe.c",
            location: "FUN_0047a020 / FUN_0047a8c0",
            note: "military and projectile dispatcher cadence",
        },
    },
    TimingSpec {
        subsystem: Subsystem::Events,
        interval_ms: EVENT_TICK_MS,
        status: FidelityStatus::Heuristic,
        source: SourceRef {
            source: "implementation",
            location: "Simulation::tick_events",
            note: "shares the economy cadence pending exact event scheduler port",
        },
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbabilitySpec {
    pub name: &'static str,
    pub denominator: u64,
    pub status: FidelityStatus,
    pub source: SourceRef,
}

pub const PROBABILITY_SPECS: [ProbabilitySpec; 5] = [
    ProbabilitySpec {
        name: "pirate_event_spawn",
        denominator: PIRATE_EVENT_GATE,
        status: FidelityStatus::Speculative,
        source: SourceRef {
            source: "implementation",
            location: "Simulation::tick_pirate_event",
            note: "active player trade ship gate is known; spawn odds still need binary pinning",
        },
    },
    ProbabilitySpec {
        name: "free_trader_target_gate",
        denominator: FREE_TRADER_TARGET_GATE_DENOMINATOR,
        status: FidelityStatus::BinaryVerified,
        source: SourceRef {
            source: "decompiled/1602_exe.c",
            location: "57713",
            note: "(rand() & 3) == 0 while seeking a trader target",
        },
    },
    ProbabilitySpec {
        name: "fire_ignition",
        denominator: crate::disaster::FIRE_IGNITION_GATE,
        status: FidelityStatus::Speculative,
        source: SourceRef {
            source: "implementation",
            location: "disaster.rs",
            note: "fire existence is RE-cited; ignition odds are not decoded",
        },
    },
    ProbabilitySpec {
        name: "fire_extinguish",
        denominator: crate::disaster::FIRE_EXTINGUISH_GATE,
        status: FidelityStatus::Speculative,
        source: SourceRef {
            source: "implementation",
            location: "disaster.rs",
            note: "extinguish odds are not decoded",
        },
    },
    ProbabilitySpec {
        name: "volcano_eruption",
        denominator: crate::disaster::VOLCANO_ERUPTION_GATE,
        status: FidelityStatus::Speculative,
        source: SourceRef {
            source: "implementation",
            location: "disaster.rs",
            note: "volcano visuals are RE-cited; eruption cadence is not decoded",
        },
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceEvent {
    pub at_ms: u32,
    pub subsystem: Subsystem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceMismatch {
    Different {
        index: usize,
        expected: TraceEvent,
        actual: TraceEvent,
    },
    Missing {
        index: usize,
        expected: TraceEvent,
    },
    Extra {
        index: usize,
        actual: TraceEvent,
    },
}

pub fn compare_trace(expected: &[TraceEvent], actual: &[TraceEvent]) -> Result<(), TraceMismatch> {
    let n = expected.len().min(actual.len());
    for i in 0..n {
        if expected[i] != actual[i] {
            return Err(TraceMismatch::Different {
                index: i,
                expected: expected[i],
                actual: actual[i],
            });
        }
    }
    if expected.len() > actual.len() {
        Err(TraceMismatch::Missing {
            index: actual.len(),
            expected: expected[actual.len()],
        })
    } else if actual.len() > expected.len() {
        Err(TraceMismatch::Extra {
            index: expected.len(),
            actual: actual[expected.len()],
        })
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TickScheduler {
    elapsed_ms: u32,
    accumulators: [(Subsystem, u32, u32); TIMING_SPECS.len()],
}

impl Default for TickScheduler {
    fn default() -> Self {
        let accumulators = TIMING_SPECS.map(|spec| (spec.subsystem, spec.interval_ms, 0));
        Self {
            elapsed_ms: 0,
            accumulators,
        }
    }
}

impl TickScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn advance_real_time(&mut self, real_dt_ms: u32, speed_multiplier: u32) -> Vec<TraceEvent> {
        self.advance_scaled(scaled_sim_ms(real_dt_ms, speed_multiplier))
    }

    pub fn advance_scaled(&mut self, mut scaled_ms: u32) -> Vec<TraceEvent> {
        let mut out = Vec::new();
        while scaled_ms > 0 {
            let dt = scaled_ms.min(MAX_STEP_MS);
            scaled_ms -= dt;
            self.elapsed_ms += dt;

            for (subsystem, interval_ms, accumulator_ms) in &mut self.accumulators {
                *accumulator_ms += dt;
                if *accumulator_ms >= *interval_ms {
                    *accumulator_ms -= *interval_ms;
                    out.push(TraceEvent {
                        at_ms: self.elapsed_ms,
                        subsystem: *subsystem,
                    });
                }
            }
        }
        out
    }
}

pub fn scaled_sim_ms(real_dt_ms: u32, speed_multiplier: u32) -> u32 {
    let scaled = real_dt_ms.saturating_mul(speed_multiplier);
    if scaled > MAX_TOTAL_MS {
        50
    } else {
        scaled
    }
}

pub fn unresolved_timing_specs() -> Vec<&'static TimingSpec> {
    TIMING_SPECS
        .iter()
        .filter(|spec| !spec.status.is_final())
        .collect()
}

pub fn unresolved_probability_specs() -> Vec<&'static ProbabilitySpec> {
    PROBABILITY_SPECS
        .iter()
        .filter(|spec| !spec.status.is_final())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::Simulation;

    fn count(events: &[TraceEvent], subsystem: Subsystem) -> usize {
        events.iter().filter(|e| e.subsystem == subsystem).count()
    }

    #[test]
    fn default_schedule_fires_expected_first_ten_seconds() {
        let mut scheduler = TickScheduler::new();
        let mut events = Vec::new();
        for _ in 0..50 {
            events.extend(scheduler.advance_real_time(200, 1));
        }

        assert_eq!(count(&events, Subsystem::Production), 10);
        assert_eq!(count(&events, Subsystem::MarketCoverage), 10);
        assert_eq!(count(&events, Subsystem::Ships), 10);
        assert_eq!(count(&events, Subsystem::Diplomacy), 2);
        assert_eq!(count(&events, Subsystem::Population), 1);
        assert_eq!(count(&events, Subsystem::Military), 1);
        assert_eq!(count(&events, Subsystem::Events), 1);

        assert!(events.contains(&TraceEvent {
            at_ms: 5_000,
            subsystem: Subsystem::Diplomacy,
        }));
        assert_eq!(
            events[events.len() - 7..],
            [
                TraceEvent {
                    at_ms: 10_000,
                    subsystem: Subsystem::Production
                },
                TraceEvent {
                    at_ms: 10_000,
                    subsystem: Subsystem::Population
                },
                TraceEvent {
                    at_ms: 10_000,
                    subsystem: Subsystem::Diplomacy
                },
                TraceEvent {
                    at_ms: 10_000,
                    subsystem: Subsystem::MarketCoverage
                },
                TraceEvent {
                    at_ms: 10_000,
                    subsystem: Subsystem::Ships
                },
                TraceEvent {
                    at_ms: 10_000,
                    subsystem: Subsystem::Military
                },
                TraceEvent {
                    at_ms: 10_000,
                    subsystem: Subsystem::Events
                },
            ]
        );
    }

    #[test]
    fn simulation_timer_snapshot_matches_fidelity_specs() {
        let sim = Simulation::new();
        let snapshot = sim.subsystem_timing_snapshot();
        let expected: Vec<_> = TIMING_SPECS
            .iter()
            .map(|spec| (spec.subsystem, spec.interval_ms))
            .collect();
        assert_eq!(snapshot, expected);
    }

    #[test]
    fn trace_comparison_reports_mismatch_location() {
        let expected = [TraceEvent {
            at_ms: 1_000,
            subsystem: Subsystem::Production,
        }];
        let actual = [TraceEvent {
            at_ms: 1_000,
            subsystem: Subsystem::Ships,
        }];
        let err = compare_trace(&expected, &actual).unwrap_err();
        assert!(matches!(
            err,
            TraceMismatch::Different {
                index: 0,
                expected: TraceEvent {
                    subsystem: Subsystem::Production,
                    ..
                },
                actual: TraceEvent {
                    subsystem: Subsystem::Ships,
                    ..
                },
            }
        ));
    }

    #[test]
    fn unresolved_specs_stay_visible() {
        let timing: Vec<_> = unresolved_timing_specs()
            .into_iter()
            .map(|spec| spec.subsystem)
            .collect();
        assert_eq!(timing, vec![Subsystem::Events]);

        let probability: Vec<_> = unresolved_probability_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        assert!(probability.contains(&"pirate_event_spawn"));
        assert!(probability.contains(&"fire_ignition"));
        assert!(probability.contains(&"volcano_eruption"));
    }

    #[test]
    fn runaway_frame_clamp_matches_simulation_policy() {
        assert_eq!(scaled_sim_ms(2_999, 1), 2_999);
        assert_eq!(scaled_sim_ms(3_000, 1), 50);
    }
}
