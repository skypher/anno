//! Native village trade.
//!
//! Manual sec. 7.5 + 8.6: tropical islands can host indigenous
//! villages with a chief's hut. The player can barter goods at the
//! hut — the natives accept a fixed list of wanted goods (typically
//! finished crafted items: cloth, tools, jewellery) and offer a
//! fixed list of native goods (typically spices, cocoa, tobacco
//! plants).
//!
//! The exchange is value-balanced: the player delivers wanted goods
//! to build up "trade credit" at that village, then spends that
//! credit on offered goods. We use `prices::price_of` to compute
//! both sides' valuation.
//!
//! RE references:
//! - `haeuser.cod` `Kind: HQ` + `prod_kind: KONTOR` + `Nativflg: 1`:
//!   the chief's hut building. `IDNEGER` constant marks the
//!   native-village building IDs.
//! - `figuren.cod` `Nummer: TRAEGER2` + `Gfx: GFXEINGEB`: the
//!   native carrier figure (used to render villagers walking out
//!   of the hut).
//! - Player slot 5 is the native faction (`free_trader::NATIVE_SLOT`).
//!   Slot 4 holds the free trader, not the natives — see the
//!   PLAYER4 starting-gold table in `crates/anno-formats/src/szs.rs`.

use crate::types::Good;

/// One native village placed by the scenario. Persisted as part of
/// the save state so the player's accumulated trade credit at each
/// village survives load.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NativeVillage {
    /// Tile coordinates of the chief's hut on its host island.
    pub island_id: u8,
    pub hut_tile_x: u16,
    pub hut_tile_y: u16,
    /// Goods the natives accept in exchange (typically `Cloth`,
    /// `Tools`, `Jewelry`). Player delivers these to build credit.
    pub wants: Vec<Good>,
    /// Goods the natives offer in return (typically `Spices`,
    /// `Cocoa`, `Tobacco`). Player spends credit on these.
    pub offers: Vec<Good>,
    /// Per-player accumulated barter credit, in standard-price gold
    /// units. Indexed by player slot. Reset when the village is
    /// destroyed.
    #[serde(default)]
    pub credit: [i32; 7],
}

/// Outcome of a native barter operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarterOutcome {
    Delivered,
    Withdrawn,
    NotWanted,
    NotOffered,
    NotEnoughGoods,
    NotEnoughCredit,
    NoVillage,
}

impl NativeVillage {
    pub fn new(island_id: u8, hut_tile_x: u16, hut_tile_y: u16) -> Self {
        Self {
            island_id,
            hut_tile_x,
            hut_tile_y,
            wants: vec![Good::Cloth, Good::Tools, Good::Jewelry],
            offers: vec![Good::Spices, Good::Cocoa, Good::Tobacco],
            credit: [0; 7],
        }
    }

    /// Player delivers `qty` of `good` to this village. Adds the
    /// standard-buy-price value of those goods to their credit.
    /// Caller is responsible for actually withdrawing the goods
    /// from the player's warehouse.
    pub fn deliver(&mut self, player: u8, good: Good, qty: u16) -> BarterOutcome {
        if !self.wants.contains(&good) {
            return BarterOutcome::NotWanted;
        }
        let p = player as usize;
        if p >= self.credit.len() {
            return BarterOutcome::NoVillage;
        }
        let value = qty as i32 * crate::prices::price_of(good).buy as i32;
        self.credit[p] = self.credit[p].saturating_add(value);
        BarterOutcome::Delivered
    }

    /// Player withdraws `qty` of `good` from this village. Spends
    /// the standard-sell-price value from their credit. Caller is
    /// responsible for actually depositing the goods into the
    /// player's warehouse.
    pub fn withdraw(&mut self, player: u8, good: Good, qty: u16) -> BarterOutcome {
        if !self.offers.contains(&good) {
            return BarterOutcome::NotOffered;
        }
        let p = player as usize;
        if p >= self.credit.len() {
            return BarterOutcome::NoVillage;
        }
        let value = qty as i32 * crate::prices::price_of(good).sell as i32;
        if self.credit[p] < value {
            return BarterOutcome::NotEnoughCredit;
        }
        self.credit[p] -= value;
        BarterOutcome::Withdrawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliver_accumulates_credit_for_wanted_good() {
        let mut v = NativeVillage::new(0, 10, 10);
        let r = v.deliver(0, Good::Cloth, 5);
        assert_eq!(r, BarterOutcome::Delivered);
        assert!(v.credit[0] > 0);
    }

    #[test]
    fn deliver_rejects_unwanted_good() {
        let mut v = NativeVillage::new(0, 10, 10);
        let r = v.deliver(0, Good::Wood, 5);
        assert_eq!(r, BarterOutcome::NotWanted);
        assert_eq!(v.credit[0], 0);
    }

    #[test]
    fn withdraw_spends_credit_only_for_offered() {
        let mut v = NativeVillage::new(0, 10, 10);
        let _ = v.deliver(0, Good::Cloth, 100);
        let credit_before = v.credit[0];
        let r = v.withdraw(0, Good::Spices, 1);
        assert_eq!(r, BarterOutcome::Withdrawn);
        assert!(v.credit[0] < credit_before);
        assert_eq!(v.withdraw(0, Good::Iron, 1), BarterOutcome::NotOffered);
    }

    #[test]
    fn withdraw_blocked_by_credit() {
        let mut v = NativeVillage::new(0, 10, 10);
        let r = v.withdraw(0, Good::Spices, 100);
        assert_eq!(r, BarterOutcome::NotEnoughCredit);
    }
}
