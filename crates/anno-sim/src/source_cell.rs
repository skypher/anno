//! Live source map-cell animation state.
//!
//! `FUN_00481fc0` allocates a 20-byte record per active command root. The
//! renderer reads its frame selector, activity, storage, and market-progress
//! fields through `FUN_0047cc80`, `FUN_0047ccd0`, and `FUN_0047cd10`.

use anno_formats::cod::BuildingDef;

/// The renderer-relevant subset of one source 20-byte map-cell record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceMapCellState {
    pub island: u8,
    pub x: u8,
    pub y: u8,
    /// Low three bits of source byte `+0x03`, scheduled by `FUN_0047daf0`.
    pub phase: u8,
    /// High four bits of source byte `+0x03`.
    pub frame_selector: u8,
    /// Source byte `+0x0e`, the current 0..=128 activity ratio.
    pub activity: u8,
    /// Source u16 `+0x08`, consumed by compiled `Workmenge`.
    pub work_material_stock: u16,
    /// Source u16 `+0x0a`, consumed by compiled `Rohmenge`.
    pub raw_material_stock: u16,
    /// Source u16 `+0x0c`, used by the `LagAniFlg` selector path.
    pub storage_fill: u16,
    /// Source u16 `+0x06`, reserved by `FUN_0047d810` while an in-flight
    /// type-8 or type-11 transfer figure travels to collect this root's
    /// output.
    pub reserved_storage: u16,
    /// Compiled definition `Maxlager` at offset `+0x30`, in the source
    /// record's 1/32-unit storage scale.
    pub storage_animation_capacity: u16,
    /// Compiled `Prodmenge` at definition offset `+0x24`.
    pub source_production_amount: u16,
    /// Compiled `Rohmenge` at definition offset `+0x26`.
    pub source_raw_material_amount: u16,
    /// Compiled `Workmenge` at definition offset `+0x28`.
    pub source_work_material_amount: u16,
    /// Source u16 `+0x10`, advanced by the map-cell scheduler and selected
    /// transfers; kind 7 (`MARKT`) renders this accumulator.
    pub progress: u16,
    /// Compiled `AnimFrame` retained for `FUN_004638c0` transitions.
    pub animation_frame: i32,
    /// Compiled `AnimAnz` retained for `FUN_004638c0` transitions.
    pub animation_count: i32,
    /// Compiled `Anicontflg` retained for `FUN_004638c0` transitions.
    pub animation_continues: bool,
    /// Compiled source kind code, recorded for renderer dispatch.
    pub kind_code: u8,
}

impl SourceMapCellState {
    /// Construct the selector-bearing subset of a zeroed
    /// `FUN_00481fc0` map-cell record.
    pub fn new(island: u8, x: u8, y: u8, definition: &BuildingDef, phase: u8) -> Option<Self> {
        let kind_code = definition.source_kind_code()?;
        if !matches!(kind_code, 1..=8 | 30) {
            return None;
        }
        Some(Self {
            island,
            x,
            y,
            phase: phase & 7,
            frame_selector: 0,
            activity: 0,
            work_material_stock: 0,
            raw_material_stock: 0,
            storage_fill: 0,
            reserved_storage: 0,
            storage_animation_capacity: definition.storage_animation_capacity,
            source_production_amount: definition.source_production_amount,
            source_raw_material_amount: definition.source_raw_material_amount,
            source_work_material_amount: definition.source_work_material_amount,
            progress: 0,
            animation_frame: definition.anim_frame,
            animation_count: definition.anim_anz,
            animation_continues: definition.animation_continues,
            kind_code,
        })
    }

    #[inline]
    pub fn matches(self, island: u8, x: u16, y: u16) -> bool {
        self.island == island && u16::from(self.x) == x && u16::from(self.y) == y
    }

    /// Reserve fixed-point output for a type-8 carrier. `FUN_0047d810`
    /// records this separately from the source's live `+0x0c` stock.
    pub fn reserve_storage(&mut self, amount: u16) -> bool {
        if amount == 0 || self.storage_fill.saturating_sub(self.reserved_storage) < amount {
            return false;
        }
        self.reserved_storage = self.reserved_storage.saturating_add(amount);
        true
    }

    /// Complete the supplier-side half of a reserved transfer.
    pub fn collect_reserved_storage(&mut self, amount: u16) -> bool {
        if amount == 0 || self.reserved_storage < amount || self.storage_fill < amount {
            return false;
        }
        self.reserved_storage -= amount;
        self.storage_fill -= amount;
        true
    }

    /// Undo a reservation when the figure cannot complete its pickup leg.
    pub fn release_storage_reservation(&mut self, amount: u16) {
        self.reserved_storage = self.reserved_storage.saturating_sub(amount);
    }

    /// `FUN_0047d910`'s source-storage ratio used by the type-11
    /// `FUN_004717b0` selector. This is deliberately independent of the
    /// type-8 supplier wave in `FUN_00471380`.
    pub fn storage_fill_score(self) -> Option<u32> {
        (self.storage_animation_capacity != 0).then_some(
            (u32::from(self.storage_fill) << 7) / u32::from(self.storage_animation_capacity),
        )
    }

    /// Apply the activity-edge rule from `FUN_0047daf0` / `FUN_004638c0`.
    /// The source map renderer adds `AnimFrame` while `activity != 0`.
    pub fn set_activity(&mut self, activity: u8) {
        let was_active = self.activity != 0;
        let is_active = activity != 0;
        if was_active != is_active && self.animation_count > 1 {
            self.frame_selector = if is_active {
                (i32::from(self.frame_selector) - self.animation_frame).rem_euclid(16) as u8
            } else if self.animation_continues {
                (i32::from(self.frame_selector) + self.animation_frame).rem_euclid(16) as u8
            } else {
                0
            };
        }
        self.activity = activity;
    }

    /// Selector returned by `FUN_0047cd10` for kinds 1 through 6.
    pub fn activity_frame_selector(self, animation_count: i32) -> i32 {
        let selector = i32::from(self.frame_selector);
        if self.activity != 0 {
            (selector + self.animation_frame).rem_euclid(animation_count)
        } else {
            selector.rem_euclid(animation_count)
        }
    }

    /// Selector returned by `FUN_0047ccd0` for kind 7.
    pub fn market_frame_selector(self, animation_count: i32) -> i32 {
        (((animation_count - 1) * i32::from(self.progress) + 0x100) >> 9).min(animation_count - 1)
    }

    /// Selector returned by `FUN_0047cc80` for `LagAniFlg` map cells.
    pub fn storage_frame_selector(self, animation_count: i32) -> i32 {
        if self.storage_animation_capacity == 0 {
            return 0;
        }
        ((animation_count - 1) * i32::from(self.storage_fill)
            + i32::from(self.storage_animation_capacity >> 1))
            / i32::from(self.storage_animation_capacity)
    }

    /// Apply the kind-7 branch of `FUN_0047d940`: a completed transfer to a
    /// MARKT command root advances its selector accumulator by the accepted
    /// source-unit amount.
    pub fn accept_market_transfer(&mut self, amount: u16) -> bool {
        if self.kind_code != 7 || amount == 0 {
            return false;
        }
        self.progress = self.progress.wrapping_add(amount);
        true
    }

    /// Apply the stock and progress arithmetic in `FUN_0047daf0` after its
    /// input-kind and stock predicates select `activity`. The source uses
    /// 1/32-unit amounts and only works at ratios at least 64/128.
    pub fn advance_source_scheduler(&mut self, activity: u8) {
        let activity = activity.min(128);
        let activity = if (self.storage_animation_capacity != 0
            && self.storage_fill >= self.storage_animation_capacity)
            || activity < 64
        {
            0
        } else {
            activity
        };
        if activity != 0 {
            self.work_material_stock = self
                .work_material_stock
                .wrapping_sub(scaled_amount(self.source_work_material_amount, activity));
            self.raw_material_stock = self
                .raw_material_stock
                .wrapping_sub(scaled_amount(self.source_raw_material_amount, activity));
            let output = scaled_amount(self.source_production_amount, activity);
            self.storage_fill = self.storage_fill.wrapping_add(output);
            self.progress = self.progress.wrapping_add(output);
        }
        self.set_activity(activity);
    }
}

#[inline]
fn scaled_amount(amount: u16, activity: u8) -> u16 {
    ((u32::from(amount) * u32::from(activity)) >> 7) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition() -> BuildingDef {
        BuildingDef {
            kind: "HANDWERK".into(),
            anim_anz: 4,
            anim_frame: 3,
            ..Default::default()
        }
    }

    #[test]
    fn activity_edges_follow_fun_004638c0() {
        let mut state = SourceMapCellState::new(1, 2, 3, &definition(), 6).unwrap();
        state.set_activity(128);
        assert_eq!(state.frame_selector, 13);
        assert_eq!(state.activity_frame_selector(4), 0);
        state.set_activity(0);
        assert_eq!(state.frame_selector, 0);
    }

    #[test]
    fn continuous_animation_retains_phase_when_activity_stops() {
        let mut definition = definition();
        definition.animation_continues = true;
        let mut state = SourceMapCellState::new(1, 2, 3, &definition, 0).unwrap();
        state.set_activity(128);
        state.set_activity(0);
        assert_eq!(state.frame_selector, 0);
    }

    #[test]
    fn kind_seven_progress_uses_source_fixed_point_selector() {
        let state = SourceMapCellState {
            progress: 512,
            ..SourceMapCellState::new(
                0,
                0,
                0,
                &BuildingDef {
                    kind: "MARKT".into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        };
        assert_eq!(state.market_frame_selector(3), 2);
    }

    #[test]
    fn storage_reservation_tracks_fun_0047d810_before_collection() {
        let state = SourceMapCellState {
            storage_fill: 160,
            ..SourceMapCellState::new(
                0,
                3,
                5,
                &BuildingDef {
                    kind: "HANDWERK".into(),
                    storage_animation_capacity: 320,
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        };

        let mut state = state;
        assert!(state.reserve_storage(128));
        assert_eq!(state.reserved_storage, 128);
        assert!(state.collect_reserved_storage(128));
        assert_eq!(state.storage_fill, 32);
        assert_eq!(state.reserved_storage, 0);
    }

    #[test]
    fn storage_fill_score_matches_fun_0047d910() {
        let state = SourceMapCellState {
            storage_fill: 160,
            ..SourceMapCellState::new(
                0,
                3,
                5,
                &BuildingDef {
                    kind: "HANDWERK".into(),
                    storage_animation_capacity: 320,
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        };

        assert_eq!(state.storage_fill_score(), Some(64));
    }

    #[test]
    fn market_transfer_advances_only_kind_seven_source_progress() {
        let mut market = SourceMapCellState::new(
            0,
            0,
            0,
            &BuildingDef {
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut workshop = SourceMapCellState::new(
            0,
            1,
            0,
            &BuildingDef {
                kind: "HANDWERK".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();

        assert!(market.accept_market_transfer(32));
        assert_eq!(market.progress, 32);
        assert!(!workshop.accept_market_transfer(32));
        assert_eq!(workshop.progress, 0);
    }

    #[test]
    fn storage_fill_uses_source_maxlager_scale_and_rounding() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            storage_animation: true,
            storage_animation_capacity: 160,
            ..Default::default()
        };
        let state = SourceMapCellState {
            storage_fill: 80,
            ..SourceMapCellState::new(0, 0, 0, &definition, 0).unwrap()
        };
        assert_eq!(state.storage_frame_selector(4), 2);
    }

    #[test]
    fn scheduler_uses_source_fixed_point_stock_and_progress_updates() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            storage_animation_capacity: 320,
            source_production_amount: 32,
            source_raw_material_amount: 64,
            source_work_material_amount: 32,
            ..Default::default()
        };
        let mut state = SourceMapCellState {
            raw_material_stock: 64,
            work_material_stock: 16,
            ..SourceMapCellState::new(0, 0, 0, &definition, 0).unwrap()
        };

        state.advance_source_scheduler(64);

        assert_eq!(state.activity, 64);
        assert_eq!(state.raw_material_stock, 32);
        assert_eq!(state.work_material_stock, 0);
        assert_eq!(state.storage_fill, 16);
        assert_eq!(state.progress, 16);
    }

    #[test]
    fn scheduler_idles_below_source_half_activity_threshold() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            source_production_amount: 32,
            source_raw_material_amount: 64,
            ..Default::default()
        };
        let mut state = SourceMapCellState::new(0, 0, 0, &definition, 0).unwrap();

        state.advance_source_scheduler(63);

        assert_eq!(state.activity, 0);
        assert_eq!(state.storage_fill, 0);
        assert_eq!(state.progress, 0);
    }
}
