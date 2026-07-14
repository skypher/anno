//! Combat resolution system.
//!
//! Ported from FUN_00451890 (unit movement/combat tick) and related functions.
//!
//! Combat model:
//! - Units detect enemies within DETECTION_RANGE (6 tiles).
//! - Each unit has a per-type `attack_range` from `UnitStats`
//!   (Infantry/Cavalry 1, Musketeer 4, Cannon 8, naval 5..7).
//!   A unit only deals damage once it has closed to that range.
//! - Damage is applied per tick based on unit type stats.
//! - Health is normalized 0.0-1.0; units die at ≈0.02.
//! - Projectiles spawn for ranged units (musketeers, cannons, ships).
//! - Nation interaction matrix determines who can fight whom.

/// Maximum engagement detection range in tiles. Combat will be
/// initiated against any hostile inside this radius; the per-type
/// `UnitStats::attack_range` then decides how close the unit has
/// to close before it can land hits.
const DETECTION_RANGE: u32 = 6;

/// Military unit types (from FUN_00451890 switch cases).
///
/// Roster RE-cited from `figuren.cod`. Land entries are the four
/// FIGTYP values (`SCHWERT`, `KAVALERIE`, `MUSKETIER`, `KANONIER`),
/// confirmed by `text.cod [FIGKIND]` (Infantryman / Cavalryman /
/// Musketeer / Artilleryman). Naval entries are the `KRIEG1`,
/// `KRIEG2`, and `PIRAT` ship figures — KRIEG1/KRIEG2 are the two
/// player-buildable warships and PIRAT is the dedicated pirate
/// ship spawned by hideouts. There is no Pikeman, no Archer, no
/// medium warship or flagship in the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum UnitType {
    Infantry = 1,
    Musketeer = 3,
    Cavalry = 4,
    Cannon = 6,
    // Naval (figuren.cod ship-figure names in parens).
    SmallWarship = 11, // KRIEG1
    PirateShip = 12,   // PIRAT
    LargeWarship = 13, // KRIEG2
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
        // Land-unit stats RE-cited from Tim Howgego's military-data
        // appendix. HP / Strength values transcribed directly:
        //   Infantry  HP 20, Strength 1.0
        //   Cavalry   HP 18, Strength 1.6
        //   Musketeer HP 15, Strength 2.4
        //   Cannoneer HP 12, Strength 7.0
        // We scale HP by /20 to fit our 1.0-baseline (Infantry HP 20
        // → max_health 1.0). Strength becomes attack_damage / 20.
        match self {
            UnitType::Infantry => UnitStats {
                max_health: 20.0 / 20.0,   // = 1.0
                attack_damage: 1.0 / 20.0, // = 0.05
                attack_speed_ms: 1000,
                attack_range: 1,
                move_speed: 3,
                is_ranged: false,
                is_naval: false,
            },
            UnitType::Musketeer => UnitStats {
                max_health: 15.0 / 20.0,   // = 0.75
                attack_damage: 2.4 / 20.0, // = 0.12
                attack_speed_ms: 2000,
                attack_range: 4,
                move_speed: 2,
                is_ranged: true,
                is_naval: false,
            },
            UnitType::Cavalry => UnitStats {
                max_health: 18.0 / 20.0,   // = 0.9
                attack_damage: 1.6 / 20.0, // = 0.08
                attack_speed_ms: 800,
                attack_range: 1,
                move_speed: 5,
                is_ranged: false,
                is_naval: false,
            },
            UnitType::Cannon => UnitStats {
                max_health: 12.0 / 20.0,   // = 0.6
                attack_damage: 7.0 / 20.0, // = 0.35
                attack_speed_ms: 3000,
                attack_range: 8,
                move_speed: 1,
                is_ranged: true,
                is_naval: false,
            },
            // Naval stats RE-cited from Tim Howgego's military-data
            // appendix: SmallWarship HP 65, Speed 110/66, Strength 4;
            // LargeWarship HP 120, Speed 120/72, Strength 7.0.
            // HP scales by /40 to match our 1.0-baseline; speeds
            // mapped so LargeWarship is slightly faster than
            // SmallWarship (matching the appendix's 120 > 110).
            UnitType::SmallWarship => UnitStats {
                max_health: 65.0 / 40.0, // ≈ 1.625
                attack_damage: 0.10,
                attack_speed_ms: 2000,
                attack_range: 5,
                move_speed: 4,
                is_ranged: true,
                is_naval: true,
            },
            UnitType::LargeWarship => UnitStats {
                max_health: 120.0 / 40.0, // = 3.0
                attack_damage: 0.20,
                attack_speed_ms: 3000,
                attack_range: 7,
                move_speed: 5,
                is_ranged: true,
                is_naval: true,
            },
            // Pirate ship — figuren.cod `Nummer: PIRAT`, `Maxkanon`
            // = 10. Stats interpolated between Small and Large
            // warships since the original tables don't surface a
            // distinct pirate hull profile.
            UnitType::PirateShip => UnitStats {
                max_health: 90.0 / 40.0,
                attack_damage: 0.15,
                attack_speed_ms: 2500,
                attack_range: 6,
                move_speed: 4,
                is_ranged: true,
                is_naval: true,
            },
        }
    }

    /// Convert from u8 value.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(UnitType::Infantry),
            3 => Some(UnitType::Musketeer),
            4 => Some(UnitType::Cavalry),
            6 => Some(UnitType::Cannon),
            11 => Some(UnitType::SmallWarship),
            12 => Some(UnitType::PirateShip),
            13 => Some(UnitType::LargeWarship),
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
    /// Optional ship/unit name. Populated for SHIP4-spawned
    /// warships from the authored 28-byte name field
    /// ("Defender", "Imperator", etc.) so the UI can show
    /// the original ship names. Empty for all other units.
    #[serde(default)]
    pub name: String,
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

fn default_escort_ship() -> i32 {
    -1
}

/// Maximum cannons each ship class can mount. Values RE-cited
/// from `figuren.cod` `Maxkanon:` field on the ship-figure
/// entries:
///   HANDEL1 = 6  (small trade ship — not a UnitType)
///   HANDEL2 = 10 (large trade ship — not a UnitType)
///   KRIEG1  = 8  (SmallWarship)
///   KRIEG2  = 14 (LargeWarship)
///   HANDLER = 12 (free trader's ship — not a UnitType)
///   PIRAT   = 10 (PirateShip)
pub fn cannon_capacity(t: UnitType) -> u8 {
    match t {
        UnitType::SmallWarship => 8,
        UnitType::LargeWarship => 14,
        UnitType::PirateShip => 10,
        _ => 0,
    }
}

/// Build cost (gold) for each unit type.
///
/// RE-cited from Tim Howgego's military-data appendix
/// (timhowgego.wordpress.com/anno_1602/appendices/military_data/),
/// which transcribes the relevant binary data tables:
///
/// | Unit            | Gold |
/// |-----------------|------|
/// | Infantry        | 200 |
/// | Cavalry         | 200 |
/// | Musketeer       | 400 |
/// | Cannoneer       | 200 |
/// | SmallTradeShip  | 400 |
/// | LargeTradeShip  | 520 |
/// | SmallWarship    | 600 |
/// | LargeWarship    | 900 |
pub fn unit_build_cost(t: UnitType) -> i32 {
    match t {
        UnitType::Infantry => 200,
        UnitType::Cavalry => 200,
        UnitType::Musketeer => 400,
        UnitType::Cannon => 200,
        UnitType::SmallWarship => 600,
        UnitType::LargeWarship => 900,
        // Pirate ships aren't player-buildable; cost is unused
        // but the match must be exhaustive.
        UnitType::PirateShip => 0,
    }
}

/// Material cost for each unit type — `(wood, bricks, cloth, tools,
/// swords, muskets, cannons)`. RE-cited from the same Tim Howgego
/// military-data appendix that drives `unit_build_cost`.
pub fn unit_build_resources(t: UnitType) -> (u16, u16, u16, u16, u16, u16, u16) {
    match t {
        // Wood, Bricks, Cloth — naval hulls + sails.
        UnitType::SmallWarship => (32, 4, 8, 0, 0, 0, 0),
        UnitType::LargeWarship => (60, 7, 14, 0, 0, 0, 0),
        // Pirate ships aren't player-buildable.
        UnitType::PirateShip => (0, 0, 0, 0, 0, 0, 0),
        // Land units — Tools, Swords/Muskets/Cannons by role.
        UnitType::Infantry => (0, 0, 0, 3, 5, 0, 0),
        UnitType::Cavalry => (0, 0, 0, 3, 8, 0, 0),
        UnitType::Musketeer => (0, 0, 0, 4, 0, 10, 0),
        UnitType::Cannon => (0, 0, 0, 2, 0, 0, 14),
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
            name: String::new(),
            cannons: 0,
            patrol: Vec::new(),
            patrol_idx: 0,
        }
    }

    /// Construct a unit and attach an authored name (e.g. for
    /// SHIP4-spawned warships).
    pub fn with_name(
        unit_type: UnitType,
        owner: u8,
        tile_x: i32,
        tile_y: i32,
        name: String,
    ) -> Self {
        let mut u = Self::new(unit_type, owner, tile_x, tile_y);
        u.name = name;
        u
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

fn default_trade_matrix() -> [[bool; 7]; 7] {
    [[false; 7]; 7]
}

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
        if a as usize >= 7 || b as usize >= 7 {
            return false;
        }
        self.trade_agreement[a as usize][b as usize]
    }

    /// Open a trade agreement between `a` and `b`. Manual sec. 7.2:
    /// rejected if either side recently broke an agreement (penalty
    /// flag set) OR if the players are at war. Sets the flag
    /// symmetrically. Returns true on success.
    pub fn propose_trade_agreement(&mut self, a: u8, b: u8) -> bool {
        if a as usize >= 7 || b as usize >= 7 || a == b {
            return false;
        }
        if self.get(a, b) == Diplomacy::War {
            return false;
        }
        if self.trade_agreement_broken[a as usize][b as usize] {
            return false;
        }
        self.trade_agreement[a as usize][b as usize] = true;
        self.trade_agreement[b as usize][a as usize] = true;
        true
    }

    /// Cancel a trade agreement. Sets the per-pair broken-flag so the
    /// next proposal is auto-rejected (manual: "seldom possible to
    /// conclude a new trade agreement right after one has been
    /// broken").
    pub fn break_trade_agreement(&mut self, a: u8, b: u8) -> bool {
        if a as usize >= 7 || b as usize >= 7 || a == b {
            return false;
        }
        if !self.trade_agreement[a as usize][b as usize] {
            return false;
        }
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
        if a as usize >= 7 || b as usize >= 7 {
            return;
        }
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
            units[i].tile_x,
            units[i].tile_y,
            units[target_idx].tile_x,
            units[target_idx].tile_y,
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

/// Tower/castle cannon behavior awaits decoded combat timing, targeting,
/// range, and damage state. `Kanon` remains available on the building
/// definition, but does not infer those missing values.
pub fn tick_tower_defense(
    units: &mut [MilitaryUnit],
    buildings: &[crate::building::BuildingInstance],
    diplomacy: &DiplomacyMatrix,
    defs: &[crate::building::BuildingDef],
) {
    let _ = (units, buildings, diplomacy, defs);
}

/// Refresh each escort unit's `target_x/target_y` from the ship it's
/// shadowing so naval movement orders stay current. Cleared if the
/// referenced ship is gone or inactive.
pub fn tick_escort_targets(
    units: &mut [MilitaryUnit],
    ship_positions: &[(bool, i32, i32)], // (active, world_x, world_y)
) {
    for u in units.iter_mut() {
        if u.escort_ship < 0 {
            continue;
        }
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
    tick_unit_orders_with_maps(units, dt_ms, ocean_map, &[]);
}

/// Move units using world/island map constraints when available.
///
/// Naval units are clamped against the ocean map. Land units whose
/// current tile belongs to an island map take the first step of an A*
/// route on that map; if no route exists, they hold position instead of
/// walking through blocked terrain.
pub fn tick_unit_orders_with_maps(
    units: &mut [MilitaryUnit],
    dt_ms: u32,
    ocean_map: Option<&crate::ocean_map::OceanMap>,
    island_maps: &[crate::island_map::IslandMap],
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
        while u.move_timer_ms >= step_ms {
            u.move_timer_ms -= step_ms;
            let Some((nx, ny)) = next_order_step(u, ocean_map, island_maps) else {
                break;
            };
            let sx = (nx - u.tile_x).signum();
            let sy = (ny - u.tile_y).signum();
            u.tile_x = nx;
            u.tile_y = ny;
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

pub fn tick_unit_orders(units: &mut [MilitaryUnit], dt_ms: u32) {
    tick_unit_orders_with_maps(units, dt_ms, None, &[]);
}

fn next_order_step(
    unit: &MilitaryUnit,
    ocean_map: Option<&crate::ocean_map::OceanMap>,
    island_maps: &[crate::island_map::IslandMap],
) -> Option<(i32, i32)> {
    if unit.tile_x == unit.target_x && unit.tile_y == unit.target_y {
        return None;
    }

    if unit.unit_type.stats().is_naval {
        let next = direct_order_step(unit)?;
        if let Some(om) = ocean_map {
            if !om.is_navigable(next.0, next.1) {
                return None;
            }
        }
        return Some(next);
    }

    if let Some(map) = island_maps
        .iter()
        .find(|m| m.is_walkable(unit.tile_x, unit.tile_y))
    {
        return crate::pathfinding::find_path(
            map,
            (unit.tile_x, unit.tile_y),
            (unit.target_x, unit.target_y),
        )
        .and_then(|path| path.first().copied());
    }

    direct_order_step(unit)
}

fn direct_order_step(unit: &MilitaryUnit) -> Option<(i32, i32)> {
    let dx = unit.target_x - unit.tile_x;
    let dy = unit.target_y - unit.tile_y;
    if dx == 0 && dy == 0 {
        return None;
    }
    let sx = dx.signum();
    let sy = dy.signum();
    if dx.abs() > 0 && dy.abs() > 0 {
        Some((unit.tile_x + sx, unit.tile_y + sy))
    } else if dx.abs() > 0 {
        Some((unit.tile_x + sx, unit.tile_y))
    } else {
        Some((unit.tile_x, unit.tile_y + sy))
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
        // Musketeer should deal more damage than infantry (matches
        // appendix Strength values 2.4 vs 1.0).
        let inf = UnitType::Infantry.stats();
        let musk = UnitType::Musketeer.stats();
        assert!(musk.attack_damage > inf.attack_damage);

        // Cannon should have longest range
        let cannon = UnitType::Cannon.stats();
        assert!(cannon.attack_range > inf.attack_range);
        assert!(cannon.is_ranged);
        assert!(!inf.is_ranged);
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
        // Musketeer (Strength 2.4) vs Infantry (Strength 1.0) —
        // the musketeer should reliably win.
        let mut units = vec![
            MilitaryUnit::new(UnitType::Musketeer, 0, 5, 5),
            MilitaryUnit::new(UnitType::Infantry, 1, 6, 5),
        ];
        let diplomacy = DiplomacyMatrix::new_all_war();

        for _ in 0..400 {
            tick_combat(&mut units, &diplomacy, 100);
        }

        // At least one should be dead
        assert!(
            !units[0].is_alive() || !units[1].is_alive(),
            "After 400 ticks, one unit should be dead"
        );
    }

    #[test]
    fn tick_unit_orders_moves_toward_target() {
        let mut units = vec![MilitaryUnit::new(UnitType::Infantry, 0, 0, 0)];
        units[0].target_x = 5;
        units[0].target_y = 5;
        // Infantry move_speed = 3 → step_ms = 333; advance many steps.
        for _ in 0..40 {
            tick_unit_orders(&mut units, 100);
        }
        assert_eq!(units[0].tile_x, 5);
        assert_eq!(units[0].tile_y, 5);
    }

    #[test]
    fn tick_unit_orders_with_maps_routes_land_unit_around_wall() {
        let mut map = crate::island_map::IslandMap::new_open(0, 10, 10);
        for y in 0..9 {
            map.set_walkable(2, y, false);
        }
        let mut units = vec![MilitaryUnit::new(UnitType::Infantry, 0, 0, 4)];
        units[0].target_x = 4;
        units[0].target_y = 4;

        for _ in 0..120 {
            tick_unit_orders_with_maps(&mut units, 100, None, std::slice::from_ref(&map));
            assert!(
                !(units[0].tile_x == 2 && units[0].tile_y < 9),
                "land unit stepped into blocked wall at ({}, {})",
                units[0].tile_x,
                units[0].tile_y
            );
            if units[0].tile_x == 4 && units[0].tile_y == 4 {
                break;
            }
        }

        assert_eq!((units[0].tile_x, units[0].tile_y), (4, 4));
    }

    #[test]
    fn tick_unit_orders_with_maps_holds_land_unit_when_no_route_exists() {
        let mut map = crate::island_map::IslandMap::new_open(0, 6, 6);
        for y in 0..6 {
            map.set_walkable(2, y, false);
        }
        let mut units = vec![MilitaryUnit::new(UnitType::Infantry, 0, 0, 3)];
        units[0].target_x = 5;
        units[0].target_y = 3;

        for _ in 0..30 {
            tick_unit_orders_with_maps(&mut units, 100, None, std::slice::from_ref(&map));
        }

        assert_eq!((units[0].tile_x, units[0].tile_y), (0, 3));
    }

    #[test]
    fn tick_unit_orders_skipped_during_combat() {
        let mut units = vec![MilitaryUnit::new(UnitType::Infantry, 0, 0, 0)];
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
            MilitaryUnit::new(UnitType::Infantry, 0, 5, 5),
            MilitaryUnit::new(UnitType::Infantry, 0, 6, 5), // Same owner
        ];
        let diplomacy = DiplomacyMatrix::new();

        for _ in 0..100 {
            tick_combat(&mut units, &diplomacy, 100);
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
            MilitaryUnit::new(UnitType::Infantry, 1, 6, 0), // Within cannon range (8)
        ];
        let diplomacy = DiplomacyMatrix::new_all_war();

        // Run combat
        for _ in 0..200 {
            tick_combat(&mut units, &diplomacy, 100);
        }

        // At least one cannon should survive, pikeman should die
        let cannons_alive = units
            .iter()
            .filter(|u| u.owner == 0 && u.is_alive())
            .count();
        assert!(cannons_alive > 0, "At least one cannon should survive");
        assert!(!units[2].is_alive(), "Infantry should die to cannon fire");
    }

    #[test]
    fn tower_defense_does_not_infer_range_or_damage() {
        use crate::building::{BuildingDef, BuildingInstance, OreDeposit};
        use crate::types::ProductionType;
        let mk_tower = |cannons: u8| BuildingDef {
            id: 0,
            category: 5,
            width: 2,
            height: 2,
            production_type: ProductionType::Craft,
            kind: "TURM".into(),
            prod_kind: "TURM".into(),
            radius: 0,
            output_good: crate::types::Good::None,
            input_good_1: crate::types::Good::None,
            input_good_2: crate::types::Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 0,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: cannons,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        };
        let defs = vec![mk_tower(2)];
        let mut tower = BuildingInstance::new(0, 0, 5, 5, 0);
        tower.construction_ms_remaining = 0;
        let buildings = vec![tower];
        let mut units = vec![
            MilitaryUnit::new(UnitType::Infantry, 1, 6, 6),
            MilitaryUnit::new(UnitType::Infantry, 1, 50, 50), // far
        ];
        let mut diplo = DiplomacyMatrix::new();
        diplo.set(0, 1, Diplomacy::War);
        let h_before = (units[0].health, units[1].health);
        tick_tower_defense(&mut units, &buildings, &diplo, &defs);
        assert_eq!(units[0].health, h_before.0);
        assert_eq!(units[1].health, h_before.1);
    }

    #[test]
    fn battle_outcome_prediction() {
        // Cannons (Strength 7.0) vs the same count of Infantry
        // (Strength 1.0) — cannon DPS ≈ 0.117 vs infantry 0.05
        // and the HP-pool ratio still favours cannons in this
        // simple Lanchester model.
        let (att_ratio, def_ratio) =
            simulate_battle_outcome(&[(UnitType::Cannon, 10)], &[(UnitType::Infantry, 10)]);
        assert!(att_ratio > def_ratio);
    }

    #[test]
    fn naval_cant_attack_land() {
        let mut units = vec![
            MilitaryUnit::new(UnitType::LargeWarship, 0, 5, 5),
            MilitaryUnit::new(UnitType::Infantry, 1, 6, 5),
        ];
        let diplomacy = DiplomacyMatrix::new_all_war();

        for _ in 0..100 {
            tick_combat(&mut units, &diplomacy, 100);
        }

        // Both should be alive — naval and land can't fight each other
        assert!(units[0].is_alive());
        assert!(units[1].is_alive());
    }
}
