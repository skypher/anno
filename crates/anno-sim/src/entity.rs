//! Local figure data used by carriers and civilians.
//!
//! `SourceFigureRecordLayout` records the executable layout consumed by
//! `FUN_00451890`; local `Figure` fields are not a byte-for-byte mirror.

/// Maximum number of active figures.
pub const MAX_FIGURES: usize = 2550;

/// Layout of the executable's figure pool, read from
/// `FUN_00451890` in `decompiled/1602_exe.c`.
///
/// The dispatcher iterates `0x9f6` records beginning at `DAT_004a2628`
/// with a `0x48`-byte stride. It consumes the accumulator, velocity, and
/// position fields before dispatching the record's state handler.
pub struct SourceFigureRecordLayout;

impl SourceFigureRecordLayout {
    pub const CAPACITY: usize = 0x9f6;
    pub const STRIDE_BYTES: usize = 0x48;
    pub const KIND_OFFSET: usize = 0x00;
    pub const DEFINITION_ID_OFFSET: usize = 0x04;
    pub const STATE_OFFSET: usize = 0x0e;
    pub const FLAGS_OFFSET: usize = 0x11;
    pub const ACCUMULATOR_OFFSET: usize = 0x14;
    pub const RATE_OFFSET: usize = 0x18;
    pub const VELOCITY_X_OFFSET: usize = 0x1c;
    pub const VELOCITY_Y_OFFSET: usize = 0x20;
    pub const VELOCITY_Z_OFFSET: usize = 0x24;
    pub const POSITION_X_OFFSET: usize = 0x28;
    pub const POSITION_Y_OFFSET: usize = 0x2c;
    pub const POSITION_Z_OFFSET: usize = 0x30;
}

/// Local simulation action tags. They are not the source record's state
/// byte, which is represented by `SourceFigureRecordLayout::STATE_OFFSET`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum ActionType {
    None = 0,
    Walking = 1,
    CarryingGoods = 2,
    Delivering = 3,
    Sailing = 4,
    Combat = 5,
    Farming = 6,
    Loading = 8,
    Fishing = 9,
    Mining = 10,
    Building = 11,
    TradeRoute = 12,
    Patrolling = 13,
    SpecialEvent = 14,
    Exploring = 15,
    ShipCombat = 16,
    Artillery = 17,
    Returning = 18,
    TradeShipAi = 0x20,
    FreeTrader = 0x21,
    Idle = 0x22,
}

/// A figure/entity in the world.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Figure {
    pub action: ActionType,
    pub owner: u8,

    /// Position in tile coordinates (fixed-point: multiply by 256 for sub-tile).
    pub tile_x: i32,
    pub tile_y: i32,

    /// Movement speed (sub-tiles per tick).
    pub speed: u16,

    /// Movement direction (0-7, compass directions).
    pub direction: u8,

    /// Target tile for pathfinding.
    pub target_x: i32,
    pub target_y: i32,

    /// Linked building instance index.
    pub building_idx: u16,

    /// Carried good type and amount.
    pub carried_good: u8,
    pub carried_amount: u16,

    /// Health/hitpoints (for military units).
    pub health: u16,

    /// Animation frame.
    pub anim_frame: u8,

    /// Movement timer accumulator.
    pub move_timer_ms: u32,

    /// Sprite set index for rendering.
    pub sprite_set: u8,

    /// Base sprite index.
    pub base_sprite: u16,

    /// Pre-computed path (sequence of tile positions to follow).
    pub path: Vec<(i32, i32)>,

    /// Current index into the path.
    pub path_idx: usize,
}

impl Figure {
    pub fn new() -> Self {
        Self {
            action: ActionType::None,
            owner: 0,
            tile_x: 0,
            tile_y: 0,
            speed: 0,
            direction: 0,
            target_x: 0,
            target_y: 0,
            building_idx: 0,
            carried_good: 0,
            carried_amount: 0,
            health: 0,
            anim_frame: 0,
            move_timer_ms: 0,
            sprite_set: 0,
            base_sprite: 0,
            path: Vec::new(),
            path_idx: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.action != ActionType::None
    }
}

#[cfg(test)]
mod tests {
    use super::SourceFigureRecordLayout as Layout;

    #[test]
    fn source_figure_record_layout_fits_dispatcher_stride() {
        assert_eq!(Layout::CAPACITY, 2550);
        assert_eq!(Layout::STRIDE_BYTES, 72);
        assert!(Layout::POSITION_Z_OFFSET + std::mem::size_of::<f32>() <= Layout::STRIDE_BYTES);
    }
}
