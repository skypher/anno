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
    /// Building number from the source definition.
    pub nummer: i32,
    /// Source `Id` field from haeuser.cod, resolved through the constant table.
    pub source_id: i32,
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
    /// Random variant count (`RandAnz`, ushort at building-definition offset 0x60).
    pub rand_anz: u16,
    /// Random variant stride in building-definition records (`RandAdd`, offset 0x62).
    pub rand_add: u16,
    /// Animation frame count
    pub anim_anz: i32,
    /// Animation sprite offset per frame
    pub anim_add: i32,
    /// Animation speed in milliseconds per frame (0 = use default 200ms)
    pub anim_time: i32,
    /// Source `NoShotFlg` bit consumed by `FUN_0046f6d0` when it constructs
    /// the ship-route obstacle overlay.
    pub no_shot: bool,
    /// Resolved `Ruinenr` code from haeuser.cod. The original parser
    /// registers symbolic tokens in `1602_exe.c:66354-66367`
    /// (`RUINE_HOLZ = 0`, `RUINE_STEIN = 2`, …, `NORUINE = 255`) and
    /// `@Ruinenr: +N` increments the inherited value.
    pub ruinenr: i32,
    /// All raw properties
    pub properties: HashMap<String, String>,
}

impl Default for BuildingDef {
    fn default() -> Self {
        Self {
            nummer: 0,
            source_id: 0,
            gfx: 0,
            baugfx: -1,
            kind: String::new(),
            size: (1, 1),
            rotate: 0,
            rand_anz: 1,
            rand_add: 0,
            anim_anz: 1,
            anim_add: 0,
            anim_time: 0,
            no_shot: false,
            ruinenr: 255,
            properties: HashMap::new(),
        }
    }
}

impl BuildingDef {
    /// Runtime kind code assigned by the `FUN_00460750` symbol table before
    /// haeuser.cod is parsed. The table deliberately aliases terrain and
    /// production labels into one code space; for example `STRASSE` and
    /// `HANDWERK` both resolve to 1.
    pub fn source_kind_code(&self) -> Option<u8> {
        match self.kind.as_str() {
            "UNUSED" => Some(0),
            "STRASSE" | "HANDWERK" => Some(1),
            "PLANTAGE" => Some(2),
            "TOR" | "BERGWERK" => Some(3),
            "MAUER" | "WEIDETIER" => Some(4),
            "MAUERSTRAND" | "JAGDHAUS" => Some(5),
            "TURM" | "FISCHEREI" => Some(6),
            "TURMSTRAND" | "MARKT" => Some(7),
            "KONTOR" => Some(8),
            "ROHSTOFF" => Some(9),
            "WALD" | "ROHSTWACHS" => Some(10),
            "BODEN" | "STEINBRUCH" => Some(11),
            "RUINE" | "ROHSTERZ" => Some(12),
            "PLATZ" | "WOHNUNG" => Some(13),
            "GEBAEUDE" => Some(14),
            "FELS" | "MILITAR" => Some(15),
            "FLUSS" | "WACHTURM" => Some(16),
            "FLUSSECK" | "WIRT" => Some(17),
            "BRUECKE" | "KAPELLE" => Some(18),
            "MEER" | "KIRCHE" => Some(19),
            "BRANDUNG" | "BADEHAUS" => Some(20),
            "BRANDECK" | "THEATER" => Some(21),
            "MUENDUNG" | "KLINIK" => Some(22),
            "STRAND" | "SCHULE" => Some(23),
            "STRANDMUND" | "HOCHSCHULE" => Some(24),
            "STRANDECKA" | "GALGEN" => Some(25),
            "STRANDECKI" | "BRUNNEN" => Some(26),
            "STRANDVARI" | "SCHLOSS" => Some(27),
            "STRANDHAUS" | "DENKMAL" => Some(28),
            "STRANDRUINE" | "TRIUMPH" => Some(29),
            "PIER" => Some(30),
            "HANG" | "PIRATWOHN" => Some(31),
            "HANGECK" | "pMAUER" => Some(32),
            "HANGQUELL" => Some(33),
            "MINE" => Some(34),
            "HQ" => Some(35),
            "HAFEN" => Some(36),
            "WMUEHLE" => Some(37),
            _ => None,
        }
    }

    /// The four movement-type path classes compiled from `Wegspeed` by the
    /// original building loader. At `0x00462852..0x0046287d` it stores
    /// `min(126, floor(speed * 32 / 100))` into the runtime definition;
    /// `FUN_0046f230` later copies one selected class into path-grid
    /// metadata. `None` preserves an absent or malformed source property.
    pub fn source_path_classes(&self) -> Option<[u8; 4]> {
        let speeds: Vec<i32> = self
            .properties
            .get("Wegspeed")?
            .split(',')
            .map(str::trim)
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        let speeds: [i32; 4] = speeds.try_into().ok()?;
        Some(speeds.map(|speed| (speed.saturating_mul(32) / 100).clamp(0, 126) as u8))
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
        let mut building_by_nummer: HashMap<i32, BuildingDef> = HashMap::new();
        // `@field: +/-value` is evaluated by the executable parser against
        // the last value assigned to that field, before `ObjFill` copies are
        // applied to the next runtime record. Keep that state separate from
        // `current`, whose fields may be replaced by an ObjFill template.
        let mut directive_values: HashMap<String, i32> = HashMap::new();
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
                            current.properties.insert(storage_key, value.to_string());
                        }
                    } else if let Some((name, expr)) = line.split_once('=') {
                        // Constants inside sub-objects
                        let name = name.trim();
                        let expr = expr.trim();
                        if !name.is_empty()
                            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        {
                            let val = Self::eval(&constants, expr);
                            constants.insert(name.to_string(), val);
                        }
                    }
                    continue;
                }
            }

            // Handle ObjFill (copy from a previously emitted template record).
            if let Some(val_str) = line.strip_prefix("ObjFill:") {
                let src_expr = val_str.split(',').next().unwrap_or(val_str).trim();
                let src_nummer = Self::eval(&constants, src_expr);
                if let Some(template) = building_by_nummer.get(&src_nummer) {
                    let nummer = current.nummer;
                    current = template.clone();
                    current.nummer = nummer;
                    constants.insert("Nummer".to_string(), current.nummer);
                }
                continue;
            }

            // Parse @Nummer: incremental building ID
            if let Some(val_str) = line.strip_prefix("@Nummer:") {
                let val_str = val_str.trim();
                if in_building {
                    building_by_nummer.insert(current.nummer, current.clone());
                    buildings.push(current.clone());
                }
                if let Some(delta) = val_str.strip_prefix('+') {
                    current.nummer += Self::eval(&constants, delta.trim());
                } else {
                    current.nummer = Self::eval(&constants, val_str);
                }
                constants.insert("Nummer".to_string(), current.nummer);
                in_building = true;
                continue;
            }

            // Parse Gfx / @Gfx
            if let Some(val_str) = line.strip_prefix("@Gfx:") {
                current.gfx =
                    Self::eval_directive(&constants, &mut directive_values, "Gfx", val_str.trim());
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Gfx:") {
                current.gfx = Self::eval(&constants, val_str.trim());
                directive_values.insert("Gfx".to_string(), current.gfx);
                continue;
            }

            // Parse Baugfx
            if let Some(val_str) = line.strip_prefix("@Baugfx:") {
                current.baugfx = Self::eval_directive(
                    &constants,
                    &mut directive_values,
                    "Baugfx",
                    val_str.trim(),
                );
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Baugfx:") {
                current.baugfx = Self::eval(&constants, val_str.trim());
                directive_values.insert("Baugfx".to_string(), current.baugfx);
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

            // Parse RandAnz / RandAdd. The original destruction path reads
            // these as ushort fields at offsets 0x60 / 0x62 and advances by
            // `rand() % RandAnz * RandAdd` building-definition records.
            if let Some(val_str) = line.strip_prefix("RandAnz:") {
                current.rand_anz = Self::eval_u16(&constants, val_str.trim());
                current
                    .properties
                    .insert("RandAnz".to_string(), current.rand_anz.to_string());
                continue;
            }
            if let Some(val_str) = line.strip_prefix("RandAdd:") {
                current.rand_add = Self::eval_u16(&constants, val_str.trim());
                current
                    .properties
                    .insert("RandAdd".to_string(), current.rand_add.to_string());
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

            // Parse NoShotFlg. The executable stores bit zero at runtime
            // definition offset 0x6a, bit 0x10; ship routing consults it in
            // `FUN_0046f6d0`.
            if let Some(val_str) = line.strip_prefix("NoShotFlg:") {
                current.no_shot = Self::eval(&constants, val_str.trim()) & 1 != 0;
                current
                    .properties
                    .insert("NoShotFlg".to_string(), val_str.trim().to_string());
                continue;
            }

            if let Some(val_str) = line.strip_prefix("@Ruinenr:") {
                current.ruinenr = Self::eval_ruinenr_directive(
                    &constants,
                    &mut directive_values,
                    "Ruinenr",
                    val_str.trim(),
                );
                current
                    .properties
                    .insert("Ruinenr".to_string(), current.ruinenr.to_string());
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Ruinenr:") {
                current.ruinenr = Self::eval_ruinenr(&constants, val_str.trim());
                directive_values.insert("Ruinenr".to_string(), current.ruinenr);
                current
                    .properties
                    .insert("Ruinenr".to_string(), current.ruinenr.to_string());
                continue;
            }

            // Parse @Id: (incremental) and Id: (absolute)
            if let Some(val_str) = line.strip_prefix("@Id:") {
                current.source_id =
                    Self::eval_directive(&constants, &mut directive_values, "Id", val_str.trim());
                current
                    .properties
                    .insert("Id".to_string(), current.source_id.to_string());
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Id:") {
                current.source_id = Self::eval(&constants, val_str.trim());
                directive_values.insert("Id".to_string(), current.source_id);
                current
                    .properties
                    .insert("Id".to_string(), current.source_id.to_string());
                continue;
            }

            // Parse constant definitions: NAME = EXPR
            if let Some((name, expr)) = line.split_once('=') {
                let name = name.trim();
                let expr = expr.trim();
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    let val = Self::eval(&constants, expr);
                    constants.insert(name.to_string(), val);
                    // Also store named values from building context
                    if in_building {
                        current.properties.insert(name.to_string(), val.to_string());
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
            building_by_nummer.insert(current.nummer, current.clone());
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

    fn eval_directive(
        constants: &HashMap<String, i32>,
        directive_values: &mut HashMap<String, i32>,
        name: &str,
        expr: &str,
    ) -> i32 {
        let expr = expr.trim();
        let value = if let Some(delta) = expr.strip_prefix('+') {
            directive_values.get(name).copied().unwrap_or(0) + Self::eval(constants, delta)
        } else if expr.starts_with('-') {
            directive_values.get(name).copied().unwrap_or(0) + Self::eval(constants, expr)
        } else {
            Self::eval(constants, expr)
        };
        directive_values.insert(name.to_string(), value);
        value
    }

    fn eval_ruinenr(constants: &HashMap<String, i32>, expr: &str) -> i32 {
        match expr.trim() {
            "RUINE_HOLZ" => 0,
            "RUINE_STEIN" => 2,
            "RUINE_FELD" => 4,
            "RUINE_ROAD_FELD" => 5,
            "RUINE_ROAD_STEIN" => 6,
            "RUINE_MINE" => 7,
            "RUINE_KONTOR_1" => 8,
            "RUINE_KONTOR_2" => 9,
            "RUINE_KONTOR_3" => 10,
            "RUINE_KONTOR_4" => 11,
            "RUINE_KONTOR_N1" => 12,
            "RUINE_KONTOR_N2" => 13,
            "RUINE_MARKT" => 14,
            "NORUINE" => 255,
            other => Self::eval(constants, other),
        }
    }

    fn eval_ruinenr_directive(
        constants: &HashMap<String, i32>,
        directive_values: &mut HashMap<String, i32>,
        name: &str,
        expr: &str,
    ) -> i32 {
        let expr = expr.trim();
        let value = if let Some(delta) = expr.strip_prefix('+') {
            directive_values.get(name).copied().unwrap_or(0) + Self::eval_ruinenr(constants, delta)
        } else if expr.starts_with('-') {
            directive_values.get(name).copied().unwrap_or(0) + Self::eval_ruinenr(constants, expr)
        } else {
            Self::eval_ruinenr(constants, expr)
        };
        directive_values.insert(name.to_string(), value);
        value
    }

    fn eval_u16(constants: &HashMap<String, i32>, expr: &str) -> u16 {
        Self::eval(constants, expr).clamp(0, u16::MAX as i32) as u16
    }

    /// Look up a building by its STADTFLD sprite index (`Gfx` value).
    pub fn building_by_gfx(&self, gfx: i32) -> Option<&BuildingDef> {
        self.buildings.iter().find(|b| b.gfx == gfx)
    }

    /// Look up a building by its resolved source `Id` field.
    pub fn building_by_source_id(&self, source_id: i32) -> Option<&BuildingDef> {
        self.buildings.iter().find(|b| b.source_id == source_id)
    }

    fn building_index_by_source_id(&self, source_id: i32) -> Option<usize> {
        self.buildings.iter().position(|b| b.source_id == source_id)
    }

    /// Resolve an original `Ruinenr` byte to the base ruin building.
    ///
    /// The binary builds this table after parsing haeuser.cod
    /// (`1602_exe.c:68896-68918`) and `FUN_00463f40` indexes it with
    /// the `Ruinenr` byte. On strand tiles the original uses the same
    /// table shifted by one entry.
    pub fn ruin_building(&self, ruinenr: u8, strand: bool) -> Option<&BuildingDef> {
        let index = self.ruin_building_index(ruinenr, strand)?;
        self.buildings.get(index)
    }

    /// Resolve an original `Ruinenr` byte and random draw to the concrete
    /// random variant definition selected by `FUN_00463f40`.
    pub fn ruin_variant_building(
        &self,
        ruinenr: u8,
        strand: bool,
        rand_value: u16,
    ) -> Option<&BuildingDef> {
        let base_index = self.ruin_building_index(ruinenr, strand)?;
        let base = self.buildings.get(base_index)?;
        let variant_count = base.rand_anz.max(1) as usize;
        let variant_stride = base.rand_add as usize;
        let variant = (rand_value as usize % variant_count) * variant_stride;
        self.buildings.get(base_index + variant).or(Some(base))
    }

    fn ruin_building_index(&self, ruinenr: u8, strand: bool) -> Option<usize> {
        let table_index = if strand && ruinenr != 0xff {
            ruinenr.saturating_add(1)
        } else {
            ruinenr
        };
        let source_id = self
            .ruin_source_id(table_index)
            .or_else(|| self.ruin_source_id(ruinenr))?;
        self.building_index_by_source_id(source_id)
    }

    fn ruin_source_id(&self, table_index: u8) -> Option<i32> {
        let c = |name: &str| self.constants.get(name).copied();
        match table_index {
            0 => Some(c("IDRUINE")?),
            1 => Some(c("IDRUINE")? + 9),
            2 => Some(c("IDRUINE")? + 10),
            3 => Some(c("IDRUINE")? + 19),
            4 => Some(c("IDRUINE")? + 20),
            5 => Some(c("IDRUINE")? + 30),
            6 => Some(c("IDRUINE")? + 35),
            7 => Some(c("IDRUINE")? + 40),
            8..=11 => Some(c("IDHAFEN")? + 20 + (table_index as i32 - 8)),
            12 => Some(c("IDNEGER")? + 9),
            13 => Some(c("IDNEGER")? + 39),
            14 => Some(c("IDDIVERS")? + 22),
            _ => None,
        }
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
        assert_eq!(cod.buildings[1].source_kind_code(), Some(11));
        assert_eq!(cod.buildings[1].source_id, 0);
        assert_eq!(
            cod.buildings[1].source_path_classes(),
            Some([38, 38, 38, 32]),
            "BODEN inherits default Wegspeed classes"
        );

        let pasture_nummer = cod.constants["HAUSWACHS"];
        let pasture = cod
            .buildings
            .iter()
            .find(|b| b.nummer == pasture_nummer)
            .expect("first pasture definition");
        assert_eq!(pasture.source_path_classes(), Some([46, 38, 54, 32]));

        let road = cod
            .buildings
            .iter()
            .find(|b| b.kind == "STRASSE")
            .expect("STRASSE definition");
        assert_eq!(road.source_path_classes(), Some([32, 32, 32, 32]));
        assert!(
            cod.buildings
                .iter()
                .all(|building| building.source_path_classes().is_some()),
            "every source building definition retains four Wegspeed classes"
        );
        assert!(
            cod.buildings
                .iter()
                .all(|building| building.source_kind_code().is_some()),
            "every top-level source building kind has an executable code"
        );
        assert!(
            cod.buildings.iter().any(|building| building.no_shot),
            "NoShotFlg definitions retain the source bit used by ship routing"
        );

        // Should have ~500 buildings
        assert!(
            cod.buildings.len() >= 490,
            "expected ~500 buildings, got {}",
            cod.buildings.len()
        );

        // Residence/HQ-kind buildings carry the per-pop
        // baseline rates. Audit confirms every non-Kontor
        // HQ entry (Nr 270 plus the native chief huts at
        // 442/448/455) uses Nahrung 1.3 / Steuer 2.6 — those
        // are the per-population food consumption and tax
        // baseline values feeding the simulation's
        // population.rs CONSUMPTION_PER_100 table.
        for b in &cod.buildings {
            if b.kind != "HQ" {
                continue;
            }
            if b.properties
                .get("Bauinfra")
                .map(|s| s.starts_with("INFRA_KONTOR"))
                .unwrap_or(false)
            {
                continue;
            }
            assert_eq!(
                b.properties.get("Nahrung").map(|s| s.as_str()),
                Some("1.3"),
                "Nr={}",
                b.nummer
            );
            assert_eq!(
                b.properties.get("Steuer").map(|s| s.as_str()),
                Some("2.6"),
                "Nr={}",
                b.nummer
            );
        }

        let ruin_cases = [
            (270, 8),   // RUINE_KONTOR_1
            (271, 9),   // ObjFill: BASE, then @Ruinenr: +1
            (272, 10),  // next @Ruinenr: +1 directive value
            (273, 11),  // next @Ruinenr: +1 directive value
            (274, 0),   // RUINE_HOLZ
            (275, 0),   // RUINE_HOLZ
            (276, 2),   // RUINE_STEIN
            (277, 2),   // RUINE_STEIN
            (359, 255), // NORUINE
        ];
        for (nummer, ruinenr) in ruin_cases {
            let b = cod
                .buildings
                .iter()
                .find(|b| b.nummer == nummer)
                .unwrap_or_else(|| panic!("missing building Nr={nummer}"));
            assert_eq!(b.ruinenr, ruinenr, "Nr={nummer} Ruinenr");
        }

        let road_id = cod.constants["IDROAD"];
        let road_gfx = cod.constants["GFXROAD"];
        for (source_id_offset, gfx_offset, kind) in [
            (20, 40, "PLATZ"),
            (21, 44, "PLATZ"),
            (22, 48, "PLATZ"),
            (23, 52, "GEBAEUDE"),
        ] {
            let building = cod
                .building_by_source_id(road_id + source_id_offset)
                .unwrap_or_else(|| panic!("missing road source ID {source_id_offset}"));
            assert_eq!(building.gfx, road_gfx + gfx_offset);
            assert_eq!(building.kind, kind);
        }

        let idruine = cod.constants["IDRUINE"];
        assert_eq!(
            cod.ruin_building(0, false).map(|b| (b.source_id, b.gfx)),
            Some((idruine, cod.constants["GFXBODEN"] + 400)),
        );
        assert_eq!(
            cod.ruin_building(0, false)
                .map(|b| (b.rand_anz, b.rand_add)),
            Some((6, 1)),
        );
        assert_eq!(
            cod.ruin_building(0, true).map(|b| (b.source_id, b.gfx)),
            Some((idruine + 9, cod.constants["GFXBODEN"] + 413)),
        );
        assert_eq!(
            cod.ruin_building(4, false)
                .map(|b| (b.rand_anz, b.rand_add)),
            Some((2, 1)),
        );
        assert_eq!(
            cod.ruin_variant_building(4, false, 1).map(|b| b.gfx),
            Some(cod.constants["GFXROHST"] + 89),
        );
        assert_eq!(
            cod.ruin_building(8, false).map(|b| (b.source_id, b.gfx)),
            Some((
                cod.constants["IDHAFEN"] + 20,
                cod.constants["GFXKONTOR"] + 144
            )),
        );
        assert_eq!(
            cod.ruin_building(14, false).map(|b| (b.source_id, b.gfx)),
            Some((
                cod.constants["IDDIVERS"] + 22,
                cod.constants["GFXMARKT"] + 192
            )),
        );
        assert!(cod.ruin_building(255, false).is_none());

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
