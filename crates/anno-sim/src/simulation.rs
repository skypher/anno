//! Main simulation dispatcher.
//!
//! Ported from FUN_00489670 (core simulation orchestrator).
//! Processes delta time in chunks of max 200ms, scaled by game speed multiplier.
//! Dispatches to 12 subsystem update functions on independent timers.

use crate::building::{BuildingDef, BuildingInstance, MAX_BUILDINGS};
use crate::economy;
use crate::entity::{Figure, MAX_FIGURES};
use crate::player::{Player, MAX_PLAYERS};
use crate::production;
use crate::types::TICKS_PER_MINUTE;

/// Maximum delta time per simulation step (prevents physics jumps).
const MAX_STEP_MS: u32 = 200;

/// Delta time clamp if scaled time exceeds this (prevents runaway).
const MAX_TOTAL_MS: u32 = 2999;

/// Auto-save interval in game ticks (~10 minutes of game time).
pub const AUTOSAVE_INTERVAL_MS: u32 = 599_999;

/// Timer state for each subsystem.
#[derive(Debug, Clone)]
struct SubsystemTimer {
    accumulator_ms: u32,
    interval_ms: u32,
}

impl SubsystemTimer {
    fn new(interval_ms: u32) -> Self {
        Self {
            accumulator_ms: 0,
            interval_ms,
        }
    }

    /// Advance timer, returns true if a tick should fire.
    fn advance(&mut self, dt_ms: u32) -> bool {
        self.accumulator_ms += dt_ms;
        if self.accumulator_ms >= self.interval_ms {
            self.accumulator_ms -= self.interval_ms;
            true
        } else {
            false
        }
    }
}

/// The main game simulation state.
pub struct Simulation {
    /// Game clock in centiseconds (600 = 1 displayed minute).
    pub game_clock: u32,
    /// Fractional tick accumulator.
    clock_frac_ms: u32,

    /// Game speed multiplier (1 = normal).
    pub speed_multiplier: u32,

    /// Is the game paused?
    pub paused: bool,

    // Subsystem timers (matching original intervals)
    timer_animation: SubsystemTimer,  // 40000ms base
    timer_production: SubsystemTimer, // 999ms
    timer_population: SubsystemTimer, // 9999ms
    timer_citizen: SubsystemTimer,    // 15000ms
    timer_island: SubsystemTimer,     // 29999ms
    timer_events: SubsystemTimer,     // variable
    timer_ships: SubsystemTimer,      // 1000ms
    timer_market: SubsystemTimer,     // 1000ms
    timer_military: SubsystemTimer,   // 9999ms
    timer_projectile: SubsystemTimer, // 9999ms
    timer_diplomacy: SubsystemTimer,  // 4999ms

    // Game state
    pub players: Vec<Player>,
    pub buildings: Vec<BuildingInstance>,
    pub building_defs: Vec<BuildingDef>,
    pub figures: Vec<Figure>,

    pub autosave_timer_ms: u32,
}

impl Simulation {
    pub fn new() -> Self {
        Self {
            game_clock: 0,
            clock_frac_ms: 0,
            speed_multiplier: 1,
            paused: false,

            timer_animation: SubsystemTimer::new(40_000),
            timer_production: SubsystemTimer::new(999),
            timer_population: SubsystemTimer::new(9_999),
            timer_citizen: SubsystemTimer::new(15_000),
            timer_island: SubsystemTimer::new(29_999),
            timer_events: SubsystemTimer::new(10_000),
            timer_ships: SubsystemTimer::new(1_000),
            timer_market: SubsystemTimer::new(1_000),
            timer_military: SubsystemTimer::new(9_999),
            timer_projectile: SubsystemTimer::new(9_999),
            timer_diplomacy: SubsystemTimer::new(4_999),

            players: Vec::new(),
            buildings: Vec::new(),
            building_defs: Vec::new(),
            figures: Vec::new(),

            autosave_timer_ms: 0,
        }
    }

    /// Main simulation tick, called with real-time delta in milliseconds.
    pub fn tick(&mut self, real_dt_ms: u32) {
        if self.paused {
            return;
        }

        // Scale by game speed
        let mut remaining = real_dt_ms * self.speed_multiplier;
        if remaining > MAX_TOTAL_MS {
            remaining = 50; // Clamp runaway (matches original behavior)
        }

        // Process in chunks of MAX_STEP_MS
        while remaining > 0 {
            let dt = remaining.min(MAX_STEP_MS);
            remaining -= dt;

            self.step(dt);
        }

        // Advance game clock
        self.clock_frac_ms += real_dt_ms * self.speed_multiplier;
        while self.clock_frac_ms >= 100 {
            self.clock_frac_ms -= 100;
            self.game_clock += 1;
        }

        // Auto-save check
        self.autosave_timer_ms += real_dt_ms;
    }

    /// Single simulation step (max 200ms).
    fn step(&mut self, dt_ms: u32) {
        // 1. Tile animation
        if self.timer_animation.advance(dt_ms) {
            self.tick_animations();
        }

        // 2. Building production
        if self.timer_production.advance(dt_ms) {
            self.tick_production();
        }

        // 3. Population/economy
        if self.timer_population.advance(dt_ms) {
            self.tick_population();
        }

        // 4. Diplomacy
        if self.timer_diplomacy.advance(dt_ms) {
            self.tick_diplomacy();
        }

        // 5. Ships
        if self.timer_ships.advance(dt_ms) {
            self.tick_ships();
        }

        // Entity movement (every step)
        self.tick_entities(dt_ms);
    }

    fn tick_animations(&mut self) {
        // TODO: advance tile animation frames
    }

    fn tick_production(&mut self) {
        for i in 0..self.buildings.len() {
            if let Some(def) = self
                .building_defs
                .iter()
                .find(|d| d.id == self.buildings[i].def_id)
            {
                let def = def.clone();
                let produced = production::tick_building(
                    &mut self.buildings[i],
                    &def,
                    self.timer_production.interval_ms,
                );

                if produced > 0 && production::needs_carrier(&self.buildings[i], &def) {
                    // TODO: spawn carrier figure
                }
            }
        }
    }

    fn tick_population(&mut self) {
        for player in &mut self.players {
            economy::tick_economy(player);
        }
    }

    fn tick_diplomacy(&mut self) {
        // TODO: AI diplomacy decisions
    }

    fn tick_ships(&mut self) {
        // TODO: ship movement and trade routes
    }

    fn tick_entities(&mut self, dt_ms: u32) {
        for figure in &mut self.figures {
            if !figure.is_active() {
                continue;
            }

            figure.move_timer_ms += dt_ms;
            if figure.speed > 0 && figure.move_timer_ms >= 100 {
                figure.move_timer_ms -= 100;
                // TODO: dispatch based on figure.action type
            }
        }
    }

    /// Get the displayed game time as (minutes, seconds).
    pub fn display_time(&self) -> (u32, u32) {
        let minutes = self.game_clock / TICKS_PER_MINUTE;
        let seconds = (self.game_clock % TICKS_PER_MINUTE) / 10;
        (minutes, seconds)
    }
}
