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
//!   tile overlay. This module currently applies fire damage directly
//!   to `BuildingInstance::health`; static `Ruinenr` replacement is
//!   handled by the simulation destruction event after a building dies.
//!
//! Per-building fire damage cap is RE-cited:
//! - `haeuser.cod` building defaults `Maxbrand: 4` — the per-
//!   building fire-damage cap. The COD parser at
//!   `1602_exe.c:68086` (`s_Maxbrand_0049a288`) reads this field at
//!   HAUS_PRODTYP level. `BuildingDef::max_brand_damage_ticks`
//!   carries the inherited source value into the simulator.
//!
//! Live fire and volcano scheduling need decoded source triggers/anchors.
//! This module keeps the lower-level effects available without inventing
//! origins from ordinary player buildings.

use crate::building::BuildingInstance;

/// Damage applied to a building when fire ticks (per 10s event
/// tick). Calibrated against a player observation reported on
/// Tim Howgego's Anno-1602 finally page:
///   "a house [observed] burning for approximately 10 minutes
///    before it eventually collapsed and was immediately rebuilt"
/// 10 min × 60s = 600 s; at 10s event ticks that's 60 ticks. With
/// `BUILDING_MAX_HEALTH = 100` we want ~100/60 ≈ 1.67 hp per tick;
/// round to 2.
pub const FIRE_TICK_DAMAGE: u16 = 2;

/// Default fire damage cap. RE: `haeuser.cod` default
/// `Maxbrand: 4` (parsed at `1602_exe.c:68086`). The simulation
/// uses each building definition's inherited `Maxbrand` value when
/// available.
pub const MAX_BRAND_DAMAGE_TICKS: u16 = crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS;

/// Damage radius of a volcanic eruption, in tiles. SPECULATIVE.
pub const VOLCANO_RADIUS: u16 = 6;

/// Damage applied to every building in the eruption radius. SPECULATIVE.
pub const VOLCANO_DAMAGE: u16 = 35;

/// Apply one source-capped fire-damage tick to a building using the
/// `Maxbrand` cap carried by its building definition. Returns `true`
/// when damage was applied.
pub fn ignite_building_with_cap(
    building: &mut BuildingInstance,
    max_brand_damage_ticks: u16,
) -> bool {
    let cap = if max_brand_damage_ticks == 0 {
        MAX_BRAND_DAMAGE_TICKS
    } else {
        max_brand_damage_ticks
    };
    if building.fire_damage_ticks >= cap {
        return false;
    }
    building.fire_damage_ticks += 1;
    let dmg = FIRE_TICK_DAMAGE.min(building.health);
    building.health = building.health.saturating_sub(dmg);
    dmg > 0
}

/// Apply a fire-damage tick using the source default cap.
pub fn ignite_building(building: &mut BuildingInstance) -> bool {
    ignite_building_with_cap(building, MAX_BRAND_DAMAGE_TICKS)
}

/// Apply volcano eruption damage to every active building within
/// `VOLCANO_RADIUS` Chebyshev tiles of the centre. Mirrors the
/// engine's per-building damage path.
pub fn erupt_at(cx: u16, cy: u16, island_id: u8, buildings: &mut [BuildingInstance]) -> u32 {
    let mut hit = 0u32;
    for b in buildings.iter_mut() {
        if !b.active || b.island_id != island_id {
            continue;
        }
        let dx = (b.tile_x as i32 - cx as i32).abs();
        let dy = (b.tile_y as i32 - cy as i32).abs();
        if dx.max(dy) > VOLCANO_RADIUS as i32 {
            continue;
        }
        let dmg = VOLCANO_DAMAGE.min(b.health);
        b.health = b.health.saturating_sub(dmg);
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
    fn ignite_drops_health() {
        let mut b = mk(0, 5, 5);
        let h0 = b.health;
        assert!(ignite_building(&mut b));
        assert!(b.health < h0);
        assert_eq!(b.fire_damage_ticks, 1);
        assert_eq!(b.health, h0 - FIRE_TICK_DAMAGE.min(BUILDING_MAX_HEALTH));
    }

    #[test]
    fn ignite_stops_at_maxbrand_tick_cap() {
        let mut b = mk(0, 5, 5);
        for _ in 0..MAX_BRAND_DAMAGE_TICKS {
            assert!(ignite_building(&mut b));
        }
        let capped_health = b.health;
        assert!(!ignite_building(&mut b));
        assert_eq!(b.fire_damage_ticks, MAX_BRAND_DAMAGE_TICKS);
        assert_eq!(b.health, capped_health);
        assert_eq!(
            b.health,
            BUILDING_MAX_HEALTH - FIRE_TICK_DAMAGE * MAX_BRAND_DAMAGE_TICKS
        );
    }

    #[test]
    fn erupt_only_damages_within_radius() {
        let mut buildings = vec![
            mk(0, 10, 10),
            mk(0, 10 + VOLCANO_RADIUS as u16, 10), // on the edge
            mk(0, 50, 50),                         // far away
            mk(1, 10, 10),                         // different island
        ];
        let hit = erupt_at(10, 10, 0, &mut buildings);
        assert_eq!(hit, 2);
        assert!(buildings[0].health < BUILDING_MAX_HEALTH);
        assert!(buildings[1].health < BUILDING_MAX_HEALTH);
        assert_eq!(buildings[2].health, BUILDING_MAX_HEALTH);
        assert_eq!(buildings[3].health, BUILDING_MAX_HEALTH);
    }
}
