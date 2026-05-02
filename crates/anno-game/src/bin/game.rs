//! Anno 1602 — Live game viewer with integrated simulation.
//!
//! Renders the isometric map while running the full simulation loop.
//! Carriers, trade ships, and military units are shown as colored markers.
//!
//! Controls:
//!   Arrow keys / mouse drag: scroll the map
//!   +/-/scroll: zoom in/out
//!   Tab: cycle through islands
//!   W: toggle world map vs single island
//!   Space: pause/unpause simulation
//!   F/G: decrease/increase game speed (1x-8x)
//!   B: toggle build mode (then 1-9 to select building, click to place;
//!      [/] cycle category, PgUp/PgDn flip page, Z cycles orientation)
//!   M: toggle music on/off
//!   N: next music track
//!   V: cycle music volume
//!   S: save screenshot
//!   D: toggle demolish mode (click to remove building, refunds 50% cost)
//!   T: toggle tax panel (Up/Down=select tier, Left/Right=adjust rate)
//!   Y: toggle diplomacy panel (Up/Down=select player, Left/Right=cycle relation)
//!   R: toggle trade-route mode (click warehouses, Enter=commit, Esc=cancel)
//!   H: toggle economy HUD overlay
//!   C: toggle service coverage overlay (green=covered, red=uncovered)
//!   K: toggle economy history graphs (gold / population / satisfaction)
//!   P: toggle production overview (per-good producer count / efficiency / stock)
//!   O: toggle player roster (state / gold / pop / units / diplomacy)
//!   A: open market (buy/sell goods at first warehouse — Left/Right by 10)
//!   U: open warehouse table (per-warehouse stock columns for current island)
//!   J: open fleet panel (active ships' route, state, cargo, profit)
//!   Shift+R: open active-routes panel (Up/Down + Bksp to delete)
//!   ?: open scenario objectives panel
//!   Enter: open chat input (multiplayer); type then Enter to send, Esc cancels
//!   F2: open scenario picker (Up/Down, Enter to relaunch with chosen .szs)
//!   F3: open save-slot picker (Up/Down, S to save, L to load)
//!   F4: build a TradeShip at the first warehouse (1000 gold)
//!   F6: toggle path-debug overlay (carrier A* paths + ship ocean paths)
//!   F7: found a colony (drop a Kontor) on the current island (500 gold)
//!   F8: export current islands as `.szs` to saves/<scenario>.export.szs
//!   F10: open settings panel (volumes, default zoom)
//!   F12: toggle perf overlay (sim/render/frame microseconds + FPS)
//!   F5: quicksave (writes saves/<scenario>.quicksave.bin)
//!   F9: quickload
//!
//! Multiplayer flags:
//!   --host PORT          run as host, broadcast snapshots every 1s
//!   --join HOST:PORT     run as client, replace local sim with received snapshots
//!   Left-click on military unit: select (Shift+click adds to selection)
//!   Right-click (with units selected): move-to order
//!   Right-click (no selection): inspect building/tile
//!   Shift+Right-click: open context menu (Inspect / Move / Demolish)
//!   Escape: quit (or close inspection / cancel build or demolish mode)

use anno_audio::engine::AudioEngine;
use anno_formats::cod::CodFile;
use anno_formats::col::parse_col;
use anno_formats::szs::{Island, IslandTile, SzsFile};
use anno_render::sprite::{SpriteCategory, SpriteManager};
use anno_sim::ai::{AiController, AiPersonality, Difficulty};
use anno_sim::building::BuildingInstance;
use anno_sim::combat::{Diplomacy, MilitaryUnit, UnitType};
use anno_sim::data_bridge;
use anno_sim::entity::ActionType;
use anno_sim::island_map::IslandMap;
use anno_sim::player::Player;
use anno_sim::simulation::Simulation;
use anno_sim::trade::{RouteStop, TradeRoute, TradeShip};
use anno_sim::types::Good;
use anno_sim::warehouse::Warehouse;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;

const WINDOW_W: u32 = 1280;
const WINDOW_H: u32 = 800;
const BG_COLOR: (u8, u8, u8) = (0x10, 0x20, 0x40);

const ZOOM_TILE_W: [i32; 3] = [64, 32, 16];
const ZOOM_TILE_H: [i32; 3] = [31, 15, 7];

/// Animation state for building sprites.
/// Maps sprite indices to their animation parameters.
struct AnimationState {
    /// For each base sprite index: (anim_anz, anim_add, anim_time_ms)
    /// anim_anz = number of frames, anim_add = sprite offset per frame
    entries: Vec<AnimEntry>,
    /// Elapsed time in ms (wraps at u32::MAX)
    elapsed_ms: u32,
}

struct AnimEntry {
    /// Base sprite index (COD gfx field)
    base_gfx: i32,
    /// Number of animation frames
    anim_anz: i32,
    /// Sprite offset per frame
    anim_add: i32,
    /// Milliseconds per frame (default 200)
    anim_time: i32,
    /// Total sprite range occupied by this building (for all rotations)
    total_sprites: i32,
}

impl AnimationState {
    fn new(cod: &CodFile) -> Self {
        let mut entries = Vec::new();
        for b in &cod.buildings {
            if b.gfx >= 0 && b.anim_anz > 1 && b.anim_add > 0 {
                let total = b.rotate.max(1) * b.anim_anz * b.anim_add;
                entries.push(AnimEntry {
                    base_gfx: b.gfx,
                    anim_anz: b.anim_anz,
                    anim_add: b.anim_add,
                    anim_time: if b.anim_time > 0 { b.anim_time } else { 200 },
                    total_sprites: total,
                });
            }
        }
        // Sort by base_gfx for binary search
        entries.sort_by_key(|e| e.base_gfx);
        Self {
            entries,
            elapsed_ms: 0,
        }
    }

    fn tick(&mut self, dt_ms: u32) {
        self.elapsed_ms = self.elapsed_ms.wrapping_add(dt_ms);
    }

    /// Given a static sprite index, return the animated sprite index.
    fn animate(&self, sprite_idx: u16) -> u16 {
        let idx = sprite_idx as i32;
        // Find which building owns this sprite via binary search
        let pos = self.entries.partition_point(|e| e.base_gfx <= idx);
        if pos == 0 {
            return sprite_idx;
        }
        let entry = &self.entries[pos - 1];
        // Check if this sprite is within the building's sprite range
        if idx >= entry.base_gfx && idx < entry.base_gfx + entry.total_sprites {
            let offset_from_base = idx - entry.base_gfx;
            // Which rotation variant is this tile in?
            let sprites_per_rotation = entry.anim_anz * entry.anim_add;
            if sprites_per_rotation <= 0 {
                return sprite_idx;
            }
            let rotation_offset = offset_from_base % sprites_per_rotation;
            let rotation_base = idx - rotation_offset;
            // The tile's position within the rotation (which sub-tile)
            let tile_in_frame = rotation_offset % entry.anim_add;
            // Current animation frame based on time
            let frame = ((self.elapsed_ms / entry.anim_time as u32) % entry.anim_anz as u32) as i32;
            let animated = rotation_base + frame * entry.anim_add + tile_in_frame;
            animated as u16
        } else {
            sprite_idx
        }
    }
}

/// Networking role chosen at startup.
enum NetRole {
    Solo,
    Host { port: u16 },
    Client { addr: String },
}

fn net_role_port(role: &NetRole) -> u16 {
    if let NetRole::Host { port } = role { *port } else { 0 }
}

/// Tiny 4x5 bitmap font for HUD rendering (ASCII 32-127).
/// Each character is a u32 bitmask: 4 columns × 5 rows, bit 0 = top-left.
mod tiny_font {
    const CHAR_W: u32 = 4;
    const CHAR_H: u32 = 5;

    /// Bitmap glyphs for ASCII 32-90 (space through Z). Others fallback to '?'.
    const GLYPHS: &[(u8, u32)] = &[
        (b' ', 0x00000),
        (b'0', 0x69BD6), (b'1', 0x46224), (b'2', 0x69246), (b'3', 0x69496),
        (b'4', 0x99F11), (b'5', 0xF8E1E), (b'6', 0x68E96), (b'7', 0xF1244),
        (b'8', 0x69696), (b'9', 0x69716), (b':', 0x04040),
        (b'A', 0x69F99), (b'B', 0xE9E9E), (b'C', 0x78867), (b'D', 0xE9996 + 1 - 1),
        (b'E', 0xF8E8F), (b'F', 0xF8E88), (b'G', 0x78B97), (b'H', 0x99F99),
        (b'I', 0xE444E), (b'J', 0x11196), (b'K', 0x9ACA9), (b'L', 0x8888F),
        (b'M', 0x9F999), (b'N', 0x9DB99), (b'O', 0x69996), (b'P', 0xE9E88),
        (b'Q', 0x699A7), (b'R', 0xE9EA9), (b'S', 0x78617), (b'T', 0xF4444),
        (b'U', 0x99996), (b'V', 0x9996A + 1 - 1), (b'W', 0x999F9), (b'X', 0x96699),
        (b'Y', 0x99644), (b'Z', 0xF1248 + 7),
        (b'a', 0x06996), (b'b', 0x8E996 + 1 - 1), (b'c', 0x07896 + 1 - 1),
        (b'd', 0x17996 + 1 - 1), (b'e', 0x06F87),
        (b'%', 0x91249), (b'+', 0x04E40), (b'-', 0x00E00), (b'/', 0x11248),
        (b'.', 0x00004), (b',', 0x00024), (b'=', 0x0E0E0), (b'?', 0x69240),
        (b'(', 0x24842), (b')', 0x42124), (b'|', 0x44444), (b'x', 0x09690),
    ];

    fn glyph(ch: u8) -> u32 {
        for &(c, g) in GLYPHS {
            if c == ch {
                return g;
            }
        }
        // Uppercase fallback
        if ch >= b'a' && ch <= b'z' {
            let upper = ch - 32;
            for &(c, g) in GLYPHS {
                if c == upper {
                    return g;
                }
            }
        }
        0x69240 // '?' fallback
    }

    /// Draw a string onto an RGBA buffer. Returns width in pixels consumed.
    pub fn draw_str(
        buf: &mut [u8], buf_w: u32, buf_h: u32,
        x: i32, y: i32, text: &str,
        color: [u8; 4], scale: u32,
    ) {
        let mut cx = x;
        for &ch in text.as_bytes() {
            let g = glyph(ch);
            for row in 0..CHAR_H {
                for col in 0..CHAR_W {
                    let bit = row * CHAR_W + col;
                    if (g >> bit) & 1 != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = cx + (col * scale + sx) as i32;
                                let py = y + (row * scale + sy) as i32;
                                if px >= 0 && py >= 0
                                    && (px as u32) < buf_w
                                    && (py as u32) < buf_h
                                {
                                    let off = ((py as u32 * buf_w + px as u32) * 4) as usize;
                                    if off + 3 < buf.len() {
                                        let a = color[3] as u16;
                                        let inv_a = 255 - a;
                                        buf[off] = ((color[0] as u16 * a + buf[off] as u16 * inv_a) / 255) as u8;
                                        buf[off+1] = ((color[1] as u16 * a + buf[off+1] as u16 * inv_a) / 255) as u8;
                                        buf[off+2] = ((color[2] as u16 * a + buf[off+2] as u16 * inv_a) / 255) as u8;
                                        buf[off+3] = 255;
                                    }
                                }
                            }
                        }
                    }
                }
                cx += 0; // row loop doesn't advance cursor
            }
            cx += (CHAR_W * scale + scale) as i32; // advance by char width + 1px spacing
        }
    }

    /// Measure text width in pixels.
    pub fn measure(text: &str, scale: u32) -> u32 {
        if text.is_empty() { return 0; }
        let chars = text.len() as u32;
        chars * (CHAR_W * scale + scale) - scale
    }
}

/// A building type available for placement.
struct BuildableBuilding {
    /// Index into building_defs / cod.buildings.
    def_idx: usize,
    /// Display name for the UI.
    name: String,
    /// Sprite index (gfx from COD) for rendering.
    sprite_idx: usize,
    /// Category bucket (drives the palette tabs).
    category: BuildCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum BuildCategory {
    Production = 0,
    Residence  = 1,
    Service    = 2,
    Military   = 3,
    Special    = 4,
}

impl BuildCategory {
    const ALL: [BuildCategory; 5] = [
        BuildCategory::Production,
        BuildCategory::Residence,
        BuildCategory::Service,
        BuildCategory::Military,
        BuildCategory::Special,
    ];

    fn label(self) -> &'static str {
        match self {
            BuildCategory::Production => "PROD",
            BuildCategory::Residence  => "RES",
            BuildCategory::Service    => "SVC",
            BuildCategory::Military   => "MIL",
            BuildCategory::Special    => "SPC",
        }
    }

    fn from_def(def: &anno_sim::building::BuildingDef) -> Self {
        let pk = def.prod_kind.as_str();
        if matches!(pk, "MARKT" | "KIRCHE" | "KAPELLE" | "SCHULE" | "HOCHSCHULE"
            | "WIRT" | "THEATER" | "ARZT" | "BADEHAUS" | "GALGEN" | "KLINIK")
        {
            return BuildCategory::Service;
        }
        if matches!(pk, "MILITAR") {
            return BuildCategory::Military;
        }
        if matches!(pk, "KONTOR") || def.kind.as_str() == "KONTOR"
            || def.kind.as_str() == "HQ"
        {
            return BuildCategory::Special;
        }
        if def.kind.as_str() == "WOHN" || matches!(pk, "WOHN") {
            return BuildCategory::Residence;
        }
        BuildCategory::Production
    }
}

/// Building placement state machine.
struct BuildingPlacer {
    active: bool,
    /// Available buildings to place.
    buildable: Vec<BuildableBuilding>,
    /// Currently selected building index (into buildable vec).
    selected: usize,
    /// Current page of buildings (9 per page) within the active category.
    page: usize,
    /// Active category tab.
    category: BuildCategory,
    /// Rotation slot 0..rotate-1 picked at placement time.
    orientation: u8,
    /// Tile coordinates under the mouse cursor (if valid).
    hover_tile: Option<(i32, i32)>,
}

impl BuildingPlacer {
    fn new(cod: &CodFile, defs: &[anno_sim::building::BuildingDef]) -> Self {
        let mut buildable = Vec::new();

        for (i, cod_b) in cod.buildings.iter().enumerate() {
            if i >= defs.len() {
                break;
            }
            let def = &defs[i];

            // Only allow placing actual buildings (not terrain, decorations, etc.)
            let dominated_kind = match def.kind.as_str() {
                "GEBAEUDE" | "HQ" => true,
                _ => false,
            };
            let has_production = def.output_good != Good::None;
            let is_service = matches!(
                def.prod_kind.as_str(),
                "MARKT" | "KIRCHE" | "KAPELLE" | "SCHULE" | "WIRT" | "THEATER" | "ARZT"
                    | "BADEHAUS" | "GALGEN"
            );
            let is_military = matches!(def.prod_kind.as_str(), "MILITAR");
            let is_kontor = def.kind.as_str() == "KONTOR" || def.prod_kind.as_str() == "KONTOR";

            if !dominated_kind && !has_production && !is_service && !is_military && !is_kontor {
                continue;
            }

            // Must have a valid sprite
            if cod_b.gfx < 0 {
                continue;
            }

            // Must have a size > 0
            if def.width == 0 || def.height == 0 {
                continue;
            }

            let name = cod_b
                .properties
                .get("Name")
                .cloned()
                .unwrap_or_else(|| format!("Building #{}", cod_b.nummer));

            buildable.push(BuildableBuilding {
                def_idx: i,
                name,
                sprite_idx: cod_b.gfx as usize,
                category: BuildCategory::from_def(def),
            });
        }

        let initial_selected = buildable
            .iter()
            .position(|b| b.category == BuildCategory::Production)
            .unwrap_or(0);
        Self {
            active: false,
            buildable,
            selected: initial_selected,
            page: 0,
            category: BuildCategory::Production,
            orientation: 0,
            hover_tile: None,
        }
    }

    fn category_indices(&self) -> Vec<usize> {
        self.buildable.iter().enumerate()
            .filter(|(_, b)| b.category == self.category)
            .map(|(i, _)| i)
            .collect()
    }

    fn next_category(&mut self) {
        let cats = BuildCategory::ALL;
        let idx = cats.iter().position(|c| *c == self.category).unwrap_or(0);
        self.category = cats[(idx + 1) % cats.len()];
        self.page = 0;
        if let Some(&first) = self.category_indices().first() {
            self.selected = first;
        }
    }

    fn prev_category(&mut self) {
        let cats = BuildCategory::ALL;
        let idx = cats.iter().position(|c| *c == self.category).unwrap_or(0);
        self.category = cats[(idx + cats.len() - 1) % cats.len()];
        self.page = 0;
        if let Some(&first) = self.category_indices().first() {
            self.selected = first;
        }
    }

    fn toggle(&mut self) {
        self.active = !self.active;
        if self.active {
            self.hover_tile = None;
        }
    }

    fn selected_building(&self) -> Option<&BuildableBuilding> {
        if !self.active || self.buildable.is_empty() {
            return None;
        }
        self.buildable.get(self.selected)
    }

    /// Indices into `buildable` for the current category page (up to 9).
    fn page_index_slice(&self) -> Vec<usize> {
        let cat = self.category_indices();
        let start = self.page * 9;
        let end = (start + 9).min(cat.len());
        if start >= cat.len() { Vec::new() } else { cat[start..end].to_vec() }
    }

    /// Convenience: borrowed view of the items on this page.
    fn page_items(&self) -> Vec<&BuildableBuilding> {
        self.page_index_slice()
            .into_iter()
            .map(|i| &self.buildable[i])
            .collect()
    }

    fn select_on_page(&mut self, slot: usize) {
        let cat = self.category_indices();
        let idx = self.page * 9 + slot;
        if let Some(&buildable_idx) = cat.get(idx) {
            self.selected = buildable_idx;
            // Reset orientation; the new building may have a different
            // rotation count, so the previous index could overflow.
            self.orientation = 0;
        }
    }

    fn page_count(&self) -> usize {
        let n = self.category_indices().len();
        if n == 0 { 0 } else { (n - 1) / 9 + 1 }
    }

    fn next_page(&mut self) {
        let max_page = self.page_count().saturating_sub(1);
        self.page = (self.page + 1).min(max_page);
    }

    fn prev_page(&mut self) {
        self.page = self.page.saturating_sub(1);
    }
}

/// Convert screen pixel coordinates to isometric tile coordinates.
/// Returns (tile_x, tile_y) relative to the island origin.
fn screen_to_tile(
    screen_x: i32,
    screen_y: i32,
    origin_x: i32,
    origin_y: i32,
    tile_w: i32,
    tile_h: i32,
) -> (i32, i32) {
    // Inverse of: sx = origin_x + (tx - ty) * half_tw
    //             sy = origin_y + (tx + ty) * half_th
    // So: tx - ty = (sx - origin_x) / half_tw
    //     tx + ty = (sy - origin_y) / half_th
    // => tx = ((sx - origin_x) / half_tw + (sy - origin_y) / half_th) / 2
    //    ty = ((sy - origin_y) / half_th - (sx - origin_x) / half_tw) / 2
    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;
    if half_tw == 0 || half_th == 0 {
        return (0, 0);
    }

    // Use fixed-point to avoid rounding issues
    let dx = screen_x - origin_x;
    let dy = screen_y - origin_y;

    // Multiply through to avoid division until the end
    let sum = dx * half_th + dy * half_tw; // proportional to (tx - ty + tx + ty) = 2*tx
    let diff = dy * half_tw - dx * half_th; // proportional to (tx + ty - tx + ty) = 2*ty

    let denom = 2 * half_tw * half_th;

    // Round toward nearest tile
    let tx = if sum >= 0 {
        (sum + denom / 2) / denom
    } else {
        (sum - denom / 2) / denom
    };
    let ty = if diff >= 0 {
        (diff + denom / 2) / denom
    } else {
        (diff - denom / 2) / denom
    };

    (tx, ty)
}

/// Check if a building can be placed at the given tile position on an island.
fn can_place_building(
    island: &Island,
    island_map: &IslandMap,
    tile_x: i32,
    tile_y: i32,
    width: u8,
    height: u8,
) -> bool {
    // Check all tiles in the footprint
    for dy in 0..height as i32 {
        for dx in 0..width as i32 {
            let tx = tile_x + dx;
            let ty = tile_y + dy;

            // Must be within island bounds
            if tx < 0 || ty < 0 || tx >= island.width as i32 || ty >= island.height as i32 {
                return false;
            }

            // Must be on walkable terrain (not water or existing building)
            if !island_map.is_walkable(tx, ty) {
                return false;
            }
        }
    }
    true
}

fn main() {
    let base_dir = find_data_dir();

    // Load palette
    let col_data =
        std::fs::read(base_dir.join("TOOLGFX/STADTFLD.COL")).expect("Failed to read STADTFLD.COL");
    let palette = parse_col(&col_data).expect("Failed to parse palette");

    // Load sprites
    println!("Loading sprites...");
    let sprite_mgr = SpriteManager::load_from_dir(&base_dir);
    let sprites_by_zoom: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_sprites(&sprite_mgr, z, &palette))
        .collect();
    for (z, sprites) in sprites_by_zoom.iter().enumerate() {
        let label = ["GFX", "MGFX", "SGFX"][z];
        println!("  {label}: {} decoded sprites", sprites.len());
    }

    // Decode entity sprite sets (carriers, soldiers, ships)
    let carrier_sprites: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_entity_sprites(&sprite_mgr, SpriteCategory::Traeger, z, &palette))
        .collect();
    let ship_sprites: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_entity_sprites(&sprite_mgr, SpriteCategory::Ship, z, &palette))
        .collect();
    let soldier_sprites: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_entity_sprites(&sprite_mgr, SpriteCategory::Soldat, z, &palette))
        .collect();
    println!(
        "Entity sprites: carriers={} ships={} soldiers={}",
        carrier_sprites[0].len(),
        ship_sprites[0].len(),
        soldier_sprites[0].len(),
    );

    // Load building definitions
    let cod_data =
        std::fs::read(base_dir.join("haeuser.cod")).expect("Failed to read haeuser.cod");
    let cod = CodFile::parse(&cod_data).expect("Failed to parse COD");
    let defs = data_bridge::load_building_defs(&cod);
    println!("Loaded {} building definitions", defs.len());

    // Load figure definitions (carriers, ships, soldiers, …) so sprite
    // indexing can use the real per-figure (gfx, rotate, anim) layout
    // instead of a one-size-fits-all heuristic.
    let figures_path = base_dir.join("figuren.cod");
    let figures = match std::fs::read(&figures_path) {
        Ok(bytes) => {
            let f = anno_formats::figuren::FiguresFile::parse(&bytes);
            println!(
                "Loaded {} figure definitions from {}",
                f.figures.len(),
                figures_path.display(),
            );
            f
        }
        Err(e) => {
            eprintln!("(figuren.cod not loaded: {e}) — using heuristic sprite indexing");
            anno_formats::figuren::FiguresFile {
                constants: Default::default(),
                figures: Vec::new(),
            }
        }
    };
    // Pull the layout bits we'll use during render. Falls back to the old
    // heuristic values if the figure can't be found.
    let traeger_def = figures.find("TRAEGER").cloned();
    let handel1_def = figures.find("HANDEL1").cloned();
    let carrier_walk_anz = traeger_def
        .as_ref()
        .and_then(|f| f.walk_anim())
        .map(|a| a.anim_anz as usize)
        .unwrap_or(8);
    let ship_walk_anz = handel1_def
        .as_ref()
        .and_then(|f| f.walk_anim())
        .map(|a| a.anim_anz as usize)
        .unwrap_or(40);
    let _ = &figures;

    // Parse CLI: positional scenario path + optional --host PORT / --join HOST:PORT
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let mut net_role: NetRole = NetRole::Solo;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw_args.len() {
        let a = &raw_args[i];
        if a == "--host" {
            i += 1;
            let port: u16 = raw_args.get(i)
                .and_then(|p| p.parse().ok())
                .expect("--host needs a port number");
            net_role = NetRole::Host { port };
        } else if a == "--join" {
            i += 1;
            let addr = raw_args.get(i)
                .cloned()
                .expect("--join needs HOST:PORT");
            net_role = NetRole::Client { addr };
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }

    // Load scenario
    let scenario_path = positional.into_iter().next().unwrap_or_else(|| {
        let szenes = base_dir.join("Szenes");
        let mut entries: Vec<_> = std::fs::read_dir(&szenes)
            .expect("Failed to read Szenes/")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".szs"))
            .collect();
        entries.sort_by_key(|e| e.file_name());
        entries
            .first()
            .map(|e| e.path().to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                eprintln!("No .szs files found");
                std::process::exit(1);
            })
    });

    let szs_data = std::fs::read(&scenario_path).expect("Failed to read scenario");
    let szs = SzsFile::parse(&szs_data).expect("Failed to parse scenario");
    let scenario_name = std::path::Path::new(&scenario_path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    println!(
        "Loaded scenario '{}': {} islands",
        scenario_name,
        szs.islands.len()
    );

    // Initialize simulation
    let mut sim = init_simulation(&szs, &cod, &defs);
    println!(
        "Simulation initialized: {} buildings, {} warehouses, {} island maps",
        sim.buildings.len(),
        sim.warehouses.len(),
        sim.island_maps.len()
    );

    // Initialize building placer
    let mut placer = BuildingPlacer::new(&cod, &defs);
    println!("Building placer: {} buildable types", placer.buildable.len());

    // Initialize animation state
    let mut anim_state = AnimationState::new(&cod);
    let mut last_anim_gen: u32 = 0; // tracks when animation frames change

    // Mutable copy of islands for adding placed building tiles
    let mut islands = szs.islands.clone();

    // Initialize audio engine
    let audio_dirs = vec![
        base_dir.join("MUSIC8"),
        base_dir.join("SPEECH8"),
        base_dir.clone(),
    ];
    let mut audio = AudioEngine::new(audio_dirs);
    audio.set_screen_size(WINDOW_W, WINDOW_H);

    // Discover music tracks
    let music_dir = base_dir.join("MUSIC8");
    let mut music_files: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&music_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_lowercase().ends_with(".wav") {
                music_files.push(name);
            }
        }
    }
    music_files.sort();
    println!("Found {} music tracks", music_files.len());

    // Load and start first music track
    let mut music_enabled = true;
    let mut music_volume: f32 = 0.4;
    let mut current_track: usize = 0;
    let mut music_slot: Option<usize> = None;

    if !music_files.is_empty() {
        if let Some(slot) = audio.streams.create(&music_files[0], 0) {
            if let Some(ref handle) = audio.stream_handle {
                audio.streams.play(slot, music_volume, handle);
                println!("Playing: {}", music_files[0]);
            }
            music_slot = Some(slot);
        }
    }

    // Load sound effects. Slots are reused from SPEECH8/MUSIC8 pools.
    let place_sound_slot = audio.waves.load("SPEECH8/1000.WAV")
        .or_else(|| audio.waves.load("1000.WAV"));
    let event_destroy_slot = audio.waves.load("SPEECH8/1010.WAV")
        .or_else(|| audio.waves.load("1010.WAV"))
        .or_else(|| audio.waves.load("SPEECH8/1000.WAV"));
    let event_obj_done_slot = audio.waves.load("SPEECH8/1020.WAV")
        .or_else(|| audio.waves.load("1020.WAV"))
        .or_else(|| audio.waves.load("SPEECH8/1000.WAV"));
    let event_war_slot = audio.waves.load("SPEECH8/1030.WAV")
        .or_else(|| audio.waves.load("1030.WAV"))
        .or_else(|| audio.waves.load("SPEECH8/1000.WAV"));

    // SDL2 setup
    let sdl = sdl2::init().expect("SDL2 init failed");
    let video = sdl.video().expect("SDL2 video init failed");

    let window = video
        .window("Anno 1602 — Game", WINDOW_W, WINDOW_H)
        .position_centered()
        .resizable()
        .build()
        .expect("window creation failed");

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .build()
        .expect("canvas creation failed");

    let texture_creator = canvas.texture_creator();
    let mut event_pump = sdl.event_pump().expect("event pump failed");

    let mut current_island: usize = 0;
    let mut scroll_x: i32 = 0;
    let mut scroll_y: i32 = 0;
    let mut display_zoom: i32 = 1;
    let mut sprite_zoom: usize = 0;
    let mut needs_redraw = true;
    let mut world_mode = false;
    let mut dragging = false;
    let mut drag_start = (0i32, 0i32);

    let mut rendered: Option<RenderState> = None;

    let timer = sdl.timer().expect("timer init failed");
    let mut last_tick = timer.ticks();

    // Networking setup
    let mut net_host: Option<anno_net::transport::NetHost> = None;
    let mut net_client: Option<anno_net::transport::NetClient> = None;
    let mut net_status = String::from("solo");
    let mut last_broadcast_ms: u32 = 0;
    let broadcast_interval_ms: u32 = 1000;
    match net_role {
        NetRole::Host { port } => {
            let addr: std::net::SocketAddr =
                format!("0.0.0.0:{port}").parse().unwrap();
            match anno_net::transport::NetHost::bind(addr, "anno-game") {
                Ok(h) => {
                    net_host = Some(h);
                    net_status = format!("HOST :{port}");
                    println!("Hosting on port {port}");
                }
                Err(e) => {
                    eprintln!("Failed to host on {port}: {e}");
                    std::process::exit(1);
                }
            }
        }
        NetRole::Client { ref addr } => {
            let parsed: std::net::SocketAddr = addr.parse().unwrap_or_else(|e| {
                eprintln!("Bad --join addr '{addr}': {e}");
                std::process::exit(1);
            });
            match anno_net::transport::NetClient::connect(parsed, "anno-game") {
                Ok(c) => {
                    net_client = Some(c);
                    net_status = format!("CLIENT → {addr}");
                    sim.paused = true; // client doesn't tick locally
                    println!("Connected to {addr} (waiting for state…)");
                }
                Err(e) => {
                    eprintln!("Failed to connect to {addr}: {e}");
                    std::process::exit(1);
                }
            }
        }
        NetRole::Solo => {}
    }

    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut minimap_clicked = false;
    let mut minimap_click_x: i32 = 0;
    let mut minimap_click_y: i32 = 0;

    /// Inspection state — what the player has right-clicked on.
    struct Inspection {
        /// Tile coordinates of the inspected location.
        tile_x: i32,
        tile_y: i32,
        /// Building instance index in sim.buildings (if any).
        building_idx: Option<usize>,
        /// Warehouse index in sim.warehouses (if any).
        warehouse_idx: Option<usize>,
        /// Description lines for the title bar.
        info: String,
    }
    let mut inspection: Option<Inspection> = None;
    let mut show_hud = true;
    let mut demolish_mode = false;
    let mut demolish_hover: Option<usize> = None; // building index under cursor
    let mut tax_panel = false;
    let mut tax_tier: usize = 0; // selected tier for tax adjustment
    let mut diplomacy_panel = false;
    let mut diplomacy_target: u8 = 1; // selected counterpart (1..6) for player 0
    let mut graph_panel = false;
    let mut prod_panel = false;
    let mut roster_panel = false;
    let mut market_panel = false;
    let mut market_sel: usize = 0;
    let mut show_paths = false;
    let mut route_list_panel = false;
    let mut route_list_sel: usize = 0;
    let mut wh_panel = false;
    let mut ship_panel = false;
    let mut obj_panel = false;
    let mut settings = anno_sim::settings::Settings::load_default();
    let mut settings_panel = false;
    let mut settings_sel: usize = 0;
    let mut show_perf = false;
    let mut perf_history: std::collections::VecDeque<(u32, u32, u32)>
        = std::collections::VecDeque::with_capacity(60);
    let mut frame_started = std::time::Instant::now();
    /// Right-click context menu: position + tile + action list.
    struct ContextMenu {
        screen_x: i32,
        screen_y: i32,
        tile_x: i32,
        tile_y: i32,
        actions: Vec<&'static str>,
        sel: usize,
    }
    let mut context_menu: Option<ContextMenu> = None;
    // Auto-pause-while-menu-open: when any modal panel is opened we
    // pause the sim so the player can read it; we only unpause on close
    // if we were the ones who paused (i.e. the player didn't manually
    // unpause via Space while reading).
    let mut prev_modal_open = false;
    let mut auto_paused = false;
    let mut scenario_picker = false;
    let mut scenario_sel: usize = 0;
    // Scan Szenes/ once at startup so the picker is populated.
    let scenario_files: Vec<std::path::PathBuf> = {
        let szenes = base_dir.join("Szenes");
        let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(&szenes)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with(".szs")
            })
            .map(|e| e.path())
            .collect();
        v.sort();
        v
    };
    if let Some(idx) = scenario_files
        .iter()
        .position(|p| p.to_string_lossy() == scenario_path)
    {
        scenario_sel = idx;
    }
    let mut chat_active = false;
    let mut chat_input = String::new();
    // Recently received chat lines (oldest first) with timestamp for TTL.
    let mut chat_log: std::collections::VecDeque<(String, std::time::Instant)> =
        std::collections::VecDeque::new();

    // Snapshots used to derive in-game event notifications (diplomacy
    // flips, AI building completions). First-tick deltas are absorbed by
    // initialising from the live state below.
    use anno_sim::combat::Diplomacy;
    let mut prev_diplomacy: [[Diplomacy; 7]; 7] = [[Diplomacy::Neutral; 7]; 7];
    for i in 0..7u8 {
        for j in 0..7u8 {
            prev_diplomacy[i as usize][j as usize] = sim.diplomacy.get(i, j);
        }
    }
    let mut prev_building_counts: [usize; 7] = [0; 7];
    for b in &sim.buildings {
        let o = b.owner as usize;
        if o < prev_building_counts.len() {
            prev_building_counts[o] += 1;
        }
    }
    // Trade route editor: while in this mode, LMB on a warehouse adds it
    // as a stop in the draft route; Enter commits, Esc cancels.
    let mut trade_route_mode = false;
    /// (island_id, x, y, mode) where mode: 0=LOAD only, 1=UNLOAD only, 2=BOTH.
    let mut draft_route_stops: Vec<(u8, u16, u16, u8)> = Vec::new();
    let mut next_route_id: u16 = sim
        .trade_routes
        .iter()
        .map(|r| r.id)
        .max()
        .map(|m| m + 1)
        .unwrap_or(1);
    let mut show_coverage = false;
    let mut selected_units: Vec<usize> = Vec::new();
    let mut shift_held = false;
    let mut save_banner: Option<(String, std::time::Instant)> = None;
    let save_dir = std::path::PathBuf::from("saves");
    let quicksave_path = save_dir.join(format!("{}.quicksave.bin", scenario_name));
    let mut save_panel = false;
    let mut save_sel: usize = 0;
    let slot_path = |slot: usize| -> std::path::PathBuf {
        save_dir.join(format!("{}.slot{}.bin", scenario_name, slot))
    };

    'main: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'main,

                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => {
                    if chat_active {
                        chat_active = false;
                        chat_input.clear();
                    } else if context_menu.is_some() {
                        context_menu = None;
                    } else if route_list_panel {
                        route_list_panel = false;
                    } else if scenario_picker {
                        scenario_picker = false;
                    } else if save_panel {
                        save_panel = false;
                    } else if settings_panel {
                        settings_panel = false;
                    } else if market_panel {
                        market_panel = false;
                    } else if wh_panel {
                        wh_panel = false;
                    } else if ship_panel {
                        ship_panel = false;
                    } else if obj_panel {
                        obj_panel = false;
                    } else if placer.active {
                        placer.active = false;
                    } else if demolish_mode {
                        demolish_mode = false;
                    } else if graph_panel {
                        graph_panel = false;
                    } else if prod_panel {
                        prod_panel = false;
                    } else if roster_panel {
                        roster_panel = false;
                    } else if trade_route_mode {
                        trade_route_mode = false;
                        draft_route_stops.clear();
                    } else if !selected_units.is_empty() {
                        selected_units.clear();
                    } else if inspection.is_some() {
                        inspection = None;
                    } else {
                        break 'main;
                    }
                }

                Event::KeyDown {
                    keycode: Some(key), ..
                } => {
                    if matches!(key, Keycode::LShift | Keycode::RShift) {
                        shift_held = true;
                    }
                    if route_list_panel {
                        let routes: Vec<u16> = sim.trade_routes.iter()
                            .filter(|r| r.owner == 0)
                            .map(|r| r.id)
                            .collect();
                        match key {
                            Keycode::Up => {
                                if route_list_sel > 0 { route_list_sel -= 1; }
                            }
                            Keycode::Down => {
                                if route_list_sel + 1 < routes.len() {
                                    route_list_sel += 1;
                                }
                            }
                            Keycode::Backspace | Keycode::Delete => {
                                if let Some(&rid) = routes.get(route_list_sel) {
                                    sim.trade_routes.retain(|r| r.id != rid);
                                    sim.trade_ships.retain(|s| s.route_id != rid);
                                    if route_list_sel > 0 { route_list_sel -= 1; }
                                }
                            }
                            Keycode::R if shift_held => { route_list_panel = false; }
                            Keycode::Escape => { route_list_panel = false; }
                            _ => {}
                        }
                        continue;
                    }
                    if let Some(menu) = context_menu.as_mut() {
                        match key {
                            Keycode::Up => {
                                if menu.sel > 0 { menu.sel -= 1; }
                            }
                            Keycode::Down => {
                                if menu.sel + 1 < menu.actions.len() {
                                    menu.sel += 1;
                                }
                            }
                            Keycode::Return | Keycode::KpEnter => {
                                let act = menu.actions[menu.sel];
                                let (tx, ty) = (menu.tile_x, menu.tile_y);
                                context_menu = None;
                                match act {
                                    "Inspect" => {
                                        inspection = Some(Inspection {
                                            tile_x: tx, tile_y: ty,
                                            building_idx: None,
                                            warehouse_idx: None,
                                            info: format!("Tile ({tx},{ty})"),
                                        });
                                    }
                                    "Move selected here" => {
                                        for &ui in &selected_units {
                                            if let Some(u) = sim.military_units.get_mut(ui) {
                                                if u.is_alive() {
                                                    u.target_x = tx;
                                                    u.target_y = ty;
                                                    u.combat_target = -1;
                                                    u.move_timer_ms = 0;
                                                }
                                            }
                                        }
                                    }
                                    "Demolish" => {
                                        demolish_mode = true;
                                    }
                                    _ => {}
                                }
                            }
                            Keycode::Escape => { context_menu = None; }
                            _ => {}
                        }
                        continue;
                    }
                    if settings_panel {
                        match key {
                            Keycode::Up => {
                                if settings_sel > 0 { settings_sel -= 1; }
                            }
                            Keycode::Down => {
                                if settings_sel + 1 < anno_sim::settings::Settings::COUNT {
                                    settings_sel += 1;
                                }
                            }
                            Keycode::Left => {
                                settings.adjust(settings_sel, -5);
                                let _ = settings.save_default();
                            }
                            Keycode::Right => {
                                settings.adjust(settings_sel, 5);
                                let _ = settings.save_default();
                            }
                            Keycode::F10 | Keycode::Escape => {
                                settings_panel = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if save_panel {
                        match key {
                            Keycode::Up => {
                                if save_sel > 0 { save_sel -= 1; }
                            }
                            Keycode::Down => {
                                if save_sel + 1 < 10 { save_sel += 1; }
                            }
                            Keycode::S => {
                                let path = slot_path(save_sel);
                                let snap = sim.snapshot();
                                let msg = match anno_sim::save::save_to_file(&path, &snap) {
                                    Ok(()) => format!(
                                        "saved slot {} → {}",
                                        save_sel,
                                        path.display(),
                                    ),
                                    Err(e) => format!("save FAILED: {e}"),
                                };
                                println!("{msg}");
                                save_banner = Some((msg, std::time::Instant::now()));
                            }
                            Keycode::L => {
                                let path = slot_path(save_sel);
                                let msg = match anno_sim::save::load_from_file(&path) {
                                    Ok(state) => {
                                        let bldgs = state.buildings.len();
                                        let gold = state.players.first()
                                            .map(|p| p.gold).unwrap_or(0);
                                        sim.apply_snapshot(state);
                                        needs_redraw = true;
                                        format!(
                                            "loaded slot {} ({} bldg, {} gold)",
                                            save_sel, bldgs, gold,
                                        )
                                    }
                                    Err(e) => format!("load FAILED: {e}"),
                                };
                                println!("{msg}");
                                save_banner = Some((msg, std::time::Instant::now()));
                                save_panel = false;
                            }
                            Keycode::F3 | Keycode::Escape => {
                                save_panel = false;
                            }
                            Keycode::Space => {
                                sim.paused = !sim.paused;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if market_panel {
                        const GOODS: &[Good] = &[
                            Good::Wood, Good::Iron, Good::Ore, Good::Gold,
                            Good::Wool, Good::Sugar, Good::Tobacco, Good::Cattle,
                            Good::Grain, Good::Flour, Good::Food, Good::Alcohol,
                            Good::Cloth, Good::Clothing, Good::Jewelry, Good::Tools,
                            Good::Bricks, Good::Swords, Good::Cannons, Good::Muskets,
                            Good::Stone, Good::Cocoa, Good::Spices, Good::Hides,
                            Good::Cotton, Good::Silk, Good::Fish, Good::Grapes,
                            Good::GoldOre, Good::TobaccoProducts,
                        ];
                        match key {
                            Keycode::Up => {
                                if market_sel > 0 { market_sel -= 1; }
                            }
                            Keycode::Down => {
                                if market_sel + 1 < GOODS.len() {
                                    market_sel += 1;
                                }
                            }
                            Keycode::Right => {
                                // Sell selected good. Plain Right=10,
                                // Shift+Right=100. Routed through
                                // apply_command so it uses live market
                                // prices and works in multiplayer.
                                if let Some(g) = GOODS.get(market_sel).copied() {
                                    let qty = if shift_held { 100 } else { 10 };
                                    sim.apply_command(
                                        &anno_sim::commands::Command::Sell {
                                            player: 0, good: g, qty,
                                        },
                                    );
                                }
                            }
                            Keycode::Left => {
                                if let Some(g) = GOODS.get(market_sel).copied() {
                                    let qty = if shift_held { 100 } else { 10 };
                                    sim.apply_command(
                                        &anno_sim::commands::Command::Buy {
                                            player: 0, good: g, qty,
                                        },
                                    );
                                }
                            }
                            Keycode::A | Keycode::Escape => {
                                market_panel = false;
                            }
                            Keycode::Space => {
                                sim.paused = !sim.paused;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if scenario_picker {
                        match key {
                            Keycode::Up => {
                                if scenario_sel > 0 { scenario_sel -= 1; }
                            }
                            Keycode::Down => {
                                if scenario_sel + 1 < scenario_files.len() {
                                    scenario_sel += 1;
                                }
                            }
                            Keycode::Return | Keycode::KpEnter => {
                                if let Some(path) = scenario_files.get(scenario_sel) {
                                    // Re-exec ourselves with the chosen scenario.
                                    if let Ok(exe) = std::env::current_exe() {
                                        let _ = std::process::Command::new(exe)
                                            .arg(path)
                                            .spawn();
                                        println!("Launching {}", path.display());
                                        std::process::exit(0);
                                    }
                                }
                                scenario_picker = false;
                            }
                            Keycode::F2 | Keycode::Escape => {
                                scenario_picker = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if chat_active {
                        // While the chat input is open, swallow keys we
                        // care about and drop the rest (the typed text
                        // arrives via TextInput events).
                        match key {
                            Keycode::Backspace => { chat_input.pop(); }
                            Keycode::Return | Keycode::KpEnter => {
                                let text = chat_input.trim().to_string();
                                if !text.is_empty() {
                                    let local_line = format!("you: {text}");
                                    chat_log.push_back((local_line, std::time::Instant::now()));
                                    if chat_log.len() > 8 { chat_log.pop_front(); }
                                    let msg = anno_net::protocol::NetMessage::chat(&text);
                                    if let Some(host) = net_host.as_mut() {
                                        host.send_to_all(&msg);
                                    } else if let Some(client) = net_client.as_mut() {
                                        let _ = client.send(&msg);
                                    }
                                }
                                chat_active = false;
                                chat_input.clear();
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if placer.active {
                        // Build mode keys
                        match key {
                            Keycode::Num1 => placer.select_on_page(0),
                            Keycode::Num2 => placer.select_on_page(1),
                            Keycode::Num3 => placer.select_on_page(2),
                            Keycode::Num4 => placer.select_on_page(3),
                            Keycode::Num5 => placer.select_on_page(4),
                            Keycode::Num6 => placer.select_on_page(5),
                            Keycode::Num7 => placer.select_on_page(6),
                            Keycode::Num8 => placer.select_on_page(7),
                            Keycode::Num9 => placer.select_on_page(8),
                            Keycode::PageUp => placer.prev_page(),
                            Keycode::PageDown => placer.next_page(),
                            Keycode::LeftBracket => placer.prev_category(),
                            Keycode::RightBracket => placer.next_category(),
                            Keycode::Z => {
                                // Cycle rotation through the selected
                                // building's configured Rotate count.
                                if let Some(b) = placer.selected_building() {
                                    let rot = cod.buildings[b.def_idx].rotate.max(1) as u8;
                                    placer.orientation = (placer.orientation + 1) % rot;
                                }
                            }
                            Keycode::B => placer.toggle(),
                            // Still allow scrolling in build mode
                            Keycode::Left => scroll_x += 48,
                            Keycode::Right => scroll_x -= 48,
                            Keycode::Up => scroll_y += 48,
                            Keycode::Down => scroll_y -= 48,
                            _ => {}
                        }
                    } else if tax_panel {
                        // Tax panel keys
                        match key {
                            Keycode::Up => {
                                if tax_tier > 0 { tax_tier -= 1; }
                            }
                            Keycode::Down => {
                                if tax_tier < 4 { tax_tier += 1; }
                            }
                            Keycode::Left | Keycode::Right => {
                                let new_rate = sim.players.first()
                                    .map(|p| {
                                        let r = p.tax_rates[tax_tier];
                                        if matches!(key, Keycode::Right) {
                                            r.saturating_add(8).min(128)
                                        } else {
                                            r.saturating_sub(8)
                                        }
                                    })
                                    .unwrap_or(64);
                                let cmd = anno_sim::commands::Command::SetTaxRate {
                                    player: 0, tier: tax_tier as u8, rate: new_rate,
                                };
                                if let Some(client) = net_client.as_mut() {
                                    let payload = cmd.encode();
                                    let msg = anno_net::protocol::NetMessage
                                        ::game_data(payload);
                                    let _ = client.send(&msg);
                                } else {
                                    sim.apply_command(&cmd);
                                }
                            }
                            Keycode::T | Keycode::Escape => {
                                tax_panel = false;
                            }
                            Keycode::Space => {
                                sim.paused = !sim.paused;
                            }
                            _ => {}
                        }
                    } else if diplomacy_panel {
                        // Diplomacy panel keys
                        match key {
                            Keycode::Up => {
                                if diplomacy_target > 1 { diplomacy_target -= 1; }
                            }
                            Keycode::Down => {
                                if diplomacy_target < 6 { diplomacy_target += 1; }
                            }
                            Keycode::Left | Keycode::Right => {
                                use anno_sim::combat::Diplomacy;
                                let cur = sim.diplomacy.get(0, diplomacy_target);
                                let next = if matches!(key, Keycode::Right) {
                                    match cur {
                                        Diplomacy::Allied => Diplomacy::Neutral,
                                        Diplomacy::Neutral => Diplomacy::War,
                                        Diplomacy::War => Diplomacy::Allied,
                                    }
                                } else {
                                    match cur {
                                        Diplomacy::Allied => Diplomacy::War,
                                        Diplomacy::Neutral => Diplomacy::Allied,
                                        Diplomacy::War => Diplomacy::Neutral,
                                    }
                                };
                                sim.diplomacy.set(0, diplomacy_target, next);
                            }
                            Keycode::Y | Keycode::Escape => {
                                diplomacy_panel = false;
                            }
                            Keycode::Space => {
                                sim.paused = !sim.paused;
                            }
                            _ => {}
                        }
                    } else {
                        // Normal mode keys
                        let scroll_speed = 48;
                        match key {
                            Keycode::Left => scroll_x += scroll_speed,
                            Keycode::Right => scroll_x -= scroll_speed,
                            Keycode::Up => scroll_y += scroll_speed,
                            Keycode::Down => scroll_y -= scroll_speed,
                            Keycode::Tab => {
                                if !world_mode && !islands.is_empty() {
                                    let start = current_island;
                                    loop {
                                        current_island =
                                            (current_island + 1) % islands.len();
                                        if !islands[current_island].tiles.is_empty()
                                            || current_island == start
                                        {
                                            break;
                                        }
                                    }
                                    needs_redraw = true;
                                    scroll_x = 0;
                                    scroll_y = 0;
                                }
                            }
                            Keycode::W => {
                                world_mode = !world_mode;
                                needs_redraw = true;
                                scroll_x = 0;
                                scroll_y = 0;
                            }
                            Keycode::Space => {
                                sim.paused = !sim.paused;
                            }
                            Keycode::F => {
                                if sim.speed_multiplier > 1 {
                                    sim.speed_multiplier -= 1;
                                }
                            }
                            Keycode::G => {
                                if sim.speed_multiplier < 8 {
                                    sim.speed_multiplier += 1;
                                }
                            }
                            Keycode::L if trade_route_mode => {
                                if let Some(last) = draft_route_stops.last_mut() {
                                    last.3 = 0; // LOAD only
                                }
                            }
                            Keycode::U if trade_route_mode => {
                                if let Some(last) = draft_route_stops.last_mut() {
                                    last.3 = 1; // UNLOAD only
                                }
                            }
                            Keycode::B if trade_route_mode => {
                                if let Some(last) = draft_route_stops.last_mut() {
                                    last.3 = 2; // BOTH
                                }
                            }
                            Keycode::B => {
                                if !world_mode {
                                    demolish_mode = false;
                                    tax_panel = false;
                                    placer.toggle();
                                }
                            }
                            Keycode::D => {
                                if !world_mode {
                                    placer.active = false;
                                    demolish_mode = !demolish_mode;
                                    tax_panel = false;
                                }
                            }
                            Keycode::T => {
                                tax_panel = !tax_panel;
                                if tax_panel {
                                    placer.active = false;
                                    demolish_mode = false;
                                    diplomacy_panel = false;
                                }
                            }
                            Keycode::Y => {
                                diplomacy_panel = !diplomacy_panel;
                                if diplomacy_panel {
                                    placer.active = false;
                                    demolish_mode = false;
                                    tax_panel = false;
                                }
                            }
                            Keycode::R if shift_held => {
                                route_list_panel = !route_list_panel;
                            }
                            Keycode::R => {
                                trade_route_mode = !trade_route_mode;
                                if trade_route_mode {
                                    placer.active = false;
                                    demolish_mode = false;
                                    tax_panel = false;
                                    diplomacy_panel = false;
                                    draft_route_stops.clear();
                                } else {
                                    draft_route_stops.clear();
                                }
                            }
                            Keycode::Return | Keycode::KpEnter => {
                                if trade_route_mode && draft_route_stops.len() >= 2 {
                                    use anno_sim::trade::{
                                        RouteStop, TradeRoute, TradeShip,
                                    };
                                    use anno_sim::types::Good;
                                    let all_goods = [
                                        Good::Wood, Good::Iron, Good::Ore, Good::Gold,
                                        Good::Wool, Good::Sugar, Good::Tobacco,
                                        Good::Cattle, Good::Grain, Good::Flour,
                                        Good::Food, Good::Alcohol, Good::Cloth,
                                        Good::Clothing, Good::Jewelry, Good::Tools,
                                        Good::Bricks, Good::Swords, Good::Cannons,
                                        Good::Muskets, Good::Stone, Good::Cocoa,
                                        Good::Spices, Good::Hides, Good::Cotton,
                                        Good::Silk, Good::Fish, Good::Grapes,
                                        Good::GoldOre, Good::TobaccoProducts,
                                    ];
                                    let mut route = TradeRoute::new(next_route_id, 0);
                                    for &(island_id, wx, wy, mode) in &draft_route_stops {
                                        let (load, unload): (Vec<(Good, u16)>, Vec<Good>) =
                                            match mode {
                                                0 => (
                                                    all_goods.iter().map(|&g| (g, 50)).collect(),
                                                    Vec::new(),
                                                ),
                                                1 => (Vec::new(), all_goods.to_vec()),
                                                _ => (
                                                    all_goods.iter().map(|&g| (g, 50)).collect(),
                                                    all_goods.to_vec(),
                                                ),
                                            };
                                        route.add_stop(RouteStop {
                                            island_id,
                                            warehouse_x: wx,
                                            warehouse_y: wy,
                                            load_goods: load,
                                            unload_goods: unload,
                                        });
                                    }
                                    route.activate();
                                    let route_id = route.id;
                                    let (sx, sy) = (
                                        draft_route_stops[0].1 as i32,
                                        draft_route_stops[0].2 as i32,
                                    );
                                    sim.trade_routes.push(route);
                                    sim.trade_ships.push(TradeShip::new(
                                        0, route_id, sx, sy,
                                    ));
                                    next_route_id += 1;
                                    println!(
                                        "Created trade route {} ({} stops) + 1 ship",
                                        route_id,
                                        draft_route_stops.len(),
                                    );
                                    draft_route_stops.clear();
                                    trade_route_mode = false;
                                } else if !trade_route_mode && !chat_active {
                                    chat_active = true;
                                    chat_input.clear();
                                }
                            }
                            Keycode::M => {
                                // Toggle music
                                music_enabled = !music_enabled;
                                if music_enabled {
                                    // Resume or start next track
                                    if let Some(slot) = music_slot {
                                        audio.streams.resume(slot);
                                    }
                                    println!("Music ON");
                                } else {
                                    if let Some(slot) = music_slot {
                                        audio.streams.stop(slot);
                                    }
                                    println!("Music OFF");
                                }
                            }
                            Keycode::N => {
                                // Next track
                                if !music_files.is_empty() {
                                    if let Some(slot) = music_slot {
                                        audio.streams.destroy(slot);
                                    }
                                    current_track = (current_track + 1) % music_files.len();
                                    if let Some(slot) =
                                        audio.streams.create(&music_files[current_track], 0)
                                    {
                                        if music_enabled {
                                            if let Some(ref handle) = audio.stream_handle {
                                                audio.streams.play(slot, music_volume, handle);
                                            }
                                        }
                                        println!("Track: {}", music_files[current_track]);
                                        music_slot = Some(slot);
                                    }
                                }
                            }
                            Keycode::V => {
                                // Cycle volume: 0.2 → 0.4 → 0.6 → 0.8 → 1.0 → 0.0 → 0.2...
                                music_volume = if music_volume >= 0.95 {
                                    0.0
                                } else {
                                    music_volume + 0.2
                                };
                                if let Some(slot) = music_slot {
                                    audio.streams.set_volume(slot, music_volume);
                                }
                                println!("Volume: {:.0}%", music_volume * 100.0);
                            }
                            Keycode::S => {
                                if let Some(ref rs) = rendered {
                                    save_ppm(&rs.rgba, rs.width, rs.height, &scenario_name);
                                }
                            }
                            Keycode::H => {
                                show_hud = !show_hud;
                            }
                            Keycode::C => {
                                show_coverage = !show_coverage;
                            }
                            Keycode::K => {
                                graph_panel = !graph_panel;
                            }
                            Keycode::P => {
                                prod_panel = !prod_panel;
                            }
                            Keycode::O => {
                                roster_panel = !roster_panel;
                            }
                            Keycode::A => {
                                market_panel = !market_panel;
                            }
                            Keycode::U => {
                                wh_panel = !wh_panel;
                            }
                            Keycode::J => {
                                ship_panel = !ship_panel;
                            }
                            Keycode::Question | Keycode::Slash => {
                                obj_panel = !obj_panel;
                            }
                            Keycode::F2 => {
                                scenario_picker = !scenario_picker;
                            }
                            Keycode::F3 => {
                                save_panel = !save_panel;
                            }
                            Keycode::F10 => {
                                settings_panel = !settings_panel;
                            }
                            Keycode::F12 => {
                                show_perf = !show_perf;
                            }
                            Keycode::F8 => {
                                // Export the current island layout to an
                                // .szs file. Useful for scenario authoring
                                // — modify in-game, then F8 to persist.
                                let path = save_dir.join(format!("{scenario_name}.export.szs"));
                                let bytes = anno_formats::szs::SzsFile::encode_islands(&islands);
                                if let Some(parent) = path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let msg = match std::fs::write(&path, &bytes) {
                                    Ok(()) => format!(
                                        "exported {} islands → {}",
                                        islands.len(), path.display(),
                                    ),
                                    Err(e) => format!("export FAILED: {e}"),
                                };
                                println!("{msg}");
                                save_banner = Some((msg, std::time::Instant::now()));
                            }
                            Keycode::F7 => {
                                // Found a colony: drop a Kontor on the
                                // active island if the player has none
                                // there yet. Costs 500 gold.
                                if !world_mode {
                                    let island = &islands[current_island];
                                    let island_id = island.number;
                                    const COLONY_COST: i32 = 500;
                                    let already = sim.warehouses.iter().any(|w| {
                                        w.active && w.owner == 0
                                            && w.island_id == island_id
                                    });
                                    if already {
                                        save_banner = Some((
                                            "colony FAILED: already have a Kontor here"
                                                .to_string(),
                                            std::time::Instant::now(),
                                        ));
                                    } else if sim.players[0].gold < COLONY_COST {
                                        save_banner = Some((
                                            "colony FAILED: need 500 gold"
                                                .to_string(),
                                            std::time::Instant::now(),
                                        ));
                                    } else {
                                        // Find a walkable spot near the
                                        // island center for the new Kontor.
                                        let map_idx = sim.island_maps.iter()
                                            .position(|m| m.island_id == island_id);
                                        if let Some(idx) = map_idx {
                                            let cx = (island.width / 2) as u16;
                                            let cy = (island.height / 2) as u16;
                                            let spot = sim.island_maps[idx]
                                                .find_open_spot(cx, cy, 2, 2, 20);
                                            if let Some((bx, by)) = spot {
                                                sim.players[0].gold -= COLONY_COST;
                                                let mut wh = anno_sim::warehouse::Warehouse::new(
                                                    island_id, 0, bx, by,
                                                );
                                                // Seed initial capacities so
                                                // carriers can deposit goods.
                                                for g in [
                                                    Good::Wood, Good::Iron, Good::Tools,
                                                    Good::Food, Good::Cloth, Good::Bricks,
                                                    Good::Stone, Good::Grain, Good::Flour,
                                                    Good::Wool, Good::Sugar, Good::Tobacco,
                                                ] {
                                                    wh.set_capacity(g, 100);
                                                }
                                                sim.warehouses.push(wh);
                                                // Initialize coverage for this island.
                                                if !sim.coverage_maps.iter()
                                                    .any(|c| c.island_id == island_id)
                                                {
                                                    sim.coverage_maps.push(
                                                        anno_sim::coverage::CoverageMap::new(
                                                            island_id,
                                                            island.width as u16,
                                                            island.height as u16,
                                                        ),
                                                    );
                                                }
                                                let msg = format!(
                                                    "Colony founded on island {} at ({},{})",
                                                    island_id, bx, by,
                                                );
                                                println!("{msg}");
                                                save_banner = Some((msg, std::time::Instant::now()));
                                                needs_redraw = true;
                                            } else {
                                                save_banner = Some((
                                                    "colony FAILED: no buildable spot found"
                                                        .to_string(),
                                                    std::time::Instant::now(),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            Keycode::F4 => {
                                // Buy a TradeShip at the player's first
                                // active warehouse (1000 gold).
                                const TRADE_SHIP_COST: i32 = 1000;
                                if sim.players.first().map(|p| p.gold).unwrap_or(0)
                                    < TRADE_SHIP_COST
                                {
                                    save_banner = Some((
                                        "ship build FAILED: not enough gold (1000 needed)"
                                            .to_string(),
                                        std::time::Instant::now(),
                                    ));
                                } else if let Some(wh) = sim.warehouses.iter()
                                    .find(|w| w.active && w.owner == 0)
                                {
                                    let (sx, sy) = (wh.tile_x as i32, wh.tile_y as i32);
                                    sim.players[0].gold -= TRADE_SHIP_COST;
                                    let ship = anno_sim::trade::TradeShip::new(
                                        0, 0, sx, sy,
                                    );
                                    let msg = format!(
                                        "Built TradeShip at ({sx},{sy}) — assign via R"
                                    );
                                    println!("{msg}");
                                    save_banner = Some((msg, std::time::Instant::now()));
                                    sim.trade_ships.push(ship);
                                } else {
                                    save_banner = Some((
                                        "ship build FAILED: no warehouse"
                                            .to_string(),
                                        std::time::Instant::now(),
                                    ));
                                }
                            }
                            Keycode::F6 => {
                                show_paths = !show_paths;
                            }
                            Keycode::F5 => {
                                let snap = sim.snapshot();
                                let msg = match anno_sim::save::save_to_file(
                                    &quicksave_path, &snap,
                                ) {
                                    Ok(()) => format!(
                                        "saved → {} ({} buildings, {} gold)",
                                        quicksave_path.display(),
                                        sim.buildings.len(),
                                        sim.players.first().map(|p| p.gold).unwrap_or(0),
                                    ),
                                    Err(e) => format!("save FAILED: {e}"),
                                };
                                println!("{msg}");
                                save_banner = Some((msg, std::time::Instant::now()));
                            }
                            Keycode::F9 => {
                                let msg = match anno_sim::save::load_from_file(
                                    &quicksave_path,
                                ) {
                                    Ok(state) => {
                                        let bldgs = state.buildings.len();
                                        let gold = state.players
                                            .first().map(|p| p.gold).unwrap_or(0);
                                        sim.apply_snapshot(state);
                                        needs_redraw = true;
                                        format!(
                                            "loaded ← {} ({} buildings, {} gold)",
                                            quicksave_path.display(), bldgs, gold,
                                        )
                                    }
                                    Err(e) => format!("load FAILED: {e}"),
                                };
                                println!("{msg}");
                                save_banner = Some((msg, std::time::Instant::now()));
                            }
                            Keycode::Equals | Keycode::Plus | Keycode::KpPlus => {
                                display_zoom = (display_zoom + 1).min(8);
                            }
                            Keycode::Minus | Keycode::KpMinus => {
                                display_zoom = (display_zoom - 1).max(1);
                            }
                            Keycode::Num1 => {
                                if sprite_zoom != 0 {
                                    sprite_zoom = 0;
                                    needs_redraw = true;
                                }
                            }
                            Keycode::Num2 => {
                                if sprite_zoom != 1 && !sprites_by_zoom[1].is_empty() {
                                    sprite_zoom = 1;
                                    needs_redraw = true;
                                }
                            }
                            Keycode::Num3 => {
                                if sprite_zoom != 2 && !sprites_by_zoom[2].is_empty() {
                                    sprite_zoom = 2;
                                    needs_redraw = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }

                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => {
                    // Check if click is on the minimap (bottom-right corner)
                    let on_minimap = if let Some(ref rs) = rendered {
                        let sx = 200.0 / rs.width as f64;
                        let sy = 150.0 / rs.height as f64;
                        let ms = sx.min(sy).min(1.0);
                        let mw = (rs.width as f64 * ms) as i32;
                        let mh = (rs.height as f64 * ms) as i32;
                        let mx = WINDOW_W as i32 - mw - 8;
                        let my = WINDOW_H as i32 - mh - 8;
                        x >= mx && x < mx + mw && y >= my && y < my + mh
                    } else {
                        false
                    };

                    if on_minimap {
                        minimap_clicked = true;
                        minimap_click_x = x;
                        minimap_click_y = y;
                    } else if placer.active && !world_mode {
                        // Try to place a building
                        if let (Some(rs), Some(bb)) =
                            (&rendered, placer.selected_building())
                        {
                            let def_idx = bb.def_idx;
                            let sprite_idx = bb.sprite_idx;
                            let def = &defs[def_idx];
                            let island_number = islands[current_island].number;
                            let bld_w = def.width;
                            let bld_h = def.height;
                            let cost = def.cost_gold;
                            let input1 = def.input_good_1;
                            let input2 = def.input_good_2;
                            let storage = def.storage_capacity;

                            // Convert screen coords to texture coords
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;

                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x, tex_y, rs.origin_x, rs.origin_y, rs.tile_w, rs.tile_h,
                            );

                            // Find the island map for validation
                            let island_map_idx = sim
                                .island_maps
                                .iter()
                                .position(|m| m.island_id == island_number);

                            // Climate gate: refuse to place a plantation
                            // building whose output_good doesn't match the
                            // island's climate.
                            let isl = &islands[current_island];
                            let climate = anno_sim::climate::climate_for_y(
                                isl.y_pos as u32, 512,
                            );
                            let climate_ok = anno_sim::climate::allows_production(
                                climate, def.output_good,
                            );
                            if !climate_ok {
                                save_banner = Some((
                                    format!(
                                        "build FAILED: {:?} needs {} climate (this island is {})",
                                        def.output_good,
                                        match climate {
                                            anno_sim::climate::Climate::North => "South",
                                            anno_sim::climate::Climate::South => "North",
                                        },
                                        climate.label(),
                                    ),
                                    std::time::Instant::now(),
                                ));
                            } else if let Some(map_idx) = island_map_idx {
                                if can_place_building(
                                    &islands[current_island], &sim.island_maps[map_idx],
                                    tile_x, tile_y, bld_w, bld_h,
                                ) {
                                    // Materials are now trickled in by
                                    // the entity tick — we just record
                                    // what the building still needs and
                                    // pay gold up front.
                                    if sim.players[0].gold >= cost as i32 {
                                        sim.players[0].gold -= cost as i32;

                                        // Per-rotation sprite stride: each
                                        // rotation slice occupies anim_anz *
                                        // anim_add sprites (typically =
                                        // footprint when not animated).
                                        let cod_b = &cod.buildings[def_idx];
                                        let rot_count = cod_b.rotate.max(1) as u8;
                                        let orient = placer.orientation % rot_count;
                                        let stride = (cod_b.anim_anz.max(1)
                                            * cod_b.anim_add.max(1)) as usize;
                                        let rot_offset = orient as usize * stride;

                                        // Add tile records to the island for rendering
                                        for dy in 0..bld_h as u8 {
                                            for dx in 0..bld_w as u8 {
                                                let tx = tile_x as u8 + dx;
                                                let ty = tile_y as u8 + dy;
                                                let tile_sprite = sprite_idx
                                                    + rot_offset
                                                    + dy as usize * bld_w as usize
                                                    + dx as usize;
                                                islands[current_island].tiles.push(IslandTile {
                                                    x: tx,
                                                    y: ty,
                                                    building_id: tile_sprite as u16,
                                                    orientation: orient,
                                                    anim_count: 0,
                                                    flags: 0,
                                                });
                                                // Mark tile as non-walkable
                                                sim.island_maps[map_idx].set_walkable(
                                                    tx as u16, ty as u16, false,
                                                );
                                            }
                                        }

                                        // Add building instance to simulation
                                        let mut instance = BuildingInstance::new(
                                            def_idx as u16,
                                            island_number,
                                            tile_x as u16,
                                            tile_y as u16,
                                            0, // human player
                                        );
                                        // Seed input materials for production
                                        if input1 != Good::None {
                                            instance.input_1_stock = storage;
                                        }
                                        if input2 != Good::None {
                                            instance.input_2_stock = storage;
                                        }
                                        // Construction time scales with footprint
                                        // (larger buildings take longer): 2s per tile.
                                        let footprint = (def.width as u32)
                                            * (def.height as u32);
                                        let build_ms = (2_000u32 * footprint).max(2_000);
                                        instance.construction_ms_total = build_ms;
                                        instance.construction_ms_remaining = build_ms;
                                        // Trickle materials: warehouses
                                        // will be drained as construction
                                        // proceeds (entity tick).
                                        instance.wood_needed = def.cost_wood;
                                        instance.tools_needed = def.cost_tools;
                                        instance.bricks_needed = def.cost_bricks;
                                        sim.buildings.push(instance);

                                        println!(
                                            "Placed {} at ({},{}) on island {} [cost: {} gold]",
                                            &placer.buildable[placer.selected].name,
                                            tile_x, tile_y,
                                            island_number,
                                            cost,
                                        );
                                        // Play placement sound
                                        if let (Some(sfx), Some(handle)) =
                                            (place_sound_slot, &audio.stream_handle)
                                        {
                                            audio.waves.play_once(
                                                sfx,
                                                WINDOW_W as i32 / 2,
                                                WINDOW_H as i32 / 2,
                                                handle,
                                            );
                                        }
                                        needs_redraw = true;
                                    } else {
                                        println!("Not enough gold! Need {}, have {}",
                                            cost, sim.players[0].gold);
                                    }
                                }
                            }
                        }
                    } else if demolish_mode && !world_mode {
                        // Try to demolish a building
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x, tex_y, rs.origin_x, rs.origin_y, rs.tile_w, rs.tile_h,
                            );

                            let island = &islands[current_island];
                            let island_id = island.number;

                            // Find building at this tile (only player-owned)
                            let building_idx = sim.buildings.iter().position(|b| {
                                b.owner == 0 && b.island_id == island_id && {
                                    let def = &defs[b.def_id as usize];
                                    let bx = b.tile_x as i32;
                                    let by = b.tile_y as i32;
                                    tile_x >= bx && tile_x < bx + def.width as i32
                                        && tile_y >= by && tile_y < by + def.height as i32
                                }
                            });

                            if let Some(bi) = building_idx {
                                let b = &sim.buildings[bi];
                                let def = &defs[b.def_id as usize];
                                let bx = b.tile_x;
                                let by = b.tile_y;
                                let bw = def.width;
                                let bh = def.height;
                                let refund = def.cost_gold / 2;
                                let name = cod.buildings[b.def_id as usize]
                                    .properties
                                    .get("Name")
                                    .cloned()
                                    .unwrap_or_else(|| format!("Bldg#{}", b.def_id));

                                // Refund half of construction cost
                                sim.players[0].gold += refund as i32;

                                // Remove building tiles from island
                                islands[current_island].tiles.retain(|t| {
                                    let in_footprint = t.x as u16 >= bx
                                        && t.x as u16 - bx < bw as u16
                                        && t.y as u16 >= by
                                        && t.y as u16 - by < bh as u16;
                                    !in_footprint
                                });

                                // Restore walkability
                                let island_map_idx = sim
                                    .island_maps
                                    .iter()
                                    .position(|m| m.island_id == island_id);
                                if let Some(map_idx) = island_map_idx {
                                    for dy in 0..bh {
                                        for dx in 0..bw {
                                            sim.island_maps[map_idx].set_walkable(
                                                bx + dx as u16,
                                                by + dy as u16,
                                                true,
                                            );
                                        }
                                    }
                                }

                                // Remove building from simulation
                                sim.buildings.remove(bi);

                                println!(
                                    "Demolished {} at ({},{}) on island {} [refund: {} gold]",
                                    name, bx, by, island_id, refund,
                                );
                                needs_redraw = true;
                                // Clear inspection if it was pointing at this building
                                if let Some(ref insp) = inspection {
                                    if insp.building_idx == Some(bi) {
                                        inspection = None;
                                    }
                                }
                            }
                        }
                    } else if trade_route_mode && !world_mode {
                        // Trade-route mode: clicking a warehouse adds it as a stop.
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x, tex_y, rs.origin_x, rs.origin_y,
                                rs.tile_w, rs.tile_h,
                            );
                            let island_id = islands[current_island].number;
                            let wh = sim.warehouses.iter().find(|w| {
                                w.active
                                    && w.island_id == island_id
                                    && (w.tile_x as i32 - tile_x).abs() <= 2
                                    && (w.tile_y as i32 - tile_y).abs() <= 2
                            });
                            if let Some(w) = wh {
                                // First stop defaults to LOAD-only
                                // (origin); subsequent stops default to
                                // UNLOAD-only. Player can override via L/U/B.
                                let default_mode = if draft_route_stops.is_empty() {
                                    0 // LOAD
                                } else {
                                    1 // UNLOAD
                                };
                                let stop = (
                                    w.island_id, w.tile_x, w.tile_y, default_mode,
                                );
                                let dup = draft_route_stops.last()
                                    .map(|&(iid, x, y, _)| {
                                        (iid, x, y) == (stop.0, stop.1, stop.2)
                                    })
                                    .unwrap_or(false);
                                if !dup {
                                    draft_route_stops.push(stop);
                                    let mode_str = ["LOAD", "UNLOAD", "BOTH"][stop.3 as usize];
                                    println!(
                                        "Route stop {}: island {} ({},{}) [{}]",
                                        draft_route_stops.len(),
                                        stop.0, stop.1, stop.2, mode_str,
                                    );
                                }
                            } else {
                                println!(
                                    "No warehouse near ({},{}) on this island",
                                    tile_x, tile_y,
                                );
                            }
                        }
                    } else {
                        // Check if the click landed on a player-owned military unit.
                        // If so, select it (Shift+click adds to selection); else drag.
                        let mut hit_unit: Option<usize> = None;
                        if !world_mode {
                            if let Some(ref rs) = rendered {
                                let dst_w = rs.width as i32 * display_zoom;
                                let dst_h = rs.height as i32 * display_zoom;
                                let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                                let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                                let tex_x = (x - dst_x) / display_zoom;
                                let tex_y = (y - dst_y) / display_zoom;
                                let (tile_x, tile_y) = screen_to_tile(
                                    tex_x, tex_y, rs.origin_x, rs.origin_y,
                                    rs.tile_w, rs.tile_h,
                                );
                                hit_unit = sim.military_units.iter().position(|u| {
                                    u.is_alive() && u.owner == 0
                                        && (u.tile_x - tile_x).abs() <= 1
                                        && (u.tile_y - tile_y).abs() <= 1
                                });
                            }
                        }
                        if let Some(ui) = hit_unit {
                            if shift_held {
                                if !selected_units.contains(&ui) {
                                    selected_units.push(ui);
                                }
                            } else {
                                selected_units.clear();
                                selected_units.push(ui);
                            }
                            println!(
                                "Selected {} unit(s)",
                                selected_units.len(),
                            );
                        } else {
                            // Empty-area click clears selection, then starts drag
                            if !selected_units.is_empty() {
                                selected_units.clear();
                            }
                            dragging = true;
                            drag_start = (x - scroll_x, y - scroll_y);
                        }
                    }
                }

                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Right,
                    x,
                    y,
                    ..
                } => {
                    // Shift + RMB: open context menu at the cursor.
                    if shift_held && !world_mode {
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x, tex_y, rs.origin_x, rs.origin_y,
                                rs.tile_w, rs.tile_h,
                            );
                            // Compose action list based on what's at the tile.
                            let mut actions: Vec<&'static str> = vec!["Inspect"];
                            if !selected_units.is_empty() {
                                actions.push("Move selected here");
                            }
                            let island_id = islands[current_island].number;
                            let has_player_building = sim.buildings.iter().any(|b| {
                                b.owner == 0 && b.island_id == island_id && {
                                    let def = &defs[b.def_id as usize];
                                    let bx = b.tile_x as i32;
                                    let by = b.tile_y as i32;
                                    tile_x >= bx && tile_x < bx + def.width as i32
                                        && tile_y >= by && tile_y < by + def.height as i32
                                }
                            });
                            if has_player_building { actions.push("Demolish"); }
                            actions.push("Cancel");
                            context_menu = Some(ContextMenu {
                                screen_x: x, screen_y: y,
                                tile_x, tile_y,
                                actions, sel: 0,
                            });
                        }
                        continue;
                    }
                    // If units are selected: issue a move order to that tile.
                    if !world_mode && !selected_units.is_empty() {
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x, tex_y, rs.origin_x, rs.origin_y,
                                rs.tile_w, rs.tile_h,
                            );
                            let mut moved = 0;
                            for &ui in &selected_units {
                                if let Some(u) = sim.military_units.get_mut(ui) {
                                    if u.is_alive() {
                                        u.target_x = tile_x;
                                        u.target_y = tile_y;
                                        u.combat_target = -1;
                                        u.move_timer_ms = 0;
                                        moved += 1;
                                    }
                                }
                            }
                            println!(
                                "Move order → ({},{}) for {moved} unit(s)",
                                tile_x, tile_y,
                            );
                        }
                        continue;
                    }
                    // Right-click: inspect tile
                    if !world_mode {
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x, tex_y, rs.origin_x, rs.origin_y, rs.tile_w, rs.tile_h,
                            );

                            let island = &islands[current_island];
                            let island_id = island.number;

                            // Find building at this tile
                            let building_idx = sim.buildings.iter().position(|b| {
                                b.island_id == island_id && {
                                    let def = &defs[b.def_id as usize];
                                    let bx = b.tile_x as i32;
                                    let by = b.tile_y as i32;
                                    tile_x >= bx && tile_x < bx + def.width as i32
                                        && tile_y >= by && tile_y < by + def.height as i32
                                }
                            });

                            // Find warehouse at this tile
                            let warehouse_idx = sim.warehouses.iter().position(|w| {
                                w.island_id == island_id
                                    && (w.tile_x as i32 - tile_x).abs() <= 2
                                    && (w.tile_y as i32 - tile_y).abs() <= 2
                            });

                            // Build info string
                            let mut info = format!("Tile ({},{}) ", tile_x, tile_y);

                            if let Some(bi) = building_idx {
                                let b = &sim.buildings[bi];
                                let def = &defs[b.def_id as usize];
                                let name = cod.buildings[b.def_id as usize]
                                    .properties
                                    .get("Name")
                                    .cloned()
                                    .unwrap_or_else(|| format!("Bldg#{}", b.def_id));
                                info.push_str(&format!("| {} ", name));
                                if def.output_good != Good::None {
                                    info.push_str(&format!(
                                        "| out:{:?}={}/{} ",
                                        def.output_good, b.output_stock, def.storage_capacity
                                    ));
                                    if def.input_good_1 != Good::None {
                                        info.push_str(&format!(
                                            "in1:{:?}={} ",
                                            def.input_good_1, b.input_1_stock
                                        ));
                                    }
                                    if def.input_good_2 != Good::None {
                                        info.push_str(&format!(
                                            "in2:{:?}={} ",
                                            def.input_good_2, b.input_2_stock
                                        ));
                                    }
                                    info.push_str(&format!(
                                        "eff:{}% ",
                                        b.efficiency as u32 * 100 / 128
                                    ));
                                }
                            }

                            if let Some(wi) = warehouse_idx {
                                let w = &sim.warehouses[wi];
                                info.push_str("| WH: ");
                                let mut goods_shown = 0;
                                let all_goods = [
                                    Good::Wood, Good::Iron, Good::Ore, Good::Gold,
                                    Good::Wool, Good::Sugar, Good::Tobacco, Good::Cattle,
                                    Good::Grain, Good::Flour, Good::Food, Good::Alcohol,
                                    Good::Cloth, Good::Clothing, Good::Jewelry,
                                    Good::Tools, Good::Bricks, Good::Swords, Good::Cannons,
                                    Good::Muskets, Good::Stone, Good::Cocoa, Good::Spices,
                                    Good::Hides, Good::Cotton, Good::Silk, Good::Fish,
                                    Good::Grapes, Good::GoldOre, Good::TobaccoProducts,
                                ];
                                for &g in &all_goods {
                                    let qty = w.stock(g);
                                    if qty > 0 {
                                        info.push_str(&format!("{:?}={} ", g, qty));
                                        goods_shown += 1;
                                        if goods_shown >= 8 {
                                            info.push_str("...");
                                            break;
                                        }
                                    }
                                }
                                if goods_shown == 0 {
                                    info.push_str("(empty)");
                                }
                            }

                            if building_idx.is_none() && warehouse_idx.is_none() {
                                // Check what tile sprite is here
                                if let Some(tile) = island.tiles.iter().find(|t| {
                                    t.x as i32 == tile_x && t.y as i32 == tile_y
                                }) {
                                    info.push_str(&format!("| sprite#{}", tile.building_id));
                                } else {
                                    info.push_str("| (empty)");
                                }
                            }

                            inspection = Some(Inspection {
                                tile_x,
                                tile_y,
                                building_idx,
                                warehouse_idx,
                                info,
                            });
                        }
                    } else {
                        inspection = None;
                    }
                }

                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    dragging = false;
                }

                Event::KeyUp { keycode: Some(k), .. }
                    if matches!(k, Keycode::LShift | Keycode::RShift) =>
                {
                    shift_held = false;
                }

                Event::TextInput { ref text, .. } if chat_active => {
                    if chat_input.len() < 200 {
                        chat_input.push_str(text);
                    }
                }

                Event::MouseMotion { x, y, .. } => {
                    mouse_x = x;
                    mouse_y = y;
                    if dragging && !placer.active {
                        scroll_x = x - drag_start.0;
                        scroll_y = y - drag_start.1;
                    }
                }

                Event::MouseWheel { y, .. } => {
                    if y > 0 {
                        display_zoom = (display_zoom + 1).min(8);
                    } else if y < 0 {
                        display_zoom = (display_zoom - 1).max(1);
                    }
                }

                _ => {}
            }
        }

        // Update hover tile for build/demolish mode cursor
        if (placer.active || demolish_mode) && !world_mode {
            if let Some(ref rs) = rendered {
                let dst_w = rs.width as i32 * display_zoom;
                let dst_h = rs.height as i32 * display_zoom;
                let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                let tex_x = (mouse_x - dst_x) / display_zoom;
                let tex_y = (mouse_y - dst_y) / display_zoom;

                let (tx, ty) = screen_to_tile(
                    tex_x, tex_y, rs.origin_x, rs.origin_y, rs.tile_w, rs.tile_h,
                );
                if placer.active {
                    placer.hover_tile = Some((tx, ty));
                }
                if demolish_mode {
                    let island_id = islands[current_island].number;
                    demolish_hover = sim.buildings.iter().position(|b| {
                        b.owner == 0 && b.island_id == island_id && {
                            let def = &defs[b.def_id as usize];
                            let bx = b.tile_x as i32;
                            let by = b.tile_y as i32;
                            tx >= bx && tx < bx + def.width as i32
                                && ty >= by && ty < by + def.height as i32
                        }
                    });
                }
            }
        } else {
            placer.hover_tile = None;
            demolish_hover = None;
        }

        // Simulation tick
        let now = timer.ticks();
        let dt_ms = now.wrapping_sub(last_tick);
        last_tick = now;

        // Net poll: drain any messages first.
        if let Some(host) = net_host.as_mut() {
            let evs = host.poll();
            for ev in evs {
                match ev {
                    anno_net::session::SessionEvent::PlayerJoined {
                        slot, player_id, name,
                    } => {
                        println!("[net] joined slot={slot} id={player_id} name={name}");
                        net_status = format!(
                            "HOST :{} ({} peers)",
                            net_role_port(&net_role),
                            host.session().player_count - 1,
                        );
                    }
                    anno_net::session::SessionEvent::PlayerLeft { slot, player_id } => {
                        println!("[net] left slot={slot} id={player_id}");
                        net_status = format!(
                            "HOST :{} ({} peers)",
                            net_role_port(&net_role),
                            host.session().player_count.saturating_sub(1),
                        );
                    }
                    anno_net::session::SessionEvent::Chat { from_player, text } => {
                        let line = format!("p{from_player}: {text}");
                        println!("[chat] {line}");
                        chat_log.push_back((line, std::time::Instant::now()));
                        if chat_log.len() > 8 { chat_log.pop_front(); }
                        // Re-broadcast to peers so everyone sees client chats.
                        let msg = anno_net::protocol::NetMessage::chat(&text);
                        host.send_to_all(&msg);
                    }
                    anno_net::session::SessionEvent::GameData { from_player, data } => {
                        // Tag-prefixed payloads are commands from clients;
                        // anything else is a stray broadcast we ignore.
                        if let Some(cmd) = anno_sim::commands::Command::decode(&data) {
                            let applied = sim.apply_command(&cmd);
                            println!(
                                "[cmd] from p{from_player}: {:?} (applied={applied})",
                                cmd,
                            );
                        }
                    }
                    _ => {}
                }
            }
            // Periodic state broadcast.
            last_broadcast_ms = last_broadcast_ms.saturating_add(dt_ms);
            if last_broadcast_ms >= broadcast_interval_ms {
                last_broadcast_ms = 0;
                let snap = sim.snapshot();
                if let Ok(payload) = bincode::serialize(&snap) {
                    let msg = anno_net::protocol::NetMessage::game_data(payload);
                    host.send_to_all(&msg);
                }
            }
        }
        if let Some(client) = net_client.as_mut() {
            let evs = client.poll();
            for ev in evs {
                match ev {
                    anno_net::session::SessionEvent::GameData { data, .. } => {
                        // Defensive: ignore command-tagged payloads at the
                        // client (the host never broadcasts those, but if
                        // they arrived bincode would garble the SaveState).
                        if data.first().copied() == Some(anno_sim::commands::COMMAND_TAG) {
                            continue;
                        }
                        if let Ok(snap) = bincode::deserialize::<anno_sim::save::SaveState>(&data) {
                            sim.apply_snapshot(snap);
                            needs_redraw = true;
                        } else {
                            eprintln!("[net] failed to deserialize snapshot ({} bytes)", data.len());
                        }
                    }
                    anno_net::session::SessionEvent::Chat { from_player, text } => {
                        let line = format!("p{from_player}: {text}");
                        println!("[chat] {line}");
                        chat_log.push_back((line, std::time::Instant::now()));
                        if chat_log.len() > 8 { chat_log.pop_front(); }
                    }
                    _ => {}
                }
            }
        }

        // Auto-pause when any modal info panel transitions open; auto-unpause
        // on close iff we were the ones who paused. The player can still hit
        // Space mid-read to override (auto_paused flips to false then).
        let any_modal = graph_panel || prod_panel || roster_panel
            || market_panel || wh_panel || ship_panel || save_panel
            || scenario_picker || tax_panel || diplomacy_panel
            || trade_route_mode || obj_panel || settings_panel
            || context_menu.is_some() || route_list_panel;
        if any_modal != prev_modal_open {
            if any_modal {
                if !sim.paused {
                    sim.paused = true;
                    auto_paused = true;
                }
            } else if auto_paused {
                sim.paused = false;
                auto_paused = false;
            }
            prev_modal_open = any_modal;
        } else if any_modal && !sim.paused {
            // Player un-paused manually while reading — disarm the auto.
            auto_paused = false;
        }

        let perf_sim_start = std::time::Instant::now();
        if dt_ms > 0 && dt_ms < 1000 {
            if net_client.is_none() {
                sim.tick(dt_ms);
            }
            // Drain objective completions for the chat log.
            if !sim.objective_completions.is_empty() {
                let drained: Vec<usize> = sim.objective_completions.drain(..).collect();
                for idx in drained {
                    if let Some((obj, _)) = sim.objectives.items.get(idx) {
                        let line = format!("[obj] complete: {}", obj.label());
                        chat_log.push_back((line, std::time::Instant::now()));
                        if chat_log.len() > 8 { chat_log.pop_front(); }
                    }
                }
                if let (Some(sfx), Some(handle)) = (event_obj_done_slot, &audio.stream_handle) {
                    audio.waves.play_once(sfx, WINDOW_W as i32 / 2, WINDOW_H as i32 / 2, handle);
                }
            }
            // Drain combat-destroyed buildings: clear matching island tiles
            // so the static renderer no longer paints the dead footprint.
            if !sim.tile_clears.is_empty() {
                let drained: Vec<_> = sim.tile_clears.drain(..).collect();
                for (island_id, bx, by, bw, bh) in drained {
                    if let Some(island) = islands.iter_mut()
                        .find(|i| i.number == island_id)
                    {
                        island.tiles.retain(|t| {
                            let in_footprint = t.x as u16 >= bx
                                && (t.x as u16) < bx + bw as u16
                                && t.y as u16 >= by
                                && (t.y as u16) < by + bh as u16;
                            !in_footprint
                        });
                    }
                    chat_log.push_back((
                        format!("[combat] building at ({bx},{by}) on island {island_id} destroyed"),
                        std::time::Instant::now(),
                    ));
                    if chat_log.len() > 8 { chat_log.pop_front(); }
                }
                if let (Some(sfx), Some(handle)) = (event_destroy_slot, &audio.stream_handle) {
                    audio.waves.play_once(sfx, WINDOW_W as i32 / 2, WINDOW_H as i32 / 2, handle);
                }
                needs_redraw = true;
            }
            // Animation cycles regardless of net role so visuals don't freeze.
            anim_state.tick(dt_ms);
            let anim_gen = anim_state.elapsed_ms / 100;
            if anim_gen != last_anim_gen {
                last_anim_gen = anim_gen;
                needs_redraw = true;
            }
        }

        // Event log: diff sim state vs last frame and post notable changes
        // into the chat log (TTL 10s like network chat).
        {
            // 1. Diplomacy flips. Only report each pair once (i < j).
            for i in 0..7u8 {
                for j in (i + 1)..7u8 {
                    let cur = sim.diplomacy.get(i, j);
                    let prev = prev_diplomacy[i as usize][j as usize];
                    if cur != prev {
                        let label = match cur {
                            Diplomacy::Allied => "ALLIED",
                            Diplomacy::Neutral => "neutral",
                            Diplomacy::War => "WAR",
                        };
                        let line = format!("[diplo] p{i} ↔ p{j} → {label}");
                        chat_log.push_back((line, std::time::Instant::now()));
                        if chat_log.len() > 8 { chat_log.pop_front(); }
                        if cur == Diplomacy::War {
                            if let (Some(sfx), Some(handle)) =
                                (event_war_slot, &audio.stream_handle)
                            {
                                audio.waves.play_once(
                                    sfx,
                                    WINDOW_W as i32 / 2,
                                    WINDOW_H as i32 / 2,
                                    handle,
                                );
                            }
                        }
                        prev_diplomacy[i as usize][j as usize] = cur;
                        prev_diplomacy[j as usize][i as usize] = cur;
                    }
                }
            }
            // 2. New AI buildings. Skip player 0 (the human's own builds
            //    aren't surprising, and they're already logged on placement).
            let mut counts: [usize; 7] = [0; 7];
            for b in &sim.buildings {
                let o = b.owner as usize;
                if o < counts.len() { counts[o] += 1; }
            }
            for owner in 1..7 {
                if counts[owner] > prev_building_counts[owner] {
                    let delta = counts[owner] - prev_building_counts[owner];
                    let line = format!("[build] p{owner} built {delta} building(s)");
                    chat_log.push_back((line, std::time::Instant::now()));
                    if chat_log.len() > 8 { chat_log.pop_front(); }
                }
                prev_building_counts[owner] = counts[owner];
            }
        }

        // Audio tick: cleanup finished sounds, auto-advance music
        audio.work_events();
        if music_enabled && !music_files.is_empty() {
            // Check if current track finished, advance to next
            if let Some(slot) = music_slot {
                if audio.streams.status(slot) == anno_audio::stream::StreamStatus::Stopped {
                    // Track might have finished naturally (sink empty)
                    audio.streams.destroy(slot);
                    current_track = (current_track + 1) % music_files.len();
                    if let Some(new_slot) =
                        audio.streams.create(&music_files[current_track], 0)
                    {
                        if let Some(ref handle) = audio.stream_handle {
                            audio.streams.play(new_slot, music_volume, handle);
                        }
                        music_slot = Some(new_slot);
                    }
                }
            }
        }

        // Re-render terrain when needed
        if needs_redraw && !islands.is_empty() {
            let sprites = &sprites_by_zoom[sprite_zoom];
            let num_sprites = sprites.len();
            let tile_w = ZOOM_TILE_W[sprite_zoom];
            let tile_h = ZOOM_TILE_H[sprite_zoom];
            if world_mode {
                let (rgba, w, h, ox, oy) =
                    render_world(&islands, sprites, num_sprites, tile_w, tile_h, &anim_state);
                rendered = Some(RenderState {
                    rgba,
                    width: w,
                    height: h,
                    origin_x: ox,
                    origin_y: oy,
                    tile_w,
                    tile_h,
                });
            } else {
                let island = &islands[current_island];
                let (rgba, w, h, ox, oy) =
                    render_island(island, sprites, num_sprites, tile_w, tile_h, &anim_state);
                rendered = Some(RenderState {
                    rgba,
                    width: w,
                    height: h,
                    origin_x: ox,
                    origin_y: oy,
                    tile_w,
                    tile_h,
                });
            }
            needs_redraw = false;
        }

        // Draw
        canvas.set_draw_color(sdl2::pixels::Color::RGB(BG_COLOR.0, BG_COLOR.1, BG_COLOR.2));
        canvas.clear();

        if let Some(ref rs) = rendered {
            if rs.width > 0 && rs.height > 0 {
                let mut texture = texture_creator
                    .create_texture_streaming(PixelFormatEnum::RGBA32, rs.width, rs.height)
                    .expect("texture creation failed");

                // Copy terrain to a mutable buffer, then overlay dynamic entities
                let mut frame = rs.rgba.clone();
                overlay_entities(
                    &mut frame,
                    rs.width,
                    rs.height,
                    rs.origin_x,
                    rs.origin_y,
                    rs.tile_w,
                    rs.tile_h,
                    &sim,
                    world_mode,
                    if world_mode { None } else { Some(&islands[current_island]) },
                    &carrier_sprites[sprite_zoom],
                    &ship_sprites[sprite_zoom],
                    &soldier_sprites[sprite_zoom],
                    &selected_units,
                    carrier_walk_anz,
                    ship_walk_anz,
                    show_paths,
                );

                // Draw service coverage overlay (C key, single-island mode)
                if show_coverage && !world_mode {
                    let island = &islands[current_island];
                    let island_id = island.number;
                    let cov = sim.coverage_maps.iter().find(|c| c.island_id == island_id);
                    if let Some(cov) = cov {
                        let half_tw = rs.tile_w / 2;
                        let half_th = rs.tile_h / 2;
                        for tile in &island.tiles {
                            let tx = tile.x as i32;
                            let ty = tile.y as i32;
                            let covered = cov.is_covered(tile.x as u16, tile.y as u16);
                            let public = cov.public_coverage_at(tile.x as u16, tile.y as u16);
                            let color: [u8; 4] = if covered {
                                let blue = (public as u16 * 40).min(180) as u8;
                                [0x30, 0xE0, blue.max(0x40), 0x50]
                            } else {
                                [0xE0, 0x30, 0x30, 0x60]
                            };
                            let sx = rs.origin_x + (tx - ty) * half_tw;
                            let sy = rs.origin_y + (tx + ty) * half_th;
                            for py in 0..rs.tile_h {
                                let row_half = if py <= rs.tile_h / 2 {
                                    py * half_tw / half_th.max(1)
                                } else {
                                    (rs.tile_h - py) * half_tw / half_th.max(1)
                                };
                                let row_start = half_tw - row_half;
                                let row_end = half_tw + row_half;
                                for px in row_start..row_end {
                                    let fx = sx + px;
                                    let fy = sy + py;
                                    if fx >= 0 && fy >= 0
                                        && (fx as u32) < rs.width
                                        && (fy as u32) < rs.height
                                    {
                                        let off =
                                            ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                        if off + 3 < frame.len() {
                                            let a = color[3] as u16;
                                            let inv_a = 255 - a;
                                            frame[off] = ((color[0] as u16 * a
                                                + frame[off] as u16 * inv_a) / 255) as u8;
                                            frame[off + 1] = ((color[1] as u16 * a
                                                + frame[off + 1] as u16 * inv_a) / 255) as u8;
                                            frame[off + 2] = ((color[2] as u16 * a
                                                + frame[off + 2] as u16 * inv_a) / 255) as u8;
                                            frame[off + 3] = 255;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Draw build mode cursor
                if placer.active && !world_mode {
                    if let Some((hover_tx, hover_ty)) = placer.hover_tile {
                        if let Some(bb) = placer.selected_building() {
                            let def = &defs[bb.def_idx];
                            let island = &islands[current_island];
                            let island_map_idx = sim
                                .island_maps
                                .iter()
                                .position(|m| m.island_id == island.number);
                            let can_place = island_map_idx.map_or(false, |idx| {
                                can_place_building(
                                    island, &sim.island_maps[idx],
                                    hover_tx, hover_ty, def.width, def.height,
                                )
                            });
                            let color = if can_place {
                                [0x00, 0xFF, 0x00, 0x80] // Green: valid
                            } else {
                                [0xFF, 0x00, 0x00, 0x80] // Red: invalid
                            };

                            let half_tw = rs.tile_w / 2;
                            let half_th = rs.tile_h / 2;
                            let sprites = &sprites_by_zoom[sprite_zoom];
                            let cod_b = &cod.buildings[bb.def_idx];
                            let stride = (cod_b.anim_anz.max(1)
                                * cod_b.anim_add.max(1)) as usize;
                            let rot_offset = placer.orientation as usize * stride;
                            // First pass: blit each tile of the building
                            // sprite at 50% alpha so the player sees what
                            // they're about to place, animated frame and
                            // current rotation included.
                            for dy in 0..def.height as i32 {
                                for dx in 0..def.width as i32 {
                                    let tx = hover_tx + dx;
                                    let ty = hover_ty + dy;
                                    let sx = rs.origin_x + (tx - ty) * half_tw;
                                    let sy = rs.origin_y + (tx + ty) * half_th;
                                    let static_idx = bb.sprite_idx
                                        + rot_offset
                                        + dy as usize * def.width as usize
                                        + dx as usize;
                                    let anim_idx = anim_state
                                        .animate(static_idx as u16) as usize;
                                    if let Some(sp) = sprites.get(anim_idx) {
                                        let bw = sp.0 as i32;
                                        let bh = sp.1 as i32;
                                        let data = &sp.2;
                                        let dst_x = sx + (rs.tile_w - bw) / 2;
                                        let dst_y = sy - (bh - rs.tile_h);
                                        for py in 0..bh {
                                            for px in 0..bw {
                                                let off_src = ((py * bw + px) * 4) as usize;
                                                if off_src + 3 >= data.len() { continue; }
                                                if data[off_src + 3] == 0 { continue; }
                                                let fx = dst_x + px;
                                                let fy = dst_y + py;
                                                if fx < 0 || fy < 0 { continue; }
                                                if (fx as u32) >= rs.width
                                                    || (fy as u32) >= rs.height { continue; }
                                                let off_dst = ((fy as u32 * rs.width
                                                    + fx as u32) * 4) as usize;
                                                if off_dst + 3 >= frame.len() { continue; }
                                                frame[off_dst] = ((data[off_src] as u16
                                                    + frame[off_dst] as u16) / 2) as u8;
                                                frame[off_dst + 1] = ((data[off_src + 1] as u16
                                                    + frame[off_dst + 1] as u16) / 2) as u8;
                                                frame[off_dst + 2] = ((data[off_src + 2] as u16
                                                    + frame[off_dst + 2] as u16) / 2) as u8;
                                                frame[off_dst + 3] = 255;
                                            }
                                        }
                                    }
                                }
                            }
                            // Second pass: green/red footprint overlay so
                            // validity is unambiguous.
                            for dy in 0..def.height as i32 {
                                for dx in 0..def.width as i32 {
                                    let tx = hover_tx + dx;
                                    let ty = hover_ty + dy;
                                    let sx = rs.origin_x + (tx - ty) * half_tw;
                                    let sy = rs.origin_y + (tx + ty) * half_th;
                                    for py in 0..rs.tile_h {
                                        for px in 0..rs.tile_w {
                                            let fx = sx + px;
                                            let fy = sy + py;
                                            if fx >= 0
                                                && fy >= 0
                                                && (fx as u32) < rs.width
                                                && (fy as u32) < rs.height
                                            {
                                                let off = ((fy as u32 * rs.width + fx as u32)
                                                    * 4)
                                                    as usize;
                                                if off + 3 < frame.len() {
                                                    let a = color[3] as u16;
                                                    let inv_a = 255 - a;
                                                    frame[off] = ((color[0] as u16 * a
                                                        + frame[off] as u16 * inv_a)
                                                        / 255)
                                                        as u8;
                                                    frame[off + 1] = ((color[1] as u16 * a
                                                        + frame[off + 1] as u16 * inv_a)
                                                        / 255)
                                                        as u8;
                                                    frame[off + 2] = ((color[2] as u16 * a
                                                        + frame[off + 2] as u16 * inv_a)
                                                        / 255)
                                                        as u8;
                                                    frame[off + 3] = 255;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Draw inspection highlight
                if let Some(ref insp) = inspection {
                    let half_tw = rs.tile_w / 2;
                    let half_th = rs.tile_h / 2;
                    let (highlight_w, highlight_h) = if let Some(bi) = insp.building_idx {
                        let def = &defs[sim.buildings[bi].def_id as usize];
                        (def.width as i32, def.height as i32)
                    } else {
                        (1, 1)
                    };
                    let base_tx = if let Some(bi) = insp.building_idx {
                        sim.buildings[bi].tile_x as i32
                    } else {
                        insp.tile_x
                    };
                    let base_ty = if let Some(bi) = insp.building_idx {
                        sim.buildings[bi].tile_y as i32
                    } else {
                        insp.tile_y
                    };
                    let highlight_color = [0xFF, 0xFF, 0x00, 0x60]; // Yellow semi-transparent
                    for dy in 0..highlight_h {
                        for dx in 0..highlight_w {
                            let tx = base_tx + dx;
                            let ty = base_ty + dy;
                            let sx = rs.origin_x + (tx - ty) * half_tw;
                            let sy = rs.origin_y + (tx + ty) * half_th;
                            // Draw diamond shape for isometric tile
                            for py in 0..rs.tile_h {
                                let row_half = if py <= rs.tile_h / 2 {
                                    py * half_tw / half_th.max(1)
                                } else {
                                    (rs.tile_h - py) * half_tw / half_th.max(1)
                                };
                                let row_start = half_tw - row_half;
                                let row_end = half_tw + row_half;
                                for px in row_start..row_end {
                                    let fx = sx + px;
                                    let fy = sy + py;
                                    if fx >= 0
                                        && fy >= 0
                                        && (fx as u32) < rs.width
                                        && (fy as u32) < rs.height
                                    {
                                        let off =
                                            ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                        if off + 3 < frame.len() {
                                            let a = highlight_color[3] as u16;
                                            let inv_a = 255 - a;
                                            frame[off] = ((highlight_color[0] as u16 * a
                                                + frame[off] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 1] = ((highlight_color[1] as u16 * a
                                                + frame[off + 1] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 2] = ((highlight_color[2] as u16 * a
                                                + frame[off + 2] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 3] = 255;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Draw draft trade-route stops (yellow diamonds with index)
                if trade_route_mode && !world_mode {
                    let island_id = islands[current_island].number;
                    let half_tw = rs.tile_w / 2;
                    let half_th = rs.tile_h / 2;
                    for (i, &(iid, wx, wy, _mode)) in draft_route_stops.iter().enumerate() {
                        if iid != island_id {
                            continue;
                        }
                        let tx = wx as i32;
                        let ty = wy as i32;
                        let cx = rs.origin_x + (tx - ty) * half_tw + half_tw;
                        let cy = rs.origin_y + (tx + ty) * half_th + half_th;
                        // Draw a yellow diamond marker
                        let size: i32 = 6;
                        for dy in -size..=size {
                            for dx in -size..=size {
                                if dx.abs() + dy.abs() > size {
                                    continue;
                                }
                                let fx = cx + dx;
                                let fy = cy + dy;
                                if fx < 0 || fy < 0 { continue; }
                                if (fx as u32) >= rs.width
                                    || (fy as u32) >= rs.height { continue; }
                                let off = ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                if off + 3 >= frame.len() { continue; }
                                frame[off] = 0xFF;
                                frame[off + 1] = 0xD0;
                                frame[off + 2] = 0x00;
                                frame[off + 3] = 0xFF;
                            }
                        }
                        // Draw stop number on top of diamond using tiny_font
                        let label = format!("{}", i + 1);
                        tiny_font::draw_str(
                            &mut frame, rs.width, rs.height,
                            cx - 2, cy - 4, &label,
                            [0x00, 0x00, 0x00, 0xFF], 1,
                        );
                    }
                }

                // Draw construction overlay (blue tint + progress bar) for
                // any building still being built on the visible island.
                if !world_mode {
                    let island_id = islands[current_island].number;
                    let half_tw = rs.tile_w / 2;
                    let half_th = rs.tile_h / 2;
                    for b in sim.buildings.iter() {
                        if b.is_built() || b.island_id != island_id {
                            continue;
                        }
                        let def = &defs[b.def_id as usize];
                        let bw = def.width as i32;
                        let bh = def.height as i32;
                        let bx = b.tile_x as i32;
                        let by = b.tile_y as i32;
                        // Blue tint per tile (semi-transparent diamond).
                        let tint: [u8; 4] = [0x40, 0x80, 0xFF, 0x60];
                        for dy in 0..bh {
                            for dx in 0..bw {
                                let tx = bx + dx;
                                let ty = by + dy;
                                let sx = rs.origin_x + (tx - ty) * half_tw;
                                let sy = rs.origin_y + (tx + ty) * half_th;
                                for py in 0..rs.tile_h {
                                    let row_half = if py <= rs.tile_h / 2 {
                                        py * half_tw / half_th.max(1)
                                    } else {
                                        (rs.tile_h - py) * half_tw / half_th.max(1)
                                    };
                                    let row_start = half_tw - row_half;
                                    let row_end = half_tw + row_half;
                                    for px in row_start..row_end {
                                        let fx = sx + px;
                                        let fy = sy + py;
                                        if fx >= 0 && fy >= 0
                                            && (fx as u32) < rs.width
                                            && (fy as u32) < rs.height
                                        {
                                            let off = ((fy as u32 * rs.width + fx as u32)
                                                * 4) as usize;
                                            if off + 3 < frame.len() {
                                                let a = tint[3] as u16;
                                                let inv_a = 255 - a;
                                                frame[off] = ((tint[0] as u16 * a
                                                    + frame[off] as u16 * inv_a) / 255) as u8;
                                                frame[off + 1] = ((tint[1] as u16 * a
                                                    + frame[off + 1] as u16 * inv_a) / 255) as u8;
                                                frame[off + 2] = ((tint[2] as u16 * a
                                                    + frame[off + 2] as u16 * inv_a) / 255) as u8;
                                                frame[off + 3] = 255;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        // Progress bar: above the building (top-back of footprint).
                        let cx_tile = bx;
                        let cy_tile = by;
                        let bar_sx = rs.origin_x
                            + (cx_tile - cy_tile) * half_tw + half_tw - bw * half_tw / 2;
                        let bar_sy = rs.origin_y
                            + (cx_tile + cy_tile) * half_th - 4;
                        let bar_w = (bw + bh) * half_tw / 2;
                        let bar_h = 3i32;
                        let prog = b.construction_progress_128() as i32;
                        let filled = bar_w * prog / 128;
                        for by2 in 0..bar_h {
                            for bx2 in 0..bar_w {
                                let fx = bar_sx + bx2;
                                let fy = bar_sy + by2;
                                if fx < 0 || fy < 0 { continue; }
                                if (fx as u32) >= rs.width
                                    || (fy as u32) >= rs.height { continue; }
                                let off = ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                if off + 3 >= frame.len() { continue; }
                                let color = if bx2 < filled {
                                    [0x40, 0xFF, 0x40, 0xFF]
                                } else {
                                    [0x20, 0x20, 0x20, 0xFF]
                                };
                                frame[off] = color[0];
                                frame[off + 1] = color[1];
                                frame[off + 2] = color[2];
                                frame[off + 3] = color[3];
                            }
                        }
                    }
                }

                // Draw demolish mode hover highlight (red)
                if demolish_mode {
                    if let Some(bi) = demolish_hover {
                        let b = &sim.buildings[bi];
                        let def = &defs[b.def_id as usize];
                        let half_tw = rs.tile_w / 2;
                        let half_th = rs.tile_h / 2;
                        let demo_color = [0xFF, 0x20, 0x20, 0x70]; // Red semi-transparent
                        for dy in 0..def.height as i32 {
                            for dx in 0..def.width as i32 {
                                let tx = b.tile_x as i32 + dx;
                                let ty = b.tile_y as i32 + dy;
                                let sx = rs.origin_x + (tx - ty) * half_tw;
                                let sy = rs.origin_y + (tx + ty) * half_th;
                                for py in 0..rs.tile_h {
                                    for px in 0..rs.tile_w {
                                        let fx = sx + px;
                                        let fy = sy + py;
                                        if fx >= 0
                                            && fy >= 0
                                            && (fx as u32) < rs.width
                                            && (fy as u32) < rs.height
                                        {
                                            let off = ((fy as u32 * rs.width + fx as u32)
                                                * 4) as usize;
                                            if off + 3 < frame.len() {
                                                let a = demo_color[3] as u16;
                                                let inv_a = 255 - a;
                                                frame[off] = ((demo_color[0] as u16 * a
                                                    + frame[off] as u16 * inv_a)
                                                    / 255) as u8;
                                                frame[off + 1] = ((demo_color[1] as u16 * a
                                                    + frame[off + 1] as u16 * inv_a)
                                                    / 255) as u8;
                                                frame[off + 2] = ((demo_color[2] as u16 * a
                                                    + frame[off + 2] as u16 * inv_a)
                                                    / 255) as u8;
                                                frame[off + 3] = 255;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Day/night tint: one full cycle per 3000 game ticks
                // (~5 in-game minutes). Warm dusk/dawn tints, deep blue at
                // midnight, no tint at midday.
                {
                    let cycle = 3000u32;
                    let phase = (sim.game_clock % cycle) as f32 / cycle as f32;
                    let two_pi = std::f32::consts::TAU;
                    let dayness = (phase * two_pi).sin(); // -1..=1
                    if dayness < 0.95 {
                        // Map dayness to RGBA tint:
                        //   dayness =  1 → no overlay
                        //   dayness =  0 → warm orange dusk
                        //   dayness = -1 → dark blue night
                        let warm = (1.0 - dayness.abs()).max(0.0); // 0..1
                        let dark = (-dayness).max(0.0);            // 0..1
                        let r = (90.0 * warm + 5.0 * dark) as u16;
                        let g = (40.0 * warm + 10.0 * dark) as u16;
                        let b = (10.0 * warm + 60.0 * dark) as u16;
                        let alpha = ((warm * 60.0) + (dark * 90.0)) as u16;
                        let alpha = alpha.min(120) as u16;
                        if alpha > 0 {
                            let inv = 255 - alpha;
                            for px in frame.chunks_exact_mut(4) {
                                px[0] = ((px[0] as u16 * inv + r * alpha) / 255) as u8;
                                px[1] = ((px[1] as u16 * inv + g * alpha) / 255) as u8;
                                px[2] = ((px[2] as u16 * inv + b * alpha) / 255) as u8;
                            }
                        }
                    }
                }

                texture
                    .update(None, &frame, (rs.width * 4) as usize)
                    .expect("texture update failed");

                let dst_w = (rs.width as i32 * display_zoom) as u32;
                let dst_h = (rs.height as i32 * display_zoom) as u32;
                let dst_x = (WINDOW_W as i32 - dst_w as i32) / 2 + scroll_x;
                let dst_y = (WINDOW_H as i32 - dst_h as i32) / 2 + scroll_y;

                canvas
                    .copy(
                        &texture,
                        None,
                        Some(Rect::new(dst_x, dst_y, dst_w, dst_h)),
                    )
                    .ok();

                // Draw minimap in the bottom-right corner
                let minimap_max_w = 200u32;
                let minimap_max_h = 150u32;
                let minimap_margin = 8i32;

                // Scale to fit minimap bounds while preserving aspect ratio
                let scale_x = minimap_max_w as f64 / rs.width as f64;
                let scale_y = minimap_max_h as f64 / rs.height as f64;
                let mini_scale = scale_x.min(scale_y).min(1.0);
                let mini_w = (rs.width as f64 * mini_scale) as u32;
                let mini_h = (rs.height as f64 * mini_scale) as u32;

                if mini_w > 0 && mini_h > 0 {
                    // Render downscaled minimap RGBA
                    let mut mini_rgba = vec![0x20u8; (mini_w * mini_h * 4) as usize];
                    for my in 0..mini_h {
                        for mx in 0..mini_w {
                            let src_x = (mx as f64 / mini_scale) as u32;
                            let src_y = (my as f64 / mini_scale) as u32;
                            if src_x < rs.width && src_y < rs.height {
                                let src_off = ((src_y * rs.width + src_x) * 4) as usize;
                                let dst_off = ((my * mini_w + mx) * 4) as usize;
                                if src_off + 3 < frame.len() && dst_off + 3 < mini_rgba.len() {
                                    mini_rgba[dst_off] = frame[src_off];
                                    mini_rgba[dst_off + 1] = frame[src_off + 1];
                                    mini_rgba[dst_off + 2] = frame[src_off + 2];
                                    mini_rgba[dst_off + 3] = if frame[src_off + 3] > 0 { 220 } else { 80 };
                                }
                            }
                        }
                    }

                    // Draw viewport rectangle on minimap
                    // The viewport in texture coords:
                    let vp_left = ((-scroll_x) as f64 / display_zoom as f64 * mini_scale) as i32;
                    let vp_top = ((-scroll_y) as f64 / display_zoom as f64 * mini_scale) as i32;
                    let vp_w = (WINDOW_W as f64 / display_zoom as f64 * mini_scale) as i32;
                    let vp_h = (WINDOW_H as f64 / display_zoom as f64 * mini_scale) as i32;

                    // Adjust for centering offset
                    let center_off_x = ((WINDOW_W as i32 - dst_w as i32) / 2) as f64
                        / display_zoom as f64 * mini_scale;
                    let center_off_y = ((WINDOW_H as i32 - dst_h as i32) / 2) as f64
                        / display_zoom as f64 * mini_scale;
                    let vp_x = vp_left - center_off_x as i32;
                    let vp_y = vp_top - center_off_y as i32;

                    // Draw viewport rect border (white)
                    let white = [0xFF, 0xFF, 0xFF, 0xFF];
                    for px in vp_x.max(0)..=(vp_x + vp_w).min(mini_w as i32 - 1) {
                        for &py in &[vp_y, vp_y + vp_h] {
                            if py >= 0 && py < mini_h as i32 {
                                let off = ((py as u32 * mini_w + px as u32) * 4) as usize;
                                if off + 3 < mini_rgba.len() {
                                    mini_rgba[off..off + 4].copy_from_slice(&white);
                                }
                            }
                        }
                    }
                    for py in vp_y.max(0)..=(vp_y + vp_h).min(mini_h as i32 - 1) {
                        for &px in &[vp_x, vp_x + vp_w] {
                            if px >= 0 && px < mini_w as i32 {
                                let off = ((py as u32 * mini_w + px as u32) * 4) as usize;
                                if off + 3 < mini_rgba.len() {
                                    mini_rgba[off..off + 4].copy_from_slice(&white);
                                }
                            }
                        }
                    }

                    // Blit minimap to a texture and draw it
                    if let Ok(mut mini_tex) = texture_creator
                        .create_texture_streaming(PixelFormatEnum::RGBA32, mini_w, mini_h)
                    {
                        mini_tex.update(None, &mini_rgba, (mini_w * 4) as usize).ok();
                        mini_tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                        let mini_x = WINDOW_W as i32 - mini_w as i32 - minimap_margin;
                        let mini_y = WINDOW_H as i32 - mini_h as i32 - minimap_margin;

                        // Draw dark background behind minimap
                        canvas.set_draw_color(sdl2::pixels::Color::RGBA(0, 0, 0, 180));
                        canvas.fill_rect(Rect::new(
                            mini_x - 2, mini_y - 2,
                            mini_w + 4, mini_h + 4,
                        )).ok();

                        canvas.copy(
                            &mini_tex,
                            None,
                            Some(Rect::new(mini_x, mini_y, mini_w, mini_h)),
                        ).ok();

                        // Handle minimap clicks — clicking the minimap scrolls the main view
                        if minimap_clicked {
                            // Convert minimap click to texture coordinates
                            let click_tex_x = (minimap_click_x - mini_x) as f64 / mini_scale;
                            let click_tex_y = (minimap_click_y - mini_y) as f64 / mini_scale;
                            // Center the viewport on the clicked point
                            scroll_x = -(click_tex_x as i32 * display_zoom)
                                + WINDOW_W as i32 / 2;
                            scroll_y = -(click_tex_y as i32 * display_zoom)
                                + WINDOW_H as i32 / 2;
                            minimap_clicked = false;
                        }
                    }
                }
            }
        }

        // Draw population/economy HUD in top-left corner
        if show_hud && !placer.active {
        if let Some(ref player) = sim.players.first() {
            let tier_names = ["Pioneer", "Settler", "Citizen", "Merchant", "Aristocrat"];
            let hud_scale = 2u32;
            let line_h = (tiny_font::measure("X", hud_scale) + hud_scale) as i32 + 2;
            let hud_w = 220u32;
            let mut lines: Vec<String> = Vec::new();

            // Population tiers
            let total_pop: u32 = player.population.iter().sum();
            lines.push(format!("POP {}", total_pop));
            for i in 0..5 {
                let pop = player.population[i];
                if pop > 0 {
                    let sat = player.satisfaction[i] as u32 * 100 / 128;
                    let tax = player.tax_rates[i] as u32 * 100 / 128;
                    lines.push(format!(
                        " {}:{} S{}% T{}%",
                        &tier_names[i][..3], pop, sat, tax
                    ));
                }
            }
            // Economy
            lines.push(format!("GOLD {}", player.gold));

            let hud_h = (lines.len() as u32 * line_h as u32) + 8;
            let mut hud_buf = vec![0u8; (hud_w * hud_h * 4) as usize];
            // Fill with semi-transparent dark background
            for i in 0..(hud_w * hud_h) as usize {
                hud_buf[i * 4] = 0;
                hud_buf[i * 4 + 1] = 0;
                hud_buf[i * 4 + 2] = 0x10;
                hud_buf[i * 4 + 3] = 180;
            }
            // Render text lines
            for (li, line) in lines.iter().enumerate() {
                let color = if line.starts_with("GOLD") {
                    [0xFF, 0xD7, 0x00, 0xFF] // Gold color
                } else if line.starts_with("POP") {
                    [0xFF, 0xFF, 0xFF, 0xFF] // White
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF] // Light gray
                };
                tiny_font::draw_str(
                    &mut hud_buf, hud_w, hud_h,
                    4, 4 + li as i32 * line_h,
                    line, color, hud_scale,
                );
            }

            if let Ok(mut hud_tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, hud_w, hud_h)
            {
                hud_tex.update(None, &hud_buf, (hud_w * 4) as usize).ok();
                hud_tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                canvas.copy(&hud_tex, None, Some(Rect::new(8, 8, hud_w, hud_h))).ok();
            }
        }
        } // show_hud

        // Inspection detail panel (top-right). Multi-line read-out for
        // whatever the player right-clicked on — fed by the same
        // `inspection` state that drives the title-bar summary.
        if let Some(ref insp) = inspection {
            let mut lines: Vec<(String, [u8; 4])> = Vec::new();
            // Climate badge for the active island so the player can see at
            // a glance what plantations are buildable here.
            if !world_mode {
                let isl = &islands[current_island];
                let climate = anno_sim::climate::climate_for_y(
                    isl.y_pos as u32, 512,
                );
                lines.push((
                    format!("Climate: {}", climate.label()),
                    match climate {
                        anno_sim::climate::Climate::North => [0xAA, 0xCC, 0xFF, 0xFF],
                        anno_sim::climate::Climate::South => [0xFF, 0xCC, 0x88, 0xFF],
                    },
                ));
            }
            // Header.
            if let Some(bi) = insp.building_idx {
                let b = &sim.buildings[bi];
                let def = &defs[b.def_id as usize];
                let name = cod.buildings[b.def_id as usize]
                    .properties
                    .get("Name")
                    .cloned()
                    .unwrap_or_else(|| format!("Bldg#{}", b.def_id));
                lines.push((name, [0xFF, 0xD7, 0x00, 0xFF]));
                lines.push((
                    format!("Tile ({},{}) {}x{}", b.tile_x, b.tile_y, def.width, def.height),
                    [0xCC, 0xCC, 0xCC, 0xFF],
                ));
                lines.push((
                    format!("Owner: p{}  Kind: {}", b.owner, def.kind),
                    [0xCC, 0xCC, 0xCC, 0xFF],
                ));
                if !b.is_built() {
                    let pct = b.construction_progress_128() as u32 * 100 / 128;
                    lines.push((
                        format!("Construction: {pct}%"),
                        [0x66, 0xCC, 0xFF, 0xFF],
                    ));
                }
                if def.output_good != Good::None {
                    lines.push((
                        format!("Out: {:?} {}/{}", def.output_good, b.output_stock, def.storage_capacity),
                        [0xCC, 0xFF, 0xCC, 0xFF],
                    ));
                    if def.input_good_1 != Good::None {
                        lines.push((
                            format!("In1: {:?} {}", def.input_good_1, b.input_1_stock),
                            [0xCC, 0xCC, 0xCC, 0xFF],
                        ));
                    }
                    if def.input_good_2 != Good::None {
                        lines.push((
                            format!("In2: {:?} {}", def.input_good_2, b.input_2_stock),
                            [0xCC, 0xCC, 0xCC, 0xFF],
                        ));
                    }
                    let eff_pct = b.efficiency as u32 * 100 / 128;
                    lines.push((format!("Efficiency: {eff_pct}%"),
                        [0xCC, 0xCC, 0xCC, 0xFF]));
                }
                if def.maintenance_cost > 0 {
                    lines.push((
                        format!("Upkeep: {}/tick", def.maintenance_cost),
                        [0xCC, 0xCC, 0xCC, 0xFF],
                    ));
                }
            } else {
                lines.push((
                    format!("Tile ({},{})", insp.tile_x, insp.tile_y),
                    [0xFF, 0xD7, 0x00, 0xFF],
                ));
            }
            if let Some(wi) = insp.warehouse_idx {
                let wh = &sim.warehouses[wi];
                lines.push((
                    format!("Warehouse @ ({},{})  owner p{}", wh.tile_x, wh.tile_y, wh.owner),
                    [0xFF, 0xD7, 0x00, 0xFF],
                ));
                let stocks = wh.all_stock();
                if stocks.is_empty() {
                    lines.push(("(empty)".to_string(), [0x88, 0x88, 0x88, 0xFF]));
                } else {
                    for (g, qty, cap) in stocks.iter().take(8) {
                        lines.push((
                            format!("  {:?}: {}/{}", g, qty, cap),
                            [0xCC, 0xCC, 0xCC, 0xFF],
                        ));
                    }
                    if stocks.len() > 8 {
                        lines.push((format!("  +{} more…", stocks.len() - 8),
                            [0x88, 0x88, 0x88, 0xFF]));
                    }
                }
            }
            // Render panel.
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 280u32;
            let panel_h = (lines.len() as i32 * line_h + 8) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            for (i, (text, color)) in lines.iter().enumerate() {
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    4, 4 + i as i32 * line_h, text, *color, scale,
                );
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = WINDOW_W as i32 - panel_w as i32 - 8;
                let ty = 8i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Shortage banner (bottom-center): pulses yellow when one or more
        // goods can't meet half the current demand. Always visible — no
        // toggle, since this is a persistent gameplay warning.
        if let Some(p) = sim.players.first() {
            let shortages = anno_sim::population::severe_shortages(p);
            if !shortages.is_empty() {
                let names: Vec<String> = shortages.iter()
                    .map(|g| format!("{:?}", g))
                    .collect();
                let label = format!("SHORTAGE: {}", names.join(" "));
                let scale = 2u32;
                let text_w = tiny_font::measure(&label, scale) as i32;
                let banner_w = (text_w + 16) as u32;
                let banner_h = (5 * scale + 8) as u32;
                let mut buf = vec![0u8; (banner_w * banner_h * 4) as usize];
                // Pulse alpha by wall-clock so it's hard to ignore.
                let pulse = ((sim.game_clock as u32 % 20) as i32 - 10).abs();
                let bg_a = (140 + pulse * 8).min(220) as u8;
                for i in 0..(banner_w * banner_h) as usize {
                    buf[i * 4] = 0x40;
                    buf[i * 4 + 1] = 0x10;
                    buf[i * 4 + 2] = 0x00;
                    buf[i * 4 + 3] = bg_a;
                }
                tiny_font::draw_str(
                    &mut buf, banner_w, banner_h,
                    8, 4, &label,
                    [0xFF, 0xCC, 0x00, 0xFF], scale,
                );
                if let Ok(mut tex) = texture_creator
                    .create_texture_streaming(PixelFormatEnum::RGBA32, banner_w, banner_h)
                {
                    tex.update(None, &buf, (banner_w * 4) as usize).ok();
                    tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                    let tx = (WINDOW_W as i32 - banner_w as i32) / 2;
                    let ty = WINDOW_H as i32 - banner_h as i32 - 60;
                    canvas.copy(&tex, None, Some(Rect::new(tx, ty, banner_w, banner_h))).ok();
                }
            }
        }

        // Draw tax adjustment panel (center-top)
        if tax_panel {
            if let Some(ref player) = sim.players.first() {
                let tier_names = ["Pioneer", "Settler", "Citizen", "Merchant", "Aristocrat"];
                let tax_scale = 2u32;
                let line_h = 14i32;
                let panel_w = 280u32;
                let panel_h = (7 * line_h as u32) + 12;
                let mut panel_buf = vec![0u8; (panel_w * panel_h * 4) as usize];
                // Dark background
                for i in 0..(panel_w * panel_h) as usize {
                    panel_buf[i * 4] = 0;
                    panel_buf[i * 4 + 1] = 0;
                    panel_buf[i * 4 + 2] = 0x18;
                    panel_buf[i * 4 + 3] = 210;
                }
                // Title
                tiny_font::draw_str(
                    &mut panel_buf, panel_w, panel_h,
                    4, 4, "TAX RATES",
                    [0xFF, 0xD7, 0x00, 0xFF], tax_scale,
                );
                // Income display
                let income = player.calculate_income();
                let costs = player.calculate_costs();
                let net = player.net_balance();
                tiny_font::draw_str(
                    &mut panel_buf, panel_w, panel_h,
                    120, 4,
                    &format!("Inc:{} Cost:{} Net:{}", income, costs, net),
                    [0xAA, 0xAA, 0xAA, 0xFF], tax_scale,
                );
                // Per-tier rows
                for i in 0..5 {
                    let y = 4 + (i + 2) as i32 * line_h;
                    let rate = player.tax_rates[i] as u32 * 100 / 128;
                    let sat = player.satisfaction[i] as u32 * 100 / 128;
                    let pop = player.population[i];
                    let selected = i == tax_tier;
                    let arrow = if selected { ">" } else { " " };
                    let line = format!(
                        "{}{} Tax:{}% Sat:{}% Pop:{}",
                        arrow, tier_names[i], rate, sat, pop
                    );
                    let color = if selected {
                        [0xFF, 0xFF, 0x00, 0xFF] // Yellow for selected
                    } else {
                        [0xCC, 0xCC, 0xCC, 0xFF]
                    };
                    tiny_font::draw_str(
                        &mut panel_buf, panel_w, panel_h,
                        4, y, &line, color, tax_scale,
                    );
                }

                if let Ok(mut tax_tex) = texture_creator
                    .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
                {
                    tax_tex.update(None, &panel_buf, (panel_w * 4) as usize).ok();
                    tax_tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                    let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                    canvas.copy(&tax_tex, None, Some(Rect::new(tx, 8, panel_w, panel_h))).ok();
                }
            }
        }

        // Draw diplomacy panel (center-top)
        if diplomacy_panel {
            use anno_sim::combat::Diplomacy;
            let dscale = 2u32;
            let line_h = 14i32;
            let panel_w = 320u32;
            let rows = 6u32; // counterparts 1..6
            let panel_h = (rows + 3) * line_h as u32 + 12;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 210;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4, "DIPLOMACY",
                [0xFF, 0xD7, 0x00, 0xFF], dscale,
            );
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4 + line_h,
                "Up/Down=player Left/Right=cycle",
                [0xAA, 0xAA, 0xAA, 0xFF], dscale,
            );
            // Per-counterpart rows
            for tgt in 1u8..=6 {
                let y = 4 + (tgt as i32 + 1) * line_h;
                let rel = sim.diplomacy.get(0, tgt);
                let rel_str = match rel {
                    Diplomacy::Allied => "ALLIED",
                    Diplomacy::Neutral => "NEUTRAL",
                    Diplomacy::War => "WAR",
                };
                let selected = tgt == diplomacy_target;
                let arrow = if selected { ">" } else { " " };
                let alive = sim.players.get(tgt as usize)
                    .map(|p| p.state != anno_sim::player::PlayerState::Empty
                            && p.state != anno_sim::player::PlayerState::Defeated)
                    .unwrap_or(false);
                let suffix = if alive { "" } else { " (no player)" };
                let line = format!("{}Player {}: {}{}", arrow, tgt, rel_str, suffix);
                let color = if selected {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else {
                    match rel {
                        Diplomacy::War => [0xFF, 0x66, 0x66, 0xFF],
                        Diplomacy::Allied => [0x66, 0xFF, 0x66, 0xFF],
                        Diplomacy::Neutral => [0xCC, 0xCC, 0xCC, 0xFF],
                    }
                };
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    4, y, &line, color, dscale,
                );
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                canvas.copy(&tex, None, Some(Rect::new(tx, 8, panel_w, panel_h))).ok();
            }
        }

        // Draw economy graph panel (G key)
        if graph_panel {
            let panel_w = 360u32;
            let panel_h = 240u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            // Background
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4, "ECONOMY HISTORY (G to close)",
                [0xFF, 0xD7, 0x00, 0xFF], 2,
            );
            let series_gold = sim.history.gold_series();
            let series_pop = sim.history.population_series();
            let series_sat = sim.history.satisfaction_series();
            // Plot each series in a horizontal band.
            let bands: [(&str, [u8; 4], i64, i64, Vec<i64>); 3] = [
                (
                    "GOLD",
                    [0xFF, 0xD0, 0x00, 0xFF],
                    series_gold.iter().copied().min().unwrap_or(0).min(0) as i64,
                    series_gold.iter().copied().max().unwrap_or(1).max(1) as i64,
                    series_gold.iter().map(|&g| g as i64).collect(),
                ),
                (
                    "POPULATION",
                    [0x40, 0xFF, 0x60, 0xFF],
                    0,
                    series_pop.iter().copied().max().unwrap_or(1).max(1) as i64,
                    series_pop.iter().map(|&p| p as i64).collect(),
                ),
                (
                    "AVG SATISFACTION (0-128)",
                    [0x60, 0xC0, 0xFF, 0xFF],
                    0,
                    128,
                    series_sat.iter().map(|&s| s as i64).collect(),
                ),
            ];
            let band_h = 60i32;
            let band_top0 = 28i32;
            for (i, (label, color, ymin, ymax, samples)) in bands.iter().enumerate() {
                let band_top = band_top0 + i as i32 * (band_h + 8);
                let band_left = 8i32;
                let band_w = panel_w as i32 - 16;
                // Frame: bottom line
                for x in 0..band_w {
                    let fx = band_left + x;
                    let fy = band_top + band_h - 1;
                    let off = ((fy as u32 * panel_w + fx as u32) * 4) as usize;
                    if off + 3 < buf.len() {
                        buf[off] = 0x40; buf[off + 1] = 0x40;
                        buf[off + 2] = 0x40; buf[off + 3] = 0xFF;
                    }
                }
                // Label and current value
                let cur = samples.last().copied().unwrap_or(0);
                let line = format!("{label}: now={cur} max={ymax}");
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    band_left, band_top, &line,
                    [0xCC, 0xCC, 0xCC, 0xFF], 1,
                );
                if samples.is_empty() { continue; }
                let span = (ymax - ymin).max(1);
                let n = samples.len();
                let plot_top = band_top + 10;
                let plot_h = band_h - 12;
                for k in 0..n {
                    let v = samples[k];
                    let frac = ((v - ymin) as f64 / span as f64).clamp(0.0, 1.0);
                    let h_pix = (frac * plot_h as f64) as i32;
                    let x = band_left + (k as i32 * band_w / n.max(1) as i32);
                    let y = plot_top + plot_h - h_pix;
                    // Plot as a small filled column.
                    for py in y..(plot_top + plot_h) {
                        if py < 0 || py >= panel_h as i32 { continue; }
                        let off = ((py as u32 * panel_w + x as u32) * 4) as usize;
                        if off + 3 < buf.len() {
                            buf[off] = color[0];
                            buf[off + 1] = color[1];
                            buf[off + 2] = color[2];
                            buf[off + 3] = color[3];
                        }
                    }
                }
            }

            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Production overview panel (P key) — aggregates per-Good across the
        // human player's buildings: producer count, average efficiency, and
        // total stock pooled across warehouses.
        if prod_panel {
            use std::collections::HashMap;
            let player_owner = 0u8;
            let mut acc: HashMap<Good, (u32, u32, u32)> = HashMap::new();
            // (n_producers, sum_efficiency_128, _placeholder)
            for b in sim.buildings.iter().filter(|b| b.owner == player_owner) {
                let def = &defs[b.def_id as usize];
                if def.output_good == Good::None { continue; }
                let entry = acc.entry(def.output_good).or_insert((0, 0, 0));
                entry.0 += 1;
                entry.1 += b.efficiency as u32;
            }
            let mut stock_by_good: HashMap<Good, u32> = HashMap::new();
            for w in sim.warehouses.iter().filter(|w| w.owner == player_owner) {
                for (g, qty, _cap) in w.all_stock() {
                    *stock_by_good.entry(g).or_insert(0) += qty as u32;
                }
            }
            let mut rows: Vec<(Good, u32, u32, u32)> = acc
                .into_iter()
                .map(|(g, (n, eff_sum, _))| {
                    let stock = *stock_by_good.get(&g).unwrap_or(&0);
                    (g, n, eff_sum / n.max(1), stock)
                })
                .collect();
            // Also list goods we have stock of but don't produce.
            for (g, stock) in &stock_by_good {
                if !rows.iter().any(|(rg, _, _, _)| rg == g) {
                    rows.push((*g, 0, 0, *stock));
                }
            }
            rows.sort_by(|a, b| b.3.cmp(&a.3).then(format!("{:?}", a.0).cmp(&format!("{:?}", b.0))));

            let panel_w = 460u32;
            let line_h = 12i32;
            let header_h = 32i32;
            let visible_rows = rows.len().min(20) as i32;
            let panel_h = (header_h + visible_rows * line_h + 12) as u32;
            let spark_w = 90i32;
            let spark_h = 8i32;
            let spark_x0 = panel_w as i32 - spark_w - 8;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4, "PRODUCTION (P to close)",
                [0xFF, 0xD7, 0x00, 0xFF], 2,
            );
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4 + 14,
                "Good      n  eff%  stock  buy/sell  history",
                [0xCC, 0xCC, 0xCC, 0xFF], 1,
            );
            for (i, (g, n, eff, stock)) in rows.iter().take(20).enumerate() {
                let y = header_h + i as i32 * line_h;
                let eff_pct = (*eff * 100 / 128).min(999);
                let label_full = format!("{:?}", g);
                let label = if label_full.len() > 9 {
                    label_full[..9].to_string()
                } else {
                    label_full
                };
                let p = sim.current_price(*g);
                let row = format!(
                    "{:<9} {:>2}  {:>3}%  {:>5}  {:>3}/{:<3}",
                    label, n, eff_pct, stock, p.buy, p.sell,
                );
                let color = if *n > 0 {
                    [0xCC, 0xFF, 0xCC, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                tiny_font::draw_str(&mut buf, panel_w, panel_h, 4, y, &row, color, 1);

                // Sparkline: stock history for this good across the last 120
                // economy ticks. Single-pixel column per sample, height
                // scaled to the per-row max (so each row is self-normalised).
                let series = sim.history.stock_series(*g);
                if !series.is_empty() {
                    let max = series.iter().copied().max().unwrap_or(1).max(1);
                    let n = series.len() as i32;
                    for (k, &v) in series.iter().enumerate() {
                        let sx = spark_x0 + k as i32 * spark_w / n.max(1);
                        let frac = v as f64 / max as f64;
                        let bar = (frac * spark_h as f64).round() as i32;
                        let bar = bar.min(spark_h);
                        for dy in 0..bar {
                            let fy = y + spark_h - 1 - dy;
                            let off = ((fy as u32 * panel_w + sx as u32) * 4) as usize;
                            if off + 3 < buf.len() {
                                buf[off] = color[0];
                                buf[off + 1] = color[1];
                                buf[off + 2] = color[2];
                                buf[off + 3] = 0xFF;
                            }
                        }
                    }
                }
            }

            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Player roster panel (Tab) — quick at-a-glance view of every slot.
        if roster_panel {
            use anno_sim::combat::Diplomacy;
            use anno_sim::player::PlayerState;
            let panel_w = 480u32;
            let header_h = 30i32;
            let line_h = 14i32;
            let n_rows = 7;
            let panel_h = (header_h + line_h * n_rows + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4, "PLAYERS (Tab to close)",
                [0xFF, 0xD7, 0x00, 0xFF], 2,
            );
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 18,
                "#  state           gold     pop  units  vs you",
                [0xCC, 0xCC, 0xCC, 0xFF], 1,
            );
            for slot in 0..n_rows as usize {
                let y = header_h + slot as i32 * line_h;
                let p = sim.players.get(slot);
                let (state_str, color, gold, pop, is_human) = match p {
                    Some(pl) => {
                        let s = match pl.state {
                            PlayerState::HumanActive => "Human",
                            PlayerState::Empty => "(empty)",
                            PlayerState::AiDefending => "AI(defend)",
                            PlayerState::AiActive => "AI(active)",
                            PlayerState::AiAllied => "AI(ally)",
                            PlayerState::Defeated => "DEFEATED",
                        };
                        let c = match pl.state {
                            PlayerState::HumanActive => [0xFF, 0xFF, 0xFF, 0xFF],
                            PlayerState::Empty => [0x66, 0x66, 0x66, 0xFF],
                            PlayerState::Defeated => [0xFF, 0x66, 0x66, 0xFF],
                            _ => [0xCC, 0xCC, 0xCC, 0xFF],
                        };
                        (s, c, pl.gold, pl.population.iter().sum::<u32>(),
                         pl.state == PlayerState::HumanActive)
                    }
                    None => ("(none)", [0x44, 0x44, 0x44, 0xFF], 0, 0u32, false),
                };
                let units = sim.military_units.iter()
                    .filter(|u| u.is_alive() && u.owner as usize == slot)
                    .count();
                let dip_str = if is_human || slot == 0 {
                    "—".to_string()
                } else {
                    match sim.diplomacy.get(0, slot as u8) {
                        Diplomacy::Allied => "ALLIED".into(),
                        Diplomacy::Neutral => "NEUTRAL".into(),
                        Diplomacy::War => "WAR".into(),
                    }
                };
                let row = format!(
                    "{}  {:<14} {:>7}  {:>5}  {:>4}   {}",
                    slot, state_str, gold, pop, units, dip_str,
                );
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    4, y, &row, color, 1,
                );
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Scenario picker (F2). Re-execs the binary with the chosen .szs.
        if scenario_picker {
            let panel_w = 480u32;
            let line_h = 12i32;
            let header_h = 30i32;
            let visible = scenario_files.len().min(20) as i32;
            let panel_h = (header_h + visible * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4, "SCENARIO PICKER (Up/Down, Enter to load, F2/Esc to close)",
                [0xFF, 0xD7, 0x00, 0xFF], 1,
            );
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 16,
                "Re-launches the game with the chosen scenario.",
                [0xCC, 0xCC, 0xCC, 0xFF], 1,
            );
            // Compute a small visible window around `scenario_sel`.
            let total = scenario_files.len();
            let max_visible = visible as usize;
            let start = if total <= max_visible {
                0
            } else {
                scenario_sel.saturating_sub(max_visible / 2)
                    .min(total - max_visible)
            };
            for (row, idx) in (start..(start + max_visible).min(total)).enumerate() {
                let path = &scenario_files[idx];
                let label = path.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let arrow = if idx == scenario_sel { "> " } else { "  " };
                let line = format!("{arrow}{label}");
                let color = if idx == scenario_sel {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                let y = header_h + row as i32 * line_h;
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    4, y, &line, color, 1,
                );
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Context menu (Shift+RMB). Floats at the cursor; Up/Dn picks,
        // Enter activates the action, Esc closes.
        if let Some(ref menu) = context_menu {
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 160u32;
            let panel_h = (menu.actions.len() as i32 * line_h + 8) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 230;
            }
            for (i, &act) in menu.actions.iter().enumerate() {
                let arrow = if i == menu.sel { ">" } else { " " };
                let line = format!("{arrow} {}", act);
                let color = if i == menu.sel {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                tiny_font::draw_str(&mut buf, panel_w, panel_h,
                    4, 4 + i as i32 * line_h, &line, color, scale);
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = menu.screen_x.min(WINDOW_W as i32 - panel_w as i32 - 4);
                let ty = menu.screen_y.min(WINDOW_H as i32 - panel_h as i32 - 4);
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Route list panel (Shift+R): list of player-owned trade routes
        // with stop count + ship count; Backspace deletes the selected
        // route and any ships running it.
        if route_list_panel {
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 380u32;
            let header_h = 28i32;
            let routes: Vec<&anno_sim::trade::TradeRoute> = sim.trade_routes
                .iter().filter(|r| r.owner == 0).collect();
            let visible = routes.len().max(1);
            let panel_h = (header_h + visible as i32 * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4, "ROUTES (Shift+R close, Up/Dn pick, Bksp delete)",
                [0xFF, 0xD7, 0x00, 0xFF], 1,
            );
            if routes.is_empty() {
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    4, header_h, "(no active routes)",
                    [0x88, 0x88, 0x88, 0xFF], scale,
                );
            } else {
                for (i, r) in routes.iter().enumerate() {
                    let ships = sim.trade_ships.iter()
                        .filter(|s| s.route_id == r.id && s.active)
                        .count();
                    let arrow = if i == route_list_sel { ">" } else { " " };
                    let line = format!(
                        "{} route {}  stops:{}  ships:{}  active:{}",
                        arrow, r.id, r.stops.len(), ships, r.active,
                    );
                    let color = if i == route_list_sel {
                        [0xFF, 0xFF, 0x00, 0xFF]
                    } else { [0xCC, 0xCC, 0xCC, 0xFF] };
                    tiny_font::draw_str(&mut buf, panel_w, panel_h,
                        4, header_h + i as i32 * line_h, &line, color, scale);
                }
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Settings panel (F10). Up/Down picks a row, Left/Right adjusts;
        // each edit auto-persists to ~/.config/anno/settings.toml.
        if settings_panel {
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 360u32;
            let header_h = 30i32;
            let n = anno_sim::settings::Settings::COUNT as i32;
            let panel_h = (header_h + n * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4,
                "SETTINGS (Up/Dn pick, Left/Right ±5, F10/Esc close)",
                [0xFF, 0xD7, 0x00, 0xFF], 1,
            );
            for i in 0..n {
                let y = header_h + i * line_h;
                let label = anno_sim::settings::Settings::label(i as usize);
                let value = settings.value(i as usize);
                let arrow = if i as usize == settings_sel { ">" } else { " " };
                let line = format!("{arrow} {:<14} {:>3}", label, value);
                let color = if i as usize == settings_sel {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                tiny_font::draw_str(&mut buf, panel_w, panel_h, 4, y, &line, color, scale);
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Save / load slot picker (F3). 10 named slots per scenario.
        if save_panel {
            let panel_w = 480u32;
            let header_h = 28i32;
            let line_h = 12i32;
            let panel_h = (header_h + 10 * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4, "SAVE SLOTS (Up/Dn pick, S=save, L=load, F3/Esc close)",
                [0xFF, 0xD7, 0x00, 0xFF], 1,
            );
            for slot in 0..10 {
                let path = slot_path(slot);
                let meta = std::fs::metadata(&path).ok();
                let exists = meta.is_some();
                let info = match meta {
                    Some(m) => {
                        let size_kb = (m.len() / 1024).max(1);
                        format!("{:>5} KiB", size_kb)
                    }
                    None => "(empty)".to_string(),
                };
                let arrow = if slot == save_sel { ">" } else { " " };
                let line = format!("{arrow} slot {}  {}", slot, info);
                let color = if slot == save_sel {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else if exists {
                    [0xCC, 0xFF, 0xCC, 0xFF]
                } else {
                    [0x88, 0x88, 0x88, 0xFF]
                };
                let y = header_h + slot as i32 * line_h;
                tiny_font::draw_str(&mut buf, panel_w, panel_h, 4, y, &line, color, 1);
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Per-island warehouse panel (U): table of every player-owned
        // warehouse on the active island, each as a column with its top
        // goods. Useful when you've placed multiple Kontors and want to
        // see at a glance where stock is piling up vs. running dry.
        if wh_panel {
            let island_id = if !world_mode {
                Some(islands[current_island].number)
            } else {
                None
            };
            let warehouses: Vec<&anno_sim::warehouse::Warehouse> = sim
                .warehouses
                .iter()
                .filter(|w| w.active && w.owner == 0
                    && island_id.map_or(true, |id| w.island_id == id))
                .collect();
            // Union of goods that any warehouse on this island carries.
            let mut all_goods: Vec<Good> = Vec::new();
            for wh in &warehouses {
                for (g, _, _) in wh.all_stock() {
                    if !all_goods.contains(&g) {
                        all_goods.push(g);
                    }
                }
            }
            all_goods.sort_by_key(|g| format!("{:?}", g));

            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let col_w = 110i32;
            let panel_w = (10 + warehouses.len().max(1) as i32 * col_w) as u32;
            let panel_h = (24 + line_h * (all_goods.len() as i32 + 2)) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4,
                "WAREHOUSES (U/Esc close)",
                [0xFF, 0xD7, 0x00, 0xFF], 2,
            );
            // Column headers: tile coords.
            for (col, wh) in warehouses.iter().enumerate() {
                let x = 10 + col as i32 * col_w;
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    x, 22,
                    &format!("@({},{})", wh.tile_x, wh.tile_y),
                    [0xCC, 0xCC, 0xCC, 0xFF], scale,
                );
            }
            // Rows: per-good stock across each warehouse column.
            for (row, &g) in all_goods.iter().enumerate() {
                let y = 22 + line_h + row as i32 * line_h;
                let label = format!("{:?}", g);
                let label = if label.len() > 9 { label[..9].to_string() } else { label };
                // Row label spans the full panel left side.
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    4, y, &label, [0xAA, 0xAA, 0xAA, 0xFF], scale,
                );
                for (col, wh) in warehouses.iter().enumerate() {
                    let x = 10 + col as i32 * col_w + 50;
                    let qty = wh.stock(g);
                    let cap = wh.capacity(g);
                    let color = if qty == 0 {
                        [0x55, 0x55, 0x55, 0xFF]
                    } else if qty * 4 < cap {
                        [0xFF, 0x88, 0x66, 0xFF]
                    } else {
                        [0xCC, 0xFF, 0xCC, 0xFF]
                    };
                    tiny_font::draw_str(
                        &mut buf, panel_w, panel_h,
                        x, y, &format!("{}/{}", qty, cap),
                        color, scale,
                    );
                }
            }
            if all_goods.is_empty() {
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    4, 22 + line_h,
                    "(all warehouses empty)",
                    [0x88, 0x88, 0x88, 0xFF], scale,
                );
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Objectives panel (?). Read-only — pulls live from sim.objectives.
        if obj_panel {
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 360u32;
            let header_h = 28i32;
            let n = sim.objectives.items.len() as i32;
            let panel_h = (header_h + n * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            let (done, total) = sim.objectives.progress();
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4,
                &format!("OBJECTIVES {}/{} (?/Esc close)", done, total),
                [0xFF, 0xD7, 0x00, 0xFF], 2,
            );
            for (i, (obj, done_flag)) in sim.objectives.items.iter().enumerate() {
                let y = header_h + i as i32 * line_h;
                let mark = if *done_flag { "[x]" } else { "[ ]" };
                let line = format!("{} {}", mark, obj.label());
                let color = if *done_flag {
                    [0x66, 0xFF, 0x66, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                tiny_font::draw_str(&mut buf, panel_w, panel_h, 4, y, &line, color, scale);
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Ship freight panel (J): one row per active TradeShip with route,
        // state, cargo, and profit. Helpful for spotting idle ships or
        // ones that aren't picking up the goods you expect.
        if ship_panel {
            use anno_sim::trade::ShipState;
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 480u32;
            let active_ships: Vec<(usize, &anno_sim::trade::TradeShip)> = sim
                .trade_ships
                .iter()
                .enumerate()
                .filter(|(_, s)| s.active)
                .collect();
            let panel_h = (28 + (active_ships.len() as i32 * 2 + 1).max(2) * line_h) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4,
                "FLEET (J/Esc close)",
                [0xFF, 0xD7, 0x00, 0xFF], 2,
            );
            if active_ships.is_empty() {
                tiny_font::draw_str(
                    &mut buf, panel_w, panel_h,
                    4, 24, "(no active ships)",
                    [0x88, 0x88, 0x88, 0xFF], scale,
                );
            } else {
                for (row, (idx, ship)) in active_ships.iter().enumerate() {
                    let y0 = 22 + row as i32 * 2 * line_h;
                    let stops = sim.trade_routes.iter()
                        .find(|r| r.id == ship.route_id)
                        .map(|r| r.stops.len())
                        .unwrap_or(0);
                    let state_str = match ship.state {
                        ShipState::Sailing => "sailing",
                        ShipState::Trading => "trading",
                        ShipState::Waiting => "waiting",
                        ShipState::Idle    => "idle",
                    };
                    let state_color = match ship.state {
                        ShipState::Sailing => [0x66, 0xCC, 0xFF, 0xFF],
                        ShipState::Trading => [0xCC, 0xFF, 0xCC, 0xFF],
                        ShipState::Waiting => [0xFF, 0xCC, 0x66, 0xFF],
                        ShipState::Idle    => [0xAA, 0xAA, 0xAA, 0xFF],
                    };
                    let head = format!(
                        "#{} p{}  route {}  stop {}/{}  @({},{}) {} profit:{}g",
                        idx, ship.owner, ship.route_id,
                        ship.current_stop + 1, stops.max(1),
                        ship.world_x, ship.world_y, state_str, ship.profit,
                    );
                    tiny_font::draw_str(
                        &mut buf, panel_w, panel_h,
                        4, y0, &head, state_color, scale,
                    );
                    let cargo = if ship.cargo.is_empty() {
                        "  cargo: (empty)".to_string()
                    } else {
                        let parts: Vec<String> = ship.cargo.iter()
                            .filter(|(_, q)| *q > 0)
                            .take(8)
                            .map(|(g, q)| format!("{:?}:{}", g, q))
                            .collect();
                        format!(
                            "  cargo {}/{}: {}",
                            ship.cargo_total,
                            anno_sim::trade::SHIP_CARGO_CAPACITY,
                            parts.join(" "),
                        )
                    };
                    tiny_font::draw_str(
                        &mut buf, panel_w, panel_h,
                        4, y0 + line_h, &cargo,
                        [0xCC, 0xCC, 0xCC, 0xFF], scale,
                    );
                }
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Market panel (A) — buy / sell goods at the player's first
        // active warehouse using `prices::price_of`.
        if market_panel {
            const GOODS: &[Good] = &[
                Good::Wood, Good::Iron, Good::Ore, Good::Gold,
                Good::Wool, Good::Sugar, Good::Tobacco, Good::Cattle,
                Good::Grain, Good::Flour, Good::Food, Good::Alcohol,
                Good::Cloth, Good::Clothing, Good::Jewelry, Good::Tools,
                Good::Bricks, Good::Swords, Good::Cannons, Good::Muskets,
                Good::Stone, Good::Cocoa, Good::Spices, Good::Hides,
                Good::Cotton, Good::Silk, Good::Fish, Good::Grapes,
                Good::GoldOre, Good::TobaccoProducts,
            ];
            let panel_w = 380u32;
            let header_h = 30i32;
            let line_h = 12i32;
            let visible = (GOODS.len() as i32).min(20);
            let panel_h = (header_h + visible * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 4, "MARKET (A close  Up/Dn pick  Lt/Rt ±10  Shift=±100)",
                [0xFF, 0xD7, 0x00, 0xFF], 1,
            );
            let gold_now = sim.players.first().map(|p| p.gold).unwrap_or(0);
            tiny_font::draw_str(
                &mut buf, panel_w, panel_h,
                4, 16, &format!("gold:{gold_now}  Good      stock  buy/sell"),
                [0xCC, 0xCC, 0xCC, 0xFF], 1,
            );
            // Sliding window centered on selection.
            let total = GOODS.len();
            let max_visible = visible as usize;
            let start = if total <= max_visible {
                0
            } else {
                market_sel.saturating_sub(max_visible / 2)
                    .min(total - max_visible)
            };
            // Pull stock for player 0's first active warehouse.
            let stock_for = |g: Good| -> u16 {
                sim.warehouses.iter()
                    .find(|w| w.active && w.owner == 0)
                    .map(|w| w.stock(g))
                    .unwrap_or(0)
            };
            for (row, idx) in (start..(start + max_visible).min(total)).enumerate() {
                let g = GOODS[idx];
                let price = sim.current_price(g);
                let stock = stock_for(g);
                let label_full = format!("{:?}", g);
                let label = if label_full.len() > 9 {
                    label_full[..9].to_string()
                } else {
                    label_full
                };
                let arrow = if idx == market_sel { ">" } else { " " };
                let line = format!(
                    "{arrow} {:<9}  {:>4}   {:>3}/{:<3}",
                    label, stock, price.buy, price.sell,
                );
                let color = if idx == market_sel {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                let y = header_h + row as i32 * line_h;
                tiny_font::draw_str(&mut buf, panel_w, panel_h, 4, y, &line, color, 1);
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        // Chat overlay (bottom-left). Each entry sticks for 10s, plus a
        // live input box while typing.
        {
            let chat_ttl = std::time::Duration::from_secs(10);
            while chat_log.front()
                .map(|(_, t)| t.elapsed() > chat_ttl)
                .unwrap_or(false)
            {
                chat_log.pop_front();
            }
            if !chat_log.is_empty() || chat_active {
                let scale = 1u32;
                let line_h = 8i32;
                let panel_w = 360u32;
                let n_lines = chat_log.len() as i32 + if chat_active { 1 } else { 0 };
                let panel_h = (n_lines.max(1) * line_h + 6) as u32;
                let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
                for i in 0..(panel_w * panel_h) as usize {
                    buf[i * 4] = 0;
                    buf[i * 4 + 1] = 0;
                    buf[i * 4 + 2] = 0x10;
                    buf[i * 4 + 3] = 180;
                }
                let mut y = 2;
                for (line, _) in chat_log.iter() {
                    let color = if line.starts_with("you:") {
                        [0xCC, 0xFF, 0xCC, 0xFF]
                    } else if line.starts_with("[diplo]") {
                        [0xFF, 0xCC, 0x66, 0xFF] // amber
                    } else if line.starts_with("[build]") {
                        [0x99, 0xCC, 0xFF, 0xFF] // cool blue
                    } else {
                        [0xFF, 0xFF, 0xFF, 0xFF]
                    };
                    tiny_font::draw_str(
                        &mut buf, panel_w, panel_h,
                        4, y, line, color, scale,
                    );
                    y += line_h;
                }
                if chat_active {
                    let prompt = format!("> {}_", chat_input);
                    tiny_font::draw_str(
                        &mut buf, panel_w, panel_h,
                        4, y, &prompt,
                        [0xFF, 0xD7, 0x00, 0xFF], scale,
                    );
                }
                if let Ok(mut tex) = texture_creator
                    .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
                {
                    tex.update(None, &buf, (panel_w * 4) as usize).ok();
                    tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                    let tx = 8i32;
                    let ty = WINDOW_H as i32 - panel_h as i32 - 8;
                    canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
                }
            }
        }

        // Title bar with simulation status
        let (minutes, seconds) = sim.display_time();
        let speed_label = if sim.paused {
            "PAUSED".to_string()
        } else {
            format!("{}x", sim.speed_multiplier)
        };
        let carriers = sim.figures.iter().filter(|f| f.is_active()).count();
        let human_gold = sim.players.first().map(|p| p.gold).unwrap_or(0);
        let zoom_label = ["GFX", "MGFX", "SGFX"][sprite_zoom];

        let title = if trade_route_mode {
            let last_mode = draft_route_stops.last()
                .map(|s| ["LOAD", "UNLOAD", "BOTH"][s.3 as usize])
                .unwrap_or("—");
            format!(
                "TRADE ROUTE — click warehouses ({} stops, last={}) — L/U/B=last-stop mode Enter=commit Esc=cancel",
                draft_route_stops.len(),
                last_mode,
            )
        } else if tax_panel {
            format!(
                "TAX PANEL — Up/Down=select tier Left/Right=adjust T/Esc=close — gold:{}",
                sim.players.first().map(|p| p.gold).unwrap_or(0),
            )
        } else if diplomacy_panel {
            use anno_sim::combat::Diplomacy;
            let cur = match sim.diplomacy.get(0, diplomacy_target) {
                Diplomacy::Allied => "ALLIED",
                Diplomacy::Neutral => "NEUTRAL",
                Diplomacy::War => "WAR",
            };
            format!(
                "DIPLOMACY — vs Player {} = {} — Up/Down=select Left/Right=cycle Y/Esc=close",
                diplomacy_target, cur,
            )
        } else if demolish_mode {
            let hover_info = if let Some(bi) = demolish_hover {
                let b = &sim.buildings[bi];
                let def = &defs[b.def_id as usize];
                let name = cod.buildings[b.def_id as usize]
                    .properties
                    .get("Name")
                    .cloned()
                    .unwrap_or_else(|| format!("Bldg#{}", b.def_id));
                let refund = def.cost_gold / 2;
                format!("{} — refund: {} gold — click to demolish", name, refund)
            } else {
                "hover over a building to demolish".to_string()
            };
            format!("DEMOLISH MODE — {} — Esc=cancel", hover_info)
        } else if let Some(ref insp) = inspection {
            format!(
                "INSPECT — {} — Esc=close",
                insp.info,
            )
        } else if placer.active {
            let page_indices = placer.page_index_slice();
            let build_list: String = page_indices
                .iter()
                .enumerate()
                .map(|(i, &b_idx)| {
                    let marker = if b_idx == placer.selected { ">" } else { " " };
                    let name = &placer.buildable[b_idx].name;
                    format!("{marker}{}:{}", i + 1, name)
                })
                .collect::<Vec<_>>()
                .join(" ");
            let sel_cost = placer
                .selected_building()
                .map(|b| defs[b.def_idx].cost_gold)
                .unwrap_or(0);
            let cat_label = placer.category.label();
            let pg_total = placer.page_count().max(1);
            let rot_count = placer
                .selected_building()
                .map(|b| cod.buildings[b.def_idx].rotate.max(1))
                .unwrap_or(1);
            let rot_label = if rot_count > 1 {
                format!(" rot:{}/{}", placer.orientation + 1, rot_count)
            } else {
                String::new()
            };
            format!(
                "BUILD MODE [{cat_label}] — gold:{} cost:{}{} — pg{}/{} — {} — [/]=cat PgUp/Dn=page Z=rot Esc=cancel",
                human_gold,
                sel_cost,
                rot_label,
                placer.page + 1,
                pg_total,
                build_list,
            )
        } else if !selected_units.is_empty() {
            format!(
                "Anno 1602 — selected {} unit(s) — RMB=move-here Esc=deselect — {:02}:{:02} {} — gold:{}",
                selected_units.len(),
                minutes, seconds, speed_label, human_gold,
            )
        } else {
            format!(
                "Anno 1602 [{}] — '{}' — {:02}:{:02} {} — carriers:{} ships:{} units:{} routes:{} gold:{} — {zoom_label} {}x — B=build D=demolish T=tax Y=diplo R=route G=graphs P=prod",
                net_status,
                scenario_name,
                minutes,
                seconds,
                speed_label,
                carriers,
                sim.trade_ships.iter().filter(|s| s.active).count(),
                sim.military_units.iter().filter(|u| u.is_alive()).count(),
                sim.trade_routes.iter().filter(|r| r.active).count(),
                human_gold,
                display_zoom,
            )
        };
        // Override with save/load banner for ~3s
        let title = if let Some((ref msg, ref t)) = save_banner {
            if t.elapsed() < std::time::Duration::from_millis(3000) {
                format!("[{msg}] {title}")
            } else {
                save_banner = None;
                title
            }
        } else {
            title
        };
        canvas.window_mut().set_title(&title).ok();

        // Perf overlay (F12). Drawn last so it lands on top of everything.
        if show_perf {
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 200u32;
            let panel_h = (line_h * 4 + 8) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x00;
                buf[i * 4 + 3] = 200;
            }
            let n = perf_history.len() as u32;
            let avg = |idx: usize| -> u32 {
                if n == 0 { return 0; }
                let s: u64 = perf_history.iter()
                    .map(|s| match idx { 0 => s.0, 1 => s.1, _ => s.2 } as u64)
                    .sum();
                (s / n as u64) as u32
            };
            let lines = [
                format!("PERF (F12) n={}", n),
                format!("sim {:>5} us", avg(0)),
                format!("render {:>5} us", avg(1)),
                format!("frame {:>5} us  {} fps",
                    avg(2),
                    if avg(2) == 0 { 0 } else { 1_000_000 / avg(2) },
                ),
            ];
            for (i, line) in lines.iter().enumerate() {
                tiny_font::draw_str(&mut buf, panel_w, panel_h,
                    4, 4 + i as i32 * line_h, line,
                    [0xCC, 0xFF, 0xCC, 0xFF], scale);
            }
            if let Ok(mut tex) = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = WINDOW_W as i32 - panel_w as i32 - 8;
                let ty = WINDOW_H as i32 - panel_h as i32 - 8;
                canvas.copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h))).ok();
            }
        }

        canvas.present();

        // Sample perf for next-frame overlay.
        let frame_us = frame_started.elapsed().as_micros() as u32;
        frame_started = std::time::Instant::now();
        let sim_us = perf_sim_start.elapsed().as_micros() as u32;
        // Render us is approximate: total frame minus the sim portion.
        let render_us = frame_us.saturating_sub(sim_us);
        if perf_history.len() >= 60 { perf_history.pop_front(); }
        perf_history.push_back((sim_us, render_us, frame_us));
    }
}

/// Cached terrain render with coordinate info for overlay.
struct RenderState {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    tile_w: i32,
    tile_h: i32,
}

/// Draw simulation entities (carriers, ships, military) on top of terrain.
fn overlay_entities(
    rgba: &mut [u8],
    img_w: u32,
    img_h: u32,
    origin_x: i32,
    origin_y: i32,
    tile_w: i32,
    tile_h: i32,
    sim: &Simulation,
    world_mode: bool,
    current_island: Option<&Island>,
    carrier_sprites: &[(u32, u32, Vec<u8>)],
    ship_sprites: &[(u32, u32, Vec<u8>)],
    soldier_sprites: &[(u32, u32, Vec<u8>)],
    selected_units: &[usize],
    carrier_walk_anz: usize,
    ship_walk_anz: usize,
    show_paths: bool,
) {
    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;

    // Helper: convert tile coords to screen pixel
    let tile_to_screen = |tx: i32, ty: i32, island_x: i32, island_y: i32| -> (i32, i32) {
        let wx = island_x + tx;
        let wy = island_y + ty;
        let sx = origin_x + (wx - wy) * half_tw;
        let sy = origin_y + (wx + wy) * half_th;
        (sx, sy)
    };

    // Path overlay (F6): trace each active carrier's remaining A* path
    // and each trade ship's ocean path. Drawn first so entity sprites
    // sit on top.
    if show_paths {
        // Carrier paths (yellow dots).
        for figure in &sim.figures {
            if !figure.is_active() { continue; }
            if figure.path_idx >= figure.path.len() { continue; }
            let (ix, iy) = if world_mode {
                if (figure.building_idx as usize) < sim.buildings.len() {
                    let bld = &sim.buildings[figure.building_idx as usize];
                    island_offset_for(bld.island_id, sim, current_island)
                } else { (0, 0) }
            } else if let Some(island) = current_island {
                if (figure.building_idx as usize) < sim.buildings.len() {
                    let bld = &sim.buildings[figure.building_idx as usize];
                    if bld.island_id != island.number { continue; }
                    (0, 0)
                } else { continue; }
            } else { (0, 0) };
            for &(tx, ty) in figure.path.iter().skip(figure.path_idx) {
                let (sx, sy) = tile_to_screen(tx, ty, ix, iy);
                draw_marker(rgba, img_w, img_h,
                    sx + half_tw, sy + half_th, 1,
                    &[0xFF, 0xE0, 0x40, 0xC0]);
            }
        }
        // Ship ocean paths (cyan dots, world coords directly).
        for ship in &sim.trade_ships {
            if !ship.active { continue; }
            for &(tx, ty) in ship.path.iter().skip(ship.path_idx) {
                let (sx, sy) = tile_to_screen(tx, ty, 0, 0);
                draw_marker(rgba, img_w, img_h,
                    sx + half_tw, sy + half_th, 1,
                    &[0x40, 0xE0, 0xFF, 0xC0]);
            }
        }
    }

    // Draw carriers (sprites if available, colored dots fallback).
    // Layout (from figuren.cod TRAEGER): 8 rotations × `carrier_walk_anz`
    // frames laid out as base + dir*anz + frame. base_sprite is the per-figure
    // offset in the BSH (TRAEGER=0, ESEL=192, etc.).
    let carrier_frames_per_dir = carrier_walk_anz.max(1);
    for figure in &sim.figures {
        if !figure.is_active() {
            continue;
        }

        // Find island position for this figure's island
        let (ix, iy) = if world_mode {
            if (figure.building_idx as usize) < sim.buildings.len() {
                let bld = &sim.buildings[figure.building_idx as usize];
                island_offset_for(bld.island_id, sim, current_island)
            } else {
                (0, 0)
            }
        } else if let Some(island) = current_island {
            if (figure.building_idx as usize) < sim.buildings.len() {
                let bld = &sim.buildings[figure.building_idx as usize];
                if bld.island_id != island.number {
                    continue;
                }
            }
            (0, 0)
        } else {
            continue;
        };

        let (sx, sy) = tile_to_screen(figure.tile_x as i32, figure.tile_y as i32, ix, iy);

        // Try sprite rendering
        let dir = (figure.direction as usize) % 8;
        let frame = (figure.anim_frame as usize) % carrier_frames_per_dir;
        let sprite_idx = figure.base_sprite as usize
            + dir * carrier_frames_per_dir
            + frame;

        // Track sprite top so we can stamp a cargo indicator above it.
        let mut sprite_top = sy + half_th - 12;
        let mut drew_sprite = false;
        if sprite_idx < carrier_sprites.len() {
            let (sw, sh, ref data) = carrier_sprites[sprite_idx];
            if sw > 0 && sh > 0 {
                let dy = sy + half_th - sh as i32;
                blit_rgba(
                    rgba, img_w, img_h,
                    sx + half_tw - sw as i32 / 2,
                    dy,
                    data, sw, sh,
                );
                sprite_top = dy;
                drew_sprite = true;
            }
        }
        if !drew_sprite {
            // Fallback: colored dot
            let color = match figure.action {
                ActionType::CarryingGoods => [0xFF, 0xDD, 0x00, 0xFF],
                ActionType::Returning => [0x88, 0xAA, 0x00, 0xFF],
                _ => [0xFF, 0xFF, 0xFF, 0xFF],
            };
            draw_marker(rgba, img_w, img_h, sx, sy, 3, &color);
        }

        // Cargo indicator: small colored chip above the figure with a
        // single-letter abbreviation when carrying. Skip when idle/empty.
        if matches!(figure.action, ActionType::CarryingGoods | ActionType::Returning)
            && figure.carried_amount > 0
        {
            let good = good_from_u8(figure.carried_good);
            if let Some((color, letter)) = cargo_chip(good) {
                let cx = sx + half_tw;
                let cy = (sprite_top - 6).max(0);
                // Backing chip
                for cy_off in 0..7i32 {
                    for cx_off in -4i32..=4i32 {
                        let fx = cx + cx_off;
                        let fy = cy + cy_off;
                        if fx < 0 || fy < 0 { continue; }
                        if (fx as u32) >= img_w || (fy as u32) >= img_h { continue; }
                        let off = ((fy as u32 * img_w + fx as u32) * 4) as usize;
                        if off + 3 >= rgba.len() { continue; }
                        rgba[off] = color[0];
                        rgba[off + 1] = color[1];
                        rgba[off + 2] = color[2];
                        rgba[off + 3] = 0xFF;
                    }
                }
                // Letter on top
                let label = String::from(letter);
                tiny_font::draw_str(
                    rgba, img_w, img_h,
                    cx - 1, cy + 1, &label,
                    [0x00, 0x00, 0x00, 0xFF], 1,
                );
            }
        }
    }

    // Draw warehouses (blue squares)
    for wh in &sim.warehouses {
        let (ix, iy) = if world_mode {
            island_offset_for(wh.island_id, sim, current_island)
        } else if let Some(island) = current_island {
            if wh.island_id != island.number {
                continue;
            }
            (0, 0)
        } else {
            continue;
        };

        let (sx, sy) = tile_to_screen(wh.tile_x as i32, wh.tile_y as i32, ix, iy);
        draw_marker(rgba, img_w, img_h, sx, sy, 4, &[0x40, 0x80, 0xFF, 0xFF]);
    }

    // Draw military units (sprites if available, colored markers fallback)
    const SOLDIER_FRAMES_PER_DIR: usize = 8;
    for (uidx, unit) in sim.military_units.iter().enumerate() {
        if !unit.is_alive() {
            continue;
        }
        let (ix, iy) = if world_mode { (0, 0) } else { (0, 0) };
        let (sx, sy) = tile_to_screen(unit.tile_x as i32, unit.tile_y as i32, ix, iy);
        let is_selected = selected_units.contains(&uidx);

        // Selection ring (drawn behind sprite/marker)
        if is_selected {
            let cx = sx + half_tw;
            let cy = sy + half_th;
            let r = (half_tw + half_th) / 2 + 2;
            draw_ring(rgba, img_w, img_h, cx, cy, r, &[0xFF, 0xFF, 0x00, 0xFF]);
        }

        // Use direction to pick a sprite frame (8 dirs × frames)
        let dir = (unit.direction as usize) % 8;
        let sprite_idx = dir * SOLDIER_FRAMES_PER_DIR;
        if sprite_idx < soldier_sprites.len() {
            let (sw, sh, ref data) = soldier_sprites[sprite_idx];
            if sw > 0 && sh > 0 {
                blit_rgba(
                    rgba, img_w, img_h,
                    sx + half_tw - sw as i32 / 2,
                    sy - sh as i32 + half_th,
                    data, sw, sh,
                );
                // For selected: also draw a small target marker at destination
                if is_selected
                    && (unit.tile_x != unit.target_x || unit.tile_y != unit.target_y)
                {
                    let (tsx, tsy) = tile_to_screen(unit.target_x, unit.target_y, ix, iy);
                    draw_marker(rgba, img_w, img_h,
                        tsx + half_tw, tsy + half_th, 3,
                        &[0xFF, 0xFF, 0x00, 0xFF]);
                }
                continue;
            }
        }

        let color = if is_selected {
            [0xFF, 0xFF, 0x00, 0xFF]
        } else if unit.owner == 0 {
            [0x00, 0xFF, 0x00, 0xFF]
        } else {
            [0xFF, 0x40, 0x40, 0xFF]
        };
        let size = if unit.unit_type.stats().is_ranged { 4 } else { 3 };
        draw_marker(rgba, img_w, img_h, sx, sy, size, &color);
    }

    // Draw trade ships (sprites if available, cyan diamonds fallback).
    //
    // SHIP.BSH layout (figuren.cod HANDEL1): Rotate:1, AnimAnz:`ship_walk_anz`.
    // The full anim cycle IS the rotation set — `ship_walk_anz` evenly-spaced
    // angles around 360°. We map our 8 compass headings onto that range.
    let ship_anz = ship_walk_anz.max(1);
    for ship in &sim.trade_ships {
        if !ship.active {
            continue;
        }
        let (sx, sy) = tile_to_screen(ship.world_x, ship.world_y, 0, 0);

        let dir = (ship.heading as usize) % 8;
        let sprite_idx = dir * ship_anz / 8;
        if sprite_idx < ship_sprites.len() {
            let (sw, sh, ref data) = ship_sprites[sprite_idx];
            if sw > 0 && sh > 0 {
                blit_rgba(
                    rgba, img_w, img_h,
                    sx + half_tw - sw as i32 / 2,
                    sy - sh as i32 + half_th,
                    data, sw, sh,
                );
                continue;
            }
        }
        draw_diamond(rgba, img_w, img_h, sx, sy, 5, &[0x00, 0xFF, 0xFF, 0xFF]);
    }
}

/// Get island world offset by island_id.
fn island_offset_for(
    _island_id: u8,
    _sim: &Simulation,
    _current_island: Option<&Island>,
) -> (i32, i32) {
    // In world mode, islands have x_pos/y_pos offsets in the SZS.
    // But we don't have direct access to the SZS islands here.
    // For now, the figures store tile coords relative to the island,
    // and we pass (0,0) since the sim doesn't store island offsets yet.
    // TODO: Store island offsets in Simulation for proper world-map overlay.
    (0, 0)
}

/// Draw a filled square marker centered at (cx, cy).
fn draw_marker(
    rgba: &mut [u8],
    img_w: u32,
    img_h: u32,
    cx: i32,
    cy: i32,
    radius: i32,
    color: &[u8; 4],
) {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let px = cx + dx;
            let py = cy + dy;
            if px >= 0 && py >= 0 && (px as u32) < img_w && (py as u32) < img_h {
                let off = ((py as u32 * img_w + px as u32) * 4) as usize;
                if off + 3 < rgba.len() {
                    rgba[off] = color[0];
                    rgba[off + 1] = color[1];
                    rgba[off + 2] = color[2];
                    rgba[off + 3] = color[3];
                }
            }
        }
    }
}

/// Draw a hollow ring (1px stroke) centered at (cx, cy) with the given radius.
fn draw_ring(
    rgba: &mut [u8],
    img_w: u32,
    img_h: u32,
    cx: i32,
    cy: i32,
    radius: i32,
    color: &[u8; 4],
) {
    if radius <= 0 {
        return;
    }
    let r2_outer = (radius * radius) as i32;
    let r2_inner = ((radius - 1).max(0) * (radius - 1).max(0)) as i32;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let d2 = dx * dx + dy * dy;
            if d2 <= r2_outer && d2 >= r2_inner {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && py >= 0 && (px as u32) < img_w && (py as u32) < img_h {
                    let off = ((py as u32 * img_w + px as u32) * 4) as usize;
                    if off + 3 < rgba.len() {
                        rgba[off] = color[0];
                        rgba[off + 1] = color[1];
                        rgba[off + 2] = color[2];
                        rgba[off + 3] = color[3];
                    }
                }
            }
        }
    }
}

/// Draw a diamond marker centered at (cx, cy).
fn draw_diamond(
    rgba: &mut [u8],
    img_w: u32,
    img_h: u32,
    cx: i32,
    cy: i32,
    radius: i32,
    color: &[u8; 4],
) {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx.abs() + dy.abs() <= radius {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && py >= 0 && (px as u32) < img_w && (py as u32) < img_h {
                    let off = ((py as u32 * img_w + px as u32) * 4) as usize;
                    if off + 3 < rgba.len() {
                        rgba[off] = color[0];
                        rgba[off + 1] = color[1];
                        rgba[off + 2] = color[2];
                        rgba[off + 3] = color[3];
                    }
                }
            }
        }
    }
}

/// Initialize the simulation from scenario data (like sim_test but automated).
fn init_simulation(
    szs: &SzsFile,
    cod: &CodFile,
    defs: &[anno_sim::building::BuildingDef],
) -> Simulation {
    let mut instances = data_bridge::load_building_instances(szs, cod, defs);

    // Seed processing buildings with input materials
    for inst in &mut instances {
        let def = &defs[inst.def_id as usize];
        if def.input_good_1 != Good::None {
            inst.input_1_stock = def.storage_capacity;
        }
        if def.input_good_2 != Good::None {
            inst.input_2_stock = def.storage_capacity;
        }
    }

    // Create warehouses — one per island with production buildings
    let mut island_ids: Vec<u8> = instances.iter().map(|i| i.island_id).collect();
    island_ids.sort();
    island_ids.dedup();

    let mut warehouses = Vec::new();
    for &island_id in &island_ids {
        let island_buildings: Vec<_> = instances
            .iter()
            .filter(|b| b.island_id == island_id)
            .collect();
        if island_buildings.is_empty() {
            continue;
        }
        let avg_x = island_buildings.iter().map(|b| b.tile_x as u32).sum::<u32>()
            / island_buildings.len() as u32;
        let avg_y = island_buildings.iter().map(|b| b.tile_y as u32).sum::<u32>()
            / island_buildings.len() as u32;
        warehouses.push(Warehouse::new(island_id, 0, avg_x as u16, avg_y as u16));
    }

    // Build island walkability maps
    let island_maps: Vec<IslandMap> = szs
        .islands
        .iter()
        .map(|island| IslandMap::from_island(island, &cod.buildings))
        .collect();

    // Build coverage maps for each island
    let coverage_maps: Vec<anno_sim::coverage::CoverageMap> = szs
        .islands
        .iter()
        .map(|island| {
            anno_sim::coverage::CoverageMap::new(island.number, island.width as u16, island.height as u16)
        })
        .collect();

    // Build ocean navigability map for ship pathfinding
    let ocean_map = anno_sim::ocean_map::OceanMap::from_scenario(szs);
    println!(
        "Ocean map: {}x{} ({} navigable tiles)",
        ocean_map.width,
        ocean_map.height,
        (0..ocean_map.height as i32)
            .flat_map(|y| (0..ocean_map.width as i32).map(move |x| (x, y)))
            .filter(|&(x, y)| ocean_map.is_navigable(x, y))
            .count()
    );

    let mut sim = Simulation::new();
    sim.building_defs = defs.to_vec();
    sim.buildings = instances;
    sim.warehouses = warehouses;
    sim.island_maps = island_maps;
    sim.coverage_maps = coverage_maps;
    sim.ocean_map = Some(ocean_map);

    // Human player
    let mut player = Player::new_human(0);
    player.population[0] = 200;
    player.population[1] = 100;
    player.population[2] = 50;
    player.gold = 10000;
    sim.players.push(player);

    // AI player
    let mut ai_player = Player::new_ai(1, 0);
    ai_player.population[0] = 150;
    ai_player.population[1] = 50;
    ai_player.gold = 8000;
    sim.players.push(ai_player);
    sim.ai_controllers
        .push(AiController::new(1, AiPersonality::Economic, Difficulty::Medium));

    // Military setup
    sim.diplomacy.set(0, 1, Diplomacy::War);
    sim.military_units
        .push(MilitaryUnit::new(UnitType::Swordsman, 0, 20, 20));
    sim.military_units
        .push(MilitaryUnit::new(UnitType::Swordsman, 0, 21, 20));
    sim.military_units
        .push(MilitaryUnit::new(UnitType::Cannon, 0, 18, 20));
    sim.military_units
        .push(MilitaryUnit::new(UnitType::Pikeman, 1, 25, 20));
    sim.military_units
        .push(MilitaryUnit::new(UnitType::Pikeman, 1, 25, 21));
    sim.military_units
        .push(MilitaryUnit::new(UnitType::Musketeer, 1, 27, 20));

    // Trade route between first two islands with warehouses
    let wh_islands: Vec<(u8, u16, u16)> = sim
        .warehouses
        .iter()
        .map(|w| (w.island_id, w.tile_x, w.tile_y))
        .collect();
    if wh_islands.len() >= 2 {
        let mut route = TradeRoute::new(0, 0);
        route.add_stop(RouteStop {
            island_id: wh_islands[0].0,
            warehouse_x: wh_islands[0].1,
            warehouse_y: wh_islands[0].2,
            load_goods: vec![(Good::Spices, 10)],
            unload_goods: vec![Good::Grain],
        });
        route.add_stop(RouteStop {
            island_id: wh_islands[1].0,
            warehouse_x: wh_islands[1].1,
            warehouse_y: wh_islands[1].2,
            load_goods: vec![(Good::Grain, 10)],
            unload_goods: vec![Good::Spices],
        });
        route.activate();

        let ship = TradeShip::new(0, 0, wh_islands[0].1 as i32, wh_islands[0].2 as i32);
        sim.trade_routes.push(route);
        sim.trade_ships.push(ship);
    }

    sim
}

fn decode_sprites(
    mgr: &SpriteManager,
    zoom: usize,
    palette: &[[u8; 3]; 256],
) -> Vec<(u32, u32, Vec<u8>)> {
    match mgr.get_set(SpriteCategory::Stadtfld, zoom) {
        Some(set) => set
            .bsh
            .sprites
            .iter()
            .map(|s| (s.width, s.height, s.decode(palette)))
            .collect(),
        None => Vec::new(),
    }
}

fn decode_entity_sprites(
    mgr: &SpriteManager,
    category: SpriteCategory,
    zoom: usize,
    palette: &[[u8; 3]; 256],
) -> Vec<(u32, u32, Vec<u8>)> {
    match mgr.get_set(category, zoom) {
        Some(set) => set
            .bsh
            .sprites
            .iter()
            .map(|s| (s.width, s.height, s.decode(palette)))
            .collect(),
        None => Vec::new(),
    }
}

/// Render all islands; returns (rgba, width, height, origin_x, origin_y).
fn render_world(
    islands: &[Island],
    sprites: &[(u32, u32, Vec<u8>)],
    num_sprites: usize,
    tile_w: i32,
    tile_h: i32,
    anim: &AnimationState,
) -> (Vec<u8>, u32, u32, i32, i32) {
    let max_world_x = islands
        .iter()
        .map(|i| i.x_pos as i32 + i.width as i32)
        .max()
        .unwrap_or(100);
    let max_world_y = islands
        .iter()
        .map(|i| i.y_pos as i32 + i.height as i32)
        .max()
        .unwrap_or(100);

    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;

    let img_w = ((max_world_x + max_world_y) * half_tw + tile_w) as u32;
    let img_h = ((max_world_x + max_world_y) * half_th + tile_h + 500) as u32;

    // Cap at reasonable size
    let scale = if img_w > 8192 || img_h > 8192 {
        8192.0 / img_w.max(img_h) as f64
    } else {
        1.0
    };

    let final_w = (img_w as f64 * scale) as u32;
    let final_h = (img_h as f64 * scale) as u32;

    let origin_x;
    let origin_y;

    if scale < 1.0 {
        let s_half_tw = (half_tw as f64 * scale) as i32;
        let s_half_th = (half_th as f64 * scale) as i32;

        let mut rgba = vec![0u8; (final_w * final_h * 4) as usize];

        origin_x = (max_world_y as f64 * s_half_tw as f64) as i32;
        origin_y = (100.0 * scale) as i32;

        for island in islands {
            if island.tiles.is_empty() {
                continue;
            }
            for tile in &island.tiles {
                let wx = island.x_pos as i32 + tile.x as i32;
                let wy = island.y_pos as i32 + tile.y as i32;
                let sx = origin_x + (wx - wy) * s_half_tw;
                let sy = origin_y + (wx + wy) * s_half_th;

                let sprite_idx = anim.animate(tile.building_id) as usize;
                if sprite_idx < num_sprites {
                    let (sw, sh, ref sdata) = sprites[sprite_idx];
                    if sw > 0 && sh > 0 {
                        let cx = sw / 2;
                        let cy = sh / 2;
                        let off = ((cy * sw + cx) * 4) as usize;
                        if off + 3 < sdata.len() && sdata[off + 3] > 0 {
                            let r = sdata[off];
                            let g = sdata[off + 1];
                            let b = sdata[off + 2];
                            for dy in 0..s_half_th.max(1) {
                                for dx in 0..s_half_tw.max(1) {
                                    let px = sx + dx;
                                    let py = sy + dy;
                                    if px >= 0
                                        && py >= 0
                                        && (px as u32) < final_w
                                        && (py as u32) < final_h
                                    {
                                        let doff =
                                            ((py as u32 * final_w + px as u32) * 4) as usize;
                                        if doff + 3 < rgba.len() {
                                            rgba[doff] = r;
                                            rgba[doff + 1] = g;
                                            rgba[doff + 2] = b;
                                            rgba[doff + 3] = 255;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        return (rgba, final_w, final_h, origin_x, origin_y);
    }

    // Full resolution
    let mut rgba = vec![0u8; (img_w * img_h * 4) as usize];
    origin_x = max_world_y * half_tw;
    origin_y = 300;

    let mut world_tiles: Vec<(i32, i32, u16)> = Vec::new();
    for island in islands {
        for tile in &island.tiles {
            let wx = island.x_pos as i32 + tile.x as i32;
            let wy = island.y_pos as i32 + tile.y as i32;
            world_tiles.push((wx, wy, tile.building_id));
        }
    }
    world_tiles.sort_by_key(|&(x, y, _)| (x + y, y));

    for &(wx, wy, building_id) in &world_tiles {
        let sx = origin_x + (wx - wy) * half_tw;
        let sy = origin_y + (wx + wy) * half_th;

        let sprite_idx = anim.animate(building_id) as usize;
        if sprite_idx >= num_sprites {
            continue;
        }

        let (sw, sh, ref sprite_data) = sprites[sprite_idx];
        if sw == 0 || sh == 0 {
            continue;
        }

        blit_rgba(&mut rgba, img_w, img_h, sx, sy - (sh as i32 - tile_h), sprite_data, sw, sh);
    }

    (rgba, img_w, img_h, origin_x, origin_y)
}

/// Render a single island; returns (rgba, width, height, origin_x, origin_y).
fn render_island(
    island: &Island,
    sprites: &[(u32, u32, Vec<u8>)],
    num_sprites: usize,
    tile_w: i32,
    tile_h: i32,
    anim: &AnimationState,
) -> (Vec<u8>, u32, u32, i32, i32) {
    let iw = island.width as i32;
    let ih = island.height as i32;

    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;

    let img_w = ((iw + ih) * half_tw) as u32 + tile_w as u32;
    let img_h = ((iw + ih) * half_th) as u32 + tile_h as u32 + 500;

    let mut rgba = vec![0u8; (img_w * img_h * 4) as usize];

    let origin_x = ih * half_tw;
    let origin_y = 300;

    let mut sorted_tiles: Vec<_> = island.tiles.iter().collect();
    sorted_tiles.sort_by_key(|t| (t.y as i32 + t.x as i32, t.y as i32));

    for tile in &sorted_tiles {
        let tx = tile.x as i32;
        let ty = tile.y as i32;

        let sx = origin_x + (tx - ty) * half_tw;
        let sy = origin_y + (tx + ty) * half_th;

        let sprite_idx = anim.animate(tile.building_id) as usize;
        if sprite_idx >= num_sprites {
            continue;
        }

        let (sw, sh, ref sprite_data) = sprites[sprite_idx];
        if sw == 0 || sh == 0 {
            continue;
        }

        blit_rgba(&mut rgba, img_w, img_h, sx, sy - (sh as i32 - tile_h), sprite_data, sw, sh);
    }

    (rgba, img_w, img_h, origin_x, origin_y)
}

/// Map a `figure.carried_good` u8 back to its `Good` enum.
/// Mirrors `anno_sim::carrier::good_from_u8` (which is private to its module).
fn good_from_u8(val: u8) -> Good {
    match val {
        1  => Good::Wood, 2  => Good::Iron, 3  => Good::Gold,
        4  => Good::Wool, 5  => Good::Sugar, 6  => Good::Tobacco,
        7  => Good::Cattle, 8  => Good::Grain, 9  => Good::Flour,
        10 => Good::Tools, 11 => Good::Bricks, 12 => Good::Swords,
        13 => Good::Muskets, 14 => Good::Cannons, 15 => Good::Food,
        16 => Good::Cloth, 17 => Good::Alcohol, 18 => Good::TobaccoProducts,
        19 => Good::Spices, 20 => Good::Cocoa, 21 => Good::Grapes,
        22 => Good::Stone, 23 => Good::Ore, 24 => Good::GoldOre,
        25 => Good::Hides, 26 => Good::Cotton, 27 => Good::Silk,
        28 => Good::Jewelry, 29 => Good::Clothing, 30 => Good::Fish,
        _ => Good::None,
    }
}

/// Pick a chip color + single-letter abbreviation for the cargo indicator
/// drawn above carriers when they're hauling goods.
fn cargo_chip(good: Good) -> Option<([u8; 4], char)> {
    let (color, ch) = match good {
        Good::None => return None,
        Good::Wood   => ([0x8B, 0x4F, 0x20, 0xFF], 'W'),
        Good::Iron   => ([0x80, 0x80, 0x90, 0xFF], 'I'),
        Good::Ore    => ([0x70, 0x60, 0x50, 0xFF], 'O'),
        Good::Stone  => ([0xA0, 0xA0, 0xA0, 0xFF], 'S'),
        Good::Gold   => ([0xFF, 0xD7, 0x00, 0xFF], 'G'),
        Good::GoldOre => ([0xC0, 0x9A, 0x40, 0xFF], 'g'),
        Good::Tools  => ([0xC0, 0xC0, 0xD0, 0xFF], 'T'),
        Good::Bricks => ([0xB0, 0x60, 0x40, 0xFF], 'B'),
        Good::Wool   => ([0xE0, 0xE0, 0xC0, 0xFF], 'w'),
        Good::Cotton => ([0xF0, 0xF0, 0xE0, 0xFF], 'C'),
        Good::Hides  => ([0x80, 0x40, 0x20, 0xFF], 'h'),
        Good::Cattle => ([0xA0, 0x60, 0x30, 0xFF], 'c'),
        Good::Grain  => ([0xE0, 0xC0, 0x40, 0xFF], 'r'),
        Good::Flour  => ([0xF8, 0xE8, 0xC0, 0xFF], 'f'),
        Good::Food   => ([0xE0, 0x40, 0x20, 0xFF], 'F'),
        Good::Sugar  => ([0xF0, 0xF0, 0xF8, 0xFF], 's'),
        Good::Tobacco => ([0x80, 0x60, 0x20, 0xFF], 'b'),
        Good::TobaccoProducts => ([0x60, 0x40, 0x20, 0xFF], 'P'),
        Good::Cocoa  => ([0x60, 0x40, 0x20, 0xFF], 'k'),
        Good::Spices => ([0xC0, 0x60, 0x20, 0xFF], 'x'),
        Good::Cloth  => ([0xA0, 0xA0, 0xE0, 0xFF], 'L'),
        Good::Clothing => ([0x80, 0x80, 0xC0, 0xFF], 'l'),
        Good::Alcohol => ([0xA0, 0x40, 0x80, 0xFF], 'A'),
        Good::Grapes => ([0x80, 0x40, 0xA0, 0xFF], 'p'),
        Good::Fish   => ([0x40, 0xA0, 0xC0, 0xFF], 'i'),
        Good::Silk   => ([0xE0, 0x80, 0xC0, 0xFF], 'k'),
        Good::Jewelry => ([0xFF, 0xC0, 0x40, 0xFF], 'J'),
        Good::Swords  => ([0xC0, 0xC0, 0xC0, 0xFF], 'V'),
        Good::Muskets => ([0x80, 0x80, 0x80, 0xFF], 'M'),
        Good::Cannons => ([0x40, 0x40, 0x40, 0xFF], 'N'),
    };
    Some((color, ch))
}

fn blit_rgba(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    x: i32,
    y: i32,
    src: &[u8],
    src_w: u32,
    src_h: u32,
) {
    for row in 0..src_h as i32 {
        let dy = y + row;
        if dy < 0 || dy >= dst_h as i32 {
            continue;
        }
        for col in 0..src_w as i32 {
            let dx = x + col;
            if dx < 0 || dx >= dst_w as i32 {
                continue;
            }
            let src_off = ((row as u32 * src_w + col as u32) * 4) as usize;
            if src_off + 3 >= src.len() {
                continue;
            }
            if src[src_off + 3] == 0 {
                continue;
            }
            let dst_off = ((dy as u32 * dst_w + dx as u32) * 4) as usize;
            if dst_off + 3 >= dst.len() {
                continue;
            }
            dst[dst_off] = src[src_off];
            dst[dst_off + 1] = src[src_off + 1];
            dst[dst_off + 2] = src[src_off + 2];
            dst[dst_off + 3] = 255;
        }
    }
}

fn save_ppm(rgba: &[u8], width: u32, height: u32, name: &str) {
    let filename = format!("{name}_game_screenshot.ppm");
    let mut ppm = Vec::with_capacity((width * height * 3 + 100) as usize);
    ppm.extend_from_slice(format!("P6\n{width} {height}\n255\n").as_bytes());
    for y in 0..height {
        for x in 0..width {
            let off = ((y * width + x) * 4) as usize;
            if off + 2 < rgba.len() {
                ppm.push(rgba[off]);
                ppm.push(rgba[off + 1]);
                ppm.push(rgba[off + 2]);
            } else {
                ppm.extend_from_slice(&[0, 0, 0]);
            }
        }
    }
    std::fs::write(&filename, &ppm).expect("Failed to write screenshot");
    println!("Screenshot saved to {filename}");
}

fn find_data_dir() -> std::path::PathBuf {
    for candidate in &["extracted", "../extracted", "../../extracted"] {
        let p = std::path::Path::new(candidate);
        if p.join("GFX/STADTFLD.BSH").exists() || p.join("haeuser.cod").exists() {
            return p.to_path_buf();
        }
    }
    eprintln!("Could not find game data directory.");
    std::process::exit(1);
}
