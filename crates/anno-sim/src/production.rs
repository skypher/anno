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
    if !building.active || !building.is_built() {
        return 0;
    }
    // Maxenergy cap: once cumulative work hits the per-building
    // limit (RE: haeuser.cod `Maxenergy`), the building stops
    // producing — analogous to needing repair / overhaul. 0 means
    // uncapped.
    if def.max_energy > 0 && building.total_work >= def.max_energy {
        return 0;
    }
    // Ore-deposit depletion: when a mine's remaining-ore counter
    // is 0, production stops permanently. RE: haeuser.cod
    // `Erzbergnr` + Tim Howgego's resources appendix (small 80t /
    // large 240t). u16::MAX = uncapped (non-mine buildings).
    if def.ore_deposit != crate::building::OreDeposit::None
        && building.remaining_ore == 0
    {
        return 0;
    }

    // Calculate efficiency from input stock levels
    building.efficiency = calculate_efficiency(building, def);

    if building.efficiency < MIN_EFFICIENCY {
        // Track consecutive idle ticks. Once we've been short of
        // input materials for `max_no_input_ticks` cycles
        // (haeuser.cod `Maxnorohst`, default 6), keep the
        // production timer reset so a sudden refill doesn't pop
        // out a finished cycle immediately — the building has to
        // genuinely "warm back up".
        building.idle_ticks = building.idle_ticks.saturating_add(1);
        if building.idle_ticks as u8 >= def.max_no_input_ticks {
            building.production_timer_ms = 0;
        }
        // Doerrflg drought: if a plantation tile (Doerrflg=1)
        // stays idle for 3× the no-input window, the field dries
        // up entirely — deactivate the building so it stops
        // ticking. Player has to bulldoze and replace.
        if def.can_dry_up
            && building.idle_ticks as u32
                >= def.max_no_input_ticks as u32 * 3
        {
            building.active = false;
        }
        return 0;
    }
    building.idle_ticks = 0;

    // Advance production timer
    building.production_timer_ms += dt_ms;

    if building.production_timer_ms < def.cycle_time_ms {
        return 0;
    }

    // Production cycle complete
    building.production_timer_ms -= def.cycle_time_ms;

    // Consume inputs
    if def.input_1_rate > 0 {
        building.input_1_stock = building.input_1_stock.saturating_sub(def.input_1_rate);
    }
    if def.input_2_rate > 0 {
        building.input_2_stock = building.input_2_stock.saturating_sub(def.input_2_rate);
    }

    // Produce output (capped at storage). Mines also debit the
    // finite ore deposit; once exhausted further ticks short-
    // circuit at the top of this function.
    let produced = def.output_rate;
    building.output_stock = (building.output_stock + produced).min(def.storage_capacity);
    building.total_work += 1;
    if def.ore_deposit != crate::building::OreDeposit::None {
        building.remaining_ore = building.remaining_ore.saturating_sub(produced);
    }

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
    if !building.is_built() {
        return false;
    }
    // Dispatch when output exceeds half capacity
    building.output_stock > def.storage_capacity / 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Good, ProductionType};

    fn test_def() -> BuildingDef {
        BuildingDef {
            id: 1, category: 0, width: 1, height: 1,
            production_type: ProductionType::Craft,
            kind: "GEBAEUDE".into(), prod_kind: "HANDWERK".into(),
            radius: 0,
            output_good: Good::Tools, input_good_1: Good::Iron,
            input_good_2: Good::None,
            output_rate: 1, input_1_rate: 1, input_2_rate: 0,
            storage_capacity: 50, cycle_time_ms: 1000, carrier_interval_ms: 0,
            cost_gold: 100, cost_tools: 0, cost_wood: 0, cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            min_tier: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: crate::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
        }
    }

    #[test]
    fn unfinished_building_does_not_produce() {
        let def = test_def();
        let mut b = BuildingInstance::new(1, 0, 0, 0, 0);
        b.input_1_stock = 50; // full input
        b.construction_ms_remaining = 5_000;
        b.construction_ms_total = 5_000;
        let produced = tick_building(&mut b, &def, 1_500);
        assert_eq!(produced, 0);
        assert!(!b.is_built());
    }

    #[test]
    fn finished_building_produces() {
        let def = test_def();
        let mut b = BuildingInstance::new(1, 0, 0, 0, 0);
        b.input_1_stock = 50;
        // Already built (defaults)
        let produced = tick_building(&mut b, &def, 2_000);
        assert!(produced > 0);
    }

    #[test]
    fn plantation_drought_deactivates_after_prolonged_idle() {
        let mut def = test_def();
        def.can_dry_up = true;
        def.max_no_input_ticks = 4;       // dries up after 4*3=12 ticks
        let mut b = BuildingInstance::new(1, 0, 0, 0, 0);
        // No input stock → efficiency below MIN, idle_ticks grows.
        b.input_1_stock = 0;
        for _ in 0..15 {
            tick_building(&mut b, &def, 200);
        }
        assert!(!b.active, "plantation should have dried up");
    }

    #[test]
    fn plantation_does_not_dry_when_flag_off() {
        let mut def = test_def();
        def.can_dry_up = false;          // not a Doerrflg plantation
        def.max_no_input_ticks = 4;
        let mut b = BuildingInstance::new(1, 0, 0, 0, 0);
        b.input_1_stock = 0;
        for _ in 0..15 {
            tick_building(&mut b, &def, 200);
        }
        assert!(b.active, "non-Doerrflg building shouldn't dry up");
    }

    #[test]
    fn mine_stops_producing_when_deposit_exhausted() {
        use crate::building::OreDeposit;
        let mut def = test_def();
        // Make this an ore-input-free mine.
        def.input_good_1 = Good::None;
        def.input_1_rate = 0;
        def.output_good = Good::Ore;
        def.ore_deposit = OreDeposit::Small;  // 80 tons
        def.output_rate = 5;
        def.storage_capacity = 200;
        def.cycle_time_ms = 100;
        let mut b = BuildingInstance::new(1, 0, 0, 0, 0);
        b.remaining_ore = OreDeposit::Small.capacity();  // 80
        // Run cycles until the deposit is exhausted.
        let mut total = 0u32;
        for _ in 0..100 {
            let p = tick_building(&mut b, &def, 200);
            total += p as u32;
            if b.remaining_ore == 0 { break; }
        }
        // Total produced should equal the deposit size (80).
        assert_eq!(total, 80);
        // Further ticks produce nothing.
        let p = tick_building(&mut b, &def, 200);
        assert_eq!(p, 0);
    }

    #[test]
    fn construction_progress_scales_to_128() {
        let mut b = BuildingInstance::new(1, 0, 0, 0, 0);
        b.construction_ms_total = 4_000;
        b.construction_ms_remaining = 4_000;
        assert_eq!(b.construction_progress_128(), 0);
        b.construction_ms_remaining = 2_000;
        assert_eq!(b.construction_progress_128(), 64);
        b.construction_ms_remaining = 0;
        assert_eq!(b.construction_progress_128(), 128);
    }
}
