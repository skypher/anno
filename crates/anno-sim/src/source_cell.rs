//! Live source map-cell animation state.
//!
//! `FUN_00481fc0` allocates a 20-byte record per active command root. The
//! renderer reads its frame selector, activity, storage, and market-progress
//! fields through `FUN_0047cc80`, `FUN_0047ccd0`, and `FUN_0047cd10`.

use anno_formats::cod::{BuildingDef, CodFile};

use crate::building::SourceBuildingCommand;

/// Compiled `Figurnr` values used by type-11 city transfer roots.
///
/// `FUN_0044ad50` forwards this definition selector to `FUN_00446ca0` when
/// it allocates the generic transfer figure. The extracted city roots use
/// `KARREN` for MARKT and ordinary KONTOR definitions, and `TRAEGER2` for
/// the two native KONTOR definitions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum SourceTransferFigure {
    #[default]
    Unknown = 0,
    Karren = 1,
    Traeger2 = 2,
}

/// Input selector passed by production-kind case 1 to `FUN_0044ab60`.
/// The allocator receives the compiled `Rohstoff` selector for `RawMaterial`
/// and `Workstoff` for `WorkMaterial`; figure-8 later deposits its cargo into
/// the matching source buffer through `FUN_0047d940`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType8TransferInput {
    RawMaterial,
    WorkMaterial,
}

/// Definition selected by `FUN_0047c830` when a type-12 plantation worker
/// harvests a raw-resource map cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceResourceHarvestTransition {
    /// The preceding compiled definition record (`-0x88`), normally the
    /// resource's `ROHSTWACHS` entry.
    Regrowth,
    /// The following compiled definition record (`+0x88`), normally the
    /// authored dry-resource entry.
    Drought,
}

const SOURCE_RESOURCE_GROWTH_MASKS: [[bool; 32]; 5] = [
    [
        false, false, true, false, false, false, false, false, false, false, false, false, false,
        false, false, false, false, false, false, false, false, false, true, false, false, false,
        false, false, false, false, false, false,
    ],
    [
        false, false, true, false, false, false, true, true, false, false, true, false, false,
        false, true, false, false, false, false, true, false, true, false, false, true, true,
        false, false, false, true, false, false,
    ],
    [
        false, true, true, false, true, false, false, true, true, true, false, true, false, false,
        true, false, false, true, false, false, true, true, false, true, false, false, true, true,
        false, true, false, true,
    ],
    [
        true, false, true, true, true, false, true, true, false, true, true, false, true, false,
        true, true, true, true, true, false, true, true, true, false, true, true, true, false,
        true, false, true, true,
    ],
    [true; 32],
];

const fn source_scheduler_enabled_default() -> bool {
    true
}

/// `FUN_0046f920` admits these outer map kinds without comparing their owner
/// or compiled `Ware` selector. All remaining kinds need both fields to match
/// the plantation worker's requested raw resource.
pub const fn source_plantation_path_kind_always_walkable(kind_code: u8) -> bool {
    matches!(kind_code, 1 | 11 | 12 | 13 | 18 | 29 | 30)
}

impl SourceTransferFigure {
    fn from_definition(definition: &BuildingDef) -> Self {
        match definition.properties.get("Figurnr").map(String::as_str) {
            Some("KARREN") => Self::Karren,
            Some("TRAEGER2") => Self::Traeger2,
            _ => Self::Unknown,
        }
    }
}

/// The renderer-relevant subset of one source 20-byte map-cell record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Compiled `Figuranz` at definition offset `+0x3e`. `FUN_0044ad50`
    /// admits this many type-11 transfer figures at this source root.
    #[serde(default)]
    pub source_transfer_figure_limit: u8,
    /// Compiled `Radius` at definition offset `+0x20`. Type-8/type-11 event
    /// routing uses this square search radius in `FUN_0045c8b0`.
    #[serde(default)]
    pub source_transfer_radius: u8,
    /// Compiled `Figurnr` passed by `FUN_0044ad50` to the generic-figure
    /// constructor. It selects the cart's authored capacity, speed, sprite,
    /// and animation layout.
    #[serde(default)]
    pub source_transfer_figure: SourceTransferFigure,
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
    /// Bit 3 of source byte `+0x03`. `FUN_00481fc0` initializes a new
    /// root with this bit set; `FUN_0047daf0` does not run production or
    /// transfer dispatch while it is clear.
    #[serde(default = "source_scheduler_enabled_default")]
    pub scheduler_enabled: bool,
    /// Source u16 `+0x04`. After a phase transition `FUN_0047daf0` decrements
    /// this timer and runs the root only on the transition that reaches zero.
    #[serde(default)]
    pub scheduler_cooldown: u16,
    /// Bit 7 of source byte `+0x0f`. The map-state transition path clears
    /// activity when it changes this bit; `FUN_0047daf0` excludes a blocked
    /// root from production while preserving its transfer handling.
    #[serde(default)]
    pub scheduler_blocked: bool,
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
    /// Compiled `Maxnorohst` at definition offset `+0x36`.
    #[serde(default)]
    pub source_max_no_raw_material_count: u16,
    /// Compiled `Interval` at definition offset `+0x3a`, measured in source
    /// map-scheduler phase transitions.
    #[serde(default)]
    pub source_scheduler_interval: u16,
    /// Low four bits of source byte `+0x0f`. `FUN_0047daf0` increments this
    /// counter while a non-full root cannot produce, saturating at 15, and
    /// clears it after a productive scheduler run.
    #[serde(default)]
    pub source_no_raw_material_count: u8,
    /// Compiled `Ware` selector at definition offset `+0x21`. A zero selector
    /// prevents `FUN_0047daf0` from deriving a nonzero production activity.
    #[serde(default)]
    pub source_output_ware_slot: u8,
    /// Compiled `Rohstoff` selector at definition offset `+0x22`. Case 2 of
    /// `FUN_0047daf0` gives this raw-resource ware to `FUN_0044b7e0`.
    #[serde(default)]
    pub source_raw_resource_ware_slot: u8,
    /// Compiled definition byte `+0xa9` for `ROHSTWACHS` records. The source
    /// uses this associated resource selector instead of their `NOWARE` byte
    /// when evaluating `FUN_0046f920` traversal and `FUN_004684a0` regrowth.
    #[serde(default)]
    pub source_growth_resource_ware_slot: u8,
    /// `Doerrflg` on a production-kind-10 resource definition. `FUN_0047c920`
    /// returns only these dry cells to their adjacent `ROHSTOFF` record;
    /// ordinary `ROHSTWACHS` cells remain in their authored regrowth state.
    #[serde(default)]
    pub source_resource_is_dry: bool,
    /// Compiled worker definition passed to `FUN_00446ca0` by
    /// `FUN_0044b7e0`: MAEHER through PFLUECKER2 occupy 0x60..=0x64.
    #[serde(default)]
    pub source_plantation_worker_definition: u8,
    /// The transient `0x20000000` map bit set after `FUN_0046f920` selects a
    /// raw-resource cell and cleared by `FUN_0047c830` on harvest.
    #[serde(default)]
    pub source_resource_reserved: bool,
    /// Low seven bits copied by `FUN_0046f920` from compiled
    /// `Wegspeed[0]`; the selector sets bit 7 while the cell is a candidate
    /// target in the source weighted path grid.
    #[serde(default)]
    pub source_path_class: u8,
    /// Compiled `Randwachs` at definition offset `+0x40`, in the source's
    /// 128-scale. `FUN_004684a0` combines it with the island's live resource
    /// strength when `FUN_0047c830` replaces a harvested raw-resource cell.
    #[serde(default)]
    pub source_resource_growth_factor: u8,
    /// Compiled `Maxenergy` at definition offset `+0x64`. The deferred
    /// category-6 source hit handler compares its map-cell accumulator with
    /// this fixed-point threshold before emitting the terminal type-7 event.
    pub source_damage_threshold: u16,
    /// The live `FUN_0047a650` hit accumulator for this command root. The
    /// executable stores it in a separate eight-byte keyed record; retaining
    /// it beside the identified root preserves the same threshold lifetime.
    #[serde(default)]
    pub source_damage_accumulator: u16,
    /// Source u16 `+0x10`, the produced-stock accumulator advanced by the
    /// map-cell scheduler. Kind 7 (`MARKT`) also receives completed transfer
    /// amounts into this field.
    pub progress: u16,
    /// Source u16 `+0x12`, the production-time accumulator. Each scheduler
    /// run adds its selected interval, and `FUN_0047d940` subtracts the
    /// current scheduler cooldown when an idle ordinary source receives a
    /// completed delivery, including a zero-amount plantation worker return.
    #[serde(default)]
    pub source_production_time: u16,
    /// Compiled `AnimFrame` retained for `FUN_004638c0` transitions.
    pub animation_frame: i32,
    /// Compiled `AnimAnz` retained for `FUN_004638c0` transitions.
    pub animation_count: i32,
    /// Compiled `Anicontflg` retained for `FUN_004638c0` transitions.
    pub animation_continues: bool,
    /// Compiled source kind code, recorded for renderer dispatch.
    pub kind_code: u8,
    /// Compiled nested `HAUS_PRODTYP Kind` at definition offset `+0x1c`.
    /// `FUN_0047e1f0` switches on this selector before invoking the generic
    /// type-11 allocator, independently of the outer map kind above.
    #[serde(default)]
    pub source_production_kind_code: u8,
}

impl SourceMapCellState {
    /// `FUN_0046f920`'s path-grid admission predicate for an already decoded
    /// static map cell. A fixed terrain kind remains traversable across every
    /// owner; all other cells require both the map owner and compiled `Ware`
    /// selector to match the worker's root.
    pub const fn admits_plantation_worker_path(
        self,
        source_owner: u8,
        raw_resource_ware_slot: u8,
    ) -> bool {
        source_plantation_path_kind_always_walkable(self.kind_code)
            || (self.source_map_owner_slot == source_owner
                && self.plantation_path_resource_ware_slot() == raw_resource_ware_slot)
    }

    /// `FUN_0046f920` writes the metadata high bit for an admitted cell whose
    /// ordinary compiled `Ware` matches the selected raw resource and whose
    /// live static-map bit is not reserved by another worker.
    pub const fn is_plantation_worker_target(
        self,
        source_owner: u8,
        raw_resource_ware_slot: u8,
    ) -> bool {
        self.admits_plantation_worker_path(source_owner, raw_resource_ware_slot)
            && self.source_production_kind_code != 10
            && self.source_output_ware_slot == raw_resource_ware_slot
            && !self.source_resource_reserved
    }

    /// `FUN_0046f920` reads `+0xa9` for production kind 10 and the ordinary
    /// `Ware` byte for every other map definition.
    pub const fn plantation_path_resource_ware_slot(self) -> u8 {
        if self.source_production_kind_code == 10 {
            self.source_growth_resource_ware_slot
        } else {
            self.source_output_ware_slot
        }
    }

    /// Construct the source roots that own scheduler or transfer dispatch.
    /// Besides the outer selector kinds retained by `FUN_00481fc0`, this
    /// includes nested production-kind-2 plantations, whose worker event is
    /// allocated through `FUN_0044b7e0` from the source command root.
    pub fn new(island: u8, x: u8, y: u8, definition: &BuildingDef, phase: u8) -> Option<Self> {
        let state = Self::new_static(island, x, y, definition, phase)?;
        (matches!(state.kind_code, 1..=8 | 30)
            || state.is_type11_transfer_root()
            || state.is_type12_plantation_root())
        .then_some(state)
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
        let source_production_kind_code =
            definition
                .source_production_kind_code()
                .unwrap_or(match definition.kind.as_str() {
                    // Unit fixtures and hand-built fallback roots may use the
                    // production label as the outer kind. Authored city roots
                    // carry the nested `ProdKind` above; unrelated outer kinds,
                    // including PIER, retain the compiled default zero.
                    "MARKT" => 7,
                    "KONTOR" => 8,
                    _ => 0,
                });
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
            source_transfer_figure_limit: definition.source_transfer_figure_limit,
            source_transfer_radius: definition.source_transfer_radius,
            source_transfer_figure: SourceTransferFigure::from_definition(definition),
            ruin_id: definition.ruinenr.clamp(0, 255) as u8,
            ruin_footprint_width: 0,
            ruin_footprint_height: 0,
            ruin_uses_strand_table: matches!(kind_code, 23..=27),
            fallback_strand_cells: 0,
            phase: phase & 7,
            scheduler_enabled: true,
            scheduler_cooldown: 0,
            scheduler_blocked: false,
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
            source_max_no_raw_material_count: definition.source_max_no_raw_material_count,
            source_scheduler_interval: definition.source_scheduler_interval,
            source_no_raw_material_count: 0,
            source_output_ware_slot: definition.source_ware_slot().unwrap_or_default(),
            source_raw_resource_ware_slot: definition
                .source_raw_resource_ware_slot()
                .unwrap_or_default(),
            source_growth_resource_ware_slot: 0,
            source_resource_is_dry: definition
                .properties
                .get("Doerrflg")
                .is_some_and(|value| value == "1"),
            source_plantation_worker_definition: definition
                .source_plantation_worker_definition()
                .unwrap_or_default(),
            source_resource_reserved: false,
            source_path_class: definition
                .source_path_classes()
                .map(|classes| classes[0])
                .unwrap_or_default(),
            source_resource_growth_factor: definition.source_resource_growth_factor,
            source_damage_threshold: definition.source_damage_threshold,
            source_damage_accumulator: 0,
            progress: 0,
            source_production_time: 0,
            animation_frame: definition.anim_frame,
            animation_count: definition.anim_anz,
            animation_continues: definition.animation_continues,
            kind_code,
            source_production_kind_code,
        })
    }

    /// `FUN_0047e1f0` calls `FUN_0044ad50` for these compiled production
    /// kinds. The extracted city roots use 7 (`MARKT`) and 8 (`KONTOR`);
    /// retaining 30 follows the executable switch for future authored data.
    #[inline]
    pub const fn is_type11_transfer_root(self) -> bool {
        matches!(self.source_production_kind_code, 7 | 8 | 30)
    }

    /// `FUN_0047daf0` dispatches the generic figure-8 transfer only through
    /// production-kind case 1, which calls `FUN_0044ab60`.
    #[inline]
    pub const fn is_type8_transfer_root(self) -> bool {
        self.source_production_kind_code == 1
    }

    /// Production kind 2 reaches `FUN_0044b7e0` only after the scheduler
    /// leaves its activity byte at zero. The outer map kind determines the
    /// command footprint, while the nested kind selects this worker branch.
    /// The allocator needs both authored selectors; raw-resource definitions
    /// themselves carry no worker ID.
    #[inline]
    pub const fn is_type12_plantation_root(self) -> bool {
        self.source_production_kind_code == 2
            && self.activity == 0
            && self.source_raw_resource_ware_slot != 0
            && self.source_plantation_worker_definition != 0
    }

    /// Model the state written by `FUN_0047c830` after a type-12 worker
    /// harvests this kind-9 cell. Both adjacent authored records inherit the
    /// preceding `ROHSTWACHS` definition's `WALD` map kind and `NOWARE`
    /// selector; only their definition index distinguishes regrowth from dry.
    pub fn replace_harvested_raw_resource(
        &mut self,
        transition: SourceResourceHarvestTransition,
    ) -> bool {
        if self.kind_code != 9 {
            self.source_resource_reserved = false;
            return false;
        }
        self.source_definition_offset = match transition {
            SourceResourceHarvestTransition::Regrowth => {
                self.source_definition_offset.saturating_sub(1)
            }
            SourceResourceHarvestTransition::Drought => {
                self.source_definition_offset.saturating_add(1)
            }
        };
        self.kind_code = 10;
        self.source_production_kind_code = 10;
        self.source_growth_resource_ware_slot = self.source_output_ware_slot;
        self.source_resource_is_dry = transition == SourceResourceHarvestTransition::Drought;
        self.source_output_ware_slot = 0;
        self.source_raw_resource_ware_slot = 0;
        self.source_plantation_worker_definition = 0;
        self.source_resource_reserved = false;
        true
    }

    /// The raw-resource half of `FUN_0047c920`: an unclaimed kind-9 cell
    /// moves one authored definition forward only when `FUN_004684a0` selects
    /// the drought branch.
    pub fn advance_raw_resource_to_drought(
        &mut self,
        transition: SourceResourceHarvestTransition,
    ) -> bool {
        if self.kind_code != 9
            || self.source_resource_reserved
            || transition != SourceResourceHarvestTransition::Drought
        {
            return false;
        }
        self.source_definition_offset = self.source_definition_offset.saturating_add(1);
        self.kind_code = 10;
        self.source_production_kind_code = 10;
        self.source_growth_resource_ware_slot = self.source_output_ware_slot;
        self.source_resource_is_dry = true;
        self.source_output_ware_slot = 0;
        self.source_raw_resource_ware_slot = 0;
        self.source_plantation_worker_definition = 0;
        true
    }

    /// The dry-resource half of `FUN_0047c920`: the preceding authored raw
    /// definition is restored when the source growth mask selects regrowth.
    pub fn restore_dry_resource(&mut self, transition: SourceResourceHarvestTransition) -> bool {
        if self.kind_code != 10
            || !self.source_resource_is_dry
            || transition != SourceResourceHarvestTransition::Regrowth
        {
            return false;
        }
        self.source_definition_offset = self.source_definition_offset.saturating_sub(1);
        self.kind_code = 9;
        self.source_production_kind_code = 9;
        self.source_output_ware_slot = self.source_growth_resource_ware_slot;
        self.source_resource_is_dry = false;
        true
    }

    /// The scheduler reaches its transfer switch only when its newly written
    /// activity is nonzero or its storage has room below `Maxlager`.
    #[inline]
    pub const fn allows_source_transfer_dispatch(self) -> bool {
        self.activity != 0 || self.storage_fill < self.storage_animation_capacity
    }

    /// Decode case 1 of `FUN_0047daf0`'s figure-8 transfer switch. The
    /// executable compares the raw and work buffers in 128-scaled units,
    /// without capping their ratios, and admits a selected input through the
    /// inclusive 256 boundary.
    pub fn source_type8_transfer_input(self) -> Option<SourceType8TransferInput> {
        if !self.is_type8_transfer_root() || !self.allows_source_transfer_dispatch() {
            return None;
        }

        let work_ratio = if self.source_work_material_amount == 0 {
            0x180
        } else {
            (u32::from(self.work_material_stock) << 7) / u32::from(self.source_work_material_amount)
        };
        if self.source_raw_material_amount == 0 {
            return (work_ratio <= 0x100).then_some(SourceType8TransferInput::WorkMaterial);
        }

        let raw_ratio =
            (u32::from(self.raw_material_stock) << 7) / u32::from(self.source_raw_material_amount);
        if raw_ratio > 0x100 || work_ratio < raw_ratio {
            (work_ratio <= 0x100).then_some(SourceType8TransferInput::WorkMaterial)
        } else {
            Some(SourceType8TransferInput::RawMaterial)
        }
    }

    /// Apply `FUN_0047daf0`'s per-root phase and cooldown gate. The phase
    /// write precedes both predicates in the executable, so disabled and
    /// cooling roots retain the current global phase without running work.
    pub fn source_scheduler_due(&mut self, global_phase: u8) -> bool {
        let global_phase = global_phase & 7;
        if self.phase == global_phase {
            return false;
        }
        self.phase = global_phase;
        if !self.scheduler_enabled {
            return false;
        }
        if self.scheduler_cooldown == 0 {
            return true;
        }
        self.scheduler_cooldown -= 1;
        self.scheduler_cooldown == 0
    }

    /// Complete one executed `FUN_0047daf0` root branch. Idle and
    /// storage-blocked roots receive its fixed 11-phase retry; an active root
    /// reloads `(Interval × activity) / 128` from definition offset `+0x3a`.
    /// The executable computes this with the MSVC signed-divide-by-128
    /// idiom `(x + (x >> 0x1f & 0x7f)) >> 7` (`1602_exe.c:89930`), which
    /// truncates toward zero, i.e. floors for the always-nonnegative
    /// `Interval × activity`. The root records that interval in source u16
    /// `+0x12`; once
    /// the accumulator exceeds 239, both it and the produced-stock `+0x10`
    /// are halved.
    pub fn complete_source_scheduler_run(&mut self) {
        if self.activity != 0 {
            self.source_no_raw_material_count = 0;
        } else if self.storage_fill < self.storage_animation_capacity {
            self.source_no_raw_material_count =
                self.source_no_raw_material_count.saturating_add(1).min(15);
        }
        self.scheduler_cooldown = if self.activity < 64 {
            11
        } else {
            ((u32::from(self.source_scheduler_interval) * u32::from(self.activity)) >> 7) as u16
        };
        self.source_production_time = self
            .source_production_time
            .wrapping_add(self.scheduler_cooldown);
        if self.source_production_time > 0xef {
            self.source_production_time >>= 1;
            self.progress >>= 1;
        }
    }

    /// Apply the source byte `+0x0f` transition used by the map-state
    /// handler: the first transition from an unblocked root clears its
    /// activity before setting the blocked bit.
    pub fn block_source_scheduler(&mut self) {
        if !self.scheduler_blocked {
            self.set_activity(0);
        }
        self.scheduler_blocked = true;
    }

    /// `FUN_0047ce60` clears bit 7 after it removes the corresponding
    /// blocked dynamic record; it does not alter the root activity byte.
    pub fn unblock_source_scheduler(&mut self) {
        self.scheduler_blocked = false;
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

    /// Reserve fixed-point output for a type-8 or type-11 carrier.
    /// `FUN_0047d810` records this separately from the source's live `+0x0c`
    /// stock.
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

    /// Model `FUN_0047d640` for a type-11 supplier arrival. The figure first
    /// consumes its reservation, then takes any source output produced after
    /// dispatch until its authored `Maxtrag` capacity is full. The return is
    /// the additional fixed-point cargo to add to the event amount.
    pub fn collect_reserved_storage_with_top_up(
        &mut self,
        carried_amount: u16,
        max_load: u16,
    ) -> u16 {
        let reserved_amount = carried_amount.min(self.reserved_storage);
        let remaining_capacity = max_load.saturating_sub(reserved_amount);
        let top_up = self
            .storage_fill
            .saturating_sub(self.reserved_storage)
            .min(remaining_capacity);
        self.reserved_storage = self.reserved_storage.saturating_sub(reserved_amount);
        self.storage_fill = self
            .storage_fill
            .saturating_sub(reserved_amount.saturating_add(top_up));
        top_up
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
        if self.source_production_kind_code != 7 || amount == 0 {
            return false;
        }
        self.progress = self.progress.wrapping_add(amount);
        true
    }

    /// Replay the ordinary-source zero-amount completion in `FUN_0047d940`.
    /// The function first adds the delivered amount to an input buffer (a
    /// no-op here), then clears an idle root's cooldown after subtracting it
    /// from source u16 `+0x12`. The executable switches on compiled nested
    /// production kind `+0x1c`, so kinds 7, 8, 14, and 15 take distinct
    /// source-record branches and leave this record unchanged.
    pub fn complete_zero_amount_source_delivery(&mut self) -> bool {
        match self.source_production_kind_code {
            7 | 8 | 14 | 15 => false,
            _ => {
                if self.activity == 0 {
                    self.source_production_time = self
                        .source_production_time
                        .wrapping_sub(self.scheduler_cooldown);
                    self.scheduler_cooldown = 0;
                }
                true
            }
        }
    }

    /// Compute `FUN_0047daf0`'s source fixed-point activity ratio from the
    /// live `Rohmenge` and `Workmenge` buffers. The executable caps each
    /// operand at 128 and cancels work below 64/128.
    pub fn source_scheduler_activity(self) -> u8 {
        if self.storage_fill >= self.storage_animation_capacity {
            return 0;
        }
        if self.scheduler_blocked {
            return 0;
        }
        if self.source_output_ware_slot == 0 {
            return 0;
        }
        if self.raw_material_stock == 0 {
            return 0;
        }
        let mut activity = if self.source_raw_material_amount == 0 {
            128
        } else {
            ((u32::from(self.raw_material_stock) << 7) / u32::from(self.source_raw_material_amount))
                .min(128) as u8
        };
        if self.source_work_material_amount != 0 {
            activity = activity.min(
                ((u32::from(self.work_material_stock) << 7)
                    / u32::from(self.source_work_material_amount))
                .min(128) as u8,
            );
        }
        (activity >= 64).then_some(activity).unwrap_or(0)
    }

    /// Apply one `FUN_0047daf0` root update. The source first consumes the
    /// stored activity byte, then derives and stores the next activity from
    /// the resulting fixed-point buffers.
    pub fn advance_source_scheduler(&mut self) {
        let previous_activity = self.activity;
        if previous_activity != 0 {
            self.work_material_stock = self.work_material_stock.wrapping_sub(scaled_amount(
                self.source_work_material_amount,
                previous_activity,
            ));
            self.raw_material_stock = self.raw_material_stock.wrapping_sub(scaled_amount(
                self.source_raw_material_amount,
                previous_activity,
            ));
            let output = scaled_amount(self.source_production_amount, previous_activity);
            self.storage_fill = self.storage_fill.wrapping_add(output);
            self.progress = self.progress.wrapping_add(output);
        }
        self.set_activity(self.source_scheduler_activity());
    }
}

/// Replay `FUN_004684a0` and the branch around it in `FUN_0047c830` for one
/// harvested raw-resource map cell. The source's two 128-scale divisions are
/// truncating for the nonnegative resource values supplied by `FUN_0046aff0`.
pub const fn source_resource_harvest_transition(
    resource_strength: u8,
    growth_factor: u8,
    island_attenuation: u8,
    ware: u8,
    island: u8,
    x: u16,
    y: u16,
) -> SourceResourceHarvestTransition {
    if island_attenuation == 0 {
        return SourceResourceHarvestTransition::Regrowth;
    }

    let grown = ((resource_strength as u16) * (growth_factor as u16)) >> 7;
    let strength = if matches!(ware, 0x34 | 0x35 | 0x39) {
        grown
    } else {
        grown - (((island_attenuation as u16) * grown) >> 7)
    };
    let band = if strength < 0x13 {
        0
    } else if strength < 0x2a {
        1
    } else if strength < 0x4c {
        2
    } else if strength < 0x6c {
        3
    } else {
        4
    };
    let mask_index =
        (((x & 3) as u32 + (y as u32) * 4 + island as u32 + ware as u32) & 31) as usize;
    if SOURCE_RESOURCE_GROWTH_MASKS[band][mask_index] {
        SourceResourceHarvestTransition::Regrowth
    } else {
        SourceResourceHarvestTransition::Drought
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
            properties: [("Ware".into(), "WERKZEUG".into())].into(),
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
    fn source_scheduler_gate_writes_phase_before_enable_and_cooldown_checks() {
        let mut state = SourceMapCellState::new(1, 2, 3, &definition(), 0).unwrap();
        state.scheduler_enabled = false;
        assert!(!state.source_scheduler_due(1));
        assert_eq!(state.phase, 1);

        state.scheduler_enabled = true;
        state.scheduler_cooldown = 2;
        assert!(!state.source_scheduler_due(2));
        assert_eq!(state.scheduler_cooldown, 1);
        assert!(state.source_scheduler_due(3));
        assert_eq!(state.scheduler_cooldown, 0);
        assert!(!state.source_scheduler_due(3));

        state.activity = 0;
        state.complete_source_scheduler_run();
        assert_eq!(state.scheduler_cooldown, 11);
        assert_eq!(state.source_production_time, 11);
        state.source_scheduler_interval = 5;
        state.activity = 64;
        state.complete_source_scheduler_run();
        // floor(Interval * activity / 128) = floor(5 * 64 / 128) = floor(2.5) = 2
        // (`FUN_0047daf0` truncates toward zero at `1602_exe.c:89930`).
        assert_eq!(state.scheduler_cooldown, 2);
        assert_eq!(state.source_production_time, 13);

        state.activity = 128;
        state.block_source_scheduler();
        assert!(state.scheduler_blocked);
        assert_eq!(state.activity, 0);
        state.unblock_source_scheduler();
        assert!(!state.scheduler_blocked);
        assert_eq!(state.activity, 0);
    }

    #[test]
    fn zero_amount_delivery_updates_only_the_ordinary_source_record_branch() {
        let mut ordinary = SourceMapCellState::new(0, 0, 0, &definition(), 0).unwrap();
        ordinary.scheduler_cooldown = 11;
        ordinary.source_production_time = 48;
        assert!(ordinary.complete_zero_amount_source_delivery());
        assert_eq!(ordinary.scheduler_cooldown, 0);
        assert_eq!(ordinary.source_production_time, 37);

        let mut special = SourceMapCellState::new(
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
        special.scheduler_cooldown = 11;
        special.source_production_time = 48;
        assert!(!special.complete_zero_amount_source_delivery());
        assert_eq!(special.scheduler_cooldown, 11);
        assert_eq!(special.source_production_time, 48);
    }

    #[test]
    fn scheduler_counts_nonfull_no_raw_material_runs_in_four_bits() {
        let mut state = SourceMapCellState::new(
            1,
            2,
            3,
            &BuildingDef {
                kind: "HANDWERK".into(),
                storage_animation_capacity: 128,
                source_max_no_raw_material_count: 9,
                ..Default::default()
            },
            0,
        )
        .unwrap();
        state.source_no_raw_material_count = 14;
        state.complete_source_scheduler_run();
        assert_eq!(state.source_no_raw_material_count, 15);
        state.complete_source_scheduler_run();
        assert_eq!(state.source_no_raw_material_count, 15);

        state.storage_fill = 128;
        state.source_no_raw_material_count = 3;
        state.complete_source_scheduler_run();
        assert_eq!(state.source_no_raw_material_count, 3);

        state.storage_fill = 0;
        state.activity = 64;
        state.complete_source_scheduler_run();
        assert_eq!(state.source_no_raw_material_count, 0);
        assert_eq!(state.source_max_no_raw_material_count, 9);

        state.source_production_time = 239;
        state.progress = 100;
        state.activity = 0;
        state.complete_source_scheduler_run();
        assert_eq!(state.source_production_time, 125);
        assert_eq!(state.progress, 50);
    }

    #[test]
    fn transfer_dispatch_uses_the_post_update_activity_or_storage_gate() {
        let mut state = SourceMapCellState::new(
            1,
            2,
            3,
            &BuildingDef {
                kind: "HANDWERK".into(),
                storage_animation_capacity: 64,
                properties: [("ProdKind".into(), "HANDWERK".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();

        assert!(state.is_type8_transfer_root());
        assert!(state.allows_source_transfer_dispatch());

        state.storage_fill = 64;
        assert!(!state.allows_source_transfer_dispatch());

        state.activity = 64;
        assert!(state.allows_source_transfer_dispatch());
    }

    #[test]
    fn type8_transfer_selector_admits_raw_at_the_inclusive_two_batch_boundary() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            storage_animation_capacity: 320,
            source_raw_material_amount: 64,
            properties: [
                ("ProdKind".into(), "HANDWERK".into()),
                ("Ware".into(), "WERKZEUG".into()),
            ]
            .into(),
            ..Default::default()
        };
        let state = SourceMapCellState {
            raw_material_stock: 128,
            ..SourceMapCellState::new(0, 0, 0, &definition, 0).unwrap()
        };

        assert_eq!(
            state.source_type8_transfer_input(),
            Some(SourceType8TransferInput::RawMaterial)
        );
    }

    #[test]
    fn type8_transfer_selector_uses_the_lower_eligible_fixed_point_input_ratio() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            storage_animation_capacity: 320,
            source_raw_material_amount: 64,
            source_work_material_amount: 64,
            properties: [
                ("ProdKind".into(), "HANDWERK".into()),
                ("Ware".into(), "WERKZEUG".into()),
            ]
            .into(),
            ..Default::default()
        };
        let state = SourceMapCellState {
            raw_material_stock: 192,
            work_material_stock: 128,
            ..SourceMapCellState::new(0, 0, 0, &definition, 0).unwrap()
        };

        assert_eq!(
            state.source_type8_transfer_input(),
            Some(SourceType8TransferInput::WorkMaterial)
        );

        let ineligible = SourceMapCellState {
            work_material_stock: 129,
            ..state
        };
        assert_eq!(ineligible.source_type8_transfer_input(), None);
    }

    #[test]
    fn source_root_retains_compiled_figuranz_for_type11_admission() {
        let definition = BuildingDef {
            kind: "MARKT".into(),
            source_transfer_figure_limit: 2,
            source_transfer_radius: 16,
            source_scheduler_interval: 7,
            ..Default::default()
        };

        assert_eq!(
            SourceMapCellState::new_static(1, 2, 3, &definition, 0)
                .unwrap()
                .source_transfer_figure_limit,
            2
        );
        assert_eq!(
            SourceMapCellState::new_static(1, 2, 3, &definition, 0)
                .unwrap()
                .source_transfer_radius,
            16
        );
        assert_eq!(
            SourceMapCellState::new_static(1, 2, 3, &definition, 0)
                .unwrap()
                .source_scheduler_interval,
            7
        );
    }

    #[test]
    fn source_root_retains_compiled_figurnr_for_type11_figure_selection() {
        let definition = BuildingDef {
            kind: "KONTOR".into(),
            properties: [("Figurnr".into(), "TRAEGER2".into())].into(),
            ..Default::default()
        };

        assert_eq!(
            SourceMapCellState::new_static(1, 2, 3, &definition, 0)
                .unwrap()
                .source_transfer_figure,
            SourceTransferFigure::Traeger2
        );
    }

    #[test]
    fn source_city_root_uses_nested_production_kind_for_type11_dispatch() {
        let definition = BuildingDef {
            kind: "GEBAEUDE".into(),
            properties: [
                ("ProdKind".into(), "MARKT".into()),
                ("Figurnr".into(), "KARREN".into()),
            ]
            .into(),
            ..Default::default()
        };

        let state = SourceMapCellState::new(1, 2, 3, &definition, 0)
            .expect("compiled MARKT root receives selector state");
        assert_eq!(state.kind_code, 14);
        assert_eq!(state.source_production_kind_code, 7);
        assert!(state.is_type11_transfer_root());
    }

    #[test]
    fn definition_indices_follow_fun_00463b10_oriented_write_order() {
        let definition = BuildingDef {
            kind: "HANDWERK".into(),
            gfx: 100,
            size: (2, 3),
            ..Default::default()
        };
        let mut state =
            SourceMapCellState::new_static(0, 0, 0, &definition, 0).expect("static source command");

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
                        state.source_definition_offset_at(dx as u8, dy as u8),
                        expected[dy as usize][dx as usize],
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
    fn city_cart_collection_tops_up_a_reservation_with_new_output() {
        let mut state = SourceMapCellState {
            storage_fill: 129,
            reserved_storage: 65,
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

        assert_eq!(state.collect_reserved_storage_with_top_up(65, 192), 64);
        assert_eq!(state.storage_fill, 0);
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
            properties: [("Ware".into(), "WERKZEUG".into())].into(),
            ..Default::default()
        };
        let mut state = SourceMapCellState {
            raw_material_stock: 64,
            work_material_stock: 16,
            ..SourceMapCellState::new(0, 0, 0, &definition, 0).unwrap()
        };

        assert_eq!(state.source_scheduler_activity(), 64);
        state.advance_source_scheduler();

        assert_eq!(state.activity, 64);
        assert_eq!(state.raw_material_stock, 64);
        assert_eq!(state.work_material_stock, 16);
        assert_eq!(state.storage_fill, 0);
        assert_eq!(state.progress, 0);

        state.advance_source_scheduler();

        assert_eq!(state.activity, 0);
        assert_eq!(state.raw_material_stock, 32);
        assert_eq!(state.work_material_stock, 0);
        assert_eq!(state.storage_fill, 16);
        assert_eq!(state.progress, 16);
    }

    #[test]
    fn scheduler_requires_a_nonzero_compiled_ware_selector() {
        let inactive_definition = BuildingDef {
            kind: "HANDWERK".into(),
            source_raw_material_amount: 64,
            ..Default::default()
        };
        let inactive = SourceMapCellState {
            raw_material_stock: 64,
            ..SourceMapCellState::new(0, 0, 0, &inactive_definition, 0).unwrap()
        };
        assert_eq!(inactive.source_output_ware_slot, 0);
        assert_eq!(inactive.source_scheduler_activity(), 0);

        let zero_capacity_definition = BuildingDef {
            properties: [("Ware".into(), "WERKZEUG".into())].into(),
            ..inactive_definition
        };
        let mut zero_capacity = SourceMapCellState {
            raw_material_stock: 64,
            ..SourceMapCellState::new(0, 0, 0, &zero_capacity_definition, 0).unwrap()
        };
        assert_eq!(zero_capacity.source_scheduler_activity(), 0);
        zero_capacity.advance_source_scheduler();
        assert_eq!(zero_capacity.raw_material_stock, 64);
        assert_eq!(zero_capacity.storage_fill, 0);

        let active_definition = BuildingDef {
            storage_animation_capacity: 64,
            ..zero_capacity_definition
        };
        let active = SourceMapCellState {
            raw_material_stock: 64,
            ..SourceMapCellState::new(0, 0, 0, &active_definition, 0).unwrap()
        };
        assert_eq!(active.source_output_ware_slot, 0x16);
        assert_eq!(active.source_scheduler_activity(), 128);
    }

    #[test]
    fn static_raw_resource_cell_retains_compiled_randwachs_factor() {
        let definition = BuildingDef {
            kind: "ROHSTOFF".into(),
            source_resource_growth_factor: 96,
            ..Default::default()
        };

        let state = SourceMapCellState::new_static(2, 7, 9, &definition, 0)
            .expect("raw resource defines a static source cell");

        assert_eq!(state.kind_code, 9);
        assert_eq!(state.source_resource_growth_factor, 96);
    }

    #[test]
    fn plantation_path_targeting_keeps_fixed_grass_terrain_owner_independent() {
        let grass = BuildingDef {
            kind: "BODEN".into(),
            properties: [("Ware".into(), "GRAS".into())].into(),
            ..Default::default()
        };
        let raw = BuildingDef {
            kind: "ROHSTOFF".into(),
            properties: [("Ware".into(), "GRAS".into())].into(),
            ..Default::default()
        };

        let grass = SourceMapCellState {
            source_map_owner_slot: 5,
            ..SourceMapCellState::new_static(1, 4, 7, &grass, 0).unwrap()
        };
        let raw = SourceMapCellState {
            source_map_owner_slot: 5,
            ..SourceMapCellState::new_static(1, 5, 7, &raw, 0).unwrap()
        };

        assert!(grass.is_plantation_worker_target(2, 0x34));
        assert!(!raw.is_plantation_worker_target(2, 0x34));
        assert!(raw.is_plantation_worker_target(5, 0x34));
        assert!(!SourceMapCellState {
            source_resource_reserved: true,
            ..grass
        }
        .is_plantation_worker_target(2, 0x34));
    }

    #[test]
    fn plantation_root_and_harvest_replacement_keep_the_source_selectors() {
        let plantation = BuildingDef {
            kind: "GEBAEUDE".into(),
            properties: [
                ("ProdKind".into(), "PLANTAGE".into()),
                ("Rohstoff".into(), "GETREIDE".into()),
                ("Figurnr".into(), "MAEHER".into()),
            ]
            .into(),
            ..Default::default()
        };
        let root = SourceMapCellState::new(1, 2, 3, &plantation, 0).unwrap();
        assert!(root.is_type12_plantation_root());
        assert_eq!(root.source_raw_resource_ware_slot, 0x2d);
        assert_eq!(root.source_plantation_worker_definition, 0x60);

        let raw = BuildingDef {
            kind: "ROHSTOFF".into(),
            gfx: 100,
            properties: [
                ("Ware".into(), "GETREIDE".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..Default::default()
        };
        let mut cell = SourceMapCellState::new_static(1, 7, 9, &raw, 0).unwrap();
        assert_eq!(cell.source_path_class, 46);
        cell.source_resource_reserved = true;
        assert!(cell.replace_harvested_raw_resource(SourceResourceHarvestTransition::Regrowth));
        assert_eq!(cell.source_definition_offset, 99);
        assert_eq!(cell.kind_code, 10);
        assert_eq!(cell.source_production_kind_code, 10);
        assert_eq!(cell.source_output_ware_slot, 0);
        assert_eq!(cell.source_growth_resource_ware_slot, 0x2d);
        assert!(!cell.source_resource_is_dry);
        assert!(cell.admits_plantation_worker_path(0, 0x2d));
        assert!(!cell.is_plantation_worker_target(0, 0x2d));
        assert!(!cell.source_resource_reserved);
    }

    #[test]
    fn resource_environment_moves_only_dry_kind10_cells_back_to_raw() {
        let raw = BuildingDef {
            kind: "ROHSTOFF".into(),
            gfx: 100,
            properties: [("Ware".into(), "GETREIDE".into())].into(),
            ..Default::default()
        };
        let mut cell = SourceMapCellState::new_static(1, 7, 9, &raw, 0).unwrap();
        cell.source_resource_growth_factor = 128;
        assert!(cell.replace_harvested_raw_resource(SourceResourceHarvestTransition::Drought));
        assert!(cell.source_resource_is_dry);
        assert!(cell.restore_dry_resource(SourceResourceHarvestTransition::Regrowth));
        assert_eq!(cell.source_definition_offset, 100);
        assert_eq!(cell.kind_code, 9);
        assert_eq!(cell.source_production_kind_code, 9);
        assert_eq!(cell.source_output_ware_slot, 0x2d);
        assert!(!cell.source_resource_is_dry);

        assert!(cell.replace_harvested_raw_resource(SourceResourceHarvestTransition::Regrowth));
        assert!(!cell.source_resource_is_dry);
        assert!(!cell.restore_dry_resource(SourceResourceHarvestTransition::Regrowth));
    }

    #[test]
    fn raw_resource_harvest_uses_the_fun_004684a0_masks_only_with_attenuation() {
        assert_eq!(
            source_resource_harvest_transition(0, 0, 0, 0x2d, 0, 0, 0),
            SourceResourceHarvestTransition::Regrowth
        );
        assert_eq!(
            source_resource_harvest_transition(128, 128, 128, 0x2d, 0, 0, 0),
            SourceResourceHarvestTransition::Drought
        );
        assert_eq!(
            source_resource_harvest_transition(128, 128, 64, 0x2d, 0, 1, 0),
            SourceResourceHarvestTransition::Regrowth
        );
        assert_eq!(
            source_resource_harvest_transition(128, 128, 64, 0x2d, 0, 0, 0),
            SourceResourceHarvestTransition::Drought
        );
    }

    #[test]
    fn raw_resource_harvest_exempts_grass_tree_and_fish_from_attenuation() {
        assert_eq!(
            source_resource_harvest_transition(128, 128, 128, 0x34, 0, 2, 0),
            SourceResourceHarvestTransition::Regrowth
        );
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

        state.advance_source_scheduler();

        assert_eq!(state.activity, 0);
        assert_eq!(state.storage_fill, 0);
        assert_eq!(state.progress, 0);
    }
}
