//! BSH sprite format parser.
//!
//! BSH files contain RLE-encoded 8-bit indexed color sprites.
//!
//! File format (from Anno 1602 RE):
//!   - Chunk header: "BSH\0"(4) + field1(4) + field2(4) + field3(4) + data_length(4) = 20 bytes
//!   - BSH data section (starting at offset 20):
//!     - Offset table: N × u32 offsets (relative to start of data section)
//!     - N is determined by first_offset / 4
//!     - Sprite data at each offset:
//!       width(u32) + height(u32) + type(u32) + data_len(u32) + pixel_data[data_len]
//!   - RLE pixel data encoding:
//!     - 0xFF: end of sprite
//!     - 0xFE: end of scanline (move to next row)
//!     - Other byte N: skip N transparent pixels, then read next byte M = count of opaque pixels,
//!       followed by M raw palette index bytes

use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum BshError {
    #[error("invalid BSH magic")]
    InvalidMagic,
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid sprite offset")]
    InvalidOffset,
}

#[derive(Debug, Clone)]
pub struct BshSprite {
    pub width: u32,
    pub height: u32,
    pub sprite_type: u32,
    /// Raw RLE-encoded pixel data
    pub rle_data: Vec<u8>,
}

#[derive(Debug)]
pub struct BshFile {
    pub sprites: Vec<BshSprite>,
}

/// Size of the BSH chunk header before the data section.
const CHUNK_HEADER_SIZE: usize = 20;

impl BshFile {
    /// Parse a BSH file from raw bytes.
    pub fn parse(data: &[u8]) -> Result<Self, BshError> {
        if data.len() < CHUNK_HEADER_SIZE {
            return Err(BshError::InvalidMagic);
        }

        // Verify magic "BSH\0"
        if &data[0..4] != b"BSH\0" {
            return Err(BshError::InvalidMagic);
        }

        // Data section starts after the 20-byte chunk header
        let data_section = &data[CHUNK_HEADER_SIZE..];
        let mut cursor = Cursor::new(data_section);

        // First offset tells us the size of the offset table
        let first_offset = cursor.read_u32::<LittleEndian>()?;
        if first_offset == 0 || first_offset % 4 != 0 {
            return Err(BshError::InvalidOffset);
        }

        let num_sprites = (first_offset / 4) as usize;

        // Read all offsets (we already read the first one)
        let mut offsets = Vec::with_capacity(num_sprites);
        offsets.push(first_offset);
        for _ in 1..num_sprites {
            offsets.push(cursor.read_u32::<LittleEndian>()?);
        }

        // Deduplicate offsets to avoid parsing the same sprite multiple times.
        // Many sprites share the same data (e.g., rotations point to same sprite).
        let mut unique_offsets: Vec<u32> = offsets.clone();
        unique_offsets.sort();
        unique_offsets.dedup();

        // Parse sprite data at each unique offset
        let mut offset_to_sprite: std::collections::HashMap<u32, usize> =
            std::collections::HashMap::new();
        let mut unique_sprites = Vec::new();

        for &offset in &unique_offsets {
            if offset as usize + 16 > data_section.len() {
                continue;
            }

            cursor.seek(SeekFrom::Start(offset as u64))?;

            let width = cursor.read_u32::<LittleEndian>()?;
            let height = cursor.read_u32::<LittleEndian>()?;
            let sprite_type = cursor.read_u32::<LittleEndian>()?;
            let data_len = cursor.read_u32::<LittleEndian>()?;

            let data_len = data_len.min((data_section.len() - offset as usize - 16) as u32);
            let mut rle_data = vec![0u8; data_len as usize];
            cursor.read_exact(&mut rle_data)?;

            offset_to_sprite.insert(offset, unique_sprites.len());
            unique_sprites.push(BshSprite {
                width,
                height,
                sprite_type,
                rle_data,
            });
        }

        // Build the sprite list preserving original indices (with duplicates)
        let mut sprites = Vec::with_capacity(num_sprites);
        for &offset in &offsets {
            if let Some(&idx) = offset_to_sprite.get(&offset) {
                sprites.push(unique_sprites[idx].clone());
            }
        }

        Ok(BshFile { sprites })
    }

    /// Number of sprites in this file.
    pub fn len(&self) -> usize {
        self.sprites.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sprites.is_empty()
    }
}

impl BshSprite {
    /// Decode RLE data into an RGBA pixel buffer.
    /// Transparent pixels are (0, 0, 0, 0).
    /// Opaque pixels use the palette to map index -> RGB, with alpha = 255.
    pub fn decode(&self, palette: &[[u8; 3]; 256]) -> Vec<u8> {
        let mut pixels = vec![0u8; (self.width * self.height * 4) as usize];
        let mut x: u32 = 0;
        let mut y: u32 = 0;
        let mut i = 0;
        let data = &self.rle_data;

        while i < data.len() {
            let byte = data[i];
            i += 1;

            match byte {
                0xFF => break, // end of sprite
                0xFE => {
                    // end of scanline
                    x = 0;
                    y += 1;
                }
                skip => {
                    x += skip as u32;
                    if i >= data.len() {
                        break;
                    }
                    let count = data[i] as u32;
                    i += 1;

                    for _ in 0..count {
                        if i >= data.len() || x >= self.width || y >= self.height {
                            break;
                        }
                        let idx = data[i] as usize;
                        i += 1;

                        let pixel_offset = ((y * self.width + x) * 4) as usize;
                        if pixel_offset + 3 < pixels.len() {
                            pixels[pixel_offset] = palette[idx][0];
                            pixels[pixel_offset + 1] = palette[idx][1];
                            pixels[pixel_offset + 2] = palette[idx][2];
                            pixels[pixel_offset + 3] = 255;
                        }
                        x += 1;
                    }
                }
            }
        }

        pixels
    }

    /// Decode to 8-bit indexed buffer (0 = transparent).
    pub fn decode_indexed(&self) -> Vec<u8> {
        let mut pixels = vec![0u8; (self.width * self.height) as usize];
        let mut x: u32 = 0;
        let mut y: u32 = 0;
        let mut i = 0;
        let data = &self.rle_data;

        while i < data.len() {
            let byte = data[i];
            i += 1;

            match byte {
                0xFF => break,
                0xFE => {
                    x = 0;
                    y += 1;
                }
                skip => {
                    x += skip as u32;
                    if i >= data.len() {
                        break;
                    }
                    let count = data[i] as u32;
                    i += 1;

                    for _ in 0..count {
                        if i >= data.len() || x >= self.width || y >= self.height {
                            break;
                        }
                        let idx = data[i];
                        i += 1;

                        let pixel_offset = (y * self.width + x) as usize;
                        if pixel_offset < pixels.len() {
                            pixels[pixel_offset] = idx;
                        }
                        x += 1;
                    }
                }
            }
        }

        pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stadtfld_bsh() {
        let data = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("extracted/GFX/STADTFLD.BSH"),
        );
        if let Ok(data) = data {
            let bsh = BshFile::parse(&data).expect("failed to parse BSH");
            assert!(bsh.len() > 100, "expected many sprites, got {}", bsh.len());

            // Check first sprite dimensions (should be 64x31 for full-zoom terrain)
            let first = &bsh.sprites[0];
            assert_eq!(first.width, 64);
            assert_eq!(first.height, 31);
            assert_eq!(first.sprite_type, 1); // RLE type

            // Decode first sprite to indexed
            let indexed = first.decode_indexed();
            assert_eq!(indexed.len(), (64 * 31) as usize);

            println!(
                "Parsed {} sprites, first: {}x{} type={}",
                bsh.len(),
                first.width,
                first.height,
                first.sprite_type
            );
        } else {
            println!("Skipping test: game files not found");
        }
    }
}
