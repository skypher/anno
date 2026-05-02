//! Per-island climate model.
//!
//! Anno 1602 distinguishes northern and southern islands by which goods
//! their plantations can grow. We approximate by inspecting an island's
//! world-space `y_pos`: islands in the upper half of the world are
//! northern, the lower half are southern.

use crate::types::Good;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Climate {
    North,
    South,
}

impl Climate {
    pub fn label(self) -> &'static str {
        match self {
            Climate::North => "North",
            Climate::South => "South",
        }
    }
}

/// Heuristic: islands sitting in the upper half of the world map (smaller
/// y_pos values) are treated as northern. `world_height` is the SZS's
/// total world height; pass 512 if unknown.
pub fn climate_for_y(y_pos: u32, world_height: u32) -> Climate {
    let split = world_height.max(2) / 2;
    if y_pos < split { Climate::North } else { Climate::South }
}

/// Subset of goods that need a specific climate to be produced (plantation
/// crops). Returns `true` if the climate supports producing this good, or
/// the good is climate-agnostic (every other Good).
pub fn allows_production(climate: Climate, good: Good) -> bool {
    let needs_south = matches!(
        good,
        Good::Tobacco | Good::TobaccoProducts | Good::Cocoa
            | Good::Sugar  | Good::Spices | Good::Cotton | Good::Silk
    );
    let needs_north = matches!(
        good,
        Good::Wool | Good::Grain | Good::Flour | Good::Cattle | Good::Hides
    );
    if needs_south { return climate == Climate::South; }
    if needs_north { return climate == Climate::North; }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_at_half() {
        assert_eq!(climate_for_y(50, 200), Climate::North);
        assert_eq!(climate_for_y(150, 200), Climate::South);
    }

    #[test]
    fn south_grows_tobacco() {
        assert!(allows_production(Climate::South, Good::Tobacco));
        assert!(!allows_production(Climate::North, Good::Tobacco));
    }

    #[test]
    fn north_grows_wool() {
        assert!(allows_production(Climate::North, Good::Wool));
        assert!(!allows_production(Climate::South, Good::Wool));
    }

    #[test]
    fn climate_agnostic_goods_pass_anywhere() {
        assert!(allows_production(Climate::North, Good::Tools));
        assert!(allows_production(Climate::South, Good::Tools));
        assert!(allows_production(Climate::North, Good::Bricks));
    }
}
