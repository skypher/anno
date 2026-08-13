//! Player economy tick.
//!
//! Ported from the settlement economy tick `FUN_0047f8a0`
//! (`1602_exe.c:91272`+). Timer: 10 000 ms intervals (game-tick
//! aligned).
//!
//! Model:
//! - Per demand category, fulfillment = `(supply << 7) / demand`
//!   clamped at 0x80 (128).
//! - Per-tier satisfaction is **recomputed fresh** each tick from
//!   goods fulfillment (`1602_exe.c:91396`, stored at struct
//!   `0x248+tier`); it is NOT decayed. See `population.rs`.
//! - Gross tax income (`FUN_0047f740`) minus running costs
//!   (`FUN_0047f6b0`), applied as `(income - costs) / 6` to the
//!   player's gold (`1602_exe.c:91438-91440`).
//!
//! Note on the `×15/16` decay in the source (`1602_exe.c:91430`,
//! `*puVar9 = *puVar9 * 0xf >> 4`): it decays the goods
//! demand/supply *accumulators* at struct `0x150`/`0x154`, NOT
//! satisfaction. An earlier port misattributed that factor to
//! satisfaction and decayed happiness every tick — that invented
//! decay has been removed.

use crate::player::Player;

/// Economy tick interval in milliseconds. The population/economy
/// ticker `FUN_0047f8a0` accumulates elapsed ms into `DAT_0054a3b4`
/// and fires when `9999 < DAT_0054a3b4` (`1602_exe.c:91300`), i.e.
/// once the accumulator passes 9999 — an effective ~10 000 ms
/// period. (Note: this constant is currently informational; the
/// live tick cadence is driven by the caller's timer, not read from
/// here.)
pub const ECONOMY_TICK_MS: u32 = 10_000;

/// Update the player's economy for one tick.
///
/// Satisfaction is set fresh in `population::update_population_demands`
/// before this runs and must NOT be decayed here (see module docs).
pub fn tick_economy(player: &mut Player) {
    // 1. Record the latest demand fulfillment ratio per category.
    for slot in &mut player.demands {
        if slot.demand > 0 {
            let fulfillment = ((slot.supply as u64 * 128) / slot.demand as u64).min(128) as u8;

            // Shift history and add new sample
            slot.fulfillment_history[3] = slot.fulfillment_history[2];
            slot.fulfillment_history[2] = slot.fulfillment_history[1];
            slot.fulfillment_history[1] = slot.fulfillment_history[0];
            slot.fulfillment_history[0] = fulfillment;
        }
    }

    // 2. Apply economy balance: (income - costs) / 6 to gold
    //    (`1602_exe.c:91438-91440`).
    let balance = player.net_balance();
    player.gold += balance;

    // 3. Track bankruptcy
    if player.is_bankrupt() {
        player.bankruptcy_ticks += 1;
    } else {
        player.bankruptcy_ticks = 0;
    }

    // 4. Update total population
    player.total_population = player.total_population();
}
