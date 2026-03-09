//! 8-bit indexed framebuffer with clipping.
//!
//! Ported from the render target structure used throughout the engine.
//! The original had this layout at the render target pointer:
//!   +0x00: clip_x, +0x04: clip_y, +0x08: clip_w, +0x0C: clip_h
//!   +0x10: pitch, +0x18: bytes_per_pixel, +0x1C: mode, +0x20: pixels

use crate::palette::RemapTable;

/// An 8-bit indexed color framebuffer with clipping rectangle.
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub pixels: Vec<u8>,
    pub clip_x: i32,
    pub clip_y: i32,
    pub clip_w: u32,
    pub clip_h: u32,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let pitch = (width + 3) & !3; // 4-byte aligned
        Self {
            width,
            height,
            pitch,
            pixels: vec![0; (pitch * height) as usize],
            clip_x: 0,
            clip_y: 0,
            clip_w: width,
            clip_h: height,
        }
    }

    pub fn set_clip(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.clip_x = x;
        self.clip_y = y;
        self.clip_w = w;
        self.clip_h = h;
    }

    pub fn clear(&mut self, color: u8) {
        self.pixels.fill(color);
    }

    /// Put a single pixel (with clip check).
    #[inline]
    pub fn put_pixel(&mut self, x: i32, y: i32, color: u8) {
        if x >= self.clip_x
            && y >= self.clip_y
            && x < self.clip_x + self.clip_w as i32
            && y < self.clip_y + self.clip_h as i32
        {
            let offset = (y as u32 * self.pitch + x as u32) as usize;
            if offset < self.pixels.len() {
                self.pixels[offset] = color;
            }
        }
    }

    /// Blit a raw bitmap sprite (type 0x02) with transparency (index 0 = transparent).
    pub fn blit_raw(&mut self, x: i32, y: i32, w: u32, h: u32, data: &[u8]) {
        for row in 0..h as i32 {
            let sy = y + row;
            if sy < self.clip_y || sy >= self.clip_y + self.clip_h as i32 {
                continue;
            }
            for col in 0..w as i32 {
                let sx = x + col;
                if sx < self.clip_x || sx >= self.clip_x + self.clip_w as i32 {
                    continue;
                }
                let src_offset = (row as u32 * w + col as u32) as usize;
                if src_offset < data.len() && data[src_offset] != 0 {
                    let dst_offset = (sy as u32 * self.pitch + sx as u32) as usize;
                    if dst_offset < self.pixels.len() {
                        self.pixels[dst_offset] = data[src_offset];
                    }
                }
            }
        }
    }

    /// Blit an RLE-encoded BSH sprite.
    pub fn blit_rle(&mut self, x: i32, y: i32, rle_data: &[u8]) {
        let mut cx = 0i32;
        let mut cy = 0i32;
        let mut i = 0;

        while i < rle_data.len() {
            let byte = rle_data[i];
            i += 1;

            match byte {
                0xFF => break,
                0xFE => {
                    cx = 0;
                    cy += 1;
                }
                skip => {
                    cx += skip as i32;
                    if i >= rle_data.len() {
                        break;
                    }
                    let count = rle_data[i] as i32;
                    i += 1;

                    let sy = y + cy;
                    let in_y_range =
                        sy >= self.clip_y && sy < self.clip_y + self.clip_h as i32;

                    for _ in 0..count {
                        if i >= rle_data.len() {
                            break;
                        }
                        let color = rle_data[i];
                        i += 1;

                        if in_y_range {
                            let sx = x + cx;
                            if sx >= self.clip_x && sx < self.clip_x + self.clip_w as i32 {
                                let offset = (sy as u32 * self.pitch + sx as u32) as usize;
                                if offset < self.pixels.len() {
                                    self.pixels[offset] = color;
                                }
                            }
                        }
                        cx += 1;
                    }
                }
            }
        }
    }

    /// Blit an RLE sprite with a color remap table (used for player colors).
    pub fn blit_rle_remapped(&mut self, x: i32, y: i32, rle_data: &[u8], remap: &RemapTable) {
        let mut cx = 0i32;
        let mut cy = 0i32;
        let mut i = 0;

        while i < rle_data.len() {
            let byte = rle_data[i];
            i += 1;

            match byte {
                0xFF => break,
                0xFE => {
                    cx = 0;
                    cy += 1;
                }
                skip => {
                    cx += skip as i32;
                    if i >= rle_data.len() {
                        break;
                    }
                    let count = rle_data[i] as i32;
                    i += 1;

                    let sy = y + cy;
                    let in_y_range =
                        sy >= self.clip_y && sy < self.clip_y + self.clip_h as i32;

                    for _ in 0..count {
                        if i >= rle_data.len() {
                            break;
                        }
                        let color = remap[rle_data[i] as usize];
                        i += 1;

                        if in_y_range {
                            let sx = x + cx;
                            if sx >= self.clip_x && sx < self.clip_x + self.clip_w as i32 {
                                let offset = (sy as u32 * self.pitch + sx as u32) as usize;
                                if offset < self.pixels.len() {
                                    self.pixels[offset] = color;
                                }
                            }
                        }
                        cx += 1;
                    }
                }
            }
        }
    }

    /// Convert the indexed framebuffer to RGBA using a palette.
    pub fn to_rgba(&self, palette: &[[u8; 3]; 256]) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = self.pixels[(y * self.pitch + x) as usize] as usize;
                rgba.push(palette[idx][0]);
                rgba.push(palette[idx][1]);
                rgba.push(palette[idx][2]);
                rgba.push(255);
            }
        }
        rgba
    }
}
