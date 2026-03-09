//! Entity/figure action state machine.
//!
//! Ported from FUN_00451890 (figure/unit movement and action system).
//! Manages up to 2550 figures with 18+ action types.

/// Maximum number of active figures.
pub const MAX_FIGURES: usize = 2550;

/// Figure action types (the 16-case switch from FUN_00451890).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
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
