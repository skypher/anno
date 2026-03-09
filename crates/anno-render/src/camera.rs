//! Isometric camera with rotation and zoom.
//!
//! Ported from the viewport structure at PTR_DAT_0049af64.
//! The camera manages scroll position, tile stepping vectors,
//! rotation (4 orientations), and zoom (3 levels).

/// Zoom level configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomLevel {
    /// 100%: 64px tiles, uses GFX/ sprites
    Full = 100,
    /// 50%: 32px tiles, uses MGFX/ sprites
    Medium = 50,
    /// 25%: 16px tiles, uses SGFX/ sprites
    Small = 25,
}

impl ZoomLevel {
    pub fn tile_width(self) -> u32 {
        match self {
            ZoomLevel::Full => 64,
            ZoomLevel::Medium => 32,
            ZoomLevel::Small => 16,
        }
    }

    pub fn tile_height(self) -> u32 {
        match self {
            ZoomLevel::Full => 31,
            ZoomLevel::Medium => 15,
            ZoomLevel::Small => 7,
        }
    }

    /// GFX set index offset for this zoom level.
    pub fn gfx_set_offset(self) -> usize {
        match self {
            ZoomLevel::Full => 0,
            ZoomLevel::Medium => 1,
            ZoomLevel::Small => 2,
        }
    }
}

/// Rotation state (90-degree increments).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    R0 = 0,
    R90 = 1,
    R180 = 2,
    R270 = 3,
}

impl Rotation {
    /// Tile stepping vectors for the current rotation.
    /// Returns ((step_x_dx, step_x_dy), (step_y_dx, step_y_dy))
    /// where step_x is the direction along each tile row,
    /// and step_y is the direction for each new row.
    pub fn step_vectors(self) -> ((i32, i32), (i32, i32)) {
        match self {
            Rotation::R0 => ((1, 1), (-1, 1)),    // standard diamond iso
            Rotation::R90 => ((1, -1), (-1, -1)),
            Rotation::R180 => ((-1, -1), (1, -1)),
            Rotation::R270 => ((-1, 1), (1, 1)),
        }
    }

    pub fn rotate_cw(self) -> Self {
        match self {
            Rotation::R0 => Rotation::R90,
            Rotation::R90 => Rotation::R180,
            Rotation::R180 => Rotation::R270,
            Rotation::R270 => Rotation::R0,
        }
    }

    pub fn rotate_ccw(self) -> Self {
        match self {
            Rotation::R0 => Rotation::R270,
            Rotation::R90 => Rotation::R0,
            Rotation::R180 => Rotation::R90,
            Rotation::R270 => Rotation::R180,
        }
    }
}

/// Isometric camera state.
pub struct Camera {
    /// Current map origin in tile coordinates.
    pub origin_x: i32,
    pub origin_y: i32,

    /// Scroll position in abstract scroll units.
    pub scroll_x: i32,
    pub scroll_y: i32,

    /// Screen dimensions in pixels.
    pub screen_width: u32,
    pub screen_height: u32,

    pub zoom: ZoomLevel,
    pub rotation: Rotation,
}

impl Camera {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            origin_x: 0,
            origin_y: 0,
            scroll_x: 0,
            scroll_y: 0,
            screen_width,
            screen_height,
            zoom: ZoomLevel::Full,
            rotation: Rotation::R0,
        }
    }

    /// Number of tile columns visible on screen.
    pub fn viewport_cols(&self) -> u32 {
        self.screen_width / self.zoom.tile_width() + 2
    }

    /// Number of tile rows visible on screen.
    pub fn viewport_rows(&self) -> u32 {
        // Original: (tile_h * 11 + screen_height) / tile_h
        let th = self.zoom.tile_height();
        (th * 11 + self.screen_height) / th
    }

    /// Set the camera to look at a specific tile coordinate.
    pub fn look_at(&mut self, tile_x: i32, tile_y: i32) {
        let ((sx_dx, sx_dy), (sy_dx, sy_dy)) = self.rotation.step_vectors();

        // Convert tile coords to scroll position
        self.scroll_x = tile_x * sx_dx + tile_y * sy_dx;
        self.scroll_y = tile_x * sx_dy + tile_y * sy_dy;

        self.update_origin();
    }

    /// Update the tile origin from scroll position (inverse isometric projection).
    fn update_origin(&mut self) {
        let ((sx_dx, sx_dy), (sy_dx, sy_dy)) = self.rotation.step_vectors();

        // The scroll-to-tile conversion depends on rotation.
        // For rotation 0: origin_x = (scroll_x + scroll_y) / 2
        //                  origin_y = (scroll_y - scroll_x) / 2
        match self.rotation {
            Rotation::R0 => {
                self.origin_x = (self.scroll_x + self.scroll_y) / 2;
                self.origin_y = (self.scroll_y - self.scroll_x) / 2;
            }
            Rotation::R90 => {
                self.origin_x = (self.scroll_x - self.scroll_y) / 2;
                self.origin_y = (-self.scroll_x - self.scroll_y) / 2;
            }
            Rotation::R180 => {
                self.origin_x = (-self.scroll_x - self.scroll_y) / 2;
                self.origin_y = (self.scroll_x - self.scroll_y) / 2;
            }
            Rotation::R270 => {
                self.origin_x = (self.scroll_y - self.scroll_x) / 2;
                self.origin_y = (self.scroll_x + self.scroll_y) / 2;
            }
        }
    }

    /// Scroll the camera by pixel offsets.
    pub fn scroll(&mut self, dx: i32, dy: i32) {
        let tw = self.zoom.tile_width() as i32;
        self.scroll_x += dx / tw;
        self.scroll_y += dy / tw;
        self.update_origin();
    }

    /// Convert screen pixel coordinates to tile coordinates.
    pub fn screen_to_tile(&self, screen_x: i32, screen_y: i32) -> (i32, i32) {
        let tw = self.zoom.tile_width() as i32;
        let th = self.zoom.tile_height() as i32;

        // Offset from screen center
        let cx = screen_x - self.screen_width as i32 / 2;
        let cy = screen_y - self.screen_height as i32 / 2;

        // Isometric to tile: standard diamond projection
        let tile_col = cx / tw + cy / th;
        let tile_row = cy / th - cx / tw;

        let ((sx_dx, sx_dy), (sy_dx, sy_dy)) = self.rotation.step_vectors();

        // Apply rotation and add camera origin
        (
            self.origin_x + tile_col * sx_dx + tile_row * sy_dx,
            self.origin_y + tile_col * sx_dy + tile_row * sy_dy,
        )
    }

    /// Convert tile coordinates to screen pixel coordinates.
    pub fn tile_to_screen(&self, tile_x: i32, tile_y: i32) -> (i32, i32) {
        let tw = self.zoom.tile_width() as i32;
        let th = self.zoom.tile_height() as i32;

        // Relative to camera origin
        let dx = tile_x - self.origin_x;
        let dy = tile_y - self.origin_y;

        // Tile to isometric screen: diamond projection
        let screen_x = (dx - dy) * tw / 2 + self.screen_width as i32 / 2;
        let screen_y = (dx + dy) * th / 2 + self.screen_height as i32 / 2;

        (screen_x, screen_y)
    }

    pub fn set_zoom(&mut self, zoom: ZoomLevel) {
        self.zoom = zoom;
    }

    pub fn set_rotation(&mut self, rotation: Rotation) {
        self.rotation = rotation;
        self.update_origin();
    }
}
