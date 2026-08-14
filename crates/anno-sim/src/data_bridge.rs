//! Bridge between parsed game files (anno-formats) and simulation data structures.
//!
//! Converts COD building definitions and SZS scenario data into the types
//! used by the simulation engine.

use std::collections::HashMap;

use anno_formats::cod::{BuildingDef as CodBuilding, CodFile};
use anno_formats::szs::{LandFigureDefinition, LandFigureFamily, SzsFile};

use crate::building::{BuildingDef, BuildingInstance, SourceBuildingCommand};
use crate::source_cell::SourceMapCellState;
use crate::source_route::{
    SourceDynamicMapObject, SourceDynamicMapObjectTable, SourceTargetDescriptor,
};
use crate::types::Good;

/// One live source `DAT_005a77e8` kind-13 map-object record.
///
/// `FUN_00478b90` creates a ten-byte runtime entry whenever an INSELHAUS
/// command installs a map object with source kind `0x0d` (`PLATZ` or
/// `WOHNUNG`). `FUN_0047b9c0` updates its low-three-bit phase before city
/// state consumes the group and fixed-point amount; `FUN_00480370` samples
/// the same physical table before allocating kind-`0x12` figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SourceKind13Location {
    pub island_id: u8,
    pub tile_x: u8,
    pub tile_y: u8,
    pub orientation: u8,
    /// INSELHAUS packed variant passed as `param_4` to `FUN_00478b90` and
    /// written to the high nibble of source byte `+0x04`. `FUN_0047b9c0`
    /// selects the record's phase clock from this field.
    #[serde(default)]
    pub variant: u8,
    /// Source map-owner bits from the live root cell. `FUN_0044b140` passes
    /// these to `FUN_0046f000` when it builds a type-3 civilian path grid.
    pub source_owner: u8,
    /// Low three bits of source byte `+0x03`, initially zeroed by
    /// `FUN_00478b90` and advanced by `FUN_0047b9c0`.
    #[serde(default)]
    pub phase: u8,
    /// Remaining source byte `+0x03` state bits. The effect queue preserves
    /// these while it advances the phase field.
    #[serde(default)]
    pub state_bits: u8,
    /// Definition `+0x2e` copied into source byte `+0x05`; for housing this
    /// is the authored `BGruppe` tier.
    #[serde(default)]
    pub population_group: u8,
    /// Source u16 at bytes `+0x06..+0x07`, initialized to `0x40` by
    /// `FUN_00478b90` and maintained in the source's 1/64-unit scale.
    #[serde(default = "source_kind13_initial_amount")]
    pub amount: u16,
    /// Source u16 at bytes `+0x08..+0x09`, initially zero and later read by
    /// the stage-transition predicates in `FUN_0047bfa0`.
    #[serde(default)]
    pub lifecycle_flags: u16,
}

const fn source_kind13_initial_amount() -> u16 {
    0x40
}

impl SourceKind13Location {
    /// Source byte `+0x03`, combining the independently retained phase and
    /// high lifecycle bits.
    pub const fn state_byte(self) -> u8 {
        (self.phase & 7) | (self.state_bits & !7)
    }

    /// Apply the low-three-bit phase write in `FUN_0047b9c0` while retaining
    /// the object state flags installed by the effect queue.
    pub fn set_phase(&mut self, phase: u8) {
        self.phase = phase & 7;
    }

    /// `DAT_0061fa4c[BGruppe]`, the fixed-point maximum admitted by
    /// `FUN_0047bbc0` and `FUN_0047c080` for this record's current tier.
    pub const fn source_amount_capacity(self) -> Option<u16> {
        match self.population_group {
            0 => Some(SOURCE_KIND13_AMOUNT_CAPACITIES[0]),
            1 => Some(SOURCE_KIND13_AMOUNT_CAPACITIES[1]),
            2 => Some(SOURCE_KIND13_AMOUNT_CAPACITIES[2]),
            3 => Some(SOURCE_KIND13_AMOUNT_CAPACITIES[3]),
            4 => Some(SOURCE_KIND13_AMOUNT_CAPACITIES[4]),
            _ => None,
        }
    }

    /// Replay the `FUN_0047bfa0` lifecycle predicate for a target BGruppe.
    /// `FUN_0047bbc0` uses it after a downgrade and `FUN_0047c080` uses it
    /// before promoting a residence to the next tier.
    pub const fn source_transition_active_for_group(self, target_group: u8) -> bool {
        let state = self.state_byte();
        let flags = self.lifecycle_flags;
        match target_group {
            // `FUN_0047bfa0` case 0 reads byte `+0x09 & 4`, i.e. the u16's
            // bit 0x400 (a neighbouring kind-8 object), not byte `+0x08 & 4`.
            0 => flags & 0x0400 != 0 || state & 0x80 != 0,
            1 => state & 0x80 != 0 && flags & 0x000c != 0,
            2 => {
                state & 0x80 != 0
                    && flags & 0x0010 != 0
                    && flags & 0x000c != 0
                    && (flags & 0x0020 != 0 || flags & 0x0100 != 0)
            }
            3 => {
                state & 0x80 != 0
                    && flags & 0x0010 != 0
                    && flags & 0x0008 != 0
                    && (flags & 0x0020 != 0 || flags & 0x0100 != 0)
                    && flags & 0x0040 != 0
            }
            4 => {
                state & 0x80 != 0
                    && flags & 0x0010 != 0
                    && flags & 0x0008 != 0
                    && flags & 0x0100 != 0
                    && flags & 0x0040 != 0
                    && flags & 0x0080 != 0
            }
            _ => false,
        }
    }
}

/// City operands read by `FUN_0047b410` before it changes one kind-13
/// record's fixed-point amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceKind13TransferInputs {
    /// Source city bytes `+0x248..+0x24c`, one satisfaction score per BGruppe.
    pub satisfaction_by_group: [u8; 5],
    /// Source city byte `+0x255`, the aggregate satisfaction score.
    pub overall_satisfaction: u8,
    /// Source city u16 `+0x1fe != 0`, which suppresses otherwise-positive
    /// kind-13 amount changes.
    pub growth_blocked: bool,
}

impl SourceKind13Location {
    /// Replay the signed amount returned by `FUN_0047b410` for this record.
    ///
    /// The caller applies a negative result through `FUN_0047bbc0` and a
    /// positive result through `FUN_0047c080`; those routines additionally
    /// redistribute amounts among neighboring map-object records.
    pub fn source_transfer_delta(self, inputs: SourceKind13TransferInputs) -> i32 {
        let group = usize::from(self.population_group);
        let Some(&group_satisfaction) = inputs.satisfaction_by_group.get(group) else {
            return 0;
        };
        let state = self.state_byte();
        let lifecycle_low = self.lifecycle_flags as u8;
        let aggregate_satisfaction = inputs.overall_satisfaction;

        // `FUN_0047b410`: when the record is not yet matured (`state & 0x80 == 0`)
        // and byte `+0x09` bit 2 (`lifecycle_flags & 0x400`, set only by a
        // neighbouring kind-8 object) is clear, the source `goto`s the decay
        // path with a zero source-satisfaction, bypassing the growth gate.
        let mut skip_growth = false;
        let (current_satisfaction, source_satisfaction) = if state & 0x80 == 0 {
            let current = if group == 0 { group_satisfaction } else { 0 };
            if self.lifecycle_flags & 0x0400 == 0 {
                skip_growth = true;
                (current, 0)
            } else {
                (current, aggregate_satisfaction)
            }
        } else {
            (group_satisfaction, aggregate_satisfaction)
        };

        if !skip_growth
            && aggregate_satisfaction > 0x57
            && current_satisfaction > 0x6b
            && state & 0x40 != 0
            && lifecycle_low & 3 == 0
        {
            if aggregate_satisfaction > 0x7f
                && current_satisfaction > 0x7f
                && !inputs.growth_blocked
            {
                let growth = (128 - i32::from(source_kind13_growth_curve(aggregate_satisfaction)))
                    * i32::from(source_kind13_variant_growth(self.variant));
                return (growth + 127) >> 7;
            }
            return 0;
        }

        let decay_score = ((i32::from(source_kind13_group_curve(current_satisfaction))
            + i32::from(source_kind13_satisfaction_curve(source_satisfaction)))
            * i32::from(source_kind13_variant_decay(self.variant)))
            >> 7;
        let state_penalty = if state & 0x40 == 0 { 0x40 } else { 0 };
        let lifecycle_penalty = match lifecycle_low & 3 {
            1 => 0x100,
            2 => 0xc0,
            _ => 0,
        };
        -decay_score - state_penalty - lifecycle_penalty
    }

    /// Convert `FUN_0047b410`'s score into the fixed-point amount passed to
    /// `FUN_0047bbc0` or `FUN_0047c080` by the phase dispatcher. The source
    /// scales decay by four and growth by six, both over 128, after replacing
    /// the root amount with its whole-resident count.
    pub fn source_dispatch_amount_delta(self, inputs: SourceKind13TransferInputs) -> i32 {
        let score = self.source_transfer_delta(inputs);
        let residents = i32::from(self.amount >> 6);
        if score < 0 {
            -((residents * -score * 4) / 128)
        } else {
            (residents * score * 6) / 128
        }
    }
}

fn source_kind13_linear_curve(index: u8, pieces: &[(u8, u8, i32, i32)]) -> u8 {
    // `FUN_00403370` fills each segment's half-open ramp `[start, end)` and then
    // writes index `end` with the exact terminal. Because the segments run in
    // source order, a shared boundary (one segment's `end` == the next
    // segment's `start`) is overwritten by the later segment's exact initial
    // value, not the earlier segment's truncated ramp. Select the segment whose
    // half-open ramp covers `index`; the final endpoint falls through to the
    // last segment's terminal.
    for &(start, end, initial, terminal) in pieces {
        if index < start || index >= end {
            continue;
        }
        let span = i32::from(end) - i32::from(start);
        let step = (terminal - initial) * 0x100 / span;
        let fixed = initial * 0x100 + (i32::from(index) - i32::from(start)) * step;
        return (fixed >> 8).clamp(0, 0xff) as u8;
    }
    if let Some(&(_, end, _, terminal)) = pieces.last() {
        if index == end {
            return terminal.clamp(0, 0xff) as u8;
        }
    }
    0
}

fn source_kind13_growth_curve(satisfaction: u8) -> u8 {
    source_kind13_linear_curve(satisfaction, &[(0, 0x80, 0x200, 0)])
}

fn source_kind13_satisfaction_curve(satisfaction: u8) -> u8 {
    source_kind13_linear_curve(
        satisfaction,
        &[
            (0, 0x33, 0xc0, 0x40),
            (0x33, 0x58, 0x40, 0x13),
            (0x58, 0x80, 0x13, 0),
        ],
    )
}

fn source_kind13_group_curve(satisfaction: u8) -> u8 {
    source_kind13_linear_curve(
        satisfaction,
        &[
            (0, 0x19, 0x6c, 0x46),
            (0x19, 0x33, 0x46, 0x20),
            (0x33, 0x58, 0x20, 0x0c),
            (0x58, 0x80, 0x0c, 0),
        ],
    )
}

fn source_kind13_variant_growth(variant: u8) -> u8 {
    source_kind13_linear_curve(variant & 0x0f, &[(0, 6, 0xa0, 0x73)])
}

fn source_kind13_variant_decay(variant: u8) -> u8 {
    source_kind13_linear_curve(variant & 0x0f, &[(0, 3, 0x66, 0x80), (3, 6, 0x80, 0xc0)])
}

/// Slot count of the source `DAT_005a77e8` kind-13 location table.
pub const SOURCE_KIND13_LOCATION_TABLE_SLOTS: usize = 0x1040;
pub const SOURCE_KIND13_PHASE_CLOCKS: usize = 16;
pub const SOURCE_KIND13_PHASE_BASE_MS: u32 = 15_000;
pub const SOURCE_KIND13_PHASE_STRIDE_MS: u32 = 64;
pub const SOURCE_KIND13_DISPATCH_RECORDS_PER_UPDATE: usize = 0x46;
/// `BGRUPPE.Maxwohn` from the five shipped `haeuser.cod` population rows.
pub const SOURCE_KIND13_MAX_RESIDENTS: [u16; 5] = [2, 6, 15, 25, 40];
/// Source `DAT_0061fa4c` values produced by `Maxwohn << 6` during the
/// BGRUPPE loader. Kind-13 record amounts use this 1/64-resident scale.
pub const SOURCE_KIND13_AMOUNT_CAPACITIES: [u16; 5] = [0x80, 0x180, 0x3c0, 0x640, 0xa00];
/// The seven contiguous `Ware` slots at `DAT_0061fa5c..=DAT_0061fa74`
/// sampled by `FUN_0047f0c0`: tobacco products through jewelry.
pub const SOURCE_CITY_LUXURY_WARE_SLOTS: [u8; 7] = [0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15];
/// `BGRUPPE.Prozent`, compiled by the haeuser loader as `Prozent * 128 / 100`
/// at `DAT_0061fa40 + BGruppe * 0x48`.
pub const SOURCE_CITY_GROUP_FULFILLMENT_TARGETS: [u8; 5] = [0, 60, 90, 99, 107];
/// Nonzero `BGRUPPE_WARE` selectors in the shipped haeuser configuration.
/// Columns follow [`SOURCE_CITY_LUXURY_WARE_SLOTS`].
pub const SOURCE_CITY_GROUP_LUXURY_REQUIREMENTS: [[bool; 7]; 5] = [
    [false, false, false, false, false, false, false],
    [false, false, false, true, true, false, false],
    [true, true, false, true, true, false, false],
    [true, true, true, true, true, false, false],
    [true, true, true, true, false, true, true],
];

/// The eight demand-slot goods at city record `+0x150 + 0x0c*slot`.
/// `FUN_0047f8a0` maps slot to source ware `slot + 0x0e`
/// (`1602_exe.c:91331`), i.e. NAHRUNG through SCHMUCK in ware-table
/// order; `FUN_0047f7b0` (`:91215-91230`) confirms the same mapping for
/// the per-ware demand display.
pub const SOURCE_CITY_DEMAND_WARE_GOODS: [crate::types::Good; 8] = [
    crate::types::Good::Food,
    crate::types::Good::TobaccoProducts,
    crate::types::Good::Spices,
    crate::types::Good::Cocoa,
    crate::types::Good::Alcohol,
    crate::types::Good::Cloth,
    crate::types::Good::Clothing,
    crate::types::Good::Jewelry,
];

/// Per-group, per-slot demand weights at `DAT_0061fa58 + BGruppe*0x48`
/// (columns follow [`SOURCE_CITY_DEMAND_WARE_GOODS`]; column zero is the
/// NAHRUNG slot, which no BGRUPPE_WARE entry populates). The haeuser
/// loader stores `ftol(Ware_float * 8192) / 600` (`1602_exe.c:66650`,
/// with the parser's 19.13 fixed-point float encoding), so the shipped
/// `0.2/0.5/0.6/0.7/0.8` coefficients compile to `2/6/8/9/10`. Verified
/// bit-exact against the running original's table with a winedbg read of
/// `DAT_0061fa40..+0x168` (2026-08-14).
pub const SOURCE_CITY_WARE_DEMAND_WEIGHTS: [[i32; 8]; 5] = [
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 6, 8, 0, 0],
    [0, 6, 6, 0, 8, 9, 0, 0],
    [0, 8, 8, 9, 9, 10, 0, 0],
    [0, 8, 8, 8, 10, 0, 6, 2],
];

/// `DAT_0049af2c`: the global NAHRUNG demand rate added per effective
/// resident per city cycle (`1602_exe.c:91422`). Compiled from
/// haeuser.cod `Nahrung: 1.3` ("Verbrauch je 100 Einwohner") as
/// `ftol(1.3 * 8192) / 600 = 17`; read live as 17. At the `15/16` decay
/// equilibrium this pulls `17*pop/256` store units (1/32 t) per 10 s
/// cycle — 1.3 t per 100 residents per minute, matching the cod comment.
pub const SOURCE_CITY_FOOD_DEMAND_RATE: i32 = 17;

/// `DAT_0061fa48 + BGruppe*0x48`: the `FUN_0047f3b0` satisfaction scale.
/// `FUN_00462d50` (`1602_exe.c:68796-68813`) compiles it after loading as
/// `fulfillment_target * demanded_ware_count`, counting nonzero weights
/// across all eight demand slots. Read live as `[0, 120, 360, 495, 642]`
/// (counts `[0, 2, 4, 5, 6]`).
pub const SOURCE_CITY_GROUP_SATISFACTION_SCALES: [u32; 5] = [0, 120, 360, 495, 642];

/// Fixed count of city records at `DAT_005dbae0`.
pub const SOURCE_CITY_RECORD_SLOTS: usize = 0x4b;

/// `FUN_00404d70`: fill one service-radius row of `DAT_005b7460`.
/// `row[dy]` is the maximum `dx` still inside the service circle. The
/// source's integer midpoint fill also mirrors each stepped-down column
/// symmetrically. Verified bit-exact against the running original's
/// compiled rows (radii 0..=10, 15..=16, 23..=24 sampled live).
pub fn source_service_radius_row(radius: u8) -> Vec<u8> {
    let r = i32::from(radius);
    let mut row = vec![0u8; radius as usize + 1];
    let mut dx = r;
    let mut dy = 0i32;
    let mut acc = r * r;
    let mut limit = (r - 1) * r;
    let mut step = 2 * r;
    let mut two_dy = 0i32;
    while dy <= dx {
        row[dy as usize] = dx as u8;
        acc += -1 - two_dy;
        // The source compares unsigned (`ja`), so a negative accumulator
        // also skips the step-down.
        if (acc as u32) <= (limit as u32) {
            dx -= 1;
            if let Some(cell) = row.get_mut((dx + 1) as usize) {
                *cell = dy as u8;
            }
            step -= 2;
            limit -= step;
        }
        dy += 1;
        two_dy += 2;
    }
    if radius == 1 {
        row[1] = 1;
    }
    row
}

/// `FUN_00478630`'s 17×17 grid at `DAT_005a6af0`:
/// `ftol(sqrt(dx² + dy²) × 0.375 + 0.5)` with the CRT's
/// truncate-toward-zero `ftol` (multiplier and bias read from the
/// executable's rdata at `0x496458`/`0x496310`; grid verified live).
/// The house-coverage scan takes the minimum class over the covering
/// marketplaces into the kind-13 record's variant nibble, which the
/// `FUN_0047b410` growth/decay curves then consume — houses near a
/// marketplace grow faster.
pub fn source_market_distance_class(dx: u8, dy: u8) -> u8 {
    let dx = f64::from(dx.min(16));
    let dy = f64::from(dy.min(16));
    ((dx * dx + dy * dy).sqrt() * 0.375 + 0.5) as u8
}

/// `FUN_00482120`'s infrastructure coverage bits: production kind code of
/// the covering building → kind-13 lifecycle flag. The transition
/// predicates (`source_transition_active_for_group`) read these — e.g.
/// pioneers need a chapel (`0x0004`), settlers→citizens need tavern +
/// chapel/church + school/college.
pub const SOURCE_HOUSE_INFRA_LIFECYCLE_BITS: [(u8, u16); 10] = [
    (0x11, 0x0010), // WIRT (tavern)
    (0x12, 0x0004), // KAPELLE (chapel)
    (0x13, 0x0008), // KIRCHE (church)
    (0x14, 0x0040), // BADEHAUS (bathhouse)
    (0x15, 0x0080), // THEATER
    (0x16, 0x0200), // KLINIK (doctor)
    (0x17, 0x0020), // SCHULE (school)
    (0x18, 0x0100), // HOCHSCHULE (college)
    (0x19, 0x0800), // GALGEN (gallows)
    (0x1a, 0x1000), // BRUNNEN (well)
];

/// Mutable source-city fields consumed by `FUN_0047f8a0` and
/// `FUN_00480370`. The source keeps these in a fixed 75-record pool; an
/// inactive slot corresponds to the `island == 0xff` sentinel tested by the
/// dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceCityRecord {
    /// Source island-table index at city-record byte `+0x18`.
    pub island_id: u8,
    /// Island-local city slot at city-record byte `+0x19`. `FUN_00468ce0`
    /// supplies the first unoccupied one of the island's eight city pointers;
    /// map cells retain this same three-bit selector.
    pub source_owner: u8,
    /// Source player slot at city-record byte `+0x1a`.
    pub owner_slot: u8,
    /// Low-three-bit phase byte at `+0x1b`, updated after city processing.
    pub phase: u8,
    /// Source city byte `+0x1c`, written from the successful construction
    /// command's target x coordinate by `FUN_00468e10`.
    #[serde(default)]
    pub tile_x: u8,
    /// Source city byte `+0x1d`, written from the construction target y
    /// coordinate by `FUN_00468e10`.
    #[serde(default)]
    pub tile_y: u8,
    /// Source city dword `+0x1e0`: `FUN_00468e10` sets this to source time
    /// plus 600 ticks when it allocates the record.
    #[serde(default)]
    pub ready_at_ticks: u32,
    /// The five BGRUPPE populations; `FUN_0047f1f0(city, 1)` sums entries
    /// one through four for the kind-12 city-dispatch threshold.
    pub tier_population: [u32; 5],
    /// Source city dword `+0x218`. `FUN_0047f790` adds this live resident
    /// amount to the five dwords at `+0x220..+0x230`; it changes when the
    /// source city-figure transfer handlers move residents between cities.
    #[serde(default)]
    pub resident_amount: u32,
    /// Source city property-table entry 13 at u16 `+0xc6`, consulted by
    /// `FUN_00423710` before it expands a controller's figure capacity from
    /// the weighted roster. `FUN_00468e10` clears the complete 600-byte city
    /// record on allocation, so this starts at zero rather than coming from
    /// `STADT4`; `FUN_0047a960` and `FUN_0047a9b0` subsequently replace it
    /// through the kind-`0x14`, subtype-`0x12` city-property event path.
    #[serde(default)]
    pub controller_figure_capacity_metric: u16,
    /// Source city bytes `+0x164 + 0x0c * i` sampled by `FUN_0047f0c0` for
    /// luxury ware slots `0x0f + i`. `FUN_0047f8a0` recomputes each byte
    /// every city cycle as the slot's supply/demand fulfillment ratio.
    #[serde(default)]
    pub luxury_satisfaction: [u8; 7],
    /// Source city byte `+0x158`: the NAHRUNG (slot zero) fulfillment
    /// ratio, the only demand-slot byte outside `luxury_satisfaction`.
    /// `FUN_0047f8a0` copies it to `+0x255` every cycle (`:91375`).
    #[serde(default)]
    pub food_fulfillment: u8,
    /// Source city dwords `+0x150 + 0x0c*slot`: smoothed ware-demand
    /// accumulators in 1/256 store units, grown by `weight * residents`
    /// each cycle and decayed by `15/16` (`FUN_0047f8a0` `:91411-91434`).
    #[serde(default)]
    pub ware_demand: [i32; 8],
    /// Source city dwords `+0x154 + 0x0c*slot`: the matching supply
    /// accumulators, credited `pull << 8` per withdrawn store unit and
    /// decayed alongside the demand accumulators.
    #[serde(default)]
    pub ware_supply: [i32; 8],
    /// Source city bytes `+0x159..+0x15b` per slot: the three-deep
    /// fulfillment history produced by the cycle's `memmove` (`:91349`),
    /// newest first.
    #[serde(default)]
    pub ware_fulfillment_history: [[u8; 3]; 8],
    /// Source city byte `+0x256`: the ware index (`0x0e..=0x15`) of the
    /// worst declining demand slot this cycle, zero when none declined
    /// (`:91355-91376`).
    #[serde(default)]
    pub worst_ware_slot: u8,
    /// Source city bytes `+0x24d..+0x251`. `FUN_00468ce0` initializes all
    /// five to `0x80`, and `FUN_0047f400` applies each to its group target.
    #[serde(default = "source_city_initial_satisfaction_weights")]
    pub satisfaction_weights: [u8; 5],
    /// Source city u16 `+0x200`; `FUN_0047f8a0` decays it by `255 / 256`
    /// before `FUN_0047f400` converts it into a satisfaction denominator.
    #[serde(default)]
    pub satisfaction_pressure: u16,
    /// Source city bytes `+0x248..+0x24c`, written by `FUN_0047f850`.
    /// Live-verified 2026-08-14: every Exile city (human, AI, trader,
    /// native, pirate) reads `0x80` per group at clock 0, so scenario
    /// cities initialize satisfied. Cities whose owner never passes the
    /// local-player satisfaction gate keep this value — with a zero
    /// default the kind-13 decay path immediately drained every AI city.
    #[serde(default = "source_city_initial_group_satisfaction")]
    pub satisfaction_by_group: [u8; 5],
    /// Source city byte `+0x255`, supplied by the city-demand dispatcher to
    /// `FUN_0047b410` as its cross-group satisfaction operand. Initialized
    /// satisfied for the same reason as `satisfaction_by_group`.
    #[serde(default = "source_city_initial_overall_satisfaction")]
    pub overall_satisfaction: u8,
    /// Source city u16 `+0x1fe`; a nonzero value suppresses positive
    /// kind-13 amount changes in `FUN_0047b410`.
    #[serde(default)]
    pub growth_blocked: bool,
    /// Source city byte `+0x257`, bit zero. `FUN_0047c080` permits a
    /// material-gated BGruppe promotion only while this bit is clear.
    #[serde(default)]
    pub promotion_blocked: bool,
    /// Source city u16s `+0x234..+0x23c`, one pending kind-13 promotion
    /// amount per target BGruppe in whole residents.
    #[serde(default)]
    pub promotion_reservations: [u16; 5],
    /// Source city bytes `+0x23e..+0x242` and `+0x243..+0x247`, the origin
    /// coordinates paired with a pending promotion reservation.
    #[serde(default)]
    pub promotion_reservation_positions: [(u8, u8); 5],
}

const fn source_city_initial_satisfaction_weights() -> [u8; 5] {
    [0x80; 5]
}

const fn source_city_initial_group_satisfaction() -> [u8; 5] {
    [0x80; 5]
}

const fn source_city_initial_overall_satisfaction() -> u8 {
    0x80
}

impl Default for SourceCityRecord {
    fn default() -> Self {
        Self {
            island_id: u8::MAX,
            source_owner: 0,
            owner_slot: 0,
            phase: 0,
            tile_x: 0,
            tile_y: 0,
            ready_at_ticks: 0,
            tier_population: [0; 5],
            resident_amount: 0,
            controller_figure_capacity_metric: 0,
            luxury_satisfaction: [0; 7],
            food_fulfillment: 0,
            ware_demand: [0; 8],
            ware_supply: [0; 8],
            ware_fulfillment_history: [[0; 3]; 8],
            worst_ware_slot: 0,
            satisfaction_weights: source_city_initial_satisfaction_weights(),
            satisfaction_pressure: 0,
            satisfaction_by_group: source_city_initial_group_satisfaction(),
            overall_satisfaction: source_city_initial_overall_satisfaction(),
            growth_blocked: false,
            promotion_blocked: false,
            promotion_reservations: [0; 5],
            promotion_reservation_positions: [(0, 0); 5],
        }
    }
}

impl SourceCityRecord {
    /// Replay `FUN_0047a960(city, 0x0d, amount)`. The event dispatcher
    /// `FUN_00480680` truncates its u32 payload to city property 13's u16
    /// storage after adding `amount` to the prior value.
    pub fn source_add_controller_figure_capacity_metric(&mut self, amount: u32) {
        self.controller_figure_capacity_metric = self
            .controller_figure_capacity_metric
            .wrapping_add(amount as u16);
    }

    /// Replay `FUN_0047a9b0(city, 0x0d, amount)`. As in the executable, the
    /// u32 subtraction is truncated by `FUN_00480680` when it reaches the
    /// property-table entry at `+0xc6`.
    pub fn source_sub_controller_figure_capacity_metric(&mut self, amount: u32) {
        self.controller_figure_capacity_metric = self
            .controller_figure_capacity_metric
            .wrapping_sub(amount as u16);
    }

    /// `FUN_0047f790(city)`: the five `+0x220` BGRUPPE totals plus the live
    /// `+0x218` resident amount, with the executable's wrapping u32 sum.
    pub fn source_resident_total(self) -> u32 {
        self.tier_population
            .into_iter()
            .fold(self.resident_amount, u32::wrapping_add)
    }

    /// Replay `FUN_0047f0c0`, `FUN_0047f400`, and `FUN_0047f850` for the
    /// five population groups. The caller supplies the live city demand
    /// bytes; the BGRUPPE selectors and targets are fixed source data.
    pub fn refresh_group_satisfaction(&mut self) {
        self.satisfaction_by_group = std::array::from_fn(|group| {
            let fulfilled: u32 = SOURCE_CITY_GROUP_LUXURY_REQUIREMENTS[group]
                .iter()
                .zip(self.luxury_satisfaction)
                .filter_map(|(&required, satisfaction)| required.then_some(u32::from(satisfaction)))
                .sum();
            let denominator = source_city_group_satisfaction_denominator(
                self.satisfaction_pressure,
                self.satisfaction_weights[group],
                SOURCE_CITY_GROUP_FULFILLMENT_TARGETS[group],
            );
            if denominator == 0 {
                0x80
            } else {
                ((fulfilled << 7) / denominator).min(0x80) as u8
            }
        });
    }

    /// The current fulfillment byte for one demand slot: `+0x158` for
    /// NAHRUNG, `+0x158 + 0x0c*slot` (`luxury_satisfaction`) otherwise.
    pub fn ware_fulfillment_current(&self, slot: usize) -> u8 {
        if slot == 0 {
            self.food_fulfillment
        } else {
            self.luxury_satisfaction[slot - 1]
        }
    }

    fn set_ware_fulfillment_current(&mut self, slot: usize, value: u8) {
        if slot == 0 {
            self.food_fulfillment = value;
        } else {
            self.luxury_satisfaction[slot - 1] = value;
        }
    }

    /// One `FUN_0047f8a0` demand/consumption cycle (`1602_exe.c:91321-91437`)
    /// for a city whose owner passes the local-player satisfaction gate.
    ///
    /// `pull(slot, want)` must withdraw up to `want` whole store units
    /// (1/32 good) of [`SOURCE_CITY_DEMAND_WARE_GOODS`]`[slot]` from the
    /// city's store and return the withdrawn amount; the source computes
    /// `want` itself from `stock - reserved` before calling
    /// `FUN_0047a9b0`, so the callback applies the same clamp.
    ///
    /// The trailing fulfillment-warning messages (`FUN_00430d50`) and the
    /// population/satisfaction UI events are presentation-only and not
    /// replayed; the `+0x1f0`/`+0x1f4` `89/90` decays touch counters this
    /// record does not yet model.
    pub fn source_ware_economy_cycle(&mut self, mut pull: impl FnMut(usize, u16) -> u16) {
        // Per-slot pull, history shift, and fulfillment ratio
        // (`:91325-91374`). `local_2c` tracks the declining slot with the
        // lowest fresh ratio as a ware index (slot + 0x0e).
        let mut worst_ware_slot = 0u8;
        for slot in 0..8 {
            let demand = self.ware_demand[slot];
            let mut supply = self.ware_supply[slot];
            let deficit = demand - supply;
            if deficit > 0 {
                // `((deficit + carry) >> 8) + 1`: floor division of the
                // positive deficit by the 256 accumulator scale, plus one.
                let want = ((deficit >> 8) + 1).min(i32::from(u16::MAX)) as u16;
                let got = pull(slot, want);
                supply += i32::from(got) << 8;
            }
            // `memmove(+0x159, +0x158, 3)`: history becomes
            // [current, old newest, old middle]; the oldest byte drops.
            let current = self.ware_fulfillment_current(slot);
            let history = self.ware_fulfillment_history[slot];
            self.ware_fulfillment_history[slot] = [current, history[0], history[1]];
            // `(supply << 7) / demand` in 32-bit int arithmetic; the
            // result is compared unsigned against 0x80, so a negative
            // quotient also clamps to full.
            let fulfillment = if demand == 0 {
                0x80
            } else {
                let ratio = supply.wrapping_shl(7).wrapping_div(demand);
                if ratio as u32 > 0x80 {
                    0x80
                } else {
                    ratio as u8
                }
            };
            self.ware_supply[slot] = supply;
            self.set_ware_fulfillment_current(slot, fulfillment);
            // Declining-trend tracking (`:91355-91372`): fresh ratio below
            // last cycle's, which itself was below the cycle before.
            let history = self.ware_fulfillment_history[slot];
            if fulfillment < history[0] && history[0] < history[1] {
                let tracked = usize::from(worst_ware_slot).checked_sub(0x0e);
                if tracked.is_none_or(|t| fulfillment < self.ware_fulfillment_current(t)) {
                    worst_ware_slot = (slot + 0x0e) as u8;
                }
            }
        }
        self.overall_satisfaction = self.food_fulfillment;
        self.worst_ware_slot = worst_ware_slot;

        // Per-group satisfaction and demand accumulation, group four down
        // to zero (`:91377-91426`).
        for group in (0..5).rev() {
            let fulfilled: u32 = (1..8)
                .filter(|&slot| SOURCE_CITY_WARE_DEMAND_WEIGHTS[group][slot] != 0)
                .map(|slot| u32::from(self.ware_fulfillment_current(slot)))
                .sum();
            let denominator = source_city_group_satisfaction_denominator_scaled(
                self.satisfaction_pressure,
                self.satisfaction_weights[group],
                SOURCE_CITY_GROUP_SATISFACTION_SCALES[group],
            );
            self.satisfaction_by_group[group] = if denominator == 0 {
                0x80
            } else {
                ((fulfilled << 7) / denominator).min(0x80) as u8
            };
            let population = self.tier_population[group];
            if population == 0 {
                // `:91404-91406`: an empty group's tax weight resets to
                // the 0x80 default, effective from the next cycle.
                self.satisfaction_weights[group] = 0x80;
            }
            // Effective consumers (`:91407-91410`): residents reserved to
            // promote into this group consume at its weights already;
            // those reserved to leave for the next group stop counting.
            let mut effective = population as i32;
            if group < 4 {
                effective -= i32::from(self.promotion_reservations[group + 1]);
            }
            effective += i32::from(self.promotion_reservations[group]);
            if effective != 0 {
                for slot in 1..8 {
                    self.ware_demand[slot] +=
                        SOURCE_CITY_WARE_DEMAND_WEIGHTS[group][slot] * effective;
                }
                self.ware_demand[0] += SOURCE_CITY_FOOD_DEMAND_RATE * effective;
            }
        }

        // `15/16` accumulator decay for every slot (`:91427-91434`,
        // unsigned in the source) and the `255/256` satisfaction-pressure
        // decay (`:91437`).
        for slot in 0..8 {
            self.ware_demand[slot] = ((self.ware_demand[slot] as u32).wrapping_mul(15) >> 4) as i32;
            self.ware_supply[slot] = ((self.ware_supply[slot] as u32).wrapping_mul(15) >> 4) as i32;
        }
        self.satisfaction_pressure = (u32::from(self.satisfaction_pressure) * 0xff >> 8) as u16;
    }

    /// Assemble the source city operands consumed by `FUN_0047b410`.
    pub const fn source_kind13_transfer_inputs(self) -> SourceKind13TransferInputs {
        SourceKind13TransferInputs {
            satisfaction_by_group: self.satisfaction_by_group,
            overall_satisfaction: self.overall_satisfaction,
            growth_blocked: self.growth_blocked,
        }
    }
}

/// `FUN_0047f400`: required fulfillment for one population group.
fn source_city_group_satisfaction_denominator(
    pressure: u16,
    weight: u8,
    fulfillment_target: u8,
) -> u32 {
    source_city_group_satisfaction_denominator_scaled(
        pressure,
        weight,
        u32::from(fulfillment_target),
    )
}

/// `FUN_0047f3b0` (`1602_exe.c:90939`): identical to `FUN_0047f400` but
/// scaled by the compiled `DAT_0061fa48` target-times-ware-count field.
/// The regular `FUN_0047f8a0` cycle divides the *sum* of the group's
/// fulfillment bytes by this, where the tax-change path (`FUN_0047f850`)
/// divides their *average* by the plain target.
fn source_city_group_satisfaction_denominator_scaled(
    pressure: u16,
    weight: u8,
    scale: u32,
) -> u32 {
    let pressure_steps = u32::from(pressure >> 5);
    let curve_input = 0x80_u32.saturating_sub(pressure_steps) * u32::from(weight);
    let curve = source_city_satisfaction_curve((curve_input >> 7) as u8);
    (curve * scale) >> 7
}

/// Runtime `DAT_0055e780` initialized by the nine `FUN_004033d0` calls in
/// `FUN_0047f8a0`. The source stores each value after truncating its 8.8
/// linear interpolation, including the terminal point of every segment.
fn source_city_satisfaction_curve(index: u8) -> u32 {
    const SEGMENTS: &[(u8, u8, u32, u32)] = &[
        (0x00, 0x4c, 0x4c, 0x59),
        (0x4c, 0x66, 0x59, 0x66),
        (0x66, 0x73, 0x66, 0x6c),
        (0x73, 0x80, 0x6c, 0x80),
        (0x80, 0x8c, 0x80, 0x93),
        (0x8c, 0xa6, 0x93, 0xd5),
        (0xa6, 0xb3, 0xd5, 0x100),
        (0xb3, 0xc0, 0x100, 0x160),
        (0xc0, 0xfe, 0x160, 0x280),
    ];

    let index = u32::from(index);
    // Each successive `FUN_004033d0` call rewrites the boundary it shares
    // with its predecessor, so a shared index takes the later segment's
    // exact initial value rather than the earlier truncated interpolation.
    for &(start, end, initial, terminal) in SEGMENTS.iter().rev() {
        let start = u32::from(start);
        let end = u32::from(end);
        if index >= start && index <= end {
            let step =
                ((terminal * 0x100) as i32 - (initial * 0x100) as i32) / (end - start) as i32;
            return ((initial * 0x100) as i32 + (index - start) as i32 * step) as u32 >> 8;
        }
    }
    0
}

/// One live source kind-4 land-figure contribution to an island's owner
/// counter at `island + 0x4a + owner`.
///
/// `FUN_0045fac0` initializes this from `SOLDAT3`, and `FUN_00453da0`
/// increments the original counter for each kind-4 figure. Keeping the
/// source key separately from local combat entities preserves the authored
/// scenario occupancy until its full figure state is reconstructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SourceKind4Occupant {
    /// Type-4 runtime slot stored in the source `SOLDAT3` record.
    pub runtime_slot: u16,
    /// Compiled figure definition used by the source type-4 dispatcher.
    pub figure_definition_id: u16,
    /// Byte zero of the source's live type-4 runtime slot. `FUN_00456d00`
    /// supplies it to `FUN_004581f0` as the local raw-grid route radius.
    #[serde(default = "crate::combat::default_source_kind4_route_radius")]
    pub route_radius: u8,
    /// Live `FUN_004581f0` failed-route counter at runtime-slot offset
    /// `+0x22`. The SOLDAT3 loader zeroes it before restoring the record.
    #[serde(default)]
    pub route_retry_count: u8,
    /// `SOLDAT3 +0x130` packed type-4 direction program generated by
    /// `FUN_004581f0`.
    #[serde(
        default = "crate::combat::default_source_kind4_route_program",
        with = "crate::serde_util::byte_array"
    )]
    pub route_program: [u8; crate::combat::SOURCE_KIND4_ROUTE_PROGRAM_CAPACITY],
    /// `SOLDAT3 +0x02` cursor of the active type-4 direction program.
    #[serde(default)]
    pub route_program_cursor: u8,
    /// Raw IEEE-754 bits of type-4 runtime `+0x14`, the terminal-route
    /// residual consumed by `FUN_00451890` at runtime `+0x18`.
    #[serde(default)]
    pub idle_remaining_bits: u32,
    /// Type-4 anchor read by `FUN_00456d00` for idle native movement.
    pub origin_descriptor: SourceTargetDescriptor,
    /// Authored world-grid coordinate.
    pub position: (u16, u16),
    pub island_id: u8,
    pub owner: u8,
    pub direction: u8,
    /// Initial source animation selected by `FUN_00446d90`.
    pub animation_state: u8,
    /// Runtime offset `+0x126`, loaded from `SOLDAT3 +0x1c`. When state bit
    /// zero is set and no live target remains, `FUN_00458190` advances this
    /// selector through the two descriptors in `state_payload`.
    #[serde(default)]
    pub state_selector: u8,
    /// Type-4 descriptor consumed by `FUN_00456d00` after scenario load.
    pub state_descriptor: SourceTargetDescriptor,
    /// `DAT_0051c688`: source-clock timestamp used by `FUN_00456d00` for
    /// its 20-tick idle-target gate.
    pub idle_timestamp_ticks: u32,
    /// Low two type-4 state bits loaded from `SOLDAT3`.
    pub state_flags: u8,
    /// Type-4 per-slot state payload from `SOLDAT3`.
    pub state_payload: [u8; 8],
    pub active: bool,
}

impl SourceKind4Occupant {
    /// Resolve the authored type-4 figure definition retained by this
    /// mutable source record.
    pub const fn definition(&self) -> Option<LandFigureDefinition> {
        LandFigureDefinition::from_id(self.figure_definition_id)
    }
}

/// Fixed source city-record pool. Scenario STADT4 records fill ascending
/// slots in scenario-island order; remaining records retain the inactive
/// sentinel represented by `None`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceCityTable {
    slots: Vec<Option<SourceCityRecord>>,
}

impl Default for SourceCityTable {
    fn default() -> Self {
        Self {
            slots: vec![None; SOURCE_CITY_RECORD_SLOTS],
        }
    }
}

impl SourceCityTable {
    /// Source city slots are traversed in their physical pool order.
    pub const fn slot_count() -> usize {
        SOURCE_CITY_RECORD_SLOTS
    }

    pub fn record(&self, slot: usize) -> Option<SourceCityRecord> {
        self.slots.get(slot).copied().flatten()
    }

    pub fn record_mut(&mut self, slot: usize) -> Option<&mut SourceCityRecord> {
        self.slots.get_mut(slot)?.as_mut()
    }

    /// Resolve the active city record selected by a kind-13 root. The source
    /// lookup keys both the island table id and the map-owner byte retained
    /// in the root record.
    pub fn slot_for_root(&self, island_id: u8, source_owner: u8) -> Option<usize> {
        self.slots.iter().position(|record| {
            record.is_some_and(|city| {
                city.island_id == island_id && city.source_owner == source_owner
            })
        })
    }

    /// The direct population arm of controller initialization in
    /// `FUN_0040f580`: scan physical city slots in order and retain the last
    /// city owned by `owner_slot` whose BGRUPPE entries 2 through 4 sum to at
    /// least ten. This is `FUN_0040f430(city, 2, 10)`.
    pub fn source_controller_populated_city_slot(&self, owner_slot: u8) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, record)| {
                let city = record.as_ref()?;
                (city.owner_slot == owner_slot
                    && city.tier_population[2..].iter().copied().sum::<u32>() >= 10)
                    .then_some(slot)
            })
            .last()
    }

    /// Replay the physical-city selection loop in `FUN_0040f580`. A city
    /// with at least ten tier-2-through-4 residents always replaces the
    /// current selection; otherwise its island's raw `FUN_0040e8b0` score
    /// replaces the selection only when strictly greater.
    pub fn source_controller_city_slot(
        &self,
        owner_slot: u8,
        mut island_score: impl FnMut(u8, u8) -> i32,
    ) -> Option<usize> {
        let mut selected = None;
        let mut selected_score = 0;
        for (slot, record) in self.slots.iter().enumerate() {
            let Some(city) = record else {
                continue;
            };
            if city.owner_slot != owner_slot {
                continue;
            }
            let score = island_score(city.island_id, city.source_owner);
            if city.tier_population[2..].iter().copied().sum::<u32>() >= 10
                || selected_score < score
            {
                selected = Some(slot);
                selected_score = score;
            }
        }
        selected
    }

    /// Allocate the city-record portion of `FUN_00468ce0` /
    /// `FUN_00468e10`. The executable reserves the first free physical
    /// record, assigns the first vacant island-local city pointer, and starts
    /// its `+0x1e0` readiness clock 600 source ticks in the future.
    pub fn allocate_source_city(
        &mut self,
        island_id: u8,
        tile_x: u8,
        tile_y: u8,
        owner_slot: u8,
        source_time_ticks: u32,
    ) -> Option<usize> {
        let source_owner = (0..8).find(|&candidate| {
            !self
                .slots
                .iter()
                .flatten()
                .any(|city| city.island_id == island_id && city.source_owner == candidate)
        })?;
        let slot = self.slots.iter().position(Option::is_none)?;
        self.slots[slot] = Some(SourceCityRecord {
            island_id,
            source_owner,
            owner_slot,
            tile_x,
            tile_y,
            ready_at_ticks: source_time_ticks.wrapping_add(600),
            ..SourceCityRecord::default()
        });
        Some(slot)
    }

    /// Restore one physical source city slot. Runtime scenario loading fills
    /// records in order; save replay and focused source audits may restore a
    /// specific slot directly.
    pub fn set_record(&mut self, slot: usize, record: Option<SourceCityRecord>) -> bool {
        let Some(destination) = self.slots.get_mut(slot) else {
            return false;
        };
        *destination = record;
        true
    }

    fn insert_next(&mut self, record: SourceCityRecord) -> bool {
        let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *slot = Some(record);
        true
    }

    /// Active city records in source pool order, for source-audit tests.
    pub fn active_records(&self) -> Vec<SourceCityRecord> {
        self.slots.iter().flatten().copied().collect()
    }

    /// Count active source cities owned by one player. This mirrors the
    /// runtime PLAYER4 byte `+0x86`: `FUN_00468ce0` increments it after a
    /// successful city allocation and `FUN_00468ed0` decrements it when the
    /// island-local city pointer is released.
    pub fn source_city_count_for_owner(&self, owner_slot: u8) -> u8 {
        self.slots
            .iter()
            .flatten()
            .filter(|city| city.owner_slot == owner_slot)
            .count()
            .try_into()
            .unwrap_or(u8::MAX)
    }

    /// Count active island-local city pointers. `FUN_00416370` uses the
    /// corresponding `+0xac..+0xc8` pointer table when its controller has at
    /// most one desired figure.
    pub fn source_city_count_on_island(&self, island_id: u8) -> u8 {
        self.slots
            .iter()
            .flatten()
            .filter(|city| city.island_id == island_id)
            .count()
            .try_into()
            .unwrap_or(u8::MAX)
    }

    /// Sum `FUN_0047f790` across every active source-city record owned by one
    /// player. This is the `local_14` / `local_18` accumulation in
    /// `FUN_00475c60` before its policy-specific modifiers.
    pub fn source_resident_total_for_owner(&self, owner_slot: u8) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|city| city.owner_slot == owner_slot)
            .fold(0_u32, |total, city| {
                total.wrapping_add(city.source_resident_total())
            })
    }
}

/// Build the source fixed city-record pool from scenario STADT4 records.
/// `FUN_00480370` indexes live island map cells using the record's island
/// table id, so this retains `Island::number`. Each parsed island has one
/// STADT4 city, which occupies local city-pointer slot zero at `+0x19`;
/// STADT4's owner slot belongs only in the city `+0x1a` player field.
pub fn source_cities_from_scenario(szs: &SzsFile) -> SourceCityTable {
    let mut cities = SourceCityTable::default();
    for island in &szs.islands {
        let Some(city) = island.city.as_ref() else {
            continue;
        };
        if !cities.insert_next(SourceCityRecord {
            island_id: island.number,
            source_owner: 0,
            owner_slot: city.owner_slot,
            phase: 0,
            tier_population: city.tier_population,
            ..SourceCityRecord::default()
        }) {
            break;
        }
    }
    cities
}

/// Extract the authored kind-4 land occupancy supplied by `SOLDAT3`.
///
/// The `SOLDAT3` loader calls `FUN_00453da0` after creating each record;
/// that helper increments the source per-island owner counter only when the
/// figure kind is 4. This list is the mutable local representation of those
/// counter contributions.
pub fn source_kind4_occupants_from_scenario(szs: &SzsFile) -> Vec<SourceKind4Occupant> {
    szs.land_figures
        .iter()
        .filter(|figure| figure.figure_kind == 4)
        .map(|figure| SourceKind4Occupant {
            runtime_slot: figure.runtime_slot,
            figure_definition_id: figure.figure_definition_id,
            route_radius: figure.route_radius,
            route_retry_count: 0,
            route_program: crate::combat::default_source_kind4_route_program(),
            route_program_cursor: 0,
            idle_remaining_bits: 0,
            origin_descriptor: SourceTargetDescriptor::from_bytes(figure.origin_descriptor),
            position: (figure.x, figure.y),
            island_id: figure.island_id,
            owner: figure.owner,
            direction: figure.direction,
            animation_state: figure.animation_state,
            state_selector: figure.state_selector,
            state_descriptor: SourceTargetDescriptor::from_bytes(figure.state_descriptor),
            idle_timestamp_ticks: figure
                .definition()
                .is_some_and(|definition| definition.family == LandFigureFamily::NativeSpearman)
                .then_some(8)
                .unwrap_or(0),
            state_flags: figure.state_flags,
            state_payload: figure.state_payload,
            active: true,
        })
        .collect()
}

/// Reconstruct the `FUN_00456d00` player globals from the scenario's raw
/// `PLAYER4 + 4` faction-state bytes.
pub fn source_kind4_dispatch_state_from_scenario(
    szs: &SzsFile,
) -> crate::combat::SourceKind4DispatchState {
    let faction_states = std::array::from_fn(|slot| {
        szs.players
            .get(slot)
            .map(|player| player.state_byte)
            .unwrap_or(0xff)
    });
    crate::combat::SourceKind4DispatchState::from_player4_faction_states(faction_states)
}

/// Reconstruct live combat units from authored type-4 `SOLDAT3` records.
///
/// The executable's figure-name table maps the compiled definition ID to a
/// player soldier ladder or the native `SPEER` ladder. The source runtime
/// slot remains attached to the combat unit so a later deletion path can
/// deactivate the matching owner-counter contribution as well.
pub fn land_units_from_scenario(szs: &SzsFile) -> Vec<crate::combat::MilitaryUnit> {
    use crate::combat::{MilitaryUnit, UnitType};

    szs.land_figures
        .iter()
        .filter(|figure| figure.figure_kind == 4)
        .filter_map(|figure| {
            let definition = figure.definition()?;
            let unit_type = match definition.family {
                LandFigureFamily::Infantry => UnitType::Infantry,
                LandFigureFamily::Cavalry => UnitType::Cavalry,
                LandFigureFamily::Musketeer => UnitType::Musketeer,
                LandFigureFamily::Cannoneer => UnitType::Cannon,
                LandFigureFamily::NativeSpearman => UnitType::NativeSpearman,
            };
            let mut unit = MilitaryUnit::new(
                unit_type,
                figure.owner,
                i32::from(figure.x),
                i32::from(figure.y),
            );
            // `FUN_0045fac0` constructs the live figure at raw / 2 + 0.25.
            unit.source_position_x = f32::from(figure.x) * 0.5 + 0.25;
            unit.source_position_y = f32::from(figure.y) * 0.5 + 0.25;
            unit.source_position_initialized = true;
            unit.source_island_id = Some(figure.island_id);
            unit.source_runtime_slot = Some(figure.runtime_slot);
            unit.source_live_runtime_slot = Some(figure.runtime_slot);
            unit.source_candidate_list_key = Some(figure.island_id);
            unit.source_figure_kind = Some(figure.figure_kind);
            unit.source_figure_definition_id = Some(definition.id);
            unit.source_energy = figure.source_energy;
            unit.source_kind6_target_descriptor_payload =
                Some([figure.state_descriptor[2], figure.state_descriptor[3]]);
            unit.source_route_radius = figure.route_radius;
            unit.source_origin_descriptor =
                Some(SourceTargetDescriptor::from_bytes(figure.origin_descriptor));
            unit.direction = figure.direction;
            let descriptor = SourceTargetDescriptor::from_bytes(figure.state_descriptor);
            if descriptor.kind() != 0 {
                unit.source_target_descriptor = Some(descriptor);
            }
            if let Some((x, y)) = descriptor.source_land_route_coordinate() {
                unit.target_x = x;
                unit.target_y = y;
            }
            Some(unit)
        })
        .collect()
}

/// The source hash table that supplies kind-12 civilian/worker allocation.
///
/// `FUN_00478b90` inserts each kind-13 root in the first free entry of its
/// 64-slot probe window. `FUN_00480370` samples the corresponding city slice
/// directly from this table, so a compact location list would alter both
/// collision placement and the shared `rand()` stream.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceKind13LocationTable {
    slots: Vec<Option<SourceKind13Location>>,
}

/// Result of the source `FUN_0047bbc0` decrease path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind13DecreaseResult {
    /// The source record and city tier total were updated in place. Neighbor
    /// transfer is already reflected in the table.
    Applied {
        remaining_amount: u16,
        redistributed_amount: u16,
    },
    /// The source selected a lower BGruppe and must emit an INSELHAUS
    /// replacement command. The root and city totals have already been
    /// changed in place; definition selection and command emission remain
    /// owned by the map-transition layer.
    DowngradeRequired {
        target_group: u8,
        remaining_amount: u16,
    },
}

/// Fixed-point construction operands read by the promotion branch of
/// `FUN_0047c080`. All six quantities are in the source's 1/32-good scale:
/// `HAUS_BAUKOST` stores its three costs at definition offsets
/// `+0x4c/+0x4e/+0x50`, and the city record stores the available balances at
/// `+0x132/+0x13e/+0x14a` after subtracting their paired reservations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceKind13PromotionMaterials {
    pub target_group: u8,
    pub tools_cost_fixed: u16,
    pub wood_cost_fixed: u16,
    pub bricks_cost_fixed: u16,
    pub available_tools_fixed: u16,
    pub available_wood_fixed: u16,
    pub available_bricks_fixed: u16,
}

impl SourceKind13PromotionMaterials {
    const fn permits(self, target_group: u8) -> bool {
        self.target_group == target_group
            && self.tools_cost_fixed <= self.available_tools_fixed
            && self.wood_cost_fixed <= self.available_wood_fixed
            && self.bricks_cost_fixed <= self.available_bricks_fixed
    }
}

/// One immutable `DAT_0061fa84[BGruppe]` promotion definition reconstructed
/// from the compiled `haeuser.cod` ordering. `FUN_0047c080` reads the costs
/// from the base definition before selecting one of these replacement
/// INSELHAUS definition offsets with a later `rand()` draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceKind13PromotionDefinition {
    pub target_group: u8,
    /// Unrotated compiled definition dimensions read by `FUN_00463830`
    /// before `FUN_00467940` selects the replacement orientation.
    pub source_size: (u8, u8),
    pub tools_cost_fixed: u16,
    pub wood_cost_fixed: u16,
    pub bricks_cost_fixed: u16,
    /// `Kanon` at compiled definition offset `+0x52`, charged by
    /// `FUN_0047b160` without participating in the promotion gate.
    pub cannons_cost_fixed: u16,
    /// Raw `Money` at compiled definition offset `+0x54`, subtracted from
    /// the owning player's balance after the city-store debits.
    pub money_cost: u32,
    /// Entries are ordered by `rand() % RandAnz`; each is an INSELHAUS
    /// definition offset, namely compiled `Id - 20000`.
    pub variant_definition_offsets: Vec<u16>,
}

impl SourceKind13PromotionDefinition {
    /// Build the material-gate operands for this target definition.
    pub const fn materials(
        &self,
        available_tools_fixed: u16,
        available_wood_fixed: u16,
        available_bricks_fixed: u16,
    ) -> SourceKind13PromotionMaterials {
        SourceKind13PromotionMaterials {
            target_group: self.target_group,
            tools_cost_fixed: self.tools_cost_fixed,
            wood_cost_fixed: self.wood_cost_fixed,
            bricks_cost_fixed: self.bricks_cost_fixed,
            available_tools_fixed,
            available_wood_fixed,
            available_bricks_fixed,
        }
    }

    /// The source replacement definition selected by the corresponding
    /// `FUN_0047c080` random draw.
    pub fn variant_definition_offset(&self, rand_value: u16) -> Option<u16> {
        let count = self.variant_definition_offsets.len();
        (count != 0).then(|| self.variant_definition_offsets[usize::from(rand_value) % count])
    }

    /// Encode the `FUN_004631b0` command emitted by a promoted residence
    /// after `FUN_00467940` selected `orientation`. `variant_random` selects
    /// the contiguous BGruppe definition; `command_random` supplies the
    /// independent low-five-bit random seed written into the command word.
    pub fn source_promotion_command(
        &self,
        location: SourceKind13Location,
        orientation: u8,
        variant_random: u16,
        command_random: u16,
        dynamic_object_owner: u8,
    ) -> Option<SourceBuildingCommand> {
        Some(SourceBuildingCommand {
            definition_offset: self.variant_definition_offset(variant_random)?,
            orientation: orientation & 3,
            variant: 0,
            metadata: location.island_id,
            map_owner_slot: location.source_owner & 7,
            random_seed: (command_random & 0x1f) as u8,
            dynamic_object_owner,
        })
    }
}

/// A BGruppe replacement selected by `FUN_0047c080`. The map-command layer
/// must use this event to emit the corresponding `FUN_00463ef0` preparation
/// and `FUN_004631b0` INSELHAUS command; the source changes the live
/// kind-13 record's group before that queued command is consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceKind13Promotion {
    pub island_id: u8,
    pub tile_x: u8,
    pub tile_y: u8,
    pub target_group: u8,
}

/// Result of the source `FUN_0047c080` increase path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceKind13IncreaseResult {
    /// Final fixed-point amount retained at the origin after source caps and
    /// both ordered neighbor-transfer loops.
    pub remaining_amount: u16,
    /// Sum of the fixed-point units accepted by neighbors in this call.
    pub redistributed_amount: u16,
    /// A reservation is created only when this increase first exceeds the
    /// current group's capacity and no target-group reservation is pending.
    pub reservation_created: bool,
    /// Present exactly when the material and lifecycle gates selected a
    /// higher BGruppe replacement command.
    pub promotion: Option<SourceKind13Promotion>,
}

impl Default for SourceKind13LocationTable {
    fn default() -> Self {
        Self {
            slots: vec![None; SOURCE_KIND13_LOCATION_TABLE_SLOTS],
        }
    }
}

impl SourceKind13LocationTable {
    const PROBE_LENGTH: usize = 0x40;
    const CITY_SLICE_LENGTH: usize = 0x440;

    /// Hash index returned by `FUN_00478c40`.
    pub const fn source_index(island_id: u8, tile_x: u8, tile_y: u8) -> usize {
        (((island_id as usize & 3) * 0x40 + (tile_x as usize & 0x3e)) * 0x10)
            + ((tile_y as usize >> 1) & 0x1f)
    }

    fn insertion_range(location: SourceKind13Location) -> std::ops::Range<usize> {
        let start = Self::source_index(location.island_id, location.tile_x, location.tile_y);
        let end = start
            .saturating_add(Self::PROBE_LENGTH)
            .min(SOURCE_KIND13_LOCATION_TABLE_SLOTS);
        start..end
    }

    fn lookup_range(island_id: u8, tile_x: u8, tile_y: u8) -> std::ops::Range<usize> {
        let start = Self::source_index(island_id, tile_x, tile_y);
        let end = start
            .saturating_add(Self::PROBE_LENGTH)
            .min(SOURCE_KIND13_LOCATION_TABLE_SLOTS);
        start..end
    }

    /// Insert one location by the source first-free-slot probe policy.
    pub fn insert(&mut self, location: SourceKind13Location) -> bool {
        let Some(slot) = Self::insertion_range(location).find(|&slot| self.slots[slot].is_none())
        else {
            return false;
        };
        self.slots[slot] = Some(location);
        true
    }

    /// Resolve a physical kind-13 entry using `FUN_00479f70`'s bounded
    /// hash-probe order.
    pub fn location_at(
        &self,
        island_id: u8,
        tile_x: u8,
        tile_y: u8,
    ) -> Option<SourceKind13Location> {
        Self::lookup_range(island_id, tile_x, tile_y).find_map(|slot| {
            self.slots[slot].filter(|location| {
                location.island_id == island_id
                    && location.tile_x == tile_x
                    && location.tile_y == tile_y
            })
        })
    }

    /// Mutable form of [`Self::location_at`] retaining the source's first
    /// coordinate match in its 64-slot probe window.
    /// Mutable access to one physical slot, for full-table passes like the
    /// `FUN_00482120` coverage rescan.
    pub fn slot_mut(&mut self, index: usize) -> Option<&mut SourceKind13Location> {
        self.slots.get_mut(index)?.as_mut()
    }

    pub fn location_at_mut(
        &mut self,
        island_id: u8,
        tile_x: u8,
        tile_y: u8,
    ) -> Option<&mut SourceKind13Location> {
        let slot = Self::lookup_range(island_id, tile_x, tile_y).find(|&slot| {
            self.slots[slot].is_some_and(|location| {
                location.island_id == island_id
                    && location.tile_x == tile_x
                    && location.tile_y == tile_y
            })
        })?;
        self.slots[slot].as_mut()
    }

    /// Replay the in-memory portion of `FUN_0047bbc0` for one location.
    ///
    /// `neighbors` must be the ordered coordinate buffer from
    /// `LAB_00472ad0`; only records matching the origin BGruppe participate.
    /// When the source emits a lower-tier map replacement, this updates the
    /// root and city state before returning
    /// [`SourceKind13DecreaseResult::DowngradeRequired`] to the map writer.
    pub fn apply_source_kind13_decrease(
        &mut self,
        city: &mut SourceCityRecord,
        island_id: u8,
        tile_x: u8,
        tile_y: u8,
        decrease: u16,
        neighbors: &[(u8, u8)],
    ) -> Option<SourceKind13DecreaseResult> {
        let origin = self.location_at(island_id, tile_x, tile_y)?;
        let group = usize::from(origin.population_group);
        let capacity = *SOURCE_KIND13_AMOUNT_CAPACITIES.get(group)?;
        let remaining = origin.amount.checked_sub(decrease)?;

        let low_satisfaction =
            origin.state_byte() & 0x40 == 0 || city.satisfaction_by_group[group] < 0x58;
        if low_satisfaction && group != 0 && remaining <= SOURCE_KIND13_AMOUNT_CAPACITIES[group - 1]
        {
            let target_group = origin.population_group - 1;
            let target = usize::from(target_group);
            city.tier_population[group] =
                city.tier_population[group].wrapping_sub(u32::from(origin.amount >> 6));
            city.tier_population[target] = city.tier_population[target]
                .wrapping_add_signed(source_kind13_population_units(i32::from(remaining)));
            let origin = self.location_at_mut(island_id, tile_x, tile_y)?;
            origin.population_group = target_group;
            origin.amount = remaining;
            let transition_active = origin.source_transition_active_for_group(target_group);
            origin.state_bits = (origin.state_bits & !0x40) | (u8::from(transition_active) << 6);
            return Some(SourceKind13DecreaseResult::DowngradeRequired {
                target_group,
                remaining_amount: remaining,
            });
        }

        city.tier_population[group] =
            city.tier_population[group].wrapping_sub(u32::from(origin.amount >> 6));
        let mut remaining = i32::from(remaining);
        let mut redistributed = 0_i32;
        if !low_satisfaction && group != 0 && remaining < i32::from(capacity / 2) {
            for &(neighbor_x, neighbor_y) in neighbors {
                if remaining == 0 || (neighbor_x, neighbor_y) == (tile_x, tile_y) {
                    continue;
                }
                let Some(neighbor) = self.location_at(island_id, neighbor_x, neighbor_y) else {
                    continue;
                };
                if neighbor.population_group != origin.population_group {
                    continue;
                }

                city.tier_population[group] =
                    city.tier_population[group].wrapping_sub(u32::from(neighbor.amount >> 6));
                let room = i32::from(capacity) - i32::from(neighbor.amount);
                let transfer = remaining.min(room);
                let neighbor = self.location_at_mut(island_id, neighbor_x, neighbor_y)?;
                neighbor.amount = (i32::from(neighbor.amount) + transfer) as u16;
                remaining -= transfer;
                redistributed += transfer;
                city.tier_population[group] =
                    city.tier_population[group].wrapping_add(u32::from(neighbor.amount >> 6));
            }
        }

        city.tier_population[group] = city.tier_population[group]
            .wrapping_add_signed(source_kind13_population_units(remaining));
        self.location_at_mut(island_id, tile_x, tile_y)?.amount = remaining as u16;
        Some(SourceKind13DecreaseResult::Applied {
            remaining_amount: remaining as u16,
            redistributed_amount: redistributed as u16,
        })
    }

    /// Replay `FUN_0047c080` for one positive kind-13 amount change.
    ///
    /// `neighbors` is the ordered `LAB_00472ad0` callback buffer. The source
    /// makes two distinct passes over it: first half of an eligible promotion
    /// is given to target-group roots, then (only when that target group has
    /// no residents) one quarter is given to under-half-capacity roots of the
    /// selected group. `materials` supplies the three already-reserved city
    /// balances and target definition costs used only by the promotion gate.
    pub fn apply_source_kind13_increase(
        &mut self,
        city: &mut SourceCityRecord,
        island_id: u8,
        tile_x: u8,
        tile_y: u8,
        increase: i32,
        neighbors: &[(u8, u8)],
        materials: Option<SourceKind13PromotionMaterials>,
    ) -> Option<SourceKind13IncreaseResult> {
        if increase <= 0 {
            return None;
        }
        let origin = self.location_at(island_id, tile_x, tile_y)?;
        let old_group = usize::from(origin.population_group);
        let current_capacity = i32::from(*SOURCE_KIND13_AMOUNT_CAPACITIES.get(old_group)?);
        let mut amount = i32::from(origin.amount).checked_add(increase)?;
        city.tier_population[old_group] =
            city.tier_population[old_group].wrapping_sub(u32::from(origin.amount >> 6));

        let mut selected_group = origin.population_group;
        let mut selected_capacity = current_capacity;
        let mut redistributed = 0_i32;
        let mut reservation_created = false;
        let mut promotion = None;

        if amount > current_capacity {
            let target_group = origin.population_group.checked_add(1)?;
            if let Some(&target_capacity) =
                SOURCE_KIND13_AMOUNT_CAPACITIES.get(usize::from(target_group))
            {
                let target = usize::from(target_group);
                let reservation_matches = city.promotion_reservations[target] != 0
                    && city.promotion_reservation_positions[target] == (tile_x, tile_y)
                    && city.phase != ((origin.state_byte() >> 3) & 7);

                if reservation_matches {
                    if city.overall_satisfaction > 0x7f && city.satisfaction_by_group[target] > 0x7f
                    {
                        if city.tier_population[target] != 0 {
                            let mut pending = amount / 2;
                            amount -= pending;
                            for &(neighbor_x, neighbor_y) in neighbors {
                                if pending == 0 || (neighbor_x, neighbor_y) == (tile_x, tile_y) {
                                    continue;
                                }
                                let Some(neighbor) =
                                    self.location_at(island_id, neighbor_x, neighbor_y)
                                else {
                                    continue;
                                };
                                if neighbor.population_group != target_group {
                                    continue;
                                }

                                city.tier_population[target] = city.tier_population[target]
                                    .wrapping_sub(u32::from(neighbor.amount >> 6));
                                let transfer = pending.min(
                                    (i32::from(target_capacity) - i32::from(neighbor.amount))
                                        .max(0),
                                );
                                let neighbor =
                                    self.location_at_mut(island_id, neighbor_x, neighbor_y)?;
                                neighbor.amount = (i32::from(neighbor.amount) + transfer) as u16;
                                pending -= transfer;
                                redistributed += transfer;
                                city.tier_population[target] = city.tier_population[target]
                                    .wrapping_add(u32::from(neighbor.amount >> 6));
                            }
                            amount += pending;
                        }

                        if !city.promotion_blocked
                            && current_capacity < amount
                            && origin.source_transition_active_for_group(target_group)
                            && materials.is_some_and(|materials| materials.permits(target_group))
                        {
                            selected_group = target_group;
                            selected_capacity = i32::from(target_capacity);
                            promotion = Some(SourceKind13Promotion {
                                island_id,
                                tile_x,
                                tile_y,
                                target_group,
                            });
                        }
                    }

                    let selected = usize::from(selected_group);
                    if city.tier_population[target] == 0 && selected_capacity < amount {
                        let mut pending = amount / 4;
                        amount -= pending;
                        for &(neighbor_x, neighbor_y) in neighbors {
                            if pending == 0 || (neighbor_x, neighbor_y) == (tile_x, tile_y) {
                                continue;
                            }
                            let Some(neighbor) =
                                self.location_at(island_id, neighbor_x, neighbor_y)
                            else {
                                continue;
                            };
                            if neighbor.population_group != selected_group
                                || i32::from(neighbor.amount) > selected_capacity / 2
                            {
                                continue;
                            }

                            city.tier_population[selected] = city.tier_population[selected]
                                .wrapping_sub(u32::from(neighbor.amount >> 6));
                            let transfer = pending
                                .min((selected_capacity - i32::from(neighbor.amount)).max(0));
                            let neighbor =
                                self.location_at_mut(island_id, neighbor_x, neighbor_y)?;
                            neighbor.amount = (i32::from(neighbor.amount) + transfer) as u16;
                            pending -= transfer;
                            redistributed += transfer;
                            city.tier_population[selected] = city.tier_population[selected]
                                .wrapping_add(u32::from(neighbor.amount >> 6));
                        }
                        amount += pending;
                    }

                    city.promotion_reservations[target] = 0;
                    city.promotion_reservation_positions[target] = (0xff, 0xff);
                } else if city.promotion_reservations[target] == 0 {
                    city.promotion_reservations[target] =
                        source_kind13_population_units(amount).try_into().ok()?;
                    city.promotion_reservation_positions[target] = (tile_x, tile_y);
                    let origin = self.location_at_mut(island_id, tile_x, tile_y)?;
                    origin.state_bits = (origin.state_bits & 0xc7) | ((city.phase & 7) << 3);
                    reservation_created = true;
                }
            }
        }

        amount = amount.min(selected_capacity);
        let selected = usize::from(selected_group);
        city.tier_population[selected] = city.tier_population[selected]
            .wrapping_add_signed(source_kind13_population_units(amount));
        let origin = self.location_at_mut(island_id, tile_x, tile_y)?;
        origin.population_group = selected_group;
        origin.amount = amount.try_into().ok()?;
        Some(SourceKind13IncreaseResult {
            remaining_amount: origin.amount,
            redistributed_amount: redistributed.try_into().ok()?,
            reservation_created,
            promotion,
        })
    }

    /// Remove source roots overwritten by a later oriented INSELHAUS command.
    pub fn remove_roots_in_footprint(
        &mut self,
        island_id: u8,
        tile_x: u8,
        tile_y: u8,
        width: i32,
        height: i32,
    ) {
        self.slots.iter_mut().for_each(|slot| {
            if slot.is_some_and(|location| {
                location.island_id == island_id
                    && i32::from(location.tile_x) >= i32::from(tile_x)
                    && i32::from(location.tile_x) < i32::from(tile_x) + width
                    && i32::from(location.tile_y) >= i32::from(tile_y)
                    && i32::from(location.tile_y) < i32::from(tile_y) + height
            }) {
                *slot = None;
            }
        });
    }

    /// Source slot range returned by `FUN_0047b6b0` / `FUN_0047b6d0`.
    pub fn city_slice(&self, island_id: u8) -> &[Option<SourceKind13Location>] {
        let start = Self::source_index(island_id, 0, 0);
        let end = start
            .saturating_add(Self::CITY_SLICE_LENGTH)
            .min(SOURCE_KIND13_LOCATION_TABLE_SLOTS);
        &self.slots[start..end]
    }

    /// Live entries in physical source slot order, for assertions and audits.
    pub fn active_locations(&self) -> Vec<SourceKind13Location> {
        self.slots.iter().flatten().copied().collect()
    }
}

/// Source signed `(amount + sign_bit * 63) >> 6` conversion used when a
/// kind-13 fixed-point amount changes a city population total.
const fn source_kind13_population_units(amount: i32) -> i32 {
    (amount + ((amount >> 31) & 0x3f)) >> 6
}

/// Mutable phase clocks and physical cursor owned by `FUN_0047b9c0`.
///
/// The source advances sixteen staggered 15,000 ms clocks, then visits 70
/// records from `DAT_005a77e8` in physical order on each engine-update slice.
/// Its later city-state transfer branches retain their own source operands.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceKind13DispatchState {
    phase_elapsed_ms: [u32; SOURCE_KIND13_PHASE_CLOCKS],
    phases: [u8; SOURCE_KIND13_PHASE_CLOCKS],
    cursor: usize,
}

impl Default for SourceKind13DispatchState {
    fn default() -> Self {
        Self {
            phase_elapsed_ms: [0; SOURCE_KIND13_PHASE_CLOCKS],
            phases: [0; SOURCE_KIND13_PHASE_CLOCKS],
            cursor: 0,
        }
    }
}

impl SourceKind13DispatchState {
    /// Advance phase clocks and replay `FUN_0047b9c0`'s 70-record phase batch.
    /// Returns the count of records whose source byte `+0x03` changed.
    pub fn advance(&mut self, table: &mut SourceKind13LocationTable, dt_ms: u32) -> usize {
        self.advance_batch(table, dt_ms).len()
    }

    /// Advance phase clocks and return the exact physical records whose phase
    /// byte changed in this 70-record `FUN_0047b9c0` batch. Callers process
    /// each returned snapshot only after the source phase write is visible.
    pub fn advance_batch(
        &mut self,
        table: &mut SourceKind13LocationTable,
        dt_ms: u32,
    ) -> Vec<SourceKind13Location> {
        for selector in 0..SOURCE_KIND13_PHASE_CLOCKS {
            let threshold = SOURCE_KIND13_PHASE_BASE_MS
                + u32::try_from(selector).unwrap_or(0) * SOURCE_KIND13_PHASE_STRIDE_MS;
            let elapsed = self.phase_elapsed_ms[selector].saturating_add(dt_ms);
            if elapsed >= threshold {
                self.phase_elapsed_ms[selector] = 0;
                self.phases[selector] = self.phases[selector].wrapping_add(1) & 7;
                // A phase tick restarts the physical sweep from the table
                // head so every record observes the new phase before the
                // following tick.
                self.cursor = 0;
            } else {
                self.phase_elapsed_ms[selector] = elapsed;
            }
        }

        let mut changed = Vec::new();
        for _ in 0..SOURCE_KIND13_DISPATCH_RECORDS_PER_UPDATE {
            let slot = self.cursor;
            self.cursor = (self.cursor + 1) % SOURCE_KIND13_LOCATION_TABLE_SLOTS;
            let Some(location) = table.slots[slot].as_mut() else {
                continue;
            };
            let phase = self.phases[usize::from(location.variant & 0x0f)];
            if location.phase != phase {
                location.set_phase(phase);
                changed.push(*location);
            }
        }
        changed
    }
}

/// Map COD good names to simulation Good enum.
///
/// Covers all 25 player-facing goods from `text.cod [WARE]` plus the
/// raw-plantation crops the engine references in production chains
/// (BAUM = forest tree, KAKAOBAUM = cocoa tree, TABAKBAUM = tobacco
/// plant, GEWUERZBAUM = spice tree, ZUCKERROHR = sugarcane). These
/// plant tokens aren't separate economic goods; they map to the
/// finished-good enum the player sees in their warehouse so the
/// production tick can consume them as an input. ALLWARE / NOWARE /
/// GRAS are flag/terrain pseudo-goods → `Good::None`.
fn parse_good(name: &str) -> Good {
    // Mapping keys verified against the four shipping COD files
    // (haeuser.cod, figuren.cod, text.cod, editor.cod). Singular
    // forms (`SCHWERT`, `STEIN`, etc.), `RUM`, `WEIN`, `GOLDERZ`
    // and other speculative spellings were removed because the
    // strings never appear in the data — defensive mappings to
    // strings the game cannot emit are dead code.
    match name {
        "HOLZ" | "BAUM" => Good::Wood,
        "EISEN" => Good::Iron,
        "EISENERZ" | "ERZE" => Good::Ore,
        "GOLD" => Good::Gold,
        "WOLLE" => Good::Wool,
        "ZUCKER" => Good::Sugar,
        "ZUCKERROHR" => Good::SugarCane,
        "TABAK" | "TABAKBAUM" => Good::Tobacco,
        "RIND" => Good::Cattle,
        "FLEISCH" => Good::Meat,
        "GETREIDE" | "KORN" => Good::Grain,
        "MEHL" => Good::Flour,
        "WERKZEUG" => Good::Tools,
        "ZIEGEL" => Good::Bricks,
        "STEINE" => Good::Stone,
        "SCHWERTER" => Good::Swords,
        "MUSKETEN" => Good::Muskets,
        "KANONEN" => Good::Cannons,
        "NAHRUNG" => Good::Food,
        "STOFFE" => Good::Cloth,
        "ALKOHOL" => Good::Alcohol,
        "TABAKWAREN" => Good::TobaccoProducts,
        "GEWUERZE" | "GEWUERZBAUM" => Good::Spices,
        "KAKAO" | "KAKAOBAUM" => Good::Cocoa,
        "WEINTRAUBEN" => Good::Grapes,
        "WILD" => Good::WildGame,
        "BAUMWOLLE" => Good::Cotton,
        "SEIDE" => Good::Silk,
        "SCHMUCK" => Good::Jewelry,
        "KLEIDUNG" => Good::Clothing,
        "FISCHE" => Good::Fish,
        // Flags / terrain pseudo-goods that don't correspond to a
        // player-facing good slot.
        "NOWARE" | "ALLWARE" | "GRAS" | "" => Good::None,
        _ => Good::None,
    }
}

/// Map a source ware-registration index (the executable's compiled
/// `def+0x21` byte; see `anno_formats::cod::source_ware_slot` for the
/// token table) to this simulator's [`Good`]. Slot `0x07` covers both
/// `FLEISCH` and `RIND` in the source; the processed good is returned.
pub fn good_for_source_ware_slot(slot: u8) -> Good {
    match slot {
        0x02 => Good::Ore,
        0x03 => Good::Gold,
        0x04 => Good::Wool,
        0x05 => Good::Sugar,
        0x06 => Good::Tobacco,
        0x07 => Good::Meat,
        0x08 => Good::Grain,
        0x09 => Good::Flour,
        0x0a => Good::Iron,
        0x0b => Good::Swords,
        0x0c => Good::Muskets,
        0x0d => Good::Cannons,
        0x0e => Good::Food,
        0x0f => Good::TobaccoProducts,
        0x10 => Good::Spices,
        0x11 => Good::Cocoa,
        0x12 => Good::Alcohol,
        0x13 => Good::Cloth,
        0x14 => Good::Clothing,
        0x15 => Good::Jewelry,
        0x16 => Good::Tools,
        0x17 => Good::Wood,
        0x18 => Good::Bricks,
        _ => Good::None,
    }
}

#[cfg(test)]
#[test]
fn good_for_source_ware_slot_roundtrips_the_registration_table() {
    // Every token the executable registers with a Good-mapped slot must
    // resolve back to the same Good through the slot index. RIND shares
    // slot 0x07 with FLEISCH; the slot mapping returns the processed
    // good, so the raw-cattle alias is the one accepted exception.
    for token in [
        "EISENERZ",
        "GOLD",
        "WOLLE",
        "ZUCKER",
        "TABAK",
        "FLEISCH",
        "KORN",
        "MEHL",
        "EISEN",
        "SCHWERTER",
        "MUSKETEN",
        "KANONEN",
        "NAHRUNG",
        "TABAKWAREN",
        "GEWUERZE",
        "KAKAO",
        "ALKOHOL",
        "STOFFE",
        "KLEIDUNG",
        "SCHMUCK",
        "WERKZEUG",
        "HOLZ",
        "ZIEGEL",
    ] {
        let slot = anno_formats::cod::source_ware_slot(token)
            .unwrap_or_else(|| panic!("{token} must be in the registration table"));
        assert_eq!(
            good_for_source_ware_slot(slot),
            parse_good(token),
            "slot round-trip mismatch for {token}"
        );
    }
    assert_eq!(good_for_source_ware_slot(0x00), Good::None);
    assert_eq!(good_for_source_ware_slot(0x3a), Good::None);
}

#[cfg(test)]
#[test]
fn parse_good_covers_all_haeuser_cod_tokens() {
    // Tokens enumerated from `extracted/haeuser.cod` Ware: / Rohstoff:
    // / Workstoff: lines (excluding the literal "ALLWARE" / "NOWARE"
    // wildcards which always resolve to None).
    let tokens = [
        "ALKOHOL",
        "BAUM",
        "BAUMWOLLE",
        "EISEN",
        "EISENERZ",
        "ERZE",
        "FISCHE",
        "FLEISCH",
        "GETREIDE",
        "GEWUERZBAUM",
        "GEWUERZE",
        "GOLD",
        "GRAS",
        "HOLZ",
        "KAKAO",
        "KAKAOBAUM",
        "KANONEN",
        "KLEIDUNG",
        "KORN",
        "MEHL",
        "MUSKETEN",
        "NAHRUNG",
        "SCHMUCK",
        "SCHWERTER",
        "STEINE",
        "STOFFE",
        "TABAK",
        "TABAKBAUM",
        "TABAKWAREN",
        "WEINTRAUBEN",
        "WERKZEUG",
        "WILD",
        "WOLLE",
        "ZIEGEL",
        "ZUCKER",
        "ZUCKERROHR",
    ];
    for tok in tokens {
        let g = parse_good(tok);
        // GRAS is the only one that legitimately maps to None.
        if tok != "GRAS" {
            assert_ne!(g, Good::None, "token {tok} should map to a real good");
        }
    }
}

#[cfg(test)]
#[test]
fn rohstoff_to_fertility_matches_audit_pairs() {
    use anno_formats::szs::Fertility;
    // Pairs derived from `cargo run --example audit_fertility_mapping`
    // — every PLANTAGE entry's Rohstoff field paired with the
    // fertility-gated crop it grows.
    let pairs: &[(&str, Option<Fertility>)] = &[
        ("GETREIDE", Some(Fertility::Grain)),
        ("TABAKBAUM", Some(Fertility::Tobacco)),
        ("GEWUERZBAUM", Some(Fertility::Spices)),
        ("ZUCKERROHR", Some(Fertility::Sugarcane)),
        ("BAUMWOLLE", Some(Fertility::Cotton)),
        ("WEINTRAUBEN", Some(Fertility::Vines)),
        ("KAKAOBAUM", Some(Fertility::Cocoa)),
        // Universal raw materials should NOT bind a fertility.
        ("BAUM", None),
        ("STEINE", None),
        ("ERZE", None),
        ("", None),
    ];
    for (rohstoff, want) in pairs {
        assert_eq!(
            rohstoff_to_fertility(rohstoff),
            *want,
            "rohstoff_to_fertility({rohstoff:?})"
        );
    }
}

#[cfg(test)]
#[test]
fn parse_bauinfra_matches_haeuser_cod_ladder() {
    // Aliases from haeuser.cod's `BESONDERE INFRASTRUKTUR
    // MARKPUNKTE` block, paired with the `INFRA_*` constant id the
    // exe's `0x00499d30` name table assigns to the rung they
    // substitute for.
    let cases: &[(&str, u8)] = &[
        ("INFRA_KONTOR_1", 17), // = INFRA_STUFE_2B
        ("INFRA_BURG_1", 23),   // = INFRA_STUFE_2G
        ("INFRA_WACHTURM", 23), // = INFRA_STUFE_2G
        ("INFRA_KONTOR_2", 22), // = INFRA_STUFE_3A
        ("INFRA_KANON", 27),    // = INFRA_STUFE_3E
        ("INFRA_KONTOR_3", 29), // = INFRA_STUFE_4A
        ("INFRA_BURG_2", 30),   // = INFRA_STUFE_4B
        ("INFRA_MUSKETE", 30),  // = INFRA_STUFE_4B
        ("INFRA_BURG_3", 32),   // = INFRA_STUFE_5B
        // Direct STUFE tokens. Note 5A/5B: haeuser.cod declares 5B
        // first, but the exe's hardcoded table numbers 5A = 31.
        ("INFRA_STUFE_1A", 15),
        ("INFRA_STUFE_5A", 31),
        ("INFRA_STUFE_5B", 32),
        // Absent / unknown token → INFRA_NIX, always buildable.
        ("", 0),
        ("INFRA_NOT_A_REAL_TAG", 0),
        // Cultural-building tags are ordinary rungs too: they gate
        // the church / school themselves.
        ("INFRA_KIRCHE", 5),
        ("INFRA_SCHULE", 3),
    ];
    for (tok, want) in cases {
        let got = parse_bauinfra(tok);
        assert_eq!(got, *want, "parse_bauinfra({tok:?}) = {got}, want {want}");
    }
}

/// The unlock sweep at the tail of `FUN_0047f8a0`
/// (`1602_exe.c:91520-91581`).
#[cfg(test)]
#[test]
fn source_city_unlock_sweep_matches_the_source_ladder() {
    const MARKT: u32 = 1 << 0;
    const KAPELLE: u32 = 1 << 1;
    const STUFE_1A: u32 = 1 << 14; // id 15
    const STUFE_2A: u32 = 1 << 15; // id 16
    const STUFE_5B: u32 = 1 << 31; // id 32

    // INFRA_MARKT and INFRA_KAPELLE are absent from haeuser.cod, so
    // their `(BGruppe, Minwohn)` stays `(0, 0)` and `0 >= 0` grants
    // them on the very first sweep of an empty city.
    let empty = source_city_unlock_sweep(&[0; 5], 0);
    assert_eq!(empty & MARKT, MARKT, "marketplace unlocks immediately");
    assert_eq!(empty & KAPELLE, KAPELLE, "chapel unlocks immediately");
    assert_eq!(empty, MARKT | KAPELLE, "nothing else unlocks at zero pop");

    // INFRA_STUFE_1A is `(BGruppe 0, Minwohn 30)`: the boundary is
    // `Minwohn <= cum[0]`, so 29 pioneers is short and 30 is enough.
    assert_eq!(source_city_unlock_sweep(&[29, 0, 0, 0, 0], 0) & STUFE_1A, 0);
    assert_eq!(
        source_city_unlock_sweep(&[30, 0, 0, 0, 0], 0) & STUFE_1A,
        STUFE_1A,
    );
    // 30 pioneers alone must NOT reach a BGruppe-1 rung: `cum[1]` is
    // still 0 because the sum runs upward, not downward.
    assert_eq!(source_city_unlock_sweep(&[30, 0, 0, 0, 0], 0) & STUFE_2A, 0);

    // The sum is cumulative from the top: 600 aristocrats satisfy
    // `cum[k] = 600` for every k, so every rung whose Minwohn <= 600
    // unlocks at once — including all the BGruppe-0/1/2/3 ones.
    let aristo = source_city_unlock_sweep(&[0, 0, 0, 0, 600], 0);
    assert_eq!(aristo & STUFE_5B, STUFE_5B, "STUFE_5B (BGruppe 4, 600)");
    assert_eq!(aristo & STUFE_1A, STUFE_1A, "lower BGruppe-0 rung too");
    for (id, (_, minwohn)) in BAUINFRA_LADDER.iter().enumerate().skip(1) {
        let bit = 1u32 << (id - 1);
        let want = u32::from(*minwohn) <= 600;
        assert_eq!(
            aristo & bit != 0,
            want,
            "id {id} ({}) Minwohn {minwohn} against 600 cumulative",
            INFRA_NAMES[id],
        );
    }

    // Grants are permanent: the sweep only ORs. A fully-set mask
    // survives an empty city untouched.
    assert_eq!(source_city_unlock_sweep(&[0; 5], u32::MAX), u32::MAX);
    // …and a previously-earned rung is not revoked when the
    // population that earned it is gone.
    let earned = source_city_unlock_sweep(&[30, 0, 0, 0, 0], 0);
    assert_eq!(
        source_city_unlock_sweep(&[0; 5], earned) & STUFE_1A,
        STUFE_1A,
    );
}

/// Tripwire: [`BAUINFRA_LADDER`] must reproduce the `Objekt: BAUINFRA`
/// block of the shipped `haeuser.cod`. The file is byte-negated on
/// disk (`decrypted = (-encrypted) & 0xFF`), so decrypt it here and
/// parse the block textually rather than trusting the transcription.
/// Self-skips without the data corpus.
#[cfg(test)]
#[test]
fn bauinfra_ladder_matches_shipped_haeuser_cod() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let Ok(raw) = std::fs::read(root.join("extracted/haeuser.cod")) else {
        println!("Skipping test: data corpus not found");
        return;
    };
    let text: String = raw
        .iter()
        .map(|&b| char::from(b.wrapping_neg()))
        .collect::<String>();

    // Walk `Objekt: BAUINFRA` … `EndObj;` collecting the
    // Nummer/BGruppe/Minwohn triples in declaration order. The
    // separator between key and value is arbitrary whitespace.
    let after_objekt = text
        .match_indices("Objekt:")
        .map(|(i, m)| &text[i + m.len()..])
        .find(|rest| rest.trim_start().starts_with("BAUINFRA"))
        .expect("BAUINFRA block present");
    let block = after_objekt
        .split_once("EndObj;")
        .expect("block terminator")
        .0;

    let field = |line: &str, key: &str| -> Option<String> {
        let line = line.split(';').next().unwrap_or(line); // strip comment
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| value.trim().to_string())
    };

    let mut declared: Vec<(String, u8, u16)> = Vec::new();
    let (mut name, mut bgruppe) = (None::<String>, None::<u8>);
    for line in block.lines() {
        if let Some(v) = field(line, "Nummer") {
            name = Some(v);
        } else if let Some(v) = field(line, "BGruppe") {
            bgruppe = v.parse().ok();
        } else if let Some(v) = field(line, "Minwohn") {
            let minwohn: u16 = v.parse().expect("numeric Minwohn");
            declared.push((
                name.take().expect("Nummer precedes Minwohn"),
                bgruppe.take().expect("BGruppe precedes Minwohn"),
                minwohn,
            ));
        }
    }

    // Every rung the file declares must land on its exe id with the
    // authored pair.
    assert_eq!(declared.len(), 30, "haeuser.cod declares 30 BAUINFRA rungs");
    for (name, bgruppe, minwohn) in &declared {
        let id = parse_bauinfra(name);
        assert_ne!(id, 0, "{name} is not a known INFRA_* constant");
        assert_eq!(
            BAUINFRA_LADDER[usize::from(id)],
            (*bgruppe, *minwohn),
            "{name} (id {id})",
        );
    }

    // Conversely, the only ids the file does NOT declare are
    // INFRA_NIX / INFRA_MARKT / INFRA_KAPELLE, and those must stay
    // `(0, 0)` — that zero threshold is what makes the marketplace
    // and chapel unlock on a settlement's first sweep.
    let names: Vec<&str> = declared.iter().map(|(n, _, _)| n.as_str()).collect();
    for (id, name) in INFRA_NAMES.iter().enumerate() {
        if names.contains(name) {
            continue;
        }
        assert!(
            matches!(id, 0 | 1 | 2),
            "{name} (id {id}) missing from haeuser.cod",
        );
        assert_eq!(BAUINFRA_LADDER[id], (0, 0), "{name} keeps the zero pair");
    }
}

/// Convert a COD building definition into a simulation BuildingDef.
fn convert_building_def(cod_building: &CodBuilding) -> BuildingDef {
    let prop = |key: &str| -> &str {
        cod_building
            .properties
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or("")
    };

    let prop_int = |key: &str| -> i32 {
        let s = prop(key);
        s.parse::<i32>().unwrap_or(0)
    };

    // Ware/Rohstoff values may have comma-separated coefficients: "ALKOHOL, 0.5"
    // Extract just the good name (first token)
    let good_name = |key: &str| -> &str {
        let val = prop(key);
        val.split(',').next().unwrap_or(val).trim()
    };

    let output_good = parse_good(good_name("Ware"));
    let input_good_1 = parse_good(good_name("Rohstoff"));
    let input_good_2 = parse_good(good_name("Workstoff"));

    let interval = prop_int("Interval").max(1) as u16;
    let maxlager = prop_int("Maxlager").max(0) as u16;
    // Only set input rates if the corresponding input good exists
    let rohmenge = if input_good_1 != Good::None {
        prop_int("Rohmenge").max(0) as u16
    } else {
        0
    };
    let workmenge = if input_good_2 != Good::None {
        prop_int("Workmenge").max(0) as u16
    } else {
        0
    };

    // Construction costs from HAUS_BAUKOST sub-object
    let cost_gold = prop_int("Money").max(0) as u32;
    let cost_tools = prop_int("Werkzeug").max(0) as u16;
    let cost_wood = prop_int("Holz").max(0) as u16;
    let cost_bricks = prop_int("Ziegel").max(0) as u16;

    // Per-building operating cost: the compiled `Kosten: <active>,
    // <stopped>` pair at definition `+0x2c`/`+0x2a`
    // (`1602_exe.c:67140-67148`), selected by `FUN_00463140` for the city
    // maintenance accumulator `+0x1d8`. The active cost applies while the
    // building runs; definitions without the property (terrain, houses,
    // roads) cost nothing — which also preserves the earlier
    // terrain-maintenance fix without a category special case. Replaces
    // the previous community-appendix approximation table. Validated
    // against the live original's per-city `+0x1d8` accumulators on
    // Exile (210/195/255/160/250/10/0 across the seven cities).
    let prod_kind_str = prop("ProdKind");
    use crate::types::Good;
    let maintenance: u16 = cod_building.source_operating_costs.0;

    // Resolve Radius property (may be a number or a constant name)
    let radius_raw = prop("Radius");
    let radius = if let Ok(n) = radius_raw.parse::<i32>() {
        n.max(0) as u16
    } else {
        // Executable-registered constants (`1602_exe.c:66467-66468`,
        // both `FUN_004020d0(..., 0x10)`); not defined in the COD file.
        match radius_raw {
            "RADIUS_MARKT" | "RADIUS_HQ" => 16,
            _ => 0,
        }
    };

    // Map COD building Kind / HAUS_PRODTYP Kind to internal category
    // and ProductionType. Kinds enumerated from haeuser.cod (top-level
    // `Kind:` values).
    use crate::types::ProductionType;
    let category: u8 = match cod_building.kind.as_str() {
        // Terrain / nature.
        "BODEN" | "WALD" | "FELS" | "STRAND" | "MEER" | "FLUSS" | "FLUSSECK" | "MUENDUNG"
        | "HANG" | "HANGECK" | "HANGQUELL" | "BRANDUNG" | "BRANDECK" | "STRANDECKA"
        | "STRANDECKI" | "STRANDMUND" | "STRANDRUINE" | "STRANDVARI" | "STRANDHAUS"
        | "WEIDETIER" | "MAUERSTRAND" | "TURMSTRAND" => 0,
        // Residential.
        "WOHNUNG" | "PIRATWOHN" => 1,
        // Production / industry / raw materials.
        "HANDWERK" | "PLANTAGE" | "ROHSTOFF" | "ROHSTERZ" | "ROHSTWACHS" | "BERGWERK"
        | "STEINBRUCH" | "MINE" | "JAGDHAUS" | "FISCHEREI" | "WMUEHLE" => 2,
        // Public services and culture.
        "KAPELLE" | "KIRCHE" | "SCHULE" | "HOCHSCHULE" | "THEATER" | "BADEHAUS" | "BRUNNEN"
        | "WIRT" | "DENKMAL" | "TRIUMPH" | "KLINIK" | "GALGEN" | "MARKT" | "PLATZ" => 3,
        // Trade / harbour.
        "KONTOR" | "HAFEN" | "WERFT" | "PIER" => 4,
        // Military.
        "MILITAR" | "MAUER" | "TOR" | "TURM" | "WACHTURM" | "SCHLOSS" => 5,
        // Transport.
        "STRASSE" | "BRUECKE" => 6,
        // Generic / catch-all.
        _ => 7,
    };
    let production_type = match prod_kind_str {
        "HANDWERK" | "BAECKER" => ProductionType::Craft,
        "PLANTAGE" | "ROHSTOFF" | "ROHSTERZ" | "ROHSTWACHS" | "JAGDHAUS" | "FISCHEREI"
        | "WEIDETIER" => ProductionType::Plantation,
        "BERGWERK" | "STEINBRUCH" | "MINE" => ProductionType::Mine,
        "WOHNUNG" => ProductionType::Residence,
        _ => ProductionType::Craft,
    };

    BuildingDef {
        id: cod_building.nummer as u16,
        category,
        width: cod_building.size.0 as u8,
        height: cod_building.size.1 as u8,
        production_type,
        kind: cod_building.kind.clone(),
        prod_kind: prod_kind_str.to_string(),
        radius,
        output_good,
        input_good_1,
        input_good_2,
        output_rate: 1, // Each cycle produces 1 unit of output
        input_1_rate: rohmenge,
        input_2_rate: workmenge,
        storage_capacity: maxlager,
        // `Interval` from haeuser.cod counts production ticks. The
        // game-loop tick is exactly 1000 ms (decompiled binary uses
        // `-1000` decrement on the production-cycle accumulator at
        // `1602_exe.c:16110`), not 999.
        cycle_time_ms: interval as u32 * 1000,

        cost_gold,
        cost_tools,
        cost_wood,
        cost_bricks,
        maintenance_cost: maintenance,
        native: prop("Nativflg") == "1",
        bauinfra: parse_bauinfra(prop("Bauinfra")),
        max_no_input_ticks: {
            let v = prop_int("Maxnorohst");
            if v > 0 {
                (v as u8).min(255)
            } else {
                6
            }
        },
        can_dry_up: prop("Doerrflg") == "1",
        wegspeed: {
            let raw = prop("Wegspeed");
            let mut quad = [100u16; 4];
            for (i, tok) in raw.split(',').map(str::trim).enumerate().take(4) {
                if let Ok(v) = tok.parse::<u16>() {
                    quad[i] = v;
                }
            }
            quad
        },
        has_door: prop("Tuerflg") == "1",
        upgradeable: prop("Ausbauflg") == "1",
        max_energy: {
            let v = prop_int("Maxenergy");
            if v > 0 {
                v as u16
            } else {
                0
            }
        },
        ore_deposit: match prop("Erzbergnr") {
            "ERZBERG_KLEIN" => crate::building::OreDeposit::Small,
            "ERZBERG_GROSS" => crate::building::OreDeposit::Large,
            _ => crate::building::OreDeposit::None,
        },
        pirate_owned: prop("Piratflg") == "1",
        defensive_cannons: prop_int("Kanon").max(0) as u8,
        max_brand_damage_ticks: {
            let v = prop_int("Maxbrand");
            if v > 0 {
                v as u16
            } else {
                crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS
            }
        },
        ruin_id: cod_building.ruinenr.clamp(0, 255) as u8,
        required_fertility: rohstoff_to_fertility(good_name("Rohstoff")),
    }
}

/// Map haeuser.cod's `Rohstoff` raw-material name to the typed
/// fertility the host island must carry. Audit-derived from
/// `cargo run --example audit_fertility_mapping`:
///
///   TABAKBAUM    → Tobacco
///   KAKAOBAUM    → Cocoa
///   ZUCKERROHR   → Sugarcane
///   WEINTRAUBEN  → Vines      (Nummer 408 → Alkohol/Wine)
///   BAUMWOLLE    → Cotton     (Nummer 404 → Wolle/Cotton)
///   GEWUERZBAUM  → Spices
///   GETREIDE     → Grain
///   (BAUM, STEINE, …) — universal, no fertility gate
fn rohstoff_to_fertility(name: &str) -> Option<anno_formats::szs::Fertility> {
    use anno_formats::szs::Fertility::*;
    Some(match name {
        "GETREIDE" => Grain,
        "TABAKBAUM" => Tobacco,
        "GEWUERZBAUM" => Spices,
        "ZUCKERROHR" => Sugarcane,
        "BAUMWOLLE" => Cotton,
        "WEINTRAUBEN" => Vines,
        "KAKAOBAUM" => Cocoa,
        _ => return None,
    })
}

/// The `INFRA_*` constant names in engine id order (0..=32).
///
/// RE: the `.data` pointer table at `0x00499d30`, walked by
/// `1602_exe.c:66461-66466` — `FUN_004020d0(*ppuVar12, iVar3, 0)`
/// registers each name as COD constant `0, 1, 2, …` until the cursor
/// reaches `0x499db4`, i.e. exactly 33 entries. The id numbering is
/// therefore hardcoded in the executable and is *not* the order the
/// `Objekt: BAUINFRA` block declares them in haeuser.cod — the shipped
/// file lists `INFRA_STUFE_5B` ahead of `INFRA_STUFE_5A`, while the exe
/// numbers `5A = 31` and `5B = 32`.
///
/// Id 0 (`INFRA_NIX`) marks "no requirement". Ids 1/2 (`INFRA_MARKT`,
/// `INFRA_KAPELLE`) exist only in the exe; haeuser.cod never declares
/// them.
pub const INFRA_NAMES: [&str; 33] = [
    "INFRA_NIX",
    "INFRA_MARKT",
    "INFRA_KAPELLE",
    "INFRA_SCHULE",
    "INFRA_WIRT",
    "INFRA_KIRCHE",
    "INFRA_BADE",
    "INFRA_THEATER",
    "INFRA_HOCHSCHULE",
    "INFRA_ARZT",
    "INFRA_GALGEN",
    "INFRA_SCHLOSS",
    "INFRA_KATHETRALE",
    "INFRA_TRIUMPH",
    "INFRA_DENKMAL",
    "INFRA_STUFE_1A",
    "INFRA_STUFE_2A",
    "INFRA_STUFE_2B",
    "INFRA_STUFE_2C",
    "INFRA_STUFE_2D",
    "INFRA_STUFE_2E",
    "INFRA_STUFE_2F",
    "INFRA_STUFE_3A",
    "INFRA_STUFE_2G",
    "INFRA_STUFE_3B",
    "INFRA_STUFE_3C",
    "INFRA_STUFE_3D",
    "INFRA_STUFE_3E",
    "INFRA_STUFE_3F",
    "INFRA_STUFE_4A",
    "INFRA_STUFE_4B",
    "INFRA_STUFE_5A",
    "INFRA_STUFE_5B",
];

/// The `(BGruppe, Minwohn)` unlock threshold for each `INFRA_*` id —
/// the runtime table `DAT_0061fbc0`, stride 4, laid out as
/// `struct { u8 bgruppe; u8 pad; u16 minwohn; }` and indexed by the
/// [`INFRA_NAMES`] id.
///
/// RE: haeuser.cod's `Objekt: BAUINFRA` block authors it. The loader
/// stores `BGruppe` into `(&DAT_0061fbc0)[id * 4]` at
/// `1602_exe.c:67114` and `Minwohn` into `*(u16 *)(&DAT_0061fbc2 +
/// id * 4)` at `1602_exe.c:67295`. `FUN_0047f8a0`'s unlock sweep
/// (`1602_exe.c:91520-91581`) reads the pair back through the cursor
/// `local_2c = &DAT_0061fbc4` (id 1) advancing by 4 per rung.
///
/// Id 0 (`INFRA_NIX`) is never consulted: `FUN_0042d530` returns
/// "buildable" before it would index. Ids 1/2 (`INFRA_MARKT`,
/// `INFRA_KAPELLE`) are absent from haeuser.cod, so they keep the
/// zero-initialised `(0, 0)` and the first sweep of any city grants
/// them unconditionally — which is why the marketplace and the chapel
/// are available from the start even in scenarios that author an empty
/// unlock mask.
pub const BAUINFRA_LADDER: [(u8, u16); 33] = [
    (0, 0),     // 0  INFRA_NIX        (never consulted)
    (0, 0),     // 1  INFRA_MARKT      (not in haeuser.cod)
    (0, 0),     // 2  INFRA_KAPELLE    (not in haeuser.cod)
    (1, 100),   // 3  INFRA_SCHULE
    (1, 50),    // 4  INFRA_WIRT
    (2, 150),   // 5  INFRA_KIRCHE
    (2, 210),   // 6  INFRA_BADE
    (3, 300),   // 7  INFRA_THEATER
    (3, 250),   // 8  INFRA_HOCHSCHULE
    (2, 50),    // 9  INFRA_ARZT
    (2, 100),   // 10 INFRA_GALGEN
    (4, 1500),  // 11 INFRA_SCHLOSS
    (4, 2500),  // 12 INFRA_KATHETRALE
    (4, 25000), // 13 INFRA_TRIUMPH
    (4, 25000), // 14 INFRA_DENKMAL
    (0, 30),    // 15 INFRA_STUFE_1A   Rinderfarm
    (1, 15),    // 16 INFRA_STUFE_2A   Steinmetz, Pflasterstrassen, Brunnen
    (1, 30),    // 17 INFRA_STUFE_2B   Kontor_1, Holzmauern
    (1, 40),    // 18 INFRA_STUFE_2C   Plantage_1 (Gewuerze/Wein/Zucker)
    (1, 75),    // 19 INFRA_STUFE_2D   Getreidefarm, Mueller, Baecker
    (1, 100),   // 20 INFRA_STUFE_2E   Werkzeugschmiede
    (1, 120),   // 21 INFRA_STUFE_2F   Erzmine_1, Erzschmelze, Werft_1
    (2, 100),   // 22 INFRA_STUFE_3A   Kontor_2
    (1, 200),   // 23 INFRA_STUFE_2G   Burg_1, Schwertbauer, Wachturm
    (2, 150),   // 24 INFRA_STUFE_3B   Goldmine
    (2, 200),   // 25 INFRA_STUFE_3C   Plantage_2 (Baumwolle/Kakao), Schneider
    (2, 300),   // 26 INFRA_STUFE_3D   unused by any building
    (2, 400),   // 27 INFRA_STUFE_3E   Kanonen
    (2, 450),   // 28 INFRA_STUFE_3F   grosse Erzmine
    (3, 250),   // 29 INFRA_STUFE_4A   Kontor_3, Goldschmied, Verzierungen
    (3, 400),   // 30 INFRA_STUFE_4B   Burg_2, Musketenbauer
    (3, 500),   // 31 INFRA_STUFE_5A   grosse Werft
    (4, 600),   // 32 INFRA_STUFE_5B   Burg_3
];

/// Resolve a haeuser.cod `Bauinfra:` token to its `INFRA_*` constant id
/// (0..=32) — the single byte the original compiles into the HAUS
/// record at `+0x2f` (`1602_exe.c:67083`) and that `FUN_0042d530`
/// (`1602_exe.c:33209`) turns into the unlock bit `1 << (id - 1)`.
///
/// An absent or unrecognised token is `INFRA_NIX` (0) = always
/// buildable, matching the zero-initialised definition template.
///
/// The `BESONDERE INFRASTRUKTUR MARKPUNKTE` block of haeuser.cod
/// defines plain `NAME = NAME` substitution aliases which the COD
/// tokenizer resolves before the value ever reaches the field, so they
/// are folded in here via [`resolve_infra_alias`].
pub fn parse_bauinfra(token: &str) -> u8 {
    if token.is_empty() {
        return 0;
    }
    let resolved = resolve_infra_alias(token).unwrap_or(token);
    INFRA_NAMES
        .iter()
        .position(|name| *name == resolved)
        .unwrap_or(0) as u8
}

/// Substitute the `BESONDERE INFRASTRUKTUR MARKPUNKTE` aliases (BURG /
/// KONTOR / WACHTURM / MUSKETE / KANON) for the `INFRA_STUFE_*` rung
/// they are declared equal to in haeuser.cod. Returns `None` for tokens
/// that aren't aliases; the caller then keeps the original token, which
/// is itself an `INFRA_*` constant name if the parse is to succeed.
fn resolve_infra_alias(token: &str) -> Option<&'static str> {
    Some(match token {
        "INFRA_BURG_1" => "INFRA_STUFE_2G",
        "INFRA_BURG_2" => "INFRA_STUFE_4B",
        "INFRA_BURG_3" => "INFRA_STUFE_5B",
        "INFRA_WACHTURM" => "INFRA_STUFE_2G",
        "INFRA_MUSKETE" => "INFRA_STUFE_4B",
        "INFRA_KONTOR_1" => "INFRA_STUFE_2B",
        "INFRA_KONTOR_2" => "INFRA_STUFE_3A",
        "INFRA_KONTOR_3" => "INFRA_STUFE_4A",
        "INFRA_KANON" => "INFRA_STUFE_3E",
        _ => return None,
    })
}

/// One city's building-unlock sweep, ported from the tail of
/// `FUN_0047f8a0` @ `0x0047f8a0` (`1602_exe.c:91520-91581`, machine
/// code `0x0047fee4..0x00480010`). Returns the owning player's updated
/// 32-bit unlock mask (`player + 0x6c`, `DAT_005b76ec`).
///
/// The source walks INFRA ids `1..=0x20` with the table cursor
/// `local_2c` starting at `&DAT_0061fbc4`, skips any bit already set,
/// and grants `1 << (id - 1)` as soon as
/// `Minwohn <= cum[BGruppe]`. Grants are queued as command `0x3d/0x39`
/// carrying `mask | bit` and applied at `1602_exe.c:84932`; bits are
/// never cleared anywhere in the binary, so unlocks are permanent even
/// if the city later shrinks or is lost.
///
/// `cum` is the *cumulative* population built at
/// `1602_exe.c:91396-91402`: `cum[4] = pop[4]` and
/// `cum[k] = pop[k] + cum[k + 1]` for `k = 3..0`. A rung's `Minwohn`
/// therefore counts residents of its `BGruppe` **and every tier above
/// it** — 600 aristocrats alone satisfy every rung in the table whose
/// threshold they clear, including the `BGruppe: 0` ones.
pub fn source_city_unlock_sweep(tier_population: &[u32; 5], mask: u32) -> u32 {
    let mut cumulative = [0u32; 5];
    cumulative[4] = tier_population[4];
    for tier in (0..4).rev() {
        cumulative[tier] = tier_population[tier].saturating_add(cumulative[tier + 1]);
    }
    let mut mask = mask;
    for id in 1..=32u8 {
        let bit = 1u32 << (id - 1);
        if mask & bit != 0 {
            continue;
        }
        let (bgruppe, minwohn) = BAUINFRA_LADDER[usize::from(id)];
        if cumulative[usize::from(bgruppe)] >= u32::from(minwohn) {
            mask |= bit;
        }
    }
    mask
}

/// Load all building definitions from a parsed COD file.
pub fn load_building_defs(cod: &CodFile) -> Vec<BuildingDef> {
    cod.buildings
        .iter()
        .map(|b| convert_building_def(b))
        .collect()
}

/// Build a lookup from COD Nummer → index into building_defs vec.
pub fn nummer_to_def_index(cod: &CodFile) -> HashMap<i32, usize> {
    let mut map = HashMap::new();
    for (i, b) in cod.buildings.iter().enumerate() {
        map.entry(b.nummer).or_insert(i);
    }
    map
}

/// Build a lookup from COD Gfx (sprite index) → index into building_defs vec.
pub fn gfx_to_def_index(cod: &CodFile) -> HashMap<i32, usize> {
    cod.gfx_to_building_map()
}

/// Production kind strings that indicate a building can produce goods.
const PRODUCTION_KINDS: &[&str] = &[
    "HANDWERK",
    "ROHSTOFF",
    "PLANTAGE",
    "BERGWERK",
    "STEINBRUCH",
    "JAGDHAUS",
    "FISCHEREI",
    "WEIDETIER",
    "ROHSTWACHS",
    "ROHSTERZ",
];

/// Check if a COD building definition is a production building.
fn is_production_building(cod_building: &CodBuilding) -> bool {
    if let Some(prod_kind) = cod_building.properties.get("ProdKind") {
        PRODUCTION_KINDS.iter().any(|&k| prod_kind == k)
    } else {
        false
    }
}

/// Load building instances from a parsed SZS scenario file.
///
/// Maps each INSELHAUS tile that has a matching building definition
/// (via source definition ID) into a BuildingInstance.
/// Only creates instances for production buildings (those with production ProdKind).
pub fn load_building_instances(
    szs: &SzsFile,
    cod: &CodFile,
    building_defs: &[BuildingDef],
) -> Vec<BuildingInstance> {
    let source_id_map: HashMap<i32, usize> = cod
        .buildings
        .iter()
        .enumerate()
        .map(|(index, building)| (building.source_id, index))
        .collect();
    let mut instances = Vec::new();

    for island in &szs.islands {
        for tile in &island.tiles {
            if let Some(&def_idx) = source_id_map.get(&tile.source_id()) {
                let cod_building = &cod.buildings[def_idx];

                // Only create instances for actual production buildings
                if !is_production_building(cod_building) {
                    continue;
                }

                let def = &building_defs[def_idx];
                // Skip terrain/decoration tiles (GRAS, NOWARE, BAUM, etc.)
                if def.output_good == Good::None {
                    continue;
                }

                // Each island's STADT4 chunk carries the slot
                // number that owns its city — that's the closest
                // proxy to per-tile ownership, since INSELHAUS
                // tiles don't carry an explicit owner byte.
                // Islands without a city default to slot 0 (the
                // player) which matches the original engine's
                // behaviour for player-built tiles on uncolonised
                // land.
                let owner = island.city.as_ref().map(|c| c.owner_slot).unwrap_or(0);
                let mut instance = BuildingInstance::new(
                    def_idx as u16,
                    island.number,
                    tile.x as u16,
                    tile.y as u16,
                    owner,
                );
                instance.source_placement_command = Some(
                    crate::building::SourceBuildingCommand::from_island_tile(*tile),
                );
                instances.push(instance);
            }
        }
    }

    instances
}

/// Replay INSELHAUS overwrite order into the renderer-relevant subset of the
/// source map-cell records. `FUN_00481450` removes records whose command
/// roots are overwritten before `FUN_00481fc0` creates the new root record;
/// this bridge retains the selector-bearing source kinds 1 through 8 plus
/// nested production-kind-2 plantation roots. Their outer map kind only
/// supplies the command footprint; `FUN_0047daf0` dispatches workers through
/// the nested production kind.
pub fn source_map_cell_states_from_scenario(
    szs: &SzsFile,
    cod: &CodFile,
) -> Vec<SourceMapCellState> {
    source_map_roots_from_scenario(szs, cod, false)
}

/// Replay final INSELHAUS overwrite order into the static roots consumed by
/// `FUN_0047a650`. This includes non-selector map kinds that are absent from
/// the renderer's live source-cell table.
pub fn source_static_map_roots_from_scenario(
    szs: &SzsFile,
    cod: &CodFile,
) -> Vec<SourceMapCellState> {
    source_map_roots_from_scenario(szs, cod, true)
}

/// Resolve the resource selector compiled at definition offset `+0xa9` for
/// `ROHSTWACHS` records. The authored resource groups place their usable
/// `ROHSTOFF` definition immediately next to each growth/dry record.
fn source_growth_resource_ware_slot(cod: &CodFile, definition: &CodBuilding) -> u8 {
    if definition.source_production_kind_code() != Some(10) {
        return 0;
    }
    let Some(index) = cod
        .buildings
        .iter()
        .position(|candidate| std::ptr::eq(candidate, definition))
    else {
        return 0;
    };
    [index.checked_add(1), index.checked_sub(1)]
        .into_iter()
        .flatten()
        .filter_map(|neighbor| cod.buildings.get(neighbor))
        .find(|candidate| candidate.source_production_kind_code() == Some(9))
        .and_then(CodBuilding::source_ware_slot)
        .unwrap_or_default()
}

/// Replay the source loader's `+0xafc` backing map. `FUN_00468550` copies
/// only owner-slot-7 non-live definitions through `FUN_00463e10`; later
/// `FUN_004641d0` uses this map when a `Ruinenr = 0xff` command is removed.
pub fn source_static_map_backing_cells_from_scenario(
    szs: &SzsFile,
    cod: &CodFile,
) -> Vec<SourceMapCellState> {
    let mut cells = HashMap::new();
    for island in &szs.islands {
        for tile in &island.tiles {
            let Some(definition) = cod.building_by_source_id(tile.source_id()) else {
                continue;
            };
            if !source_loader_copies_static_backing(cod, *tile, definition) {
                continue;
            }
            let (width, height) = if matches!(tile.orientation & 3, 1 | 3) {
                (definition.size.1, definition.size.0)
            } else {
                definition.size
            };
            if width <= 0 || height <= 0 {
                continue;
            }
            let command = crate::building::SourceBuildingCommand::from_island_tile(*tile);
            let Some(mut root) =
                SourceMapCellState::new_static(island.number, tile.x, tile.y, definition, 0)
            else {
                continue;
            };
            root.source_growth_resource_ware_slot =
                source_growth_resource_ware_slot(cod, definition);
            root.set_footprint(width, height);
            root.set_source_command(command);
            root.configure_terminal_replacement(cod);
            for dy in 0..height {
                for dx in 0..width {
                    let x = i32::from(tile.x) + dx;
                    let y = i32::from(tile.y) + dy;
                    let (Ok(x), Ok(y)) = (u8::try_from(x), u8::try_from(y)) else {
                        continue;
                    };
                    let mut cell = root;
                    cell.x = x;
                    cell.y = y;
                    cell.source_definition_offset =
                        root.source_definition_offset_at(dx as u8, dy as u8);
                    cells.insert((island.number, u16::from(x), u16::from(y)), cell);
                }
            }
        }
    }
    cells.into_values().collect()
}

/// `FUN_00468550` copies only owner-slot-7 definitions whose outer `Kind`
/// code is neither 12 nor 29 and for which `FUN_00480b70` returns zero.
/// That helper switches on the nested production-kind code. Kind 9 uses its
/// compiled `Ware` byte; kind 10 uses the associated `+0xa9` resource
/// selector reconstructed from its neighboring kind-9 definition. Kind 0
/// checks the outer kind code directly.
fn source_loader_copies_static_backing(
    cod: &CodFile,
    tile: anno_formats::szs::IslandTile,
    definition: &CodBuilding,
) -> bool {
    if tile.source_owner() != 7 {
        return false;
    }
    let outer_kind = definition.source_kind_code();
    if matches!(outer_kind, Some(12 | 29)) {
        return false;
    }
    match definition.source_production_kind_code() {
        Some(9) => definition
            .source_ware_slot()
            .is_some_and(|slot| matches!(slot, 0x34 | 0x35 | 0x39)),
        Some(10) => cod
            .buildings
            .iter()
            .position(|candidate| std::ptr::eq(candidate, definition))
            .and_then(|index| cod.buildings.get(index + 1))
            .and_then(CodBuilding::source_ware_slot)
            .is_some_and(|slot| matches!(slot, 0x34 | 0x35 | 0x39)),
        Some(1..=8 | 13..=15 | 17..=27 | 30 | 31) | None => false,
        Some(12 | 29) => false,
        Some(0) => !matches!(outer_kind, Some(1 | 30)),
        Some(_) => true,
    }
}

fn source_map_roots_from_scenario(
    szs: &SzsFile,
    cod: &CodFile,
    include_all_static_kinds: bool,
) -> Vec<SourceMapCellState> {
    let mut states = Vec::new();
    let mut final_kind_cells = HashMap::new();
    let mut static_cells = HashMap::new();

    for island in &szs.islands {
        for tile in &island.tiles {
            let Some(definition) = cod.building_by_source_id(tile.source_id()) else {
                continue;
            };
            let (width, height) = if matches!(tile.orientation & 3, 1 | 3) {
                (definition.size.1, definition.size.0)
            } else {
                definition.size
            };
            if width <= 0 || height <= 0 {
                continue;
            }
            let right = i32::from(tile.x) + width;
            let bottom = i32::from(tile.y) + height;
            states.retain(|state: &SourceMapCellState| {
                state.island != island.number
                    || i32::from(state.x) < i32::from(tile.x)
                    || i32::from(state.x) >= right
                    || i32::from(state.y) < i32::from(tile.y)
                    || i32::from(state.y) >= bottom
            });

            let kind_code = definition.source_kind_code().unwrap_or(u8::MAX);
            for dy in 0..height {
                for dx in 0..width {
                    let x = i32::from(tile.x) + dx;
                    let y = i32::from(tile.y) + dy;
                    let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
                        continue;
                    };
                    final_kind_cells.insert((island.number, x, y), kind_code);
                }
            }

            let command = crate::building::SourceBuildingCommand::from_island_tile(*tile);
            let static_state =
                SourceMapCellState::new_static(island.number, tile.x, tile.y, definition, 0).map(
                    |mut state| {
                        state.source_growth_resource_ware_slot =
                            source_growth_resource_ware_slot(cod, definition);
                        state.set_footprint(width, height);
                        state.set_source_command(command);
                        state.configure_terminal_replacement(cod);
                        state
                    },
                );
            if include_all_static_kinds {
                if let Some(state) = static_state {
                    for dy in 0..height {
                        for dx in 0..width {
                            let x = i32::from(tile.x) + dx;
                            let y = i32::from(tile.y) + dy;
                            let (Ok(x), Ok(y)) = (u8::try_from(x), u8::try_from(y)) else {
                                continue;
                            };
                            let mut cell = state;
                            cell.x = x;
                            cell.y = y;
                            cell.source_definition_offset =
                                state.source_definition_offset_at(dx as u8, dy as u8);
                            static_cells.insert((island.number, u16::from(x), u16::from(y)), cell);
                        }
                    }
                }
            }

            // `FUN_00481450` allocates the live `FUN_0047cbf0` record from the
            // nested `HAUS_PRODTYP Kind` at definition offset `+0x1c`
            // (`1602_exe.c:92790-92892`), never from the outer `HAUS Kind`.
            if !include_all_static_kinds
                && !static_state.is_some_and(SourceMapCellState::allocates_source_scheduler_record)
            {
                continue;
            }
            if let Some(mut state) = static_state {
                state.set_footprint(width, height);
                state.set_source_command(command);
                states.push(state);
            }
        }
    }

    if include_all_static_kinds {
        for state in static_cells.values_mut() {
            let mut selectors = 0_u64;
            for dy in 0..usize::from(state.footprint_height) {
                for dx in 0..usize::from(state.footprint_width) {
                    let source_order_index = dy * usize::from(state.footprint_width)
                        + (usize::from(state.footprint_width) - 1 - dx);
                    let x = u16::from(state.x) + dx as u16;
                    let y = u16::from(state.y) + dy as u16;
                    if source_order_index < u64::BITS as usize
                        && final_kind_cells
                            .get(&(state.island, x, y))
                            .is_some_and(|kind| matches!(*kind, 23..=27))
                    {
                        selectors |= 1_u64 << source_order_index;
                    }
                }
            }
            state.set_fallback_strand_cells(selectors);
        }
        return static_cells.into_values().collect();
    }

    for state in &mut states {
        let mut selectors = 0_u64;
        for dy in 0..usize::from(state.footprint_height) {
            for dx in 0..usize::from(state.footprint_width) {
                let source_order_index = dy * usize::from(state.footprint_width)
                    + (usize::from(state.footprint_width) - 1 - dx);
                let x = u16::from(state.x) + dx as u16;
                let y = u16::from(state.y) + dy as u16;
                if source_order_index < u64::BITS as usize
                    && final_kind_cells
                        .get(&(state.island, x, y))
                        .is_some_and(|kind| matches!(*kind, 23..=27))
                {
                    selectors |= 1_u64 << source_order_index;
                }
            }
        }
        state.set_fallback_strand_cells(selectors);
    }

    states
}

/// Reconstruct the five `DAT_0061fa84[BGruppe]` housing definitions used by
/// `FUN_0047bbc0` and `FUN_0047c080`.
///
/// The haeuser loader stores the first parsed nested `WOHNUNG` definition for
/// each BGruppe. `Werkzeug`, `Holz`, `Ziegel`, and `Kanon` are shifted left
/// five when compiled at offsets `+0x4c..+0x52`; raw `Money` occupies `+0x54`
/// (`1602_exe.c:67386-67531`). Its `RandAnz/RandAdd` layout selects the
/// contiguous replacement definition before `FUN_004631b0` writes the map
/// command.
pub fn source_kind13_promotion_definitions(
    cod: &CodFile,
) -> [Option<SourceKind13PromotionDefinition>; 5] {
    std::array::from_fn(|group| {
        let group = group as u8;
        let base = cod.source_population_group_building(group)?;
        let fixed_cost = |key: &str| {
            (base
                .properties
                .get(key)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0)
                .max(0) as u16)
                .wrapping_shl(5)
        };
        let money_cost = base
            .properties
            .get("Money")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0)
            .max(0) as u32;
        let variant_count = base.rand_anz.max(1);
        let source_size = (
            u8::try_from(base.size.0).ok()?.max(1),
            u8::try_from(base.size.1).ok()?.max(1),
        );
        let variant_definition_offsets = (0..variant_count)
            .map(|rand_value| {
                let variant = cod.source_population_group_variant(group, rand_value)?;
                let offset = variant
                    .source_id
                    .checked_sub(anno_formats::szs::INSELHAUS_SOURCE_ID_BASE)?;
                u16::try_from(offset).ok()
            })
            .collect::<Option<Vec<_>>>()?;
        Some(SourceKind13PromotionDefinition {
            target_group: group,
            source_size,
            tools_cost_fixed: fixed_cost("Werkzeug"),
            wood_cost_fixed: fixed_cost("Holz"),
            bricks_cost_fixed: fixed_cost("Ziegel"),
            cannons_cost_fixed: fixed_cost("Kanon"),
            money_cost,
            variant_definition_offsets,
        })
    })
}

/// Extract the placement anchors feeding the source kind-13 location table.
///
/// The executable inserts one entry through `FUN_00478b90` for each live
/// `PLATZ`/`WOHNUNG` root. This preserves scenario command order, so later
/// commands that overwrite its oriented footprint remove the earlier entry.
pub fn source_kind13_locations_from_scenario(
    szs: &SzsFile,
    cod: &CodFile,
) -> SourceKind13LocationTable {
    let mut locations = SourceKind13LocationTable::default();

    // Islands whose authored kind-13 list ships in SIEDLER records are
    // restored from those records alone: the source's SIEDLER loader
    // overwrites the INSELHAUS-created defaults (subtracting the created
    // residents from the city first — the `0x484054` step), so tile-scan
    // insertion would add phantom records the original does not have
    // (e.g. New Horizons0's plaza tiles produced four group-3 residents
    // per AI island that wrapped the city populations negative).
    let siedler_islands: std::collections::HashSet<u8> = szs
        .settler_houses
        .iter()
        .map(|house| house.island_id)
        .collect();
    for island in &szs.islands {
        for tile in &island.tiles {
            let Some(definition) = cod.building_by_source_id(tile.source_id()) else {
                continue;
            };
            let (width, height) = if matches!(tile.orientation & 3, 1 | 3) {
                (definition.size.1, definition.size.0)
            } else {
                definition.size
            };
            if width <= 0 || height <= 0 {
                continue;
            }

            locations.remove_roots_in_footprint(island.number, tile.x, tile.y, width, height);
            if definition.source_kind_code() != Some(13) {
                continue;
            }
            if siedler_islands.contains(&island.number) {
                continue;
            }
            locations.insert(SourceKind13Location {
                island_id: island.number,
                tile_x: tile.x,
                tile_y: tile.y,
                orientation: tile.orientation & 3,
                variant: (tile.orientation >> 2) & 0x0f,
                source_owner: tile.source_owner(),
                phase: 0,
                state_bits: 0,
                population_group: definition.source_population_group().unwrap_or(0),
                amount: source_kind13_initial_amount(),
                lifecycle_flags: 0,
            });
        }
    }

    // Authored residences ship in the per-island SIEDLER chunks, not in
    // the INSELHAUS tile layer — e.g. Exile's 26 settler houses (26 ×
    // amount 0x180 = the STADT4 population of 156). The source loader
    // (`0x483ee0`) rebuilds the runtime house list from these records,
    // deriving each house's city from the map cell selector; the city
    // populations themselves come from STADT4, so seeding locations here
    // adds no residents.
    for house in &szs.settler_houses {
        let source_owner = szs
            .islands
            .iter()
            .find(|island| island.number == house.island_id)
            .and_then(|island| {
                island
                    .tiles
                    .iter()
                    .find(|tile| tile.x == house.tile_x && tile.y == house.tile_y)
            })
            .map(|tile| tile.source_owner())
            .unwrap_or(0);
        locations.insert(SourceKind13Location {
            island_id: house.island_id,
            tile_x: house.tile_x,
            tile_y: house.tile_y,
            orientation: 0,
            variant: house.variant & 0x0f,
            source_owner,
            phase: house.phase,
            state_bits: house.state_bits,
            population_group: house.population_group,
            amount: house.amount,
            lifecycle_flags: house.lifecycle_flags,
        });
    }

    locations
}

/// Reconstruct the source island map-object tables that the INSELHAUS loader
/// creates for `Kind=HQ` definitions.
///
/// `FUN_00465170` allocates the first free slot when the tile's current
/// three-bit map-owner value does not already name a live object. It then
/// writes that slot across the definition's oriented footprint via
/// `FUN_0046ae20`. INSELHAUS stores the definition offset, so the lookup here
/// must use `IslandTile::source_id`, not the definition's GFX value.
pub fn source_dynamic_map_objects_from_scenario(
    szs: &SzsFile,
    cod: &CodFile,
) -> Vec<SourceDynamicMapObject> {
    let mut objects = Vec::new();

    for island in &szs.islands {
        let mut table = SourceDynamicMapObjectTable::new(island.number);
        let mut slot_overlay = HashMap::<(u8, u8), u8>::new();

        for tile in &island.tiles {
            let Some(definition) = cod.building_by_source_id(tile.source_id()) else {
                continue;
            };
            if definition.source_kind_code() != Some(0x23) {
                continue;
            }

            let current_slot = slot_overlay
                .get(&(tile.x, tile.y))
                .copied()
                .unwrap_or_else(|| tile.source_owner());
            if table.object(current_slot).is_some() {
                continue;
            }

            let Some(object) = table.allocate(tile.source_dynamic_object_owner(), (tile.x, tile.y))
            else {
                continue;
            };

            let (width, height) = if matches!(tile.orientation, 1 | 3) {
                (definition.size.1, definition.size.0)
            } else {
                definition.size
            };
            if width <= 0 || height <= 0 {
                continue;
            }

            for y in i32::from(tile.y)..i32::from(tile.y) + height {
                for x in i32::from(tile.x)..i32::from(tile.x) + width {
                    if x < i32::from(island.width) && y < i32::from(island.height) {
                        slot_overlay.insert((x as u8, y as u8), object.slot);
                    }
                }
            }
        }

        objects.extend(table.objects());
    }

    objects
}

/// Locate KONTOR (warehouse) tiles in INSELHAUS data and
/// emit a `Warehouse` per occurrence, anchored on the actual
/// tile position rather than an averaged centroid. This is
/// faithful to where the scenario author placed the Kontor.
///
/// Caller pairs this with each island's `city.owner_slot`
/// when present so the warehouse inherits the right slot;
/// uncolonised islands default to slot 0.
///
/// Capacity comes from haeuser.cod's `Maxlager` field on the
/// matching building def — KONTOR_1 = 50, KONTOR_2 = 75,
/// KONTOR_3 = 100.
pub fn kontor_warehouses_from_szs(
    szs: &anno_formats::szs::SzsFile,
    cod: &CodFile,
    building_defs: &[BuildingDef],
) -> Vec<crate::warehouse::Warehouse> {
    use crate::warehouse::Warehouse;
    let source_id_map: HashMap<i32, usize> = cod
        .buildings
        .iter()
        .enumerate()
        .map(|(index, building)| (building.source_id, index))
        .collect();
    let mut out = Vec::new();
    for island in &szs.islands {
        let owner = island.city.as_ref().map(|c| c.owner_slot).unwrap_or(0);
        for tile in &island.tiles {
            let Some(&def_idx) = source_id_map.get(&tile.source_id()) else {
                continue;
            };
            let Some(def) = building_defs.get(def_idx) else {
                continue;
            };
            // ProdKind=KONTOR identifies warehouse tiles.
            if def.prod_kind != "KONTOR" {
                continue;
            }
            // Carry the Kontor's authored storage capacity (50/
            // 75/100 tons across KONTOR_1/2/3, 20 t for the
            // small variants) into the warehouse so deposits
            // hit the right ceiling instead of the legacy 30 t
            // default.
            let cap = if def.storage_capacity > 0 {
                def.storage_capacity
            } else {
                30
            };
            let city_population = island
                .city
                .as_ref()
                .map(|city| city.tier_population)
                .unwrap_or([0; 5]);
            let mut warehouse = Warehouse::with_capacity_and_population(
                island.number,
                owner,
                tile.x as u16,
                tile.y as u16,
                cap,
                city_population,
            );
            let footprint = if tile.orientation & 1 == 0 {
                (def.width.max(1), def.height.max(1))
            } else {
                (def.height.max(1), def.width.max(1))
            };
            warehouse.set_source_footprint(footprint);
            warehouse.set_source_path_class(crate::carrier::source_path_class(def.wegspeed[0]));
            out.push(warehouse);
        }
    }
    out
}

/// Seed the two directed source diplomacy tables from PLAYER4. In
/// `FUN_00478160`, slot offset `0xC0` receives `DAT_005b7770` and slot offset
/// `0x140` receives `DAT_005b77b0`; both arrays use the source's eight-byte
/// per-target stride. `FUN_0045cd20` excludes a combat candidate only when
/// the former table's byte is `3`.
pub fn diplomacy_from_player4_relationships(
    players: &[anno_formats::szs::PlayerSlotInit],
) -> crate::combat::DiplomacyMatrix {
    let mut dm = crate::combat::DiplomacyMatrix::new();
    let n = players.len().min(7);
    for i in 0..n {
        for j in 0..n {
            dm.set_source_relationship_code(i as u8, j as u8, players[i].relations_0xc0[j]);
            dm.set_source_attitude_code(i as u8, j as u8, players[i].relationships[j] as u8);
            dm.set_source_diplomacy_score_inputs(
                i as u8,
                j as u8,
                players[i].diplomacy_activity_0x80[j],
                players[i].diplomacy_base_0x40[j],
                players[i].diplomacy_scale_0x60[j],
            );
        }
        dm.set_source_diplomacy_policy_inputs(
            i as u8,
            players[i].state_byte,
            players[i].diplomacy_policy_flags_0x1c,
            players[i].diplomacy_peer_population_threshold_0x20,
            players[i].diplomacy_own_population_threshold_0x22,
            players[i].diplomacy_own_city_strength_0x24,
            players[i].diplomacy_peer_city_strength_0x25,
        );
    }
    dm
}

/// Map a raw PLAYER4 `relations_0xc0` code to the source candidate exclusion
/// state recovered from `FUN_0045cd20`.
pub fn code_to_diplomacy(code: u32) -> crate::combat::Diplomacy {
    use crate::combat::Diplomacy;
    match code {
        3 => Diplomacy::Allied,
        0..=2 => Diplomacy::Neutral,
        _ => Diplomacy::Neutral,
    }
}

/// Building Nummer references for native + pirate dwellings,
/// derived from haeuser.cod's `Nativflg=1` / `Piratflg=1`
/// tagged entries (`cargo run --example
/// probe_native_pirate_buildings`).
///
/// Native village (slot 5):
///   442 = Chief's hut / Kontor (variant A)
///   443 = Warrior's hut (MILITAR)
///   444 = Native dwelling (PIRATWOHN)
///   445 = Spice plantation (GEWUERZE)
///   446–447 = Tobacco plantations (TABAKWAREN)
///   448 = Chief's hut / Kontor (variant B)
///   449–450 = Additional native dwellings + warrior hut
///   451–454 = More native plantations (incl. cliff-side TABAK)
///
/// Pirate stronghold (slot 6):
///   455 = Pirate Kontor (also Nativflg=1 in COD — the same
///         building doubles as both faction's hub)
///   456–458 = Pirate dwellings (PIRATWOHN)
///   459–460 = Pirate watchtowers (WACHTURM)
pub const NATIVE_KONTOR_A: i32 = 442;
pub const NATIVE_KONTOR_B: i32 = 448;
pub const PIRATE_KONTOR: i32 = 455;

/// All native-faction building Nummers (442..=458 inclusive).
pub const NATIVE_BUILDING_NUMMERS: std::ops::RangeInclusive<i32> = 442..=458;

/// All pirate-faction building Nummers (455..=460 inclusive,
/// overlapping the native range at 455-458).
pub const PIRATE_BUILDING_NUMMERS: std::ops::RangeInclusive<i32> = 455..=460;

/// `route_id` value used for SHIP4 traders that have no
/// configured route. Picked so it can never collide with a
/// real `TradeRoute::id` (those start at 0 and grow
/// monotonically). `tick_trade_ship` skips ships whose
/// route_id doesn't match any active route, so these
/// "stranded" traders sit at their spawn coordinates until
/// the player or AI assigns them to a route.
pub const UNROUTED_TRADER_ROUTE_ID: u16 = u16::MAX;

/// Convert SHIP4 records whose `ShipClass::is_warship()` is
/// true into `MilitaryUnit` instances for the simulation's
/// naval combat path. Trader ships are skipped — those need a
/// `TradeShip` with a route, which the scenario doesn't seed
/// directly. Returns the new units in the same order as the
/// underlying SHIP4 records, so callers can correlate by
/// index when annotating ship names later.
pub fn warships_from_ships(ships: &[anno_formats::szs::Ship]) -> Vec<crate::combat::MilitaryUnit> {
    use crate::combat::{MilitaryUnit, UnitType};
    use anno_formats::szs::ShipClass;
    ships
        .iter()
        .filter_map(|s| {
            let class = s.class()?;
            let unit_type = match class {
                ShipClass::SmallWarship => UnitType::SmallWarship,
                ShipClass::LargeWarship => UnitType::LargeWarship,
                ShipClass::PirateShip => UnitType::PirateShip,
                _ => return None,
            };
            let mut unit =
                MilitaryUnit::with_name(unit_type, s.owner, s.x as i32, s.y as i32, s.name.clone());
            unit.source_live_runtime_slot = Some(s.runtime_slot);
            unit.source_candidate_list_key = Some(s.candidate_list_key);
            unit.source_figure_kind = Some(s.figure_kind);
            unit.source_figure_definition_id = Some(s.figure_definition_id);
            unit.source_energy = s.stored_energy;
            unit.source_score_state = s.heading_byte;
            unit.source_kind6_policy_raw_slots = s.source_kind6_policy_raw_slots();
            unit.source_kind6_target_descriptor_payload =
                Some(s.source_kind6_target_descriptor_payload());
            unit.source_cargo_slots = s.cargo_slots;
            unit.direction = s.source_direction;
            Some(unit)
        })
        .collect()
}

/// Populate the compiled category-6 policy bytes after SHIP4 entities have
/// been created. The original loader resolves each low-16 raw value through
/// the same haeuser definition table before combat dispatch reads it.
pub fn resolve_ship_kind6_policy_slots(
    cod: &CodFile,
    warships: &mut [crate::combat::MilitaryUnit],
    traders: &mut [crate::trade::TradeShip],
) {
    for ship in warships {
        ship.source_kind6_policy_ware_slots =
            cod.source_kind6_policy_ware_slots(ship.source_kind6_policy_raw_slots);
    }
    for ship in traders {
        ship.source_kind6_policy_ware_slots =
            cod.source_kind6_policy_ware_slots(ship.source_kind6_policy_raw_slots);
    }
}

/// Convert SHIP4 records whose class is `SmallTrader` or
/// `LargeTrader` into `TradeShip` instances. The resulting
/// ships have `route_id = UNROUTED_TRADER_ROUTE_ID` (a
/// sentinel that never matches a real route), so the trade
/// tick leaves them inert until a route is assigned. They
/// still spawn at their authored coordinates so the player
/// sees them in the world.
pub fn traders_from_ships(
    ships: &[anno_formats::szs::Ship],
    cargo_config: crate::trade::ShipCargoConfig,
) -> Vec<crate::trade::TradeShip> {
    use crate::trade::{TradeShip, TradeShipClass};
    use anno_formats::szs::ShipClass;
    ships
        .iter()
        .filter(|s| {
            matches!(
                s.class(),
                Some(ShipClass::SmallTrader | ShipClass::LargeTrader)
            )
        })
        .map(|s| {
            let class = match s.class().expect("filtered trader ship class") {
                ShipClass::SmallTrader => TradeShipClass::SmallTrader,
                ShipClass::LargeTrader => TradeShipClass::LargeTrader,
                _ => unreachable!("filtered to trader classes"),
            };
            let mut t = TradeShip::new_with_class(
                s.owner,
                UNROUTED_TRADER_ROUTE_ID,
                s.x as i32,
                s.y as i32,
                class,
                cargo_config.capacity_for(class),
            )
            .with_name(s.name.clone());
            // Carry the authored heading so the renderer
            // shows the ship facing the right direction.
            t.heading = s.heading();
            t.source_figure_kind = Some(s.figure_kind);
            t.source_runtime_slot = Some(s.runtime_slot);
            t.source_candidate_list_key = Some(s.candidate_list_key);
            t.source_direction = s.source_direction;
            t.source_figure_definition_id = Some(s.figure_definition_id);
            t.source_energy = s.stored_energy;
            t.source_score_state = s.heading_byte;
            t.source_kind6_policy_raw_slots = s.source_kind6_policy_raw_slots();
            t.source_kind6_target_descriptor_payload =
                Some(s.source_kind6_target_descriptor_payload());
            t.source_cargo_slots = s.cargo_slots;
            // Decode the authored cargo into the typed hold. The packed
            // low byte uses the ship-cargo id space (2 = tools, 7 = wood,
            // 4 = food — the only ids in the shipping corpus,
            // live-verified against the original's hold UI); bits 8..=21
            // carry the 1/32-good quantity.
            for &slot in &s.cargo_slots {
                if slot == 0 {
                    continue;
                }
                let good = match slot & 0xff {
                    2 => Good::Tools,
                    7 => Good::Wood,
                    4 => Good::Food,
                    _ => continue,
                };
                let quantity = (((slot >> 8) & 0x3fff) / 32) as u16;
                if quantity > 0 {
                    // Authored cargo is stored per source slot and may
                    // exceed the config-derived total capacity clamp the
                    // interactive `load` applies; take it as shipped.
                    t.load_unchecked(good, quantity);
                }
            }
            t.source_target_approach_radius = cargo_config.target_approach_radius_for(class);
            t
        })
        .collect()
}

/// Whether a plantation/farm building can be placed on the
/// given island. The check is purely a fertility lookup —
/// ownership, infrastructure tier, and tile-level placement
/// rules are validated by other passes.
///
/// Universal buildings (`required_fertility = None`, e.g.
/// foresters, brick kilns) always pass. Fertility-bound
/// plantations (`Some(Fertility::Tobacco)`, etc.) require
/// the corresponding non-sentinel byte in the island's
/// 8-slot fertility map.
///
/// Pre-placed scenario buildings are NOT subject to this
/// check — `load_building_instances` honours the scenario
/// author's decisions verbatim. The check applies to the
/// player/AI build-action path, where the original engine
/// rejects placements that violate the fertility gate.
pub fn island_can_host_building(def: &BuildingDef, island: &anno_formats::szs::Island) -> bool {
    let Some(req) = def.required_fertility else {
        return true;
    };
    island.active_fertilities().contains(&req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warships_from_ships_routes_warships_only() {
        use crate::combat::UnitType;
        use anno_formats::szs::Ship;
        let mk = |owner: u8, class: u8, x: u16, y: u16| {
            let mut raw_record = [0; anno_formats::szs::SHIP4_RECORD_BYTES];
            raw_record[0x2c..0x2e].copy_from_slice(&[0x61, 0x72]);
            raw_record[0x132..0x13a].copy_from_slice(&0x8877_6655_4433_2211_u64.to_le_bytes());
            Ship {
                raw_record,
                name: "test".into(),
                x,
                y,
                owner,
                figure_definition_id: class.into(),
                ship_class: class,
                stored_energy: u16::from(class) + 100,
                runtime_slot: class.into(),
                figure_kind: if owner == 5 { 3 } else { 1 },
                candidate_list_key: 9,
                source_direction: 6,
                animation_state: 0,
                heading_byte: 4,
                cargo_slots: [0; 7],
            }
        };
        let ships = vec![
            mk(0, 0x15, 10, 10), // SmallTrader  → skip
            mk(1, 0x19, 20, 20), // SmallWarship → keep
            mk(2, 0x1B, 30, 30), // LargeWarship → keep
            mk(0, 0x17, 40, 40), // LargeTrader  → skip
            mk(5, 0x1F, 50, 50), // PirateShip   → keep
            mk(0, 0xFE, 0, 0),   // unknown     → skip
        ];
        let units = warships_from_ships(&ships);
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].unit_type, UnitType::SmallWarship);
        assert_eq!(units[0].owner, 1);
        assert_eq!(units[0].source_figure_kind, Some(1));
        assert_eq!(units[0].source_runtime_slot, None);
        assert_eq!(units[0].source_live_runtime_slot, Some(0x19));
        assert_eq!(units[0].source_candidate_list_key, Some(9));
        assert_eq!(units[0].source_figure_definition_id, Some(0x19));
        assert_eq!(units[0].source_energy, 125);
        assert_eq!(units[0].source_score_state, 4);
        assert_eq!(
            units[0].source_kind6_policy_raw_slots[0],
            0x8877_6655_4433_2211
        );
        assert_eq!(
            units[0].source_kind6_target_descriptor_payload,
            Some([0x61, 0x72])
        );
        assert_eq!(units[0].direction, 6);
        assert_eq!(units[1].unit_type, UnitType::LargeWarship);
        assert_eq!(units[2].unit_type, UnitType::PirateShip);
        assert_eq!(units[2].owner, 5);
        assert_eq!(units[2].source_figure_kind, Some(3));
        // Position should round-trip from u16 → i32.
        assert_eq!(units[0].tile_x, 20);
        assert_eq!(units[0].tile_y, 20);
    }

    #[test]
    fn traders_from_ships_routes_traders_only_with_sentinel_id() {
        use anno_formats::szs::Ship;
        let mk = |owner: u8, class: u8, x: u16, y: u16| {
            let mut raw_record = [0; anno_formats::szs::SHIP4_RECORD_BYTES];
            raw_record[0x2c..0x2e].copy_from_slice(&[0x28, 0x39]);
            raw_record[0x13a..0x142].copy_from_slice(&0x0123_4567_89ab_cdef_u64.to_le_bytes());
            Ship {
                raw_record,
                name: "test".into(),
                x,
                y,
                owner,
                figure_definition_id: class.into(),
                ship_class: class,
                stored_energy: u16::from(class) + 100,
                runtime_slot: class.into(),
                figure_kind: if owner == 5 { 3 } else { 1 },
                candidate_list_key: 9,
                source_direction: 6,
                animation_state: 0,
                heading_byte: 4,
                cargo_slots: [0; 7],
            }
        };
        let ships = vec![
            mk(0, 0x15, 10, 10), // SmallTrader  → keep
            mk(1, 0x19, 20, 20), // SmallWarship → skip
            mk(2, 0x17, 30, 30), // LargeTrader  → keep
            mk(0, 0x1B, 40, 40), // LargeWarship → skip
            mk(0, 0x1F, 50, 50), // PirateShip   → skip
            mk(0, 0xFE, 0, 0),   // unknown     → skip
        ];
        let mut cargo_config = crate::trade::ShipCargoConfig::default();
        cargo_config.small_trader_target_approach_radius = 4;
        cargo_config.large_trader_target_approach_radius = 2;
        let traders = traders_from_ships(&ships, cargo_config);
        assert_eq!(traders.len(), 2);
        for t in &traders {
            assert_eq!(
                t.route_id, UNROUTED_TRADER_ROUTE_ID,
                "spawn ships use the sentinel route id"
            );
            assert!(t.active, "spawn ships start active");
        }
        assert_eq!(traders[0].owner, 0);
        assert_eq!(traders[0].name, "test");
        assert_eq!(traders[0].world_x, 10);
        assert_eq!(traders[0].source_figure_kind, Some(1));
        assert_eq!(traders[0].source_runtime_slot, Some(0x15));
        assert_eq!(traders[0].source_candidate_list_key, Some(9));
        assert_eq!(traders[0].source_direction, 6);
        assert_eq!(traders[0].source_figure_definition_id, Some(0x15));
        assert_eq!(traders[0].source_energy, 121);
        assert_eq!(traders[0].source_score_state, 4);
        assert_eq!(
            traders[0].source_kind6_policy_raw_slots[1],
            0x0123_4567_89ab_cdef
        );
        assert_eq!(
            traders[0].source_kind6_target_descriptor_payload,
            Some([0x28, 0x39])
        );
        assert_eq!(traders[0].heading, 2);
        assert_eq!(traders[0].class, crate::trade::TradeShipClass::SmallTrader);
        assert_eq!(traders[0].cargo_capacity(), 40);
        assert_eq!(traders[0].source_target_approach_radius, 4);
        assert_eq!(traders[1].owner, 2);
        assert_eq!(traders[1].world_x, 30);
        assert_eq!(traders[1].class, crate::trade::TradeShipClass::LargeTrader);
        assert_eq!(traders[1].cargo_capacity(), 60);
        assert_eq!(traders[1].source_target_approach_radius, 2);
    }

    #[test]
    fn ship_kind6_policy_resolution_uses_compiled_ware_and_special_ids() {
        use anno_formats::cod::BuildingDef as CodBuilding;

        let cod = CodFile {
            constants: HashMap::new(),
            buildings: vec![CodBuilding {
                source_id: anno_formats::cod::SOURCE_DEFINITION_ID_BASE + 7,
                properties: HashMap::from([("Ware".into(), "wKANONIER2".into())]),
                ..Default::default()
            }],
        };
        let mut warships = vec![crate::combat::MilitaryUnit::new(
            crate::combat::UnitType::SmallWarship,
            1,
            4,
            5,
        )];
        warships[0].source_kind6_policy_raw_slots[0] = 7;
        let mut traders = vec![crate::trade::TradeShip::new_with_capacity(2, 9, 6, 7, 40)];
        traders[0].source_kind6_policy_raw_slots[0] = 0x26ad;

        resolve_ship_kind6_policy_slots(&cod, &mut warships, &mut traders);

        assert_eq!(warships[0].source_kind6_policy_ware_slots[0], 0x26);
        assert_eq!(traders[0].source_kind6_policy_ware_slots[0], 0x19);
    }

    #[test]
    fn soldat3_kind_four_records_supply_island_owner_occupancy() {
        use anno_formats::szs::{LandFigure, LandFigureFamily, ScenarioMeta, SOLDAT3_RECORD_BYTES};

        let scenario = SzsFile {
            chunks: Vec::new(),
            islands: Vec::new(),
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
            land_figures: vec![
                LandFigure {
                    raw_record: [0; SOLDAT3_RECORD_BYTES],
                    x: 0,
                    y: 0,
                    source_energy: 18,
                    figure_definition_id: 5,
                    runtime_slot: 14,
                    origin_descriptor: [0x33, 7, 0, 0],
                    route_radius: 7,
                    figure_kind: 4,
                    island_id: 7,
                    owner: 2,
                    direction: 7,
                    animation_state: 3,
                    state_selector: 1,
                    state_descriptor: [0x38, 0, 16, 32],
                    state_flags: 1,
                    state_payload: [9; 8],
                },
                LandFigure {
                    raw_record: [0; SOLDAT3_RECORD_BYTES],
                    x: 0,
                    y: 0,
                    source_energy: 0,
                    figure_definition_id: 0,
                    runtime_slot: 15,
                    origin_descriptor: [0; 4],
                    route_radius: 0,
                    figure_kind: 12,
                    island_id: 3,
                    owner: 1,
                    direction: 0,
                    animation_state: 0,
                    state_selector: 0,
                    state_descriptor: [0; 4],
                    state_flags: 0,
                    state_payload: [0; 8],
                },
            ],
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        let occupants = source_kind4_occupants_from_scenario(&scenario);
        assert_eq!(
            occupants,
            vec![SourceKind4Occupant {
                runtime_slot: 14,
                figure_definition_id: 5,
                route_radius: 7,
                route_retry_count: 0,
                route_program: crate::combat::default_source_kind4_route_program(),
                route_program_cursor: 0,
                idle_remaining_bits: 0,
                origin_descriptor: SourceTargetDescriptor::from_bytes([0x33, 7, 0, 0]),
                position: (0, 0),
                island_id: 7,
                owner: 2,
                direction: 7,
                animation_state: 3,
                state_selector: 1,
                state_descriptor: SourceTargetDescriptor::from_bytes([0x38, 0, 16, 32]),
                idle_timestamp_ticks: 0,
                state_flags: 1,
                state_payload: [9; 8],
                active: true,
            }]
        );
        let definition = occupants[0].definition().expect("cavalry definition");
        assert_eq!(definition.family, LandFigureFamily::Cavalry);
        assert_eq!(definition.variant, 1);
        assert_eq!(definition.source_figure_name(), "KAVALERIE1");

        let units = land_units_from_scenario(&scenario);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].unit_type, crate::combat::UnitType::Cavalry);
        assert_eq!(units[0].owner, 2);
        assert_eq!((units[0].tile_x, units[0].tile_y), (0, 0));
        assert_eq!(units[0].direction, 7);
        assert_eq!(units[0].source_island_id, Some(7));
        assert_eq!(units[0].source_runtime_slot, Some(14));
        assert_eq!(units[0].source_live_runtime_slot, Some(14));
        assert_eq!(units[0].source_candidate_list_key, Some(7));
        assert_eq!(units[0].source_figure_kind, Some(4));
        assert_eq!(units[0].source_figure_definition_id, Some(5));
        assert_eq!(units[0].source_energy, 18);
        assert_eq!(units[0].source_route_radius, 7);
        assert_eq!(
            units[0]
                .source_origin_descriptor
                .map(SourceTargetDescriptor::bytes),
            Some([0x33, 7, 0, 0])
        );
        assert_eq!(
            units[0]
                .source_target_descriptor
                .map(SourceTargetDescriptor::bytes),
            Some([0x38, 0, 16, 32])
        );
        assert_eq!((units[0].target_x, units[0].target_y), (16, 32));
    }

    #[test]
    fn scenario_hq_tiles_reconstruct_one_oriented_dynamic_map_object() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let source_id = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![CodBuilding {
                source_id,
                kind: "HQ".into(),
                size: (2, 4),
                ..Default::default()
            }],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 4,
                width: 16,
                height: 16,
                x_pos: 100,
                y_pos: 200,
                fertilities: [7; 8],
                tiles: vec![
                    IslandTile {
                        building_id: 3,
                        x: 2,
                        y: 3,
                        orientation: 1,
                        anim_count: 0,
                        flags: 6 << 6,
                    },
                    // `FUN_0046ae20` has already overlaid slot 0 at this
                    // cell of the rotated 4 x 2 footprint, so the second
                    // HQ record must not allocate another object.
                    IslandTile {
                        building_id: 3,
                        x: 3,
                        y: 3,
                        orientation: 1,
                        anim_count: 0,
                        flags: 0,
                    },
                ],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        assert_eq!(
            source_dynamic_map_objects_from_scenario(&szs, &cod),
            vec![SourceDynamicMapObject {
                island: 4,
                slot: 0,
                owner: 6,
                local_position: (2, 3),
            }]
        );
    }

    #[test]
    fn source_cell_seeder_replays_oriented_command_overwrites() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let base = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                CodBuilding {
                    source_id: base + 1,
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "HANDWERK".into())].into(),
                    size: (1, 1),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 2,
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "MARKT".into())].into(),
                    size: (1, 1),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 3,
                    kind: "GEBAEUDE".into(),
                    // Rotating this 1 x 2 command makes it cover (3, 4),
                    // the earlier market root, while creating no cell state.
                    size: (1, 2),
                    ..Default::default()
                },
            ],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 6,
                width: 16,
                height: 16,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![
                    IslandTile {
                        building_id: 1,
                        x: 1,
                        y: 1,
                        orientation: 0,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 2,
                        x: 3,
                        y: 4,
                        orientation: 0,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 3,
                        x: 2,
                        y: 4,
                        orientation: 1,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 1,
                        x: 8,
                        y: 9,
                        orientation: 0,
                        anim_count: 0,
                        flags: 0,
                    },
                ],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        let states = source_map_cell_states_from_scenario(&szs, &cod);
        assert_eq!(states.len(), 2);
        assert!(states.iter().any(|state| state.matches(6, 1, 1)));
        assert!(states.iter().any(|state| state.matches(6, 8, 9)));
        assert!(!states.iter().any(|state| state.matches(6, 3, 4)));

        let static_cells = source_static_map_roots_from_scenario(&szs, &cod);
        assert_eq!(static_cells.len(), 4);
        assert!(static_cells
            .iter()
            .any(|state| { state.matches(6, 2, 4) && state.kind_code == 14 }));
        assert!(static_cells
            .iter()
            .any(|state| { state.matches(6, 3, 4) && state.kind_code == 14 }));
    }

    #[test]
    fn source_cell_seeder_retains_type_twelve_plantation_roots() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let base = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![CodBuilding {
                source_id: base + 1,
                kind: "GEBAEUDE".into(),
                size: (2, 3),
                properties: [
                    ("ProdKind".into(), "PLANTAGE".into()),
                    ("Rohstoff".into(), "GETREIDE".into()),
                    ("Figurnr".into(), "MAEHER".into()),
                ]
                .into(),
                ..Default::default()
            }],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 6,
                width: 16,
                height: 16,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![IslandTile {
                    building_id: 1,
                    x: 3,
                    y: 4,
                    orientation: 1,
                    anim_count: 0,
                    flags: 0,
                }],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        let states = source_map_cell_states_from_scenario(&szs, &cod);
        assert_eq!(states.len(), 1);
        assert!(states[0].matches(6, 3, 4));
        assert_eq!(
            (
                states[0].source_command_anchor_x,
                states[0].source_command_anchor_y
            ),
            (3, 4)
        );
        assert_eq!(
            (states[0].footprint_width, states[0].footprint_height),
            (3, 2)
        );
        assert_eq!(states[0].source_orientation, 1);
        assert!(states[0].is_type12_plantation_root());
    }

    #[test]
    fn source_backing_cells_follow_loader_filter_and_command_overwrites() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let base = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                CodBuilding {
                    source_id: base + 1,
                    kind: "BODEN".into(),
                    gfx: 1,
                    size: (2, 1),
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "UNUSED".into(),
                    )]),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 2,
                    kind: "GEBAEUDE".into(),
                    gfx: 2,
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "HANDWERK".into(),
                    )]),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 3,
                    kind: "FLUSS".into(),
                    gfx: 3,
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "UNUSED".into(),
                    )]),
                    ..Default::default()
                },
            ],
        };
        let owner_seven = |building_id, x, y| IslandTile {
            building_id,
            x,
            y,
            orientation: 0,
            anim_count: 0xc0,
            flags: 1,
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 6,
                width: 16,
                height: 16,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![
                    owner_seven(1, 2, 3),
                    owner_seven(2, 2, 3),
                    owner_seven(3, 3, 3),
                ],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        let backing = source_static_map_backing_cells_from_scenario(&szs, &cod);

        assert_eq!(backing.len(), 2);
        let left = backing
            .iter()
            .find(|cell| cell.matches(6, 2, 3))
            .expect("retained Boden backing cell");
        assert_eq!(left.kind_code, 11);
        assert_eq!(left.source_definition_offset, 1);
        assert_eq!(
            (left.source_command_anchor_x, left.source_command_anchor_y),
            (2, 3)
        );
        let right = backing
            .iter()
            .find(|cell| cell.matches(6, 3, 3))
            .expect("later Fluss overwrite");
        assert_eq!(right.kind_code, 16);
        assert_eq!(right.source_definition_offset, 3);
        assert_eq!(
            (right.source_command_anchor_x, right.source_command_anchor_y),
            (3, 3)
        );
    }

    #[test]
    fn source_backing_loader_filter_separates_outer_and_production_kind_gates() {
        use anno_formats::szs::IslandTile;

        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                CodBuilding {
                    kind: "BODEN".into(),
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "UNUSED".into(),
                    )]),
                    ..Default::default()
                },
                CodBuilding {
                    kind: "STRASSE".into(),
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "UNUSED".into(),
                    )]),
                    ..Default::default()
                },
                CodBuilding {
                    kind: "PIER".into(),
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "UNUSED".into(),
                    )]),
                    ..Default::default()
                },
                CodBuilding {
                    kind: "RUINE".into(),
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "UNUSED".into(),
                    )]),
                    ..Default::default()
                },
                CodBuilding {
                    kind: "STRANDRUINE".into(),
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "UNUSED".into(),
                    )]),
                    ..Default::default()
                },
                CodBuilding {
                    kind: "BODEN".into(),
                    properties: std::collections::HashMap::from([(
                        "ProdKind".into(),
                        "HANDWERK".into(),
                    )]),
                    ..Default::default()
                },
            ],
        };
        let tile = IslandTile {
            anim_count: 0xc0,
            flags: 1,
            ..Default::default()
        };

        assert!(source_loader_copies_static_backing(
            &cod,
            tile,
            &cod.buildings[0]
        ));
        assert!(!source_loader_copies_static_backing(
            &cod,
            tile,
            &cod.buildings[1]
        ));
        assert!(!source_loader_copies_static_backing(
            &cod,
            tile,
            &cod.buildings[2]
        ));
        assert!(!source_loader_copies_static_backing(
            &cod,
            tile,
            &cod.buildings[3]
        ));
        assert!(!source_loader_copies_static_backing(
            &cod,
            tile,
            &cod.buildings[4]
        ));
        assert!(!source_loader_copies_static_backing(
            &cod,
            tile,
            &cod.buildings[5]
        ));
    }

    #[test]
    fn source_cell_seeder_retains_terminal_fallback_cell_selectors() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let base = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                CodBuilding {
                    source_id: base + 1,
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "HANDWERK".into())].into(),
                    size: (2, 2),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 2,
                    kind: "STRAND".into(),
                    size: (1, 1),
                    ..Default::default()
                },
            ],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 6,
                width: 16,
                height: 16,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![
                    IslandTile {
                        building_id: 1,
                        x: 1,
                        y: 1,
                        orientation: 7 << 2,
                        anim_count: 2 << 6,
                        flags: 0,
                    },
                    // This overwrites source fallback cell zero: (2, 1).
                    IslandTile {
                        building_id: 2,
                        x: 2,
                        y: 1,
                        orientation: 0,
                        anim_count: 0,
                        flags: 0,
                    },
                ],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        let states = source_map_cell_states_from_scenario(&szs, &cod);
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].source_variant, 7);
        assert_eq!(states[0].source_map_owner_slot, 2);
        assert_eq!(states[0].fallback_strand_cells, 1);
        assert!(states[0].fallback_uses_strand_table(0));
        assert!(!states[0].fallback_uses_strand_table(1));
    }

    #[test]
    fn kind13_location_extractor_replays_footprints_and_orientation() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let base = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                CodBuilding {
                    source_id: base + 1,
                    kind: "WOHNUNG".into(),
                    size: (1, 2),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 2,
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "HANDWERK".into())].into(),
                    size: (1, 1),
                    ..Default::default()
                },
                CodBuilding {
                    source_id: base + 3,
                    kind: "PLATZ".into(),
                    size: (1, 1),
                    properties: std::collections::HashMap::from([("BGruppe".into(), "4".into())]),
                    ..Default::default()
                },
            ],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 6,
                width: 16,
                height: 16,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![
                    IslandTile {
                        building_id: 1,
                        x: 3,
                        y: 4,
                        orientation: 1,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 2,
                        x: 3,
                        y: 4,
                        orientation: 0,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 3,
                        x: 8,
                        y: 9,
                        orientation: 5,
                        anim_count: 0,
                        flags: 0,
                    },
                ],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        assert_eq!(
            source_kind13_locations_from_scenario(&szs, &cod).active_locations(),
            vec![SourceKind13Location {
                island_id: 6,
                tile_x: 8,
                tile_y: 9,
                orientation: 1,
                variant: 1,
                source_owner: 0,
                phase: 0,
                state_bits: 0,
                population_group: 4,
                amount: 0x40,
                lifecycle_flags: 0,
            }]
        );
    }

    #[test]
    fn kind13_promotion_definitions_preserve_cost_scale_and_variant_order() {
        let base = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE;
        let housing = |source_id, rand_anz, properties| CodBuilding {
            source_id,
            rand_anz,
            rand_add: 1,
            properties,
            ..Default::default()
        };
        let group_one = std::collections::HashMap::from([
            ("ProdKind".into(), "WOHNUNG".into()),
            ("BGruppe".into(), "1".into()),
            ("Werkzeug".into(), "3".into()),
            ("Holz".into(), "4".into()),
            ("Ziegel".into(), "5".into()),
            ("Kanon".into(), "6".into()),
            ("Money".into(), "7".into()),
        ]);
        let variant = std::collections::HashMap::from([
            ("ProdKind".into(), "WOHNUNG".into()),
            ("BGruppe".into(), "1".into()),
        ]);
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                housing(base + 11, 3, group_one),
                housing(base + 12, 0, variant.clone()),
                housing(base + 13, 0, variant),
            ],
        };

        let definitions = source_kind13_promotion_definitions(&cod);
        assert_eq!(
            definitions[1],
            Some(SourceKind13PromotionDefinition {
                target_group: 1,
                source_size: (1, 1),
                tools_cost_fixed: 96,
                wood_cost_fixed: 128,
                bricks_cost_fixed: 160,
                cannons_cost_fixed: 192,
                money_cost: 7,
                variant_definition_offsets: vec![11, 12, 13],
            })
        );
        assert_eq!(
            definitions[1]
                .as_ref()
                .and_then(|definition| definition.variant_definition_offset(4)),
            Some(12)
        );
        assert_eq!(
            definitions[1].as_ref().and_then(|definition| {
                definition.source_promotion_command(
                    SourceKind13Location {
                        island_id: 4,
                        tile_x: 9,
                        tile_y: 7,
                        orientation: 0,
                        variant: 0,
                        source_owner: 5,
                        phase: 0,
                        state_bits: 0,
                        population_group: 1,
                        amount: 0x40,
                        lifecycle_flags: 0,
                    },
                    3,
                    4,
                    0x1234,
                    2,
                )
            }),
            Some(SourceBuildingCommand {
                definition_offset: 12,
                orientation: 3,
                variant: 0,
                metadata: 4,
                map_owner_slot: 5,
                random_seed: 0x14,
                dynamic_object_owner: 2,
            })
        );
    }

    #[test]
    fn kind13_location_table_uses_source_hash_probe_and_city_slice() {
        let mut table = SourceKind13LocationTable::default();
        let first = SourceKind13Location {
            island_id: 2,
            tile_x: 8,
            tile_y: 9,
            orientation: 0,
            variant: 0,
            source_owner: 1,
            phase: 0,
            state_bits: 0,
            population_group: 0,
            amount: 0x40,
            lifecycle_flags: 0,
        };
        let colliding = SourceKind13Location {
            tile_x: 9,
            tile_y: 8,
            ..first
        };
        assert_eq!(
            SourceKind13LocationTable::source_index(2, 8, 9),
            SourceKind13LocationTable::source_index(2, 9, 8)
        );
        assert!(table.insert(first));
        assert!(table.insert(colliding));
        let start = SourceKind13LocationTable::source_index(2, 8, 9);
        assert_eq!(table.city_slice(2)[start - 2 * 0x400], Some(first));
        assert_eq!(table.city_slice(2)[start - 2 * 0x400 + 1], Some(colliding));
        assert_eq!(table.location_at(2, 9, 8), Some(colliding));
        table.location_at_mut(2, 9, 8).unwrap().amount = 0x123;
        assert_eq!(table.location_at(2, 9, 8).unwrap().amount, 0x123);
        table.remove_roots_in_footprint(2, 8, 9, 1, 1);
        assert_eq!(
            table.active_locations(),
            vec![SourceKind13Location {
                amount: 0x123,
                ..colliding
            }]
        );
    }

    #[test]
    fn kind13_decrease_replays_ordered_neighbor_redistribution_and_downgrade_handoff() {
        let origin = SourceKind13Location {
            island_id: 2,
            tile_x: 8,
            tile_y: 9,
            source_owner: 1,
            state_bits: 0x40,
            population_group: 1,
            amount: 200,
            ..SourceKind13Location {
                island_id: 0,
                tile_x: 0,
                tile_y: 0,
                orientation: 0,
                variant: 0,
                source_owner: 0,
                phase: 0,
                state_bits: 0,
                population_group: 0,
                amount: 0x40,
                lifecycle_flags: 0,
            }
        };
        let neighbor_one = SourceKind13Location {
            tile_x: 9,
            tile_y: 9,
            amount: 320,
            ..origin
        };
        let neighbor_two = SourceKind13Location {
            tile_x: 10,
            tile_y: 9,
            amount: 0,
            ..origin
        };
        let different_group = SourceKind13Location {
            tile_x: 11,
            tile_y: 9,
            population_group: 2,
            ..origin
        };
        let mut table = SourceKind13LocationTable::default();
        assert!(table.insert(origin));
        assert!(table.insert(neighbor_one));
        assert!(table.insert(neighbor_two));
        assert!(table.insert(different_group));
        let mut city = SourceCityRecord {
            tier_population: [0, 8, 0, 0, 0],
            satisfaction_by_group: [0, 0x58, 0, 0, 0],
            ..Default::default()
        };

        assert_eq!(
            table.apply_source_kind13_decrease(
                &mut city,
                2,
                8,
                9,
                100,
                &[(9, 9), (10, 9), (11, 9), (8, 9)],
            ),
            Some(SourceKind13DecreaseResult::Applied {
                remaining_amount: 0,
                redistributed_amount: 100,
            })
        );
        assert_eq!(city.tier_population, [0, 6, 0, 0, 0]);
        assert_eq!(table.location_at(2, 8, 9).unwrap().amount, 0);
        assert_eq!(table.location_at(2, 9, 9).unwrap().amount, 384);
        assert_eq!(table.location_at(2, 10, 9).unwrap().amount, 36);
        assert_eq!(table.location_at(2, 11, 9).unwrap().amount, 200);

        let mut downgrade_table = SourceKind13LocationTable::default();
        let downgrade_origin = SourceKind13Location {
            state_bits: 0,
            amount: 90,
            ..origin
        };
        assert!(downgrade_table.insert(downgrade_origin));
        let mut downgrade_city = SourceCityRecord {
            tier_population: [0, 1, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            downgrade_table.apply_source_kind13_decrease(&mut downgrade_city, 2, 8, 9, 20, &[],),
            Some(SourceKind13DecreaseResult::DowngradeRequired {
                target_group: 0,
                remaining_amount: 70,
            })
        );
        assert_eq!(downgrade_city.tier_population, [1, 0, 0, 0, 0]);
        assert_eq!(
            downgrade_table.location_at(2, 8, 9),
            Some(SourceKind13Location {
                population_group: 0,
                amount: 70,
                ..downgrade_origin
            })
        );
    }

    #[test]
    fn kind13_transition_predicate_replays_every_bgruppe_lifecycle_mask() {
        let base = SourceKind13Location {
            island_id: 0,
            tile_x: 0,
            tile_y: 0,
            orientation: 0,
            variant: 0,
            source_owner: 0,
            phase: 0,
            state_bits: 0,
            population_group: 0,
            amount: 0x40,
            lifecycle_flags: 0,
        };
        assert!(SourceKind13Location {
            lifecycle_flags: 0x0400,
            ..base
        }
        .source_transition_active_for_group(0));
        assert!(SourceKind13Location {
            state_bits: 0x80,
            lifecycle_flags: 0x000c,
            ..base
        }
        .source_transition_active_for_group(1));
        assert!(SourceKind13Location {
            state_bits: 0x80,
            lifecycle_flags: 0x011c,
            ..base
        }
        .source_transition_active_for_group(2));
        assert!(SourceKind13Location {
            state_bits: 0x80,
            lifecycle_flags: 0x0158,
            ..base
        }
        .source_transition_active_for_group(3));
        assert!(SourceKind13Location {
            state_bits: 0x80,
            lifecycle_flags: 0x01d8,
            ..base
        }
        .source_transition_active_for_group(4));
        assert!(!SourceKind13Location {
            state_bits: 0x80,
            lifecycle_flags: 0x0158,
            ..base
        }
        .source_transition_active_for_group(4));
    }

    #[test]
    fn kind13_increase_replays_reservation_and_matured_promotion() {
        let origin = SourceKind13Location {
            island_id: 2,
            tile_x: 8,
            tile_y: 9,
            orientation: 1,
            variant: 0,
            source_owner: 3,
            phase: 2,
            state_bits: 0x80,
            population_group: 0,
            amount: 100,
            lifecycle_flags: 0,
        };
        let mut reservation_table = SourceKind13LocationTable::default();
        assert!(reservation_table.insert(origin));
        let mut reservation_city = SourceCityRecord {
            phase: 5,
            tier_population: [1, 0, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            reservation_table.apply_source_kind13_increase(
                &mut reservation_city,
                2,
                8,
                9,
                100,
                &[],
                None,
            ),
            Some(SourceKind13IncreaseResult {
                remaining_amount: 0x80,
                redistributed_amount: 0,
                reservation_created: true,
                promotion: None,
            })
        );
        assert_eq!(reservation_city.tier_population, [2, 0, 0, 0, 0]);
        assert_eq!(reservation_city.promotion_reservations, [0, 3, 0, 0, 0]);
        assert_eq!(
            reservation_city.promotion_reservation_positions,
            [(0, 0), (8, 9), (0, 0), (0, 0), (0, 0)]
        );
        assert_eq!(
            reservation_table.location_at(2, 8, 9).unwrap().state_byte(),
            0xaa
        );

        let promotion_origin = SourceKind13Location {
            amount: 0x80,
            lifecycle_flags: 0x000c,
            ..origin
        };
        let target_neighbor = SourceKind13Location {
            tile_x: 9,
            population_group: 1,
            amount: 100,
            ..promotion_origin
        };
        let mut promotion_table = SourceKind13LocationTable::default();
        assert!(promotion_table.insert(promotion_origin));
        assert!(promotion_table.insert(target_neighbor));
        let mut promotion_city = SourceCityRecord {
            phase: 5,
            tier_population: [2, 1, 0, 0, 0],
            satisfaction_by_group: [0x80, 0x80, 0, 0, 0],
            overall_satisfaction: 0x80,
            promotion_reservations: [0, 5, 0, 0, 0],
            promotion_reservation_positions: [(0, 0), (8, 9), (0, 0), (0, 0), (0, 0)],
            ..Default::default()
        };
        let materials = SourceKind13PromotionMaterials {
            target_group: 1,
            tools_cost_fixed: 32,
            wood_cost_fixed: 64,
            bricks_cost_fixed: 96,
            available_tools_fixed: 32,
            available_wood_fixed: 64,
            available_bricks_fixed: 96,
        };
        assert_eq!(
            promotion_table.apply_source_kind13_increase(
                &mut promotion_city,
                2,
                8,
                9,
                192,
                &[(9, 9)],
                Some(materials),
            ),
            Some(SourceKind13IncreaseResult {
                remaining_amount: 160,
                redistributed_amount: 160,
                reservation_created: false,
                promotion: Some(SourceKind13Promotion {
                    island_id: 2,
                    tile_x: 8,
                    tile_y: 9,
                    target_group: 1,
                }),
            })
        );
        assert_eq!(promotion_city.tier_population, [0, 6, 0, 0, 0]);
        assert_eq!(promotion_city.promotion_reservations, [0; 5]);
        assert_eq!(
            promotion_city.promotion_reservation_positions,
            [(0, 0), (0xff, 0xff), (0, 0), (0, 0), (0, 0)]
        );
        assert_eq!(
            promotion_table.location_at(2, 8, 9),
            Some(SourceKind13Location {
                population_group: 1,
                amount: 160,
                ..promotion_origin
            })
        );
        assert_eq!(promotion_table.location_at(2, 9, 9).unwrap().amount, 260);
    }

    #[test]
    fn kind13_amount_capacities_match_shipped_bgruppe_maxwohn_rows() {
        assert_eq!(SOURCE_KIND13_MAX_RESIDENTS, [2, 6, 15, 25, 40]);
        assert_eq!(
            SOURCE_KIND13_AMOUNT_CAPACITIES,
            [0x80, 0x180, 0x3c0, 0x640, 0xa00]
        );

        let location = SourceKind13Location {
            island_id: 0,
            tile_x: 0,
            tile_y: 0,
            orientation: 0,
            variant: 0,
            source_owner: 0,
            phase: 0,
            state_bits: 0,
            population_group: 3,
            amount: 0x40,
            lifecycle_flags: 0,
        };
        assert_eq!(location.source_amount_capacity(), Some(0x640));
        assert_eq!(
            SourceKind13Location {
                population_group: 5,
                ..location
            }
            .source_amount_capacity(),
            None
        );
    }

    #[test]
    fn city_group_satisfaction_replays_bgruppe_selectors_and_denominator_curve() {
        assert_eq!(
            SOURCE_CITY_LUXURY_WARE_SLOTS,
            [0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15]
        );
        assert_eq!(SOURCE_CITY_GROUP_FULFILLMENT_TARGETS, [0, 60, 90, 99, 107]);
        assert_eq!(
            SOURCE_CITY_GROUP_LUXURY_REQUIREMENTS[4],
            [true, true, true, true, false, true, true]
        );
        assert_eq!(source_city_group_satisfaction_denominator(0, 128, 60), 60);
        assert_eq!(
            source_city_group_satisfaction_denominator(0x20, 128, 60),
            59
        );

        let mut city = SourceCityRecord {
            luxury_satisfaction: [0, 0, 0, 20, 40, 0, 0],
            overall_satisfaction: 91,
            growth_blocked: true,
            ..Default::default()
        };
        city.refresh_group_satisfaction();
        assert_eq!(city.satisfaction_by_group, [128, 128, 85, 77, 23]);
        assert_eq!(
            city.source_kind13_transfer_inputs(),
            SourceKind13TransferInputs {
                satisfaction_by_group: [128, 128, 85, 77, 23],
                overall_satisfaction: 91,
                growth_blocked: true,
            }
        );

        city.luxury_satisfaction = [17, 18, 18, 18, 0, 18, 18];
        city.refresh_group_satisfaction();
        assert_eq!(city.satisfaction_by_group[4], 128);
    }

    #[test]
    fn service_radius_rows_match_the_live_compiled_tables() {
        // Rows read out of the running original's `DAT_005b7460`
        // (2026-08-14). The generator is the `FUN_00404d70` integer
        // midpoint fill.
        assert_eq!(source_service_radius_row(0), [0]);
        assert_eq!(source_service_radius_row(1), [1, 1]);
        assert_eq!(source_service_radius_row(5), [5, 5, 5, 4, 3, 2]);
        assert_eq!(
            source_service_radius_row(8),
            [8, 8, 8, 7, 7, 6, 5, 4, 2]
        );
        assert_eq!(
            source_service_radius_row(10),
            [10, 10, 10, 10, 9, 9, 8, 7, 6, 5, 3]
        );
        assert_eq!(
            source_service_radius_row(16),
            [16, 16, 16, 16, 15, 15, 15, 14, 14, 13, 12, 12, 11, 9, 8, 6, 3]
        );
        assert_eq!(
            source_service_radius_row(24),
            [
                24, 24, 24, 24, 24, 23, 23, 23, 23, 22, 22, 21, 21, 20, 19, 19, 18, 17, 16,
                15, 13, 12, 10, 8, 4
            ]
        );
    }

    #[test]
    fn market_distance_classes_match_the_live_grid() {
        // `DAT_005a6af0` sampled live: trunc(sqrt(dx²+dy²)·0.375 + 0.5).
        let expected_row0 = [0, 0, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 5, 5, 5, 6, 6];
        for (dx, &expected) in expected_row0.iter().enumerate() {
            assert_eq!(
                source_market_distance_class(dx as u8, 0),
                expected,
                "class({dx}, 0)"
            );
        }
        assert_eq!(source_market_distance_class(4, 4), 2);
        assert_eq!(source_market_distance_class(16, 16), 8);
        assert_eq!(source_market_distance_class(12, 5), 5);
    }

    #[test]
    fn ware_economy_cycle_accumulates_pulls_and_decays_like_fun_0047f8a0() {
        // Pioneers only: NAHRUNG demand grows by rate 17 per resident and
        // decays 15/16; slot pulls are `(deficit >> 8) + 1` whole units.
        let mut city = SourceCityRecord {
            tier_population: [300, 0, 0, 0, 0],
            ..Default::default()
        };
        let mut pulls: Vec<(usize, u16)> = Vec::new();
        city.source_ware_economy_cycle(|slot, want| {
            pulls.push((slot, want));
            want
        });
        // First cycle: every accumulator starts at zero, so no slot pulls
        // and every fulfillment byte reports full (demand == 0 -> 0x80).
        assert!(pulls.is_empty());
        assert_eq!(city.food_fulfillment, 0x80);
        assert_eq!(city.luxury_satisfaction, [0x80; 7]);
        assert_eq!(city.overall_satisfaction, 0x80);
        assert_eq!(city.satisfaction_by_group, [0x80; 5]);
        // 300 residents accumulate 17*300 = 5100 food demand, decayed
        // 15/16 to 4781 (`:91422`, `:91430`).
        assert_eq!(city.ware_demand[0], 4781);
        assert_eq!(city.ware_demand[1..], [0; 7]);

        city.source_ware_economy_cycle(|slot, want| {
            pulls.push((slot, want));
            want
        });
        // Second cycle: deficit 4781 pulls (4781 >> 8) + 1 = 19 units,
        // crediting supply 19 << 8 = 4864 -> fulfillment clamps at 0x80.
        assert_eq!(pulls, [(0, 19)]);
        assert_eq!(city.food_fulfillment, 0x80);
        // History captured the pre-cycle byte.
        assert_eq!(city.ware_fulfillment_history[0], [0x80, 0, 0]);
        // demand (4781 + 5100) * 15 / 16 = 9263; supply 4864 * 15 / 16 = 4560.
        assert_eq!(city.ware_demand[0], 9263);
        assert_eq!(city.ware_supply[0], 4560);
    }

    #[test]
    fn ware_economy_cycle_starves_settlers_on_an_empty_store() {
        let mut city = SourceCityRecord {
            tier_population: [0, 100, 0, 0, 0],
            ..Default::default()
        };
        // Cycle one seeds settler demand: ALKOHOL 6*100, STOFFE 8*100,
        // NAHRUNG 17*100, each decayed 15/16.
        city.source_ware_economy_cycle(|_, _| 0);
        assert_eq!(city.ware_demand[0], 1593);
        assert_eq!(city.ware_demand[4], 562);
        assert_eq!(city.ware_demand[5], 750);
        // Cycle two: the empty store fails every pull, so the demanded
        // slots report zero fulfillment while undemanded slots stay full.
        city.source_ware_economy_cycle(|_, _| 0);
        assert_eq!(city.food_fulfillment, 0);
        assert_eq!(city.overall_satisfaction, 0);
        assert_eq!(city.luxury_satisfaction, [0x80, 0x80, 0x80, 0, 0, 0x80, 0x80]);
        // Settlers (both demanded slots empty) collapse to zero; higher
        // groups retain the undemanded slots' full bytes over their
        // scales: g2 (128+128)*128/360, g3 (3*128)*128/495,
        // g4 (5*128)*128/642. Pioneers have scale zero -> always full.
        assert_eq!(city.satisfaction_by_group, [128, 0, 91, 99, 127]);
    }

    #[test]
    fn ware_economy_cycle_resets_empty_group_tax_and_counts_reservations() {
        let mut city = SourceCityRecord {
            tier_population: [100, 0, 0, 0, 0],
            satisfaction_weights: [0x40; 5],
            promotion_reservations: [0, 20, 0, 0, 0],
            ..Default::default()
        };
        city.source_ware_economy_cycle(|_, _| 0);
        // `:91404`: only groups without residents reset their tax weight
        // to the 0x80 default; group zero keeps the player's setting.
        assert_eq!(city.satisfaction_weights, [0x40, 0x80, 0x80, 0x80, 0x80]);
        // `:91407-91410`: the 20 residents reserved to promote into group
        // one consume at settler weights already (ALKOHOL 6*20 -> 112
        // after decay, STOFFE 8*20 -> 150) while group zero's food
        // consumers drop to 80: total NAHRUNG (17*80 + 17*20) * 15 / 16.
        assert_eq!(city.ware_demand[4], 112);
        assert_eq!(city.ware_demand[5], 150);
        assert_eq!(city.ware_demand[0], 1593);
    }

    #[test]
    fn ware_economy_cycle_tracks_declining_worst_slot() {
        // A slot whose fresh ratio undercuts a two-cycle decline is
        // reported as `slot + 0x0e` in `worst_ware_slot` (`:91355-91376`).
        let mut city = SourceCityRecord {
            tier_population: [0, 100, 0, 0, 0],
            ..Default::default()
        };
        let mut supply_units = [0u16; 8];
        supply_units[4] = 400; // plentiful ALKOHOL at first
        supply_units[5] = 400; // plentiful STOFFE
        for _ in 0..3 {
            city.source_ware_economy_cycle(|slot, want| {
                let take = want.min(supply_units[slot]);
                supply_units[slot] -= take;
                take
            });
        }
        assert_eq!(city.worst_ware_slot, 0);
        // The store runs dry. NAHRUNG never had stock, so its byte
        // flatlines at zero without the strict three-sample decline the
        // tracker requires; STOFFE — stocked, then starved, and with the
        // larger demand weight than ALKOHOL — is the slot whose monotone
        // decline trips first, reported as ware index 0x13.
        let mut tripped = None;
        for _ in 0..6 {
            city.source_ware_economy_cycle(|_, _| 0);
            if city.worst_ware_slot != 0 {
                tripped = Some(city.worst_ware_slot);
                break;
            }
        }
        assert_eq!(tripped, Some(0x13));
    }

    #[test]
    fn city_resident_totals_match_fun_0047f790_and_owner_accumulation() {
        let city = SourceCityRecord {
            owner_slot: 2,
            tier_population: [10, 20, 30, 40, 50],
            resident_amount: 7,
            ..Default::default()
        };
        assert_eq!(city.source_resident_total(), 157);

        let mut cities = SourceCityTable::default();
        assert!(cities.set_record(0, Some(city)));
        assert!(cities.set_record(
            1,
            Some(SourceCityRecord {
                owner_slot: 2,
                resident_amount: u32::MAX,
                ..Default::default()
            })
        ));
        assert!(cities.set_record(
            2,
            Some(SourceCityRecord {
                owner_slot: 3,
                resident_amount: 90,
                ..Default::default()
            })
        ));

        assert_eq!(cities.source_resident_total_for_owner(2), 156);
        assert_eq!(cities.source_resident_total_for_owner(3), 90);
    }

    #[test]
    fn controller_populated_city_selection_uses_physical_last_match() {
        let mut cities = SourceCityTable::default();
        assert!(cities.set_record(
            1,
            Some(SourceCityRecord {
                owner_slot: 2,
                tier_population: [0, 99, 9, 0, 0],
                ..Default::default()
            })
        ));
        assert!(cities.set_record(
            3,
            Some(SourceCityRecord {
                owner_slot: 2,
                tier_population: [0, 0, 10, 0, 0],
                ..Default::default()
            })
        ));
        assert!(cities.set_record(
            4,
            Some(SourceCityRecord {
                owner_slot: 2,
                tier_population: [0, 0, 4, 6, 1],
                ..Default::default()
            })
        ));

        assert_eq!(cities.source_controller_populated_city_slot(2), Some(4));
        assert_eq!(cities.source_controller_populated_city_slot(3), None);
    }

    #[test]
    fn controller_city_selection_replaces_a_populated_city_with_later_higher_score() {
        let mut cities = SourceCityTable::default();
        assert!(cities.set_record(
            1,
            Some(SourceCityRecord {
                island_id: 4,
                source_owner: 2,
                owner_slot: 2,
                tier_population: [0, 0, 10, 0, 0],
                ..Default::default()
            })
        ));
        assert!(cities.set_record(
            3,
            Some(SourceCityRecord {
                island_id: 9,
                source_owner: 5,
                owner_slot: 2,
                ..Default::default()
            })
        ));

        assert_eq!(
            cities.source_controller_city_slot(2, |island, source_owner| {
                if (island, source_owner) == (9, 5) {
                    80
                } else {
                    20
                }
            }),
            Some(3)
        );
    }

    #[test]
    fn city_property_thirteen_starts_zero_and_replays_source_event_truncation() {
        let mut city = SourceCityRecord::default();
        assert_eq!(city.controller_figure_capacity_metric, 0);

        city.source_add_controller_figure_capacity_metric(0x1_0021);
        assert_eq!(city.controller_figure_capacity_metric, 0x21);

        city.source_sub_controller_figure_capacity_metric(0x22);
        assert_eq!(city.controller_figure_capacity_metric, u16::MAX);
    }

    #[test]
    fn kind13_state_byte_preserves_lifecycle_bits_while_advancing_phase() {
        let mut location = SourceKind13Location {
            island_id: 1,
            tile_x: 2,
            tile_y: 3,
            orientation: 0,
            variant: 0,
            source_owner: 4,
            phase: 0,
            state_bits: 0xa0,
            population_group: 2,
            amount: 0x40,
            lifecycle_flags: 0,
        };

        location.set_phase(11);
        assert_eq!(location.phase, 3);
        assert_eq!(location.state_byte(), 0xa3);
    }

    #[test]
    fn kind13_transfer_delta_replays_source_growth_and_decay_branches() {
        let mut location = SourceKind13Location {
            island_id: 1,
            tile_x: 2,
            tile_y: 3,
            orientation: 0,
            variant: 0,
            source_owner: 4,
            phase: 0,
            state_bits: 0,
            population_group: 0,
            amount: 0x40,
            lifecycle_flags: 0,
        };
        let full_satisfaction = SourceKind13TransferInputs {
            satisfaction_by_group: [128, 0, 0, 0, 0],
            overall_satisfaction: 128,
            growth_blocked: false,
        };

        location.state_bits = 0xc0;
        assert_eq!(location.source_transfer_delta(full_satisfaction), 160);
        assert_eq!(
            location.source_transfer_delta(SourceKind13TransferInputs {
                growth_blocked: true,
                ..full_satisfaction
            }),
            0
        );

        location.state_bits = 0;
        assert_eq!(
            location.source_transfer_delta(SourceKind13TransferInputs {
                satisfaction_by_group: [0; 5],
                overall_satisfaction: 0,
                growth_blocked: false,
            }),
            -303
        );
        location.state_bits = 0x40;
        location.lifecycle_flags = 2;
        assert_eq!(
            location.source_transfer_delta(SourceKind13TransferInputs {
                satisfaction_by_group: [0; 5],
                overall_satisfaction: 0,
                growth_blocked: false,
            }),
            -431
        );
    }

    #[test]
    fn kind13_dispatch_uses_staggered_clocks_and_physical_record_order() {
        let mut table = SourceKind13LocationTable::default();
        assert!(table.insert(SourceKind13Location {
            island_id: 0,
            tile_x: 0,
            tile_y: 0,
            orientation: 0,
            variant: 1,
            source_owner: 0,
            phase: 0,
            state_bits: 0xa0,
            population_group: 0,
            amount: 0x40,
            lifecycle_flags: 0,
        }));
        let mut dispatch = SourceKind13DispatchState::default();

        assert_eq!(dispatch.advance(&mut table, 15_063), 0);
        assert_eq!(table.active_locations()[0].state_byte(), 0xa0);

        assert_eq!(dispatch.advance(&mut table, 1), 1);
        assert_eq!(table.active_locations()[0].state_byte(), 0xa1);
    }

    #[test]
    fn production_loader_resolves_inselhaus_source_ids_not_gfx() {
        use anno_formats::szs::{Island, IslandTile, ScenarioMeta};

        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![CodBuilding {
                source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
                gfx: 9000,
                kind: "GEBAEUDE".into(),
                properties: HashMap::from([
                    ("ProdKind".into(), "HANDWERK".into()),
                    ("Ware".into(), "HOLZ".into()),
                ]),
                ..Default::default()
            }],
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 2,
                width: 1,
                height: 1,
                x_pos: 0,
                y_pos: 0,
                fertilities: [7; 8],
                tiles: vec![IslandTile {
                    building_id: 3,
                    x: 0,
                    y: 0,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                }],
                city: None,
            }],
            players: Vec::new(),
            mission: None,
            scenario: ScenarioMeta::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        let defs = load_building_defs(&cod);
        let instances = load_building_instances(&szs, &cod, &defs);
        assert_eq!(instances.len(), 1);
        assert_eq!(
            instances[0].source_placement_command,
            Some(crate::building::SourceBuildingCommand::from_island_tile(
                szs.islands[0].tiles[0]
            ))
        );
    }

    #[test]
    fn load_building_instances_picks_owner_from_stadt4() {
        // New Horizons2 has cities owned by multiple slots
        // (player on island 0, AI rivals on later islands,
        // pirates on island 21). Building instances on those
        // islands should inherit the city's owner_slot.
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let cod_data = match std::fs::read(base.join("extracted/haeuser.cod")) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: haeuser.cod not found");
                return;
            }
        };
        let szs_data = match std::fs::read(base.join("extracted/Szenes/New Horizons2.szs")) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: New Horizons2.szs not found");
                return;
            }
        };
        let cod = CodFile::parse(&cod_data).unwrap();
        let defs = load_building_defs(&cod);
        let szs = SzsFile::parse(&szs_data).unwrap();
        let instances = load_building_instances(&szs, &cod, &defs);

        // Map island_number → expected owner_slot from STADT4.
        let mut expected_owner: std::collections::HashMap<u8, u8> =
            std::collections::HashMap::new();
        for island in &szs.islands {
            if let Some(city) = island.city.as_ref() {
                expected_owner.insert(island.number, city.owner_slot);
            }
        }
        // Every instance's owner should match its island's
        // STADT4 owner_slot.
        for inst in &instances {
            if let Some(want) = expected_owner.get(&inst.island_id) {
                assert_eq!(
                    inst.owner, *want,
                    "building on island {} should be owned by slot {}, got {}",
                    inst.island_id, want, inst.owner
                );
            }
        }
        // Cross-slot diversity: at least 2 distinct owners
        // across the building set, otherwise the wiring is
        // probably broken (everything would be slot 0).
        let owners: std::collections::HashSet<u8> = instances.iter().map(|b| b.owner).collect();
        assert!(
            owners.len() >= 2,
            "expected ≥2 distinct owners across New Horizons2's buildings, got {owners:?}"
        );
    }

    #[test]
    fn shipping_scenarios_expose_authored_kontors_by_source_id() {
        // INSELHAUS records are source-definition offsets, not GFX values.
        // The source-ID lookup exposes authored Kontors that the former GFX
        // lookup missed, including the native and pirate settlements in the
        // shipping corpus.
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let cod_data = match std::fs::read(base.join("extracted/haeuser.cod")) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: haeuser.cod not found");
                return;
            }
        };
        let cod = CodFile::parse(&cod_data).unwrap();
        let defs = load_building_defs(&cod);
        let szenes = base.join("extracted/Szenes");
        if !szenes.exists() {
            println!("Skipping: scenes dir not found");
            return;
        }
        let mut scenarios = 0;
        let mut kontors = 0;
        for entry in std::fs::read_dir(&szenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let szs = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let whs = kontor_warehouses_from_szs(&szs, &cod, &defs);
            for warehouse in &whs {
                assert!(
                    szs.islands
                        .iter()
                        .any(|island| island.number == warehouse.island_id),
                    "{:?} yielded a Kontor on an unknown island {}",
                    path.file_stem().unwrap(),
                    warehouse.island_id
                );
            }
            kontors += whs.len();
            scenarios += 1;
        }
        assert!(scenarios > 0, "audit must cover at least one scenario");
        assert!(kontors > 0, "source-ID audit must recover authored Kontors");
    }

    #[test]
    fn native_pirate_kontor_constants_match_haeuser_cod() {
        // Pin the canonical Kontor Nummers for native + pirate
        // settlements against haeuser.cod.
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let cod_data = match std::fs::read(base.join("extracted/haeuser.cod")) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: haeuser.cod not found");
                return;
            }
        };
        let cod = CodFile::parse(&cod_data).unwrap();
        for &nr in &[NATIVE_KONTOR_A, NATIVE_KONTOR_B, PIRATE_KONTOR] {
            let b = cod
                .buildings
                .iter()
                .find(|b| b.nummer == nr)
                .unwrap_or_else(|| panic!("Nr={nr} not in haeuser.cod"));
            assert_eq!(b.kind, "HQ", "Nr={nr} should be Kind=HQ");
            assert_eq!(
                b.properties.get("ProdKind").map(|s| s.as_str()),
                Some("KONTOR"),
                "Nr={nr} should be ProdKind=KONTOR"
            );
        }
        // Pirate Kontor must carry both flags.
        let pirat = cod
            .buildings
            .iter()
            .find(|b| b.nummer == PIRATE_KONTOR)
            .unwrap();
        assert_eq!(
            pirat.properties.get("Piratflg").map(|s| s.as_str()),
            Some("1")
        );
    }

    #[test]
    fn diplomacy_from_player4_seeds_both_directed_source_diplomacy_tables() {
        use crate::combat::Diplomacy;
        use anno_formats::szs::PlayerSlotInit;
        let mk = |relations_0xc0: [u32; 7], relationships: [u32; 7]| PlayerSlotInit {
            relations_0xc0,
            relationships,
            ..Default::default()
        };
        let players = vec![
            mk([0, 1, 2, 3, 3, 3, 3], [3, 0, 1, 2, 3, 3, 3]),
            mk([0, 0, 0, 3, 3, 3, 3], [0, 3, 0, 2, 3, 3, 3]),
            mk([0, 0, 0, 3, 3, 3, 3], [1, 0, 3, 2, 3, 3, 3]),
            mk([3, 3, 3, 0, 0, 0, 0], [2, 2, 3, 0, 0, 0, 0]),
        ];
        let dm = diplomacy_from_player4_relationships(&players);
        assert_eq!(dm.get(0, 1), Diplomacy::Neutral);
        assert_eq!(dm.get(1, 2), Diplomacy::Neutral);
        assert_eq!(dm.get(0, 3), Diplomacy::Allied);
        assert_eq!(dm.get(3, 2), Diplomacy::Allied);
        assert_eq!(dm.source_relationship_code(0, 1), 1);
        assert_eq!(dm.source_relationship_code(0, 2), 2);
        assert_eq!(dm.source_relationship_code(0, 0), 0);
        assert_eq!(dm.source_relationship_code(1, 2), 0);
        assert_eq!(dm.source_relationship_code(0, 3), 3);
        assert_eq!(dm.source_relationship_code(3, 2), 3);
        assert_eq!(dm.source_attitude_code(0, 1), 0);
        assert_eq!(dm.source_attitude_code(0, 2), 1);
        assert_eq!(dm.source_attitude_code(0, 0), 3);
        assert_eq!(dm.source_attitude_code(0, 3), 2);
        assert_eq!(dm.source_attitude_code(3, 2), 3);
        assert_eq!(code_to_diplomacy(1), Diplomacy::Neutral);
        assert_eq!(code_to_diplomacy(2), Diplomacy::Neutral);
    }

    #[test]
    fn source_kind_four_dispatch_state_preserves_raw_player4_factions() {
        use anno_formats::szs::PlayerSlotInit;

        let mut human = PlayerSlotInit::default();
        human.state_byte = 0;
        let mut ai = PlayerSlotInit::default();
        ai.state_byte = 0x0c;
        let mut native = PlayerSlotInit::default();
        native.state_byte = 0x0e;
        let mut inactive = PlayerSlotInit::default();
        inactive.state_byte = 0xff;
        let scenario = SzsFile {
            chunks: Vec::new(),
            islands: Vec::new(),
            players: vec![
                human,
                ai,
                inactive.clone(),
                inactive.clone(),
                inactive.clone(),
                native,
            ],
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };

        let dispatch = source_kind4_dispatch_state_from_scenario(&scenario);
        assert_eq!(dispatch.active_player_slot, 0);
        assert!(dispatch.single_player);
        assert_eq!(
            dispatch.faction_states,
            [0, 0x0c, 0xff, 0xff, 0xff, 0x0e, 0xff]
        );
    }

    #[test]
    fn island_can_host_building_gates_fertility_bound_plantations() {
        use crate::types::ProductionType;
        use anno_formats::szs::{Fertility, Island};
        let mk_island = |ferts: [u8; 8]| Island {
            number: 0,
            width: 10,
            height: 10,
            x_pos: 0,
            y_pos: 0,
            fertilities: ferts,
            tiles: Vec::new(),
            city: None,
        };
        let mk_def = |req: Option<Fertility>| {
            let mut d = BuildingDef {
                id: 0,
                category: 0,
                width: 1,
                height: 1,
                production_type: ProductionType::Craft,
                kind: "PLANTAGE".into(),
                prod_kind: "PLANTAGE".into(),
                radius: 0,
                output_good: Good::None,
                input_good_1: Good::None,
                input_good_2: Good::None,
                output_rate: 1,
                input_1_rate: 0,
                input_2_rate: 0,
                storage_capacity: 0,
                cycle_time_ms: 1000,
                cost_gold: 0,
                cost_tools: 0,
                cost_wood: 0,
                cost_bricks: 0,
                maintenance_cost: 0,
                native: false,
                bauinfra: 0,
                max_no_input_ticks: 6,
                can_dry_up: false,
                wegspeed: [100; 4],
                has_door: false,
                upgradeable: false,
                max_energy: 0,
                ore_deposit: crate::building::OreDeposit::None,
                pirate_owned: false,
                defensive_cannons: 0,
                max_brand_damage_ticks: crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
                ruin_id: crate::building::NO_RUIN_ID,
                required_fertility: req,
            };
            d.required_fertility = req;
            d
        };

        // Universal building (no fertility requirement) passes
        // even on a barren island.
        let universal = mk_def(None);
        let barren = mk_island([7; 8]);
        assert!(island_can_host_building(&universal, &barren));

        // Tobacco plantation requires byte 1 in the map.
        let tobacco = mk_def(Some(Fertility::Tobacco));
        assert!(
            !island_can_host_building(&tobacco, &barren),
            "barren island should reject tobacco"
        );
        let tobacco_isle = mk_island([1, 7, 7, 7, 7, 7, 7, 7]);
        assert!(
            island_can_host_building(&tobacco, &tobacco_isle),
            "byte=1 island should accept tobacco"
        );

        // Multi-fertility island accepts every matching crop.
        let multi = mk_island([3, 6, 7, 7, 7, 7, 7, 7]);
        let sugarcane = mk_def(Some(Fertility::Sugarcane));
        let cocoa = mk_def(Some(Fertility::Cocoa));
        let cotton = mk_def(Some(Fertility::Cotton));
        assert!(island_can_host_building(&sugarcane, &multi));
        assert!(island_can_host_building(&cocoa, &multi));
        assert!(
            !island_can_host_building(&cotton, &multi),
            "cotton missing from {{Sugarcane, Cocoa}} island"
        );
    }

    #[test]
    fn load_defs_from_cod() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/haeuser.cod");

        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: haeuser.cod not found");
                return;
            }
        };

        let cod = CodFile::parse(&data).unwrap();
        let defs = load_building_defs(&cod);

        assert_eq!(defs.len(), cod.buildings.len());

        // Find production buildings (those with actual output goods)
        let production: Vec<_> = defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.output_good != Good::None)
            .collect();

        println!("Total defs: {}", defs.len());
        println!("Production buildings: {}", production.len());

        // Print some production buildings
        for (i, d) in production.iter().take(10) {
            let cod_b = &cod.buildings[*i];
            println!(
                "  #{} (cod #{}) {:?} → {:?} (input: {:?} x{}, {:?} x{}) interval={}ms storage={}",
                i,
                cod_b.nummer,
                d.output_good,
                cod_b.properties.get("Ware").unwrap_or(&"?".into()),
                d.input_good_1,
                d.input_1_rate,
                d.input_good_2,
                d.input_2_rate,
                d.cycle_time_ms,
                d.storage_capacity,
            );
        }

        assert!(
            production.len() >= 20,
            "expected >= 20 production buildings"
        );

        // Sanity-check the category/production_type mapping landed.
        let any_residence = defs
            .iter()
            .any(|d| d.production_type == crate::types::ProductionType::Residence);
        let any_plantation = defs
            .iter()
            .any(|d| d.production_type == crate::types::ProductionType::Plantation);
        let any_mine = defs
            .iter()
            .any(|d| d.production_type == crate::types::ProductionType::Mine);
        assert!(any_residence, "expected at least one Residence");
        assert!(any_plantation, "expected at least one Plantation");
        assert!(any_mine, "expected at least one Mine");
        let cat_used: std::collections::HashSet<u8> = defs.iter().map(|d| d.category).collect();
        assert!(
            cat_used.len() >= 4,
            "category mapping should produce multiple categories, got {:?}",
            cat_used,
        );

        let source_maxbrand_values: std::collections::HashSet<_> = cod
            .buildings
            .iter()
            .filter_map(|b| b.properties.get("Maxbrand"))
            .map(|v| v.as_str())
            .collect();
        assert_eq!(
            source_maxbrand_values,
            std::collections::HashSet::from(["4"])
        );
        assert!(
            defs.iter().all(
                |d| d.max_brand_damage_ticks == crate::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS
            ),
            "converted definitions should inherit haeuser.cod Maxbrand: 4",
        );
        let ruin_cases = [
            (270, 8),  // RUINE_KONTOR_1
            (271, 9),  // ObjFill: BASE, then @Ruinenr: +1
            (272, 10), // next @Ruinenr: +1 directive value
            (273, 11), // next @Ruinenr: +1 directive value
            (274, 0),  // RUINE_HOLZ
            (275, 0),  // RUINE_HOLZ
            (276, 2),  // RUINE_STEIN
            (277, 2),  // RUINE_STEIN
            // Nr 356, a beach wall, is the last block that authors
            // `Ruinenr: NORUINE`. The stone gates and the stone watchtower
            // after it restate no `Ruinenr` and therefore take the
            // `ObjFill: 0,MAXHAUS` template's `RUINE_STEIN`. Nr 359 read 255
            // only while the parser carried the previous record forward.
            (356, crate::building::NO_RUIN_ID),
            (357, 2), // stone gate
            (358, 2), // stone gate
            (359, 2), // stone watchtower
        ];
        for (nummer, ruin_id) in ruin_cases {
            let def = defs
                .iter()
                .find(|d| d.id == nummer)
                .unwrap_or_else(|| panic!("missing converted building Nr={nummer}"));
            assert_eq!(def.ruin_id, ruin_id, "Nr={nummer} ruin_id");
        }
    }

    #[test]
    fn load_scenario_buildings() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();

        let cod_data = match std::fs::read(base.join("extracted/haeuser.cod")) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: haeuser.cod not found");
                return;
            }
        };

        // Find any .szs file
        let szenes_dir = base.join("extracted/Szenes");
        let szs_path = match std::fs::read_dir(&szenes_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .find(|e| e.file_name().to_string_lossy().ends_with(".szs"))
                .map(|e| e.path()),
            Err(_) => None,
        };

        let szs_path = match szs_path {
            Some(p) => p,
            None => {
                println!("Skipping: no .szs files found");
                return;
            }
        };

        let cod = CodFile::parse(&cod_data).unwrap();
        let defs = load_building_defs(&cod);
        let szs_data = std::fs::read(&szs_path).unwrap();
        let szs = SzsFile::parse(&szs_data).unwrap();

        let instances = load_building_instances(&szs, &cod, &defs);
        println!(
            "Scenario '{}': {} production building instances",
            szs_path.file_stem().unwrap().to_string_lossy(),
            instances.len()
        );

        for inst in instances.iter().take(10) {
            let def = &defs[inst.def_id as usize];
            println!(
                "  island={} pos=({},{}) output={:?} storage={}",
                inst.island_id, inst.tile_x, inst.tile_y, def.output_good, def.storage_capacity,
            );
        }
    }
}
