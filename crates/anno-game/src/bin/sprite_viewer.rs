//! Sprite viewer — loads STADTFLD.BSH with the game palette and displays sprites.
//!
//! Controls:
//!   Arrow keys or scroll wheel: browse sprites
//!   Page Up/Down: skip 100 sprites
//!   Home/End: jump to first/last sprite
//!   G: enter sprite index (type number, press Enter)
//!   +/-: zoom in/out
//!   Escape: quit

use anno_formats::bsh::BshFile;
use anno_formats::col::parse_col;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use std::path::Path;

const WINDOW_W: u32 = 1024;
const WINDOW_H: u32 = 768;
const BG_COLOR: (u8, u8, u8) = (0x30, 0x30, 0x30);

fn main() {
    let base_dir = find_data_dir();

    // Load palette
    let col_path = base_dir.join("TOOLGFX/STADTFLD.COL");
    let col_data = std::fs::read(&col_path).unwrap_or_else(|e| {
        eprintln!("Failed to read {}: {e}", col_path.display());
        std::process::exit(1);
    });
    let palette = parse_col(&col_data).unwrap_or_else(|e| {
        eprintln!("Failed to parse palette: {e}");
        std::process::exit(1);
    });

    // Load sprites
    let bsh_path = base_dir.join("GFX/STADTFLD.BSH");
    let bsh_data = std::fs::read(&bsh_path).unwrap_or_else(|e| {
        eprintln!("Failed to read {}: {e}", bsh_path.display());
        std::process::exit(1);
    });
    let bsh = BshFile::parse(&bsh_data).unwrap_or_else(|e| {
        eprintln!("Failed to parse BSH: {e}");
        std::process::exit(1);
    });

    println!("Loaded {} sprites from STADTFLD.BSH", bsh.len());

    // SDL2 setup
    let sdl = sdl2::init().expect("SDL2 init failed");
    let video = sdl.video().expect("SDL2 video init failed");

    let window = video
        .window("Anno 1602 — Sprite Viewer", WINDOW_W, WINDOW_H)
        .position_centered()
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
    let mut current_sprite: usize = 0;
    let mut zoom: u32 = 2;

    'main: loop {
        // Handle events
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => break 'main,

                Event::KeyDown {
                    keycode: Some(key), ..
                } => match key {
                    Keycode::Right | Keycode::Down => {
                        if current_sprite + 1 < bsh.len() {
                            current_sprite += 1;
                        }
                    }
                    Keycode::Left | Keycode::Up => {
                        current_sprite = current_sprite.saturating_sub(1);
                    }
                    Keycode::PageDown => {
                        current_sprite = (current_sprite + 100).min(bsh.len() - 1);
                    }
                    Keycode::PageUp => {
                        current_sprite = current_sprite.saturating_sub(100);
                    }
                    Keycode::Home => {
                        current_sprite = 0;
                    }
                    Keycode::End => {
                        current_sprite = bsh.len() - 1;
                    }
                    Keycode::Equals | Keycode::Plus | Keycode::KpPlus => {
                        zoom = (zoom + 1).min(8);
                    }
                    Keycode::Minus | Keycode::KpMinus => {
                        zoom = (zoom - 1).max(1);
                    }
                    _ => {}
                },

                Event::MouseWheel { y, .. } => {
                    if y > 0 && current_sprite + 1 < bsh.len() {
                        current_sprite += 1;
                    } else if y < 0 {
                        current_sprite = current_sprite.saturating_sub(1);
                    }
                }

                _ => {}
            }
        }

        // Render
        canvas.set_draw_color(sdl2::pixels::Color::RGB(BG_COLOR.0, BG_COLOR.1, BG_COLOR.2));
        canvas.clear();

        let sprite = &bsh.sprites[current_sprite];
        let rgba = sprite.decode(&palette);

        if sprite.width > 0 && sprite.height > 0 {
            let mut texture = texture_creator
                .create_texture_streaming(
                    PixelFormatEnum::RGBA32,
                    sprite.width,
                    sprite.height,
                )
                .expect("texture creation failed");

            texture
                .update(
                    None,
                    &rgba,
                    (sprite.width * 4) as usize,
                )
                .expect("texture update failed");

            let dst_w = sprite.width * zoom;
            let dst_h = sprite.height * zoom;
            let dst_x = (WINDOW_W.saturating_sub(dst_w)) as i32 / 2;
            let dst_y = (WINDOW_H.saturating_sub(dst_h)) as i32 / 2;

            canvas
                .copy(
                    &texture,
                    None,
                    Some(Rect::new(dst_x, dst_y, dst_w, dst_h)),
                )
                .expect("copy failed");
        }

        // Draw info text as simple colored rectangles for sprite index indicator
        // (SDL2 text rendering requires sdl2_ttf, so we show info in the title bar)
        canvas.window_mut().set_title(&format!(
            "Anno 1602 — Sprite #{current_sprite}/{} — {}×{} type={} — zoom {zoom}x",
            bsh.len(),
            sprite.width,
            sprite.height,
            sprite.sprite_type,
        )).ok();

        canvas.present();
    }
}

/// Find the extracted game data directory.
fn find_data_dir() -> std::path::PathBuf {
    // Check command line arg first
    if let Some(dir) = std::env::args().nth(1) {
        let p = Path::new(&dir).to_path_buf();
        if p.exists() {
            return p;
        }
    }

    // Check relative to crate/workspace
    for candidate in &[
        "extracted",
        "../extracted",
        "../../extracted",
    ] {
        let p = Path::new(candidate);
        if p.join("GFX/STADTFLD.BSH").exists() {
            return p.to_path_buf();
        }
    }

    eprintln!("Could not find game data directory. Pass the path to 'extracted/' as an argument.");
    std::process::exit(1);
}
