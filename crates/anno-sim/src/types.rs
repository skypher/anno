//! Core game types and enumerations.
//!
//! Derived from the building definition table and goods enumeration
//! in the decompiled binary.

/// Population tiers (5 levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
/// The original engine has 59 goods entries; these are the most important ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    Grapes = 21,     // WEINTRAUBEN — raw material for wine/alcohol
    Stone = 22,      // STEINE — quarried stone
    Ore = 23,        // EISENERZ — iron ore (before smelting)
    GoldOre = 24,    // GOLDERZ — gold ore (before smelting)
    Hides = 25,      // HAEUTE — animal hides
    Cotton = 26,     // BAUMWOLLE — cotton (alternative to wool)
    Silk = 27,       // SEIDE — silk
    Jewelry = 28,    // SCHMUCK — jewelry
    Clothing = 29,   // KLEIDUNG — clothing
    Fish = 30,       // FISCHE — fish
    // ... more goods exist in the original
}

/// Military unit types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MilitaryUnit {
    Swordsman = 1,
    Cavalry = 2,
    Musketeer = 3,
    Cannoneer = 4,
}

/// Production building type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
