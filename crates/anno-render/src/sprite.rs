//! Sprite management and rendering dispatch.
//!
//! Ported from FUN_0048a140 (sprite rendering dispatch) and the
//! BSH sprite table at DAT_0049d778.

use crate::framebuffer::Framebuffer;
use crate::palette::RemapTable;
use anno_formats::bsh::BshFile;

/// A loaded sprite set (one BSH file).
pub struct SpriteSet {
    pub bsh: BshFile,
}

impl SpriteSet {
    pub fn new(bsh: BshFile) -> Self {
        Self { bsh }
    }

    /// Draw a sprite at screen position with optional player color remap.
    pub fn draw(
        &self,
        fb: &mut Framebuffer,
        sprite_idx: usize,
        x: i32,
        y: i32,
        player_color: Option<&RemapTable>,
    ) {
        if sprite_idx >= self.bsh.sprites.len() {
            return;
        }

        let sprite = &self.bsh.sprites[sprite_idx];

        match player_color {
            Some(remap) => {
                fb.blit_rle_remapped(x, y, &sprite.rle_data, remap);
            }
            None => {
                fb.blit_rle(x, y, &sprite.rle_data);
            }
        }
    }

    pub fn sprite_dimensions(&self, sprite_idx: usize) -> Option<(u32, u32)> {
        self.bsh
            .sprites
            .get(sprite_idx)
            .map(|s| (s.width, s.height))
    }
}

/// Manages multiple sprite sets (different zoom levels and categories).
pub struct SpriteManager {
    /// Sprite sets indexed by: [zoom_level * categories + category]
    sets: Vec<Option<SpriteSet>>,
}

/// Sprite category indices matching the original engine's BSH loading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteCategory {
    Stadtfld = 0, // City tiles (terrain + buildings)
    Soldat = 1,   // Soldiers/military units
    Ship = 2,     // Ships
    Traeger = 3,  // Carriers/porters
    Maeher = 4,   // Harvesters
    Tiere = 5,    // Animals
    Effekte = 6,  // Effects (smoke, fire, etc.)
    Numbers = 7,  // Number overlays
    Schatten = 8, // Shadows
    Fische = 9,   // Fish
    Gaukler = 10, // Entertainers
}

const NUM_CATEGORIES: usize = 11;
const NUM_ZOOM_LEVELS: usize = 3;

impl SpriteManager {
    pub fn new() -> Self {
        let mut sets = Vec::with_capacity(NUM_CATEGORIES * NUM_ZOOM_LEVELS);
        for _ in 0..NUM_CATEGORIES * NUM_ZOOM_LEVELS {
            sets.push(None);
        }
        Self { sets }
    }

    pub fn load_set(&mut self, category: SpriteCategory, zoom_index: usize, bsh: BshFile) {
        let idx = zoom_index * NUM_CATEGORIES + category as usize;
        if idx < self.sets.len() {
            self.sets[idx] = Some(SpriteSet::new(bsh));
        }
    }

    pub fn get_set(&self, category: SpriteCategory, zoom_index: usize) -> Option<&SpriteSet> {
        let idx = zoom_index * NUM_CATEGORIES + category as usize;
        self.sets.get(idx).and_then(|s| s.as_ref())
    }

    /// Load all sprite sets from a game data directory.
    ///
    /// Expects the directory to contain GFX/, MGFX/, and SGFX/ subdirectories
    /// with BSH files for each sprite category.
    pub fn load_from_dir(base: &std::path::Path) -> Self {
        let mut mgr = Self::new();

        let zoom_dirs = [
            (0, "GFX"),  // full zoom
            (1, "MGFX"), // medium zoom
            (2, "SGFX"), // small zoom
        ];

        let categories = [
            (SpriteCategory::Stadtfld, "STADTFLD"),
            (SpriteCategory::Soldat, "SOLDAT"),
            (SpriteCategory::Ship, "SHIP"),
            (SpriteCategory::Traeger, "TRAEGER"),
            (SpriteCategory::Maeher, "MAEHER"),
            (SpriteCategory::Tiere, "TIERE"),
            (SpriteCategory::Effekte, "EFFEKTE"),
            (SpriteCategory::Numbers, "NUMBERS"),
            (SpriteCategory::Schatten, "SCHATTEN"),
            (SpriteCategory::Fische, "FISCHE"),
            (SpriteCategory::Gaukler, "GAUKLER"),
        ];

        for &(zoom_idx, zoom_dir) in &zoom_dirs {
            let dir = base.join(zoom_dir);
            if !dir.exists() {
                continue;
            }
            for &(category, name) in &categories {
                // Try case-insensitive match: STADTFLD.BSH, Stadtfld.bsh, etc.
                let bsh_name = format!("{name}.BSH");
                let path = Self::find_case_insensitive(&dir, &bsh_name);
                if let Some(path) = path {
                    match std::fs::read(&path) {
                        Ok(data) => match BshFile::parse(&data) {
                            Ok(bsh) => {
                                let count = bsh.sprites.len();
                                mgr.load_set(category, zoom_idx, bsh);
                                eprintln!(
                                    "  Loaded {}/{} ({} sprites)",
                                    zoom_dir,
                                    path.file_name().unwrap_or_default().to_string_lossy(),
                                    count
                                );
                            }
                            Err(e) => eprintln!("  Failed to parse {path:?}: {e}"),
                        },
                        Err(e) => eprintln!("  Failed to read {path:?}: {e}"),
                    }
                }
            }
        }

        mgr
    }

    /// Find a file by name in a directory, case-insensitive.
    fn find_case_insensitive(
        dir: &std::path::Path,
        name: &str,
    ) -> Option<std::path::PathBuf> {
        let name_lower = name.to_lowercase();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    == name_lower
                {
                    return Some(entry.path());
                }
            }
        }
        None
    }
}
