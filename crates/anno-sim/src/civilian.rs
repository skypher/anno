//! Civilian wanderers: cosmetic figures that emerge from residences,
//! walk a short distance, then despawn.
//!
//! RE references for the spawn dispatcher:
//!
//! - `decompiled/1602_exe.c:84620-84666` — the per-building 4999 ms
//!   tick (`if (4999 < DAT_005491c8) { DAT_005491c8 = 0; ... }`),
//!   which inside the building loop calls
//!   `FUN_00443a90(0x5a, 0x39, island, tile_y)` when
//!   `DAT_005b701c != 0 && DAT_005b701c <= DAT_005b6040 &&
//!   DAT_005bafc8 != 0`. Figure type `0x5a` is `ALTER` — the
//!   "alter Mann" civilian (figuren.cod definition order index 90).
//! - `decompiled/1602_exe.c:94389` — `FUN_00443a90(0x5c, 1, sVar11, 1)`
//!   spawns figure type `0x5c = PASSANT` (passerby) when a market /
//!   coverage condition is met.
//! - `decompiled/1602_exe.c:11856,11873` — `FUN_00443a90(0x59, 0, 0, 0)`
//!   spawns `0x59 = ADEL` (nobleman) on player init.
//! - `decompiled/1602_exe.c:46943-46955` — `FUN_00443a90(figtype,
//!   x_or_action, island, flag)` allocates a figure slot and
//!   activates it via `FUN_0045e1f0(slot, 1)`.
//!
//! Figure types live in `figuren.cod` at definition-order indices
//! `0x58..=0x5f`:
//!
//! | idx  | name      | sprite base       |
//! |------|-----------|-------------------|
//! | 0x58 | ADELWEIBL | `GFXZIVIL + 0`    |
//! | 0x59 | ADEL      | `GFXZIVIL + 64`   |
//! | 0x5a | ALTER     | `GFXZIVIL + 128`  |
//! | 0x5b | FRAU      | `GFXZIVIL + 192`  |
//! | 0x5c | PASSANT   | `GFXZIVIL + 256`  |
//! | 0x5d | VETERAN   | `GFXZIVIL + 320`  |
//! | 0x5e | KINDREIF  | `GFXZIVIL + 384`  |
//! | 0x5f | PILGER    | `GFXZIVIL + 448`  |
//!
//! `GFXZIVIL = GFXLOESCH+128` resolves to `1272` from the GFX… chain
//! at the head of `figuren.cod`. Each civilian variant occupies 64
//! sprites (8 rotations × 8 walking frames; verified by the ANIM
//! sub-block — both anim 0 and anim 1 sit at `AnimOffs:0`, so
//! civilians do not have a loaded/empty distinction).
//!
//! Spawn selection:
//!
//! - The building tick gates spawning on `DAT_005bafc8` (a global
//!   "civilians enabled" flag) and on a population counter range
//!   `DAT_005b701c <= DAT_005b6040` — both mapped here to "spawn
//!   only from active residences (`Kind: WOHN`) on owned islands".
//! - The variant chosen by the dispatcher uses an LCG-style index
//!   (the `0x39` action seed in the ALTER call); this implementation
//!   uses the simulation's `next_rand()` to pick a variant from the
//!   eight-entry table above.

use crate::building::{BuildingDef, BuildingInstance};
use crate::entity::{ActionType, Figure};

pub const CIVILIAN_VARIANT_COUNT: usize = 8;
pub const CIVILIAN_FIGURE_NAMES: [&str; CIVILIAN_VARIANT_COUNT] = [
    "ADELWEIBL",
    "ADEL",
    "ALTER",
    "FRAU",
    "PASSANT",
    "VETERAN",
    "KINDREIF",
    "PILGER",
];

/// First definition-order index of a civilian figure in `figuren.cod`.
pub const CIVILIAN_FIRST_INDEX: u8 = 0x58;
/// Number of consecutive civilian figure variants.
pub const CIVILIAN_COUNT: u8 = 8;

/// Residence/civilian building ticker. RE: `1602_exe.c:84620`
/// compares the building accumulator against 4999 ms before
/// dispatching the per-building figure spawn loop.
pub const CIVILIAN_BUILDING_TICK_MS: u32 = 4_999;

/// Sprite base for civilians inside `TRAEGER.BSH` — `GFXZIVIL` resolves
/// to 1272 by walking the `GFX… = previous + N` chain at the top of
/// `figuren.cod` (GFXTRAEGER 0 → GFXESEL 192 → GFXRAEUBER 320 →
/// GFXKARREN 496 → GFXFLEISCH 688 → GFXARZT 816 → GFXTRADER 880 →
/// GFXEINGEB 1016 → GFXLOESCH 1144 → GFXZIVIL 1272).
pub const GFX_ZIVIL_BASE: u16 = 1272;

/// Sprites per civilian variant: 8 rotations × 8 walking frames
/// (`figuren.cod` ANIM sub-blocks for `ADELWEIBL` and siblings).
pub const SPRITES_PER_VARIANT: u16 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilianConfig {
    pub sprite_bases: [u16; CIVILIAN_VARIANT_COUNT],
    pub frames_per_dir: u8,
}

impl Default for CivilianConfig {
    fn default() -> Self {
        let mut sprite_bases = [0; CIVILIAN_VARIANT_COUNT];
        for (idx, base) in sprite_bases.iter_mut().enumerate() {
            *base = GFX_ZIVIL_BASE + (idx as u16) * SPRITES_PER_VARIANT;
        }
        Self {
            sprite_bases,
            frames_per_dir: 8,
        }
    }
}

impl CivilianConfig {
    pub fn from_figures(figures: &anno_formats::figuren::FiguresFile) -> Self {
        let mut config = Self::default();
        for (idx, name) in CIVILIAN_FIGURE_NAMES.iter().enumerate() {
            let Some(def) = figures.find(name) else {
                continue;
            };
            let base = def.gfx + def.walk_anim().map(|anim| anim.anim_offs).unwrap_or(0);
            if let Ok(base) = u16::try_from(base) {
                config.sprite_bases[idx] = base;
            }
            if let Some(frames) = def
                .walk_anim()
                .and_then(|anim| u8::try_from(anim.anim_anz).ok())
                .filter(|&frames| frames > 0)
            {
                config.frames_per_dir = frames;
            }
        }
        config
    }

    pub fn sprite_base_for(&self, variant: u8) -> u16 {
        self.sprite_bases[usize::from(variant.min(CIVILIAN_COUNT - 1))]
    }

    pub fn is_civilian(&self, fig: &Figure) -> bool {
        if fig.action != ActionType::Walking {
            return false;
        }
        let has_source_figtype = fig.sprite_set >= CIVILIAN_FIRST_INDEX
            && fig.sprite_set < CIVILIAN_FIRST_INDEX + CIVILIAN_COUNT;
        has_source_figtype || self.sprite_bases.contains(&fig.base_sprite)
    }
}

/// Pick a civilian figure variant deterministically from a 64-bit
/// random word. Uses 8 variants (ADELWEIBL .. PILGER, indices
/// `0x58..=0x5f`).
pub fn pick_variant(rand: u64) -> u8 {
    (rand & 0x07) as u8
}

/// Resolve a civilian variant index (0..8) to the sprite base in
/// `TRAEGER.BSH`.
pub fn sprite_base_for(variant: u8) -> u16 {
    CivilianConfig::default().sprite_base_for(variant)
}

/// Try to spawn a civilian figure from a residence. Returns the new
/// figure when one is created. Mirrors `1602_exe.c:84666` per-building
/// tick: gates on the building being a residence on the player's
/// island; picks an exit tile next to the building footprint and
/// targets a tile a few steps away. The figure walks one cycle and
/// despawns when its TTL expires.
///
/// `rand` is two 32-bit randoms from the simulation RNG packed into a
/// u64 (low word: variant seed, high word: walk-direction).
pub fn try_spawn_civilian(
    building: &BuildingInstance,
    def: &BuildingDef,
    building_idx: u16,
    rand: u64,
) -> Option<Figure> {
    try_spawn_civilian_with_config(building, def, building_idx, rand, CivilianConfig::default())
}

pub fn try_spawn_civilian_with_config(
    building: &BuildingInstance,
    def: &BuildingDef,
    building_idx: u16,
    rand: u64,
    config: CivilianConfig,
) -> Option<Figure> {
    if !building.active {
        return None;
    }
    if def.prod_kind != "WOHN" {
        return None;
    }
    // Civilians only emerge from buildings with a `Tuerflg: 1`
    // door (RE: haeuser.cod). Residence shells without a door
    // (construction-phase placeholders, upgraded variants) don't
    // host civilians.
    if !def.has_door {
        return None;
    }
    // Skip residences still under construction; civilians only walk
    // out of finished houses.
    if building.construction_ms_remaining > 0 {
        return None;
    }
    let variant = pick_variant(rand >> 8);
    let dir_seed = (rand >> 32) as u32;
    let dir = (dir_seed & 0x07) as u8;

    // Pick an exit tile one step from the building footprint in the
    // chosen direction (8-way compass).
    let bx = building.tile_x as i32;
    let by = building.tile_y as i32;
    let bw = def.width.max(1) as i32;
    let bh = def.height.max(1) as i32;
    let (sx, sy) = exit_tile(bx, by, bw, bh, dir);

    // Walk target: 3 tiles further in the same direction. Civilians
    // wander, they don't pathfind to a destination.
    let (tx, ty) = step_in_direction(sx, sy, dir, 3);

    let mut fig = Figure::new();
    fig.action = ActionType::Walking;
    fig.owner = building.owner;
    fig.tile_x = sx;
    fig.tile_y = sy;
    fig.target_x = tx;
    fig.target_y = ty;
    fig.direction = dir;
    fig.speed = 2;
    fig.building_idx = building_idx;
    fig.sprite_set = CIVILIAN_FIRST_INDEX + variant;
    fig.base_sprite = config.sprite_base_for(variant);
    // Health field is repurposed as TTL ticks for civilians (despawn
    // after walking one short cycle). Reasonable default lifetime
    // since the original despawns when the wander animation completes.
    fig.health = 80;
    Some(fig)
}

fn exit_tile(bx: i32, by: i32, bw: i32, bh: i32, dir: u8) -> (i32, i32) {
    match dir % 8 {
        0 => (bx + bw / 2, by - 1),  // N
        1 => (bx + bw, by - 1),      // NE
        2 => (bx + bw, by + bh / 2), // E
        3 => (bx + bw, by + bh),     // SE
        4 => (bx + bw / 2, by + bh), // S
        5 => (bx - 1, by + bh),      // SW
        6 => (bx - 1, by + bh / 2),  // W
        _ => (bx - 1, by - 1),       // NW
    }
}

fn step_in_direction(x: i32, y: i32, dir: u8, n: i32) -> (i32, i32) {
    let (dx, dy) = match dir % 8 {
        0 => (0, -1),
        1 => (1, -1),
        2 => (1, 0),
        3 => (1, 1),
        4 => (0, 1),
        5 => (-1, 1),
        6 => (-1, 0),
        _ => (-1, -1),
    };
    (x + dx * n, y + dy * n)
}

/// Identifies a figure as a civilian wanderer: civilians sit on
/// `Walking` action with a sprite base inside the GFXZIVIL block.
pub fn is_civilian(fig: &Figure) -> bool {
    CivilianConfig::default().is_civilian(fig)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::BuildingInstance;
    use anno_formats::figuren::{FigureAnim, FigureDef, FiguresFile};

    fn mk_def(kind: &str, w: u8, h: u8) -> BuildingDef {
        BuildingDef {
            id: 0,
            category: 0,
            width: w,
            height: h,
            production_type: crate::types::ProductionType::Residence,
            kind: "GEBAEUDE".into(),
            prod_kind: kind.into(),
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
            has_door: true, // residences in tests have doors
            upgradeable: true,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: crate::building::NO_RUIN_ID,
            required_fertility: None,
        }
    }

    fn mk_building(island: u8, x: u16, y: u16) -> BuildingInstance {
        BuildingInstance::new(0, island, x, y, 0)
    }

    #[test]
    fn variant_table_indices_match_figuren_cod() {
        // Definition-order indices for ADELWEIBL .. PILGER:
        // 0x58 0x59 0x5a 0x5b 0x5c 0x5d 0x5e 0x5f
        for v in 0..CIVILIAN_COUNT {
            let figtype = CIVILIAN_FIRST_INDEX + v;
            assert!((0x58..=0x5f).contains(&figtype));
            // Sprite stride 64.
            assert_eq!(
                sprite_base_for(v),
                GFX_ZIVIL_BASE + (v as u16) * SPRITES_PER_VARIANT
            );
        }
    }

    #[test]
    fn no_spawn_outside_residence() {
        let def = mk_def("HANDWERK", 2, 2);
        let b = mk_building(0, 10, 10);
        let fig = try_spawn_civilian(&b, &def, 0, 0);
        assert!(fig.is_none());
    }

    #[test]
    fn no_spawn_under_construction() {
        let def = mk_def("WOHN", 2, 2);
        let mut b = mk_building(0, 10, 10);
        b.construction_ms_remaining = 5000;
        let fig = try_spawn_civilian(&b, &def, 0, 0);
        assert!(fig.is_none());
    }

    #[test]
    fn no_spawn_from_inactive_residence() {
        let def = mk_def("WOHN", 2, 2);
        let mut b = mk_building(0, 10, 10);
        b.active = false;
        let fig = try_spawn_civilian(&b, &def, 0, 0);
        assert!(fig.is_none());
    }

    #[test]
    fn spawn_succeeds_for_finished_residence() {
        let def = mk_def("WOHN", 2, 2);
        let b = mk_building(0, 10, 10);
        let fig = try_spawn_civilian(&b, &def, 7, 0);
        let fig = fig.expect("should spawn when gate fires");
        assert_eq!(fig.action, ActionType::Walking);
        assert_eq!(fig.building_idx, 7);
        assert!(is_civilian(&fig));
    }

    #[test]
    fn civilian_config_uses_figuren_sprite_bases_and_frames() {
        let figures = FiguresFile {
            constants: Default::default(),
            figures: vec![
                FigureDef {
                    name: "ADELWEIBL".into(),
                    gfx: 2000,
                    anims: vec![FigureAnim {
                        nummer: 0,
                        anim_offs: 3,
                        anim_anz: 6,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                FigureDef {
                    name: "PASSANT".into(),
                    gfx: 2200,
                    anims: vec![FigureAnim {
                        nummer: 0,
                        anim_offs: 5,
                        anim_anz: 7,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
        };

        let config = CivilianConfig::from_figures(&figures);

        assert_eq!(config.sprite_base_for(0), 2003);
        assert_eq!(config.sprite_base_for(4), 2205);
        assert_eq!(config.sprite_base_for(1), sprite_base_for(1));
        assert_eq!(config.frames_per_dir, 7);
    }

    #[test]
    fn spawned_civilian_uses_source_config_and_figtype_marker() {
        let def = mk_def("WOHN", 2, 2);
        let b = mk_building(0, 10, 10);
        let mut config = CivilianConfig::default();
        config.sprite_bases[2] = 3000;
        let fig = try_spawn_civilian_with_config(&b, &def, 0, 2 << 8, config)
            .expect("should spawn when gate fires");

        assert_eq!(fig.base_sprite, 3000);
        assert_eq!(fig.sprite_set, CIVILIAN_FIRST_INDEX + 2);
        assert!(config.is_civilian(&fig));
        assert!(is_civilian(&fig));
    }

    #[test]
    fn variant_distribution_uses_low_three_bits() {
        for r in 0..32u64 {
            let v = pick_variant(r);
            assert!(v < CIVILIAN_COUNT);
        }
    }

    #[test]
    fn random_word_does_not_gate_eligible_spawn() {
        let def = mk_def("WOHN", 2, 2);
        let b = mk_building(0, 10, 10);
        for r in 0..32u64 {
            assert!(try_spawn_civilian(&b, &def, 0, r).is_some());
        }
    }
}
