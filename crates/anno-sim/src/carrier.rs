//! Type-8 carrier dispatching and movement.
//!
//! The generic `TRAEGER` constructor (`FUN_0044ab60`) starts at a requesting
//! source root. `FUN_00471380` then finds the first reachable producer of the
//! requested good and reserves its fixed-point output through `FUN_0047d810`.
//! The figure walks empty to that supplier, collects the reservation through
//! `FUN_0047d640`, and returns loaded to the requesting root for
//! `FUN_0047d940` delivery.

use crate::building::{BuildingDef, BuildingInstance};
use crate::entity::{ActionType, CargoRoute, Figure};
use crate::island_map::IslandMap;
use crate::source_cell::{SourceMapCellState, SourceType8TransferInput};
use crate::source_route::{
    source_route_positions, SourcePathBlockedCellDecision, SourcePathTargetRect, SourceRouteStep,
};
use crate::types::{Good, SOURCE_WARE_GOODS};
use crate::warehouse::Warehouse;

/// Carrier walking speed in sub-tiles per movement tick (100ms).
pub const CARRIER_SPEED: u16 = 4;

/// `FUN_00451890` converts a simulation delta to figure-motion time by
/// multiplying it by this constant.
const SOURCE_MOTION_TIME_SCALE: f32 = 0.05;

/// The figure-definition loader stores `Speed × 0.0001` at runtime offset
/// `+0x10` before `FUN_0044a690` applies the terrain `Wegspeed` divisor.
const SOURCE_FIGURE_SPEED_SCALE: f32 = 0.0001;

const SOURCE_DIAGONAL_DISTANCE: f32 = std::f32::consts::SQRT_2;

/// Fallback TRAEGER cargo capacity per trip, from figuren.cod `Maxtrag: 4`.
const DEFAULT_CARRIER_MAX_LOAD: u16 = 4;

/// TRAEGER `Gfx: GFXTRAEGER` resolves to this BSH sprite base.
const DEFAULT_CARRIER_SPRITE_BASE: u16 = 0;
const DEFAULT_CARRIER_FRAME_SPEED_MS: u16 = 85;
const DEFAULT_CARRIER_FRAMES_PER_DIRECTION: u8 = 8;

/// Fallback KARREN cargo capacity per trip, from figuren.cod `Maxtrag: 6`.
const DEFAULT_CITY_CART_MAX_LOAD: u16 = 6;

/// KARREN `Gfx: GFXKARREN` resolves to this BSH sprite base.
const DEFAULT_CITY_CART_SPRITE_BASE: u16 = 496;
const DEFAULT_CITY_CART_FRAME_SPEED_MS: u16 = 60;
const DEFAULT_CITY_CART_FRAMES_PER_DIRECTION: u8 = 8;

/// Source-map quantities use 1/32-unit fixed-point storage.
const SOURCE_STORAGE_UNIT: u16 = 32;

/// Compile a `Wegspeed` percentage into the low-seven-bit source path class.
/// `FUN_00462852` uses `min(126, floor(speed * 32 / 100))`.
pub const fn source_path_class(speed: u16) -> u8 {
    let scaled = (speed as u32).saturating_mul(32) / 100;
    if scaled > 126 {
        126
    } else {
        scaled as u8
    }
}

/// `FUN_0045d0b0` scales a moving figure's authored `AnimSpeed` by its
/// current terrain path class. Idle `ENDLESS` animation uses the raw speed.
pub const fn source_animation_frame_duration_ms(
    authored_anim_speed_ms: u16,
    terrain_wegspeed: u16,
    moving: bool,
) -> u32 {
    let raw = if authored_anim_speed_ms == 0 {
        1
    } else {
        authored_anim_speed_ms as u32
    };
    let duration = if moving {
        raw.saturating_mul(source_path_class(terrain_wegspeed) as u32) / 32
    } else {
        raw
    };
    if duration == 0 {
        1
    } else {
        duration
    }
}

/// The source loader multiplies one authored decimal rate by the 8192.0
/// constant at `0x496450`, truncates it, and divides by its 600-tick minute
/// scale before FUN_0047f7b0 consumes it.
const fn city_demand_rate(authored_tenths: u32) -> u32 {
    authored_tenths.saturating_mul(8_192) / 6_000
}

const FOOD_DEMAND_RATE: u32 = city_demand_rate(13);
const CLOTH_DEMAND_RATES: [u32; 5] = [
    0,
    city_demand_rate(6),
    city_demand_rate(7),
    city_demand_rate(8),
    0,
];
const ALCOHOL_DEMAND_RATES: [u32; 5] = [
    0,
    city_demand_rate(5),
    city_demand_rate(6),
    city_demand_rate(7),
    city_demand_rate(8),
];
const TOBACCO_PRODUCTS_DEMAND_RATES: [u32; 5] = [
    0,
    0,
    city_demand_rate(5),
    city_demand_rate(6),
    city_demand_rate(6),
];
const SPICES_DEMAND_RATES: [u32; 5] = [
    0,
    0,
    city_demand_rate(5),
    city_demand_rate(6),
    city_demand_rate(6),
];
const COCOA_DEMAND_RATES: [u32; 5] = [0, 0, 0, city_demand_rate(7), city_demand_rate(6)];
const CLOTHING_DEMAND_RATES: [u32; 5] = [0, 0, 0, 0, city_demand_rate(5)];
const JEWELRY_DEMAND_RATES: [u32; 5] = [0, 0, 0, 0, city_demand_rate(2)];

fn city_demand_target(good: Good, population: [u32; 5]) -> u32 {
    let weighted_total = match good {
        Good::Food => population
            .into_iter()
            .fold(0u32, |total, tier| total.saturating_add(tier))
            .saturating_mul(FOOD_DEMAND_RATE),
        Good::Cloth => weighted_population(population, CLOTH_DEMAND_RATES),
        Good::Alcohol => weighted_population(population, ALCOHOL_DEMAND_RATES),
        Good::TobaccoProducts => weighted_population(population, TOBACCO_PRODUCTS_DEMAND_RATES),
        Good::Spices => weighted_population(population, SPICES_DEMAND_RATES),
        Good::Cocoa => weighted_population(population, COCOA_DEMAND_RATES),
        Good::Clothing => weighted_population(population, CLOTHING_DEMAND_RATES),
        Good::Jewelry => weighted_population(population, JEWELRY_DEMAND_RATES),
        _ => 0,
    };
    weighted_total.saturating_mul(6) >> 8
}

fn weighted_population(population: [u32; 5], rates: [u32; 5]) -> u32 {
    population
        .into_iter()
        .zip(rates)
        .fold(0u32, |total, (population, rate)| {
            total.saturating_add(population.saturating_mul(rate))
        })
}

/// Source-derived carrier constants used by the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarrierConfig {
    /// Maximum goods moved by one TRAEGER trip.
    pub max_load: u16,
    /// Authored TRAEGER `Speed` used by source-grid traversal.
    pub movement_speed: u16,
    /// Resolved empty-walk TRAEGER sprite base in its BSH file.
    pub sprite_base: u16,
    /// Authored `ANIM 0` frame duration.
    pub frame_speed_ms: u16,
    /// Authored `ANIM 0` frames in each rotation strip.
    pub frames_per_direction: u8,
}

impl Default for CarrierConfig {
    fn default() -> Self {
        Self {
            max_load: DEFAULT_CARRIER_MAX_LOAD,
            movement_speed: 220,
            sprite_base: DEFAULT_CARRIER_SPRITE_BASE,
            frame_speed_ms: DEFAULT_CARRIER_FRAME_SPEED_MS,
            frames_per_direction: DEFAULT_CARRIER_FRAMES_PER_DIRECTION,
        }
    }
}

impl CarrierConfig {
    pub fn from_figure_def(def: &anno_formats::figuren::FigureDef) -> Self {
        let default = Self::default();
        let max_load = u16::try_from(def.max_load())
            .ok()
            .filter(|&n| n > 0)
            .unwrap_or(default.max_load);
        let movement_speed = u16::try_from(def.speed())
            .ok()
            .filter(|&n| n > 0)
            .unwrap_or(default.movement_speed);
        let sprite_base = def
            .gfx
            .checked_add(
                def.walk_anim()
                    .map(|animation| animation.anim_offs)
                    .unwrap_or(0),
            )
            .and_then(|sprite| u16::try_from(sprite).ok())
            .unwrap_or(default.sprite_base);
        let frame_speed_ms = def
            .walk_anim()
            .and_then(|anim| u16::try_from(anim.anim_speed).ok())
            .filter(|&speed| speed > 0)
            .unwrap_or(default.frame_speed_ms);
        let frames_per_direction = def
            .walk_anim()
            .and_then(|anim| u8::try_from(anim.anim_anz).ok())
            .filter(|&frames| frames > 0)
            .unwrap_or(default.frames_per_direction);
        Self {
            max_load,
            movement_speed,
            sprite_base,
            frame_speed_ms,
            frames_per_direction,
        }
    }
}

/// Source-derived figure capacity for the type-11 MARKT/KONTOR transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CityCartConfig {
    /// Maximum goods moved by one authored type-11 figure trip.
    pub max_load: u16,
    /// Authored transfer-figure `Speed` used by source-grid traversal.
    pub movement_speed: u16,
    /// Resolved empty-walk transfer-figure sprite base in its BSH file.
    pub sprite_base: u16,
    /// Authored `ANIM 0` frame duration.
    pub frame_speed_ms: u16,
    /// Authored `ANIM 0` frames in each rotation strip.
    pub frames_per_direction: u8,
}

/// Per-good selector bytes built by the city object before a type-11 cart
/// search. FUN_00480610 writes zero for unavailable city capacity, one for
/// ordinary demand, and two for below-half population demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CityCartEligibility {
    pub owner: u8,
    priorities: [u8; 25],
}

impl CityCartEligibility {
    /// Reproduce FUN_00480610 from the local city inventory, its source
    /// shared capacity, and the STADT4 BGRUPPE population counts.
    pub fn from_city_store(warehouse: &Warehouse, city_capacity_fixed: u32) -> Self {
        let mut priorities = [0; 25];
        for (raw_good, good) in SOURCE_WARE_GOODS {
            let stock_fixed = u32::from(warehouse.city_stock_fixed(good));
            if city_capacity_fixed.saturating_sub(stock_fixed) < u32::from(SOURCE_STORAGE_UNIT) {
                continue;
            }
            let target = city_demand_target(good, warehouse.city_population);
            priorities[raw_good as usize] = if stock_fixed < target / 2 { 2 } else { 1 };
        }
        Self {
            owner: warehouse.owner,
            priorities,
        }
    }

    /// Construct decoded selector bytes directly for a city object.
    pub fn from_priorities(owner: u8, priorities: [u8; 25]) -> Self {
        Self { owner, priorities }
    }

    fn priority(self, good: Good) -> u8 {
        good.source_ware_slot()
            .and_then(|slot| self.priorities.get(slot as usize).copied())
            .unwrap_or(0)
    }
}

impl Default for CityCartConfig {
    fn default() -> Self {
        Self {
            max_load: DEFAULT_CITY_CART_MAX_LOAD,
            movement_speed: 300,
            sprite_base: DEFAULT_CITY_CART_SPRITE_BASE,
            frame_speed_ms: DEFAULT_CITY_CART_FRAME_SPEED_MS,
            frames_per_direction: DEFAULT_CITY_CART_FRAMES_PER_DIRECTION,
        }
    }
}

impl CityCartConfig {
    pub fn from_figure_def(def: &anno_formats::figuren::FigureDef) -> Self {
        let default = Self::default();
        let max_load = u16::try_from(def.max_load())
            .ok()
            .filter(|&n| n > 0)
            .unwrap_or(default.max_load);
        let movement_speed = u16::try_from(def.speed())
            .ok()
            .filter(|&n| n > 0)
            .unwrap_or(default.movement_speed);
        let sprite_base = def
            .gfx
            .checked_add(
                def.walk_anim()
                    .map(|animation| animation.anim_offs)
                    .unwrap_or(0),
            )
            .and_then(|sprite| u16::try_from(sprite).ok())
            .unwrap_or(default.sprite_base);
        let frame_speed_ms = def
            .walk_anim()
            .and_then(|anim| u16::try_from(anim.anim_speed).ok())
            .filter(|&speed| speed > 0)
            .unwrap_or(default.frame_speed_ms);
        let frames_per_direction = def
            .walk_anim()
            .and_then(|anim| u8::try_from(anim.anim_anz).ok())
            .filter(|&frames| frames > 0)
            .unwrap_or(default.frames_per_direction);
        Self {
            max_load,
            movement_speed,
            sprite_base,
            frame_speed_ms,
            frames_per_direction,
        }
    }
}

/// A source root which can satisfy a generic carrier request. This is derived
/// from active buildings before dispatch so the mutable source-state table can
/// reserve the selected root without aliasing the building collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarrierSupplier {
    pub island: u8,
    pub owner: u8,
    pub x: u16,
    pub y: u16,
    pub good: Good,
    pub available: u16,
    pub storage: CarrierSupplierStorage,
    /// `Wegspeed[Speedtyp]` compiled into the source-grid path class for
    /// this producer root. The generic supplier set uses TRAEGER's
    /// `Speedtyp: 0`; city scheduling derives a KARREN `Speedtyp: 2` view.
    pub source_path_class: u8,
    /// Oriented source-map footprint of this root. `FUN_004706e0` marks each
    /// of these cells as an owner-specific callback target.
    pub source_footprint: (u8, u8),
}

/// Backing stock selected by the generic source search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarrierSupplierStorage {
    SourceRoot,
    Warehouse(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CarrierRequest {
    good: Good,
    amount: u16,
}

/// Choose the input whose stock is closest to exhaustion. The source scheduler
/// starts a generic carrier at or below two production batches: `FUN_0047daf0`
/// dispatches when `stock * 128 / rate <= 256`, i.e. `stock <= 2 * rate`
/// inclusive, mirroring `carrier_request_for_source_input`.
fn carrier_request(
    building: &BuildingInstance,
    def: &BuildingDef,
    max_load: u16,
) -> Option<CarrierRequest> {
    [
        (def.input_good_1, building.input_1_stock, def.input_1_rate),
        (def.input_good_2, building.input_2_stock, def.input_2_rate),
    ]
    .into_iter()
    .filter_map(|(good, stock, rate)| {
        if good == Good::None {
            return None;
        }
        let batch = rate.max(1);
        let target = batch.saturating_mul(2);
        (stock <= target).then_some((good, stock, batch, target))
    })
    .min_by_key(|(_, stock, batch, _)| (u32::from(*stock) << 16) / u32::from(*batch))
    .map(|(good, stock, _, target)| CarrierRequest {
        good,
        amount: target.saturating_sub(stock).min(max_load).max(1),
    })
}

/// Construct a carrier request for the exact input selected by
/// `FUN_0047daf0` case 1. This preserves the source root's fixed-point
/// choice instead of recomputing priority from rounded building stock.
fn carrier_request_for_source_input(
    building: &BuildingInstance,
    def: &BuildingDef,
    input: SourceType8TransferInput,
    max_load: u16,
) -> Option<CarrierRequest> {
    let (good, stock, rate) = match input {
        SourceType8TransferInput::RawMaterial => {
            (def.input_good_1, building.input_1_stock, def.input_1_rate)
        }
        SourceType8TransferInput::WorkMaterial => {
            (def.input_good_2, building.input_2_stock, def.input_2_rate)
        }
    };
    if good == Good::None {
        return None;
    }
    let batch = rate.max(1);
    let target = batch.saturating_mul(2);
    (stock <= target).then_some(CarrierRequest {
        good,
        amount: target.saturating_sub(stock).min(max_load).max(1),
    })
}

/// `FUN_00471380`'s type-8 supplier admission test (`1602_exe.c:80352-80353`):
///
/// ```c
/// if (((*(byte *)(uVar4 + 0x21) == param_8) ||
///     ((param_8 != 0 && (*(byte *)(uVar4 + 0x21) == 1)))) && ...
/// ```
///
/// The candidate tile's compiled `Ware` byte (definition offset `+0x21`) must
/// equal the requested ware, or be `ALLWARE` (1) for any non-`NOWARE` request.
/// `ALLWARE` is what every KONTOR and MARKT declares, which is how a workshop
/// carrier reaches warehouse stock. The executable applies no map-kind test
/// here at all.
///
/// The comparison runs through `good_for_source_ware_slot`, the same forward
/// table that derives a supplier's `Good` from its compiled byte, because
/// `Good::source_ware_slot` maps slot `0x07` from `Cattle` rather than from
/// the `Meat` that `FLEISCH` parses to; going the other way would silently
/// reject every butcher.
fn source_supplier_ware_admits(candidate_ware_slot: u8, requested_good: Good) -> bool {
    if requested_good == Good::None {
        return candidate_ware_slot == 0;
    }
    candidate_ware_slot == 1
        || crate::data_bridge::good_for_source_ware_slot(candidate_ware_slot) == requested_good
}

/// Run FUN_00471380's first-reservable type-8 supplier search. The callback
/// accepts the first root of the requesting settlement whose requested input
/// has at least half a TRAEGER load available, reserving up to the full
/// fixed-point load.
///
/// `map_owner_slot` is the requesting root's settlement slot, which
/// `FUN_0044ab60` reads out of that root's own map word
/// (`uVar5 = *puVar1 >> 0x13 & 7`, `1602_exe.c:51975`) and stores in the
/// figure-8 event record at `+0x2b`; `FUN_004704d0` compares it against every
/// candidate cell's slot bits (`1602_exe.c:79457-79462`). It is a settlement
/// index, not a player index — two settlements of the same player on one
/// island do not supply each other.
///
/// `source_radius` is the requesting root's compiled `Radius` (`def+0x20`),
/// passed through the event record at `+0x00` (`1602_exe.c:51975-52007`). It
/// bounds the search twice: the scratch window `FUN_004704d0` allocates is the
/// whole coordinate space the wave has, and `FUN_00471280` then carves that
/// rectangle down to the `FUN_00404d70` disc. Both write the impassable
/// direction marker, so a supplier inside the disc whose only walkable route
/// leaves it is unreachable rather than merely distant.
#[allow(clippy::too_many_arguments)]
fn select_carrier_source_wave(
    start: (i32, i32),
    island: u8,
    map_owner_slot: u8,
    source_radius: u8,
    requested_good: Good,
    origin_footprint: (u8, u8),
    suppliers: &[CarrierSupplier],
    source_cells: &[SourceMapCellState],
    warehouses: &[Warehouse],
    map: &IslandMap,
    max_load: u16,
) -> Option<(usize, CarrierSupplier, (i32, i32), Vec<(i32, i32)>, u16)> {
    let max_fixed = max_load.checked_mul(SOURCE_STORAGE_UNIT)?;
    // `FUN_00471380` derives the reservation floor as `param_9 / 2` clamped to
    // `0x40`: `local_24 = param_9 / 2; if (0x3f < local_24) local_24 = 0x40;`.
    // For the stock TRAEGER (`Maxtrag: 4`, `max_fixed == 128`) the clamp is
    // inert (64 == max_fixed / 2); it only bites a modded `Maxtrag > 4`.
    let minimum_fixed = (max_fixed / 2).min(0x40);
    let mut grid = map.carrier_path_grid();

    // `FUN_00459150` sizes the scratch window from the requesting root's
    // oriented footprint (`1602_exe.c:61664-61671`): each axis is
    // `((size - 1) & 1) + 1 + radius * 2`, so an even extent buys one extra
    // cell on the high side. `left`/`top` are `centre - radius`.
    let radius = usize::from(source_radius);
    let extra_x = usize::from((origin_footprint.0.max(1) - 1) & 1);
    let extra_y = usize::from((origin_footprint.1.max(1) - 1) & 1);
    let window_origin = (
        start.0 - i32::from(source_radius),
        start.1 - i32::from(source_radius),
    );
    let window_width = extra_x + 1 + radius * 2;
    let window_height = extra_y + 1 + radius * 2;
    // A root whose oriented footprint misses the window is never rasterised,
    // so it can never carry the goal bit. The source gets that for free — its
    // raster only ever walks window cells — while the port scans the island's
    // whole root table, so the same bound has to be applied by hand. The test
    // uses the candidate's extents rather than its anchor because
    // `FUN_004704d0` stamps the goal bit per cell: a root anchored outside the
    // window still becomes a target through whichever of its cells reach in.
    let intersects_window = |candidate: &SourceMapCellState| {
        if source_radius == 0 {
            return true;
        }
        let last_x = i32::from(candidate.x) + i32::from(candidate.footprint_width.max(1)) - 1;
        let last_y = i32::from(candidate.y) + i32::from(candidate.footprint_height.max(1)) - 1;
        last_x >= window_origin.0
            && i32::from(candidate.x) < window_origin.0 + window_width as i32
            && last_y >= window_origin.1
            && i32::from(candidate.y) < window_origin.1 + window_height as i32
    };

    for candidate in source_cells.iter().copied() {
        if candidate.island != island
            || candidate.source_map_owner_slot != map_owner_slot
            || !source_supplier_ware_admits(candidate.source_output_ware_slot, requested_good)
            || !intersects_window(&candidate)
        {
            continue;
        }
        let Some(supplier) = suppliers.iter().copied().find(|supplier| {
            supplier.island == candidate.island
                && supplier.x == u16::from(candidate.x)
                && supplier.y == u16::from(candidate.y)
        }) else {
            continue;
        };
        let position = (i32::from(candidate.x), i32::from(candidate.y));
        let Some(target) = SourcePathTargetRect::new(
            position,
            usize::from(supplier.source_footprint.0.max(1)),
            usize::from(supplier.source_footprint.1.max(1)),
        ) else {
            continue;
        };
        // FUN_004704d0 replaces both the callback bit and the low-seven-bit
        // path class with the candidate root's `Wegspeed[Speedtyp]` entry.
        grid.set_target_region_metadata(target, (supplier.source_path_class & 0x7f) | 0x80);
    }

    // `FUN_00459150` (`1602_exe.c:61677`) runs `FUN_004710b0` on the carrier's
    // own tile between the raster and the flood, reopening the requesting
    // workshop's whole footprint at cost class `0x28` with the goal bit clear.
    // `FUN_004704d0` has just stamped that footprint as a goal — the workshop
    // is nested production kind 1, inside `1..=8` (`1602_exe.c:79457`) — and a
    // goal cell terminates the wave, so the search would otherwise be confined
    // to its single anchor tile.
    if let Some(footprint) = SourcePathTargetRect::new(
        start,
        usize::from(origin_footprint.0.max(1)),
        usize::from(origin_footprint.1.max(1)),
    ) {
        grid.open_source_object_footprint(footprint);
    }

    // The window is the scratch-grid allocation itself (`1602_exe.c:79439`
    // stamps `0x0c` over every cell of it before the island intersection is
    // filled in), so it also clips the footprint reopened above; applying it
    // after `FUN_004710b0` reproduces that clip. `FUN_00471280` then carves
    // the rectangle to the `FUN_00404d70` disc, which is the order
    // `FUN_00459150` runs them in (`1602_exe.c:61675-61681`).
    //
    // A zero `Radius` keeps the port's unclipped search: the source would give
    // such a root a degenerate `1 x 1` window that can reach nothing at all,
    // and the port's synthetic maps and hand-built fixtures leave the compiled
    // byte at zero. This mirrors the type-11 guard in
    // `select_city_cart_source_wave`.
    if source_radius != 0 {
        grid.block_outside_rect(window_origin, window_width, window_height);
        grid.block_outside_source_radius_window(
            window_origin,
            window_width,
            window_height,
            radius,
            extra_x,
            extra_y,
        );
    }

    let mut selected: Option<(usize, CarrierSupplier, (i32, i32), u16)> = None;
    let result = grid
        .search_with_blocked_cell_callback(start, |position, _| {
            let Some((state_idx, candidate, supplier)) =
                source_cells.iter().copied().enumerate().rev().find_map(
                    |(state_idx, candidate)| {
                        let supplier = suppliers.iter().copied().find(|supplier| {
                            supplier.island == candidate.island
                                && supplier.x == u16::from(candidate.x)
                                && supplier.y == u16::from(candidate.y)
                        })?;
                        let within_footprint = candidate.island == island
                            && candidate.source_map_owner_slot == map_owner_slot
                            && source_supplier_ware_admits(
                                candidate.source_output_ware_slot,
                                requested_good,
                            )
                            && position.0 >= i32::from(candidate.x)
                            && position.1 >= i32::from(candidate.y)
                            && position.0
                                < i32::from(candidate.x)
                                    + i32::from(supplier.source_footprint.0.max(1))
                            && position.1
                                < i32::from(candidate.y)
                                    + i32::from(supplier.source_footprint.1.max(1));
                        within_footprint.then_some((state_idx, candidate, supplier))
                    },
                )
            else {
                return SourcePathBlockedCellDecision::Block;
            };
            if supplier.good != requested_good {
                return SourcePathBlockedCellDecision::Block;
            }
            let available_fixed = match supplier.storage {
                CarrierSupplierStorage::SourceRoot => candidate
                    .storage_fill
                    .saturating_sub(candidate.reserved_storage),
                CarrierSupplierStorage::Warehouse(warehouse_idx) => warehouses
                    .get(warehouse_idx)
                    .map(|warehouse| {
                        warehouse
                            .stock(requested_good)
                            .saturating_sub(warehouse.reserved(requested_good))
                            .saturating_mul(SOURCE_STORAGE_UNIT)
                    })
                    .unwrap_or(0),
            };
            let reserved_fixed = available_fixed.min(max_fixed);
            if reserved_fixed < minimum_fixed || reserved_fixed == 0 {
                return SourcePathBlockedCellDecision::Block;
            }
            selected = Some((state_idx, supplier, position, reserved_fixed));
            SourcePathBlockedCellDecision::Complete
        })
        .ok()?;
    let (state_idx, supplier, reached, reserved_fixed) = selected?;
    if result.position != reached {
        return None;
    }
    let path = source_route_positions(start, &result.steps)?;
    (path.last().copied() == Some(reached)).then_some((
        state_idx,
        supplier,
        reached,
        path,
        reserved_fixed,
    ))
}

/// Try to spawn a generic type-8 carrier for a production input request.
/// Returns `Some(figure)` only after reserving the selected supplier's source
/// storage.
pub fn try_spawn_carrier(
    building: &BuildingInstance,
    def: &BuildingDef,
    suppliers: &[CarrierSupplier],
    source_cells: &mut [SourceMapCellState],
    warehouses: &mut [Warehouse],
    island_maps: &[IslandMap],
    config: CarrierConfig,
) -> Option<Figure> {
    let request = carrier_request(building, def, config.max_load)?;
    try_spawn_carrier_for_request(
        building,
        request,
        suppliers,
        source_cells,
        warehouses,
        island_maps,
        config,
    )
}

/// Spawn a type-8 carrier for the input selector already chosen from a source
/// root's fixed-point transfer branch.
pub fn try_spawn_carrier_for_source_input(
    building: &BuildingInstance,
    def: &BuildingDef,
    input: SourceType8TransferInput,
    suppliers: &[CarrierSupplier],
    source_cells: &mut [SourceMapCellState],
    warehouses: &mut [Warehouse],
    island_maps: &[IslandMap],
    config: CarrierConfig,
) -> Option<Figure> {
    let request = carrier_request_for_source_input(building, def, input, config.max_load)?;
    try_spawn_carrier_for_request(
        building,
        request,
        suppliers,
        source_cells,
        warehouses,
        island_maps,
        config,
    )
}

fn try_spawn_carrier_for_request(
    building: &BuildingInstance,
    request: CarrierRequest,
    suppliers: &[CarrierSupplier],
    source_cells: &mut [SourceMapCellState],
    warehouses: &mut [Warehouse],
    island_maps: &[IslandMap],
    config: CarrierConfig,
) -> Option<Figure> {
    let start = (i32::from(building.tile_x), i32::from(building.tile_y));
    let map = island_maps
        .iter()
        .find(|map| map.island_id == building.island_id);

    // `FUN_004710b0` resolves the footprint through `FUN_00463980`, i.e. from
    // the live map record under the figure's own tile; the port's equivalent
    // record is the requesting root's `SourceMapCellState`. The same record
    // supplies the two bounds `FUN_0044ab60` copies into the figure-8 event:
    // the compiled `Radius` at `+0x00` and the root's own settlement slot at
    // `+0x2b` (`1602_exe.c:51975-52007`).
    let origin_cell = source_cells
        .iter()
        .copied()
        .find(|cell| cell.matches(building.island_id, building.tile_x, building.tile_y));
    let origin_footprint = origin_cell
        .map(|cell| (cell.footprint_width, cell.footprint_height))
        .unwrap_or((1, 1));
    let source_radius = origin_cell
        .map(|cell| cell.source_transfer_radius)
        .unwrap_or(0);
    let map_owner_slot = origin_cell
        .map(|cell| cell.source_map_owner_slot)
        .unwrap_or(0);

    let mut selected: Option<(usize, CarrierSupplier, (i32, i32), Vec<(i32, i32)>, u16)> = None;
    if let Some(map) = map {
        let (state_idx, supplier, reached, path, cargo_fixed) = select_carrier_source_wave(
            start,
            building.island_id,
            map_owner_slot,
            source_radius,
            request.good,
            origin_footprint,
            suppliers,
            source_cells,
            warehouses,
            map,
            config.max_load,
        )?;
        selected = Some((state_idx, supplier, reached, path, cargo_fixed));
    }

    // No static source grid is available for this island. Keep the direct
    // coordinate fallback for synthetic maps and save replay diagnostics.
    for (state_idx, cell) in source_cells.iter().enumerate() {
        if map.is_some() {
            break;
        }
        if cell.island != building.island_id {
            continue;
        }
        let Some(supplier) = suppliers.iter().find(|supplier| {
            supplier.island == cell.island
                && supplier.owner == building.owner
                && supplier.x == u16::from(cell.x)
                && supplier.y == u16::from(cell.y)
                && supplier.good == request.good
                && supplier.available >= request.amount
        }) else {
            continue;
        };
        // `FUN_0047d810` picks the backing store from the nested production
        // kind at definition offset `+0x1c`: `if ((6 < uVar4) && ((uVar4 < 9
        // || (uVar4 == 0x1e))))` reserves from the owner's city store, and
        // every other kind reserves from the root's own `+0x0c` storage
        // (`1602_exe.c:89700-89722`).
        let valid_storage = match supplier.storage {
            CarrierSupplierStorage::SourceRoot => {
                !cell.is_type11_transfer_root()
                    && cell.storage_fill.saturating_sub(cell.reserved_storage)
                        >= request.amount.saturating_mul(SOURCE_STORAGE_UNIT)
            }
            CarrierSupplierStorage::Warehouse(warehouse_idx) => {
                cell.is_type11_transfer_root()
                    && warehouses.get(warehouse_idx).is_some_and(|warehouse| {
                        warehouse
                            .stock(request.good)
                            .saturating_sub(warehouse.reserved(request.good))
                            >= request.amount
                    })
            }
        };
        if !valid_storage {
            continue;
        }
        let goal = (i32::from(supplier.x), i32::from(supplier.y));
        let path = direct_path(start, goal);
        if selected
            .as_ref()
            .is_none_or(|(_, _, _, best_path, _)| path.len() < best_path.len())
        {
            selected = Some((
                state_idx,
                *supplier,
                goal,
                path,
                request.amount.saturating_mul(SOURCE_STORAGE_UNIT),
            ));
        }
    }

    let (state_idx, selected_supplier, reached, path, cargo_fixed) = selected?;
    let cargo = cargo_fixed / SOURCE_STORAGE_UNIT;
    if cargo_fixed == 0
        || (matches!(
            selected_supplier.storage,
            CarrierSupplierStorage::Warehouse(_)
        ) && cargo == 0)
    {
        return None;
    }
    let supplier = source_cells.get_mut(state_idx)?;
    let reserved = match selected_supplier.storage {
        CarrierSupplierStorage::SourceRoot => supplier.reserve_storage(cargo_fixed),
        CarrierSupplierStorage::Warehouse(warehouse_idx) => {
            warehouses.get_mut(warehouse_idx).is_some_and(|warehouse| {
                if !warehouse.reserve(request.good, cargo) {
                    return false;
                }
                if warehouse.reserve_city_good_fixed(request.good, cargo_fixed) {
                    return true;
                }
                warehouse.release_reservation(request.good, cargo);
                false
            })
        }
    };
    if !reserved {
        return None;
    }

    let mut carrier = Figure::new();
    carrier.action = ActionType::CarryingGoods;
    carrier.owner = building.owner;
    carrier.tile_x = i32::from(building.tile_x);
    carrier.tile_y = i32::from(building.tile_y);
    carrier.target_x = reached.0;
    carrier.target_y = reached.1;
    // `FUN_0047d810` and the arrival handlers key the backing store off the
    // nested production kind at definition offset `+0x1c`
    // (`1602_exe.c:89700-89703`), never the outer `HAUS Kind`.
    carrier.destination_kind = supplier.source_production_kind_code;
    carrier.supplier_x = u16::from(supplier.x);
    carrier.supplier_y = u16::from(supplier.y);
    carrier.carried_good = request.good as u8;
    carrier.carried_amount = cargo;
    carrier.cargo_fixed = cargo_fixed;
    carrier.source_transfer_max_load_fixed = config.max_load.saturating_mul(SOURCE_STORAGE_UNIT);
    carrier.speed = CARRIER_SPEED;
    carrier.source_move_speed = config.movement_speed;
    carrier.base_sprite = config.sprite_base;
    carrier.initialize_source_position();
    carrier.source_position_z = map
        .and_then(|map| map.source_terrain_height(start))
        .unwrap_or(0.0)
        .max(0.56);
    carrier.path = path;
    carrier.path_idx = 0;
    Some(carrier)
}

/// `FUN_004706e0`, the type-11 window raster, sets a cell's goal bit only for
/// nested production kinds `1..=6` (`1602_exe.c:79565-79570`:
/// `if ((*(uint *)(iVar3 + 0x1c) == 0) || (6 < *(uint *)(iVar3 + 0x1c))) bVar10 = false;`).
/// The type-8 raster `FUN_004704d0` uses `1..=8` at `1602_exe.c:79457`; the
/// two rasters are otherwise the same routine, and this is the difference that
/// stops a city cart from collecting out of another MARKT or KONTOR store —
/// including the requesting root's own.
#[inline]
const fn source_city_cart_goal_kind(production_kind_code: u8) -> bool {
    production_kind_code != 0 && production_kind_code <= 6
}

/// Start a type-11 city cart from a MARKT or KONTOR source root.
/// `FUN_004717b0` retains the strictly highest `storage_fill × 128 /
/// Maxlager` candidate, accepts consumer goods 11 through 24 at any positive
/// score, and otherwise requires a score above 127. The caller supplies only
/// producers that already passed the decoded city-good eligibility table.
fn select_city_cart_source_wave(
    start: (i32, i32),
    island: u8,
    source_radius: u8,
    map_owner_slot: u8,
    origin_footprint: (u8, u8),
    city: CityCartEligibility,
    suppliers: &[CarrierSupplier],
    source_cells: &[SourceMapCellState],
    map: &IslandMap,
) -> Option<(
    usize,
    CarrierSupplier,
    (i32, i32),
    Vec<(i32, i32)>,
    Vec<SourceRouteStep>,
    u32,
)> {
    let mut grid = map.city_cart_path_grid();
    let radius = i32::from(source_radius);

    // `FUN_004717b0` applies no map-kind test to a candidate producer
    // (`1602_exe.c:80578-80586`): it indexes the city eligibility table by the
    // candidate's compiled `Ware` byte and scores it `fill * 128 / Maxlager`,
    // both of which the acceptance callback below already replays. The goal
    // bit itself comes from the raster, `FUN_004706e0` (`1602_exe.c:79565`),
    // which admits only nested production kinds `1..=6` — a city cart never
    // collects from another MARKT, KONTOR or kind-30 root.
    for candidate in source_cells.iter().copied() {
        if candidate.island != island
            || candidate.source_map_owner_slot != map_owner_slot
            || !source_city_cart_goal_kind(candidate.source_production_kind_code)
        {
            continue;
        }
        if source_radius != 0
            && ((i32::from(candidate.x) - start.0).abs() > radius
                || (i32::from(candidate.y) - start.1).abs() > radius)
        {
            continue;
        }
        let Some(supplier) = suppliers.iter().copied().find(|supplier| {
            supplier.storage == CarrierSupplierStorage::SourceRoot
                && supplier.island == candidate.island
                && supplier.x == u16::from(candidate.x)
                && supplier.y == u16::from(candidate.y)
        }) else {
            continue;
        };
        let position = (i32::from(candidate.x), i32::from(candidate.y));
        let Some(target) = SourcePathTargetRect::new(
            position,
            usize::from(supplier.source_footprint.0.max(1)),
            usize::from(supplier.source_footprint.1.max(1)),
        ) else {
            continue;
        };
        grid.set_target_region_metadata(target, (supplier.source_path_class & 0x7f) | 0x80);
    }

    // `FUN_004596b0` (`1602_exe.c:61878`) runs `FUN_004710b0` on the cart's own
    // tile straight after the raster, which reopens the requesting root's whole
    // footprint at cost class `0x28` with the goal bit cleared. The raster has
    // just stamped that footprint as a goal (the root is production kind 7 or 8
    // in this settlement), and a goal cell terminates the wave rather than
    // expanding it, so without this step a MARKT or KONTOR can only ever leave
    // its single anchor tile.
    if let Some(footprint) = SourcePathTargetRect::new(
        start,
        usize::from(origin_footprint.0.max(1)),
        usize::from(origin_footprint.1.max(1)),
    ) {
        grid.open_source_object_footprint(footprint);
    }

    // The window is the scratch-grid allocation in the source
    // (`1602_exe.c:79546-79553`), so it also clips the footprint reopened
    // above; applying it last reproduces that clip.
    if source_radius != 0 {
        let window_origin = (start.0.checked_sub(radius)?, start.1.checked_sub(radius)?);
        let window_size = usize::try_from(radius.checked_mul(2)?.checked_add(1)?).ok()?;
        grid.block_outside_rect(window_origin, window_size, window_size);
    }

    let mut selected: Option<(usize, CarrierSupplier, (i32, i32), u32)> = None;
    let _ = grid.search_with_blocked_cell_callback(start, |position, _| {
        let Some((state_idx, candidate, supplier)) = source_cells
            .iter()
            .copied()
            .enumerate()
            .rev()
            .find_map(|(state_idx, candidate)| {
                let supplier = suppliers.iter().copied().find(|supplier| {
                    supplier.storage == CarrierSupplierStorage::SourceRoot
                        && supplier.island == candidate.island
                        && supplier.x == u16::from(candidate.x)
                        && supplier.y == u16::from(candidate.y)
                })?;
                let within_footprint = candidate.island == island
                    && candidate.source_map_owner_slot == map_owner_slot
                    && source_city_cart_goal_kind(candidate.source_production_kind_code)
                    && position.0 >= i32::from(candidate.x)
                    && position.1 >= i32::from(candidate.y)
                    && position.0
                        < i32::from(candidate.x) + i32::from(supplier.source_footprint.0.max(1))
                    && position.1
                        < i32::from(candidate.y) + i32::from(supplier.source_footprint.1.max(1));
                within_footprint.then_some((state_idx, candidate, supplier))
            })
        else {
            return SourcePathBlockedCellDecision::Block;
        };
        let Some(score) = candidate.storage_fill_score() else {
            return SourcePathBlockedCellDecision::Block;
        };
        let Some(source_good_slot) = supplier.good.source_ware_slot() else {
            return SourcePathBlockedCellDecision::Block;
        };
        let priority = city.priority(supplier.good);
        if score == 0
            || priority == 0
            || !((11..=24).contains(&source_good_slot) || score > 127)
            || candidate
                .storage_fill
                .saturating_sub(candidate.reserved_storage)
                < SOURCE_STORAGE_UNIT
        {
            return SourcePathBlockedCellDecision::Block;
        }
        if selected
            .as_ref()
            .is_some_and(|(_, _, _, best_score)| score <= *best_score)
        {
            return SourcePathBlockedCellDecision::Block;
        }
        if priority == 2 {
            selected = Some((state_idx, supplier, position, score));
            SourcePathBlockedCellDecision::AdvanceFrontier
        } else {
            if selected
                .as_ref()
                .is_none_or(|(_, _, _, best_score)| score > *best_score)
            {
                selected = Some((state_idx, supplier, position, score));
            }
            SourcePathBlockedCellDecision::Block
        }
    });
    selected.and_then(|(state_idx, supplier, reached, score)| {
        let steps = grid.steps_to_reached_marker(start, reached)?;
        let path = source_route_positions(start, &steps)?;
        (path.last().copied() == Some(reached))
            .then_some((state_idx, supplier, reached, path, steps, score))
    })
}

pub fn try_spawn_city_cart(
    origin: SourceMapCellState,
    city: CityCartEligibility,
    suppliers: &[CarrierSupplier],
    source_cells: &mut [SourceMapCellState],
    island_maps: &[IslandMap],
    config: CityCartConfig,
) -> Option<Figure> {
    if !origin.is_type11_transfer_root() {
        return None;
    }

    let start = (i32::from(origin.x), i32::from(origin.y));
    let map = island_maps
        .iter()
        .find(|map| map.island_id == origin.island);
    let mut selected: Option<(
        usize,
        CarrierSupplier,
        (i32, i32),
        Vec<(i32, i32)>,
        Vec<SourceRouteStep>,
        u32,
    )> = None;

    if let Some(map) = map {
        let (state_idx, supplier, reached, path, route_steps, score) =
            select_city_cart_source_wave(
                start,
                origin.island,
                origin.source_transfer_radius,
                origin.source_map_owner_slot,
                (origin.footprint_width, origin.footprint_height),
                city,
                suppliers,
                source_cells,
                map,
            )?;
        selected = Some((state_idx, supplier, reached, path, route_steps, score));
    }

    for (state_idx, candidate) in source_cells.iter().copied().enumerate() {
        if map.is_some() {
            break;
        }
        if candidate.island != origin.island
            || !source_city_cart_goal_kind(candidate.source_production_kind_code)
        {
            continue;
        }
        let Some(supplier) = suppliers.iter().find(|supplier| {
            supplier.storage == CarrierSupplierStorage::SourceRoot
                && supplier.island == candidate.island
                && supplier.owner == city.owner
                && supplier.x == u16::from(candidate.x)
                && supplier.y == u16::from(candidate.y)
        }) else {
            continue;
        };
        let Some(score) = candidate.storage_fill_score() else {
            continue;
        };
        let Some(source_good_slot) = supplier.good.source_ware_slot() else {
            continue;
        };
        let priority = city.priority(supplier.good);
        if score == 0
            || priority == 0
            || !((11..=24).contains(&source_good_slot) || score > 127)
            || candidate
                .storage_fill
                .saturating_sub(candidate.reserved_storage)
                < SOURCE_STORAGE_UNIT
        {
            continue;
        }
        if selected
            .as_ref()
            .is_some_and(|(_, _, _, _, _, best_score)| score <= *best_score)
        {
            continue;
        }
        let goal = (i32::from(supplier.x), i32::from(supplier.y));
        let path = direct_path(start, goal);
        let route_steps = source_route_steps_from_positions(start, &path)?;
        if priority == 2 {
            selected = Some((state_idx, *supplier, goal, path, route_steps, score));
            break;
        }
        selected = Some((state_idx, *supplier, goal, path, route_steps, score));
    }

    let (state_idx, supplier, reached, path, route_steps, _) = selected?;
    let state = source_cells.get_mut(state_idx)?;
    let available_fixed = state.storage_fill.saturating_sub(state.reserved_storage);
    let cargo_fixed = available_fixed.min(config.max_load.saturating_mul(SOURCE_STORAGE_UNIT));
    let cargo = cargo_fixed / SOURCE_STORAGE_UNIT;
    if cargo_fixed < SOURCE_STORAGE_UNIT || !state.reserve_storage(cargo_fixed) {
        return None;
    }

    let mut cart = Figure::new();
    cart.action = ActionType::CarryingGoods;
    cart.cargo_route = CargoRoute::CityCart;
    cart.owner = city.owner;
    cart.tile_x = start.0;
    cart.tile_y = start.1;
    cart.target_x = reached.0;
    cart.target_y = reached.1;
    cart.destination_kind = state.source_production_kind_code;
    cart.supplier_x = supplier.x;
    cart.supplier_y = supplier.y;
    cart.origin_island = origin.island;
    cart.origin_x = u16::from(origin.x);
    cart.origin_y = u16::from(origin.y);
    cart.origin_kind = origin.kind_code;
    cart.origin_source_map_owner_slot = origin.source_map_owner_slot;
    cart.origin_production_kind = origin.source_production_kind_code;
    cart.carried_good = supplier.good as u8;
    cart.carried_amount = cargo;
    cart.cargo_fixed = cargo_fixed;
    cart.source_transfer_max_load_fixed = config.max_load.saturating_mul(SOURCE_STORAGE_UNIT);
    cart.speed = CARRIER_SPEED;
    cart.source_move_speed = config.movement_speed;
    cart.source_animation_frame_speed_ms = config.frame_speed_ms;
    cart.source_animation_frames_per_direction = config.frames_per_direction;
    cart.base_sprite = config.sprite_base;
    cart.initialize_source_position();
    cart.source_position_z = map
        .and_then(|map| map.source_terrain_height(start))
        .unwrap_or(0.0);
    cart.path = path;
    cart.source_event_route_steps = route_steps;
    cart.path_idx = 0;
    Some(cart)
}

/// Move a carrier one step along its path. Returns true on arrival.
pub fn step_carrier(figure: &mut Figure) -> bool {
    if figure.speed == 0 {
        return false;
    }

    if figure.path_idx < figure.path.len() {
        let (nx, ny) = figure.path[figure.path_idx];
        let dx = nx - figure.tile_x;
        let dy = ny - figure.tile_y;
        figure.direction = direction_from_delta(dx, dy);
        figure.tile_x = nx;
        figure.tile_y = ny;
        figure.path_idx += 1;
        figure.path_idx >= figure.path.len()
    } else {
        let dx = figure.target_x - figure.tile_x;
        let dy = figure.target_y - figure.tile_y;
        if dx == 0 && dy == 0 {
            return true;
        }
        if dx.abs() >= dy.abs() {
            figure.tile_x += dx.signum();
        } else {
            figure.tile_y += dy.signum();
        }
        figure.direction = direction_from_delta(dx, dy);
        figure.tile_x == figure.target_x && figure.tile_y == figure.target_y
    }
}

/// Advance one source-grid carrier step according to `FUN_0044a690` and
/// `FUN_00451890`. The retained path positions are integer cells, so this
/// stores only the source distance still required before the next cell edge.
/// `source_move_speed == 0` retains the direct one-cell behavior for legacy
/// figures that do not carry source route state.
pub fn advance_source_carrier(figure: &mut Figure, elapsed_ms: u32, terrain_wegspeed: u16) -> bool {
    if figure.source_move_speed == 0 {
        return step_carrier(figure);
    }
    if !figure.source_position_initialized {
        figure.initialize_source_position();
    }

    let terrain_wegspeed = terrain_wegspeed.max(1) as f32;
    let mut traversal = elapsed_ms as f32
        * SOURCE_MOTION_TIME_SCALE
        * (figure.source_move_speed as f32 * SOURCE_FIGURE_SPEED_SCALE * 32.0 / terrain_wegspeed);

    // `FUN_00451890` retains the unspent frame time after a movement segment
    // reaches zero and immediately dispatches the next route segment. Keep
    // consuming it here instead of dropping the overrun at a cell boundary.
    loop {
        let next = figure
            .path
            .get(figure.path_idx)
            .copied()
            .unwrap_or((figure.target_x, figure.target_y));
        let dx = next.0 - figure.tile_x;
        let dy = next.1 - figure.tile_y;
        if dx == 0 && dy == 0 {
            return step_carrier(figure);
        }
        // The source-routed figures represented here enter `FUN_0044a690`'s
        // direct direction case, which changes facing before integrating the
        // first fraction of a route segment.
        figure.direction = direction_from_delta(dx, dy);
        let distance = if dx != 0 && dy != 0 {
            SOURCE_DIAGONAL_DISTANCE
        } else {
            1.0
        };
        if figure.source_step_remaining <= 0.0 {
            figure.source_step_remaining = distance;
        }
        if traversal < figure.source_step_remaining {
            figure.source_position_x += dx as f32 * traversal / distance;
            figure.source_position_y += dy as f32 * traversal / distance;
            figure.source_step_remaining -= traversal;
            return false;
        }

        traversal -= figure.source_step_remaining;
        figure.source_step_remaining = 0.0;
        if step_carrier(figure) {
            figure.initialize_source_position();
            return true;
        }
        figure.initialize_source_position();
        if traversal <= 0.0 {
            return false;
        }
    }
}

fn direction_from_delta(dx: i32, dy: i32) -> u8 {
    match (dx.signum(), dy.signum()) {
        (0, -1) => 0,
        (1, -1) => 1,
        (1, 0) => 2,
        (1, 1) => 3,
        (0, 1) => 4,
        (-1, 1) => 5,
        (-1, 0) => 6,
        (-1, -1) => 7,
        _ => 0,
    }
}

/// Advance a type-8 figure across an arrival boundary. On the outbound leg it
/// changes to `Returning` while retaining the picked cargo; the caller applies
/// the source-state transfer at that boundary. On the return leg it despawns.
pub fn handle_arrival(
    figure: &mut Figure,
    buildings: &[BuildingInstance],
    island_maps: &[IslandMap],
) -> bool {
    if figure.cargo_route == CargoRoute::CityCart {
        return match figure.action {
            ActionType::CarryingGoods => {
                let start = (figure.tile_x, figure.tile_y);
                let goal = (i32::from(figure.origin_x), i32::from(figure.origin_y));
                let (route_steps, path) = match island_maps
                    .iter()
                    .find(|map| map.island_id == figure.origin_island)
                {
                    Some(map) => {
                        match source_carrier_return_path(map, start, goal, CargoRoute::CityCart) {
                            Some(route) => route,
                            None => return true,
                        }
                    }
                    None => {
                        let path = direct_path(start, goal);
                        let Some(steps) = source_route_steps_from_positions(start, &path) else {
                            return true;
                        };
                        (steps, path)
                    }
                };
                figure.target_x = goal.0;
                figure.target_y = goal.1;
                figure.action = ActionType::Returning;
                figure.reset_source_animation();
                figure.path = path;
                figure.source_event_route_steps = route_steps;
                figure.path_idx = 0;
                figure.source_step_remaining = 0.0;
                false
            }
            ActionType::Returning => true,
            _ => true,
        };
    }

    match figure.action {
        ActionType::CarryingGoods => {
            let Some(building) = buildings.get(figure.building_idx as usize) else {
                return true;
            };
            let start = (figure.tile_x, figure.tile_y);
            let goal = (i32::from(building.tile_x), i32::from(building.tile_y));
            let (_route_steps, path) = match island_maps
                .iter()
                .find(|map| map.island_id == building.island_id)
            {
                Some(map) => {
                    match source_carrier_return_path(map, start, goal, CargoRoute::InputCarrier) {
                        Some(route) => route,
                        None => return true,
                    }
                }
                None => {
                    let path = direct_path(start, goal);
                    let Some(steps) = source_route_steps_from_positions(start, &path) else {
                        return true;
                    };
                    (steps, path)
                }
            };
            figure.target_x = goal.0;
            figure.target_y = goal.1;
            figure.action = ActionType::Returning;
            figure.reset_source_animation();
            figure.path = path;
            figure.path_idx = 0;
            figure.source_step_remaining = 0.0;
            false
        }
        ActionType::Returning => true,
        _ => true,
    }
}

/// Rebuild the loaded return route with the same fixed-cost wave that creates
/// the outbound type-8/type-11 route. `FUN_0046c7d0` accepts the return root
/// through its blocked-cell callback, so a root need not be a traversable
/// static terrain cell for this search to complete.
fn source_carrier_return_path(
    map: &IslandMap,
    start: (i32, i32),
    goal: (i32, i32),
    cargo_route: CargoRoute,
) -> Option<(Vec<SourceRouteStep>, Vec<(i32, i32)>)> {
    let mut grid = match cargo_route {
        CargoRoute::InputCarrier => map.carrier_path_grid(),
        CargoRoute::CityCart => map.city_cart_path_grid(),
    };
    let steps = grid.route_to(start, goal).ok()?;
    let path = source_route_positions(start, &steps)?;
    (path.last().copied() == Some(goal) || (start == goal && path.is_empty()))
        .then_some((steps, path))
}

/// Reconstruct source route steps for synthetic-map fallbacks, where no
/// source grid exists to retain the original metadata.
fn source_route_steps_from_positions(
    start: (i32, i32),
    positions: &[(i32, i32)],
) -> Option<Vec<SourceRouteStep>> {
    let mut previous = start;
    let mut steps = Vec::with_capacity(positions.len());
    for &position in positions {
        let direction = match (position.0 - previous.0, position.1 - previous.1) {
            (0, -1) => 1,
            (1, -1) => 2,
            (1, 0) => 3,
            (1, 1) => 4,
            (0, 1) => 5,
            (-1, 1) => 6,
            (-1, 0) => 7,
            (-1, -1) => 8,
            _ => return None,
        };
        steps.push(SourceRouteStep {
            direction,
            metadata: 0,
        });
        previous = position;
    }
    Some(steps)
}

/// Generate a direct path (no obstacles) from start to goal.
fn direct_path(start: (i32, i32), goal: (i32, i32)) -> Vec<(i32, i32)> {
    let mut path = Vec::new();
    let mut pos = start;
    while pos != goal {
        let dx = goal.0 - pos.0;
        let dy = goal.1 - pos.1;
        if dx != 0 && dy != 0 {
            pos = (pos.0 + dx.signum(), pos.1 + dy.signum());
        } else if dx != 0 {
            pos = (pos.0 + dx.signum(), pos.1);
        } else {
            pos = (pos.0, pos.1 + dy.signum());
        }
        path.push(pos);
    }
    path
}

/// Convert Good u8 repr back to Good enum.
pub(crate) fn good_from_u8(val: u8) -> Good {
    match val {
        1 => Good::Wood,
        2 => Good::Iron,
        3 => Good::Gold,
        4 => Good::Wool,
        5 => Good::Sugar,
        6 => Good::Tobacco,
        7 => Good::Cattle,
        8 => Good::Grain,
        9 => Good::Flour,
        10 => Good::Tools,
        11 => Good::Bricks,
        12 => Good::Swords,
        13 => Good::Muskets,
        14 => Good::Cannons,
        15 => Good::Food,
        16 => Good::Cloth,
        17 => Good::Alcohol,
        18 => Good::TobaccoProducts,
        19 => Good::Spices,
        20 => Good::Cocoa,
        21 => Good::Grapes,
        22 => Good::Stone,
        23 => Good::Ore,
        25 => Good::WildGame,
        26 => Good::Cotton,
        27 => Good::Silk,
        28 => Good::Jewelry,
        29 => Good::Clothing,
        30 => Good::Fish,
        31 => Good::Meat,
        32 => Good::SugarCane,
        _ => Good::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProductionType;

    fn consumer_def() -> BuildingDef {
        BuildingDef {
            production_type: ProductionType::Craft,
            input_good_1: Good::Iron,
            input_1_rate: 2,
            ..Default::default()
        }
    }

    /// A producer root shaped the way haeuser.cod ships one: outer
    /// `Kind: GEBAEUDE` with the production label in the nested
    /// `HAUS_PRODTYP Kind`, and the produced `Ware` that `FUN_00471380`
    /// matches against a workshop's request (`1602_exe.c:80352-80353`).
    fn supplier_state(x: u8, y: u8, fill: u16, good: Good) -> SourceMapCellState {
        SourceMapCellState {
            storage_fill: fill,
            source_output_ware_slot: good.source_ware_slot().unwrap(),
            ..SourceMapCellState::new(
                0,
                x,
                y,
                &anno_formats::cod::BuildingDef {
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "HANDWERK".into())].into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        }
    }

    #[test]
    fn source_selected_input_accepts_the_inclusive_two_batch_boundary() {
        let mut consumer = BuildingInstance::new(0, 0, 5, 5, 0);
        consumer.input_1_stock = 4;
        let definition = consumer_def();

        assert_eq!(
            carrier_request_for_source_input(
                &consumer,
                &definition,
                SourceType8TransferInput::RawMaterial,
                4,
            ),
            Some(CarrierRequest {
                good: Good::Iron,
                amount: 1,
            })
        );
    }

    #[test]
    fn source_carrier_step_uses_runtime_speed_and_raw_wegspeed() {
        let mut carrier = Figure::new();
        carrier.action = ActionType::CarryingGoods;
        carrier.speed = CARRIER_SPEED;
        carrier.source_move_speed = 220;
        carrier.target_x = 1;
        carrier.path = vec![(1, 0)];

        assert!(!advance_source_carrier(&mut carrier, 100, 100));
        assert_eq!(carrier.direction, 2);
        assert_eq!((carrier.tile_x, carrier.tile_y), (0, 0));

        for _ in 0..27 {
            assert!(!advance_source_carrier(&mut carrier, 100, 100));
        }
        assert_eq!((carrier.tile_x, carrier.tile_y), (0, 0));
        assert!((carrier.source_position_x - 1.4856).abs() < 0.000_001);
        assert_eq!(carrier.source_position_y, 0.5);
        assert!(advance_source_carrier(&mut carrier, 100, 100));
        assert_eq!((carrier.tile_x, carrier.tile_y), (1, 0));
        assert_eq!(carrier.source_step_remaining, 0.0);
        assert_eq!(
            (carrier.source_position_x, carrier.source_position_y),
            (1.5, 0.5)
        );
    }

    #[test]
    fn source_carrier_diagonal_step_uses_sqrt_two_distance() {
        let mut carrier = Figure::new();
        carrier.action = ActionType::CarryingGoods;
        carrier.speed = CARRIER_SPEED;
        carrier.source_move_speed = 220;
        carrier.target_x = 1;
        carrier.target_y = 1;
        carrier.path = vec![(1, 1)];

        for _ in 0..40 {
            assert!(!advance_source_carrier(&mut carrier, 100, 100));
        }
        assert_eq!((carrier.tile_x, carrier.tile_y), (0, 0));
        assert!((carrier.source_position_x - 1.495_606_7).abs() < 0.000_001);
        assert!((carrier.source_position_y - 1.495_606_7).abs() < 0.000_001);
        assert!(advance_source_carrier(&mut carrier, 100, 100));
        assert_eq!((carrier.tile_x, carrier.tile_y), (1, 1));
        assert_eq!(
            (carrier.source_position_x, carrier.source_position_y),
            (1.5, 1.5)
        );
    }

    #[test]
    fn source_carrier_carries_overrun_into_the_next_route_cell() {
        let mut carrier = Figure::new();
        carrier.action = ActionType::CarryingGoods;
        carrier.speed = CARRIER_SPEED;
        carrier.source_move_speed = 12_500;
        carrier.target_x = 3;
        carrier.path = vec![(1, 0), (2, 0), (3, 0)];

        assert!(!advance_source_carrier(&mut carrier, 100, 100));
        assert_eq!((carrier.tile_x, carrier.tile_y), (2, 0));
        assert_eq!(carrier.path_idx, 2);
        assert_eq!(carrier.source_step_remaining, 0.0);
        assert_eq!(
            (carrier.source_position_x, carrier.source_position_y),
            (2.5, 0.5)
        );
    }

    #[test]
    fn generic_carrier_reserves_nearest_reachable_matching_supplier() {
        let consumer = BuildingInstance::new(0, 0, 5, 5, 0);
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 4,
                y: 4,
                good: Good::Iron,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 0,
                source_footprint: (1, 1),
            },
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 20,
                y: 20,
                good: Good::Iron,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 0,
                source_footprint: (1, 1),
            },
        ];
        let mut states = vec![
            supplier_state(4, 4, 128, Good::Iron),
            supplier_state(20, 20, 128, Good::Iron),
        ];

        let carrier = try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &suppliers,
            &mut states,
            &mut [],
            &[],
            CarrierConfig {
                sprite_base: 12,
                ..CarrierConfig::default()
            },
        )
        .expect("matching supplier should be reserved");

        assert_eq!((carrier.target_x, carrier.target_y), (4, 4));
        assert_eq!(carrier.carried_good, Good::Iron as u8);
        assert_eq!(carrier.carried_amount, 4);
        assert_eq!(carrier.base_sprite, 12);
        assert_eq!(states[0].reserved_storage, 128);
        assert_eq!(states[1].reserved_storage, 0);
    }

    #[test]
    fn generic_carrier_ignores_nonmatching_or_unavailable_supplier() {
        let consumer = BuildingInstance::new(0, 0, 5, 5, 0);
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 4,
                y: 4,
                good: Good::Wood,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 0,
                source_footprint: (1, 1),
            },
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 8,
                y: 5,
                good: Good::Iron,
                available: 3,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 0,
                source_footprint: (1, 1),
            },
        ];
        let mut states = vec![
            supplier_state(4, 4, 128, Good::Wood),
            supplier_state(8, 5, 128, Good::Iron),
        ];

        assert!(try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &suppliers,
            &mut states,
            &mut [],
            &[],
            CarrierConfig::default(),
        )
        .is_none());
    }

    #[test]
    fn generic_carrier_source_wave_reserves_the_first_qualifying_fixed_load() {
        let consumer = BuildingInstance::new(0, 0, 0, 0, 0);
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 4,
            y: 0,
            good: Good::Iron,
            available: 4,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        };
        let mut states = [supplier_state(4, 0, 96, Good::Iron)];

        let carrier = try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[supplier],
            &mut states,
            &mut [],
            &[IslandMap::new_open(0, 5, 1)],
            CarrierConfig::default(),
        )
        .expect("96 fixed units exceed the 64-unit TRAEGER reservation floor");

        assert_eq!((carrier.target_x, carrier.target_y), (4, 0));
        assert_eq!((carrier.supplier_x, carrier.supplier_y), (4, 0));
        assert_eq!(carrier.carried_amount, 3);
        assert_eq!(carrier.cargo_fixed, 96);
        assert_eq!(states[0].reserved_storage, 96);
    }

    #[test]
    fn generic_carrier_preserves_nonintegral_source_reservation() {
        let consumer = BuildingInstance::new(0, 0, 0, 0, 0);
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 4,
            y: 0,
            good: Good::Iron,
            available: 4,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        };
        let mut states = [supplier_state(4, 0, 65, Good::Iron)];

        let carrier = try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[supplier],
            &mut states,
            &mut [],
            &[IslandMap::new_open(0, 5, 1)],
            CarrierConfig::default(),
        )
        .expect("the 64-unit threshold admits a 65-unit source reservation");

        assert_eq!(carrier.carried_amount, 2);
        assert_eq!(carrier.cargo_fixed, 65);
        assert_eq!(states[0].reserved_storage, 65);
    }

    #[test]
    fn generic_carrier_source_wave_rejects_stock_below_half_load() {
        let consumer = BuildingInstance::new(0, 0, 0, 0, 0);
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 4,
            y: 0,
            good: Good::Iron,
            available: 4,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        };
        let mut states = [supplier_state(4, 0, 32, Good::Iron)];

        assert!(try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[supplier],
            &mut states,
            &mut [],
            &[IslandMap::new_open(0, 5, 1)],
            CarrierConfig::default(),
        )
        .is_none());
        assert_eq!(states[0].reserved_storage, 0);
    }

    #[test]
    fn generic_carrier_uses_source_grid_callback_cell_and_route() {
        let consumer = BuildingInstance::new(0, 0, 0, 0, 0);
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 3,
            y: 0,
            good: Good::Iron,
            available: 4,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (2, 1),
        };
        let mut states = [supplier_state(3, 0, 128, Good::Iron)];
        let mut map = IslandMap::new_open(0, 5, 1);
        map.set_walkable(2, 0, false);

        let carrier = try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[supplier],
            &mut states,
            &mut [],
            &[map],
            CarrierConfig::default(),
        )
        .expect("source-grid wave reaches the footprint despite A* walkability");

        assert_eq!((carrier.target_x, carrier.target_y), (3, 0));
        assert_eq!((carrier.supplier_x, carrier.supplier_y), (3, 0));
        assert_eq!(carrier.path.last(), Some(&(3, 0)));
        assert!(carrier.path.contains(&(2, 0)));
    }

    #[test]
    fn generic_carrier_keeps_supplier_anchor_when_wave_reaches_footprint_edge() {
        let consumer = BuildingInstance::new(0, 0, 2, 2, 0);
        // The supplier's east cell (1, 1) is the one the wave meets; its
        // anchor (0, 1) is a further step west.
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 0,
            y: 1,
            good: Good::Iron,
            available: 4,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (2, 1),
        };
        let mut states = [supplier_state(0, 1, 128, Good::Iron)];

        let carrier = try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[supplier],
            &mut states,
            &mut [],
            &[IslandMap::new_open(0, 5, 5)],
            CarrierConfig::default(),
        )
        .expect("the footprint's east edge is a valid source-grid callback");

        assert_eq!((carrier.target_x, carrier.target_y), (1, 1));
        assert_eq!((carrier.supplier_x, carrier.supplier_y), (0, 1));
        assert_eq!(carrier.path.last(), Some(&(1, 1)));
    }

    #[test]
    fn kontor_supplier_reserves_city_stock_without_storage_animation_fill() {
        let consumer = BuildingInstance::new(0, 0, 5, 5, 0);
        let suppliers = [CarrierSupplier {
            island: 0,
            owner: 0,
            x: 4,
            y: 4,
            good: Good::Iron,
            available: 4,
            storage: CarrierSupplierStorage::Warehouse(0),
            source_path_class: 0,
            source_footprint: (1, 1),
        }];
        let mut states = vec![
            SourceMapCellState::new(
                0,
                4,
                4,
                &anno_formats::cod::BuildingDef {
                    kind: "GEBAEUDE".into(),
                    // Every KONTOR in haeuser.cod declares `Ware: ALLWARE`,
                    // which is the `def+0x21 == 1` half of `FUN_00471380`'s
                    // supplier test (`1602_exe.c:80352-80353`).
                    properties: [
                        ("ProdKind".into(), "KONTOR".into()),
                        ("Ware".into(), "ALLWARE".into()),
                    ]
                    .into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap(),
        ];

        let mut warehouses = [Warehouse::new(0, 0, 4, 4)];
        warehouses[0].deposit(Good::Iron, 4);
        assert!(try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &suppliers,
            &mut states,
            &mut warehouses,
            &[],
            CarrierConfig::default(),
        )
        .is_some());
        assert_eq!(states[0].storage_fill, 0);
        assert_eq!(states[0].reserved_storage, 0);
        assert_eq!(warehouses[0].reserved(Good::Iron), 4);
    }

    /// The requesting workshop's own live record. `FUN_0044ab60` reads the
    /// compiled `Radius` and the settlement slot out of that root's map word
    /// (`uVar5 = *puVar1 >> 0x13 & 7`) into the figure-8 event record
    /// (`1602_exe.c:51974-52007`), and `FUN_00459150` bounds the whole search
    /// with the pair. Its own `Ware` stays `NOWARE`, so the raster never marks
    /// the requester as a goal for an Iron request.
    fn requester_state(x: u8, y: u8, radius: u8, slot: u8) -> SourceMapCellState {
        SourceMapCellState {
            source_transfer_radius: radius,
            source_map_owner_slot: slot,
            ..SourceMapCellState::new(
                0,
                x,
                y,
                &anno_formats::cod::BuildingDef {
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "HANDWERK".into())].into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        }
    }

    fn iron_supplier(x: u16, y: u16) -> CarrierSupplier {
        CarrierSupplier {
            island: 0,
            owner: 0,
            x,
            y,
            good: Good::Iron,
            available: 4,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        }
    }

    /// A square island of open ground with the listed cells left out.
    /// `populate_static_island_cells` pre-fills the whole TRAEGER template
    /// with the `0x0c` blocker and opens only the tiles it is given, exactly
    /// as `FUN_004704d0` fills its window (`1602_exe.c:79439-79446`), so an
    /// absent tile is impassable.
    fn open_island_map_without(size: u8, blocked: &[(u8, u8)]) -> IslandMap {
        let ground = anno_formats::cod::BuildingDef {
            source_id: 20_001,
            kind: "BODEN".to_owned(),
            properties: [("Wegspeed".to_owned(), "100,100,100,100".to_owned())].into(),
            ..Default::default()
        };
        let tiles = (0..size)
            .flat_map(|y| (0..size).map(move |x| (x, y)))
            .filter(|position| !blocked.contains(position))
            .map(|(x, y)| anno_formats::szs::IslandTile {
                building_id: 1,
                x,
                y,
                orientation: 0,
                anim_count: 0,
                flags: 0,
            })
            .collect();
        let island = anno_formats::szs::Island {
            number: 0,
            width: size,
            height: size,
            x_pos: 0,
            y_pos: 0,
            fertilities: [7; 8],
            tiles,
            city: None,
        };
        IslandMap::from_island(&island, &[ground])
    }

    /// `FUN_00471280` carves the raw `radius * 2 + 1` window down to the
    /// `FUN_00404d70` disc (`1602_exe.c:80102-80131`). Five rows above the
    /// centre `DAT_005b7460[5]` retains a half-width of 2, so an Iron
    /// producer at `dx == 2` is still a goal.
    #[test]
    fn generic_carrier_source_wave_accepts_a_supplier_on_the_radius_disc_edge() {
        let consumer = BuildingInstance::new(0, 0, 10, 10, 0);
        let mut states = vec![
            requester_state(10, 10, 5, 0),
            supplier_state(12, 5, 128, Good::Iron),
        ];

        let carrier = try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[iron_supplier(12, 5)],
            &mut states,
            &mut [],
            &[IslandMap::new_open(0, 20, 20)],
            CarrierConfig::default(),
        )
        .expect("a producer inside the radius-5 disc is reachable");

        assert_eq!((carrier.target_x, carrier.target_y), (12, 5));
        assert_eq!(states[1].reserved_storage, 128);
    }

    /// One column further out the same row is outside the disc — but still
    /// inside the `11 x 11` rectangle, so only `FUN_00471280`'s carve rejects
    /// it. Before the radius bound existed the port searched the whole island
    /// and accepted this producer.
    #[test]
    fn generic_carrier_source_wave_rejects_a_supplier_one_tile_outside_the_disc() {
        let consumer = BuildingInstance::new(0, 0, 10, 10, 0);
        let mut states = vec![
            requester_state(10, 10, 5, 0),
            supplier_state(13, 5, 128, Good::Iron),
        ];

        assert!(try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[iron_supplier(13, 5)],
            &mut states,
            &mut [],
            &[IslandMap::new_open(0, 20, 20)],
            CarrierConfig::default(),
        )
        .is_none());
        assert_eq!(states[1].reserved_storage, 0);
    }

    /// The disc is a wall, not a scoring bound: `FUN_00471280` writes the same
    /// `0x0c` direction marker the raster uses for impassable ground
    /// (`1602_exe.c:80140-80147`), and `FUN_00471380` has no step budget, so a
    /// producer sitting inside the disc whose only walkable route leaves it is
    /// unreachable. The wall here spans exactly the disc's width three rows
    /// above the centre (`half_width == 4`), leaving the two carved-out cells
    /// of that row as the island's only way around.
    #[test]
    fn generic_carrier_source_wave_cannot_route_around_the_disc() {
        let wall: Vec<(u8, u8)> = (6..=14).map(|x| (x, 7)).collect();
        let consumer = BuildingInstance::new(0, 0, 10, 10, 0);
        let mut states = vec![
            requester_state(10, 10, 5, 0),
            supplier_state(10, 5, 128, Good::Iron),
        ];

        assert!(try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[iron_supplier(10, 5)],
            &mut states,
            &mut [],
            &[open_island_map_without(20, &wall)],
            CarrierConfig::default(),
        )
        .is_none());
        assert_eq!(states[1].reserved_storage, 0);

        // The detour at `x == 5` and `x == 15` is open ground, so the same
        // fixture succeeds for a root whose compiled `Radius` leaves the
        // search unclipped — which is what every type-8 search did before the
        // window and the carve were ported.
        let mut unbounded = vec![
            requester_state(10, 10, 0, 0),
            supplier_state(10, 5, 128, Good::Iron),
        ];
        assert!(try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[iron_supplier(10, 5)],
            &mut unbounded,
            &mut [],
            &[open_island_map_without(20, &wall)],
            CarrierConfig::default(),
        )
        .is_some());
    }

    /// `FUN_004704d0` compares the candidate cell's slot bits with the event
    /// record's `+0x2b` (`1602_exe.c:79457-79462`), which `FUN_0044ab60` took
    /// from the requesting root's own map word. That is a settlement index,
    /// not a player index: one player's second settlement on the same island
    /// is as foreign to this workshop as another player's.
    #[test]
    fn generic_carrier_source_wave_rejects_a_producer_from_another_settlement_slot() {
        let consumer = BuildingInstance::new(0, 0, 10, 10, 0);
        let mut states = vec![
            requester_state(10, 10, 5, 0),
            SourceMapCellState {
                source_map_owner_slot: 1,
                ..supplier_state(11, 10, 128, Good::Iron)
            },
        ];

        assert!(try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[iron_supplier(11, 10)],
            &mut states,
            &mut [],
            &[IslandMap::new_open(0, 20, 20)],
            CarrierConfig::default(),
        )
        .is_none());
        assert_eq!(states[1].reserved_storage, 0);

        let mut same_settlement = vec![
            requester_state(10, 10, 5, 0),
            supplier_state(11, 10, 128, Good::Iron),
        ];
        assert!(try_spawn_carrier(
            &consumer,
            &consumer_def(),
            &[iron_supplier(11, 10)],
            &mut same_settlement,
            &mut [],
            &[IslandMap::new_open(0, 20, 20)],
            CarrierConfig::default(),
        )
        .is_some());
    }

    #[test]
    fn outbound_arrival_preserves_cargo_for_loaded_return() {
        let mut figure = Figure::new();
        figure.action = ActionType::CarryingGoods;
        figure.tile_x = 8;
        figure.tile_y = 5;
        figure.building_idx = 0;
        figure.carried_good = Good::Iron as u8;
        figure.carried_amount = 4;
        figure.anim_frame = 5;
        figure.source_animation_elapsed_ms = 425;
        let buildings = vec![BuildingInstance::new(0, 0, 2, 5, 0)];

        assert!(!handle_arrival(&mut figure, &buildings, &[]));
        assert_eq!(figure.action, ActionType::Returning);
        assert_eq!(figure.carried_amount, 4);
        assert_eq!((figure.target_x, figure.target_y), (2, 5));
        assert_eq!(figure.anim_frame, 0);
        assert_eq!(figure.source_animation_elapsed_ms, 0);
    }

    #[test]
    fn loaded_city_cart_return_keeps_the_source_wave_grid() {
        let mut figure = Figure::new();
        figure.action = ActionType::CarryingGoods;
        figure.cargo_route = CargoRoute::CityCart;
        figure.origin_island = 0;
        figure.origin_x = 0;
        figure.origin_y = 0;
        figure.tile_x = 2;
        let mut map = IslandMap::new_open(0, 3, 1);
        map.set_walkable(1, 0, false);

        assert!(!handle_arrival(&mut figure, &[], &[map]));
        assert_eq!(figure.action, ActionType::Returning);
        assert_eq!(figure.path, vec![(1, 0), (0, 0)]);
    }

    #[test]
    fn loaded_input_carrier_return_keeps_the_source_wave_grid() {
        let mut figure = Figure::new();
        figure.action = ActionType::CarryingGoods;
        figure.building_idx = 0;
        figure.tile_x = 2;
        let buildings = [BuildingInstance::new(0, 0, 0, 0, 0)];
        let mut map = IslandMap::new_open(0, 3, 1);
        map.set_walkable(1, 0, false);

        assert!(!handle_arrival(&mut figure, &buildings, &[map]));
        assert_eq!(figure.action, ActionType::Returning);
        assert_eq!(figure.path, vec![(1, 0), (0, 0)]);
    }

    #[test]
    fn carrier_config_uses_figuren_maxtrag() {
        let mut fig = anno_formats::figuren::FigureDef::default();
        fig.properties.insert("Maxtrag".into(), "7".into());
        fig.properties.insert("Speed".into(), "235".into());
        fig.gfx = 12;
        fig.anims.push(anno_formats::figuren::FigureAnim {
            nummer: 0,
            anim_anz: 6,
            anim_speed: 91,
            ..Default::default()
        });
        let config = CarrierConfig::from_figure_def(&fig);
        assert_eq!(config.max_load, 7);
        assert_eq!(config.movement_speed, 235);
        assert_eq!(config.sprite_base, 12);
        assert_eq!(config.frame_speed_ms, 91);
        assert_eq!(config.frames_per_direction, 6);
    }

    #[test]
    fn city_cart_uses_karren_maxtrag() {
        let mut fig = anno_formats::figuren::FigureDef::default();
        fig.properties.insert("Maxtrag".into(), "6".into());
        fig.properties.insert("Speed".into(), "300".into());
        fig.gfx = 496;
        fig.anims.push(anno_formats::figuren::FigureAnim {
            nummer: 0,
            anim_anz: 7,
            anim_speed: 73,
            ..Default::default()
        });
        let config = CityCartConfig::from_figure_def(&fig);
        assert_eq!(config.max_load, 6);
        assert_eq!(config.movement_speed, 300);
        assert_eq!(config.sprite_base, 496);
        assert_eq!(config.frame_speed_ms, 73);
        assert_eq!(config.frames_per_direction, 7);
    }

    #[test]
    fn source_path_class_matches_compiled_wegspeed_scale() {
        assert_eq!(source_path_class(100), 32);
        assert_eq!(source_path_class(170), 54);
        assert_eq!(source_path_class(u16::MAX), 126);
    }

    #[test]
    fn source_animation_duration_uses_the_live_path_class_only_while_moving() {
        assert_eq!(source_animation_frame_duration_ms(85, 100, true), 85);
        assert_eq!(source_animation_frame_duration_ms(85, 150, true), 127);
        assert_eq!(source_animation_frame_duration_ms(60, 150, true), 90);
        assert_eq!(source_animation_frame_duration_ms(85, 150, false), 85);
    }

    #[test]
    fn city_cart_prefers_the_highest_eligible_storage_score() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut lower = supplier_state(2, 0, 128, Good::Cloth);
        lower.storage_animation_capacity = 320;
        let mut higher = supplier_state(4, 0, 192, Good::Cloth);
        higher.storage_animation_capacity = 320;
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 2,
                y: 0,
                good: Good::Cloth,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 0,
                source_footprint: (1, 1),
            },
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 4,
                y: 0,
                good: Good::Cloth,
                available: 6,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 0,
                source_footprint: (1, 1),
            },
        ];
        let mut states = vec![lower, higher];

        let city = CityCartEligibility::from_priorities(0, [1; 25]);
        let cart = try_spawn_city_cart(
            origin,
            city,
            &suppliers,
            &mut states,
            &[],
            CityCartConfig {
                sprite_base: 496,
                ..CityCartConfig::default()
            },
        )
        .expect("highest-fill supplier should be reserved");

        assert_eq!((cart.target_x, cart.target_y), (4, 0));
        assert_eq!(cart.cargo_route, CargoRoute::CityCart);
        assert_eq!(cart.carried_amount, 6);
        assert_eq!(cart.cargo_fixed, 192);
        assert_eq!(cart.base_sprite, 496);
        assert_eq!(cart.source_animation_frame_speed_ms, 60);
        assert_eq!(cart.source_animation_frames_per_direction, 8);
        assert_eq!(states[0].reserved_storage, 0);
        assert_eq!(states[1].reserved_storage, 192);
    }

    #[test]
    fn city_cart_source_radius_excludes_a_higher_score_outside_the_event_window() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                source_transfer_radius: 2,
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut near = supplier_state(2, 0, 128, Good::Cloth);
        near.storage_animation_capacity = 320;
        let mut far = supplier_state(4, 0, 192, Good::Cloth);
        far.storage_animation_capacity = 320;
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 2,
                y: 0,
                good: Good::Cloth,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 32,
                source_footprint: (1, 1),
            },
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 4,
                y: 0,
                good: Good::Cloth,
                available: 6,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 32,
                source_footprint: (1, 1),
            },
        ];
        let mut states = [near, far];

        let cart = try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, [1; 25]),
            &suppliers,
            &mut states,
            &[IslandMap::new_open(0, 5, 1)],
            CityCartConfig::default(),
        )
        .expect("in-radius source root should be selected");

        assert_eq!((cart.target_x, cart.target_y), (2, 0));
        assert_eq!(states[0].reserved_storage, 128);
        assert_eq!(states[1].reserved_storage, 0);
    }

    /// `FUN_004706e0` (`1602_exe.c:79565-79570`) refuses the goal bit to any
    /// nested production kind above 6, so a MARKT or KONTOR is never a city
    /// cart's collection target — the store-to-store hop does not exist. The
    /// port used to test only `is_type11_transfer_root()` on its fallback
    /// path and nothing at all on the wave path, which let a Kontor sitting on
    /// a `Ware`-bearing terrain cell offer itself as its own supplier.
    #[test]
    fn city_cart_source_wave_never_collects_from_another_transfer_root() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut other_market = SourceMapCellState {
            storage_fill: 320,
            source_output_ware_slot: Good::Cloth.source_ware_slot().unwrap(),
            ..SourceMapCellState::new(
                0,
                2,
                0,
                &anno_formats::cod::BuildingDef {
                    kind: "GEBAEUDE".into(),
                    properties: [("ProdKind".into(), "MARKT".into())].into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        };
        other_market.storage_animation_capacity = 320;
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 2,
            y: 0,
            good: Good::Cloth,
            available: 10,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        };

        assert!(
            try_spawn_city_cart(
                origin,
                CityCartEligibility::from_priorities(0, [1; 25]),
                &[supplier],
                &mut [other_market],
                &[IslandMap::new_open(0, 3, 1)],
                CityCartConfig::default(),
            )
            .is_none(),
            "a kind-7 root must not be a city cart's supplier"
        );

        // The same root as a plain workshop is collected, so the rejection is
        // the production kind and not the geometry.
        let mut workshop = supplier_state(2, 0, 320, Good::Cloth);
        workshop.storage_animation_capacity = 320;
        assert!(try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, [1; 25]),
            &[supplier],
            &mut [workshop],
            &[IslandMap::new_open(0, 3, 1)],
            CityCartConfig::default(),
        )
        .is_some());
    }

    #[test]
    fn city_cart_source_wave_rejects_a_supplier_from_another_map_owner() {
        let mut origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        origin.source_map_owner_slot = 2;
        let mut source = supplier_state(2, 0, 192, Good::Cloth);
        source.storage_animation_capacity = 320;
        source.source_map_owner_slot = 3;
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 2,
            y: 0,
            good: Good::Cloth,
            available: 6,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        };

        assert!(try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, [1; 25]),
            &[supplier],
            &mut [source],
            &[IslandMap::new_open(0, 3, 1)],
            CityCartConfig::default(),
        )
        .is_none());
    }

    #[test]
    fn city_cart_source_wave_uses_map_owner_not_city_player_owner() {
        let mut origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        origin.source_map_owner_slot = 2;
        let mut source = supplier_state(2, 0, 192, Good::Cloth);
        source.storage_animation_capacity = 320;
        source.source_map_owner_slot = 2;
        let supplier = CarrierSupplier {
            island: 0,
            owner: 3,
            x: 2,
            y: 0,
            good: Good::Cloth,
            available: 6,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        };

        let cart = try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, [1; 25]),
            &[supplier],
            &mut [source],
            &[IslandMap::new_open(0, 3, 1)],
            CityCartConfig::default(),
        )
        .expect("matching source map owner supplies the city cart");

        assert_eq!(cart.owner, 0);
        assert_eq!((cart.target_x, cart.target_y), (2, 0));
    }

    #[test]
    fn city_cart_preserves_nonintegral_source_reservation() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut source = supplier_state(2, 0, 65, Good::Cloth);
        source.storage_animation_capacity = 320;
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 2,
            y: 0,
            good: Good::Cloth,
            available: 6,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        };
        let mut priorities = [0; 25];
        priorities[Good::Cloth.source_ware_slot().unwrap() as usize] = 1;
        let mut states = [source];

        let cart = try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, priorities),
            &[supplier],
            &mut states,
            &[IslandMap::new_open(0, 3, 1)],
            CityCartConfig::default(),
        )
        .expect("a consumer good accepts positive source stock");

        assert_eq!(cart.carried_amount, 2);
        assert_eq!(cart.cargo_fixed, 65);
        assert_eq!(states[0].reserved_storage, 65);
    }

    #[test]
    fn city_cart_rejects_a_good_with_no_city_capacity() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut source = supplier_state(2, 0, 192, Good::Cloth);
        source.storage_animation_capacity = 320;
        let suppliers = [CarrierSupplier {
            island: 0,
            owner: 0,
            x: 2,
            y: 0,
            good: Good::Cloth,
            available: 6,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 0,
            source_footprint: (1, 1),
        }];
        let mut warehouse = Warehouse::new(0, 0, 0, 0);
        warehouse.deposit(Good::Cloth, 50);

        assert!(try_spawn_city_cart(
            origin,
            CityCartEligibility::from_city_store(
                &warehouse,
                warehouse.city_storage_capacity_fixed(1),
            ),
            &suppliers,
            &mut [source],
            &[],
            CityCartConfig::default(),
        )
        .is_none());
    }

    #[test]
    fn city_cart_priority_two_overrides_an_earlier_ordinary_candidate() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut ordinary = supplier_state(2, 0, 192, Good::Bricks);
        ordinary.storage_animation_capacity = 320;
        let mut priority = supplier_state(4, 0, 256, Good::Cloth);
        priority.storage_animation_capacity = 320;
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 2,
                y: 0,
                good: Good::Bricks,
                available: 6,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 0,
                source_footprint: (1, 1),
            },
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 4,
                y: 0,
                good: Good::Cloth,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 0,
                source_footprint: (1, 1),
            },
        ];
        let warehouse = Warehouse::with_capacity_and_population(0, 0, 0, 0, 50, [0, 100, 0, 0, 0]);
        let city = CityCartEligibility::from_city_store(
            &warehouse,
            warehouse.city_storage_capacity_fixed(1),
        );

        let cart = try_spawn_city_cart(
            origin,
            city,
            &suppliers,
            &mut [ordinary, priority],
            &[],
            CityCartConfig::default(),
        )
        .expect("priority-two city good should be selected");

        assert_eq!((cart.target_x, cart.target_y), (4, 0));
        assert_eq!(cart.carried_good, Good::Cloth as u8);
    }

    #[test]
    fn city_cart_priority_two_uses_source_wave_order_not_source_record_order() {
        let origin = SourceMapCellState::new(
            0,
            2,
            2,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut west = supplier_state(1, 2, 128, Good::Cloth);
        west.storage_animation_capacity = 320;
        let mut east = supplier_state(3, 2, 128, Good::Alcohol);
        east.storage_animation_capacity = 320;
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 1,
                y: 2,
                good: Good::Cloth,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 32,
                source_footprint: (1, 1),
            },
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 3,
                y: 2,
                good: Good::Alcohol,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 32,
                source_footprint: (1, 1),
            },
        ];
        let mut priorities = [0; 25];
        priorities[Good::Cloth.source_ware_slot().unwrap() as usize] = 2;
        priorities[Good::Alcohol.source_ware_slot().unwrap() as usize] = 2;

        // Record order is east-then-west; the wave reaches west first and
        // `FUN_004717b0` breaks out of its band on the first priority-two hit,
        // so the wave order is what decides.
        let cart = try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, priorities),
            &suppliers,
            &mut [east, west],
            &[IslandMap::new_open(0, 5, 5)],
            CityCartConfig::default(),
        )
        .expect("source wave should reach the western root first");

        assert_eq!((cart.target_x, cart.target_y), (1, 2));
        assert_eq!(cart.carried_good, Good::Cloth as u8);
    }

    #[test]
    fn city_cart_uses_the_source_grid_route_after_selection() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut source = supplier_state(4, 0, 128, Good::Cloth);
        source.storage_animation_capacity = 320;
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 4,
            y: 0,
            good: Good::Cloth,
            available: 4,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (1, 1),
        };
        let mut priorities = [0; 25];
        priorities[Good::Cloth.source_ware_slot().unwrap() as usize] = 2;
        let mut map = IslandMap::new_open(0, 5, 1);
        map.set_walkable(2, 0, false);

        let cart = try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, priorities),
            &[supplier],
            &mut [source],
            &[map],
            CityCartConfig::default(),
        )
        .expect("source-grid route should not be replaced with A* walkability");

        assert_eq!(cart.path, vec![(1, 0), (2, 0), (3, 0), (4, 0)]);
    }

    #[test]
    fn city_cart_source_wave_accepts_a_non_anchor_oriented_footprint_cell() {
        let origin = SourceMapCellState::new(
            0,
            2,
            2,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut source = supplier_state(0, 1, 128, Good::Cloth);
        source.storage_animation_capacity = 320;
        let mut north = supplier_state(2, 0, 128, Good::Alcohol);
        north.storage_animation_capacity = 320;
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 0,
                y: 1,
                good: Good::Cloth,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 32,
                source_footprint: (2, 1),
            },
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 2,
                y: 0,
                good: Good::Alcohol,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 32,
                source_footprint: (1, 1),
            },
        ];
        let mut priorities = [0; 25];
        priorities[Good::Cloth.source_ware_slot().unwrap() as usize] = 2;
        priorities[Good::Alcohol.source_ware_slot().unwrap() as usize] = 2;

        let cart = try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, priorities),
            &suppliers,
            &mut [source, north],
            &[IslandMap::new_open(0, 5, 5)],
            CityCartConfig::default(),
        )
        .expect("source wave should select the nearer non-anchor footprint cell");

        assert_eq!((cart.target_x, cart.target_y), (1, 1));
        assert_eq!((cart.supplier_x, cart.supplier_y), (0, 1));
        assert_eq!(cart.carried_good, Good::Cloth as u8);
    }

    #[test]
    fn city_cart_source_wave_uses_the_last_root_to_write_an_overlapping_cell() {
        let origin = SourceMapCellState::new(
            0,
            2,
            2,
            &anno_formats::cod::BuildingDef {
                kind: "GEBAEUDE".into(),
                properties: [("ProdKind".into(), "MARKT".into())].into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut west = supplier_state(0, 1, 128, Good::Cloth);
        west.storage_animation_capacity = 320;
        let mut east = supplier_state(1, 1, 128, Good::Alcohol);
        east.storage_animation_capacity = 320;
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 0,
                y: 1,
                good: Good::Cloth,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 32,
                source_footprint: (2, 1),
            },
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 1,
                y: 1,
                good: Good::Alcohol,
                available: 4,
                storage: CarrierSupplierStorage::SourceRoot,
                source_path_class: 32,
                source_footprint: (1, 1),
            },
        ];
        let mut priorities = [0; 25];
        priorities[Good::Cloth.source_ware_slot().unwrap() as usize] = 2;
        priorities[Good::Alcohol.source_ware_slot().unwrap() as usize] = 2;

        let cart = try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, priorities),
            &suppliers,
            &mut [west, east],
            &[IslandMap::new_open(0, 5, 5)],
            CityCartConfig::default(),
        )
        .expect("last writer should own the shared source-grid target cell");

        assert_eq!((cart.target_x, cart.target_y), (1, 1));
        assert_eq!(cart.carried_good, Good::Alcohol as u8);
    }

    #[test]
    fn city_eligibility_uses_source_fixed_point_population_target() {
        let mut warehouse =
            Warehouse::with_capacity_and_population(0, 0, 0, 0, 50, [0, 100, 0, 0, 0]);
        let capacity = warehouse.city_storage_capacity_fixed(1);

        assert_eq!(
            city_demand_target(Good::Cloth, warehouse.city_population),
            18
        );
        let selector = CityCartEligibility::from_city_store(&warehouse, capacity);
        assert_eq!(selector.priority(Good::Cloth), 2);
        assert_eq!(selector.priority(Good::Iron), 1);

        warehouse.deposit(Good::Cloth, 3);
        let selector = CityCartEligibility::from_city_store(&warehouse, capacity);
        assert_eq!(selector.priority(Good::Cloth), 1);
    }

    #[test]
    fn city_capacity_adds_one_market_store_per_extra_transfer_root() {
        let warehouse = Warehouse::with_capacity(0, 0, 0, 0, 50);

        assert_eq!(warehouse.city_storage_capacity_fixed(1), 1_600);
        assert_eq!(warehouse.city_storage_capacity_fixed(3), 2_240);
    }
}
