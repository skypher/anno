//! Per-good buy / sell prices, in gold per unit.
//!
//! Numbers are loosely modeled on the original Anno 1602 economy: raw
//! materials are cheap, refined goods are expensive, exotic plantation
//! goods (silk, spices, jewelry) are top-tier. Sell prices are roughly
//! 55-60% of buy prices, leaving room for profit on long routes while
//! still rewarding production over pure trading.

use crate::types::Good;

#[derive(Debug, Clone, Copy)]
pub struct GoodPrice {
    /// Gold the warehouse charges to sell to a ship's cargo.
    pub buy: i32,
    /// Gold the warehouse pays when a ship deposits to it.
    pub sell: i32,
}

/// Default buy/sell prices for any good not in the table below.
pub const DEFAULT_PRICE: GoodPrice = GoodPrice { buy: 6, sell: 4 };

/// Look up the (buy, sell) price for a good.
pub fn price_of(good: Good) -> GoodPrice {
    use Good::*;
    match good {
        None => DEFAULT_PRICE,
        // Raw materials
        Wood    => GoodPrice { buy: 4,  sell: 2  },
        Stone   => GoodPrice { buy: 5,  sell: 3  },
        Ore     => GoodPrice { buy: 6,  sell: 3  },
        GoldOre => GoodPrice { buy: 25, sell: 14 },
        Iron    => GoodPrice { buy: 8,  sell: 5  },
        Gold    => GoodPrice { buy: 50, sell: 28 },
        // Animals / plantation raw goods
        Wool    => GoodPrice { buy: 6,  sell: 3  },
        Cotton  => GoodPrice { buy: 7,  sell: 4  },
        WildGame   => GoodPrice { buy: 7,  sell: 4  },
        Cattle  => GoodPrice { buy: 5,  sell: 3  },
        Grain   => GoodPrice { buy: 5,  sell: 3  },
        Sugar     => GoodPrice { buy: 9,  sell: 5  },
        SugarCane => GoodPrice { buy: 6,  sell: 3  },
        Meat      => GoodPrice { buy: 8,  sell: 5  },
        Tobacco => GoodPrice { buy: 24, sell: 13 },
        Cocoa   => GoodPrice { buy: 30, sell: 18 },
        Spices  => GoodPrice { buy: 30, sell: 18 },
        Silk    => GoodPrice { buy: 35, sell: 20 },
        Grapes  => GoodPrice { buy: 8,  sell: 5  },
        Fish    => GoodPrice { buy: 5,  sell: 3  },
        // Processed
        Flour     => GoodPrice { buy: 8,  sell: 4  },
        Food      => GoodPrice { buy: 12, sell: 7  },
        Cloth     => GoodPrice { buy: 12, sell: 7  },
        Clothing  => GoodPrice { buy: 25, sell: 14 },
        Alcohol   => GoodPrice { buy: 18, sell: 10 },
        TobaccoProducts => GoodPrice { buy: 30, sell: 18 },
        Bricks    => GoodPrice { buy: 6,  sell: 3  },
        Tools     => GoodPrice { buy: 18, sell: 10 },
        Jewelry   => GoodPrice { buy: 50, sell: 28 },
        // Weapons
        Swords    => GoodPrice { buy: 40, sell: 22 },
        Muskets   => GoodPrice { buy: 50, sell: 28 },
        Cannons   => GoodPrice { buy: 70, sell: 40 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jewelry_more_expensive_than_wood() {
        assert!(price_of(Good::Jewelry).buy > price_of(Good::Wood).buy * 5);
    }

    #[test]
    fn sell_under_buy_for_profit_margin() {
        for g in [
            Good::Wood, Good::Iron, Good::Tools, Good::Food,
            Good::Cloth, Good::Tobacco, Good::Spices, Good::Jewelry,
        ] {
            let p = price_of(g);
            assert!(p.sell < p.buy, "sell {} should be < buy {} for {:?}", p.sell, p.buy, g);
            // And not absurdly low (margin sanity).
            assert!(p.sell * 2 >= p.buy, "margin too steep on {:?}", g);
        }
    }

    #[test]
    fn none_uses_default() {
        let p = price_of(Good::None);
        assert_eq!(p.buy, DEFAULT_PRICE.buy);
        assert_eq!(p.sell, DEFAULT_PRICE.sell);
    }
}
