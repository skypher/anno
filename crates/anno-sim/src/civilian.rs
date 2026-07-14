//! Civilian figure definitions and rendering metadata.
//!
//! `figuren.cod` contains the eight civilian and five worker definitions
//! selected by the source kind-12 allocator. Their sprite layouts and
//! authored walking animation cadences are decoded here.
//! The 4,999 ms per-building command dispatch at
//! `1602_exe.c:84620-84666` is deliberately not used as a spawn rule:
//! its `0x5a` value is a class-`0x39` command subcode handled by
//! `FUN_004766a0`, rather than a call to the figure allocator
//! `FUN_00446ca0`.
//! `FUN_00476350` does invoke `FUN_0044a200` every 5,000 ms, but that
//! allocator samples its own `[0x66, 0x67, 0x66, 0x66, 0x65, 0x66, 0x65,
//! 0x66]` definition table rather than this civilian range. It is therefore
//! not used as a civilian spawn rule.
//!
//! Figure types live in `figuren.cod` at definition-order indices
//! `0x58..=0x64`:
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

/// First definition-order index selected by the source kind-12 allocator.
pub const KIND12_FIRST_INDEX: u8 = CIVILIAN_FIRST_INDEX;
/// Number of consecutive civilian and worker definitions selected by kind 12.
pub const KIND12_FIGURE_COUNT: u8 = 13;
pub const KIND12_FIGURE_NAMES: [&str; KIND12_FIGURE_COUNT as usize] = [
    "ADELWEIBL",
    "ADEL",
    "ALTER",
    "FRAU",
    "PASSANT",
    "VETERAN",
    "KINDREIF",
    "PILGER",
    "MAEHER",
    "STEINKLOPFER",
    "HOLZFAELLER",
    "PFLUECKER",
    "PFLUECKER2",
];

/// Sprite base for civilians inside `TRAEGER.BSH` — `GFXZIVIL` resolves
/// to 1272 by walking the `GFX… = previous + N` chain at the top of
/// `figuren.cod` (GFXTRAEGER 0 → GFXESEL 192 → GFXRAEUBER 320 →
/// GFXKARREN 496 → GFXFLEISCH 688 → GFXARZT 816 → GFXTRADER 880 →
/// GFXEINGEB 1016 → GFXLOESCH 1144 → GFXZIVIL 1272).
pub const GFX_ZIVIL_BASE: u16 = 1272;

/// Sprites per civilian variant: 8 rotations × 8 walking frames
/// (`figuren.cod` ANIM sub-blocks for `ADELWEIBL` and siblings).
pub const SPRITES_PER_VARIANT: u16 = 64;

/// Fallback `AnimSpeed` values for civilian `ANIM 0` walk cycles in the
/// `ADELWEIBL` through `PILGER` definition order.
pub const CIVILIAN_WALK_ANIM_SPEEDS_MS: [u16; CIVILIAN_VARIANT_COUNT] =
    [85, 85, 105, 85, 85, 100, 85, 85];

const KIND12_WALK_ANIM_SPEEDS_MS: [u16; KIND12_FIGURE_COUNT as usize] =
    [85, 85, 105, 85, 85, 100, 85, 85, 85, 85, 85, 85, 85];
const KIND12_MOVEMENT_SPEEDS: [u16; KIND12_FIGURE_COUNT as usize] = [
    200, 230, 160, 200, 220, 200, 250, 200, 220, 220, 220, 220, 220,
];
const KIND12_SPRITE_BASES: [u16; KIND12_FIGURE_COUNT as usize] = [
    GFX_ZIVIL_BASE,
    GFX_ZIVIL_BASE + 64,
    GFX_ZIVIL_BASE + 128,
    GFX_ZIVIL_BASE + 192,
    GFX_ZIVIL_BASE + 256,
    GFX_ZIVIL_BASE + 320,
    GFX_ZIVIL_BASE + 384,
    GFX_ZIVIL_BASE + 448,
    0,
    352,
    224,
    608,
    1120,
];

/// Resolve the kind-12 figure definition selected after `FUN_0044b140`
/// reaches a type-3 route target.
///
/// `FUN_00442a90` initializes all 33 four-entry rows at `DAT_004e857c` to
/// `0x60` (`MAEHER`), then writes the nine rows below. `FUN_0044b140` uses
/// `table[target_kind * 4 + (rand() & 3)]`; a failed route keeps its separate
/// initial `[0x5f, 0x5e, 0x64, 0x5d]` selection instead.
pub const fn source_kind12_definition(target_kind: u8, random: u16) -> u8 {
    let row = match target_kind {
        7 => [0x5f, 0x60, 0x63, 0x5d],
        17 => [0x5f, 0x5e, 0x64, 0x5d],
        18 => [0x5e, 0x5f, 0x60, 0x64],
        19 => [0x5e, 0x61, 0x64, 0x5d],
        20 | 21 => [0x5e, 0x61, 0x60, 0x5d],
        22 => [0x5f, 0x60, 0x62, 0x61],
        23 => [0x60, 0x63, 0x61, 0x5d],
        24 => [0x61, 0x5e, 0x61, 0x60],
        _ => [0x60; 4],
    };
    row[(random & 3) as usize]
}

/// Initial kind-12 figure selection before `FUN_0044b140` either writes a
/// route-specific definition or leaves the source route terminator in place.
pub const fn source_kind12_initial_definition(random: u16) -> u8 {
    [0x5f, 0x5e, 0x64, 0x5d][(random & 3) as usize]
}

/// Return whether the selected `FUN_0044b140` permission branch admits a
/// source map-object kind into its type-3 civilian route grid.
///
/// Every branch admits kind 13. The remaining permissions come from the
/// four stack masks prepared by `FUN_00480370` immediately before it calls
/// `FUN_0044b140`.
pub const fn source_civilian_path_kind_permitted(branch: u8, kind_code: u8) -> bool {
    match kind_code {
        13 => true,
        17 | 22 => branch & 3 == 0,
        7 | 24 => branch & 3 == 1 || branch & 3 == 3 && kind_code == 24,
        18 | 19 | 21 => branch & 3 == 2,
        20 | 23 => branch & 3 == 3,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilianConfig {
    pub sprite_bases: [u16; KIND12_FIGURE_COUNT as usize],
    pub frames_per_dir: u8,
    pub frame_speeds_ms: [u16; KIND12_FIGURE_COUNT as usize],
    pub movement_speeds: [u16; KIND12_FIGURE_COUNT as usize],
}

impl Default for CivilianConfig {
    fn default() -> Self {
        Self {
            sprite_bases: KIND12_SPRITE_BASES,
            frames_per_dir: 8,
            frame_speeds_ms: KIND12_WALK_ANIM_SPEEDS_MS,
            movement_speeds: KIND12_MOVEMENT_SPEEDS,
        }
    }
}

impl CivilianConfig {
    pub fn from_figures(figures: &anno_formats::figuren::FiguresFile) -> Self {
        let mut config = Self::default();
        for (idx, name) in KIND12_FIGURE_NAMES.iter().enumerate() {
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
            if let Some(speed) = def
                .walk_anim()
                .and_then(|anim| u16::try_from(anim.anim_speed).ok())
                .filter(|&speed| speed > 0)
            {
                config.frame_speeds_ms[idx] = speed;
            }
            if let Ok(speed) = u16::try_from(def.speed()) {
                if speed > 0 {
                    config.movement_speeds[idx] = speed;
                }
            }
        }
        config
    }

    /// Resolved BSH sprite base for a source kind-12 definition.
    pub fn sprite_base_for_definition(&self, definition: u8) -> u16 {
        definition
            .checked_sub(KIND12_FIRST_INDEX)
            .filter(|&idx| idx < KIND12_FIGURE_COUNT)
            .map(usize::from)
            .map(|idx| self.sprite_bases[idx])
            .unwrap_or(self.sprite_bases[0])
    }

    pub fn sprite_base_for(&self, variant: u8) -> u16 {
        self.sprite_base_for_definition(CIVILIAN_FIRST_INDEX + variant.min(CIVILIAN_COUNT - 1))
    }

    pub fn frame_speed_for(&self, fig: &Figure) -> u32 {
        let variant = fig
            .sprite_set
            .checked_sub(KIND12_FIRST_INDEX)
            .filter(|&idx| idx < KIND12_FIGURE_COUNT)
            .map(usize::from)
            .or_else(|| {
                self.sprite_bases
                    .iter()
                    .position(|&base| base == fig.base_sprite)
            })
            .unwrap_or(0);
        u32::from(self.frame_speeds_ms[variant].max(1))
    }

    pub fn movement_speed_for(&self, fig: &Figure) -> u16 {
        fig.sprite_set
            .checked_sub(KIND12_FIRST_INDEX)
            .filter(|&idx| idx < KIND12_FIGURE_COUNT)
            .map(usize::from)
            .map(|idx| self.movement_speeds[idx])
            .unwrap_or(KIND12_MOVEMENT_SPEEDS[0])
    }

    pub fn movement_speed_for_definition(&self, definition: u8) -> u16 {
        definition
            .checked_sub(KIND12_FIRST_INDEX)
            .filter(|&idx| idx < KIND12_FIGURE_COUNT)
            .map(usize::from)
            .map(|idx| self.movement_speeds[idx])
            .unwrap_or(KIND12_MOVEMENT_SPEEDS[0])
    }

    pub fn is_civilian(&self, fig: &Figure) -> bool {
        if fig.action != ActionType::Walking {
            return false;
        }
        let has_source_figtype = fig.sprite_set >= CIVILIAN_FIRST_INDEX
            && fig.sprite_set < CIVILIAN_FIRST_INDEX + CIVILIAN_COUNT;
        has_source_figtype || self.sprite_bases[..CIVILIAN_VARIANT_COUNT].contains(&fig.base_sprite)
    }

    /// Kind-12 also selects five plantation-worker figures. They share the
    /// type-3 route state machine but render from `MAEHER.BSH`, not
    /// `TRAEGER.BSH`.
    pub fn is_worker(&self, fig: &Figure) -> bool {
        fig.action == ActionType::Walking
            && (CIVILIAN_FIRST_INDEX + CIVILIAN_COUNT..KIND12_FIRST_INDEX + KIND12_FIGURE_COUNT)
                .contains(&fig.sprite_set)
    }

    pub fn is_kind12(&self, fig: &Figure) -> bool {
        self.is_civilian(fig) || self.is_worker(fig)
    }
}

/// Resolve a civilian variant index (0..8) to the sprite base in
/// `TRAEGER.BSH`.
pub fn sprite_base_for(variant: u8) -> u16 {
    CivilianConfig::default().sprite_base_for(variant)
}

/// Identifies a figure as a civilian: civilians sit on
/// `Walking` action with a sprite base inside the GFXZIVIL block.
pub fn is_civilian(fig: &Figure) -> bool {
    CivilianConfig::default().is_civilian(fig)
}

/// Whether a walking figure uses the source kind-12 route state machine.
pub fn is_kind12(fig: &Figure) -> bool {
    CivilianConfig::default().is_kind12(fig)
}

/// Whether a kind-12 definition renders from `MAEHER.BSH`.
pub const fn source_kind12_is_worker(definition: u8) -> bool {
    definition >= CIVILIAN_FIRST_INDEX + CIVILIAN_COUNT
        && definition < KIND12_FIRST_INDEX + KIND12_FIGURE_COUNT
}

#[cfg(test)]
mod tests {
    use super::*;
    use anno_formats::figuren::{FigureAnim, FigureDef, FiguresFile};

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
    fn civilian_config_uses_figuren_sprite_bases_frames_and_speeds() {
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
                        anim_speed: 91,
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
                        anim_speed: 73,
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
        assert_eq!(config.frame_speeds_ms[0], 91);
        assert_eq!(config.frame_speeds_ms[4], 73);

        let mut figure = Figure::new();
        figure.sprite_set = CIVILIAN_FIRST_INDEX + 4;
        assert_eq!(config.frame_speed_for(&figure), 73);
    }

    #[test]
    fn default_civilian_config_preserves_authored_variant_cadences() {
        assert_eq!(
            CivilianConfig::default().frame_speeds_ms[..CIVILIAN_VARIANT_COUNT],
            [85, 85, 105, 85, 85, 100, 85, 85]
        );
    }

    #[test]
    fn kind12_worker_definitions_keep_their_maeher_bsh_layout() {
        let config = CivilianConfig::default();
        assert_eq!(config.sprite_base_for_definition(0x60), 0);
        assert_eq!(config.sprite_base_for_definition(0x61), 352);
        assert_eq!(config.sprite_base_for_definition(0x62), 224);
        assert_eq!(config.sprite_base_for_definition(0x63), 608);
        assert_eq!(config.sprite_base_for_definition(0x64), 1120);
        assert_eq!(config.movement_speed_for_definition(0x60), 220);
        assert!(source_kind12_is_worker(0x64));
        assert!(!source_kind12_is_worker(0x5f));
    }

    #[test]
    fn civilian_route_permission_masks_match_fun_00480370() {
        let permitted: Vec<_> = (0..4)
            .map(|branch| {
                (0..=24)
                    .filter(|&kind| source_civilian_path_kind_permitted(branch, kind))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(permitted[0], [13, 17, 22]);
        assert_eq!(permitted[1], [7, 13, 24]);
        assert_eq!(permitted[2], [13, 18, 19, 21]);
        assert_eq!(permitted[3], [13, 20, 23, 24]);
    }

    #[test]
    fn kind12_definition_table_matches_fun_00442a90_initializer() {
        assert_eq!(
            (0..4)
                .map(|draw| source_kind12_initial_definition(draw))
                .collect::<Vec<_>>(),
            [0x5f, 0x5e, 0x64, 0x5d]
        );
        assert_eq!(
            (0..4)
                .map(|draw| source_kind12_definition(7, draw))
                .collect::<Vec<_>>(),
            [0x5f, 0x60, 0x63, 0x5d]
        );
        assert_eq!(
            (0..4)
                .map(|draw| source_kind12_definition(22, draw))
                .collect::<Vec<_>>(),
            [0x5f, 0x60, 0x62, 0x61]
        );
        assert_eq!(source_kind12_definition(13, 0), 0x60);
        assert_eq!(source_kind12_definition(31, 3), 0x60);
    }

    #[test]
    fn configured_civilian_uses_source_figtype_marker() {
        let mut config = CivilianConfig::default();
        config.sprite_bases[2] = 3000;
        let mut fig = Figure::new();
        fig.action = ActionType::Walking;
        fig.base_sprite = 3000;
        fig.sprite_set = CIVILIAN_FIRST_INDEX + 2;

        assert_eq!(fig.base_sprite, 3000);
        assert_eq!(fig.sprite_set, CIVILIAN_FIRST_INDEX + 2);
        assert!(config.is_civilian(&fig));
        assert!(is_civilian(&fig));
    }
}
