//! Per-good buy / sell price bounds, in gold per unit.
//!
//! The original warehouse price UI clamps each `text.cod [WARE]` entry
//! between a floor and a ceiling. The ceiling table is `DAT_0049ae50`
//! in `1602.exe`; the floor is computed by the UI as
//! `ceiling * 300 / 1024` (`1602_exe.c:43251`, `43294`, `43307`).

use crate::types::Good;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoodPrice {
    /// Ceiling: gold the warehouse may charge to sell to a ship's cargo.
    pub buy: i32,
    /// Floor: gold the warehouse may pay when a ship deposits to it.
    pub sell: i32,
}

/// No ware / wildcard sentinel.
pub const NO_PRICE: GoodPrice = GoodPrice { buy: 0, sell: 0 };

/// Fallback for local intermediate goods that do not have a player-facing
/// `text.cod [WARE]` price slot.
pub const DEFAULT_PRICE: GoodPrice = GoodPrice { buy: 6, sell: 4 };

/// `DAT_0049ae50` from `extracted/1602.exe`, indexed by original
/// `text.cod [WARE]` id. Entries 0 and 1 are sentinels ("empty",
/// "each product"); 2..=24 are the player-facing warehouse goods.
pub const ORIGINAL_PRICE_CEILING_BY_WARE_ID: [i32; 25] = [
    0,   // empty
    0,   // each product
    90,  // iron ore
    700, // gold
    24,  // wool
    30,  // sugar
    35,  // tobacco
    10,  // cattle
    5,   // grain
    10,  // flour
    130, // iron
    180, // swords
    250, // muskets
    500, // cannon
    26,  // food
    100, // tobacco products
    60,  // spices
    50,  // cocoa
    80,  // liquor
    50,  // cloth
    200, // clothes
    900, // jewelry
    140, // tools
    30,  // wood
    45,  // bricks
];

/// Look up the (buy, sell) price for a good.
pub fn price_of(good: Good) -> GoodPrice {
    match good {
        Good::None => NO_PRICE,
        _ => original_ware_id(good)
            .map(source_price_for_ware_id)
            .unwrap_or_else(|| intermediate_fallback_price(good)),
    }
}

fn source_price_for_ware_id(ware_id: usize) -> GoodPrice {
    let buy = ORIGINAL_PRICE_CEILING_BY_WARE_ID[ware_id];
    GoodPrice {
        buy,
        sell: original_slider_floor(buy),
    }
}

fn original_slider_floor(ceiling: i32) -> i32 {
    ceiling * 300 / 1024
}

pub fn original_ware_id(good: Good) -> Option<usize> {
    use Good::*;
    Some(match good {
        Ore => 2,
        Gold => 3,
        Wool => 4,
        Sugar => 5,
        Tobacco => 6,
        Cattle => 7,
        Grain => 8,
        Flour => 9,
        Iron => 10,
        Swords => 11,
        Muskets => 12,
        Cannons => 13,
        Food => 14,
        TobaccoProducts => 15,
        Spices => 16,
        Cocoa => 17,
        Alcohol => 18,
        Cloth => 19,
        Clothing => 20,
        Jewelry => 21,
        Tools => 22,
        Wood => 23,
        Bricks => 24,
        _ => return std::option::Option::None,
    })
}

fn intermediate_fallback_price(good: Good) -> GoodPrice {
    use Good::*;
    match good {
        Stone => GoodPrice { buy: 5, sell: 3 },
        Cotton => GoodPrice { buy: 7, sell: 4 },
        WildGame => GoodPrice { buy: 7, sell: 4 },
        SugarCane => GoodPrice { buy: 6, sell: 3 },
        Meat => GoodPrice { buy: 8, sell: 5 },
        Silk => GoodPrice { buy: 35, sell: 20 },
        Grapes => GoodPrice { buy: 8, sell: 5 },
        Fish => GoodPrice { buy: 5, sell: 3 },
        _ => DEFAULT_PRICE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ware_price_ceiling_comes_from_original_table() {
        assert_eq!(price_of(Good::Ore), GoodPrice { buy: 90, sell: 26 });
        assert_eq!(
            price_of(Good::Gold),
            GoodPrice {
                buy: 700,
                sell: 205
            }
        );
        assert_eq!(price_of(Good::Tools), GoodPrice { buy: 140, sell: 41 });
        assert_eq!(price_of(Good::Wood), GoodPrice { buy: 30, sell: 8 });
        assert_eq!(price_of(Good::Bricks), GoodPrice { buy: 45, sell: 13 });
    }

    #[test]
    fn local_good_ids_are_not_used_as_ware_ids() {
        assert_eq!(Good::Tools as usize, 10);
        assert_eq!(original_ware_id(Good::Tools), Some(22));
        assert_eq!(price_of(Good::Tools).buy, 140);
        assert_ne!(
            price_of(Good::Tools).buy,
            ORIGINAL_PRICE_CEILING_BY_WARE_ID[Good::Tools as usize]
        );
    }

    #[test]
    fn original_ware_id_is_only_source_price_domain() {
        assert_eq!(original_ware_id(Good::Ore), Some(2));
        assert_eq!(original_ware_id(Good::Bricks), Some(24));
        assert_eq!(original_ware_id(Good::Silk), None);
        assert_eq!(original_ware_id(Good::Fish), None);
        assert_eq!(original_ware_id(Good::SugarCane), None);
    }

    #[test]
    fn source_floor_stays_below_ceiling() {
        for g in [
            Good::Wood,
            Good::Iron,
            Good::Tools,
            Good::Food,
            Good::Cloth,
            Good::Tobacco,
            Good::Spices,
            Good::Jewelry,
        ] {
            let p = price_of(g);
            assert!(
                p.sell < p.buy,
                "sell {} should be < buy {} for {:?}",
                p.sell,
                p.buy,
                g
            );
            assert_eq!(p.sell, p.buy * 300 / 1024);
        }
    }

    #[test]
    fn none_has_no_price() {
        let p = price_of(Good::None);
        assert_eq!(p, NO_PRICE);
    }

    #[test]
    fn non_ware_intermediates_keep_local_fallbacks() {
        assert_eq!(price_of(Good::Stone), GoodPrice { buy: 5, sell: 3 });
        assert_eq!(price_of(Good::SugarCane), GoodPrice { buy: 6, sell: 3 });
        assert_eq!(price_of(Good::Fish), GoodPrice { buy: 5, sell: 3 });
    }
}
