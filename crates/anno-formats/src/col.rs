//! COL palette file parser.
//!
//! COL files contain a 256-entry RGB palette used by the game's 8-bit renderer.
//!
//! File format:
//!   - Header: "COL\0" (4 bytes) + metadata (16 bytes) = 20 bytes
//!   - Palette: 256 × 4 bytes (R, G, B, padding)
//!   - Total: 1044 bytes

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ColError {
    #[error("invalid COL magic")]
    InvalidMagic,
    #[error("file too small: expected 1044 bytes, got {0}")]
    TooSmall(usize),
}

/// A 256-entry RGB palette.
pub type Palette = [[u8; 3]; 256];

const HEADER_SIZE: usize = 20;
const PALETTE_ENTRIES: usize = 256;
const ENTRY_SIZE: usize = 4; // R, G, B, padding

/// Parse a COL file into a 256-entry RGB palette.
pub fn parse_col(data: &[u8]) -> Result<Palette, ColError> {
    let expected_size = HEADER_SIZE + PALETTE_ENTRIES * ENTRY_SIZE;
    if data.len() < expected_size {
        return Err(ColError::TooSmall(data.len()));
    }

    if &data[0..4] != b"COL\0" {
        return Err(ColError::InvalidMagic);
    }

    let mut palette = [[0u8; 3]; 256];
    for i in 0..PALETTE_ENTRIES {
        let offset = HEADER_SIZE + i * ENTRY_SIZE;
        palette[i] = [data[offset], data[offset + 1], data[offset + 2]];
    }

    Ok(palette)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stadtfld_col() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/TOOLGFX/STADTFLD.COL");

        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping test: {path:?} not found");
                return;
            }
        };

        let palette = parse_col(&data).expect("failed to parse COL");

        // Entry 0 should be black
        assert_eq!(palette[0], [0, 0, 0]);

        // Entry 1 should be dark red (0x80, 0, 0)
        assert_eq!(palette[1], [0x80, 0, 0]);

        // Entry 2 should be dark green (0, 0x80, 0)
        assert_eq!(palette[2], [0, 0x80, 0]);

        // Last entries should be the standard Windows colors
        assert_eq!(palette[255], [0xFF, 0xFF, 0xFF]); // white

        println!("Palette loaded: first 8 entries:");
        for i in 0..8 {
            println!(
                "  [{i}] R={:02x} G={:02x} B={:02x}",
                palette[i][0], palette[i][1], palette[i][2]
            );
        }
    }
}
