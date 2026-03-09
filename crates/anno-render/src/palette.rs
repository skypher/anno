//! 256-color palette management with remap tables for player colors and effects.
//!
//! Ported from the palette initialization at FUN_004918b0 and the
//! remap table generators FUN_004031b0/FUN_00403220/FUN_004032f0.

/// A 256-entry RGB palette.
pub type Palette = [[u8; 3]; 256];

/// A 256-entry color remap table (maps source palette index -> destination palette index).
pub type RemapTable = [u8; 256];

/// Number of player color remap tables.
pub const NUM_PLAYER_COLORS: usize = 8;

/// Find the nearest palette index for an RGB color using squared Euclidean distance.
pub fn nearest_color(palette: &Palette, r: u8, g: u8, b: u8) -> u8 {
    let mut best_idx = 0u8;
    let mut best_dist = u32::MAX;

    for (i, entry) in palette.iter().enumerate() {
        let dr = r as i32 - entry[0] as i32;
        let dg = g as i32 - entry[1] as i32;
        let db = b as i32 - entry[2] as i32;
        let dist = (dr * dr + dg * dg + db * db) as u32;
        if dist < best_dist {
            best_dist = dist;
            best_idx = i as u8;
        }
    }

    best_idx
}

/// Build a luminance-based remap table.
///
/// For each palette entry, converts to grayscale using the formula:
///   gray = (R * 0x22 + G * 0x21 + B * 0x21) / 100
/// Then finds the nearest palette entry for (gray, gray, gray).
pub fn build_luminance_remap(palette: &Palette) -> RemapTable {
    let mut table = [0u8; 256];
    for (i, entry) in palette.iter().enumerate() {
        let gray = (entry[0] as u32 * 0x22 + entry[1] as u32 * 0x21 + entry[2] as u32 * 0x21)
            / 100;
        let gray = gray.min(255) as u8;
        table[i] = nearest_color(palette, gray, gray, gray);
    }
    table
}

/// Build a color-tinted remap table with RGB scale factors.
///
/// Each palette color is multiplied by (r_factor, g_factor, b_factor) and then
/// mapped to the nearest palette entry.
pub fn build_tinted_remap(palette: &Palette, r_factor: f32, g_factor: f32, b_factor: f32) -> RemapTable {
    let mut table = [0u8; 256];
    for (i, entry) in palette.iter().enumerate() {
        let r = (entry[0] as f32 * r_factor).min(255.0) as u8;
        let g = (entry[1] as f32 * g_factor).min(255.0) as u8;
        let b = (entry[2] as f32 * b_factor).min(255.0) as u8;
        table[i] = nearest_color(palette, r, g, b);
    }
    table
}

/// Standard named colors used by the engine.
pub fn resolve_named_colors(palette: &Palette) -> NamedColors {
    NamedColors {
        white: nearest_color(palette, 255, 255, 255),
        black: nearest_color(palette, 0, 0, 0),
        red: nearest_color(palette, 255, 0, 0),
        green: nearest_color(palette, 0, 255, 0),
        blue: nearest_color(palette, 0, 0, 255),
        cyan: nearest_color(palette, 0, 255, 255),
        yellow: nearest_color(palette, 255, 255, 0),
    }
}

pub struct NamedColors {
    pub white: u8,
    pub black: u8,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub cyan: u8,
    pub yellow: u8,
}
