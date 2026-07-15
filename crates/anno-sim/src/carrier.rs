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
use crate::source_cell::SourceMapCellState;
use crate::source_route::{
    SourcePathBlockedCellDecision, SourcePathTargetRect, source_route_positions,
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
    (((speed as u32).saturating_mul(32) / 100).min(126)) as u8
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
    if duration == 0 { 1 } else { duration }
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

/// Source-derived cart capacity for the type-11 MARKT/KONTOR/HAUPT transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CityCartConfig {
    /// Maximum goods moved by one KARREN trip.
    pub max_load: u16,
    /// Authored KARREN `Speed` used by source-grid traversal.
    pub movement_speed: u16,
    /// Resolved empty-walk KARREN sprite base in its BSH file.
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
/// starts a generic carrier below two production batches, which corresponds to
/// its 256/128 stock-ratio threshold.
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
        (stock < target).then_some((good, stock, batch, target))
    })
    .min_by_key(|(_, stock, batch, _)| (u32::from(*stock) << 16) / u32::from(*batch))
    .map(|(good, stock, _, target)| CarrierRequest {
        good,
        amount: target.saturating_sub(stock).min(max_load).max(1),
    })
}

/// Run FUN_00471380's first-reservable type-8 supplier search. The callback
/// accepts the first same-owner root whose requested input has at least half
/// a TRAEGER load available, reserving up to the full fixed-point load.
fn select_carrier_source_wave(
    start: (i32, i32),
    island: u8,
    owner: u8,
    requested_good: Good,
    suppliers: &[CarrierSupplier],
    source_cells: &[SourceMapCellState],
    warehouses: &[Warehouse],
    map: &IslandMap,
    max_load: u16,
) -> Option<(usize, CarrierSupplier, (i32, i32), Vec<(i32, i32)>, u16)> {
    let max_fixed = max_load.checked_mul(SOURCE_STORAGE_UNIT)?;
    let minimum_fixed = max_fixed / 2;
    let mut grid = map.carrier_path_grid();

    for candidate in source_cells.iter().copied() {
        if candidate.island != island || !matches!(candidate.kind_code, 1..=8) {
            continue;
        }
        let Some(supplier) = suppliers.iter().copied().find(|supplier| {
            supplier.island == candidate.island
                && supplier.owner == owner
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

    let mut selected: Option<(usize, CarrierSupplier, (i32, i32), u16)> = None;
    let result = grid
        .search_with_blocked_cell_callback(start, |position, _| {
            let Some((state_idx, candidate, supplier)) =
                source_cells.iter().copied().enumerate().rev().find_map(
                    |(state_idx, candidate)| {
                        let supplier = suppliers.iter().copied().find(|supplier| {
                            supplier.island == candidate.island
                                && supplier.owner == owner
                                && supplier.x == u16::from(candidate.x)
                                && supplier.y == u16::from(candidate.y)
                        })?;
                        let within_footprint = candidate.island == island
                            && matches!(candidate.kind_code, 1..=8)
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
    let start = (i32::from(building.tile_x), i32::from(building.tile_y));
    let map = island_maps
        .iter()
        .find(|map| map.island_id == building.island_id);

    let mut selected: Option<(usize, CarrierSupplier, (i32, i32), Vec<(i32, i32)>, u16)> = None;
    if let Some(map) = map {
        let (state_idx, supplier, reached, path, cargo_fixed) = select_carrier_source_wave(
            start,
            building.island_id,
            building.owner,
            request.good,
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
        let valid_storage = match supplier.storage {
            CarrierSupplierStorage::SourceRoot => {
                !matches!(cell.kind_code, 7 | 8)
                    && cell.storage_fill.saturating_sub(cell.reserved_storage)
                        >= request.amount.saturating_mul(SOURCE_STORAGE_UNIT)
            }
            CarrierSupplierStorage::Warehouse(warehouse_idx) => {
                matches!(cell.kind_code, 7 | 8)
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
        CarrierSupplierStorage::Warehouse(warehouse_idx) => warehouses
            .get_mut(warehouse_idx)
            .is_some_and(|warehouse| {
                if !warehouse.reserve(request.good, cargo) {
                    return false;
                }
                if warehouse.reserve_city_good_fixed(request.good, cargo_fixed) {
                    return true;
                }
                warehouse.release_reservation(request.good, cargo);
                false
            }),
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
    carrier.destination_kind = supplier.kind_code;
    carrier.supplier_x = supplier.x;
    carrier.supplier_y = supplier.y;
    carrier.carried_good = request.good as u8;
    carrier.carried_amount = cargo;
    carrier.cargo_fixed = cargo_fixed;
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

/// Start a type-11 city cart from a MARKT, KONTOR, or HAUPT source root.
/// `FUN_004717b0` retains the strictly highest `storage_fill × 128 /
/// Maxlager` candidate, accepts consumer goods 11 through 24 at any positive
/// score, and otherwise requires a score above 127. The caller supplies only
/// producers that already passed the decoded city-good eligibility table.
fn select_city_cart_source_wave(
    start: (i32, i32),
    island: u8,
    city: CityCartEligibility,
    suppliers: &[CarrierSupplier],
    source_cells: &[SourceMapCellState],
    map: &IslandMap,
) -> Option<(usize, CarrierSupplier, (i32, i32), Vec<(i32, i32)>, u32)> {
    let mut grid = map.city_cart_path_grid();

    for candidate in source_cells.iter().copied() {
        if candidate.island != island || !matches!(candidate.kind_code, 1..=6) {
            continue;
        }
        let Some(supplier) = suppliers.iter().copied().find(|supplier| {
            supplier.storage == CarrierSupplierStorage::SourceRoot
                && supplier.island == candidate.island
                && supplier.owner == city.owner
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
                        && supplier.owner == city.owner
                        && supplier.x == u16::from(candidate.x)
                        && supplier.y == u16::from(candidate.y)
                })?;
                let within_footprint = candidate.island == island
                    && matches!(candidate.kind_code, 1..=6)
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
            .then_some((state_idx, supplier, reached, path, score))
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
    if !matches!(origin.kind_code, 7 | 8 | 30) {
        return None;
    }

    let start = (i32::from(origin.x), i32::from(origin.y));
    let map = island_maps
        .iter()
        .find(|map| map.island_id == origin.island);
    let mut selected: Option<(usize, CarrierSupplier, (i32, i32), Vec<(i32, i32)>, u32)> = None;

    if let Some(map) = map {
        let (state_idx, supplier, reached, path, score) =
            select_city_cart_source_wave(start, origin.island, city, suppliers, source_cells, map)?;
        selected = Some((state_idx, supplier, reached, path, score));
    }

    for (state_idx, candidate) in source_cells.iter().copied().enumerate() {
        if map.is_some() {
            break;
        }
        if candidate.island != origin.island || matches!(candidate.kind_code, 7 | 8 | 30) {
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
            .is_some_and(|(_, _, _, _, best_score)| score <= *best_score)
        {
            continue;
        }
        let goal = (i32::from(supplier.x), i32::from(supplier.y));
        let path = direct_path(start, goal);
        if priority == 2 {
            selected = Some((state_idx, *supplier, goal, path, score));
            break;
        }
        selected = Some((state_idx, *supplier, goal, path, score));
    }

    let (state_idx, supplier, reached, path, _) = selected?;
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
    cart.destination_kind = supplier.kind_code;
    cart.supplier_x = supplier.x;
    cart.supplier_y = supplier.y;
    cart.origin_island = origin.island;
    cart.origin_x = u16::from(origin.x);
    cart.origin_y = u16::from(origin.y);
    cart.origin_kind = origin.kind_code;
    cart.carried_good = supplier.good as u8;
    cart.carried_amount = cargo;
    cart.cargo_fixed = cargo_fixed;
    cart.speed = CARRIER_SPEED;
    cart.source_move_speed = config.movement_speed;
    cart.base_sprite = config.sprite_base;
    cart.initialize_source_position();
    cart.source_position_z = map
        .and_then(|map| map.source_terrain_height(start))
        .unwrap_or(0.0);
    cart.path = path;
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
                let path = match island_maps
                    .iter()
                    .find(|map| map.island_id == figure.origin_island)
                {
                    Some(map) => match source_carrier_return_path(map, start, goal, CargoRoute::CityCart) {
                        Some(path) => path,
                        None => return true,
                    },
                    None => direct_path(start, goal),
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
        };
    }

    match figure.action {
        ActionType::CarryingGoods => {
            let Some(building) = buildings.get(figure.building_idx as usize) else {
                return true;
            };
            let start = (figure.tile_x, figure.tile_y);
            let goal = (i32::from(building.tile_x), i32::from(building.tile_y));
            let path = match island_maps
                .iter()
                .find(|map| map.island_id == building.island_id)
            {
                Some(map) => match source_carrier_return_path(
                    map,
                    start,
                    goal,
                    CargoRoute::InputCarrier,
                ) {
                    Some(path) => path,
                    None => return true,
                },
                None => direct_path(start, goal),
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
) -> Option<Vec<(i32, i32)>> {
    let mut grid = match cargo_route {
        CargoRoute::InputCarrier => map.carrier_path_grid(),
        CargoRoute::CityCart => map.city_cart_path_grid(),
    };
    let steps = grid.route_to(start, goal).ok()?;
    let path = source_route_positions(start, &steps)?;
    (path.last().copied() == Some(goal) || (start == goal && path.is_empty())).then_some(path)
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

    fn supplier_state(x: u8, y: u8, fill: u16) -> SourceMapCellState {
        SourceMapCellState {
            storage_fill: fill,
            ..SourceMapCellState::new(
                0,
                x,
                y,
                &anno_formats::cod::BuildingDef {
                    kind: "HANDWERK".into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap()
        }
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
        let mut states = vec![supplier_state(4, 4, 128), supplier_state(20, 20, 128)];

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
        let mut states = vec![supplier_state(4, 4, 128), supplier_state(8, 5, 128)];

        assert!(
            try_spawn_carrier(
                &consumer,
                &consumer_def(),
                &suppliers,
                &mut states,
                &mut [],
                &[],
                CarrierConfig::default(),
            )
            .is_none()
        );
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
        let mut states = [supplier_state(4, 0, 96)];

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
        let mut states = [supplier_state(4, 0, 65)];

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
        let mut states = [supplier_state(4, 0, 32)];

        assert!(
            try_spawn_carrier(
                &consumer,
                &consumer_def(),
                &[supplier],
                &mut states,
                &mut [],
                &[IslandMap::new_open(0, 5, 1)],
                CarrierConfig::default(),
            )
            .is_none()
        );
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
        let mut states = [supplier_state(3, 0, 128)];
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
        let supplier = CarrierSupplier {
            island: 0,
            owner: 0,
            x: 1,
            y: 1,
            good: Good::Iron,
            available: 4,
            storage: CarrierSupplierStorage::SourceRoot,
            source_path_class: 32,
            source_footprint: (2, 1),
        };
        let mut states = [supplier_state(1, 1, 128)];

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

        assert_eq!((carrier.target_x, carrier.target_y), (2, 1));
        assert_eq!((carrier.supplier_x, carrier.supplier_y), (1, 1));
        assert_eq!(carrier.path.last(), Some(&(2, 1)));
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
                    kind: "KONTOR".into(),
                    ..Default::default()
                },
                0,
            )
            .unwrap(),
        ];

        let mut warehouses = [Warehouse::new(0, 0, 4, 4)];
        warehouses[0].deposit(Good::Iron, 4);
        assert!(
            try_spawn_carrier(
                &consumer,
                &consumer_def(),
                &suppliers,
                &mut states,
                &mut warehouses,
                &[],
                CarrierConfig::default(),
            )
            .is_some()
        );
        assert_eq!(states[0].storage_fill, 0);
        assert_eq!(states[0].reserved_storage, 0);
        assert_eq!(warehouses[0].reserved(Good::Iron), 4);
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
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut lower = supplier_state(2, 0, 128);
        lower.storage_animation_capacity = 320;
        let mut higher = supplier_state(4, 0, 192);
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
        assert_eq!(cart.base_sprite, 496);
        assert_eq!(states[0].reserved_storage, 0);
        assert_eq!(states[1].reserved_storage, 192);
    }

    #[test]
    fn city_cart_preserves_nonintegral_source_reservation() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut source = supplier_state(2, 0, 65);
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
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut source = supplier_state(2, 0, 192);
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

        assert!(
            try_spawn_city_cart(
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
            .is_none()
        );
    }

    #[test]
    fn city_cart_priority_two_overrides_an_earlier_ordinary_candidate() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut ordinary = supplier_state(2, 0, 192);
        ordinary.storage_animation_capacity = 320;
        let mut priority = supplier_state(4, 0, 256);
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
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut west = supplier_state(1, 2, 128);
        west.storage_animation_capacity = 320;
        let mut east = supplier_state(3, 2, 128);
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

        let cart = try_spawn_city_cart(
            origin,
            CityCartEligibility::from_priorities(0, priorities),
            &suppliers,
            &mut [west, east],
            &[IslandMap::new_open(0, 5, 5)],
            CityCartConfig::default(),
        )
        .expect("source wave should reach the eastern root first");

        assert_eq!((cart.target_x, cart.target_y), (3, 2));
        assert_eq!(cart.carried_good, Good::Alcohol as u8);
    }

    #[test]
    fn city_cart_uses_the_source_grid_route_after_selection() {
        let origin = SourceMapCellState::new(
            0,
            0,
            0,
            &anno_formats::cod::BuildingDef {
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut source = supplier_state(4, 0, 128);
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
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut source = supplier_state(1, 1, 128);
        source.storage_animation_capacity = 320;
        let mut north = supplier_state(2, 0, 128);
        north.storage_animation_capacity = 320;
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 1,
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

        assert_eq!((cart.target_x, cart.target_y), (2, 1));
        assert_eq!((cart.supplier_x, cart.supplier_y), (1, 1));
        assert_eq!(cart.carried_good, Good::Cloth as u8);
    }

    #[test]
    fn city_cart_source_wave_uses_the_last_root_to_write_an_overlapping_cell() {
        let origin = SourceMapCellState::new(
            0,
            2,
            2,
            &anno_formats::cod::BuildingDef {
                kind: "MARKT".into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut west = supplier_state(1, 1, 128);
        west.storage_animation_capacity = 320;
        let mut east = supplier_state(2, 1, 128);
        east.storage_animation_capacity = 320;
        let suppliers = [
            CarrierSupplier {
                island: 0,
                owner: 0,
                x: 1,
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

        assert_eq!((cart.target_x, cart.target_y), (2, 1));
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
