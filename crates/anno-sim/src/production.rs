//! Production chain simulation.
//!
//! Ported from FUN_0047daf0 (building production/processing tick).
//! Timer: 999ms intervals.
//!
//! Production model:
//! - Each building has input/output goods and a cycle time
//! - Efficiency = min(input1_ratio, input2_ratio) on 0-128 scale
//! - Below 50% (64/128) efficiency, production halts
//! - Output accumulates and is dispatched via carriers

use crate::building::{BuildingDef, BuildingInstance};

/// Production tick interval in milliseconds.
pub const PRODUCTION_TICK_MS: u32 = 999;

/// Minimum efficiency threshold for production (50% = 64/128).
pub const MIN_EFFICIENCY: u8 = 64;

/// Update production for a single building.
/// Returns the amount of goods produced this tick (0 if not producing).
pub fn tick_building(building: &mut BuildingInstance, def: &BuildingDef, dt_ms: u32) -> u16 {
    if !building.active {
        return 0;
    }

    // Calculate efficiency from input stock levels
    building.efficiency = calculate_efficiency(building, def);

    if building.efficiency < MIN_EFFICIENCY {
        return 0;
    }

    // Advance production timer
    building.production_timer_ms += dt_ms;

    if building.production_timer_ms < def.cycle_time_ms as u32 {
        return 0;
    }

    // Production cycle complete
    building.production_timer_ms -= def.cycle_time_ms as u32;

    // Consume inputs
    if def.input_1_rate > 0 {
        building.input_1_stock = building.input_1_stock.saturating_sub(def.input_1_rate);
    }
    if def.input_2_rate > 0 {
        building.input_2_stock = building.input_2_stock.saturating_sub(def.input_2_rate);
    }

    // Produce output (capped at storage)
    let produced = def.output_rate;
    building.output_stock = (building.output_stock + produced).min(def.storage_capacity);
    building.total_work += 1;

    produced
}

/// Calculate production efficiency (0-128 scale).
///
/// Efficiency = min(input1_stock / input1_capacity, input2_stock / input2_capacity) * 128
/// If a building has no inputs, efficiency is always 128 (100%).
fn calculate_efficiency(building: &BuildingInstance, def: &BuildingDef) -> u8 {
    if def.input_1_rate == 0 && def.input_2_rate == 0 {
        return 128; // No inputs needed (raw resource)
    }

    let mut eff = 128u32;

    if def.input_1_rate > 0 && def.storage_capacity > 0 {
        let ratio = (building.input_1_stock as u32 * 128) / def.storage_capacity as u32;
        eff = eff.min(ratio);
    }

    if def.input_2_rate > 0 && def.storage_capacity > 0 {
        let ratio = (building.input_2_stock as u32 * 128) / def.storage_capacity as u32;
        eff = eff.min(ratio);
    }

    eff.min(128) as u8
}

/// Check if a building needs carrier dispatch (output buffer is getting full).
pub fn needs_carrier(building: &BuildingInstance, def: &BuildingDef) -> bool {
    if def.storage_capacity == 0 {
        return false;
    }
    // Dispatch when output exceeds half capacity
    building.output_stock > def.storage_capacity / 2
}
