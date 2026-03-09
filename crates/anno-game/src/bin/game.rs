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
//!   B: toggle build mode (then 1-9 to select building, click to place)
//!   M: toggle music on/off
//!   N: next music track
//!   V: cycle music volume
//!   S: save screenshot
//!   Escape: quit (or cancel build mode)

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

/// A building type available for placement.
struct BuildableBuilding {
    /// Index into building_defs / cod.buildings.
    def_idx: usize,
    /// Display name for the UI.
    name: String,
    /// Sprite index (gfx from COD) for rendering.
    sprite_idx: usize,
}

/// Building placement state machine.
struct BuildingPlacer {
    active: bool,
    /// Available buildings to place.
    buildable: Vec<BuildableBuilding>,
    /// Currently selected building index (into buildable vec).
    selected: usize,
    /// Current page of buildings (9 per page).
    page: usize,
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
            });
        }

        Self {
            active: false,
            buildable,
            selected: 0,
            page: 0,
            hover_tile: None,
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

    /// Get the buildings visible on the current page (up to 9).
    fn page_items(&self) -> &[BuildableBuilding] {
        let start = self.page * 9;
        let end = (start + 9).min(self.buildable.len());
        if start >= self.buildable.len() {
            &[]
        } else {
            &self.buildable[start..end]
        }
    }

    fn select_on_page(&mut self, slot: usize) {
        let idx = self.page * 9 + slot;
        if idx < self.buildable.len() {
            self.selected = idx;
        }
    }

    fn next_page(&mut self) {
        let max_page = self.buildable.len().saturating_sub(1) / 9;
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

    // Load building definitions
    let cod_data =
        std::fs::read(base_dir.join("haeuser.cod")).expect("Failed to read haeuser.cod");
    let cod = CodFile::parse(&cod_data).expect("Failed to parse COD");
    let defs = data_bridge::load_building_defs(&cod);
    println!("Loaded {} building definitions", defs.len());

    // Load scenario
    let scenario_path = std::env::args().nth(1).unwrap_or_else(|| {
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

    // Load building placement sound effect
    let place_sound_slot = audio.waves.load("SPEECH8/1000.WAV")
        .or_else(|| audio.waves.load("1000.WAV"));

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

    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut minimap_clicked = false;
    let mut minimap_click_x: i32 = 0;
    let mut minimap_click_y: i32 = 0;

    'main: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'main,

                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => {
                    if placer.active {
                        placer.active = false;
                    } else {
                        break 'main;
                    }
                }

                Event::KeyDown {
                    keycode: Some(key), ..
                } => {
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
                            Keycode::PageUp | Keycode::LeftBracket => placer.prev_page(),
                            Keycode::PageDown | Keycode::RightBracket => placer.next_page(),
                            Keycode::B => placer.toggle(),
                            // Still allow scrolling in build mode
                            Keycode::Left => scroll_x += 48,
                            Keycode::Right => scroll_x -= 48,
                            Keycode::Up => scroll_y += 48,
                            Keycode::Down => scroll_y -= 48,
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
                            Keycode::B => {
                                if !world_mode {
                                    placer.toggle();
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

                            if let Some(map_idx) = island_map_idx {
                                if can_place_building(
                                    &islands[current_island], &sim.island_maps[map_idx],
                                    tile_x, tile_y, bld_w, bld_h,
                                ) {
                                    // Deduct construction costs
                                    if sim.players[0].gold >= cost as i32 {
                                        sim.players[0].gold -= cost as i32;

                                        // Add tile records to the island for rendering
                                        for dy in 0..bld_h as u8 {
                                            for dx in 0..bld_w as u8 {
                                                let tx = tile_x as u8 + dx;
                                                let ty = tile_y as u8 + dy;
                                                let tile_sprite = sprite_idx
                                                    + dy as usize * bld_w as usize
                                                    + dx as usize;
                                                islands[current_island].tiles.push(IslandTile {
                                                    x: tx,
                                                    y: ty,
                                                    building_id: tile_sprite as u16,
                                                    orientation: 0,
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
                    } else {
                        dragging = true;
                        drag_start = (x - scroll_x, y - scroll_y);
                    }
                }

                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    dragging = false;
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

        // Update hover tile for build mode cursor
        if placer.active && !world_mode {
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
                placer.hover_tile = Some((tx, ty));
            }
        } else {
            placer.hover_tile = None;
        }

        // Simulation tick
        let now = timer.ticks();
        let dt_ms = now.wrapping_sub(last_tick);
        last_tick = now;
        if dt_ms > 0 && dt_ms < 1000 {
            sim.tick(dt_ms);
            anim_state.tick(dt_ms);
            // Check if animation frames changed (triggers terrain re-render)
            // Use a coarse generation: changes every ~100ms
            let anim_gen = anim_state.elapsed_ms / 100;
            if anim_gen != last_anim_gen {
                last_anim_gen = anim_gen;
                needs_redraw = true;
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
                );

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
                            // Draw footprint outline for each tile
                            for dy in 0..def.height as i32 {
                                for dx in 0..def.width as i32 {
                                    let tx = hover_tx + dx;
                                    let ty = hover_ty + dy;
                                    let sx = rs.origin_x + (tx - ty) * half_tw;
                                    let sy = rs.origin_y + (tx + ty) * half_th;
                                    // Draw a filled rectangle at tile position
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
                                                    // Alpha blend
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

        let title = if placer.active {
            let page_items = placer.page_items();
            let build_list: String = page_items
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let marker = if placer.page * 9 + i == placer.selected {
                        ">"
                    } else {
                        " "
                    };
                    format!("{marker}{}:{}", i + 1, b.name)
                })
                .collect::<Vec<_>>()
                .join(" ");
            let sel_cost = placer
                .selected_building()
                .map(|b| defs[b.def_idx].cost_gold)
                .unwrap_or(0);
            format!(
                "BUILD MODE — gold:{} cost:{} — pg{}/{} — {} — [/]=page Esc=cancel click=place",
                human_gold,
                sel_cost,
                placer.page + 1,
                (placer.buildable.len() + 8) / 9,
                build_list,
            )
        } else {
            format!(
                "Anno 1602 — '{}' — {:02}:{:02} {} — carriers:{} ships:{} units:{} gold:{} — {zoom_label} {}x — M=music N=next V=vol B=build",
                scenario_name,
                minutes,
                seconds,
                speed_label,
                carriers,
                sim.trade_ships.iter().filter(|s| s.active).count(),
                sim.military_units.iter().filter(|u| u.is_alive()).count(),
                human_gold,
                display_zoom,
            )
        };
        canvas.window_mut().set_title(&title).ok();

        canvas.present();
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

    // Draw carriers (yellow dots)
    for figure in &sim.figures {
        if !figure.is_active() {
            continue;
        }

        // Find island position for this figure's island
        let (ix, iy) = if world_mode {
            // In world mode, figures use absolute tile coords;
            // we need the island offset. Use island_id from the building.
            if (figure.building_idx as usize) < sim.buildings.len() {
                let bld = &sim.buildings[figure.building_idx as usize];
                island_offset_for(bld.island_id, sim, current_island)
            } else {
                (0, 0)
            }
        } else if let Some(island) = current_island {
            // Single island mode: only show figures on this island
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
        let color = match figure.action {
            ActionType::CarryingGoods => [0xFF, 0xDD, 0x00, 0xFF], // Yellow: carrying
            ActionType::Returning => [0x88, 0xAA, 0x00, 0xFF],     // Olive: returning
            _ => [0xFF, 0xFF, 0xFF, 0xFF],                         // White: other
        };
        draw_marker(rgba, img_w, img_h, sx, sy, 3, &color);
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

    // Draw military units
    for unit in &sim.military_units {
        if !unit.is_alive() {
            continue;
        }
        let color = if unit.owner == 0 {
            [0x00, 0xFF, 0x00, 0xFF] // Green: human
        } else {
            [0xFF, 0x40, 0x40, 0xFF] // Red: AI
        };
        // Military units use absolute tile coords
        let (ix, iy) = if world_mode { (0, 0) } else { (0, 0) };
        let (sx, sy) = tile_to_screen(unit.tile_x as i32, unit.tile_y as i32, ix, iy);
        let size = if unit.unit_type.stats().is_ranged { 4 } else { 3 };
        draw_marker(rgba, img_w, img_h, sx, sy, size, &color);
    }

    // Draw trade ships (cyan diamonds)
    for ship in &sim.trade_ships {
        if !ship.active {
            continue;
        }
        // Ships use world coordinates directly
        let (sx, sy) = tile_to_screen(ship.world_x, ship.world_y, 0, 0);
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
