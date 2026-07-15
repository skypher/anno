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

/// Distinguishes the two decoded producer-to-root transfer protocols.
///
/// Type 8 (`TRAEGER`) supplies one named production input. Type 11
/// (`KARREN`) selects a filled supplier root and returns its output to a
/// city root. Both use the walking action states, but their reservation and
/// delivery accounting are different.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum CargoRoute {
    #[default]
    InputCarrier = 0,
    CityCart = 1,
}

/// The production-kind-2 type-12 plantation-worker state machine. This is
/// separate from `CargoRoute`: workers reach a map cell, switch through the
/// source lifecycle-2 harvest state, then return empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum SourceWorkerRoute {
    #[default]
    None = 0,
    ToResource = 1,
    Searching = 2,
    Harvesting = 3,
    Returning = 4,
    ReturningSearch = 5,
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

    /// Authored source figure `Speed` for type-8/type-11 route movement.
    /// Zero retains the legacy one-cell step behavior for non-source figures
    /// and save fixtures that predate the source movement state.
    #[serde(default)]
    pub source_move_speed: u16,

    /// Authored `ANIM 0` duration selected from the type-11 root's compiled
    /// `Figurnr`. Zero keeps manually constructed figures on the legacy
    /// KARREN runtime configuration.
    #[serde(default)]
    pub source_animation_frame_speed_ms: u16,

    /// Authored `ANIM 0` frames per direction for this source transfer
    /// figure. Zero uses the same manual-figure fallback as the frame time.
    #[serde(default)]
    pub source_animation_frames_per_direction: u8,

    /// Remaining source-grid distance in the current route cell. Cardinal
    /// cells start at one; diagonal cells start at √2, matching
    /// `FUN_0044a690` and `FUN_00451890`.
    #[serde(default)]
    pub source_step_remaining: f32,

    /// Continuous source-grid X position integrated by the source figure
    /// dispatcher between route-cell boundaries.
    #[serde(default)]
    pub source_position_x: f32,

    /// Continuous source-grid Y position integrated by the source figure
    /// dispatcher between route-cell boundaries.
    #[serde(default)]
    pub source_position_y: f32,

    /// Source-grid Z coordinate initialised from the live map cell's
    /// `Posoffs × 0.028` terrain elevation.
    #[serde(default)]
    pub source_position_z: f32,

    /// Distinguishes a saved/in-flight continuous position from the serde
    /// zero default used by manually constructed figures.
    #[serde(default)]
    pub source_position_initialized: bool,

    /// Source event-table slot at figure offset `+0x44` for categories that
    /// allocate through `DAT_00505e38`. `None` means this figure has no such
    /// shared map-event ownership.
    #[serde(default)]
    pub source_event_slot: Option<u16>,

    /// Marks a generic type-17 terrain figure allocated by `FUN_0044bd00`.
    /// Its route and lifecycle state live in the shared source event slot.
    #[serde(default)]
    pub source_terrain_event_active: bool,

    /// Movement direction (0-7, compass directions).
    pub direction: u8,

    /// Target tile for pathfinding.
    pub target_x: i32,
    pub target_y: i32,

    /// Source command-root kind selected as an in-flight carrier's supplier.
    /// `0` means this figure has no source-routed supplier.
    #[serde(default)]
    pub destination_kind: u8,

    /// Source command-root position selected for an in-flight carrier. This
    /// remains the producer anchor when a type-11 cart navigates to an edge
    /// cell of the root's oriented footprint.
    #[serde(default)]
    pub supplier_x: u16,
    #[serde(default)]
    pub supplier_y: u16,

    /// Which source transfer handler owns this figure.
    #[serde(default)]
    pub cargo_route: CargoRoute,
    /// Active only for the `FUN_0044b7e0` / `FUN_0045afd0` plantation worker
    /// path. The selected raw cell is retained in `supplier_x/y`.
    #[serde(default)]
    pub source_worker_route: SourceWorkerRoute,
    /// Centered root coordinate stored in the worker's source event at
    /// `FUN_0044b7e0` construction time. It can differ from `origin_x/y`,
    /// which retain the command anchor for root-state lookup.
    #[serde(default)]
    pub source_worker_home_x: i32,
    #[serde(default)]
    pub source_worker_home_y: i32,

    /// Type-11 city-cart origin. Type-8 figures retain their requesting
    /// building through `building_idx` and leave these fields at zero.
    #[serde(default)]
    pub origin_island: u8,
    #[serde(default)]
    pub origin_x: u16,
    #[serde(default)]
    pub origin_y: u16,
    #[serde(default)]
    pub origin_kind: u8,
    /// Map-owner selector of the source city record that owns this type-11
    /// root. `FUN_00481170` indexes the island's city-record table with this
    /// selector when it updates the root-count field at `+0x1fa`.
    #[serde(default)]
    pub origin_source_map_owner_slot: u8,
    /// Nested `HAUS_PRODTYP Kind` for a type-11 city-cart origin. This is
    /// distinct from `origin_kind`, which remains the outer map-command
    /// selector used to locate the root after the cart reaches a supplier.
    #[serde(default)]
    pub origin_production_kind: u8,

    /// Linked building instance index.
    pub building_idx: u16,

    /// Carried good type and amount.
    pub carried_good: u8,
    pub carried_amount: u16,
    /// Exact source-map quantity carried by a type-8 TRAEGER, in 1/32-good
    /// units. `carried_amount` remains the whole-good display quantity.
    #[serde(default)]
    pub cargo_fixed: u16,
    /// Authored type-8/type-11 `Maxtrag`, retained in the source 1/32-good scale.
    /// `FUN_0047d640` uses it at supplier arrival to top up the reservation
    /// with output produced while the figure was walking to that supplier.
    #[serde(default)]
    pub source_transfer_max_load_fixed: u16,

    /// Health/hitpoints (for military units).
    pub health: u16,

    /// Animation frame.
    pub anim_frame: u8,

    /// Source animation-time remainder in milliseconds. `FUN_0045d0b0`
    /// subtracts one authored frame duration whenever this accumulator reaches
    /// it, so this is not a shared visual clock.
    #[serde(default)]
    pub source_animation_elapsed_ms: u32,

    /// Current generic-figure animation selector. `FUN_00446d90` resets the
    /// animation counters when this changes to a distinct authored selector;
    /// a plantation worker starts at selector 1, switches to 2 after reaching
    /// its resource, then returns to 0 after harvesting.
    #[serde(default)]
    pub source_animation_state: u8,

    /// Movement timer accumulator.
    pub move_timer_ms: u32,

    /// Sprite set index for rendering.
    pub sprite_set: u8,

    /// Base sprite index.
    pub base_sprite: u16,

    /// Pre-computed path (sequence of tile positions to follow).
    pub path: Vec<(i32, i32)>,

    /// Exact predecessor steps produced by the source grid for the current
    /// shared-event leg. Type-11 copies these into `DAT_00505e38` before the
    /// `FUN_0045c450` dispatcher consumes its bounded route program.
    #[serde(default)]
    pub source_event_route_steps: Vec<crate::source_route::SourceRouteStep>,

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
            source_move_speed: 0,
            source_animation_frame_speed_ms: 0,
            source_animation_frames_per_direction: 0,
            source_step_remaining: 0.0,
            source_position_x: 0.0,
            source_position_y: 0.0,
            source_position_z: 0.0,
            source_position_initialized: false,
            source_event_slot: None,
            source_terrain_event_active: false,
            direction: 0,
            target_x: 0,
            target_y: 0,
            destination_kind: 0,
            supplier_x: 0,
            supplier_y: 0,
            cargo_route: CargoRoute::InputCarrier,
            source_worker_route: SourceWorkerRoute::None,
            source_worker_home_x: 0,
            source_worker_home_y: 0,
            origin_island: 0,
            origin_x: 0,
            origin_y: 0,
            origin_kind: 0,
            origin_source_map_owner_slot: 0,
            origin_production_kind: 0,
            building_idx: 0,
            carried_good: 0,
            carried_amount: 0,
            cargo_fixed: 0,
            source_transfer_max_load_fixed: 0,
            health: 0,
            anim_frame: 0,
            source_animation_elapsed_ms: 0,
            source_animation_state: 0,
            move_timer_ms: 0,
            sprite_set: 0,
            base_sprite: 0,
            path: Vec::new(),
            source_event_route_steps: Vec::new(),
            path_idx: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.action != ActionType::None
    }

    /// Match the `tile + 0.5` horizontal coordinates passed to
    /// `FUN_00446ca0` by the source figure constructors.
    pub fn initialize_source_position(&mut self) {
        self.source_position_x = self.tile_x as f32 + 0.5;
        self.source_position_y = self.tile_y as f32 + 0.5;
        self.source_position_initialized = true;
    }

    /// Reset the per-figure animation counters when source dispatch selects
    /// a new `ANIM` record, matching `FUN_00446d90` / `FUN_0045d0b0`.
    pub fn reset_source_animation(&mut self) {
        self.anim_frame = 0;
        self.source_animation_elapsed_ms = 0;
    }

    /// Mirror the counter-reset portion of `FUN_00446d90` for source figure
    /// states whose compiled animation descriptors differ.
    pub fn select_source_animation_state(&mut self, state: u8) {
        if self.source_animation_state != state {
            self.source_animation_state = state;
            self.reset_source_animation();
        }
    }

    /// Advance an `ENDLESS` source animation using the selected frame's
    /// current duration. This mirrors the accumulator/frame loop in
    /// `FUN_0045d0b0` for the animation kinds used by carriers.
    pub fn advance_source_animation(
        &mut self,
        elapsed_ms: u32,
        frame_duration_ms: u32,
        frames_per_direction: u8,
    ) {
        let duration = frame_duration_ms.max(1);
        let frames = frames_per_direction.max(1);
        self.source_animation_elapsed_ms =
            self.source_animation_elapsed_ms.saturating_add(elapsed_ms);
        while self.source_animation_elapsed_ms >= duration {
            self.source_animation_elapsed_ms -= duration;
            self.anim_frame = self.anim_frame.wrapping_add(1) % frames;
        }
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

    #[test]
    fn new_figure_starts_with_zero_source_animation_time() {
        assert_eq!(super::Figure::new().source_animation_elapsed_ms, 0);
    }

    #[test]
    fn source_animation_reset_clears_frame_and_elapsed_time() {
        let mut figure = super::Figure::new();
        figure.anim_frame = 6;
        figure.source_animation_elapsed_ms = 1_275;

        figure.reset_source_animation();

        assert_eq!(figure.anim_frame, 0);
        assert_eq!(figure.source_animation_elapsed_ms, 0);
    }

    #[test]
    fn source_animation_state_reset_occurs_only_on_selection_change() {
        let mut figure = super::Figure::new();
        figure.anim_frame = 3;
        figure.source_animation_elapsed_ms = 71;

        figure.select_source_animation_state(2);
        assert_eq!(figure.source_animation_state, 2);
        assert_eq!(
            (figure.anim_frame, figure.source_animation_elapsed_ms),
            (0, 0)
        );

        figure.anim_frame = 4;
        figure.source_animation_elapsed_ms = 17;
        figure.select_source_animation_state(2);
        assert_eq!(
            (figure.anim_frame, figure.source_animation_elapsed_ms),
            (4, 17)
        );
    }

    #[test]
    fn source_animation_keeps_a_per_figure_frame_remainder() {
        let mut figure = super::Figure::new();
        figure.advance_source_animation(84, 85, 8);
        assert_eq!(
            (figure.anim_frame, figure.source_animation_elapsed_ms),
            (0, 84)
        );

        figure.advance_source_animation(1, 85, 8);
        assert_eq!(
            (figure.anim_frame, figure.source_animation_elapsed_ms),
            (1, 0)
        );

        figure.advance_source_animation(180, 60, 8);
        assert_eq!(
            (figure.anim_frame, figure.source_animation_elapsed_ms),
            (4, 0)
        );
    }
}
