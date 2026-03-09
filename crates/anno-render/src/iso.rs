//! Isometric tile map renderer.
//!
//! Ported from FUN_00489a60 (main tile row renderer) and
//! FUN_00489c90 (per-tile renderer).
//! Renders the game world as a diamond-grid isometric view with
//! two passes per row (ground layer, then buildings).

use crate::camera::{Camera, ZoomLevel};
use crate::framebuffer::Framebuffer;
use crate::palette::RemapTable;
use crate::sprite::{SpriteCategory, SpriteManager};

/// Tile cell data packed into 32 bits.
/// Matches the original bit layout from island tile maps.
#[derive(Debug, Clone, Copy, Default)]
pub struct TileCell(pub u32);

impl TileCell {
    /// Building/terrain definition ID (bits 0-12).
    pub fn building_id(self) -> u16 {
        (self.0 & 0x1FFF) as u16
    }

    /// Rotation of the building (bits 13-14).
    pub fn rotation(self) -> u8 {
        ((self.0 >> 13) & 0x3) as u8
    }

    /// Animation frame counter (bits 15-18).
    pub fn anim_frame(self) -> u8 {
        ((self.0 >> 15) & 0xF) as u8
    }

    /// Player/owner index (bits 19-21).
    pub fn player(self) -> u8 {
        ((self.0 >> 19) & 0x7) as u8
    }

    /// Construction flag (bit 26).
    pub fn under_construction(self) -> bool {
        (self.0 >> 26) & 1 != 0
    }

    /// Damage flag (bit 27).
    pub fn damaged(self) -> bool {
        (self.0 >> 27) & 1 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Building definition data (136 bytes each in original).
#[derive(Debug, Clone)]
pub struct BuildingDef {
    pub id: u16,
    pub category: BuildingCategory,
    pub width: u8,  // tile footprint width
    pub height: u8, // tile footprint height
    pub y_offset: i32,
    pub base_sprite_id: u32,
    pub anim_frames: u8,
    pub anim_speed: u8,
    pub rotation_offset: u32, // sprite offset per rotation
}

/// Building categories from the original engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BuildingCategory {
    Road = 1,
    Production = 3,
    Wall = 4,
    HuntingLodge = 5,
    Fishery = 6,
    Market = 7,
    TradingPost = 8,
    RawResource = 9,
    Terrain = 10,
    Quarry = 11,
    Ruins = 12,
    Residence = 13,
    Structure = 14,
    Military = 15,
    Watchtower = 16,
    Chapel = 18,
    Church = 19,
    Bathhouse = 20,
    Theater = 21,
    Clinic = 22,
    School = 23,
    University = 24,
    Gallows = 25,
    Fountain = 26,
    Palace = 27,
    Monument = 28,
    TriumphalArch = 29,
    Headquarters = 30,
    PirateDwelling = 31,
    Unknown = 255,
}

/// World map data: collection of islands.
pub struct WorldMap {
    pub islands: Vec<Island>,
    /// Global tile map: island index per cell (0xFF = ocean).
    pub global_map: Vec<u8>,
    pub map_width: u32,
    pub map_height: u32,
}

/// A single island in the world.
pub struct Island {
    pub id: u8,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub owner: u8,
    pub tiles: Vec<TileCell>,
}

impl Island {
    pub fn get_tile(&self, local_x: u32, local_y: u32) -> TileCell {
        if local_x >= self.width || local_y >= self.height {
            return TileCell(0);
        }
        let idx = (local_y * self.width + local_x) as usize;
        self.tiles.get(idx).copied().unwrap_or(TileCell(0))
    }
}

/// Render the visible portion of the isometric map.
pub fn render_map(
    fb: &mut Framebuffer,
    camera: &Camera,
    world: &WorldMap,
    sprites: &SpriteManager,
    building_defs: &[BuildingDef],
    player_remaps: &[RemapTable],
) {
    let tw = camera.zoom.tile_width() as i32;
    let th = camera.zoom.tile_height() as i32;
    let half_tw = tw / 2;
    let zoom_idx = camera.zoom.gfx_set_offset();

    let stadtfld = match sprites.get_set(SpriteCategory::Stadtfld, zoom_idx) {
        Some(s) => s,
        None => return,
    };

    let rows = camera.viewport_rows();
    let cols = camera.viewport_cols();

    let ((step_x_dx, step_x_dy), (step_y_dx, step_y_dy)) = camera.rotation.step_vectors();

    // Starting tile position
    let mut row_tile_x = camera.origin_x;
    let mut row_tile_y = camera.origin_y;

    for row in 0..rows {
        // Diamond stagger: alternate rows offset by half tile width
        let x_offset = if row % 2 == 1 { half_tw } else { 0 };
        let base_screen_y = row as i32 * th;

        let mut tile_x = row_tile_x;
        let mut tile_y = row_tile_y;

        // Pass 1: ground layer (pass=0), Pass 2: buildings (pass=1)
        for pass in 0..2 {
            let mut tx = tile_x;
            let mut ty = tile_y;

            for col in 0..cols {
                let screen_x = col as i32 * tw + x_offset;
                let screen_y = base_screen_y;

                // Bounds check against world map
                if tx >= 0
                    && ty >= 0
                    && (tx as u32) < world.map_width
                    && (ty as u32) < world.map_height
                {
                    let map_idx = (ty as u32 * world.map_width + tx as u32) as usize;

                    if map_idx < world.global_map.len() {
                        let island_idx = world.global_map[map_idx];

                        if island_idx == 0xFF {
                            // Ocean tile — render water sprite
                            if pass == 0 {
                                // Use sprite 0 as ocean placeholder
                                stadtfld.draw(fb, 0, screen_x, screen_y, None);
                            }
                        } else if let Some(island) =
                            world.islands.get(island_idx as usize)
                        {
                            let local_x = tx as u32 - island.x;
                            let local_y = ty as u32 - island.y;
                            let cell = island.get_tile(local_x, local_y);

                            if !cell.is_empty() {
                                let bid = cell.building_id() as usize;
                                if let Some(bdef) = building_defs.get(bid) {
                                    // Ground pass: render terrain/roads
                                    // Building pass: render structures
                                    let is_ground =
                                        matches!(bdef.category, BuildingCategory::Terrain | BuildingCategory::Road);

                                    if (pass == 0 && is_ground) || (pass == 1 && !is_ground) {
                                        let sprite_id = compute_sprite_id(bdef, &cell);

                                        let player = cell.player() as usize;
                                        let remap = if player < player_remaps.len() {
                                            Some(&player_remaps[player])
                                        } else {
                                            None
                                        };

                                        stadtfld.draw(
                                            fb,
                                            sprite_id as usize,
                                            screen_x,
                                            screen_y - bdef.y_offset,
                                            remap,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                tx += step_x_dx;
                ty += step_x_dy;
            }
        }

        // Advance to next row
        row_tile_x += step_y_dx;
        row_tile_y += step_y_dy;
    }
}

/// Compute the sprite index for a building, considering rotation and animation.
fn compute_sprite_id(def: &BuildingDef, cell: &TileCell) -> u32 {
    let mut id = def.base_sprite_id;

    // Add rotation offset
    id += cell.rotation() as u32 * def.rotation_offset;

    // Add animation frame
    if def.anim_frames > 0 {
        id += (cell.anim_frame() % def.anim_frames) as u32;
    }

    id
}
