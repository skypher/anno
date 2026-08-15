//! Core game types and enumerations.
//!
//! Derived from the building definition table and goods enumeration
//! in the decompiled binary.

/// Population tiers (5 levels).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[repr(u8)]
pub enum PopTier {
    Pioneer = 0,
    Settler = 1,
    Citizen = 2,
    Merchant = 3,
    Aristocrat = 4,
}

pub const NUM_POP_TIERS: usize = 5;

/// Goods/resource types.
///
/// The Anno 1602 engine stores 59 internal good slots (`text.cod`
/// `[WARE]` block + flag/wildcard entries like ALLWARE/NOWARE +
/// terrain growth pseudo-goods like GRAS/BAUM/TABAKBAUM that don't
/// surface to the player). The 25 player-facing economic goods from
/// the manual's WARE list are all enumerated below; we add a handful
/// of extras (Stone, WildGame, Cotton, Silk, Fish, Grapes, Meat,
/// SugarCane) that appear in haeuser.cod production chains but
/// aren't shown as distinct entries in the manual. These discriminants
/// are local simulation ids, not `text.cod [WARE]` ids; source tables
/// that use WARE order need an explicit mapping.
/// `PartialOrd`/`Ord` follow the discriminant order above. They exist so the
/// per-good maps on `Warehouse` can be `BTreeMap`s: bincode encodes a map by
/// iterating it, so a `HashMap` would leak `RandomState` ordering straight
/// into `save::state_hash` and break the lockstep comparison signal.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[repr(u8)]
pub enum Good {
    None = 0,
    // Raw materials
    Wood = 1,
    Iron = 2,
    Gold = 3,
    Wool = 4,
    Sugar = 5,
    Tobacco = 6,
    Cattle = 7,
    Grain = 8,
    Flour = 9,
    // Processed goods
    Tools = 10,
    Bricks = 11,
    Swords = 12,
    Muskets = 13,
    Cannons = 14,
    Food = 15,
    Cloth = 16,
    Alcohol = 17,
    TobaccoProducts = 18,
    Spices = 19,
    Cocoa = 20,
    Grapes = 21, // WEINTRAUBEN — raw material for wine/alcohol
    Stone = 22,  // STEINE — quarried stone
    Ore = 23,    // EISENERZ — iron ore (before smelting)
    // Discriminant 24 was Good::GoldOre, but no GOLDERZ string
    // appears in any COD file — gold mines emit GOLD directly
    // (haeuser.cod has only the bare "GOLD" identifier). Slot
    // left vacant to preserve numeric ordering for the later
    // variants below.
    WildGame = 25, // WILD — wild game from hunting lodges
    // (not "HAEUTE"/hides — that string never
    // appears in any COD file; the actual
    // huntable raw material is `WILD`)
    Cotton = 26,   // BAUMWOLLE — cotton (alternative to wool)
    Silk = 27,     // SEIDE — silk
    Jewelry = 28,  // SCHMUCK — jewelry
    Clothing = 29, // KLEIDUNG — clothing
    Fish = 30,     // FISCHE — fish
    Meat = 31,     // FLEISCH — butcher's output, distinct from
    // raw cattle / hunted game
    SugarCane = 32, // ZUCKERROHR — plantation raw, separate
                    // good from the refined ZUCKER (Sugar)
}

/// Source text.cod WARE slots 2 through 24, in executable table order.
/// The local Good discriminants intentionally differ, so source systems
/// indexed by WARE must use this mapping rather than casting Good to u8.
pub const SOURCE_WARE_GOODS: [(u8, Good); 23] = [
    (2, Good::Ore),
    (3, Good::Gold),
    (4, Good::Wool),
    (5, Good::Sugar),
    (6, Good::Tobacco),
    (7, Good::Cattle),
    (8, Good::Grain),
    (9, Good::Flour),
    (10, Good::Iron),
    (11, Good::Swords),
    (12, Good::Muskets),
    (13, Good::Cannons),
    (14, Good::Food),
    (15, Good::TobaccoProducts),
    (16, Good::Spices),
    (17, Good::Cocoa),
    (18, Good::Alcohol),
    (19, Good::Cloth),
    (20, Good::Clothing),
    (21, Good::Jewelry),
    (22, Good::Tools),
    (23, Good::Wood),
    (24, Good::Bricks),
];

impl Good {
    /// Source text.cod WARE slot used by executable city-good tables.
    pub const fn source_ware_slot(self) -> Option<u8> {
        match self {
            Self::Ore => Some(2),
            Self::Gold => Some(3),
            Self::Wool => Some(4),
            Self::Sugar => Some(5),
            Self::Tobacco => Some(6),
            Self::Cattle => Some(7),
            Self::Grain => Some(8),
            Self::Flour => Some(9),
            Self::Iron => Some(10),
            Self::Swords => Some(11),
            Self::Muskets => Some(12),
            Self::Cannons => Some(13),
            Self::Food => Some(14),
            Self::TobaccoProducts => Some(15),
            Self::Spices => Some(16),
            Self::Cocoa => Some(17),
            Self::Alcohol => Some(18),
            Self::Cloth => Some(19),
            Self::Clothing => Some(20),
            Self::Jewelry => Some(21),
            Self::Tools => Some(22),
            Self::Wood => Some(23),
            Self::Bricks => Some(24),
            Self::None
            | Self::Grapes
            | Self::Stone
            | Self::WildGame
            | Self::Cotton
            | Self::Silk
            | Self::Fish
            | Self::Meat
            | Self::SugarCane => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Good, SOURCE_WARE_GOODS};

    #[test]
    fn source_ware_slots_match_text_cod_order() {
        assert_eq!(Good::Food.source_ware_slot(), Some(14));
        assert_eq!(Good::Cloth.source_ware_slot(), Some(19));
        assert_eq!(Good::Tools.source_ware_slot(), Some(22));
        assert_eq!(Good::Wood.source_ware_slot(), Some(23));
        assert_eq!(Good::Bricks.source_ware_slot(), Some(24));
        assert_eq!(SOURCE_WARE_GOODS.len(), 23);
    }
}

/// Military unit types — mirrors the four `FIGTYP_*` land entries
/// in `figuren.cod` (`SCHWERT`, `KAVALERIE`, `MUSKETIER`,
/// `KANONIER`). The discriminants follow the order of `[FIGKIND]`
/// in `text.cod`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MilitaryUnit {
    Infantry = 1,
    Cavalry = 2,
    Musketeer = 3,
    Cannoneer = 4,
}

/// Production building type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum ProductionType {
    Craft = 1,
    Plantation = 2,
    Mine = 3,
    Residence = 7,
    Fire = 9,
    Volcano = 10,
}

/// Game difficulty level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Difficulty {
    Easy = 0,
    Medium = 1,
    Hard = 2,
}

/// Game time: 600 ticks = 1 displayed minute.
pub const TICKS_PER_MINUTE: u32 = 600;
pub const TICKS_PER_SECOND: u32 = 10;
