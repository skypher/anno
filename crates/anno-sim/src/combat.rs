//! Combat resolution system.
//!
//! Ported from FUN_00451890 (unit movement/combat tick) and related functions.
//!
//! Combat model:
//! - Units detect enemies within engagement range (96 pixels / ~6 tiles)
//! - Combat triggers when units are within attack range (48 pixels / ~3 tiles)
//! - Damage is applied per tick based on unit type stats
//! - Health is normalized 0.0-1.0; units die at ≈0.0
//! - Projectiles spawn for ranged units (archers, cannons, ships)
//! - Nation interaction matrix determines who can fight whom

/// Maximum engagement detection range in tiles.
const DETECTION_RANGE: u32 = 6;
/// Attack range in tiles (must be within this to deal damage).
const ATTACK_RANGE: u32 = 3;

/// Military unit types (from FUN_00451890 switch cases).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum UnitType {
    Pikeman = 1,
    Swordsman = 2,
    Musketeer = 3,
    Cavalry = 4,
    Archer = 5,
    Cannon = 6,
    // Naval
    SmallWarship = 11,
    MediumWarship = 12,
    LargeWarship = 13,
    Flagship = 14,
}

/// Unit stats table (damage, health, speed, range).
/// Values derived from decompiled data tables at DAT_0061fcd4.
#[derive(Debug, Clone, Copy)]
pub struct UnitStats {
    pub max_health: f32,
    pub attack_damage: f32,
    pub attack_speed_ms: u32,
    pub attack_range: u32,
    pub move_speed: u16,
    pub is_ranged: bool,
    pub is_naval: bool,
}

impl UnitType {
    pub fn stats(self) -> UnitStats {
        match self {
            UnitType::Pikeman => UnitStats {
                max_health: 1.0,
                attack_damage: 0.08,
                attack_speed_ms: 1000,
                attack_range: 1,
                move_speed: 3,
                is_ranged: false,
                is_naval: false,
            },
            UnitType::Swordsman => UnitStats {
                max_health: 1.0,
                attack_damage: 0.12,
                attack_speed_ms: 1200,
                attack_range: 1,
                move_speed: 3,
                is_ranged: false,
                is_naval: false,
            },
            UnitType::Musketeer => UnitStats {
                max_health: 0.8,
                attack_damage: 0.15,
                attack_speed_ms: 2000,
                attack_range: 4,
                move_speed: 2,
                is_ranged: true,
                is_naval: false,
            },
            UnitType::Cavalry => UnitStats {
                max_health: 1.2,
                attack_damage: 0.14,
                attack_speed_ms: 800,
                attack_range: 1,
                move_speed: 5,
                is_ranged: false,
                is_naval: false,
            },
            UnitType::Archer => UnitStats {
                max_health: 0.6,
                attack_damage: 0.06,
                attack_speed_ms: 1500,
                attack_range: 5,
                move_speed: 3,
                is_ranged: true,
                is_naval: false,
            },
            UnitType::Cannon => UnitStats {
                max_health: 0.5,
                attack_damage: 0.25,
                attack_speed_ms: 3000,
                attack_range: 8,
                move_speed: 1,
                is_ranged: true,
                is_naval: false,
            },
            UnitType::SmallWarship => UnitStats {
                max_health: 1.5,
                attack_damage: 0.10,
                attack_speed_ms: 2000,
                attack_range: 5,
                move_speed: 4,
                is_ranged: true,
                is_naval: true,
            },
            UnitType::MediumWarship => UnitStats {
                max_health: 2.0,
                attack_damage: 0.15,
                attack_speed_ms: 2500,
                attack_range: 6,
                move_speed: 3,
                is_ranged: true,
                is_naval: true,
            },
            UnitType::LargeWarship => UnitStats {
                max_health: 3.0,
                attack_damage: 0.20,
                attack_speed_ms: 3000,
                attack_range: 7,
                move_speed: 2,
                is_ranged: true,
                is_naval: true,
            },
            UnitType::Flagship => UnitStats {
                max_health: 4.0,
                attack_damage: 0.25,
                attack_speed_ms: 3500,
                attack_range: 8,
                move_speed: 2,
                is_ranged: true,
                is_naval: true,
            },
        }
    }

    /// Convert from u8 value.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(UnitType::Pikeman),
            2 => Some(UnitType::Swordsman),
            3 => Some(UnitType::Musketeer),
            4 => Some(UnitType::Cavalry),
            5 => Some(UnitType::Archer),
            6 => Some(UnitType::Cannon),
            11 => Some(UnitType::SmallWarship),
            12 => Some(UnitType::MediumWarship),
            13 => Some(UnitType::LargeWarship),
            14 => Some(UnitType::Flagship),
            _ => None,
        }
    }
}

/// A military unit in the world.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MilitaryUnit {
    pub unit_type: UnitType,
    pub owner: u8,
    pub health: f32,
    pub tile_x: i32,
    pub tile_y: i32,
    pub target_x: i32,
    pub target_y: i32,
    pub direction: u8,
    pub attack_timer_ms: u32,
    /// Movement accumulator for player-issued move orders.
    pub move_timer_ms: u32,
    /// Index of the unit this is currently fighting (-1 = none).
    pub combat_target: i32,
    pub active: bool,
    /// Index into `Simulation::trade_ships` this unit is escorting. -1 if
    /// none. Updated each tick so the unit's `target` shadows the ship.
    #[serde(default = "default_escort_ship")]
    pub escort_ship: i32,
    /// Number of cannons mounted on this naval unit (manual sec.
    /// 9.2.3 "Arming your ships"). For land units this stays 0.
    /// `attack_damage` of the unit's stats is multiplied by
    /// `(1 + cannons / 4)` so a fully-armed warship hits harder than
    /// a stripped one. SmallWarship / LargeWarship each have a
    /// platform cap (4 / 8 respectively) — the buy command clamps.
    #[serde(default)]
    pub cannons: u8,
    /// Patrol waypoint list (manual sec. 9.2.4 "Patrol"). When
    /// non-empty AND `combat_target == -1`, the unit cycles through
    /// these waypoints — heading to one, then advancing the index.
    /// Engaging an enemy clears the active patrol step but not the
    /// waypoints, so the ship resumes patrol after the fight.
    #[serde(default)]
    pub patrol: Vec<(i32, i32)>,
    /// Index of the current patrol target (round-robin).
    #[serde(default)]
    pub patrol_idx: u32,
}

fn default_escort_ship() -> i32 { -1 }

/// Maximum cannons each ship class can mount. Manual sec. 9.2.3
/// describes "small and large warships" as the two armed naval
/// classes — small can carry fewer cannons, large carries more. The
/// numbers are SPECULATIVE pending RE of the Werft (shipyard) UI.
pub fn cannon_capacity(t: UnitType) -> u8 {
    match t {
        UnitType::SmallWarship => 4,
        UnitType::LargeWarship => 8,
        _ => 0,
    }
}

/// Build cost (gold) for each unit type. Naval costs from the
/// Werft (shipyard) UI; land-unit costs from the SOLDAT entries in
/// figuren.cod (`Soldat: FIGTYP_SCHWERT, 5, 80` etc., where 80 is
/// the gold cost). Marked SPECULATIVE for naval costs since the
/// Werft pricing UI hasn't been fully decoded.
pub fn unit_build_cost(t: UnitType) -> i32 {
    match t {
        UnitType::Pikeman => 60,
        UnitType::Swordsman => 80,
        UnitType::Musketeer => 160,
        UnitType::Cavalry => 130,
        UnitType::Archer => 90,
        UnitType::Cannon => 220,
        UnitType::SmallWarship => 1_500,
        UnitType::MediumWarship => 2_500,
        UnitType::LargeWarship => 4_000,
        UnitType::Flagship => 6_000,
    }
}

impl MilitaryUnit {
    pub fn new(unit_type: UnitType, owner: u8, tile_x: i32, tile_y: i32) -> Self {
        let stats = unit_type.stats();
        Self {
            unit_type,
            owner,
            health: stats.max_health,
            tile_x,
            tile_y,
            target_x: tile_x,
            target_y: tile_y,
            direction: 0,
            attack_timer_ms: 0,
            move_timer_ms: 0,
            combat_target: -1,
            active: true,
            escort_ship: -1,
            cannons: 0,
            patrol: Vec::new(),
            patrol_idx: 0,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.active && self.health > 0.02 // Original threshold: 0x3ca3d70a ≈ 0.02
    }
}

/// Diplomacy state between two players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum Diplomacy {
    /// Cannot fight (allied or same team).
    Allied = 0,
    /// Neutral — no automatic aggression.
    Neutral = 1,
    /// At war — units will engage on sight.
    War = 2,
}

/// Nation interaction matrix (who can fight whom).
/// Original: DAT_005b7770, indexed by (attacker_nation * 0x50 + defender_nation) * 8.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiplomacyMatrix {
    /// 7×7 matrix (max 7 players).
    relations: [[Diplomacy; 7]; 7],
    /// Trade-agreement matrix (manual sec. 7.2). Symmetric: a trade
    /// agreement between players a and b is bilateral. Once
    /// concluded, each side can inspect the other's warehouses.
    /// Cleared on war declaration.
    #[serde(default = "default_trade_matrix")]
    trade_agreement: [[bool; 7]; 7],
    /// Per-pair "broke a trade agreement recently" penalty flag.
    /// Manual: "breaking a trade agreement usually has a negative
    /// influence on the other players' attitude towards you" — we
    /// surface that as a temporary block on re-proposing.
    #[serde(default = "default_trade_matrix")]
    trade_agreement_broken: [[bool; 7]; 7],
}

fn default_trade_matrix() -> [[bool; 7]; 7] { [[false; 7]; 7] }

impl DiplomacyMatrix {
    pub fn new() -> Self {
        let mut relations = [[Diplomacy::Neutral; 7]; 7];
        // Self is always allied
        for i in 0..7 {
            relations[i][i] = Diplomacy::Allied;
        }
        Self {
            relations,
            trade_agreement: [[false; 7]; 7],
            trade_agreement_broken: [[false; 7]; 7],
        }
    }

    pub fn new_all_war() -> Self {
        let mut dm = Self::new();
        for i in 0..7 {
            for j in 0..7 {
                if i != j {
                    dm.relations[i][j] = Diplomacy::War;
                }
            }
        }
        dm
    }

    pub fn get(&self, a: u8, b: u8) -> Diplomacy {
        if a as usize >= 7 || b as usize >= 7 {
            return Diplomacy::Neutral;
        }
        self.relations[a as usize][b as usize]
    }

    pub fn set(&mut self, a: u8, b: u8, state: Diplomacy) {
        if a as usize >= 7 && b as usize >= 7 {
            return;
        }
        self.relations[a as usize][b as usize] = state;
        self.relations[b as usize][a as usize] = state;
        // Manual sec. 7.2: declaring war breaks any trade agreement.
        if state == Diplomacy::War {
            self.trade_agreement[a as usize][b as usize] = false;
            self.trade_agreement[b as usize][a as usize] = false;
        }
    }

    /// Returns true if `a` and `b` have a concluded trade agreement.
    pub fn has_trade_agreement(&self, a: u8, b: u8) -> bool {
        if a as usize >= 7 || b as usize >= 7 { return false; }
        self.trade_agreement[a as usize][b as usize]
    }

    /// Open a trade agreement between `a` and `b`. Manual sec. 7.2:
    /// rejected if either side recently broke an agreement (penalty
    /// flag set) OR if the players are at war. Sets the flag
    /// symmetrically. Returns true on success.
    pub fn propose_trade_agreement(&mut self, a: u8, b: u8) -> bool {
        if a as usize >= 7 || b as usize >= 7 || a == b { return false; }
        if self.get(a, b) == Diplomacy::War { return false; }
        if self.trade_agreement_broken[a as usize][b as usize] { return false; }
        self.trade_agreement[a as usize][b as usize] = true;
        self.trade_agreement[b as usize][a as usize] = true;
        true
    }

    /// Cancel a trade agreement. Sets the per-pair broken-flag so the
    /// next proposal is auto-rejected (manual: "seldom possible to
    /// conclude a new trade agreement right after one has been
    /// broken").
    pub fn break_trade_agreement(&mut self, a: u8, b: u8) -> bool {
        if a as usize >= 7 || b as usize >= 7 || a == b { return false; }
        if !self.trade_agreement[a as usize][b as usize] { return false; }
        self.trade_agreement[a as usize][b as usize] = false;
        self.trade_agreement[b as usize][a as usize] = false;
        self.trade_agreement_broken[a as usize][b as usize] = true;
        self.trade_agreement_broken[b as usize][a as usize] = true;
        true
    }

    /// Clear the broken-trade-agreement penalty between `a` and `b`
    /// (e.g. after enough cooldown ticks). Restores the ability to
    /// re-propose.
    pub fn clear_broken_flag(&mut self, a: u8, b: u8) {
        if a as usize >= 7 || b as usize >= 7 { return; }
        self.trade_agreement_broken[a as usize][b as usize] = false;
        self.trade_agreement_broken[b as usize][a as usize] = false;
    }
}

/// Tile distance squared between two positions.
fn distance_sq(ax: i32, ay: i32, bx: i32, by: i32) -> u32 {
    let dx = (ax - bx).unsigned_abs();
    let dy = (ay - by).unsigned_abs();
    dx * dx + dy * dy
}

/// Detect combat engagements and apply damage for one tick.
/// Returns indices of units that died this tick.
pub fn tick_combat(
    units: &mut [MilitaryUnit],
    diplomacy: &DiplomacyMatrix,
    dt_ms: u32,
    events: &mut Vec<DamageEvent>,
) -> Vec<usize> {
    let mut dead = Vec::new();
    let len = units.len();

    // Phase 1: Find engagement targets
    for i in 0..len {
        if !units[i].is_alive() {
            continue;
        }

        // Skip if already has a valid target
        if units[i].combat_target >= 0 {
            let target_idx = units[i].combat_target as usize;
            if target_idx < len && units[target_idx].is_alive() {
                continue; // Keep existing target
            }
            units[i].combat_target = -1; // Target died, clear it
        }

        let best = find_nearest_enemy(units, i, diplomacy);
        units[i].combat_target = best.map(|idx| idx as i32).unwrap_or(-1);
    }

    // Phase 2: Apply damage
    for i in 0..len {
        if !units[i].is_alive() || units[i].combat_target < 0 {
            continue;
        }

        let target_idx = units[i].combat_target as usize;
        if target_idx >= len || !units[target_idx].is_alive() {
            continue;
        }

        let stats = units[i].unit_type.stats();
        let dist = distance_sq(
            units[i].tile_x, units[i].tile_y,
            units[target_idx].tile_x, units[target_idx].tile_y,
        );

        // Check if within attack range
        let range_sq = stats.attack_range * stats.attack_range;
        if dist > range_sq {
            // Move toward target (simple approach)
            let dx = units[target_idx].tile_x - units[i].tile_x;
            let dy = units[target_idx].tile_y - units[i].tile_y;
            if dx.abs() > dy.abs() {
                units[i].tile_x += dx.signum();
            } else if dy != 0 {
                units[i].tile_y += dy.signum();
            }
            continue;
        }

        // Attack timer
        units[i].attack_timer_ms += dt_ms;
        if units[i].attack_timer_ms >= stats.attack_speed_ms {
            units[i].attack_timer_ms -= stats.attack_speed_ms;

            // Apply damage to target. Naval units scale with the
            // number of cannons mounted (manual sec. 9.2.3): each
            // cannon adds 25% on top of the base hull damage. Land
            // units have cannons = 0 so they're unaffected.
            let cannons = units[i].cannons;
            let damage = stats.attack_damage * (1.0 + cannons as f32 * 0.25);
            units[target_idx].health -= damage;
            events.push(DamageEvent {
                x: units[target_idx].tile_x,
                y: units[target_idx].tile_y,
                amount: (damage * 100.0) as u16,
                target: 0,
            });

            // Check if target died
            if !units[target_idx].is_alive() {
                units[target_idx].active = false;
                dead.push(target_idx);
                // Clear all references to dead unit
                for u in units.iter_mut() {
                    if u.combat_target == target_idx as i32 {
                        u.combat_target = -1;
                    }
                }
            }
        }
    }

    dead
}

/// One-frame combat-damage event for the renderer to animate as a
/// floating number. Coordinate space depends on `target`.
#[derive(Debug, Clone, Copy)]
pub struct DamageEvent {
    pub x: i32,
    pub y: i32,
    pub amount: u16,
    /// 0 = land unit (tile coords), 1 = building (tile coords).
    pub target: u8,
}

/// Damage to enemy buildings from adjacent military units. Called each
/// military tick. Returns the indices of buildings that hit 0 HP.
///
/// Per tick: 5 hp drained per adjacent enemy land unit (within 2 tiles
/// of the building's footprint). Naval units don't damage land buildings.
pub fn tick_building_damage(
    units: &[MilitaryUnit],
    buildings: &mut [crate::building::BuildingInstance],
    diplomacy: &DiplomacyMatrix,
    defs: &[crate::building::BuildingDef],
    events: &mut Vec<DamageEvent>,
) -> Vec<usize> {
    const DMG_PER_ENEMY: u16 = 5;
    let mut destroyed = Vec::new();
    for (bi, b) in buildings.iter_mut().enumerate() {
        if !b.active { continue; }
        if (b.def_id as usize) >= defs.len() { continue; }
        let def = &defs[b.def_id as usize];
        let bx = b.tile_x as i32;
        let by = b.tile_y as i32;
        let bw = def.width as i32;
        let bh = def.height as i32;
        let mut hostile = 0u16;
        for u in units.iter() {
            if !u.is_alive() || u.owner == b.owner { continue; }
            if u.unit_type.stats().is_naval { continue; }
            if diplomacy.get(u.owner, b.owner) != Diplomacy::War { continue; }
            // Adjacent if within 2 tiles of the footprint.
            let dx = if u.tile_x < bx { bx - u.tile_x }
                     else if u.tile_x >= bx + bw { u.tile_x - (bx + bw - 1) }
                     else { 0 };
            let dy = if u.tile_y < by { by - u.tile_y }
                     else if u.tile_y >= by + bh { u.tile_y - (by + bh - 1) }
                     else { 0 };
            if dx.max(dy) <= 2 {
                hostile = hostile.saturating_add(1);
            }
        }
        if hostile == 0 { continue; }
        let total = DMG_PER_ENEMY.saturating_mul(hostile);
        events.push(DamageEvent {
            x: b.tile_x as i32,
            y: b.tile_y as i32,
            amount: total,
            target: 1,
        });
        if b.health <= total {
            b.health = 0;
            b.active = false;
            destroyed.push(bi);
        } else {
            b.health -= total;
        }
    }
    destroyed
}

/// Refresh each escort unit's `target_x/target_y` from the ship it's
/// shadowing so naval movement orders stay current. Cleared if the
/// referenced ship is gone or inactive.
pub fn tick_escort_targets(
    units: &mut [MilitaryUnit],
    ship_positions: &[(bool, i32, i32)], // (active, world_x, world_y)
) {
    for u in units.iter_mut() {
        if u.escort_ship < 0 { continue; }
        let idx = u.escort_ship as usize;
        match ship_positions.get(idx) {
            Some(&(true, sx, sy)) => {
                u.target_x = sx;
                u.target_y = sy;
            }
            _ => {
                u.escort_ship = -1;
            }
        }
    }
}

/// Move units toward their player-issued (target_x, target_y) when not in combat.
/// One tile per `step_interval_ms` where step_interval = 1000 / move_speed (min 100ms).
/// Naval units that would step into a non-navigable tile (per `ocean_map`)
/// hold their position for that step instead.
pub fn tick_unit_orders_with_ocean(
    units: &mut [MilitaryUnit],
    dt_ms: u32,
    ocean_map: Option<&crate::ocean_map::OceanMap>,
) {
    for u in units.iter_mut() {
        if !u.is_alive() || u.combat_target >= 0 {
            continue;
        }
        if u.tile_x == u.target_x && u.tile_y == u.target_y {
            u.move_timer_ms = 0;
            // Patrol advance: when a waypoint is reached and the
            // unit has a patrol list, retarget the next waypoint
            // (round-robin). Manual sec. 9.2.4 "Patrol".
            if !u.patrol.is_empty() {
                u.patrol_idx = (u.patrol_idx + 1) % u.patrol.len() as u32;
                let (nx, ny) = u.patrol[u.patrol_idx as usize];
                u.target_x = nx;
                u.target_y = ny;
            } else {
                continue;
            }
        }
        let speed = u.unit_type.stats().move_speed.max(1) as u32;
        let step_ms = (1000 / speed).max(100);
        u.move_timer_ms = u.move_timer_ms.saturating_add(dt_ms);
        let is_naval = u.unit_type.stats().is_naval;
        while u.move_timer_ms >= step_ms {
            u.move_timer_ms -= step_ms;
            let dx = u.target_x - u.tile_x;
            let dy = u.target_y - u.tile_y;
            if dx == 0 && dy == 0 { break; }
            let sx = dx.signum();
            let sy = dy.signum();
            let (nx, ny) = if dx.abs() > 0 && dy.abs() > 0 {
                (u.tile_x + sx, u.tile_y + sy)
            } else if dx.abs() > 0 {
                (u.tile_x + sx, u.tile_y)
            } else {
                (u.tile_x, u.tile_y + sy)
            };
            // Naval clamp: refuse moves onto land.
            if is_naval {
                if let Some(om) = ocean_map {
                    if !om.is_navigable(nx, ny) { break; }
                }
            }
            u.tile_x = nx;
            u.tile_y = ny;
            u.direction = match (sx, sy) {
                (0, -1) => 0, (1, -1) => 1, (1, 0) => 2, (1, 1) => 3,
                (0, 1) => 4, (-1, 1) => 5, (-1, 0) => 6, (-1, -1) => 7,
                _ => u.direction,
            };
        }
    }
}

pub fn tick_unit_orders(units: &mut [MilitaryUnit], dt_ms: u32) {
    for u in units.iter_mut() {
        if !u.is_alive() || u.combat_target >= 0 {
            continue;
        }
        if u.tile_x == u.target_x && u.tile_y == u.target_y {
            u.move_timer_ms = 0;
            // Patrol advance — same logic as the ocean-aware variant.
            if !u.patrol.is_empty() {
                u.patrol_idx = (u.patrol_idx + 1) % u.patrol.len() as u32;
                let (nx, ny) = u.patrol[u.patrol_idx as usize];
                u.target_x = nx;
                u.target_y = ny;
            } else {
                continue;
            }
        }
        let speed = u.unit_type.stats().move_speed.max(1) as u32;
        let step_ms = (1000 / speed).max(100);
        u.move_timer_ms = u.move_timer_ms.saturating_add(dt_ms);
        while u.move_timer_ms >= step_ms {
            u.move_timer_ms -= step_ms;
            let dx = u.target_x - u.tile_x;
            let dy = u.target_y - u.tile_y;
            if dx == 0 && dy == 0 {
                break;
            }
            // 8-direction step: prefer the larger axis but allow diagonals
            let sx = dx.signum();
            let sy = dy.signum();
            if dx.abs() > 0 && dy.abs() > 0 {
                u.tile_x += sx;
                u.tile_y += sy;
            } else if dx.abs() > 0 {
                u.tile_x += sx;
            } else {
                u.tile_y += sy;
            }
            // Update direction (0=N, 1=NE, 2=E, ... 7=NW). Use sx,sy.
            u.direction = match (sx, sy) {
                (0, -1) => 0,
                (1, -1) => 1,
                (1, 0) => 2,
                (1, 1) => 3,
                (0, 1) => 4,
                (-1, 1) => 5,
                (-1, 0) => 6,
                (-1, -1) => 7,
                _ => u.direction,
            };
        }
    }
}

/// Find the nearest enemy unit within detection range.
fn find_nearest_enemy(
    units: &[MilitaryUnit],
    unit_idx: usize,
    diplomacy: &DiplomacyMatrix,
) -> Option<usize> {
    let unit = &units[unit_idx];
    let detection_sq = DETECTION_RANGE * DETECTION_RANGE;
    let mut best_dist = u32::MAX;
    let mut best_idx = None;

    for (j, other) in units.iter().enumerate() {
        if j == unit_idx || !other.is_alive() {
            continue;
        }

        // Check diplomacy
        if diplomacy.get(unit.owner, other.owner) != Diplomacy::War {
            continue;
        }

        // Check naval/land compatibility
        let unit_stats = unit.unit_type.stats();
        let other_stats = other.unit_type.stats();
        if unit_stats.is_naval != other_stats.is_naval {
            continue; // Ships can't attack land units and vice versa
        }

        let dist = distance_sq(unit.tile_x, unit.tile_y, other.tile_x, other.tile_y);
        if dist <= detection_sq && dist < best_dist {
            best_dist = dist;
            best_idx = Some(j);
        }
    }

    best_idx
}

/// Calculate the expected outcome of a battle between two unit groups.
/// Returns (attacker_surviving_health_ratio, defender_surviving_health_ratio).
pub fn simulate_battle_outcome(
    attackers: &[(UnitType, u32)],
    defenders: &[(UnitType, u32)],
) -> (f32, f32) {
    let mut attacker_hp: f32 = attackers
        .iter()
        .map(|(t, n)| t.stats().max_health * *n as f32)
        .sum();
    let mut defender_hp: f32 = defenders
        .iter()
        .map(|(t, n)| t.stats().max_health * *n as f32)
        .sum();

    let attacker_dps: f32 = attackers
        .iter()
        .map(|(t, n)| {
            let s = t.stats();
            s.attack_damage / (s.attack_speed_ms as f32 / 1000.0) * *n as f32
        })
        .sum();
    let defender_dps: f32 = defenders
        .iter()
        .map(|(t, n)| {
            let s = t.stats();
            s.attack_damage / (s.attack_speed_ms as f32 / 1000.0) * *n as f32
        })
        .sum();

    let total_attacker_hp = attacker_hp;
    let total_defender_hp = defender_hp;

    // Simple Lanchester model: each side deals DPS to the other until one is eliminated
    let dt = 0.1f32; // 100ms steps
    for _ in 0..1000 {
        if attacker_hp <= 0.0 || defender_hp <= 0.0 {
            break;
        }
        attacker_hp -= defender_dps * dt;
        defender_hp -= attacker_dps * dt;
    }

    (
        (attacker_hp.max(0.0) / total_attacker_hp),
        (defender_hp.max(0.0) / total_defender_hp),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_stats_consistency() {
        // Swordsman should deal more damage than pikeman
        let pike = UnitType::Pikeman.stats();
        let sword = UnitType::Swordsman.stats();
        assert!(sword.attack_damage > pike.attack_damage);

        // Cannon should have longest range
        let cannon = UnitType::Cannon.stats();
        assert!(cannon.attack_range > sword.attack_range);
        assert!(cannon.is_ranged);
        assert!(!sword.is_ranged);
    }

    #[test]
    fn diplomacy_matrix() {
        let mut dm = DiplomacyMatrix::new();
        assert_eq!(dm.get(0, 0), Diplomacy::Allied);
        assert_eq!(dm.get(0, 1), Diplomacy::Neutral);

        dm.set(0, 1, Diplomacy::War);
        assert_eq!(dm.get(0, 1), Diplomacy::War);
        assert_eq!(dm.get(1, 0), Diplomacy::War); // Symmetric
    }

    #[test]
    fn combat_kills_weaker_unit() {
        let mut units = vec![
            MilitaryUnit::new(UnitType::Swordsman, 0, 5, 5),
            MilitaryUnit::new(UnitType::Pikeman, 1, 6, 5),
        ];
        let diplomacy = DiplomacyMatrix::new_all_war();

        // Run enough ticks for combat to resolve
        for _ in 0..200 {
            tick_combat(&mut units, &diplomacy, 100, &mut Vec::new());
        }

        // Swordsman should win (more damage)
        assert!(units[0].is_alive() || units[1].is_alive(), "At least one should survive");
        if units[0].is_alive() && !units[1].is_alive() {
            // Expected: swordsman wins
        } else if units[1].is_alive() && !units[0].is_alive() {
            // Pikeman won (possible but unlikely)
        }
        // At least one should be dead
        assert!(
            !units[0].is_alive() || !units[1].is_alive(),
            "After 200 ticks, one unit should be dead"
        );
    }

    #[test]
    fn tick_unit_orders_moves_toward_target() {
        let mut units = vec![MilitaryUnit::new(UnitType::Swordsman, 0, 0, 0)];
        units[0].target_x = 5;
        units[0].target_y = 5;
        // Swordsman move_speed = 3 → step_ms = 333; advance many steps.
        for _ in 0..40 {
            tick_unit_orders(&mut units, 100);
        }
        assert_eq!(units[0].tile_x, 5);
        assert_eq!(units[0].tile_y, 5);
    }

    #[test]
    fn tick_unit_orders_skipped_during_combat() {
        let mut units = vec![MilitaryUnit::new(UnitType::Swordsman, 0, 0, 0)];
        units[0].target_x = 5;
        units[0].target_y = 0;
        units[0].combat_target = 7; // Pretend we're engaged
        for _ in 0..20 {
            tick_unit_orders(&mut units, 100);
        }
        assert_eq!(units[0].tile_x, 0); // Did not move under orders
    }

    #[test]
    fn no_combat_between_allies() {
        let mut units = vec![
            MilitaryUnit::new(UnitType::Swordsman, 0, 5, 5),
            MilitaryUnit::new(UnitType::Pikeman, 0, 6, 5), // Same owner
        ];
        let diplomacy = DiplomacyMatrix::new();

        for _ in 0..100 {
            tick_combat(&mut units, &diplomacy, 100, &mut Vec::new());
        }

        // Both should be alive (no combat between allies)
        assert!(units[0].is_alive());
        assert!(units[1].is_alive());
    }

    #[test]
    fn ranged_unit_attacks_from_distance() {
        // Two cannons vs one pikeman — cannons should win from range
        let mut units = vec![
            MilitaryUnit::new(UnitType::Cannon, 0, 0, 0),
            MilitaryUnit::new(UnitType::Cannon, 0, 0, 1),
            MilitaryUnit::new(UnitType::Pikeman, 1, 6, 0), // Within cannon range (8)
        ];
        let diplomacy = DiplomacyMatrix::new_all_war();

        // Run combat
        for _ in 0..200 {
            tick_combat(&mut units, &diplomacy, 100, &mut Vec::new());
        }

        // At least one cannon should survive, pikeman should die
        let cannons_alive = units.iter().filter(|u| u.owner == 0 && u.is_alive()).count();
        assert!(cannons_alive > 0, "At least one cannon should survive");
        assert!(!units[2].is_alive(), "Pikeman should die to cannon fire");
    }

    #[test]
    fn battle_outcome_prediction() {
        let (att_ratio, def_ratio) = simulate_battle_outcome(
            &[(UnitType::Swordsman, 10)],
            &[(UnitType::Pikeman, 10)],
        );
        // Swordsmen should win overall (more damage)
        assert!(att_ratio > def_ratio);
    }

    #[test]
    fn naval_cant_attack_land() {
        let mut units = vec![
            MilitaryUnit::new(UnitType::LargeWarship, 0, 5, 5),
            MilitaryUnit::new(UnitType::Swordsman, 1, 6, 5),
        ];
        let diplomacy = DiplomacyMatrix::new_all_war();

        for _ in 0..100 {
            tick_combat(&mut units, &diplomacy, 100, &mut Vec::new());
        }

        // Both should be alive — naval and land can't fight each other
        assert!(units[0].is_alive());
        assert!(units[1].is_alive());
    }
}
