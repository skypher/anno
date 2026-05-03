//! Natural disasters: fire and volcano eruptions.
//!
//! RE references:
//!
//! - `figuren.cod` `Nummer: VULKAN` (figure index 0x12) — eruption
//!   visual, AnimAnz 30 frames at 48ms each, sound `WAV_VULKAN1, 3`
//!   (i.e. one of three vulkan1.wav / vulkan2.wav / vulkan3.wav,
//!   loaded at `1602_exe.c:106445-106447`).
//! - `figuren.cod` `Nummer: BRANDMARKT` (figure index 0x08) — fire
//!   marker placed on a burning building (`Nowalkani: 1` so figures
//!   path-around it). 32 frames per anim, three anims (start-up,
//!   peak burn, dying down) at 50 ms each.
//! - `haeuser.cod` building Kind `BRANDECK` exists for the burning
//!   ruin tile pre-removal; we reuse the existing combat-damage
//!   pipeline (`combat::DamageEvent`, `BuildingInstance::health`)
//!   instead of adding a separate ruin record.
//!
//! Per-building fire damage cap is RE-cited:
//! - `haeuser.cod` building defaults `Maxbrand: 4` — the per-
//!   building maximum number of fire-damage ticks before the
//!   building burns out. The COD parser at `1602_exe.c:68086`
//!   (`s_Maxbrand_0049a288`) reads this field at HAUS_PRODTYP
//!   level. Below: `MAX_BRAND_DAMAGE_TICKS = 4`.
//!
//! Eruption / ignition probabilities remain SPECULATIVE — those
//! live in dispatcher functions not yet located. We gate spawns on
//! the existing event-tick rate (`tick_events` 10 000 ms) so the
//! disaster cadence stays sane in long games.

use crate::building::BuildingInstance;
use crate::combat::DamageEvent;

/// Probability gate (1-in-N) that a fire ignites somewhere on the
/// human player's settlement during a single event tick. SPECULATIVE.
pub const FIRE_IGNITION_GATE: u64 = 8;

/// Damage applied to a building when fire ticks. Tuned so that
/// `MAX_BRAND_DAMAGE_TICKS * FIRE_TICK_DAMAGE = 20` hp out of 100,
/// i.e. an unattended fire eats 20% of building HP before
/// burnout. SPECULATIVE.
pub const FIRE_TICK_DAMAGE: u16 = 5;

/// Maximum fire damage ticks a single building can absorb before
/// burnout. RE: `haeuser.cod` default `Maxbrand: 4` (parsed at
/// `1602_exe.c:68086`). Each `ignite_building` call counts as one
/// tick; multiplied by `FIRE_TICK_DAMAGE` this caps total fire hp
/// loss per ignition cycle at 20.
pub const MAX_BRAND_DAMAGE_TICKS: u16 = 4;

/// Probability gate (1-in-N) that a fire on a building this tick
/// extinguishes naturally. SPECULATIVE.
pub const FIRE_EXTINGUISH_GATE: u64 = 4;

/// Probability gate (1-in-N) that a volcanic island erupts during a
/// single event tick. SPECULATIVE.
pub const VOLCANO_ERUPTION_GATE: u64 = 32;

/// Damage radius of a volcanic eruption, in tiles. SPECULATIVE.
pub const VOLCANO_RADIUS: u16 = 6;

/// Damage applied to every building in the eruption radius. SPECULATIVE.
pub const VOLCANO_DAMAGE: u16 = 35;

/// Mark a building as on fire by setting its health into the
/// "burning" range and emitting a damage event. The combat pipeline
/// already removes buildings whose health reaches 0.
pub fn ignite_building(
    building: &mut BuildingInstance,
    damage_events: &mut Vec<DamageEvent>,
) {
    let dmg = FIRE_TICK_DAMAGE.min(building.health);
    building.health = building.health.saturating_sub(dmg);
    damage_events.push(DamageEvent {
        x: building.tile_x as i32,
        y: building.tile_y as i32,
        amount: dmg,
        target: 1,
    });
}

/// Apply volcano eruption damage to every active building within
/// `VOLCANO_RADIUS` Chebyshev tiles of the centre. Mirrors the
/// engine's per-building damage path so destruction goes through
/// the existing combat clean-up.
pub fn erupt_at(
    cx: u16,
    cy: u16,
    island_id: u8,
    buildings: &mut [BuildingInstance],
    damage_events: &mut Vec<DamageEvent>,
) -> u32 {
    let mut hit = 0u32;
    for b in buildings.iter_mut() {
        if !b.active || b.island_id != island_id { continue; }
        let dx = (b.tile_x as i32 - cx as i32).abs();
        let dy = (b.tile_y as i32 - cy as i32).abs();
        if dx.max(dy) > VOLCANO_RADIUS as i32 { continue; }
        let dmg = VOLCANO_DAMAGE.min(b.health);
        b.health = b.health.saturating_sub(dmg);
        damage_events.push(DamageEvent {
            x: b.tile_x as i32,
            y: b.tile_y as i32,
            amount: dmg,
            target: 1,
        });
        hit += 1;
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::{BuildingInstance, BUILDING_MAX_HEALTH};

    fn mk(island: u8, x: u16, y: u16) -> BuildingInstance {
        let mut b = BuildingInstance::new(0, island, x, y, 0);
        b.construction_ms_remaining = 0;
        b
    }

    #[test]
    fn ignite_drops_health_and_emits_event() {
        let mut b = mk(0, 5, 5);
        let h0 = b.health;
        let mut events = Vec::new();
        ignite_building(&mut b, &mut events);
        assert!(b.health < h0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].amount, FIRE_TICK_DAMAGE.min(BUILDING_MAX_HEALTH));
    }

    #[test]
    fn erupt_only_damages_within_radius() {
        let mut buildings = vec![
            mk(0, 10, 10),
            mk(0, 10 + VOLCANO_RADIUS as u16, 10), // on the edge
            mk(0, 50, 50),                          // far away
            mk(1, 10, 10),                          // different island
        ];
        let mut events = Vec::new();
        let hit = erupt_at(10, 10, 0, &mut buildings, &mut events);
        assert_eq!(hit, 2);
        assert!(buildings[0].health < BUILDING_MAX_HEALTH);
        assert!(buildings[1].health < BUILDING_MAX_HEALTH);
        assert_eq!(buildings[2].health, BUILDING_MAX_HEALTH);
        assert_eq!(buildings[3].health, BUILDING_MAX_HEALTH);
    }
}
