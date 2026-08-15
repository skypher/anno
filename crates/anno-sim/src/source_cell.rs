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

/// Which record `FUN_0047d940` credited for one completed delivery.
///
/// The executable returns only 0 or 1, and it returns 0 for the kind-7 MARKT
/// branch even though that branch does advance the selector accumulator (case
/// 7 falls through into `case 8: return 0`, `1602_exe.c:89771-89778`). This
/// enum keeps the branch identity instead of that return value, because the
/// caller has to know which buffer moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceDeliveryOutcome {
    /// Production kind 8 (`KONTOR`): the record is left unchanged.
    Rejected,
    /// Credited to the raw buffer `+0x0a`, consumed by compiled `Rohmenge`.
    RawMaterial,
    /// Credited to the work buffer `+0x08`, consumed by compiled `Workmenge`.
    WorkMaterial,
    /// Credited to the kind-7 MARKT selector accumulator `+0x10`.
    MarketAccumulator,
    /// Production kind 14 keeps its two buffers in the 18-byte `FUN_00478e70`
    /// record (`+0x0a` for the `Workstoff` match, `+0x0c` otherwise), which
    /// this state does not model.
    MapObjectRecord { work_material: bool },
    /// Production kind 15 (`MILITAR`) credits the ware-indexed weapon store of
    /// the 48-byte `FUN_004797f0` record at this slot index.
    MilitaryStore { store_slot: u8 },
}

impl SourceDeliveryOutcome {
    /// True when the amount landed in this record rather than in one of the
    /// other record types the executable keeps for kinds 14 and 15.
    #[inline]
    pub const fn credited_here(self) -> bool {
        matches!(
            self,
            Self::RawMaterial | Self::WorkMaterial | Self::MarketAccumulator
        )
    }
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

/// Number of parallel growth clocks kept by the source. `FUN_0047ca80`
/// advances `DAT_0054a2f4[i]` (accumulator) and `DAT_00562da8[i]` (three-bit
/// counter) for `i` in `0..32`; bucket `i` fires every `40000 + 7000*i` ms,
/// i.e. 40 000 through 257 000.
pub const SOURCE_GROWTH_BUCKET_COUNT: usize = 32;

/// Period of one growth bucket, in milliseconds.
#[inline]
pub const fn source_growth_bucket_period_ms(bucket: usize) -> u32 {
    40_000 + 7_000 * (bucket as u32)
}

/// One compiled 0x88-byte definition record of a raw-resource group, holding
/// exactly the fields the growth pipeline reads out of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceResourceRecord {
    /// `@Nummer` slot. The compiled table is 0x88-byte records in this order,
    /// so `record ± 1` is the source's `def ± 0x88` step.
    pub record: i32,
    /// Compiled `Gfx` at definition offset `+0x84`, which the transition
    /// writers move into the tile's low 13 bits. `FUN_0047ca80` reaches the
    /// *next* record's copy as `def+0x10c`.
    pub gfx: u16,
    /// Compiled `AnimAnz` at `+0x78`: the number of bucket rollovers this
    /// record needs before the sweep promotes the tile.
    pub anim_count: u8,
    /// Value compiled into `+0x3a`. On a production-kind-10 record
    /// `FUN_00462d50` has already rewritten the authored `Interval` into a
    /// bucket index; every other record still holds the raw `Interval`, which
    /// `FUN_0047c760` clamps to 31 when it is used as a bucket.
    pub bucket: u8,
    /// `AnimTime` at `+0x70` equals `TIMENEVER`, registered as **0**
    /// (`1602_exe.c:44507`, `:66370`). Only then are tile bits 15..=18 free to
    /// carry the growth phase; the sea resource (`AnimTime: 130`) animates
    /// with those bits and never receives a phase write.
    pub writes_phase_bits: bool,
}

/// The compiled record group a raw-resource map cell moves through:
/// `[HAUSWACHS, HAUSFERT, DOERR]`, adjacent 0x88-byte records in `@Nummer`
/// order. Only the seven crops are triples — haeuser.cod contains exactly
/// seven `Doerrflg: 1` records; pasture, forest, palms and the sea resource
/// are pairs with no withered entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceResourceRecordLink {
    /// `ripe - 1`: the `ROHSTWACHS` growth record.
    pub growth: SourceResourceRecord,
    /// The `ROHSTOFF` record a plantation worker can harvest.
    pub ripe: SourceResourceRecord,
    /// `ripe + 1` when that record carries `Doerrflg`.
    pub dry: Option<SourceResourceRecord>,
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

/// `FUN_0046f920` (`1602_exe.c:78809-78815`) admits these outer map kinds
/// without comparing their owner or compiled `Ware` selector. All remaining
/// kinds need both fields to match the plantation worker's requested raw
/// resource.
///
/// The unconditional half is bit-for-bit the type-8 transfer wave's arm, so it
/// forwards to [`crate::island_map::source_transfer_wave_opens_ground_kind`];
/// the name is kept because the plantation raster's `default:` arm differs.
pub const fn source_plantation_path_kind_always_walkable(kind_code: u8) -> bool {
    crate::island_map::source_transfer_wave_opens_ground_kind(kind_code)
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
    /// Compiled active `Kosten` at definition offset `+0x2c`. The city
    /// maintenance accumulator `+0x1d8` (`FUN_0047f450`/`FUN_00463140`)
    /// charges this per constructed building; terrain and houses carry no
    /// `Kosten` property and cost nothing.
    #[serde(default)]
    pub source_operating_cost: u16,
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
    /// Compiled `Workstoff` selector at definition offset `+0x23`. This is
    /// the ware whose deliveries `FUN_0047d940` credits to the work buffer
    /// `+0x08`; every other ware lands in the raw buffer `+0x0a`
    /// (`1602_exe.c:89797-89807`). `FUN_0047daf0` likewise requests this
    /// selector when the work ratio is the scarcer of the two.
    #[serde(default)]
    pub source_work_material_ware_slot: u8,
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
    /// Compiled `Wegspeed[2]` path class at definition offset `+0x5a`, the
    /// KARREN entry.
    ///
    /// The window rasters read `def[0x58 + speedtyp] & 0x7f` off whatever
    /// definition the live map word names (`1602_exe.c:79479`, `:79587`), so
    /// the class a cell contributes depends on the *searching figure*, not on
    /// the cell. A live cell record therefore has to carry every class its
    /// rasters can select; `source_path_class` is the `Speedtyp: 0` TRAEGER
    /// entry and this is the `Speedtyp: 2` entry `FUN_004706e0` selects for a
    /// type-11 city cart.
    #[serde(default)]
    pub source_path_class_city_cart: u8,
    /// Compiled `Randwachs` at definition offset `+0x40`, in the source's
    /// 128-scale.
    ///
    /// `FUN_004684a0` does **not** read this off the resource cell — it reads
    /// it off whatever record occupies the same coordinate in the base terrain
    /// layer `param_1[0x2bf]`, this port's `source_static_map_backing_cells`.
    /// That is the layer the property is authored on: `ObjFill: 0,MAXHAUS`
    /// gives every slot `Randwachs: 100` and only the desert markers override
    /// it (25 / 40 / 50 / 75), while no crop, forest or pasture record authors
    /// it at all. Reading a resource cell's own copy was right by accident on
    /// ordinary ground — both are 128 — and wrong on desert.
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
    /// `@Nummer` slot of the compiled record this cell currently shows. The
    /// runtime table is 0x88-byte records in this order, so every `def ± 0x88`
    /// step in the resource pipeline is `record ± 1` here.
    #[serde(default)]
    pub source_definition_record: i32,
    /// The `[ROHSTWACHS, ROHSTOFF, DOERR]` record group this cell belongs to,
    /// resolved once from haeuser.cod. Present only on raw-resource cells.
    #[serde(default)]
    pub source_resource_records: Option<SourceResourceRecordLink>,
    /// This cell owns a live entry in the source growth table `DAT_00562dc8`.
    ///
    /// The executable keeps that as a 32860-slot open-addressed hash table
    /// keyed by `((island & 7) * 0x40 + (x & 0x3f)) * 0x40 + (y & 0x3f)`, with
    /// a 92-slot linear probe window. It is **not** saved: `FUN_00481450` case
    /// 10 rebuilds it from tile state while replaying INSELHAUS commands, so
    /// per-cell fields hold the same information. The one behaviour this does
    /// not reproduce is `FUN_0047c760`'s failure mode — when every slot in a
    /// cell's probe window is taken the insert **silently does nothing** and
    /// that tile never grows. Here enrolment always succeeds.
    #[serde(default)]
    pub source_growth_enrolled: bool,
    /// Table byte 3: which of the 32 bucket clocks this entry follows.
    #[serde(default)]
    pub source_growth_bucket: u8,
    /// Table byte 4: the snapshot of `DAT_00562da8[bucket]` taken at the last
    /// visit. The sweep advances the entry only when the counter has moved.
    #[serde(default)]
    pub source_growth_phase_seen: u8,
    /// Table byte 5: completed growth phases, seeded from the tile's bits
    /// 15..=18 at enrolment and written back into them on every visit.
    #[serde(default)]
    pub source_growth_phase: u8,
    /// The command word's bits 17..=21, which `FUN_004631b0` filled with one
    /// `rand() & 0x1F` at command construction and the save persists. It is
    /// the sole RNG input to the growth pipeline: `FUN_00481450` case 10 arms
    /// a newly placed kind-10 tile with `def[0x3a] + param_7 % 3`.
    #[serde(default)]
    pub source_placement_variant: u8,
    /// Compiled `Destroyflg` at definition offset `+0x6a`, bit 7.
    ///
    /// `FUN_00479ca0` refuses to register an affliction on a definition that
    /// carries it (`1602_exe.c:86842`), and `FUN_0047f510` uses it together
    /// with production kind 7 to recognise the "Zerstörter Marktplatz"
    /// record (`Id: IDDIVERS+22`) it reaps once per city event cycle.
    ///
    /// It is authored on far more than the ruin records: every `HANG`,
    /// `STRAND*`, `FLUSS*`, `RUINE`, `STRANDRUINE`, the `HQ` at `IDHAFEN+20`,
    /// two native `GEBAEUDE` plantations, and the destroyed marketplace.
    #[serde(default)]
    pub source_destroy_flag: bool,
    /// Compiled `HAUS_BAUKOST Holz` at definition offset `+0x4e`, in the
    /// source's `<< 5` fixed-point scale. `FUN_0047a020`'s fire-spread branch
    /// and `FUN_0047b540`'s ignition roll both select a `DAT_0049af08` row
    /// with `Ziegel <= Holz` off an arbitrary target definition, so this has
    /// to be present on every record and not only the five housing rows.
    #[serde(default)]
    pub source_wood_cost_fixed: u16,
    /// Compiled `HAUS_BAUKOST Ziegel` at definition offset `+0x50`.
    #[serde(default)]
    pub source_bricks_cost_fixed: u16,
    /// Compiled `Wegspeed[3]` path class at definition offset `+0x5b`, the
    /// class `FUN_00472930` writes into the shared scratch grid for the
    /// hazard area scan (`param_3 == 3`). Every authored `Wegspeed` quad ends
    /// in `100`, so the shipped data compiles this to 32 throughout — and
    /// `FUN_0046f8a0` gives classes `0..=0x20` the same `(0x40, 0x5b)`
    /// orthogonal/diagonal step costs, which makes the flood uniform-cost.
    #[serde(default)]
    pub source_path_class_loaded_road: u8,
}

/// One `Objekt: HAUS_BAUKOST` material cost in the source's compiled
/// fixed-point scale. `1602_exe.c:67386-67531` shifts `Werkzeug`, `Holz`,
/// `Ziegel` and `Kanon` left five when it writes definition offsets
/// `+0x4c..+0x52`; the same conversion already backs
/// [`crate::data_bridge::SourceKind13PromotionDefinition`].
fn source_construction_cost_fixed(definition: &BuildingDef, key: &str) -> u16 {
    (definition
        .properties
        .get(key)
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(0)
        .max(0) as u16)
        .wrapping_shl(5)
}

impl SourceMapCellState {
    /// `FUN_0046f920`'s path-grid admission predicate for an already decoded
    /// static map cell. A fixed terrain kind remains traversable across every
    /// owner; all other cells require both the map owner and compiled `Ware`
    /// selector to match the worker's root:
    ///
    /// ```text
    /// if ((param_6 == param_8) && ((*param_3 >> 0x13 & 7) == param_2))
    /// ```
    ///
    /// (`1602_exe.c:78803`). The comparison is exact — there is no wildcard
    /// for the "no settlement" selector 7, unlike the city hazard scan at
    /// `1602_exe.c:12128`, which admits `(tile & 0x380000) == 0x380000`
    /// explicitly. `source_owner` is not a player id and is not taken from the
    /// worker's city: `FUN_0044b7e0` reads it straight off the producing
    /// root's own map tile (`uVar6 = *puVar1 >> 0x13 & 7`,
    /// `1602_exe.c:52409`) and stores it at figure record `+0x2b`, which
    /// `FUN_0045b200` passes here as `param_2` (`1602_exe.c:62982`).
    ///
    /// A player therefore harvests wild woodland only because founding
    /// rewrites that woodland's selector: `FUN_00465170` runs `FUN_0046ac60`
    /// over the Kontor's `max(Radius, 8)` disc and claims every unowned
    /// non-`MEER` tile into the new settlement slot
    /// (`1602_exe.c:70669-70689`, `:74480-74483`). Outside any settlement both
    /// the harvester and the trees keep 7 and still compare equal.
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
    pub fn new(island: u8, x: u8, y: u8, definition: &BuildingDef, phase: u8) -> Option<Self> {
        let state = Self::new_static(island, x, y, definition, phase)?;
        state.allocates_source_scheduler_record().then_some(state)
    }

    /// `FUN_00481450` decides which placed tile receives a live 20-byte
    /// `FUN_0047cbf0` record. It switches on the **nested** `HAUS_PRODTYP
    /// Kind` at definition offset `+0x1c` (`1602_exe.c:92790-92892`), not on
    /// the outer `HAUS Kind` at `+0x04`: cases 1 through 8 are HANDWERK,
    /// PLANTAGE, BERGWERK, WEIDETIER, JAGDHAUS, FISCHEREI, MARKT and KONTOR,
    /// and case `0x1e` follows the same allocation. Production kind 10 goes to
    /// `FUN_00481f50`, 13 to `FUN_00479f70`, 14 to `FUN_00478c70`, and 15 to
    /// `FUN_00481d50`, each of which keeps a different record type.
    ///
    /// Every stock production building in haeuser.cod carries outer
    /// `Kind: GEBAEUDE` (code 14) or another terrain-shaped label such as
    /// `HQ` or `STRANDHAUS`, so testing the outer kind here admitted roads,
    /// walls and towers while excluding every real producer.
    ///
    /// `FUN_00481450` cases `0x11..=0x1b` — the WIRT/KAPELLE/KIRCHE/BADEHAUS/
    /// THEATER/KLINIK/SCHULE/HOCHSCHULE/GALGEN/BRUNNEN/SCHLOSS service
    /// buildings — also allocate a record; they are not admitted here because
    /// no part of this port schedules them yet.
    #[inline]
    pub const fn allocates_source_scheduler_record(self) -> bool {
        matches!(self.source_production_kind_code, 1..=8 | 30)
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
        // Compiled definition offset `+0x1c`. `ObjFill: 0,MAXHAUS` pre-fills
        // every slot from a `@Nummer: 0` block whose `HAUS_PRODTYP Kind` is
        // `UNUSED`, so an absent nested kind compiles to zero.
        let source_production_kind_code = definition.source_production_kind_code().unwrap_or(0);
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
            source_operating_cost: definition.source_operating_costs.0,
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
            source_work_material_ware_slot: definition
                .source_work_material_ware_slot()
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
            source_path_class_city_cart: definition
                .source_path_classes()
                .map(|classes| classes[2])
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
            source_definition_record: definition.nummer,
            source_resource_records: None,
            source_growth_enrolled: false,
            source_growth_bucket: 0,
            source_growth_phase_seen: 0,
            source_growth_phase: 0,
            source_placement_variant: 0,
            source_destroy_flag: definition
                .properties
                .get("Destroyflg")
                .is_some_and(|value| value.trim() == "1"),
            source_wood_cost_fixed: source_construction_cost_fixed(definition, "Holz"),
            source_bricks_cost_fixed: source_construction_cost_fixed(definition, "Ziegel"),
            source_path_class_loaded_road: definition
                .source_path_classes()
                .map(|classes| classes[3])
                .unwrap_or_default(),
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

    /// Resolve the compiled record group this cell belongs to and record the
    /// slot it currently occupies. Every `def ± 0x88` step the resource
    /// pipeline takes is resolved through this group, never through `Gfx ± 1`.
    pub fn configure_source_resource_records(&mut self, cod: &CodFile, definition: &BuildingDef) {
        self.source_definition_record = definition.nummer;
        self.source_resource_records = source_resource_record_link(cod, definition);
    }

    /// The record this cell currently shows, selected the way the executable
    /// selects it: by dereferencing the tile's `Gfx` through the definition
    /// table and reading `+0x1c` / `Doerrflg`.
    #[inline]
    pub fn current_source_resource_record(self) -> Option<SourceResourceRecord> {
        let link = self.source_resource_records?;
        Some(
            match (self.source_production_kind_code, self.source_resource_is_dry) {
                (10, true) => link.dry?,
                (10, false) => link.growth,
                _ => link.ripe,
            },
        )
    }

    /// Replay `FUN_0047c760` (`1602_exe.c:88815`): clamp the bucket to 31,
    /// snapshot that bucket's current counter, and seed the phase byte from
    /// the tile's live bits 15..=18. The whole routine `0x47c760-0x47c80b`
    /// contains no `rand()` call, so arming never perturbs the RNG stream.
    pub fn arm_source_growth_timer(
        &mut self,
        bucket: u8,
        bucket_phases: [u8; SOURCE_GROWTH_BUCKET_COUNT],
    ) {
        let bucket = bucket.min(31);
        self.source_growth_bucket = bucket;
        self.source_growth_phase_seen = bucket_phases[usize::from(bucket)];
        self.source_growth_phase = self.source_variant & 0x0f;
        self.source_growth_enrolled = true;
    }

    /// Arm a freshly placed production-kind-10 tile the way `FUN_00481450`
    /// case 10 does (`1602_exe.c:92837-92842`): the record's own bucket plus
    /// the command word's persisted `rand() & 0x1F` taken modulo three. This
    /// is the only jitter anywhere in the pipeline; harvest re-enrolment and
    /// the drought sweep arm with the plain `def[0x3a]`.
    pub fn arm_placed_source_growth_timer(
        &mut self,
        bucket_phases: [u8; SOURCE_GROWTH_BUCKET_COUNT],
    ) -> bool {
        if self.source_production_kind_code != 10 || self.source_growth_enrolled {
            return false;
        }
        let Some(record) = self.current_source_resource_record() else {
            return false;
        };
        let jitter = self.source_placement_variant % 3;
        self.arm_source_growth_timer(record.bucket.saturating_add(jitter), bucket_phases);
        true
    }

    /// Move this cell onto one growth-side record: rewrite the tile's low 13
    /// bits from that record's `Gfx`, clear the phase bits when the record
    /// leaves them free, and re-enrol on that record's bucket.
    ///
    /// The outer map kind is deliberately left alone. A group's records all
    /// carry the same outer `Kind` in the shipped data — `WALD` for crops,
    /// forest and palms, `BODEN` for pasture, `MEER` for the sea resource —
    /// and the executable never writes an outer kind anywhere: it re-derives
    /// every definition field from the tile's `Gfx`. Forcing `kind_code = 10`
    /// here used to demote ripe pasture out of `FUN_0046f920`'s always-
    /// walkable terrain set.
    fn enter_source_growth_record(
        &mut self,
        record: SourceResourceRecord,
        dry: bool,
        bucket_phases: [u8; SOURCE_GROWTH_BUCKET_COUNT],
    ) {
        self.source_definition_offset = record.gfx;
        self.source_definition_record = record.record;
        self.source_production_kind_code = 10;
        if self.source_output_ware_slot != 0 {
            self.source_growth_resource_ware_slot = self.source_output_ware_slot;
        }
        self.source_resource_is_dry = dry;
        self.source_output_ware_slot = 0;
        self.source_raw_resource_ware_slot = 0;
        self.source_plantation_worker_definition = 0;
        // `if (*(int *)(iVar4 + 0x70) == 0) { *puVar5 = uVar3 & 0xfff87fff; }`
        if record.writes_phase_bits {
            self.source_variant = 0;
        }
        self.arm_source_growth_timer(record.bucket, bucket_phases);
    }

    /// Model the state written by `FUN_0047c830` (`1602_exe.c:88874`) after a
    /// type-12 worker harvests this cell.
    ///
    /// The gate is the **nested** production kind at definition offset `+0x1c`
    /// (`if (*(int *)(iVar1 + 0x1c) == 9)`, `1602_exe.c:88888`), not the outer
    /// map kind. With the shipped data a ripe crop is outer `WALD` (10) over
    /// nested `ROHSTOFF` (9) and ripe pasture is outer `BODEN` (11), so the
    /// superseded `self.kind_code != 9` guard never fired on real data and
    /// island woodland was an infinite resource. Only a synthetic
    /// `kind: "ROHSTOFF"` fixture matched it.
    ///
    /// The reservation bit 29 is cleared unconditionally, on the taken and the
    /// untaken branch alike (`*puVar5 = uVar3 & 0xdfffffff`).
    pub fn replace_harvested_raw_resource(
        &mut self,
        transition: SourceResourceHarvestTransition,
        bucket_phases: [u8; SOURCE_GROWTH_BUCKET_COUNT],
    ) -> bool {
        let target = self.harvest_target_record(transition);
        self.source_resource_reserved = false;
        let Some(target) = target else {
            return false;
        };
        self.enter_source_growth_record(
            target,
            transition == SourceResourceHarvestTransition::Drought,
            bucket_phases,
        );
        true
    }

    /// The raw-resource half of `FUN_0047c920` (`1602_exe.c:88924-88938`): an
    /// unreserved nested-kind-9 cell moves to `def + 0x88` only when
    /// `FUN_004684a0` selects the drought branch, and re-enrols on that
    /// record's `+0x3a`.
    pub fn advance_raw_resource_to_drought(
        &mut self,
        transition: SourceResourceHarvestTransition,
        bucket_phases: [u8; SOURCE_GROWTH_BUCKET_COUNT],
    ) -> bool {
        if self.source_resource_reserved || transition != SourceResourceHarvestTransition::Drought {
            return false;
        }
        let Some(target) = self.harvest_target_record(transition) else {
            return false;
        };
        self.enter_source_growth_record(target, true, bucket_phases);
        true
    }

    /// Both drought-side destinations of a nested-kind-9 cell: `def - 0x88`
    /// for regrowth and `def + 0x88` for withering.
    ///
    /// A group with no authored `Doerrflg` record — pasture, forest, palms,
    /// the sea resource — has no withering destination. The executable steps
    /// to `ripe + 0x88` regardless and lands on the *next variant's* growth
    /// record, a data hazard reachable only on drought islands (the drought
    /// byte is zero on every New Horizons0 island). This declines the
    /// transition instead of reproducing the corruption.
    fn harvest_target_record(
        self,
        transition: SourceResourceHarvestTransition,
    ) -> Option<SourceResourceRecord> {
        if self.source_production_kind_code != 9 {
            return None;
        }
        let link = self.source_resource_records?;
        match transition {
            SourceResourceHarvestTransition::Regrowth => Some(link.growth),
            SourceResourceHarvestTransition::Drought => link.dry,
        }
    }

    /// The dry-resource half of `FUN_0047c920` (`1602_exe.c:88940-88958`): the
    /// preceding `ROHSTOFF` record is restored when the growth mask selects
    /// regrowth. The gate is `def[0x1c] == 10 && (def[0x47] & 8) != 0`, again
    /// the nested production kind rather than the outer map kind.
    ///
    /// The executable also re-enrols the restored tile, arming it with
    /// `def-0xd6` — the ripe record's `+0x3a`, which `FUN_00462d50` never
    /// rewrote, so it is a raw authored `Interval` clamped to a bucket. That
    /// entry can never complete: the sweep only advances the phase byte for
    /// production kind 10, so it sits in the table writing a frozen counter
    /// into the ripe tile's frame bits until the slot is reused. This declines
    /// to reproduce that leak. It is reachable only on drought islands, and
    /// the drought byte is zero on every New Horizons0 island.
    pub fn restore_dry_resource(&mut self, transition: SourceResourceHarvestTransition) -> bool {
        if self.source_production_kind_code != 10
            || !self.source_resource_is_dry
            || transition != SourceResourceHarvestTransition::Regrowth
        {
            return false;
        }
        self.ripen_source_raw_resource()
    }

    /// Write the ripe `ROHSTOFF` record over this cell. `FUN_0047ca80` reaches
    /// it as `def+0x10c` — the *next* record's `Gfx` — and leaves the phase
    /// bits alone; only the low 13 bits move.
    fn ripen_source_raw_resource(&mut self) -> bool {
        let Some(link) = self.source_resource_records else {
            return false;
        };
        self.source_definition_offset = link.ripe.gfx;
        self.source_definition_record = link.ripe.record;
        self.source_production_kind_code = 9;
        self.source_output_ware_slot = self.source_growth_resource_ware_slot;
        self.source_raw_resource_ware_slot = 0;
        self.source_resource_is_dry = false;
        true
    }

    /// Replay one `FUN_0047ca80` sweep visit of this cell's table entry
    /// (`1602_exe.c:88998-89024`). Returns whether the tile word changed.
    ///
    /// ```text
    /// if e[0] != 0xFF && counter[e[3]] != e[4] {
    ///     e[4] = counter[e[3]]
    ///     def = gfx_table[tile & 0x1FFF]
    ///     if def[0x1c] == 10 && ++e[5] >= def[0x78] {
    ///         if (def[0x47] & 8) == 0 { tile.gfx = def[0x10c] }   // not Doerrflg
    ///         e[0] = 0xFF                                          // free, always
    ///     } else if def[0x70] == 0 {
    ///         tile = (tile & 0xFFF87FFF) | ((e[5] & 0xF) << 15)
    ///     }
    /// }
    /// ```
    ///
    /// The phase byte is incremented **only** for production kind 10
    /// (`0x47cb5f` jumps past the `inc`), so a stale entry left on a
    /// demolished tile keeps writing its frozen counter into that tile's frame
    /// bits — the executable never removes an entry on demolition.
    ///
    /// `DAT_00562da8` is a three-bit counter compared against a per-entry
    /// snapshot, so an entry whose bucket rolls an exact multiple of eight
    /// times between two sweep visits would be skipped. That cannot happen at
    /// the shipped rates: the fastest bucket is 40 s and a full sweep is 160
    /// sub-steps, about 32 s of simulated time.
    pub fn tick_source_growth_entry(
        &mut self,
        bucket_phases: [u8; SOURCE_GROWTH_BUCKET_COUNT],
    ) -> bool {
        if !self.source_growth_enrolled {
            return false;
        }
        let counter = bucket_phases[usize::from(self.source_growth_bucket & 31)];
        if counter == self.source_growth_phase_seen {
            return false;
        }
        self.source_growth_phase_seen = counter;
        let Some(record) = self.current_source_resource_record() else {
            return false;
        };
        if self.source_production_kind_code == 10 {
            self.source_growth_phase = self.source_growth_phase.wrapping_add(1);
            if u32::from(record.anim_count) <= u32::from(self.source_growth_phase) {
                let ripened = !self.source_resource_is_dry && self.ripen_source_raw_resource();
                self.source_growth_enrolled = false;
                return ripened;
            }
        }
        if record.writes_phase_bits {
            let phase = self.source_growth_phase & 0x0f;
            let changed = self.source_variant != phase;
            self.source_variant = phase;
            return changed;
        }
        false
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
        // Command-word bits 17..=21. `FUN_004631b0` filled them with one
        // `rand() & 0x1F` when the command was constructed and the save
        // persists them; `FUN_00481450` case 10 spends them as growth jitter.
        self.source_placement_variant = command.random_seed & 0x1f;
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

    /// Replay `FUN_0047d940` (`1602_exe.c:89760-89845`) for one delivery of
    /// `amount` source units of `ware_slot` arriving at this command root.
    /// `amount` is in the source 1/32-good fixed point: a type-12 plantation
    /// worker carries exactly `0x20`, the value `FUN_00471c50` returns from a
    /// successful target search and `FUN_0045b200` stores at figure `+0x28`
    /// (`1602_exe.c:62988-62990`, `:80682`).
    ///
    /// The executable switches on the compiled nested production kind at
    /// definition offset `+0x1c`:
    ///
    /// * `default` (`:89776-89790`) credits the work buffer `+0x08` when the
    ///   delivered ware equals the definition's `Workstoff` byte `+0x23` and
    ///   the raw buffer `+0x0a` otherwise, then clears an idle root's
    ///   cooldown after subtracting it from the production timer `+0x12`.
    /// * case 7 (`:89771-89775`) advances a MARKT root's selector
    ///   accumulator `+0x10` and falls through to case 8.
    /// * case 8 (`:89776`) returns 0 without touching the record.
    /// * case `0xe` (`:89791-89800`) and case `0xf` (`:89801-89814`) credit
    ///   the `FUN_00478e70` and `FUN_004797f0` records instead. Those are
    ///   different record types which this 20-byte `FUN_0047cbf0` state does
    ///   not model, and [`Self::allocates_source_scheduler_record`] never
    ///   builds a live root for either kind, so the two branches are reported
    ///   rather than applied.
    pub fn complete_source_delivery(
        &mut self,
        ware_slot: u8,
        amount: u16,
    ) -> SourceDeliveryOutcome {
        match self.source_production_kind_code {
            7 => {
                self.progress = self.progress.wrapping_add(amount);
                SourceDeliveryOutcome::MarketAccumulator
            }
            8 => SourceDeliveryOutcome::Rejected,
            14 => SourceDeliveryOutcome::MapObjectRecord {
                work_material: ware_slot == self.source_work_material_ware_slot,
            },
            // `psVar1 = (short *)(iVar2 + -0x10 + param_4 * 2)`. The record
            // pointer returned by `FUN_004797f0` sits `0x10` bytes above the
            // store array, so the slot index is `ware - 0x0b`: the barracks
            // hold SCHWERTER, MUSKETEN and KANONEN, which `1602_exe.c:90216`
            // reads back as `*(ushort *)(rec + 6 + (ware - 0xb) * 2)`.
            15 => SourceDeliveryOutcome::MilitaryStore {
                store_slot: ware_slot.wrapping_sub(0x0b),
            },
            _ => {
                let work_material = ware_slot == self.source_work_material_ware_slot;
                if work_material {
                    self.work_material_stock = self.work_material_stock.wrapping_add(amount);
                } else {
                    self.raw_material_stock = self.raw_material_stock.wrapping_add(amount);
                }
                if self.activity == 0 {
                    self.source_production_time = self
                        .source_production_time
                        .wrapping_sub(self.scheduler_cooldown);
                    self.scheduler_cooldown = 0;
                }
                if work_material {
                    SourceDeliveryOutcome::WorkMaterial
                } else {
                    SourceDeliveryOutcome::RawMaterial
                }
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

/// Resolve the compiled `[ROHSTWACHS, ROHSTOFF, DOERR]` record group that
/// `definition` belongs to, anchored on the ripe `ROHSTOFF` record.
///
/// The link is the 0x88-byte record stride, i.e. `@Nummer ± 1`. Adjacent
/// records' `Gfx` values are not adjacent: a Weizen group runs `GFXROHST+0`
/// (growth, which reserves its five `AnimAnz` sprite slots), `+5` (ripe) and
/// `+48` (withered), so the gaps from the ripe record are `-5` and `+43`.
///
/// `RandAnz` / `RandAdd` deliberately play no part. Variant selection
/// (`base + (rand() % RandAnz) * RandAdd * 0x88`) happens only in the planting
/// and ruin paths (`1602_exe.c:88741`, `:69745`); forest is
/// `RandAnz: 11, RandAdd: 2`, and the ±1 record step stays inside the selected
/// variant's own pair.
///
/// A record whose neighbour is not the expected production kind yields no
/// link. The shipped file has one such shape — the trailing `Rohstoffe-Wald
/// Palmen` block authors ripe records with no preceding `ROHSTWACHS`, so the
/// executable's unconditional `def - 0x88` would land on a pasture record.
pub fn source_resource_record_link(
    cod: &CodFile,
    definition: &BuildingDef,
) -> Option<SourceResourceRecordLink> {
    let ripe_record = match definition.source_production_kind_code()? {
        9 => definition.nummer,
        10 if definition.source_resource_is_dry() => definition.nummer - 1,
        10 => definition.nummer + 1,
        _ => return None,
    };
    let ripe = cod.building_by_nummer(ripe_record)?;
    let growth = cod.building_by_nummer(ripe_record - 1)?;
    if ripe.source_production_kind_code() != Some(9)
        || growth.source_production_kind_code() != Some(10)
        || growth.source_resource_is_dry()
    {
        return None;
    }
    let dry = cod
        .building_by_nummer(ripe_record + 1)
        .filter(|dry| dry.source_production_kind_code() == Some(10) && dry.source_resource_is_dry());
    Some(SourceResourceRecordLink {
        growth: source_resource_record(growth),
        ripe: source_resource_record(ripe),
        dry: dry.map(source_resource_record),
    })
}

fn source_resource_record(definition: &BuildingDef) -> SourceResourceRecord {
    SourceResourceRecord {
        record: definition.nummer,
        gfx: u16::try_from(definition.gfx).unwrap_or(0),
        anim_count: u8::try_from(definition.anim_anz).unwrap_or(u8::MAX),
        // `FUN_00462d50` rewrote `+0x3a` into a bucket index for kind-10
        // records only; every other record still holds its authored
        // `Interval`, which `FUN_0047c760` clamps to 31 when used as a bucket.
        bucket: definition
            .source_growth_bucket()
            .unwrap_or_else(|| definition.source_scheduler_interval.min(31) as u8),
        writes_phase_bits: definition.anim_time == 0,
    }
}

/// `FUN_0047c830`'s own branch around `FUN_004684a0` (`1602_exe.c:88891`):
///
/// ```text
/// target = def - 0x88
/// if (island[0x45] != 0 && FUN_004684a0(island, ware, x, y) == 0)
///     target = def + 0x88
/// ```
///
/// The drought byte `INSEL5[0x66]` -> `island+0x45` is zero on every New
/// Horizons0 island, so a harvest there always regrows and the dither table is
/// never consulted. This gate is deliberately *not* inside
/// [`source_resource_harvest_transition`]: the placement path at
/// `1602_exe.c:7755-7759` calls that function without it.
pub const fn source_harvest_regrowth_transition(
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
    source_resource_harvest_transition(
        resource_strength,
        growth_factor,
        island_attenuation,
        ware,
        island,
        x,
        y,
    )
}

/// Replay `FUN_004684a0` (`1602_exe.c:72690`) for one raw-resource map cell.
/// The source's two 128-scale divisions are truncating for the nonnegative
/// resource values supplied by `FUN_0046aff0`.
///
/// `growth_factor` is `Randwachs` read off the **base terrain layer**
/// `param_1[0x2bf]`, not off the resource cell's own definition — see
/// [`crate::simulation::Simulation::source_base_terrain_growth_factor`].
///
/// There is deliberately no `island_attenuation == 0 -> Regrowth` shortcut
/// here. That gate belongs to the caller `FUN_0047c830`, which only consults
/// this function at all when the island drought byte `+0x45` is nonzero
/// (`1602_exe.c:88891`). The placement path at `1602_exe.c:7755-7759` calls it
/// with no such gate, and there a zero result legitimately plants an already
/// withered field.
pub const fn source_resource_harvest_transition(
    resource_strength: u8,
    growth_factor: u8,
    island_attenuation: u8,
    ware: u8,
    island: u8,
    x: u16,
    y: u16,
) -> SourceResourceHarvestTransition {
    let grown = ((resource_strength as u16) * (growth_factor as u16)) >> 7;
    let strength = if island_attenuation == 0 || matches!(ware, 0x34 | 0x35 | 0x39) {
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

    /// Shaped like every real producer in haeuser.cod: the outer `HAUS Kind`
    /// is `GEBAEUDE` and the production label lives in the nested
    /// `HAUS_PRODTYP Kind`, which the parser exposes as `ProdKind`. Putting a
    /// production label in the outer kind, as these fixtures used to, is a
    /// shape the shipped data never has.
    fn production_definition(prod_kind: &str) -> BuildingDef {
        BuildingDef {
            kind: "GEBAEUDE".into(),
            anim_anz: 4,
            anim_frame: 3,
            properties: [
                ("ProdKind".into(), prod_kind.into()),
                ("Ware".into(), "WERKZEUG".into()),
            ]
            .into(),
            ..Default::default()
        }
    }

    fn definition() -> BuildingDef {
        production_definition("HANDWERK")
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

    /// The ordinary branch of `FUN_0047d940` (`1602_exe.c:89776-89790`) both
    /// credits a buffer and clears an idle root's cooldown. The superseded
    /// `complete_zero_amount_source_delivery` only ever ran the cooldown tail,
    /// so its expectation was `scheduler_cooldown == 0 && production_time ==
    /// 37` with both stock buffers left at 0; the amount is now credited too.
    #[test]
    fn ordinary_delivery_credits_a_buffer_and_clears_the_idle_cooldown() {
        let mut ordinary = SourceMapCellState::new(0, 0, 0, &definition(), 0).unwrap();
        ordinary.scheduler_cooldown = 11;
        ordinary.source_production_time = 48;
        assert_eq!(
            ordinary.complete_source_delivery(0x35, 0x20),
            SourceDeliveryOutcome::RawMaterial
        );
        assert_eq!(ordinary.raw_material_stock, 0x20);
        assert_eq!(ordinary.work_material_stock, 0);
        assert_eq!(ordinary.scheduler_cooldown, 0);
        assert_eq!(ordinary.source_production_time, 37);
    }

    /// `if (*(byte *)(iVar2 + 0x23) == param_4)` selects the work buffer
    /// `+0x08`; every other ware falls to the raw buffer `+0x0a`
    /// (`1602_exe.c:89781-89787`). The iron smelter is the stock example:
    /// `Rohstoff: EISENERZ` with `Workstoff: HOLZ`.
    #[test]
    fn delivery_picks_the_buffer_by_the_definition_workstoff_byte() {
        let mut smelter = SourceMapCellState::new(
            0,
            0,
            0,
            &BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [
                    ("ProdKind".into(), "HANDWERK".into()),
                    ("Ware".into(), "EISEN".into()),
                    ("Rohstoff".into(), "EISENERZ".into()),
                    ("Workstoff".into(), "HOLZ".into()),
                ]
                .into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        assert_eq!(smelter.source_work_material_ware_slot, 0x17);
        assert_eq!(smelter.source_raw_resource_ware_slot, 0x02);

        // HOLZ matches `Workstoff` -> work buffer `+0x08`.
        assert_eq!(
            smelter.complete_source_delivery(0x17, 0x20),
            SourceDeliveryOutcome::WorkMaterial
        );
        assert_eq!(smelter.work_material_stock, 0x20);
        assert_eq!(smelter.raw_material_stock, 0);

        // EISENERZ does not -> raw buffer `+0x0a`.
        assert_eq!(
            smelter.complete_source_delivery(0x02, 0x40),
            SourceDeliveryOutcome::RawMaterial
        );
        assert_eq!(smelter.work_material_stock, 0x20);
        assert_eq!(smelter.raw_material_stock, 0x40);
    }

    /// A busy root (`+0x0e != 0`) jumps straight to the dirty-flag tail and
    /// keeps its cooldown (`goto LAB_0047da94`, `1602_exe.c:89788`).
    #[test]
    fn busy_root_keeps_its_cooldown_but_still_takes_the_delivery() {
        let mut busy = SourceMapCellState::new(0, 0, 0, &definition(), 0).unwrap();
        busy.activity = 96;
        busy.scheduler_cooldown = 11;
        busy.source_production_time = 48;
        assert_eq!(
            busy.complete_source_delivery(0x35, 0x20),
            SourceDeliveryOutcome::RawMaterial
        );
        assert_eq!(busy.raw_material_stock, 0x20);
        assert_eq!(busy.scheduler_cooldown, 11);
        assert_eq!(busy.source_production_time, 48);
    }

    /// Case 8 (`KONTOR`) returns 0 with the record untouched, and case 7
    /// (`MARKT`) advances only the selector accumulator `+0x10` before falling
    /// through into it (`1602_exe.c:89771-89778`).
    #[test]
    fn market_and_kontor_take_the_dedicated_branches() {
        let mut market =
            SourceMapCellState::new(0, 0, 0, &production_definition("MARKT"), 0).unwrap();
        market.scheduler_cooldown = 11;
        market.source_production_time = 48;
        assert_eq!(
            market.complete_source_delivery(0x17, 0x20),
            SourceDeliveryOutcome::MarketAccumulator
        );
        assert_eq!(market.progress, 0x20);
        assert_eq!(market.raw_material_stock, 0);
        assert_eq!(market.work_material_stock, 0);
        assert_eq!(market.scheduler_cooldown, 11);
        assert_eq!(market.source_production_time, 48);

        let mut kontor =
            SourceMapCellState::new(0, 0, 0, &production_definition("KONTOR"), 0).unwrap();
        kontor.scheduler_cooldown = 11;
        kontor.source_production_time = 48;
        assert_eq!(
            kontor.complete_source_delivery(0x17, 0x20),
            SourceDeliveryOutcome::Rejected
        );
        assert!(!kontor.complete_source_delivery(0x17, 0x20).credited_here());
        assert_eq!(kontor.progress, 0);
        assert_eq!(kontor.raw_material_stock, 0);
        assert_eq!(kontor.work_material_stock, 0);
        assert_eq!(kontor.scheduler_cooldown, 11);
        assert_eq!(kontor.source_production_time, 48);
    }

    /// Cases `0xe` and `0xf` credit records this 20-byte state does not hold.
    /// The kind-15 store index follows `iVar2 + -0x10 + param_4 * 2`, which
    /// `1602_exe.c:90216` reads back as `rec + 6 + (ware - 0x0b) * 2`: the
    /// barracks hold SCHWERTER (`0x0b`), MUSKETEN (`0x0c`) and KANONEN
    /// (`0x0d`) in slots 0, 1 and 2.
    #[test]
    fn map_object_and_military_kinds_report_their_own_record() {
        let military = BuildingDef {
            kind: "GEBAEUDE".into(),
            properties: [
                ("ProdKind".into(), "MILITAR".into()),
                ("Ware".into(), "NOWARE".into()),
            ]
            .into(),
            ..Default::default()
        };
        // `FUN_00481450` case 0xf routes production kind 15 to
        // `FUN_00481d50`, never to a `FUN_0047cbf0` record, so no live root
        // exists for it; only the static view retains the state.
        assert!(SourceMapCellState::new(0, 0, 0, &military, 0).is_none());
        let mut state = SourceMapCellState::new_static(0, 0, 0, &military, 0).unwrap();
        assert_eq!(state.source_production_kind_code, 15);
        assert_eq!(
            state.complete_source_delivery(0x0c, 0x20),
            SourceDeliveryOutcome::MilitaryStore { store_slot: 1 }
        );
        assert!(!state.complete_source_delivery(0x0c, 0x20).credited_here());
        assert_eq!(state.raw_material_stock, 0);
        assert_eq!(state.work_material_stock, 0);
        assert_eq!(state.progress, 0);

        let mut kind14 = SourceMapCellState::new_static(
            0,
            0,
            0,
            &BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [
                    ("ProdKind".into(), "GEBAEUDE".into()),
                    ("Workstoff".into(), "HOLZ".into()),
                ]
                .into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        assert_eq!(kind14.source_production_kind_code, 14);
        assert_eq!(
            kind14.complete_source_delivery(0x17, 0x20),
            SourceDeliveryOutcome::MapObjectRecord {
                work_material: true
            }
        );
        assert_eq!(
            kind14.complete_source_delivery(0x02, 0x20),
            SourceDeliveryOutcome::MapObjectRecord {
                work_material: false
            }
        );
        assert_eq!(kind14.raw_material_stock, 0);
        assert_eq!(kind14.work_material_stock, 0);
    }

    /// One harvested tile is `0x20`, which drives the scheduler straight to
    /// full activity for a `Rohmenge: 1` root and yields exactly one whole
    /// good of output.
    #[test]
    fn one_harvest_unit_produces_one_whole_good_at_the_forester() {
        let forester = BuildingDef {
            kind: "GEBAEUDE".into(),
            source_production_amount: 32,
            source_raw_material_amount: 32,
            storage_animation_capacity: 320,
            properties: [
                ("ProdKind".into(), "PLANTAGE".into()),
                ("Ware".into(), "HOLZ".into()),
                ("Rohstoff".into(), "BAUM".into()),
                ("Figurnr".into(), "HOLZFAELLER".into()),
            ]
            .into(),
            ..Default::default()
        };
        let mut root = SourceMapCellState::new(0, 0, 0, &forester, 0).unwrap();
        assert_eq!(root.source_scheduler_activity(), 0);
        assert_eq!(
            root.complete_source_delivery(0x35, 0x20),
            SourceDeliveryOutcome::RawMaterial
        );
        assert_eq!(root.raw_material_stock, 0x20);
        assert_eq!(root.source_scheduler_activity(), 128);
        root.advance_source_scheduler();
        assert_eq!(root.activity, 128);
        root.advance_source_scheduler();
        assert_eq!(root.raw_material_stock, 0);
        assert_eq!(root.storage_fill, 32);
    }

    #[test]
    fn scheduler_counts_nonfull_no_raw_material_runs_in_four_bits() {
        let mut state = SourceMapCellState::new(
            1,
            2,
            3,
            &BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
                kind: "GEBAEUDE".into(),
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
            kind: "GEBAEUDE".into(),
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
            kind: "GEBAEUDE".into(),
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
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "MARKT".into())].into(),
            source_transfer_figure_limit: 2,
            source_operating_costs: (0, 0),
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
            kind: "GEBAEUDE".into(),
            properties: [
                ("ProdKind".into(), "KONTOR".into()),
                ("Figurnr".into(), "TRAEGER2".into()),
            ]
            .into(),
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
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "MARKT".into())].into(),
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
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
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
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "HANDWERK".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();

        // Superseded expectation: `accept_market_transfer` returned true for
        // the MARKT root and false for the workshop, leaving the workshop
        // untouched. `FUN_0047d940` does not silently drop the workshop's
        // amount; it credits the raw buffer instead.
        assert_eq!(
            market.complete_source_delivery(0x16, 32),
            SourceDeliveryOutcome::MarketAccumulator
        );
        assert_eq!(market.progress, 32);
        assert_eq!(
            workshop.complete_source_delivery(0x16, 32),
            SourceDeliveryOutcome::RawMaterial
        );
        assert_eq!(workshop.progress, 0);
        assert_eq!(workshop.raw_material_stock, 32);
    }

    #[test]
    fn storage_fill_uses_source_maxlager_scale_and_rounding() {
        let definition = BuildingDef {
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
            kind: "GEBAEUDE".into(),
            storage_animation_capacity: 320,
            source_production_amount: 32,
            source_raw_material_amount: 64,
            source_work_material_amount: 32,
            properties: [
                ("ProdKind".into(), "HANDWERK".into()),
                ("Ware".into(), "WERKZEUG".into()),
            ]
            .into(),
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
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
            properties: [
                ("ProdKind".into(), "HANDWERK".into()),
                ("Ware".into(), "WERKZEUG".into()),
            ]
            .into(),
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

    /// The shipped Weizen record group, shaped exactly like haeuser.cod: three
    /// consecutive `@Nummer` slots, outer `Kind: WALD` throughout, nested
    /// `ROHSTWACHS` / `ROHSTOFF` / `ROHSTWACHS + Doerrflg`, `AnimAnz: 5`,
    /// `AnimTime: TIMENEVER`, `Interval: 175`, and the `Gfx` values the file
    /// actually authors — `GFXROHST+0`, `+5` and `+48`, i.e. 584 / 589 / 632.
    fn weizen_group() -> CodFile {
        let growth = BuildingDef {
            nummer: 40,
            kind: "WALD".into(),
            gfx: 584,
            anim_anz: 5,
            anim_time: 0,
            source_scheduler_interval: 175,
            properties: [
                ("ProdKind".into(), "ROHSTWACHS".into()),
                ("Ware".into(), "NOWARE".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..Default::default()
        };
        let ripe = BuildingDef {
            nummer: 41,
            gfx: 589,
            anim_anz: 1,
            source_scheduler_interval: 0,
            properties: [
                ("ProdKind".into(), "ROHSTOFF".into()),
                ("Ware".into(), "GETREIDE".into()),
                ("Rohstoff".into(), "NOWARE".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..growth.clone()
        };
        let dry = BuildingDef {
            nummer: 42,
            gfx: 632,
            properties: [
                ("ProdKind".into(), "ROHSTWACHS".into()),
                ("Ware".into(), "NOWARE".into()),
                ("Doerrflg".into(), "1".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..growth.clone()
        };
        CodFile {
            constants: std::collections::HashMap::new(),
            buildings: vec![growth, ripe, dry],
        }
    }

    /// The shipped `IDWEIDE` pasture pair: outer `Kind: BODEN` on both records,
    /// `AnimAnz: 1`, `Interval: 300`, and no `Doerrflg` third record.
    fn pasture_group() -> CodFile {
        let growth = BuildingDef {
            nummer: 1,
            kind: "BODEN".into(),
            gfx: 4,
            anim_anz: 1,
            anim_time: 0,
            source_scheduler_interval: 300,
            properties: [
                ("ProdKind".into(), "ROHSTWACHS".into()),
                ("Ware".into(), "NOWARE".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..Default::default()
        };
        let ripe = BuildingDef {
            nummer: 2,
            gfx: 44,
            source_scheduler_interval: 0,
            properties: [
                ("ProdKind".into(), "ROHSTOFF".into()),
                ("Ware".into(), "GRAS".into()),
                ("Rohstoff".into(), "NOWARE".into()),
                ("Wegspeed".into(), "145,120,170,100".into()),
            ]
            .into(),
            ..growth.clone()
        };
        CodFile {
            constants: std::collections::HashMap::new(),
            buildings: vec![growth, ripe],
        }
    }

    fn linked_cell(cod: &CodFile, index: usize) -> SourceMapCellState {
        let definition = &cod.buildings[index];
        let mut cell = SourceMapCellState::new_static(1, 7, 9, definition, 0)
            .expect("raw-resource record defines a static source cell");
        cell.configure_source_resource_records(cod, definition);
        cell
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

        // Superseded fixture: a synthetic `kind: "ROHSTOFF"` record with
        // `gfx: 100`, expecting `source_definition_offset == 99` after the
        // harvest. That outer kind never occurs in haeuser.cod; a ripe crop is
        // outer `WALD` over nested `ROHSTOFF`, and the growth record is a whole
        // 0x88-byte record back, five `Gfx` slots away.
        let cod = weizen_group();
        let mut cell = linked_cell(&cod, 1);
        assert_eq!(cell.kind_code, 10);
        assert_eq!(cell.source_production_kind_code, 9);
        assert_eq!(cell.source_path_class, 46);
        cell.source_resource_reserved = true;
        assert!(cell.replace_harvested_raw_resource(
            SourceResourceHarvestTransition::Regrowth,
            [0; SOURCE_GROWTH_BUCKET_COUNT],
        ));
        assert_eq!(cell.source_definition_offset, 584);
        assert_eq!(cell.source_definition_record, 40);
        assert_eq!(cell.kind_code, 10);
        assert_eq!(cell.source_production_kind_code, 10);
        assert_eq!(cell.source_output_ware_slot, 0);
        assert_eq!(cell.source_growth_resource_ware_slot, 0x2d);
        assert!(!cell.source_resource_is_dry);
        assert!(cell.admits_plantation_worker_path(0, 0x2d));
        assert!(!cell.is_plantation_worker_target(0, 0x2d));
        assert!(!cell.source_resource_reserved);
        assert!(cell.source_growth_enrolled);
    }

    /// `FUN_0047c830` gates on the **nested** production kind at `+0x1c`
    /// (`1602_exe.c:88888`). Guarding on the outer map kind instead admitted
    /// only a synthetic `kind: "ROHSTOFF"` fixture: with the shipped data a
    /// ripe crop is outer `WALD` (10) and ripe pasture is outer `BODEN` (11),
    /// so the transition never fired and island woodland never depleted.
    #[test]
    fn harvest_guard_uses_the_nested_production_kind_not_the_outer_map_kind() {
        let crops = weizen_group();
        let mut crop = linked_cell(&crops, 1);
        assert_ne!(crop.kind_code, 9, "the outer kind is WALD, not ROHSTOFF");
        assert!(crop.replace_harvested_raw_resource(
            SourceResourceHarvestTransition::Regrowth,
            [0; SOURCE_GROWTH_BUCKET_COUNT],
        ));

        let pastures = pasture_group();
        let mut pasture = linked_cell(&pastures, 1);
        assert_eq!(pasture.kind_code, 11);
        assert!(pasture.replace_harvested_raw_resource(
            SourceResourceHarvestTransition::Regrowth,
            [0; SOURCE_GROWTH_BUCKET_COUNT],
        ));
        assert_eq!(pasture.source_definition_offset, 4);
        // The executable writes only the tile's low 13 bits and re-derives
        // every definition field from them, so a group's outer kind never
        // changes. Forcing it to 10 dropped ripe pasture out of
        // `FUN_0046f920`'s always-walkable terrain set.
        assert_eq!(pasture.kind_code, 11);
        assert!(source_plantation_path_kind_always_walkable(
            pasture.kind_code
        ));

        // A growth record is not a harvest target; the gate rejects it.
        let mut growing = linked_cell(&crops, 0);
        assert!(!growing.replace_harvested_raw_resource(
            SourceResourceHarvestTransition::Regrowth,
            [0; SOURCE_GROWTH_BUCKET_COUNT],
        ));
    }

    /// The harvest and sweep writers move a whole 0x88-byte record and read
    /// *that* record's `Gfx`. Adjacent records' `Gfx` values are not adjacent:
    /// Weizen's three records are `GFXROHST+0`, `+5` and `+48`, so from the
    /// ripe record the real gaps are `-5` and `+43` — `gfx ± 1` is wrong.
    #[test]
    fn harvest_follows_the_record_stride_not_a_gfx_index() {
        let cod = weizen_group();
        let link = source_resource_record_link(&cod, &cod.buildings[1])
            .expect("the Weizen group resolves from its ripe record");
        assert_eq!((link.growth.record, link.growth.gfx), (40, 584));
        assert_eq!((link.ripe.record, link.ripe.gfx), (41, 589));
        assert_eq!(
            link.dry.map(|dry| (dry.record, dry.gfx)),
            Some((42, 632)),
            "the third record is the authored Doerrflg field"
        );
        assert_eq!(link.ripe.gfx - link.growth.gfx, 5);
        assert_eq!(link.dry.unwrap().gfx - link.ripe.gfx, 43);

        // Every record of the group resolves the same triple.
        for index in 0..3 {
            assert_eq!(
                source_resource_record_link(&cod, &cod.buildings[index]),
                Some(link),
                "record {index}"
            );
        }

        // Pasture, forest, palms and the sea resource are pairs: haeuser.cod
        // authors exactly seven `Doerrflg: 1` records, all of them crops.
        let pastures = pasture_group();
        let pair = source_resource_record_link(&pastures, &pastures.buildings[1])
            .expect("the pasture pair resolves");
        assert_eq!(pair.dry, None);

        let mut cell = linked_cell(&pastures, 1);
        assert!(!cell.advance_raw_resource_to_drought(
            SourceResourceHarvestTransition::Drought,
            [0; SOURCE_GROWTH_BUCKET_COUNT],
        ));
    }

    /// `FUN_00462d50` (`1602_exe.c:68880-68889`) rewrites `Interval` at `+0x3a`
    /// into a bucket index for every production-kind-10 record. Every row is
    /// one of the shipped resource groups.
    #[test]
    fn growth_bucket_matches_every_shipped_resource_interval() {
        let bucket = |interval: u16, anim_anz: i32| {
            BuildingDef {
                source_scheduler_interval: interval,
                anim_anz,
                properties: [("ProdKind".into(), "ROHSTWACHS".into())].into(),
                ..Default::default()
            }
            .source_growth_bucket()
        };

        // record          | Interval | AnimAnz | per phase | bucket | period
        assert_eq!(bucket(175, 5), Some(0)); // Weizen         35 000    40 000
        assert_eq!(bucket(330, 5), Some(3)); // Baumwolle etc. 66 000    61 000
        assert_eq!(bucket(300, 5), Some(2)); // Zuckerrohr     60 000    54 000
        assert_eq!(bucket(900, 5), Some(20)); // Wald / Palmen 180 000  180 000
        assert_eq!(bucket(450, 6), Some(5)); // Meereszeichen  75 000    75 000
        assert_eq!(bucket(300, 1), Some(32)); // IDWEIDE      300 000  clamped
        assert_eq!(bucket(200, 1), Some(22)); // IDBODEN      200 000  194 000

        for (bucket_index, period) in [(0, 40_000), (3, 61_000), (20, 180_000), (31, 257_000)] {
            assert_eq!(source_growth_bucket_period_ms(bucket_index), period);
        }

        // `FUN_0047c760` clamps the arming bucket to 31, so the 32 the
        // `IDWEIDE` pasture derives lands on the 257 000 ms clock.
        let mut pasture = linked_cell(&pasture_group(), 0);
        pasture.arm_source_growth_timer(32, [0; SOURCE_GROWTH_BUCKET_COUNT]);
        assert_eq!(pasture.source_growth_bucket, 31);

        // Only production kind 10 has its `+0x3a` rewritten; the map-cell
        // scheduler keeps reading the authored `Interval` on every other kind.
        let ripe = &weizen_group().buildings[1].clone();
        assert_eq!(ripe.source_growth_bucket(), None);
        assert_eq!(
            BuildingDef {
                source_scheduler_interval: 8,
                properties: [("ProdKind".into(), "PLANTAGE".into())].into(),
                ..Default::default()
            }
            .source_growth_bucket(),
            None
        );
    }

    /// The sweep writes the phase into tile bits 15..=18 only when the record's
    /// `AnimTime` is `TIMENEVER`, registered as 0 (`1602_exe.c:44507`,
    /// `:66370`). The sea resource authors `AnimTime: 130` and animates with
    /// those bits, so it counts to `AnimAnz` and promotes without ever
    /// receiving a phase write.
    #[test]
    fn sweep_writes_phase_bits_only_when_anim_time_is_timenever() {
        let mut phases = [0_u8; SOURCE_GROWTH_BUCKET_COUNT];
        let mut crop = linked_cell(&weizen_group(), 0);
        crop.source_growth_resource_ware_slot = 0x2d;
        crop.arm_source_growth_timer(0, phases);
        assert_eq!(crop.source_variant, 0);

        phases[0] = 1;
        assert!(crop.tick_source_growth_entry(phases));
        assert_eq!(crop.source_growth_phase, 1);
        assert_eq!(crop.source_variant, 1, "AnimTime == TIMENEVER");

        let sea = {
            let mut cod = weizen_group();
            for building in &mut cod.buildings {
                building.anim_time = 130;
                building.anim_anz = 6;
            }
            cod
        };
        let mut fish = linked_cell(&sea, 0);
        fish.source_growth_resource_ware_slot = 0x39;
        fish.arm_source_growth_timer(5, phases);
        phases[5] = 1;
        assert!(!fish.tick_source_growth_entry(phases));
        assert_eq!(fish.source_growth_phase, 1);
        assert_eq!(fish.source_variant, 0, "AnimTime: 130 owns bits 15..=18");
    }

    /// The whole cycle: harvest enrols the cell on its growth record's bucket,
    /// `AnimAnz` rollovers later the sweep writes the ripe record's `Gfx` and
    /// frees the slot. A visit whose bucket counter has not moved does nothing.
    #[test]
    fn growth_timer_ripens_a_harvested_cell_after_anim_anz_rollovers() {
        let cod = weizen_group();
        let mut phases = [0_u8; SOURCE_GROWTH_BUCKET_COUNT];
        let mut cell = linked_cell(&cod, 1);
        assert!(cell.replace_harvested_raw_resource(
            SourceResourceHarvestTransition::Regrowth,
            phases,
        ));
        assert!(cell.source_growth_enrolled);
        assert_eq!(cell.source_growth_bucket, 0, "Weizen is bucket 0");
        assert_eq!(cell.source_growth_phase, 0);

        // An unmoved counter is skipped: `counter[e[3]] != e[4]` fails.
        assert!(!cell.tick_source_growth_entry(phases));
        assert_eq!(cell.source_growth_phase, 0);

        for phase in 1..5 {
            phases[0] = (phases[0] + 1) & 7;
            cell.tick_source_growth_entry(phases);
            assert_eq!(cell.source_growth_phase, phase);
            assert_eq!(cell.source_production_kind_code, 10, "still growing");
        }

        phases[0] = (phases[0] + 1) & 7;
        assert!(cell.tick_source_growth_entry(phases));
        assert_eq!(cell.source_definition_offset, 589);
        assert_eq!(cell.source_definition_record, 41);
        assert_eq!(cell.source_production_kind_code, 9);
        assert_eq!(cell.source_output_ware_slot, 0x2d);
        assert_eq!(cell.kind_code, 10, "outer WALD is never rewritten");
        assert!(
            !cell.source_growth_enrolled,
            "`*PTR_DAT_0049aec0 = 0xff` frees the slot on promotion"
        );
        assert!(cell.is_plantation_worker_target(0, 0x2d));

        // A withered field counts the same way but keeps its record:
        // `if ((def[0x47] & 8) == 0)` skips the promotion write.
        let mut dry = linked_cell(&cod, 2);
        dry.source_growth_resource_ware_slot = 0x2d;
        dry.arm_source_growth_timer(0, phases);
        for _ in 0..5 {
            phases[0] = (phases[0] + 1) & 7;
            dry.tick_source_growth_entry(phases);
        }
        assert_eq!(dry.source_definition_offset, 632);
        assert_eq!(dry.source_production_kind_code, 10);
        assert!(!dry.source_growth_enrolled);
    }

    /// `FUN_00481450` case 10 (`1602_exe.c:92840`) arms a newly placed
    /// production-kind-10 tile with `def[0x3a] + param_7 % 3`, where `param_7`
    /// is the command word's bits 17..=21 — the one `rand() & 0x1F` that
    /// `FUN_004631b0` drew when the command was built. Nothing else in the
    /// pipeline jitters.
    #[test]
    fn placement_arming_adds_the_persisted_command_jitter() {
        let cod = weizen_group();
        let phases = [0_u8; SOURCE_GROWTH_BUCKET_COUNT];
        for (seed, expected) in [(0, 0), (1, 1), (2, 2), (3, 0), (17, 2), (31, 1)] {
            let mut cell = linked_cell(&cod, 0);
            cell.set_source_command(crate::building::SourceBuildingCommand {
                definition_offset: 584,
                orientation: 0,
                variant: 0,
                metadata: 0,
                map_owner_slot: 7,
                random_seed: seed,
                dynamic_object_owner: 0,
            });
            assert!(cell.arm_placed_source_growth_timer(phases));
            assert_eq!(cell.source_growth_bucket, expected, "seed {seed}");
            // Idempotent: an already-enrolled tile is not armed twice.
            assert!(!cell.arm_placed_source_growth_timer(phases));
        }

        // Harvest re-enrolment takes the plain `def[0x3a]`, no jitter.
        let mut ripe = linked_cell(&cod, 1);
        ripe.source_placement_variant = 31;
        assert!(ripe.replace_harvested_raw_resource(
            SourceResourceHarvestTransition::Regrowth,
            phases,
        ));
        assert_eq!(ripe.source_growth_bucket, 0);

        // Only kind-10 tiles get a timer; natural ripe terrain is not enrolled
        // at load and starts growing when a worker harvests it.
        let mut natural = linked_cell(&cod, 1);
        assert!(!natural.arm_placed_source_growth_timer(phases));
        assert!(!natural.source_growth_enrolled);
    }

    #[test]
    fn resource_environment_moves_only_dry_kind10_cells_back_to_raw() {
        let cod = weizen_group();
        let phases = [0_u8; SOURCE_GROWTH_BUCKET_COUNT];
        // Superseded fixture: `kind: "ROHSTOFF"`, `gfx: 100`, expecting
        // `101` withered and `100` restored. Real Weizen is 589 ripe, 632
        // withered — `+43`, not `+1`.
        let mut cell = linked_cell(&cod, 1);
        assert!(cell.replace_harvested_raw_resource(
            SourceResourceHarvestTransition::Drought,
            phases,
        ));
        assert!(cell.source_resource_is_dry);
        assert_eq!(cell.source_definition_offset, 632);
        assert!(cell.restore_dry_resource(SourceResourceHarvestTransition::Regrowth));
        assert_eq!(cell.source_definition_offset, 589);
        assert_eq!(cell.kind_code, 10);
        assert_eq!(cell.source_production_kind_code, 9);
        assert_eq!(cell.source_output_ware_slot, 0x2d);
        assert!(!cell.source_resource_is_dry);

        assert!(cell.replace_harvested_raw_resource(
            SourceResourceHarvestTransition::Regrowth,
            phases,
        ));
        assert!(!cell.source_resource_is_dry);
        assert!(!cell.restore_dry_resource(SourceResourceHarvestTransition::Regrowth));
    }

    /// `FUN_004684a0` itself has no attenuation shortcut — the gate is
    /// `FUN_0047c830`'s `if (*(char *)((int)param_1 + 0x45) != '\0')`
    /// (`1602_exe.c:88891`). The superseded expectation for the first case was
    /// `Regrowth`, produced by an early return that belonged to the caller; the
    /// dither table's band-0 bit for that cell is in fact clear.
    #[test]
    fn raw_resource_harvest_uses_the_fun_004684a0_masks_only_with_attenuation() {
        assert_eq!(
            source_resource_harvest_transition(0, 0, 0, 0x2d, 0, 0, 0),
            SourceResourceHarvestTransition::Drought
        );
        assert_eq!(
            source_harvest_regrowth_transition(0, 0, 0, 0x2d, 0, 0, 0),
            SourceResourceHarvestTransition::Regrowth,
            "a zero drought byte never reaches FUN_004684a0 from the harvest"
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
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), "HANDWERK".into())].into(),
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
