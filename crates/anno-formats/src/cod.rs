//! COD file parser for game data (haeuser.cod, figuren.cod, text.cod).
//!
//! COD files are text-based configuration files with key-value properties and
//! stateful incremental definitions. Encrypted files use byte negation:
//!   decrypted = (-encrypted) & 0xFF
//!
//! The haeuser.cod file defines ~500 building types with properties like:
//!   - @Nummer: building ID (incremental with +1)
//!   - Gfx/@Gfx: sprite index in STADTFLD.BSH (absolute or relative)
//!   - Kind: building category (BODEN, ROHSTOFF, HANDWERK, WOHN, etc.)
//!   - Size: tile dimensions
//!   - Various production, cost, and behavior properties

use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CodError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A parsed COD file containing constants and building definitions.
#[derive(Debug)]
pub struct CodFile {
    /// Named constants (GFXBODEN, IDBODEN, etc.)
    pub constants: HashMap<String, i32>,
    /// Building definitions indexed by Nummer (0..N)
    pub buildings: Vec<BuildingDef>,
}

/// A building/terrain definition from haeuser.cod.
#[derive(Debug, Clone)]
pub struct BuildingDef {
    /// Building number (sequential ID used in INSELHAUS records as sprite index)
    pub nummer: i32,
    /// Sprite index in STADTFLD.BSH
    pub gfx: i32,
    /// Building sprite index (for construction display)
    pub baugfx: i32,
    /// Building category
    pub kind: String,
    /// Tile dimensions (width, height)
    pub size: (i32, i32),
    /// Number of rotations
    pub rotate: i32,
    /// Animation frame count
    pub anim_anz: i32,
    /// Animation sprite offset per frame
    pub anim_add: i32,
    /// Animation speed in milliseconds per frame (0 = use default 200ms)
    pub anim_time: i32,
    /// All raw properties
    pub properties: HashMap<String, String>,
}

impl Default for BuildingDef {
    fn default() -> Self {
        Self {
            nummer: 0,
            gfx: 0,
            baugfx: -1,
            kind: String::new(),
            size: (1, 1),
            rotate: 0,
            anim_anz: 1,
            anim_add: 0,
            anim_time: 0,
            properties: HashMap::new(),
        }
    }
}

impl CodFile {
    /// Parse a COD file from raw bytes.
    pub fn parse(data: &[u8]) -> Result<Self, CodError> {
        let text = Self::decrypt(data);
        Self::parse_text(&text)
    }

    /// Decrypt COD file bytes. Encrypted files use byte negation.
    fn decrypt(data: &[u8]) -> String {
        // Check if already plaintext (first bytes are printable ASCII)
        let raw = if data.len() > 4
            && data[..4]
                .iter()
                .all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
        {
            data.to_vec()
        } else {
            // Byte negation: decrypted = (-byte) & 0xFF
            data.iter().map(|&b| (-(b as i16) & 0xFF) as u8).collect()
        };

        // Convert from CP1252/Latin-1 to UTF-8 by treating each byte as a Unicode code point
        raw.iter().map(|&b| char::from(b)).collect()
    }

    fn parse_text(text: &str) -> Result<CodFile, CodError> {
        let mut constants: HashMap<String, i32> = HashMap::new();
        let mut buildings: Vec<BuildingDef> = Vec::new();
        let mut current = BuildingDef::default();
        let mut in_building = false;
        let mut obj_depth = 0i32;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }

            // Strip inline comments
            let line = line.split(';').next().unwrap_or(line).trim();
            if line.is_empty() {
                continue;
            }


            // Handle sub-objects (Objekt: ... EndObj;)
            // We still parse key:value pairs inside sub-objects and store them
            // as properties on the current building.
            if line.starts_with("Objekt:") || line.starts_with("Objekt\t") {
                obj_depth += 1;
                // Store which sub-object type we're in (HAUS_PRODTYP, HAUS_BAUKOST, etc.)
                if let Some(obj_type) = line.split_whitespace().nth(1) {
                    current
                        .properties
                        .insert("_last_objekt".to_string(), obj_type.to_string());
                }
                continue;
            }
            if line.starts_with("EndObj") {
                obj_depth = (obj_depth - 1).max(0);
                continue;
            }
            if obj_depth > 0 {
                // Allow @Nummer: to reset depth (it's a new building)
                if line.starts_with("@Nummer:") {
                    obj_depth = 0;
                    // Fall through to @Nummer handling below
                } else {
                    // Parse key:value inside sub-objects as building properties
                    if let Some((key, value)) = line.split_once(':') {
                        let key = key.trim().trim_start_matches('@');
                        let value = value.trim();
                        if !key.is_empty() {
                            // Avoid overwriting the outer Kind with the inner Kind
                            let storage_key = if key == "Kind" {
                                "ProdKind".to_string()
                            } else {
                                key.to_string()
                            };
                            current
                                .properties
                                .insert(storage_key, value.to_string());
                        }
                    } else if let Some((name, expr)) = line.split_once('=') {
                        // Constants inside sub-objects
                        let name = name.trim();
                        let expr = expr.trim();
                        if !name.is_empty()
                            && name
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                        {
                            let val = Self::eval(&constants, expr);
                            constants.insert(name.to_string(), val);
                        }
                    }
                    continue;
                }
            }

            // Handle ObjFill (copy from template — we just keep current state)
            if line.starts_with("ObjFill:") {
                continue;
            }

            // Parse @Nummer: incremental building ID
            if let Some(val_str) = line.strip_prefix("@Nummer:") {
                let val_str = val_str.trim();
                if in_building {
                    buildings.push(current.clone());
                }
                if let Some(delta) = val_str.strip_prefix('+') {
                    current.nummer += Self::eval(&constants, delta.trim());
                } else {
                    current.nummer = Self::eval(&constants, val_str);
                }
                in_building = true;
                continue;
            }

            // Parse Gfx / @Gfx
            if let Some(val_str) = line.strip_prefix("@Gfx:") {
                let val_str = val_str.trim();
                if let Some(delta) = val_str.strip_prefix('+') {
                    current.gfx += Self::eval(&constants, delta.trim());
                } else if val_str.starts_with('-') {
                    current.gfx += Self::eval(&constants, val_str);
                } else {
                    current.gfx = Self::eval(&constants, val_str);
                }
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Gfx:") {
                current.gfx = Self::eval(&constants, val_str.trim());
                continue;
            }

            // Parse Baugfx
            if let Some(val_str) = line.strip_prefix("Baugfx:") {
                current.baugfx = Self::eval(&constants, val_str.trim());
                continue;
            }

            // Parse Kind
            if let Some(val_str) = line.strip_prefix("Kind:") {
                current.kind = val_str.trim().to_string();
                continue;
            }

            // Parse Size
            if let Some(val_str) = line.strip_prefix("Size:") {
                let parts: Vec<&str> = val_str.split(',').collect();
                if parts.len() >= 2 {
                    current.size = (
                        Self::eval(&constants, parts[0].trim()),
                        Self::eval(&constants, parts[1].trim()),
                    );
                }
                continue;
            }

            // Parse Rotate
            if let Some(val_str) = line.strip_prefix("Rotate:") {
                current.rotate = Self::eval(&constants, val_str.trim());
                continue;
            }

            // Parse AnimAnz
            if let Some(val_str) = line.strip_prefix("AnimAnz:") {
                current.anim_anz = Self::eval(&constants, val_str.trim());
                continue;
            }

            // Parse AnimAdd
            if let Some(val_str) = line.strip_prefix("AnimAdd:") {
                current.anim_add = Self::eval(&constants, val_str.trim());
                continue;
            }

            // Parse AnimTime (ms per animation frame)
            if let Some(val_str) = line.strip_prefix("AnimTime:") {
                current.anim_time = Self::eval(&constants, val_str.trim());
                continue;
            }

            // Parse @Id: (incremental) and Id: (absolute)
            if line.starts_with("@Id:") || line.starts_with("Id:") {
                // Store as property but don't need special handling
                if let Some((key, value)) = line.split_once(':') {
                    current
                        .properties
                        .insert(key.trim().to_string(), value.trim().to_string());
                }
                continue;
            }

            // Parse constant definitions: NAME = EXPR
            if let Some((name, expr)) = line.split_once('=') {
                let name = name.trim();
                let expr = expr.trim();
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    let val = Self::eval(&constants, expr);
                    constants.insert(name.to_string(), val);
                    // Also store named values from building context
                    if in_building {
                        current
                            .properties
                            .insert(name.to_string(), val.to_string());
                    }
                }
                continue;
            }

            // Other key: value properties
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().trim_start_matches('@');
                current
                    .properties
                    .insert(key.to_string(), value.trim().to_string());
            }
        }

        // Push last building
        if in_building {
            buildings.push(current);
        }

        Ok(CodFile {
            constants,
            buildings,
        })
    }

    /// Evaluate a simple expression: number, constant name, or NAME+/-NUM.
    fn eval(constants: &HashMap<String, i32>, expr: &str) -> i32 {
        let expr = expr.trim();
        if expr.is_empty() {
            return 0;
        }

        // Try direct number
        if let Ok(n) = expr.parse::<i32>() {
            return n;
        }

        // Try NAME+NUM or NAME-NUM
        for (i, c) in expr.char_indices() {
            if i > 0 && (c == '+' || c == '-') {
                let name = expr[..i].trim();
                let offset_str = expr[i..].trim();
                if let Some(&base) = constants.get(name) {
                    if let Ok(offset) = offset_str.parse::<i32>() {
                        return base + offset;
                    }
                }
            }
        }

        // Try just a constant name
        if let Some(&val) = constants.get(expr) {
            return val;
        }

        // Try as float (e.g., "1.3")
        if let Ok(f) = expr.parse::<f64>() {
            return f as i32;
        }

        0
    }

    /// Look up a building by its sprite index (Gfx value).
    /// This is what INSELHAUS building_id maps to.
    pub fn building_by_gfx(&self, gfx: i32) -> Option<&BuildingDef> {
        self.buildings.iter().find(|b| b.gfx == gfx)
    }

    /// Build a lookup table: gfx → building index.
    pub fn gfx_to_building_map(&self) -> HashMap<i32, usize> {
        let mut map = HashMap::new();
        for (i, b) in self.buildings.iter().enumerate() {
            map.entry(b.gfx).or_insert(i);
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_haeuser_cod() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/haeuser.cod");

        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping test: {path:?} not found");
                return;
            }
        };

        let cod = CodFile::parse(&data).expect("failed to parse COD");

        println!("Constants: {}", cod.constants.len());
        println!("Buildings: {}", cod.buildings.len());

        // Check some known constants
        assert_eq!(cod.constants.get("GFXBODEN"), Some(&0));
        assert_eq!(cod.constants.get("GFXHANG"), Some(&432));
        assert_eq!(cod.constants.get("GFXMEER"), Some(&680));

        // First entry is the UNUSED template (nummer=0), second is real building 0 (BODEN)
        assert_eq!(cod.buildings[0].nummer, 0);
        assert_eq!(cod.buildings[0].kind, "UNUSED");

        // The real building 0 is the second entry
        assert_eq!(cod.buildings[1].nummer, 0);
        assert_eq!(cod.buildings[1].gfx, 0);
        assert_eq!(cod.buildings[1].kind, "BODEN");

        // Should have ~500 buildings
        assert!(
            cod.buildings.len() >= 490,
            "expected ~500 buildings, got {}",
            cod.buildings.len()
        );

        // Print sample buildings
        println!("\nSample buildings:");
        for b in cod.buildings.iter().take(5) {
            println!(
                "  #{}: gfx={} kind={} size={:?} rotate={}",
                b.nummer, b.gfx, b.kind, b.size, b.rotate
            );
        }

        // Verify production properties are captured from sub-objects
        let production_buildings: Vec<_> = cod
            .buildings
            .iter()
            .filter(|b| {
                matches!(
                    b.properties.get("ProdKind").map(|s| s.as_str()),
                    Some("HANDWERK" | "ROHSTOFF" | "PLANTAGE" | "BERGWERK" | "STEINBRUCH")
                )
            })
            .collect();
        println!(
            "\nProduction buildings (HANDWERK/ROHSTOFF/PLANTAGE/BERGWERK/STEINBRUCH): {}",
            production_buildings.len()
        );
        assert!(
            production_buildings.len() >= 20,
            "expected >= 20 production buildings, got {}",
            production_buildings.len()
        );

        // Print some production buildings
        for b in production_buildings.iter().take(8) {
            println!(
                "  #{}: kind={} prodkind={} Ware={} Rohstoff={} Interval={} Maxlager={}",
                b.nummer,
                b.kind,
                b.properties.get("ProdKind").unwrap_or(&"?".into()),
                b.properties.get("Ware").unwrap_or(&"?".into()),
                b.properties.get("Rohstoff").unwrap_or(&"?".into()),
                b.properties.get("Interval").unwrap_or(&"?".into()),
                b.properties.get("Maxlager").unwrap_or(&"?".into()),
            );
        }
    }
}
