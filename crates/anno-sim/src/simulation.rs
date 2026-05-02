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
use crate::history::EconomyHistory;
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

    /// Rolling economy/population history for the human player (slot 0).
    pub history: EconomyHistory,

    /// Footprint cleanup events from buildings destroyed in combat.
    /// `(island_id, tile_x, tile_y, width, height)`. The renderer drains
    /// this each frame to clear `Island::tiles` for the destroyed
    /// footprint and refresh display.
    pub tile_clears: Vec<(u8, u16, u16, u8, u8)>,

    /// Active scenario objectives for the human player. Re-evaluated each
    /// economy tick. The renderer reads `progress()` and the per-item
    /// `done` flag to draw the objectives panel.
    pub objectives: crate::objectives::ObjectiveSet,

    /// Indices of objectives that flipped to done since last drain.
    /// Game binary consumes these to push events into the chat log.
    pub objective_completions: Vec<usize>,

    /// xorshift RNG state for the event ticker. Persisted as a sim field
    /// only; not serialized into save files (each load reseeds).
    rng_state: u64,

    /// Per-good current market prices (indexed by `Good as u8`). Drifts
    /// from the static base in `prices::price_of` each market tick based
    /// on aggregate warehouse stock — a glut drives sell prices down,
    /// scarcity drives buy prices up.
    pub current_prices: Vec<crate::prices::GoodPrice>,

    /// Per-island fog-of-war bitmap. Lazily allocated on first sighting.
    pub exploration: Vec<crate::exploration::ExplorationMap>,
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

            history: EconomyHistory::new(),

            tile_clears: Vec::new(),

            objectives: crate::objectives::ObjectiveSet::default_starter(),
            objective_completions: Vec::new(),

            rng_state: 0xCBF29CE484222325,

            exploration: Vec::new(),

            current_prices: (0..31u8)
                .map(|i| {
                    let g = match i {
                        0 => crate::types::Good::None,
                        1 => crate::types::Good::Wood, 2 => crate::types::Good::Iron,
                        3 => crate::types::Good::Gold, 4 => crate::types::Good::Wool,
                        5 => crate::types::Good::Sugar, 6 => crate::types::Good::Tobacco,
                        7 => crate::types::Good::Cattle, 8 => crate::types::Good::Grain,
                        9 => crate::types::Good::Flour, 10 => crate::types::Good::Tools,
                        11 => crate::types::Good::Bricks, 12 => crate::types::Good::Swords,
                        13 => crate::types::Good::Muskets, 14 => crate::types::Good::Cannons,
                        15 => crate::types::Good::Food, 16 => crate::types::Good::Cloth,
                        17 => crate::types::Good::Alcohol,
                        18 => crate::types::Good::TobaccoProducts,
                        19 => crate::types::Good::Spices, 20 => crate::types::Good::Cocoa,
                        21 => crate::types::Good::Grapes, 22 => crate::types::Good::Stone,
                        23 => crate::types::Good::Ore, 24 => crate::types::Good::GoldOre,
                        25 => crate::types::Good::Hides, 26 => crate::types::Good::Cotton,
                        27 => crate::types::Good::Silk, 28 => crate::types::Good::Jewelry,
                        29 => crate::types::Good::Clothing, 30 => crate::types::Good::Fish,
                        _ => crate::types::Good::None,
                    };
                    crate::prices::price_of(g)
                })
                .collect(),
        }
    }

    /// Live price for a good, applying the market-dynamics modifier when
    /// available, falling back to the static base otherwise.
    pub fn current_price(&self, good: crate::types::Good) -> crate::prices::GoodPrice {
        let i = good as u8 as usize;
        self.current_prices.get(i).copied()
            .unwrap_or_else(|| crate::prices::price_of(good))
    }

    /// Recompute `current_prices` from aggregate warehouse stocks.
    /// Each good's price scales by `factor = clamp(BASE_STOCK / total, 0.5, 2.0)`
    /// where `BASE_STOCK = 100`. Called from `tick_market_coverage`.
    fn tick_market_prices(&mut self) {
        const BASE_STOCK: u32 = 100;
        let goods = [
            crate::types::Good::Wood, crate::types::Good::Iron,
            crate::types::Good::Gold, crate::types::Good::Wool,
            crate::types::Good::Sugar, crate::types::Good::Tobacco,
            crate::types::Good::Cattle, crate::types::Good::Grain,
            crate::types::Good::Flour, crate::types::Good::Tools,
            crate::types::Good::Bricks, crate::types::Good::Swords,
            crate::types::Good::Muskets, crate::types::Good::Cannons,
            crate::types::Good::Food, crate::types::Good::Cloth,
            crate::types::Good::Alcohol, crate::types::Good::TobaccoProducts,
            crate::types::Good::Spices, crate::types::Good::Cocoa,
            crate::types::Good::Grapes, crate::types::Good::Stone,
            crate::types::Good::Ore, crate::types::Good::GoldOre,
            crate::types::Good::Hides, crate::types::Good::Cotton,
            crate::types::Good::Silk, crate::types::Good::Jewelry,
            crate::types::Good::Clothing, crate::types::Good::Fish,
        ];
        for g in goods {
            let total: u32 = self.warehouses.iter()
                .filter(|w| w.active)
                .map(|w| w.stock(g) as u32)
                .sum();
            let denom = total.max(1);
            // factor in tenths: 10 = 1.0×, 5 = 0.5×, 20 = 2.0×.
            let factor_tenths = ((BASE_STOCK * 10) / denom).clamp(5, 20);
            let base = crate::prices::price_of(g);
            let buy = (base.buy * factor_tenths as i32 / 10).max(1);
            let sell = (base.sell * factor_tenths as i32 / 10).max(1);
            let i = g as u8 as usize;
            if let Some(slot) = self.current_prices.get_mut(i) {
                *slot = crate::prices::GoodPrice { buy, sell };
            }
        }
    }

    fn next_rand(&mut self) -> u64 {
        if self.rng_state == 0 { self.rng_state = 0xCBF29CE484222325; }
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }

    fn tick_events(&mut self) {
        // Pirates only spawn when there is an active player trade ship to
        // hunt — otherwise the world is too empty for it to matter.
        let target_ship = self.trade_ships.iter()
            .find(|s| s.active && s.owner == 0);
        let Some(ship) = target_ship else { return; };
        let (sx, sy) = (ship.world_x, ship.world_y);

        // 1-in-3 chance per event tick.
        let r = self.next_rand();
        if r % 3 != 0 { return; }

        // Pirate appears on the closest map edge ~30 tiles from the ship.
        let dx = ((r >> 8) as i32 % 60) - 30;
        let dy = ((r >> 16) as i32 % 60) - 30;
        let px = (sx + dx).max(0);
        let py = (sy + dy).max(0);

        // Slot 6 is the pirate faction. Make them at war with everyone
        // else (idempotent — set on every spawn).
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        const PIRATE: u8 = 6;
        for j in 0..6u8 {
            self.diplomacy.set(PIRATE, j, Diplomacy::War);
        }
        let mut pirate = MilitaryUnit::new(UnitType::SmallWarship, PIRATE, px, py);
        pirate.target_x = sx;
        pirate.target_y = sy;
        self.military_units.push(pirate);
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

        // 7. Random events (pirates, etc.)
        if self.timer_events.advance(dt_ms) {
            self.tick_events();
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
            // Track idle streak for maintenance scaling. Built but
            // non-producing buildings accumulate; producers reset.
            if self.buildings[i].is_built() && def.output_good != crate::types::Good::None {
                if produced == 0 {
                    self.buildings[i].idle_ticks =
                        self.buildings[i].idle_ticks.saturating_add(1);
                } else {
                    self.buildings[i].idle_ticks = 0;
                }
            }

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
                            &self.coverage_maps,
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
        // Refresh building maintenance totals per player so the economy
        // tick has up-to-date running costs. Also compute per-player
        // housing capacity from completed WOHN residences, scaled by
        // each residence's `house_tier` (Pioneer 4 → Aristocrat 20).
        const HOUSING_BY_TIER: [u32; 5] = [4, 8, 12, 16, 20];
        let mut maintenance: Vec<u32> = vec![0; self.players.len()];
        let mut housing: Vec<u32> = vec![0; self.players.len()];
        // Promotion pass: WOHN buildings whose tier is fully satisfied
        // upgrade up. Done before maintenance/cap so the housing cap
        // immediately reflects the new sizes.
        for b in self.buildings.iter_mut() {
            if !b.active || !b.is_built() { continue; }
            let def_id = b.def_id as usize;
            if def_id >= self.building_defs.len() { continue; }
            let def = &self.building_defs[def_id];
            let is_residence = def.kind == "WOHN" || def.prod_kind == "WOHN";
            if !is_residence { continue; }
            let owner = b.owner as usize;
            let Some(p) = self.players.get(owner) else { continue; };
            let t = b.house_tier as usize;
            if t < 4 && p.satisfaction[t] >= 100 {
                b.house_tier += 1;
            }
        }
        for b in &self.buildings {
            if !b.active || !b.is_built() { continue; }
            let owner = b.owner as usize;
            if owner >= maintenance.len() { continue; }
            let def_id = b.def_id as usize;
            if def_id < self.building_defs.len() {
                let mut cost = self.building_defs[def_id].maintenance_cost as u32;
                if b.idle_ticks >= crate::building::IDLE_MAINTENANCE_THRESHOLD {
                    cost /= 2;
                }
                maintenance[owner] = maintenance[owner].saturating_add(cost);
                let kind = self.building_defs[def_id].kind.as_str();
                let pk = self.building_defs[def_id].prod_kind.as_str();
                if kind == "WOHN" || pk == "WOHN" {
                    let t = (b.house_tier as usize).min(4);
                    housing[owner] = housing[owner]
                        .saturating_add(HOUSING_BY_TIER[t]);
                }
            }
        }
        for (i, player) in self.players.iter_mut().enumerate() {
            player.building_maintenance = maintenance[i];
            // Update demands and consume goods from warehouses
            population::update_population_demands(player, &mut self.warehouses, i as u8);
            // Apply economy (gold balance, bankruptcy, satisfaction decay)
            economy::tick_economy(player);
            // Grow / shrink population by tier and promote satisfied tiers up,
            // clamped to current housing capacity.
            population::update_population_growth(player, housing[i]);
        }

        // Sample player 0 (the human) into the rolling history (gold,
        // pop, satisfaction, income/costs, AND per-good warehouse stocks).
        if let Some(p0) = self.players.first() {
            self.history.record_full(p0, &self.warehouses, 0);
            // Re-evaluate scenario objectives against the human player.
            let just_done = self.objectives.evaluate(
                p0, &self.buildings, &self.building_defs, &self.warehouses, 0,
            );
            self.objective_completions.extend(just_done);
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
                        // Collect every def producing the requested good
                        // that the AI can afford, then prefer the one we
                        // don't already have (variety > monoculture). Ties
                        // break on cost so cheaper alternatives still win.
                        let owner = self.ai_controllers[ai_idx].player_idx;
                        let gold = self.players[player_idx].gold;
                        let mut counts: std::collections::HashMap<u16, u32> =
                            std::collections::HashMap::new();
                        for b in &self.buildings {
                            if b.owner == owner {
                                *counts.entry(b.def_id).or_insert(0) += 1;
                            }
                        }
                        let pick = self.building_defs.iter().enumerate()
                            .filter(|(_, d)| d.output_good == good
                                && d.cost_gold as i32 <= gold)
                            .min_by_key(|(idx, d)| {
                                let n = counts.get(&(*idx as u16)).copied().unwrap_or(0);
                                (n, d.cost_gold)
                            });
                        if let Some((def_id, def)) = pick {
                            // Find an AI warehouse to anchor near, then a spot
                            // on that island where the footprint fits.
                            let wh = self.warehouses.iter().find(|w| {
                                w.active && w.owner == owner
                            });
                            if let Some(wh) = wh {
                                let island_id = wh.island_id;
                                let cx = wh.tile_x;
                                let cy = wh.tile_y;
                                let w = def.width as u16;
                                let h = def.height as u16;
                                let cost = def.cost_gold;
                                let footprint = (w as u32) * (h as u32);
                                let build_ms = (2_000u32 * footprint).max(2_000);
                                let map_idx = self.island_maps.iter()
                                    .position(|m| m.island_id == island_id);
                                if let Some(idx) = map_idx {
                                    let spot = self.island_maps[idx]
                                        .find_reachable_spot(cx, cy, w, h, 12, (cx, cy));
                                    if let Some((bx, by)) = spot {
                                        // Mark footprint blocked
                                        for dy in 0..h {
                                            for dx in 0..w {
                                                self.island_maps[idx]
                                                    .set_walkable(bx + dx, by + dy, false);
                                            }
                                        }
                                        let mut inst = BuildingInstance::new(
                                            def_id as u16, island_id, bx, by, owner,
                                        );
                                        inst.construction_ms_total = build_ms;
                                        inst.construction_ms_remaining = build_ms;
                                        // Defer materials: the entity tick
                                        // will trickle them in from
                                        // warehouses as construction proceeds.
                                        inst.wood_needed = def.cost_wood;
                                        inst.tools_needed = def.cost_tools;
                                        inst.bricks_needed = def.cost_bricks;
                                        self.buildings.push(inst);
                                        self.players[player_idx].gold -= cost as i32;
                                    }
                                }
                            }
                        }
                    }
                    AiAction::RequestMilitary { unit_count } => {
                        // Spawn `unit_count` Swordsmen near the AI's first
                        // active warehouse, capped by gold (100 / unit).
                        let owner = self.ai_controllers[ai_idx].player_idx;
                        const SWORDSMAN_COST: i32 = 100;
                        let spawn = self.warehouses.iter().find(|w| {
                            w.active && w.owner == owner
                        });
                        if let Some(spawn) = spawn {
                            let (sx, sy) = (spawn.tile_x as i32, spawn.tile_y as i32);
                            let mut spent = 0;
                            for k in 0..(unit_count as i32) {
                                if self.players[player_idx].gold < SWORDSMAN_COST {
                                    break;
                                }
                                self.players[player_idx].gold -= SWORDSMAN_COST;
                                spent += 1;
                                // Stagger spawn positions in a small ring.
                                let dx = (k % 3) - 1;
                                let dy = (k / 3) % 3 - 1;
                                self.military_units.push(MilitaryUnit::new(
                                    crate::combat::UnitType::Swordsman,
                                    owner,
                                    sx + dx,
                                    sy + dy,
                                ));
                            }
                            self.players[player_idx].military_maintenance =
                                self.players[player_idx]
                                    .military_maintenance
                                    .saturating_add((spent * 2) as u32);
                        }
                    }
                    AiAction::EstablishTradeRoute => {
                        let owner = self.ai_controllers[ai_idx].player_idx;
                        // Pick at most 4 of the AI's warehouses across
                        // distinct islands and turn them into a route.
                        let mut by_island: std::collections::HashMap<u8, (u16, u16)> =
                            std::collections::HashMap::new();
                        for wh in &self.warehouses {
                            if wh.active && wh.owner == owner {
                                by_island.entry(wh.island_id)
                                    .or_insert((wh.tile_x, wh.tile_y));
                            }
                        }
                        if by_island.len() < 2 { continue; }
                        let mut stops: Vec<(u8, u16, u16)> = by_island
                            .into_iter()
                            .map(|(iid, (x, y))| (iid, x, y))
                            .collect();
                        stops.sort_by_key(|s| s.0);
                        stops.truncate(4);

                        const SHIP_COST: i32 = 1000;
                        if self.players[player_idx].gold < SHIP_COST {
                            continue;
                        }
                        self.players[player_idx].gold -= SHIP_COST;

                        // Match the human-side trade-route editor: every stop
                        // loads/unloads every known good — the trade tick only
                        // moves what's actually there.
                        use crate::trade::{RouteStop, TradeRoute, TradeShip};
                        let all_goods = [
                            crate::types::Good::Wood, crate::types::Good::Iron,
                            crate::types::Good::Ore, crate::types::Good::Gold,
                            crate::types::Good::Wool, crate::types::Good::Sugar,
                            crate::types::Good::Tobacco, crate::types::Good::Cattle,
                            crate::types::Good::Grain, crate::types::Good::Flour,
                            crate::types::Good::Food, crate::types::Good::Alcohol,
                            crate::types::Good::Cloth, crate::types::Good::Clothing,
                            crate::types::Good::Jewelry, crate::types::Good::Tools,
                            crate::types::Good::Bricks, crate::types::Good::Swords,
                            crate::types::Good::Cannons, crate::types::Good::Muskets,
                            crate::types::Good::Stone, crate::types::Good::Cocoa,
                            crate::types::Good::Spices, crate::types::Good::Hides,
                            crate::types::Good::Cotton, crate::types::Good::Silk,
                            crate::types::Good::Fish, crate::types::Good::Grapes,
                            crate::types::Good::GoldOre,
                            crate::types::Good::TobaccoProducts,
                        ];
                        let next_id = self.trade_routes.iter()
                            .map(|r| r.id).max().map(|m| m + 1).unwrap_or(1);
                        let mut route = TradeRoute::new(next_id, owner);
                        for &(iid, wx, wy) in &stops {
                            route.add_stop(RouteStop {
                                island_id: iid,
                                warehouse_x: wx,
                                warehouse_y: wy,
                                load_goods: all_goods.iter()
                                    .map(|&g| (g, 50)).collect(),
                                unload_goods: all_goods.to_vec(),
                            });
                        }
                        route.activate();
                        let route_id = route.id;
                        let (sx, sy) = (stops[0].1 as i32, stops[0].2 as i32);
                        self.trade_routes.push(route);
                        self.trade_ships.push(TradeShip::new(
                            owner, route_id, sx, sy,
                        ));
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
                                        let price = crate::prices::price_of(*good).sell;
                                        self.players[player_idx].gold += sell as i32 * price;
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

        // Refresh dynamic market prices on the same cadence.
        self.tick_market_prices();
        // Reveal tiles around player-owned entities.
        self.tick_exploration();
    }

    fn tick_exploration(&mut self) {
        const PLAYER: u8 = 0;
        const SIGHT_RADIUS: i32 = 5;
        // Helper: get-or-create the per-island exploration map.
        let ensure = |sim: &mut Simulation, island_id: u8| -> usize {
            if let Some(idx) = sim.exploration.iter()
                .position(|e| e.island_id == island_id)
            {
                return idx;
            }
            // Pull dimensions from the island map if it exists, else default.
            let (w, h) = sim.island_maps.iter()
                .find(|m| m.island_id == island_id)
                .map(|m| (m.width, m.height))
                .unwrap_or((128, 128));
            sim.exploration.push(
                crate::exploration::ExplorationMap::new(island_id, w, h),
            );
            sim.exploration.len() - 1
        };
        // Buildings.
        let bldg_seeds: Vec<(u8, i32, i32)> = self.buildings.iter()
            .filter(|b| b.owner == PLAYER && b.active)
            .map(|b| (b.island_id, b.tile_x as i32, b.tile_y as i32))
            .collect();
        for (iid, x, y) in bldg_seeds {
            let idx = ensure(self, iid);
            self.exploration[idx].mark_radius(x, y, SIGHT_RADIUS);
        }
        // Warehouses.
        let wh_seeds: Vec<(u8, i32, i32)> = self.warehouses.iter()
            .filter(|w| w.owner == PLAYER && w.active)
            .map(|w| (w.island_id, w.tile_x as i32, w.tile_y as i32))
            .collect();
        for (iid, x, y) in wh_seeds {
            let idx = ensure(self, iid);
            self.exploration[idx].mark_radius(x, y, SIGHT_RADIUS);
        }
        // Military units (land only — naval roams the world map, no
        // island-tile coords meaningful to per-island bitmap).
        let unit_seeds: Vec<(u8, i32, i32)> = self.military_units.iter()
            .filter(|u| u.is_alive() && u.owner == PLAYER
                && !u.unit_type.stats().is_naval)
            .map(|u| {
                // Try to find which island this unit is on by walkability map.
                let island_id = self.island_maps.iter()
                    .find(|m| m.is_walkable(u.tile_x, u.tile_y))
                    .map(|m| m.island_id).unwrap_or(0);
                (island_id, u.tile_x, u.tile_y)
            })
            .collect();
        for (iid, x, y) in unit_seeds {
            let idx = ensure(self, iid);
            self.exploration[idx].mark_radius(x, y, SIGHT_RADIUS);
        }
    }

    fn tick_diplomacy(&mut self) {
        // Score each player slot: military weight + a fraction of gold.
        // Used by AI controllers to decide war / peace based on power.
        let mut scores = vec![0i64; self.players.len()];
        for u in &self.military_units {
            if u.is_alive() {
                let i = u.owner as usize;
                if i < scores.len() { scores[i] += 10; }
            }
        }
        for (i, p) in self.players.iter().enumerate() {
            scores[i] += (p.gold.max(0) as i64) / 200;
        }

        for ctrl_idx in 0..self.ai_controllers.len() {
            let me = self.ai_controllers[ctrl_idx].player_idx as usize;
            if me >= scores.len() { continue; }
            // Only Military and Balanced personalities flip relations.
            let personality = self.ai_controllers[ctrl_idx].personality;
            use crate::ai::AiPersonality;
            use crate::combat::Diplomacy;
            use crate::player::PlayerState;

            let aggressor = matches!(
                personality,
                AiPersonality::Military | AiPersonality::Balanced,
            );
            let pacifist = matches!(personality, AiPersonality::Economic);

            let my_score = scores[me];
            for other in 0..scores.len() {
                if other == me { continue; }
                // Ignore empty / defeated slots.
                if let Some(p) = self.players.get(other) {
                    if matches!(p.state, PlayerState::Empty | PlayerState::Defeated) {
                        continue;
                    }
                } else { continue; }
                let other_score = scores[other];
                let cur = self.diplomacy.get(me as u8, other as u8);

                // Declare war on a clearly weaker neutral neighbor.
                if aggressor
                    && cur == Diplomacy::Neutral
                    && my_score >= 20
                    && other_score * 2 <= my_score
                {
                    self.diplomacy.set(me as u8, other as u8, Diplomacy::War);
                    continue;
                }
                // Sue for peace if outmatched (any personality).
                if cur == Diplomacy::War && other_score >= my_score * 2 {
                    self.diplomacy.set(me as u8, other as u8, Diplomacy::Neutral);
                    continue;
                }
                // Pacifist AIs back out of any war they aren't winning.
                if pacifist && cur == Diplomacy::War && other_score >= my_score {
                    self.diplomacy.set(me as u8, other as u8, Diplomacy::Neutral);
                }
            }

            // Reactive defense: scan AI warehouses for nearby hostile
            // military and spawn defenders if any are within 8 tiles.
            const DEFENDER_COST: i32 = 100;
            const DEFENDER_RADIUS: i32 = 8;
            const DEFENDERS_PER_THREAT: u32 = 2;
            let owner = me as u8;
            let warehouse_targets: Vec<(u16, u16, u8)> = self.warehouses.iter()
                .filter(|w| w.active && w.owner == owner)
                .map(|w| (w.tile_x, w.tile_y, w.island_id))
                .collect();
            for (wx, wy, _wh_island) in warehouse_targets {
                // Is there a hostile military unit within radius?
                let threat = self.military_units.iter().any(|u| {
                    if !u.is_alive() || u.owner == owner { return false; }
                    if self.diplomacy.get(owner, u.owner) != Diplomacy::War {
                        return false;
                    }
                    let dx = (u.tile_x - wx as i32).abs();
                    let dy = (u.tile_y - wy as i32).abs();
                    dx.max(dy) <= DEFENDER_RADIUS
                });
                if !threat { continue; }
                // Don't keep spawning if we already have defenders nearby.
                let existing = self.military_units.iter().filter(|u| {
                    u.is_alive() && u.owner == owner
                        && (u.tile_x - wx as i32).abs() <= DEFENDER_RADIUS
                        && (u.tile_y - wy as i32).abs() <= DEFENDER_RADIUS
                }).count();
                if existing >= DEFENDERS_PER_THREAT as usize { continue; }
                if self.players[me].gold < DEFENDER_COST { continue; }
                let needed = DEFENDERS_PER_THREAT as i32 - existing as i32;
                for k in 0..needed.max(1) {
                    if self.players[me].gold < DEFENDER_COST { break; }
                    self.players[me].gold -= DEFENDER_COST;
                    let dx = (k % 3) - 1;
                    let dy = (k / 3) - 1;
                    self.military_units.push(MilitaryUnit::new(
                        crate::combat::UnitType::Swordsman,
                        owner,
                        wx as i32 + dx,
                        wy as i32 + dy,
                    ));
                }
            }

            // Naval escort: each AI trade ship gets a shadowing SmallWarship
            // when the AI is at war with anyone. If no idle warship is
            // available and the AI has gold, spawn one near the trade ship.
            const ESCORT_COST: i32 = 500;
            let at_war = (0..scores.len()).any(|j| {
                j as u8 != owner
                    && self.diplomacy.get(owner, j as u8) == Diplomacy::War
            });
            if at_war {
                let ship_indices: Vec<(usize, i32, i32)> = self.trade_ships
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.active && s.owner == owner)
                    .map(|(i, s)| (i, s.world_x, s.world_y))
                    .collect();
                for (ship_idx, sx, sy) in ship_indices {
                    let already_escorted = self.military_units.iter().any(|u| {
                        u.is_alive() && u.owner == owner
                            && u.escort_ship == ship_idx as i32
                    });
                    if already_escorted { continue; }
                    // Look for an idle warship to assign.
                    let idle_warship = self.military_units.iter_mut().find(|u| {
                        u.is_alive() && u.owner == owner
                            && u.unit_type.stats().is_naval
                            && u.escort_ship < 0
                    });
                    if let Some(u) = idle_warship {
                        u.escort_ship = ship_idx as i32;
                        continue;
                    }
                    // Otherwise spawn a fresh SmallWarship beside the ship.
                    if self.players[me].gold < ESCORT_COST { continue; }
                    self.players[me].gold -= ESCORT_COST;
                    let mut unit = MilitaryUnit::new(
                        crate::combat::UnitType::SmallWarship,
                        owner,
                        sx,
                        sy,
                    );
                    unit.escort_ship = ship_idx as i32;
                    self.military_units.push(unit);
                }
            }

            // Offensive raids: aggressors with a clear advantage send half
            // their idle units toward an enemy warehouse on the same island.
            if aggressor {
                for enemy in 0..scores.len() {
                    if enemy as u8 == owner { continue; }
                    if self.diplomacy.get(owner, enemy as u8) != Diplomacy::War {
                        continue;
                    }
                    if scores[me] <= scores[enemy] { continue; }
                    // Pick a target warehouse: the enemy's first active one.
                    let target = self.warehouses.iter().find(|w| {
                        w.active && w.owner == enemy as u8
                    });
                    let Some(target) = target else { continue; };
                    let tx = target.tile_x as i32;
                    let ty = target.tile_y as i32;
                    // Find this AI's idle units — those whose current
                    // target is roughly their own tile.
                    let mut idle_indices: Vec<usize> = Vec::new();
                    for (i, u) in self.military_units.iter().enumerate() {
                        if !u.is_alive() || u.owner != owner { continue; }
                        let stuck = u.target_x == u.tile_x && u.target_y == u.tile_y;
                        if stuck && u.combat_target < 0 {
                            idle_indices.push(i);
                        }
                    }
                    let send = (idle_indices.len() / 2).max(1).min(idle_indices.len());
                    for i in idle_indices.into_iter().take(send) {
                        self.military_units[i].target_x = tx;
                        self.military_units[i].target_y = ty;
                        self.military_units[i].combat_target = -1;
                        self.military_units[i].move_timer_ms = 0;
                    }
                    break; // One front per tick
                }
            }
        }
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

        // Building damage from adjacent hostile land units.
        let destroyed = combat::tick_building_damage(
            &self.military_units,
            &mut self.buildings,
            &self.diplomacy,
            &self.building_defs,
        );
        // Remove destroyed buildings (in reverse) and emit tile-clear events.
        for &bi in destroyed.iter().rev() {
            let b = &self.buildings[bi];
            let def = &self.building_defs[b.def_id as usize];
            let island_id = b.island_id;
            let bx = b.tile_x;
            let by = b.tile_y;
            let bw = def.width;
            let bh = def.height;
            // Free tiles in the island walkability map.
            if let Some(map) = self.island_maps.iter_mut()
                .find(|m| m.island_id == island_id)
            {
                for dy in 0..bh as u16 {
                    for dx in 0..bw as u16 {
                        map.set_walkable(bx + dx, by + dy, true);
                    }
                }
            }
            self.tile_clears.push((island_id, bx, by, bw, bh));
            self.buildings.swap_remove(bi);
        }

        // Remove dead units (reverse order to preserve indices)
        let mut dead_sorted = dead;
        dead_sorted.sort_unstable();
        dead_sorted.dedup();
        for &idx in dead_sorted.iter().rev() {
            self.military_units.swap_remove(idx);
        }
    }

    fn tick_entities(&mut self, dt_ms: u32) {
        // Refresh escort targets so warships stay glued to their assigned
        // trade ship before move orders are stepped.
        let positions: Vec<(bool, i32, i32)> = self.trade_ships.iter()
            .map(|s| (s.active, s.world_x, s.world_y))
            .collect();
        combat::tick_escort_targets(&mut self.military_units, &positions);
        // Move military units toward player-issued targets (every step).
        combat::tick_unit_orders(&mut self.military_units, dt_ms);

        // Trickle construction materials from the player's warehouses,
        // then decrement the construction timer only once the materials
        // for this building are done.
        use crate::types::Good;
        let mut take_one = |island_id: u8, owner: u8, good: Good| -> bool {
            for w in self.warehouses.iter_mut().filter(|w| {
                w.active && w.owner == owner && w.island_id == island_id
            }) {
                if w.withdraw(good, 1) > 0 { return true; }
            }
            false
        };
        for b in self.buildings.iter_mut() {
            if b.is_built() { continue; }
            // Try to consume one unit of each pending material from a
            // warehouse on the same island. If a line is short, that
            // material simply isn't drawn this tick — construction stalls.
            if b.wood_needed > 0 && take_one(b.island_id, b.owner, Good::Wood) {
                b.wood_needed -= 1;
            }
            if b.tools_needed > 0 && take_one(b.island_id, b.owner, Good::Tools) {
                b.tools_needed -= 1;
            }
            if b.bricks_needed > 0 && take_one(b.island_id, b.owner, Good::Bricks) {
                b.bricks_needed -= 1;
            }
            // Construction time only flows once all materials are met.
            if b.wood_needed == 0 && b.tools_needed == 0 && b.bricks_needed == 0
                && b.construction_ms_remaining > 0
            {
                b.construction_ms_remaining =
                    b.construction_ms_remaining.saturating_sub(dt_ms);
            }
        }

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

    /// Try to deduct construction materials from the player's warehouses
    /// on `island_id`. Returns true if every required cost was withdrawn,
    /// false (with no withdrawals applied) when any line is short.
    pub fn warehouse_pay_materials(
        &mut self,
        island_id: u8,
        owner: u8,
        cost_wood: u16,
        cost_tools: u16,
        cost_bricks: u16,
    ) -> bool {
        use crate::types::Good;
        let total = |good: Good| -> u16 {
            self.warehouses.iter()
                .filter(|w| w.active && w.owner == owner && w.island_id == island_id)
                .map(|w| w.stock(good))
                .sum::<u16>()
        };
        if total(Good::Wood) < cost_wood
            || total(Good::Tools) < cost_tools
            || total(Good::Bricks) < cost_bricks
        {
            return false;
        }
        let mut take = |good: Good, mut amount: u16| {
            for w in self.warehouses.iter_mut().filter(|w| {
                w.active && w.owner == owner && w.island_id == island_id
            }) {
                if amount == 0 { break; }
                amount -= w.withdraw(good, amount);
            }
        };
        take(Good::Wood, cost_wood);
        take(Good::Tools, cost_tools);
        take(Good::Bricks, cost_bricks);
        true
    }

    /// Apply a player-issued command to the authoritative state.
    /// Returns true if it was applied successfully.
    pub fn apply_command(&mut self, cmd: &crate::commands::Command) -> bool {
        use crate::commands::Command;
        match *cmd {
            Command::SetTaxRate { player, tier, rate } => {
                let pi = player as usize;
                let ti = tier as usize;
                if pi >= self.players.len() || ti >= 5 { return false; }
                self.players[pi].tax_rates[ti] = rate;
                true
            }
            Command::SetDiplomacy { a, b, state } => {
                self.diplomacy.set(a, b, state);
                true
            }
            Command::Buy { player, good, qty } => {
                let pi = player as usize;
                if pi >= self.players.len() { return false; }
                let price = self.current_price(good).buy;
                let max_aff = (self.players[pi].gold / price).max(0) as u16;
                let want = qty.min(max_aff);
                if want == 0 { return false; }
                if let Some(wh) = self.warehouses.iter_mut()
                    .find(|w| w.active && w.owner == player)
                {
                    let dep = wh.deposit(good, want);
                    self.players[pi].gold -= dep as i32 * price;
                    return dep > 0;
                }
                false
            }
            Command::Sell { player, good, qty } => {
                let pi = player as usize;
                if pi >= self.players.len() { return false; }
                let price = self.current_price(good).sell;
                if let Some(wh) = self.warehouses.iter_mut()
                    .find(|w| w.active && w.owner == player)
                {
                    let took = wh.withdraw(good, qty);
                    self.players[pi].gold += took as i32 * price;
                    return took > 0;
                }
                false
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{AiController, AiPersonality, Difficulty};

    #[test]
    fn ai_request_build_places_building() {
        use crate::ai::AiAction;
        use crate::types::{Good, ProductionType};
        use crate::building::BuildingDef;

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 5_000;
        // One open island map for AI.
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.warehouses.push(Warehouse::new(0, 1, 15, 15));
        // A single buildable def: a Tools workshop.
        sim.building_defs.push(BuildingDef {
            id: 0, category: 0, width: 2, height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools, input_good_1: Good::Iron,
            input_good_2: Good::None,
            output_rate: 1, input_1_rate: 1, input_2_rate: 0,
            storage_capacity: 50, cycle_time_ms: 1000, carrier_interval_ms: 0,
            cost_gold: 500, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 0,
        });
        // Drive the build path manually.
        let action = AiAction::RequestBuild { good: Good::Tools, priority: 0 };
        // Inline the dispatch loop (tick_ai runs the controller too, which
        // would emit its own actions; we want a focused single-action test).
        let owner = 1u8;
        let player_idx = owner as usize;
        let gold_before = sim.players[player_idx].gold;
        match action {
            AiAction::RequestBuild { good, .. } => {
                let pick = sim.building_defs.iter().enumerate()
                    .filter(|(_, d)| d.output_good == good
                        && d.cost_gold as i32 <= sim.players[player_idx].gold)
                    .min_by_key(|(_, d)| d.cost_gold);
                let (def_id, def) = pick.unwrap();
                let wh = sim.warehouses.iter().find(|w| w.owner == owner).unwrap();
                let cx = wh.tile_x; let cy = wh.tile_y;
                let w = def.width as u16; let h = def.height as u16;
                let cost = def.cost_gold;
                let map_idx = sim.island_maps.iter()
                    .position(|m| m.island_id == wh.island_id).unwrap();
                let spot = sim.island_maps[map_idx]
                    .find_open_spot(cx, cy, w, h, 12).unwrap();
                for dy in 0..h {
                    for dx in 0..w {
                        sim.island_maps[map_idx].set_walkable(spot.0 + dx, spot.1 + dy, false);
                    }
                }
                let mut inst = BuildingInstance::new(
                    def_id as u16, wh.island_id, spot.0, spot.1, owner,
                );
                inst.construction_ms_total = 8_000;
                inst.construction_ms_remaining = 8_000;
                sim.buildings.push(inst);
                sim.players[player_idx].gold -= cost as i32;
            }
            _ => unreachable!(),
        }
        assert_eq!(sim.buildings.len(), 1);
        assert_eq!(sim.buildings[0].owner, owner);
        assert!(sim.players[player_idx].gold < gold_before);
        assert!(!sim.buildings[0].is_built()); // construction in progress
    }

    #[test]
    fn ai_defends_warehouse_when_threatened() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        // Drain p0's starting gold and pump the AI's so the diplomacy
        // tick won't sue for peace before reaching the defense block.
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        sim.warehouses.push(Warehouse::new(0, 1, 30, 30));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        // Hostile unit within 8 tiles of the AI's warehouse.
        sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 0, 32, 32));
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Military, Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        let ai_units: usize = sim.military_units.iter()
            .filter(|u| u.owner == 1 && u.is_alive())
            .count();
        assert!(ai_units >= 1, "AI should have spawned at least one defender");
        assert!(sim.players[1].gold < 5_000);
    }

    #[test]
    fn exploration_reveals_around_player_warehouse() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.warehouses.push(Warehouse::new(0, 0, 15, 15));
        sim.tick_exploration();
        let m = sim.exploration.iter()
            .find(|e| e.island_id == 0).unwrap();
        // Tile right at the warehouse must be revealed.
        assert!(m.is_explored(15, 15));
        // Tile far away must not be (default radius 5).
        assert!(!m.is_explored(0, 0));
    }

    #[test]
    fn idle_building_maintenance_halves() {
        use crate::types::{Good, ProductionType};
        use crate::building::{BuildingDef, BuildingInstance, IDLE_MAINTENANCE_THRESHOLD};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.building_defs.push(BuildingDef {
            id: 0, category: 0, width: 1, height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools, input_good_1: Good::Iron,
            input_good_2: Good::None,
            output_rate: 1, input_1_rate: 1, input_2_rate: 0,
            storage_capacity: 50, cycle_time_ms: 1000, carrier_interval_ms: 0,
            cost_gold: 0, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 10,
        });
        let mut b = BuildingInstance::new(0, 0, 0, 0, 0);
        b.idle_ticks = IDLE_MAINTENANCE_THRESHOLD; // already qualifies
        sim.buildings.push(b);
        sim.tick_population();
        // Idle threshold met → maintenance halved (10 → 5).
        assert_eq!(sim.players[0].building_maintenance, 5);

        // Reset idle ticks → full maintenance.
        sim.buildings[0].idle_ticks = 0;
        sim.tick_population();
        assert_eq!(sim.players[0].building_maintenance, 10);
    }

    #[test]
    fn construction_stalls_without_materials() {
        use crate::types::{Good, ProductionType};
        use crate::building::{BuildingDef, BuildingInstance};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.building_defs.push(BuildingDef {
            id: 0, category: 0, width: 1, height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools, input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0, input_1_rate: 0, input_2_rate: 0,
            storage_capacity: 0, cycle_time_ms: 0, carrier_interval_ms: 0,
            cost_gold: 0, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 0,
        });
        let mut b = BuildingInstance::new(0, 0, 0, 0, 0);
        b.construction_ms_total = 1_000;
        b.construction_ms_remaining = 1_000;
        b.wood_needed = 5; // demands wood we don't have
        sim.buildings.push(b);

        // No warehouse has wood. Tick entities; construction must NOT advance.
        sim.tick_entities(500);
        assert_eq!(sim.buildings[0].construction_ms_remaining, 1_000);
        assert_eq!(sim.buildings[0].wood_needed, 5);
    }

    #[test]
    fn construction_trickles_materials_from_warehouse() {
        use crate::types::Good;
        use crate::building::BuildingInstance;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut wh = Warehouse::new(0, 0, 0, 0);
        wh.set_capacity(Good::Wood, 100);
        wh.deposit(Good::Wood, 10);
        sim.warehouses.push(wh);
        let mut b = BuildingInstance::new(0, 0, 5, 5, 0);
        b.construction_ms_total = 1_000;
        b.construction_ms_remaining = 1_000;
        b.wood_needed = 3;
        sim.buildings.push(b);

        // 1st tick: drain 1 wood, construction stays paused (need still 2).
        sim.tick_entities(50);
        assert_eq!(sim.buildings[0].wood_needed, 2);
        assert_eq!(sim.buildings[0].construction_ms_remaining, 1_000);
        // 2nd tick: 1 more drained.
        sim.tick_entities(50);
        assert_eq!(sim.buildings[0].wood_needed, 1);
        // 3rd: drained, materials done; this tick the timer also
        // decrements by dt_ms.
        sim.tick_entities(50);
        assert_eq!(sim.buildings[0].wood_needed, 0);
        assert!(sim.buildings[0].construction_ms_remaining < 1_000);
        assert_eq!(sim.warehouses[0].stock(Good::Wood), 7);
    }

    #[test]
    fn warehouse_pay_materials_consumes_when_sufficient() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut w = Warehouse::new(0, 0, 0, 0);
        w.set_capacity(Good::Wood, 100);
        w.set_capacity(Good::Tools, 100);
        w.set_capacity(Good::Bricks, 100);
        w.deposit(Good::Wood, 50);
        w.deposit(Good::Tools, 30);
        w.deposit(Good::Bricks, 20);
        sim.warehouses.push(w);
        assert!(sim.warehouse_pay_materials(0, 0, 10, 5, 8));
        let wh = &sim.warehouses[0];
        assert_eq!(wh.stock(Good::Wood), 40);
        assert_eq!(wh.stock(Good::Tools), 25);
        assert_eq!(wh.stock(Good::Bricks), 12);
    }

    #[test]
    fn warehouse_pay_materials_refuses_when_short() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut w = Warehouse::new(0, 0, 0, 0);
        w.set_capacity(Good::Wood, 100);
        w.deposit(Good::Wood, 5);
        sim.warehouses.push(w);
        // Need 10 wood, only have 5 → no withdrawal.
        assert!(!sim.warehouse_pay_materials(0, 0, 10, 0, 0));
        assert_eq!(sim.warehouses[0].stock(Good::Wood), 5);
    }

    #[test]
    fn market_prices_drop_when_glutted() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut w = Warehouse::new(0, 0, 0, 0);
        w.set_capacity(Good::Wood, 10_000);
        w.deposit(Good::Wood, 5_000); // huge surplus
        sim.warehouses.push(w);
        let base = crate::prices::price_of(Good::Wood);
        sim.tick_market_coverage();
        let now = sim.current_price(Good::Wood);
        // Glut should at least halve the price (factor floor is 0.5).
        assert!(now.buy <= base.buy / 2 + 1, "got {now:?}");
    }

    #[test]
    fn market_prices_rise_when_scarce() {
        use crate::types::Good;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        // No warehouse, so total stock = 0 → factor capped at 2.0
        let base = crate::prices::price_of(Good::Tools);
        sim.tick_market_coverage();
        let now = sim.current_price(Good::Tools);
        assert!(now.buy >= base.buy * 2 - 1, "got {now:?} vs base {base:?}");
    }

    #[test]
    fn pirates_eventually_spawn_to_attack_player_trade_ships() {
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.trade_ships.push(TradeShip::new(0, 0, 100, 100));
        // Drive event ticks until a pirate appears (1-in-3 odds; cap to
        // keep the test bounded).
        let mut spawned = false;
        for _ in 0..30 {
            sim.tick_events();
            if sim.military_units.iter().any(|u| u.owner == 6) {
                spawned = true;
                break;
            }
        }
        assert!(spawned, "expected at least one pirate spawn within 30 ticks");
        // Pirate is at war with player 0.
        use crate::combat::Diplomacy;
        assert_eq!(sim.diplomacy.get(6, 0), Diplomacy::War);
    }

    #[test]
    fn pirates_skip_spawn_when_no_player_trade_ships() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        for _ in 0..30 {
            sim.tick_events();
        }
        assert!(sim.military_units.iter().all(|u| u.owner != 6));
    }

    #[test]
    fn buildings_destroyed_by_adjacent_enemy_units() {
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        use crate::types::{Good, ProductionType};
        use crate::building::{BuildingDef, BuildingInstance};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.building_defs.push(BuildingDef {
            id: 0, category: 0, width: 2, height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools, input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0, input_1_rate: 0, input_2_rate: 0,
            storage_capacity: 0, cycle_time_ms: 1000, carrier_interval_ms: 0,
            cost_gold: 0, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 0,
        });
        let b = BuildingInstance::new(0, 0, 10, 10, 0); // player 0 owns
        sim.buildings.push(b);
        // Enemy unit standing right next to the footprint.
        sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 1, 11, 12));
        // Run several military ticks until building dies.
        for _ in 0..30 {
            sim.tick_military();
            if sim.buildings.is_empty() { break; }
        }
        assert!(sim.buildings.is_empty(), "building should have been destroyed");
        // Tile-clear event was queued.
        assert!(!sim.tile_clears.is_empty());
        // IslandMap walkability restored.
        assert!(sim.island_maps[0].is_walkable(10, 10));
    }

    #[test]
    fn buildings_safe_from_neutral_units() {
        use crate::combat::{MilitaryUnit, UnitType};
        use crate::types::{Good, ProductionType};
        use crate::building::{BuildingDef, BuildingInstance};

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.island_maps.push(IslandMap::new_open(0, 30, 30));
        sim.building_defs.push(BuildingDef {
            id: 0, category: 0, width: 2, height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools, input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0, input_1_rate: 0, input_2_rate: 0,
            storage_capacity: 0, cycle_time_ms: 1000, carrier_interval_ms: 0,
            cost_gold: 0, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 0,
        });
        sim.buildings.push(BuildingInstance::new(0, 0, 10, 10, 0));
        sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 1, 11, 12));
        // No diplomacy edit → relation stays Neutral, no damage.
        for _ in 0..30 {
            sim.tick_military();
        }
        assert_eq!(sim.buildings.len(), 1);
        assert_eq!(sim.buildings[0].health, crate::building::BUILDING_MAX_HEALTH);
    }

    #[test]
    fn ai_spawns_escort_warship_for_trade_ship_when_at_war() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::Diplomacy;
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        sim.warehouses.push(Warehouse::new(0, 1, 10, 10));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        // AI has a trade ship.
        sim.trade_ships.push(TradeShip::new(1, 0, 50, 60));
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Military, Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        let escorts: Vec<&_> = sim.military_units.iter()
            .filter(|u| u.owner == 1 && u.escort_ship == 0).collect();
        assert!(!escorts.is_empty(), "expected an escort warship");
        assert!(escorts[0].unit_type.stats().is_naval);
        assert!(sim.players[1].gold < 5_000);
    }

    #[test]
    fn escort_targets_track_their_ship() {
        use crate::combat::{MilitaryUnit, UnitType};
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.trade_ships.push(TradeShip::new(0, 0, 100, 200));
        let mut warship = MilitaryUnit::new(UnitType::SmallWarship, 0, 50, 50);
        warship.escort_ship = 0;
        sim.military_units.push(warship);
        sim.tick_entities(50);
        let u = &sim.military_units[0];
        assert_eq!(u.target_x, 100);
        assert_eq!(u.target_y, 200);
    }

    #[test]
    fn escort_clears_when_ship_inactive() {
        use crate::combat::{MilitaryUnit, UnitType};
        use crate::trade::TradeShip;
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        let mut s = TradeShip::new(0, 0, 5, 5);
        s.active = false;
        sim.trade_ships.push(s);
        let mut warship = MilitaryUnit::new(UnitType::SmallWarship, 0, 50, 50);
        warship.escort_ship = 0;
        sim.military_units.push(warship);
        sim.tick_entities(50);
        assert_eq!(sim.military_units[0].escort_ship, -1);
    }

    #[test]
    fn ai_marches_units_at_enemy_warehouse_when_winning() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        // Enemy warehouse at (50,50), AI's at (10,10).
        sim.warehouses.push(Warehouse::new(0, 0, 50, 50));
        sim.warehouses.push(Warehouse::new(0, 1, 10, 10));
        sim.diplomacy.set(0, 1, Diplomacy::War);
        // 4 idle AI units sitting near their warehouse.
        for k in 0..4 {
            let mut u = MilitaryUnit::new(UnitType::Swordsman, 1, 10 + k, 10);
            u.target_x = u.tile_x;
            u.target_y = u.tile_y;
            sim.military_units.push(u);
        }
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Military, Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        // At least half of the AI's units should now be marching toward
        // the enemy warehouse (50,50).
        let marching = sim.military_units.iter().filter(|u| {
            u.owner == 1 && u.target_x == 50 && u.target_y == 50
        }).count();
        assert!(marching >= 2, "expected ≥2 units marching, got {marching}");
    }

    #[test]
    fn ai_does_not_defend_against_neutral() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 1_000;
        sim.warehouses.push(Warehouse::new(0, 1, 30, 30));
        // Neutral player walking near the warehouse — not a threat.
        sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 0, 32, 32));
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Military, Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        let ai_units: usize = sim.military_units.iter()
            .filter(|u| u.owner == 1 && u.is_alive())
            .count();
        assert_eq!(ai_units, 0);
    }

    #[test]
    fn ai_request_build_prefers_variety() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::types::{Good, ProductionType};
        use crate::building::BuildingDef;

        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 10_000;
        sim.players[1].population[1] = 150; // make AI tick fire
        sim.players[1].total_population = 150;
        sim.island_maps.push(IslandMap::new_open(0, 60, 60));
        sim.warehouses.push(Warehouse::new(0, 1, 30, 30));

        let mk_def = |cost: u32| BuildingDef {
            id: 0, category: 0, width: 2, height: 2,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Food, input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0, input_1_rate: 0, input_2_rate: 0,
            storage_capacity: 50, cycle_time_ms: 1000, carrier_interval_ms: 0,
            cost_gold: cost, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 0,
        };
        // Two defs producing the same Good. Cheaper one would always win
        // under the old logic.
        sim.building_defs.push(mk_def(500));   // def 0 (cheap)
        sim.building_defs.push(mk_def(800));   // def 1 (expensive)

        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Economic, Difficulty::Hard,
        ));
        // Drive tick_ai twice, resetting cooldowns between so we get
        // back-to-back builds.
        sim.tick_ai();
        sim.ai_controllers[0].build_cooldown = 0;
        sim.tick_ai();

        let mut def_ids: Vec<u16> = sim.buildings
            .iter()
            .filter(|b| b.owner == 1)
            .map(|b| b.def_id)
            .collect();
        def_ids.sort();
        assert_eq!(def_ids.len(), 2, "expected two AI builds, got {def_ids:?}");
        assert_eq!(def_ids, vec![0, 1],
            "AI should diversify across both defs, got {def_ids:?}");
    }

    #[test]
    fn ai_establishes_trade_route_when_eligible() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 5_000;
        // Two warehouses on different islands so the AI is eligible.
        sim.warehouses.push(Warehouse::new(0, 1, 10, 10));
        sim.warehouses.push(Warehouse::new(1, 1, 50, 50));
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Economic, Difficulty::Hard,
        ));
        // Drive AI tick.
        sim.tick_ai();
        assert!(!sim.trade_routes.is_empty(), "AI should have created a route");
        assert!(!sim.trade_ships.is_empty(), "AI should have spawned a ship");
        assert_eq!(sim.trade_routes[0].owner, 1);
        assert_eq!(sim.trade_routes[0].stops.len(), 2);
        // Ship cost was deducted.
        assert!(sim.players[1].gold < 5_000);
    }

    #[test]
    fn ai_skips_trade_route_with_one_island() {
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 5_000;
        // Only one island warehouse — not eligible.
        sim.warehouses.push(Warehouse::new(0, 1, 10, 10));
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Economic, Difficulty::Hard,
        ));
        sim.tick_ai();
        assert!(sim.trade_routes.is_empty());
        assert!(sim.trade_ships.is_empty());
    }

    #[test]
    fn residence_promotes_when_tier_fully_satisfied() {
        use crate::building::{BuildingDef, BuildingInstance};
        use crate::types::{Good, ProductionType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        // Make tier 0 (Pioneer) fully satisfied so the WOHN gets
        // promoted to Settler tier on the next tick_population.
        sim.players[0].satisfaction[0] = 128;
        sim.building_defs.push(BuildingDef {
            id: 0, category: 0, width: 1, height: 1,
            production_type: ProductionType::Residence,
            kind: "WOHN".into(), prod_kind: "WOHN".into(),
            radius: 0,
            output_good: Good::None, input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0, input_1_rate: 0, input_2_rate: 0,
            storage_capacity: 0, cycle_time_ms: 0, carrier_interval_ms: 0,
            cost_gold: 0, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 0,
        });
        sim.buildings.push(BuildingInstance::new(0, 0, 0, 0, 0));
        assert_eq!(sim.buildings[0].house_tier, 0);
        sim.tick_population();
        assert_eq!(sim.buildings[0].house_tier, 1);
    }

    #[test]
    fn building_maintenance_aggregates_per_player() {
        use crate::types::{Good, ProductionType};
        use crate::building::{BuildingDef, BuildingInstance};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        let mk_def = |maint: u16| BuildingDef {
            id: 0, category: 0, width: 1, height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools, input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0, input_1_rate: 0, input_2_rate: 0,
            storage_capacity: 50, cycle_time_ms: 1000, carrier_interval_ms: 0,
            cost_gold: 0, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: maint,
        };
        sim.building_defs.push(mk_def(5)); // def 0 cost 5
        sim.building_defs.push(mk_def(8)); // def 1 cost 8
        // Player 0: 2× def0 + 1× def1 → 5+5+8 = 18
        sim.buildings.push(BuildingInstance::new(0, 0, 0, 0, 0));
        sim.buildings.push(BuildingInstance::new(0, 0, 1, 1, 0));
        sim.buildings.push(BuildingInstance::new(1, 0, 2, 2, 0));
        // Player 1: 1× def1 → 8
        sim.buildings.push(BuildingInstance::new(1, 0, 3, 3, 1));
        // Make all "built" so they count.
        for b in &mut sim.buildings { b.construction_ms_remaining = 0; }
        sim.tick_population();
        assert_eq!(sim.players[0].building_maintenance, 18);
        assert_eq!(sim.players[1].building_maintenance, 8);
    }

    #[test]
    fn unfinished_buildings_do_not_pay_maintenance() {
        use crate::types::{Good, ProductionType};
        use crate::building::{BuildingDef, BuildingInstance};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.building_defs.push(BuildingDef {
            id: 0, category: 0, width: 1, height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools, input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0, input_1_rate: 0, input_2_rate: 0,
            storage_capacity: 0, cycle_time_ms: 1000, carrier_interval_ms: 0,
            cost_gold: 0, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 7,
        });
        // One under construction, one finished.
        let mut bb = BuildingInstance::new(0, 0, 0, 0, 0);
        bb.construction_ms_total = 5_000;
        bb.construction_ms_remaining = 5_000;
        sim.buildings.push(bb);
        sim.buildings.push(BuildingInstance::new(0, 0, 1, 1, 0));
        sim.tick_population();
        // Only the finished building should be counted.
        assert_eq!(sim.players[0].building_maintenance, 7);
    }

    #[test]
    fn ai_declares_war_on_weak_neighbor() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        // Drain the human player's default starting gold so AI(1) clearly
        // outscores them on the (units * 10 + gold/200) heuristic.
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        // AI(1) is Military and beefy; player 0 is weak.
        for _ in 0..10 {
            sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 1, 0, 0));
        }
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Military, Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        assert_eq!(sim.diplomacy.get(1, 0), Diplomacy::War);
        // Symmetric.
        assert_eq!(sim.diplomacy.get(0, 1), Diplomacy::War);
    }

    #[test]
    fn ai_sues_for_peace_when_outmatched() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        // Start at war.
        sim.diplomacy.set(1, 0, Diplomacy::War);
        // Player 0 vastly outmuscles AI(1).
        for _ in 0..30 {
            sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 0, 0, 0));
        }
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Military, Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        assert_eq!(sim.diplomacy.get(1, 0), Diplomacy::Neutral);
    }

    #[test]
    fn economic_ai_does_not_declare_war() {
        use crate::ai::{AiController, AiPersonality, Difficulty};
        use crate::combat::{Diplomacy, MilitaryUnit, UnitType};
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.players.push(Player::new_ai(1, 0));
        sim.players[0].gold = 0;
        sim.players[1].gold = 5_000;
        for _ in 0..10 {
            sim.military_units.push(MilitaryUnit::new(UnitType::Swordsman, 1, 0, 0));
        }
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Economic, Difficulty::Hard,
        ));
        sim.tick_diplomacy();
        // Economic AI shouldn't have flipped to war even with the upper hand.
        assert_eq!(sim.diplomacy.get(1, 0), Diplomacy::Neutral);
    }

    #[test]
    fn ai_request_military_spawns_units() {
        let mut sim = Simulation::new();
        // Player slot 1 is the AI we're driving.
        sim.players.push(Player::new_human(0)); // slot 0 (unused for this test)
        sim.players.push(Player::new_ai(1, 0));
        sim.players[1].gold = 5_000;
        sim.players[1].total_population = 200;
        sim.players[1].population[1] = 200; // make calculate_costs etc. sane
        // AI needs a warehouse to spawn near.
        sim.warehouses.push(Warehouse::new(0, 1, 30, 40));
        // Wire an AI controller bound to slot 1 with the Military personality
        // running on Hard so its target unit count is reasonable.
        sim.ai_controllers.push(AiController::new(
            1, AiPersonality::Military, Difficulty::Hard,
        ));
        // Force-trigger the AI tick once.
        sim.tick_ai();
        // Expect at least one swordsman spawned for player 1.
        let owned: usize = sim.military_units.iter()
            .filter(|u| u.owner == 1 && u.is_alive())
            .count();
        assert!(owned > 0, "AI should have spawned at least one unit, got {owned}");
        // And paid for them.
        assert!(sim.players[1].gold < 5_000);
    }
}
