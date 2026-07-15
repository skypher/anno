//! Live source map-cell animation state.
//!
//! `FUN_00481fc0` allocates a 20-byte record per active command root. The
//! renderer reads its frame selector, activity, storage, and market-progress
//! fields through `FUN_0047cc80`, `FUN_0047ccd0`, and `FUN_0047cd10`.

use anno_formats::cod::{BuildingDef, CodFile};

use crate::building::SourceBuildingCommand;

/// The renderer-relevant subset of one source 20-byte map-cell record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceMapCellState {
    pub island: u8,
    pub x: u8,
    pub y: u8,
    /// Low 13 bits selecting this cell's compiled map definition. `Gfx:`
    /// initializes the root index at definition offset `+0x84`, and
    /// `FUN_00463b10` increments it through the oriented footprint.
    #[serde(default)]
    pub source_definition_offset: u16,
    /// Anchor of the compiled source command which owns this destination
    /// cell. `FUN_00463250` recovers these coordinates from a cell's
    /// definition-offset and orientation before replaying a backing command.
    #[serde(default)]
    pub source_command_anchor_x: u8,
    #[serde(default)]
    pub source_command_anchor_y: u8,
    /// Oriented source-command footprint. `FUN_00463f40` uses these extents
    /// when its type-7 terminal handler replaces or clears the root.
    pub footprint_width: u8,
    pub footprint_height: u8,
    /// Unrotated dimensions held in the compiled map definition. The
    /// terminal handler compares these with the selected ruin definition
    /// before applying the command orientation.
    pub source_definition_width: u8,
    pub source_definition_height: u8,
    /// Low two orientation bits of the original map command. The terminal
    /// writer forwards these bits to the selected ruin replacement.
    pub source_orientation: u8,
    /// Packed frame selector (`bits 15..=18`) from the root command. The
    /// terminal writer forwards it to `FUN_00463ef0` for every replacement.
    pub source_variant: u8,
    /// Packed map-owner selector (`bits 19..=21`) from the root command.
    /// The terminal writer retains it while rewriting the root footprint.
    pub source_map_owner_slot: u8,
    /// Source `Ruinenr` selected by `FUN_00463f40`; `0xff` enters its
    /// per-tile clear branch instead of issuing a ruin replacement command.
    pub ruin_id: u8,
    /// The replacement definition's oriented footprint. The terminal handler
    /// draws once when this matches the destroyed root, otherwise once per
    /// rewritten source cell.
    pub ruin_footprint_width: u8,
    pub ruin_footprint_height: u8,
    /// Source kinds 23 through 27 select the shifted strand-ruin table.
    pub ruin_uses_strand_table: bool,
    /// Per-cell strand-table selectors for `FUN_00463f40`'s mismatched-size
    /// fallback. Bit `dy × width + (width - 1 - dx)` follows the source
    /// right-to-left write order; extracted haeuser.cod footprints occupy at
    /// most 36 cells, so one u64 retains every selector.
    pub fallback_strand_cells: u64,
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
    /// Compiled `Maxenergy` at definition offset `+0x64`. The deferred
    /// category-6 source hit handler compares its map-cell accumulator with
    /// this fixed-point threshold before emitting the terminal type-7 event.
    pub source_damage_threshold: u16,
    /// The live `FUN_0047a650` hit accumulator for this command root. The
    /// executable stores it in a separate eight-byte keyed record; retaining
    /// it beside the identified root preserves the same threshold lifetime.
    #[serde(default)]
    pub source_damage_accumulator: u16,
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
        let state = Self::new_static(island, x, y, definition, phase)?;
        matches!(state.kind_code, 1..=8 | 30).then_some(state)
    }

    /// Construct terminal metadata for any compiled static map root. Unlike
    /// [`Self::new`], this retains kinds outside the selector-state subset so
    /// `FUN_0047a650` can accumulate category-6 damage at every map target.
    pub fn new_static(
        island: u8,
        x: u8,
        y: u8,
        definition: &BuildingDef,
        phase: u8,
    ) -> Option<Self> {
        let kind_code = definition.source_kind_code()?;
        Some(Self {
            island,
            x,
            y,
            source_definition_offset: u16::try_from(definition.gfx).unwrap_or(0),
            source_command_anchor_x: x,
            source_command_anchor_y: y,
            footprint_width: u8::try_from(definition.size.0).unwrap_or(1).max(1),
            footprint_height: u8::try_from(definition.size.1).unwrap_or(1).max(1),
            source_definition_width: u8::try_from(definition.size.0).unwrap_or(1).max(1),
            source_definition_height: u8::try_from(definition.size.1).unwrap_or(1).max(1),
            source_orientation: 0,
            source_variant: 0,
            source_map_owner_slot: 0,
            ruin_id: definition.ruinenr.clamp(0, 255) as u8,
            ruin_footprint_width: 0,
            ruin_footprint_height: 0,
            ruin_uses_strand_table: matches!(kind_code, 23..=27),
            fallback_strand_cells: 0,
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
            source_damage_threshold: definition.source_damage_threshold,
            source_damage_accumulator: 0,
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

    /// Record the oriented extents used by the source map-command writer.
    pub fn set_footprint(&mut self, width: i32, height: i32) {
        self.footprint_width = u8::try_from(width).unwrap_or(1).max(1);
        self.footprint_height = u8::try_from(height).unwrap_or(1).max(1);
    }

    /// Compiled definition index written at one oriented destination cell by
    /// `FUN_00463b10`. `dx` and `dy` are measured in the oriented footprint
    /// held by this state.
    pub fn source_definition_offset_at(self, dx: u8, dy: u8) -> u16 {
        let width = u16::from(self.source_definition_width.max(1));
        let height = u16::from(self.source_definition_height.max(1));
        let dx = u16::from(dx);
        let dy = u16::from(dy);
        let increment = match self.source_orientation & 3 {
            0 => dy * width + dx,
            1 => (height - 1 - dx) * width + dy,
            2 => (height - 1 - dy) * width + (width - 1 - dx),
            _ => dx * width + (width - 1 - dy),
        };
        self.source_definition_offset.wrapping_add(increment)
    }

    /// Retain the low orientation bits passed to the terminal map writer.
    pub fn set_source_orientation(&mut self, orientation: u8) {
        self.source_orientation = orientation & 3;
    }

    /// Retain the packed source command used to seed this root. This is
    /// required when a `Ruinenr = NORUINE` terminal event restores a backing
    /// command to the visible INSELHAUS stream.
    pub fn set_source_command(&mut self, command: SourceBuildingCommand) {
        self.source_definition_offset = command.definition_offset;
        self.source_orientation = command.orientation & 3;
        self.source_variant = command.variant & 0x0f;
        self.source_map_owner_slot = command.map_owner_slot & 7;
    }

    /// Encode the root fields retained by the terminal map writer as an
    /// INSELHAUS command at the supplied anchored position.
    pub fn to_source_island_tile(self) -> anno_formats::szs::IslandTile {
        SourceBuildingCommand {
            definition_offset: self.source_definition_offset,
            orientation: self.source_orientation,
            variant: self.source_variant,
            metadata: 0,
            map_owner_slot: self.source_map_owner_slot,
            random_seed: 0,
            dynamic_object_owner: 0,
        }
        .to_island_tile(self.x, self.y)
    }

    /// Preserve the root fields that `FUN_00463f40` forwards unchanged to
    /// the replacement map and draw writers.
    pub fn set_terminal_command_fields(&mut self, variant: u8, map_owner_slot: u8) {
        self.source_variant = variant & 0x0f;
        self.source_map_owner_slot = map_owner_slot & 7;
    }

    /// Record the source-order strand selectors sampled by the terminal
    /// fallback branch after the scenario's final INSELHAUS overwrite pass.
    pub fn set_fallback_strand_cells(&mut self, selectors: u64) {
        self.fallback_strand_cells = selectors;
    }

    #[inline]
    pub fn fallback_uses_strand_table(self, source_order_index: usize) -> bool {
        source_order_index < u64::BITS as usize
            && (self.fallback_strand_cells & (1_u64 << source_order_index)) != 0
    }

    /// Resolve the terminal handler's ruin table entry from the parsed COD
    /// definitions. `FUN_00463f40` chooses the shifted table only for the
    /// source strand-kind range 23 through 27.
    pub fn configure_terminal_replacement(&mut self, cod: &CodFile) {
        self.ruin_uses_strand_table = matches!(self.kind_code, 23..=27);
        let Some(ruin) = cod.ruin_building(self.ruin_id, self.ruin_uses_strand_table) else {
            return;
        };
        self.ruin_footprint_width = u8::try_from(ruin.size.0).unwrap_or(0);
        self.ruin_footprint_height = u8::try_from(ruin.size.1).unwrap_or(0);
    }

    /// Number of MSVC `rand()` draws consumed by `FUN_00463f40` before it
    /// applies this root's replacement command.
    pub fn source_kind6_terminal_random_draw_count(self) -> usize {
        if self.ruin_id == crate::building::NO_RUIN_ID {
            return 0;
        }
        if self.ruin_footprint_width == self.source_definition_width
            && self.ruin_footprint_height == self.source_definition_height
        {
            return 1;
        }
        usize::from(self.footprint_width) * usize::from(self.footprint_height)
    }

    /// Apply `FUN_0047a650`'s static-map damage arithmetic. Map-root hits
    /// use `floor(51 * raw_strength / 128)`, add it as an unsigned 16-bit
    /// value, and emit the terminal event once the accumulated value reaches
    /// the compiled `Maxenergy * 32` threshold. The source then frees its
    /// keyed accumulator record, represented here by resetting the value.
    pub fn apply_source_kind6_map_hit(&mut self, raw_strength: u16) -> bool {
        let scaled = source_kind6_map_hit_strength(raw_strength);
        self.source_damage_accumulator = self.source_damage_accumulator.wrapping_add(scaled);
        if self.source_damage_accumulator < self.source_damage_threshold {
            return false;
        }
        self.source_damage_accumulator = 0;
        true
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

/// The non-direct branch of `FUN_0047a650`. Category-6 static map targets
/// arrive through this branch; raw-strength direct hits are reserved for the
/// consumer's classification `0x0d` path.
#[inline]
pub fn source_kind6_map_hit_strength(raw_strength: u16) -> u16 {
    ((u32::from(raw_strength) * 51) >> 7) as u16
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
    fn definition_indices_follow_fun_00463b10_oriented_write_order() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            gfx: 100,
            size: (2, 3),
            ..Default::default()
        };
        let mut state = SourceMapCellState::new_static(0, 0, 0, &definition, 0)
            .expect("static source command");

        let cases = [
            (0, 2, 3, [[100, 101, 0], [102, 103, 0], [104, 105, 0]]),
            (1, 3, 2, [[104, 102, 100], [105, 103, 101], [0, 0, 0]]),
            (2, 2, 3, [[105, 104, 0], [103, 102, 0], [101, 100, 0]]),
            (3, 3, 2, [[101, 103, 105], [100, 102, 104], [0, 0, 0]]),
        ];

        for (orientation, width, height, expected) in cases {
            state.set_source_orientation(orientation);
            state.set_footprint(width, height);
            for dy in 0..height {
                for dx in 0..width {
                    assert_eq!(
                        state.source_definition_offset_at(dx, dy),
                        expected[usize::from(dy)][usize::from(dx)],
                        "orientation {orientation}, destination ({dx}, {dy})"
                    );
                }
            }
        }
    }

    #[test]
    fn retains_the_compiled_category_six_damage_threshold() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            source_damage_threshold: 1_600,
            ..Default::default()
        };

        assert_eq!(
            SourceMapCellState::new(1, 2, 3, &definition, 0)
                .unwrap()
                .source_damage_threshold,
            1_600
        );
    }

    #[test]
    fn category_six_map_hits_use_scaled_strength_and_reset_after_threshold() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            source_damage_threshold: 4,
            ..Default::default()
        };
        let mut state = SourceMapCellState::new(1, 2, 3, &definition, 0).unwrap();

        assert_eq!(source_kind6_map_hit_strength(6), 2);
        assert!(!state.apply_source_kind6_map_hit(6));
        assert_eq!(state.source_damage_accumulator, 2);
        assert!(state.apply_source_kind6_map_hit(6));
        assert_eq!(state.source_damage_accumulator, 0);
    }

    #[test]
    fn terminal_replacement_draw_count_follows_source_footprint_branch() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            size: (2, 3),
            ruinenr: 4,
            ..Default::default()
        };
        let mut state = SourceMapCellState::new(1, 2, 3, &definition, 0).unwrap();

        state.set_footprint(3, 2);
        state.ruin_footprint_width = 2;
        state.ruin_footprint_height = 3;
        assert_eq!(state.source_kind6_terminal_random_draw_count(), 1);

        state.ruin_footprint_width = 1;
        state.ruin_footprint_height = 1;
        assert_eq!(state.source_kind6_terminal_random_draw_count(), 6);

        state.ruin_id = crate::building::NO_RUIN_ID;
        assert_eq!(state.source_kind6_terminal_random_draw_count(), 0);
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
