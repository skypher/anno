//! Main simulation dispatcher.
//!
//! Ported from FUN_00489670 (core simulation orchestrator).
//! Processes delta time in chunks of max 200ms, scaled by game speed multiplier.
//! Dispatches to 12 subsystem update functions on independent timers.

use crate::ai::{AiAction, AiController};
use crate::building::{BuildingDef, BuildingInstance};
use crate::carrier;
use crate::combat::{self, DiplomacyMatrix, MilitaryUnit};
use crate::coverage::CoverageMap;
use crate::ocean_map::OceanMap;
use crate::economy;
use crate::entity::{ActionType, Figure};
use crate::island_map::IslandMap;
use crate::population;
use crate::player::Player;
use crate::production;
use crate::trade::{self, TradeRoute, TradeShip};
use crate::types::TICKS_PER_MINUTE;
use crate::warehouse::Warehouse;

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
    pub warehouses: Vec<Warehouse>,
    pub island_maps: Vec<IslandMap>,
    pub ai_controllers: Vec<AiController>,
    pub military_units: Vec<MilitaryUnit>,
    pub diplomacy: DiplomacyMatrix,
    pub trade_routes: Vec<TradeRoute>,
    pub trade_ships: Vec<TradeShip>,
    pub coverage_maps: Vec<CoverageMap>,
    pub ocean_map: Option<OceanMap>,

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
            warehouses: Vec::new(),
            island_maps: Vec::new(),
            ai_controllers: Vec::new(),
            military_units: Vec::new(),
            diplomacy: DiplomacyMatrix::new(),
            trade_routes: Vec::new(),
            trade_ships: Vec::new(),
            coverage_maps: Vec::new(),
            ocean_map: None,

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

        // 5. Marketplace coverage
        if self.timer_market.advance(dt_ms) {
            self.tick_market_coverage();
        }

        // 6. Ships
        if self.timer_ships.advance(dt_ms) {
            self.tick_ships();
        }

        // 6. Military combat
        if self.timer_military.advance(dt_ms) {
            self.tick_military();
        }

        // Entity movement (every step)
        self.tick_entities(dt_ms);
    }

    fn tick_animations(&mut self) {
        // TODO: advance tile animation frames
    }

    fn tick_production(&mut self) {
        let mut new_carriers = Vec::new();

        for i in 0..self.buildings.len() {
            let def_id = self.buildings[i].def_id;
            if def_id as usize >= self.building_defs.len() {
                continue;
            }
            let def = self.building_defs[def_id as usize].clone();
            let produced = production::tick_building(
                &mut self.buildings[i],
                &def,
                self.timer_production.interval_ms,
            );

            if produced > 0 && production::needs_carrier(&self.buildings[i], &def) {
                // Check if this building already has an active carrier
                let has_carrier = self.figures.iter().any(|f| {
                    f.is_active()
                        && f.building_idx == i as u16
                        && matches!(
                            f.action,
                            ActionType::CarryingGoods | ActionType::Returning
                        )
                });

                if !has_carrier {
                    if let Some(mut c) =
                        carrier::try_spawn_carrier(
                            &mut self.buildings[i],
                            &def,
                            &self.warehouses,
                            &self.island_maps,
                        )
                    {
                        c.building_idx = i as u16;
                        new_carriers.push(c);
                    }
                }
            }
        }

        self.figures.extend(new_carriers);
    }

    fn tick_population(&mut self) {
        for (i, player) in self.players.iter_mut().enumerate() {
            // Update demands and consume goods from warehouses
            population::update_population_demands(player, &mut self.warehouses, i as u8);
            // Apply economy (gold balance, bankruptcy, satisfaction decay)
            economy::tick_economy(player);
        }

        // AI decision-making
        self.tick_ai();
    }

    fn tick_ai(&mut self) {
        for ai_idx in 0..self.ai_controllers.len() {
            let player_idx = self.ai_controllers[ai_idx].player_idx as usize;
            if player_idx >= self.players.len() {
                continue;
            }

            let actions = self.ai_controllers[ai_idx].tick(
                &self.players[player_idx],
                &self.buildings,
                &self.building_defs,
                &self.warehouses,
            );

            // Apply AI actions
            for action in actions {
                match action {
                    AiAction::SetTaxRate { tier, rate } => {
                        if (tier as usize) < 5 {
                            self.players[player_idx].tax_rates[tier as usize] = rate;
                        }
                    }
                    AiAction::RequestBuild { good, priority: _ } => {
                        // Log the request (actual building placement requires island map integration)
                        let _ = good; // Will be used when building placement is implemented
                    }
                    AiAction::RequestMilitary { unit_count: _ } => {
                        // Military unit production not yet implemented
                    }
                    AiAction::SellExcess => {
                        // Sell excess goods from warehouses for gold
                        let owner = self.ai_controllers[ai_idx].player_idx;
                        for wh in &mut self.warehouses {
                            if wh.owner == owner {
                                // Sell any goods above 20 units
                                let stock = wh.all_stock();
                                for (good, amount, _cap) in &stock {
                                    if *amount > 20 {
                                        let sell = amount - 20;
                                        wh.withdraw(*good, sell);
                                        // Gold per unit varies by good; approximate at 5 gold/unit
                                        self.players[player_idx].gold += sell as i32 * 5;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn tick_market_coverage(&mut self) {
        // Collect warehouse positions per island
        let mut wh_by_island: std::collections::HashMap<u8, Vec<(u16, u16, u16)>> =
            std::collections::HashMap::new();
        for wh in &self.warehouses {
            if wh.active {
                // Warehouse base radius = 22 (RADIUS_HQ from original binary)
                wh_by_island
                    .entry(wh.island_id)
                    .or_default()
                    .push((wh.tile_x, wh.tile_y, 22));
            }
        }

        // Recompute coverage for each island that has a coverage map
        for cov in &mut self.coverage_maps {
            let whs = wh_by_island.get(&cov.island_id).map(|v| v.as_slice()).unwrap_or(&[]);
            cov.recompute(&self.buildings, &self.building_defs, whs);
        }
    }

    fn tick_diplomacy(&mut self) {
        // TODO: AI diplomacy decisions
    }

    fn tick_ships(&mut self) {
        for ship_idx in 0..self.trade_ships.len() {
            if !self.trade_ships[ship_idx].active {
                continue;
            }
            let route_id = self.trade_ships[ship_idx].route_id;
            if let Some(route) = self.trade_routes.iter().find(|r| r.id == route_id) {
                let route = route.clone();
                let gold = trade::tick_trade_ship(
                    &mut self.trade_ships[ship_idx],
                    &route,
                    &mut self.warehouses,
                    self.ocean_map.as_ref(),
                );
                // Apply gold to ship owner
                let owner = self.trade_ships[ship_idx].owner as usize;
                if owner < self.players.len() {
                    self.players[owner].gold += gold;
                }
            }
        }
    }

    fn tick_military(&mut self) {
        if self.military_units.is_empty() {
            return;
        }

        let dead = combat::tick_combat(
            &mut self.military_units,
            &self.diplomacy,
            self.timer_military.interval_ms,
        );

        // Remove dead units (reverse order to preserve indices)
        let mut dead_sorted = dead;
        dead_sorted.sort_unstable();
        dead_sorted.dedup();
        for &idx in dead_sorted.iter().rev() {
            self.military_units.swap_remove(idx);
        }
    }

    fn tick_entities(&mut self, dt_ms: u32) {
        let mut despawn_indices = Vec::new();

        for (idx, figure) in self.figures.iter_mut().enumerate() {
            if !figure.is_active() {
                continue;
            }

            figure.move_timer_ms += dt_ms;
            if figure.speed > 0 && figure.move_timer_ms >= 100 {
                figure.move_timer_ms -= 100;

                match figure.action {
                    ActionType::CarryingGoods | ActionType::Returning => {
                        let arrived = carrier::step_carrier(figure);
                        if arrived {
                            let should_despawn = carrier::handle_arrival(
                                figure,
                                &mut self.warehouses,
                                &self.buildings,
                                &self.island_maps,
                            );
                            if should_despawn {
                                despawn_indices.push(idx);
                            }
                        }
                    }
                    _ => {
                        // Other action types not yet implemented
                    }
                }
            }
        }

        // Remove despawned figures (iterate in reverse to preserve indices)
        for &idx in despawn_indices.iter().rev() {
            self.figures.swap_remove(idx);
        }
    }

    /// Get the displayed game time as (minutes, seconds).
    pub fn display_time(&self) -> (u32, u32) {
        let minutes = self.game_clock / TICKS_PER_MINUTE;
        let seconds = (self.game_clock % TICKS_PER_MINUTE) / 10;
        (minutes, seconds)
    }
}
