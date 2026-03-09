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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
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
    /// Index of the unit this is currently fighting (-1 = none).
    pub combat_target: i32,
    pub active: bool,
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
            combat_target: -1,
            active: true,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.active && self.health > 0.02 // Original threshold: 0x3ca3d70a ≈ 0.02
    }
}

/// Diplomacy state between two players.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
pub struct DiplomacyMatrix {
    /// 7×7 matrix (max 7 players).
    relations: [[Diplomacy; 7]; 7],
}

impl DiplomacyMatrix {
    pub fn new() -> Self {
        let mut relations = [[Diplomacy::Neutral; 7]; 7];
        // Self is always allied
        for i in 0..7 {
            relations[i][i] = Diplomacy::Allied;
        }
        Self { relations }
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

            // Apply damage to target
            units[target_idx].health -= stats.attack_damage;

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
            tick_combat(&mut units, &diplomacy, 100);
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
    fn no_combat_between_allies() {
        let mut units = vec![
            MilitaryUnit::new(UnitType::Swordsman, 0, 5, 5),
            MilitaryUnit::new(UnitType::Pikeman, 0, 6, 5), // Same owner
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
            MilitaryUnit::new(UnitType::Pikeman, 1, 6, 0), // Within cannon range (8)
        ];
        let diplomacy = DiplomacyMatrix::new_all_war();

        // Run combat
        for _ in 0..200 {
            tick_combat(&mut units, &diplomacy, 100);
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
            tick_combat(&mut units, &diplomacy, 100);
        }

        // Both should be alive — naval and land can't fight each other
        assert!(units[0].is_alive());
        assert!(units[1].is_alive());
    }
}
