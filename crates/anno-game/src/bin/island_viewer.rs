//! Island viewer — loads a scenario file and renders islands using STADTFLD.BSH sprites.
//!
//! Controls:
//!   Arrow keys / mouse drag: scroll the map
//!   +/-: zoom in/out
//!   Tab: cycle through islands (single island mode)
//!   W: toggle world map (all islands) vs single island
//!   S: save current view as PNG screenshot
//!   Escape: quit

use anno_formats::col::parse_col;
use anno_formats::szs::{Island, SzsFile};
use anno_render::sprite::{SpriteCategory, SpriteManager};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;

const WINDOW_W: u32 = 1280;
const WINDOW_H: u32 = 800;
const BG_COLOR: (u8, u8, u8) = (0x10, 0x20, 0x40);


/// Tile dimensions per zoom level
const ZOOM_TILE_W: [i32; 3] = [64, 32, 16];
const ZOOM_TILE_H: [i32; 3] = [31, 15, 7];

/// Pre-decode all sprites in a sprite set to RGBA.
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

fn main() {
    let base_dir = find_data_dir();

    // Load palette
    let col_data = std::fs::read(base_dir.join("TOOLGFX/STADTFLD.COL"))
        .expect("Failed to read STADTFLD.COL");
    let palette = parse_col(&col_data).expect("Failed to parse palette");

    // Load all sprite sets across zoom levels
    println!("Loading sprites...");
    let sprite_mgr = SpriteManager::load_from_dir(&base_dir);

    // Pre-decode STADTFLD sprites for each zoom level
    let sprites_by_zoom: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_sprites(&sprite_mgr, z, &palette))
        .collect();

    for (z, sprites) in sprites_by_zoom.iter().enumerate() {
        let label = ["GFX", "MGFX", "SGFX"][z];
        println!("  {label}: {} decoded sprites", sprites.len());
    }

    // Load scenario
    let scenario_path = std::env::args().nth(1).unwrap_or_else(|| {
        let szenes = base_dir.join("Szenes");
        let mut entries: Vec<_> = std::fs::read_dir(&szenes)
            .expect("Failed to read Szenes/")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .ends_with(".szs")
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());
        if let Some(entry) = entries.first() {
            return entry.path().to_string_lossy().into_owned();
        }
        eprintln!("No .szs files found");
        std::process::exit(1);
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

    for island in &szs.islands {
        if !island.tiles.is_empty() {
            println!(
                "  Island {} at ({},{}) {}x{} tiles={}",
                island.number, island.x_pos, island.y_pos, island.width, island.height,
                island.tiles.len()
            );
        }
    }

    // SDL2 setup
    let sdl = sdl2::init().expect("SDL2 init failed");
    let video = sdl.video().expect("SDL2 video init failed");

    let window = video
        .window("Anno 1602 — Island Viewer", WINDOW_W, WINDOW_H)
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
    let mut display_zoom: i32 = 1; // display scaling (1x-8x)
    let mut sprite_zoom: usize = 0; // sprite zoom level (0=GFX, 1=MGFX, 2=SGFX)
    let mut needs_redraw = true;
    let mut world_mode = false;
    let mut dragging = false;
    let mut drag_start = (0i32, 0i32);

    let mut rendered: Option<(Vec<u8>, u32, u32)> = None;

    'main: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => break 'main,

                Event::KeyDown {
                    keycode: Some(key), ..
                } => {
                    let scroll_speed = 48;
                    match key {
                        Keycode::Left => scroll_x += scroll_speed,
                        Keycode::Right => scroll_x -= scroll_speed,
                        Keycode::Up => scroll_y += scroll_speed,
                        Keycode::Down => scroll_y -= scroll_speed,
                        Keycode::Tab => {
                            if !world_mode && !szs.islands.is_empty() {
                                // Skip to next island that has tiles
                                let start = current_island;
                                loop {
                                    current_island = (current_island + 1) % szs.islands.len();
                                    if !szs.islands[current_island].tiles.is_empty()
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
                        Keycode::S => {
                            if let Some((ref data, w, h)) = rendered {
                                save_png(data, w, h, &scenario_name);
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

                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => {
                    dragging = true;
                    drag_start = (x - scroll_x, y - scroll_y);
                }

                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    dragging = false;
                }

                Event::MouseMotion { x, y, .. } if dragging => {
                    scroll_x = x - drag_start.0;
                    scroll_y = y - drag_start.1;
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

        if needs_redraw && !szs.islands.is_empty() {
            let sprites = &sprites_by_zoom[sprite_zoom];
            let num_sprites = sprites.len();
            let tile_w = ZOOM_TILE_W[sprite_zoom];
            let tile_h = ZOOM_TILE_H[sprite_zoom];
            if world_mode {
                rendered = Some(render_world(
                    &szs.islands, sprites, num_sprites, tile_w, tile_h,
                ));
            } else {
                let island = &szs.islands[current_island];
                rendered = Some(render_island(
                    island, sprites, num_sprites, tile_w, tile_h,
                ));
            }
            needs_redraw = false;
        }

        // Draw
        canvas.set_draw_color(sdl2::pixels::Color::RGB(BG_COLOR.0, BG_COLOR.1, BG_COLOR.2));
        canvas.clear();

        if let Some((ref rgba_data, tex_w, tex_h)) = rendered {
            if tex_w > 0 && tex_h > 0 {
                let mut texture = texture_creator
                    .create_texture_streaming(PixelFormatEnum::RGBA32, tex_w, tex_h)
                    .expect("texture creation failed");

                texture
                    .update(None, rgba_data, (tex_w * 4) as usize)
                    .expect("texture update failed");

                let dst_w = (tex_w as i32 * display_zoom) as u32;
                let dst_h = (tex_h as i32 * display_zoom) as u32;
                let dst_x = (WINDOW_W as i32 - dst_w as i32) / 2 + scroll_x;
                let dst_y = (WINDOW_H as i32 - dst_h as i32) / 2 + scroll_y;

                canvas
                    .copy(
                        &texture,
                        None,
                        Some(Rect::new(dst_x, dst_y, dst_w, dst_h)),
                    )
                    .ok();
            }
        }

        // Update title
        let zoom_label = ["GFX", "MGFX", "SGFX"][sprite_zoom];
        let title = if world_mode {
            format!(
                "Anno 1602 — '{}' World Map — {} islands — {zoom_label} {}x — W=toggle, 1/2/3=sprites, S=screenshot",
                scenario_name,
                szs.islands.iter().filter(|i| !i.tiles.is_empty()).count(),
                display_zoom,
            )
        } else if !szs.islands.is_empty() {
            let island = &szs.islands[current_island];
            format!(
                "Anno 1602 — Island {} ({},{}) {}x{} — {}/{} — {zoom_label} {}x — Tab/W/1-3/S",
                island.number, island.x_pos, island.y_pos, island.width, island.height,
                current_island + 1, szs.islands.len(), display_zoom,
            )
        } else {
            "Anno 1602 — No islands".to_string()
        };
        canvas.window_mut().set_title(&title).ok();

        canvas.present();
    }
}

/// Render all islands on a world map using isometric projection.
fn render_world(
    islands: &[Island],
    sprites: &[(u32, u32, Vec<u8>)],
    num_sprites: usize,
    tile_w: i32,
    tile_h: i32,
) -> (Vec<u8>, u32, u32) {
    // Find world bounds from island positions
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

    // Cap at reasonable size to avoid OOM
    let scale = if img_w > 8192 || img_h > 8192 {
        let s = 8192.0 / img_w.max(img_h) as f64;
        println!(
            "World map scaled to {:.0}% ({img_w}x{img_h} -> {}x{})",
            s * 100.0,
            (img_w as f64 * s) as u32,
            (img_h as f64 * s) as u32
        );
        s
    } else {
        1.0
    };

    let final_w = (img_w as f64 * scale) as u32;
    let final_h = (img_h as f64 * scale) as u32;

    if scale < 1.0 {
        // For scaled world maps, render at reduced resolution
        // Just render each island at its world position with reduced tile size
        let s_half_tw = (half_tw as f64 * scale) as i32;
        let s_half_th = (half_th as f64 * scale) as i32;

        let mut rgba = vec![0u8; (final_w * final_h * 4) as usize];

        let origin_x = (max_world_y as f64 * s_half_tw as f64) as i32;
        let origin_y = (100.0 * scale) as i32;

        for island in islands {
            if island.tiles.is_empty() {
                continue;
            }

            for tile in &island.tiles {
                let wx = island.x_pos as i32 + tile.x as i32;
                let wy = island.y_pos as i32 + tile.y as i32;

                let sx = origin_x + (wx - wy) * s_half_tw;
                let sy = origin_y + (wx + wy) * s_half_th;

                // Draw a small colored diamond for each tile
                let sprite_idx = tile.building_id as usize;
                if sprite_idx < num_sprites {
                    let (_, _, ref sdata) = sprites[sprite_idx];
                    // Sample center pixel color
                    let sw = sprites[sprite_idx].0;
                    let sh = sprites[sprite_idx].1;
                    if sw > 0 && sh > 0 {
                        let cx = sw / 2;
                        let cy = sh / 2;
                        let off = ((cy * sw + cx) * 4) as usize;
                        if off + 3 < sdata.len() && sdata[off + 3] > 0 {
                            let r = sdata[off];
                            let g = sdata[off + 1];
                            let b = sdata[off + 2];
                            // Draw a 2x1 pixel
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

        return (rgba, final_w, final_h);
    }

    // Full resolution world render
    let mut rgba = vec![0u8; (img_w * img_h * 4) as usize];
    let origin_x = max_world_y * half_tw;
    let origin_y = 300;

    // Collect all tiles with world coordinates, sorted for draw order
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

        let sprite_idx = building_id as usize;
        if sprite_idx >= num_sprites {
            continue;
        }

        let (sw, sh, ref sprite_data) = sprites[sprite_idx];
        if sw == 0 || sh == 0 {
            continue;
        }

        blit_rgba(&mut rgba, img_w, img_h, sx, sy - (sh as i32 - tile_h), sprite_data, sw, sh);
    }

    (rgba, img_w, img_h)
}

/// Render a single island's tiles to an RGBA buffer using isometric projection.
fn render_island(
    island: &Island,
    sprites: &[(u32, u32, Vec<u8>)],
    num_sprites: usize,
    tile_w: i32,
    tile_h: i32,
) -> (Vec<u8>, u32, u32) {
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

        let sprite_idx = tile.building_id as usize;
        if sprite_idx >= num_sprites {
            continue;
        }

        let (sw, sh, ref sprite_data) = sprites[sprite_idx];
        if sw == 0 || sh == 0 {
            continue;
        }

        blit_rgba(&mut rgba, img_w, img_h, sx, sy - (sh as i32 - tile_h), sprite_data, sw, sh);
    }

    (rgba, img_w, img_h)
}

/// Blit an RGBA sprite onto an RGBA buffer.
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

/// Save RGBA buffer as PNG file.
fn save_png(rgba: &[u8], width: u32, height: u32, name: &str) {
    let filename = format!("{name}_screenshot.ppm");
    // Write as PPM (no extra dependency needed)
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
        if p.join("GFX/STADTFLD.BSH").exists() {
            return p.to_path_buf();
        }
    }
    eprintln!("Could not find game data directory.");
    std::process::exit(1);
}
