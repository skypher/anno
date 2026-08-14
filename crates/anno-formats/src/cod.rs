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

/// Base added by the executable before looking up a SHIP4 policy-slot
/// definition in the compiled haeuser table.
pub const SOURCE_DEFINITION_ID_BASE: i32 = 0x4e20;

/// Translate one `Ware` token through the executable's registration table.
/// The values `0x19..=0x2c` are the sixteen soldier, cavalry, musketeer,
/// cannonier, and pioneer policy items tested by `FUN_00458e60`.
pub fn source_ware_slot(token: &str) -> Option<u8> {
    Some(match token {
        "NOWARE" => 0x00,
        "ALLWARE" => 0x01,
        "EISENERZ" => 0x02,
        "GOLD" => 0x03,
        "WOLLE" => 0x04,
        "ZUCKER" => 0x05,
        "TABAK" => 0x06,
        "FLEISCH" | "RIND" => 0x07,
        "KORN" => 0x08,
        "MEHL" => 0x09,
        "EISEN" => 0x0a,
        "SCHWERTER" => 0x0b,
        "MUSKETEN" => 0x0c,
        "KANONEN" => 0x0d,
        "NAHRUNG" => 0x0e,
        "TABAKWAREN" => 0x0f,
        "GEWUERZE" => 0x10,
        "KAKAO" => 0x11,
        "ALKOHOL" => 0x12,
        "STOFFE" => 0x13,
        "KLEIDUNG" => 0x14,
        "SCHMUCK" => 0x15,
        "WERKZEUG" => 0x16,
        "HOLZ" => 0x17,
        "ZIEGEL" => 0x18,
        "wSOLDAT1" => 0x19,
        "wSOLDAT2" => 0x1a,
        "wSOLDAT3" => 0x1b,
        "wSOLDAT4" => 0x1c,
        "wKAVALERIE1" => 0x1d,
        "wKAVALERIE2" => 0x1e,
        "wKAVALERIE3" => 0x1f,
        "wKAVALERIE4" => 0x20,
        "wMUSKETIER1" => 0x21,
        "wMUSKETIER2" => 0x22,
        "wMUSKETIER3" => 0x23,
        "wMUSKETIER4" => 0x24,
        "wKANONIER1" => 0x25,
        "wKANONIER2" => 0x26,
        "wKANONIER3" => 0x27,
        "wKANONIER4" => 0x28,
        "wPIONIER1" => 0x29,
        "wPIONIER2" => 0x2a,
        "wPIONIER3" => 0x2b,
        "wPIONIER4" => 0x2c,
        "GETREIDE" => 0x2d,
        "TABAKBAUM" => 0x2e,
        "GEWUERZBAUM" => 0x2f,
        "ZUCKERROHR" => 0x30,
        "BAUMWOLLE" => 0x31,
        "WEINTRAUBEN" => 0x32,
        "KAKAOBAUM" => 0x33,
        "GRAS" => 0x34,
        "BAUM" => 0x35,
        "STEINE" => 0x36,
        "ERZE" => 0x37,
        "WILD" => 0x38,
        "FISCHE" => 0x39,
        "SCHATZ" => 0x3a,
        _ => return None,
    })
}

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

impl CodFile {
    /// Resolve the low bytes written by `FUN_0045f9b2` for SHIP4 policy
    /// slots. Ordinary entries dereference a compiled haeuser definition;
    /// the executable's absent-definition fallback maps its finite special
    /// range directly to `0x19..=0x4a`.
    pub fn source_kind6_policy_ware_slots(&self, raw_slots: [u64; 8]) -> [u8; 8] {
        std::array::from_fn(|index| {
            let raw_id = raw_slots[index] as u16;
            let source_id = SOURCE_DEFINITION_ID_BASE + i32::from(raw_id);
            self.building_by_source_id(source_id)
                .and_then(BuildingDef::source_ware_slot)
                .or_else(|| {
                    (0x74cd..=0x74fe)
                        .contains(&source_id)
                        .then_some((source_id as u8).wrapping_add(0x4c))
                })
                .unwrap_or(0)
        })
    }
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
    /// Source terrain elevation compiled from `Posoffs` at runtime
    /// definition offset `0x0c`. Figure constructors multiply this integer
    /// by `0.028` before writing their Z coordinate.
    pub source_position_offset: i32,
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
    /// Source animation-frame phase (`AnimFrame`, compiled offset `0x7c`).
    /// The renderer adds this phase to an INSELHAUS command's packed frame
    /// selector before multiplying by [`Self::anim_add`].
    pub anim_frame: i32,
    /// Animation speed in milliseconds per frame (0 = use default 200ms)
    pub anim_time: i32,
    /// Source `Anicontflg` bit at compiled definition offset `0x47`, bit 0.
    /// `FUN_004638c0` uses it when a dynamic map cell changes activity.
    pub animation_continues: bool,
    /// Source `LagAniFlg` bit at compiled definition offset `0x47`, bit 2.
    /// The STADTFLD draw loop selects the storage-ratio frame branch when set.
    pub storage_animation: bool,
    /// Compiled `Maxlager` at definition offset `0x30`. The source parser
    /// stores the COD amount in its 1/32-unit map-cell scale.
    pub storage_animation_capacity: u16,
    /// Compiled `Prodmenge` at definition offset `0x24`, in 1/32-unit
    /// map-cell production scale.
    pub source_production_amount: u16,
    /// Compiled `Rohmenge` at definition offset `0x26`, in 1/32-unit
    /// map-cell storage scale.
    pub source_raw_material_amount: u16,
    /// Compiled `Workmenge` at definition offset `0x28`, in 1/32-unit
    /// map-cell storage scale.
    pub source_work_material_amount: u16,
    /// Compiled `Maxnorohst` at definition offset `0x36`. `FUN_0047daf0`
    /// compares it with a root's saturated no-raw-material counter before
    /// its production-kind-specific notification branch.
    pub source_max_no_raw_material_count: u16,
    /// Compiled `Interval` at definition offset `0x3a`. `FUN_0047daf0`
    /// reloads a root's scheduler cooldown from this authored tick count.
    pub source_scheduler_interval: u16,
    /// Compiled `Randwachs` byte at definition offset `+0x40`. The source
    /// parser converts its authored percentage into 128-scale with integer
    /// division; `FUN_004684a0` multiplies an island resource strength by
    /// this factor before selecting a raw-cell growth transition.
    pub source_resource_growth_factor: u8,
    /// Compiled definition offset `+0x64`, populated from `Maxenergy:` by
    /// `FUN_0046248d..0x004624c4` as `round(Maxenergy * 32)`. The deferred
    /// category-6 hit handler accumulates raw strength against this value.
    pub source_damage_threshold: u16,
    /// Compiled `Radius` at definition offset `+0x20`, used by
    /// `FUN_0045c8b0` to bound type-8/type-11 transfer path searches.
    pub source_transfer_radius: u8,
    /// Compiled `Figuranz` at definition offset `+0x3e`, the number of
    /// simultaneous type-11 transfer figures admitted by `FUN_0044ad50`.
    pub source_transfer_figure_limit: u8,
    /// Compiled `Kosten: <active>, <stopped>` operating costs at definition
    /// `+0x2c`/`+0x2a` (`1602_exe.c:67140-67148`). `FUN_00463140` selects by
    /// the building's activity for the city maintenance accumulator
    /// `+0x1d8`; definitions without the property (terrain, houses) cost
    /// nothing.
    pub source_operating_costs: (u16, u16),
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
            source_position_offset: 0,
            rotate: 0,
            rand_anz: 1,
            rand_add: 0,
            anim_anz: 1,
            anim_add: 0,
            anim_frame: 0,
            anim_time: 0,
            animation_continues: false,
            storage_animation: false,
            storage_animation_capacity: 0,
            source_production_amount: 0,
            source_raw_material_amount: 0,
            source_work_material_amount: 0,
            source_max_no_raw_material_count: 0,
            source_scheduler_interval: 0,
            source_resource_growth_factor: 0,
            source_damage_threshold: 0,
            source_transfer_radius: 0,
            source_transfer_figure_limit: 0,
            source_operating_costs: (0, 0),
            no_shot: false,
            ruinenr: 255,
            properties: HashMap::new(),
        }
    }
}

impl BuildingDef {
    /// Compiled byte stored at definition offset `+0x21` from the current
    /// `Ware:` property. Coefficients following the token are parsed by the
    /// executable separately and do not affect this byte.
    pub fn source_ware_slot(&self) -> Option<u8> {
        self.properties
            .get("Ware")
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .and_then(source_ware_slot)
    }

    /// Compiled `Rohstoff` selector at definition offset `+0x22`. Production
    /// kind 2 passes this byte to `FUN_0044b7e0`, whose type-12 worker selects
    /// a matching raw-resource map cell through `FUN_0046f920`.
    pub fn source_raw_resource_ware_slot(&self) -> Option<u8> {
        self.properties
            .get("Rohstoff")
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .and_then(source_ware_slot)
    }

    /// Compiled `Workstoff` selector at definition offset `+0x23`.
    /// `FUN_0047daf0` requests this ware whenever the work buffer
    /// (`+0x08`) rather than the raw buffer is the scarcer input
    /// (`1602_exe.c:90000-90010`), and `FUN_0047d940` compares an arriving
    /// delivery against it to choose which buffer to credit
    /// (`1602_exe.c:89797-89807`).
    pub fn source_work_material_ware_slot(&self) -> Option<u8> {
        self.properties
            .get("Workstoff")
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .and_then(source_ware_slot)
    }

    /// Compiled figure definition at definition offset `+0x3c`, read by every
    /// nested-production worker allocator reached from `FUN_0047daf0`'s
    /// dispatch table at `0x47e258`: `FUN_0044b7e0` (kind 2 `PLANTAGE`),
    /// `FUN_0044bb40` (kind 4 `WEIDETIER`) and `FUN_0044b9a0` (kind 6
    /// `FISCHEREI`). Kind 5 (`JAGDHAUS`) never reaches an allocator, so its
    /// `Figurnr: JAEGER` is compiled but unused; it is mapped here only so the
    /// selector space stays contiguous.
    ///
    /// The numbers are `figuren.cod` definition order. `MAEHER` opens the
    /// `Blocknr: 4` worker block at `0x60`, and `JAEGER`, `FISCHER`, `RIND`
    /// and `SCHAF` follow `PFLUECKER2` in the shipped file without a gap.
    pub fn source_plantation_worker_definition(&self) -> Option<u8> {
        match self.properties.get("Figurnr").map(String::as_str) {
            Some("MAEHER") => Some(0x60),
            Some("STEINKLOPFER") => Some(0x61),
            Some("HOLZFAELLER") => Some(0x62),
            Some("PFLUECKER") => Some(0x63),
            Some("PFLUECKER2") => Some(0x64),
            Some("JAEGER") => Some(0x65),
            Some("FISCHER") => Some(0x66),
            Some("RIND") => Some(0x67),
            Some("SCHAF") => Some(0x68),
            _ => None,
        }
    }

    /// Runtime code at compiled definition offset `+0x04`, parsed from the
    /// outer `HAUS Kind:` field. The table deliberately aliases terrain and
    /// production labels into one code space; for example `STRASSE` and
    /// `HANDWERK` both resolve to 1.
    pub fn source_kind_code(&self) -> Option<u8> {
        Self::source_kind_code_for(&self.kind)
    }

    /// Runtime code at compiled definition offset `+0x1c`, parsed from the
    /// nested `HAUS_PRODTYP Kind:` field. `FUN_00480b70` switches on this
    /// field after `FUN_00468550` has applied its outer-kind exclusions.
    pub fn source_production_kind_code(&self) -> Option<u8> {
        self.properties
            .get("ProdKind")
            .and_then(|kind| Self::source_kind_code_for(kind))
    }

    /// `Doerrflg` at compiled definition offset `+0x47`, bit 3. It marks the
    /// third record of a crop group — the withered field — and is authored on
    /// exactly seven records in the shipped haeuser.cod. `FUN_0047ca80` reads
    /// it to decide whether a completed growth phase may promote the tile, and
    /// `FUN_0047c920` uses it to select its dry-cell recovery branch.
    pub fn source_resource_is_dry(&self) -> bool {
        self.properties
            .get("Doerrflg")
            .is_some_and(|value| value.trim() == "1")
    }

    /// Growth-timer bucket that `FUN_00462d50` writes back **over** the
    /// authored `Interval` at compiled definition offset `+0x3a`, for every
    /// production-kind-10 record, right after haeuser.cod is parsed
    /// (`1602_exe.c:68880-68889`):
    ///
    /// ```text
    /// per_phase_ms = (u32)Interval * 1000 / AnimAnz     // AnimAnz = def+0x78
    /// threshold = 47000; k = 1
    /// do { if (per_phase_ms < threshold) break; threshold += 7000; k += 1 }
    /// while (threshold < 0x40741)
    /// def[0x3a] = k - 1
    /// ```
    ///
    /// which is the largest `k` with `40000 + 7000k <= per_phase_ms`,
    /// saturating at 32 — `FUN_0047c760` then clamps that to 31, so the
    /// slowest reachable clock is bucket 31 at 257 000 ms. Growth rates do
    /// differ per crop: Weizen lands in bucket 0, the 330-interval crops in
    /// bucket 3, forest and palms in bucket 20.
    ///
    /// This is deliberately *not* stored over
    /// [`Self::source_scheduler_interval`]: the map-cell scheduler
    /// `FUN_0047daf0` reads the same `+0x3a` field on every other production
    /// kind, and overwriting it would corrupt those roots' cooldowns.
    pub fn source_growth_bucket(&self) -> Option<u8> {
        if self.source_production_kind_code() != Some(10) {
            return None;
        }
        let anim_count = u32::try_from(self.anim_anz).unwrap_or(1).max(1);
        let per_phase_ms = u32::from(self.source_scheduler_interval) * 1000 / anim_count;
        let mut threshold = 47_000_u32;
        let mut steps = 1_u32;
        while per_phase_ms >= threshold && threshold < 0x4_0741 {
            threshold += 7_000;
            steps += 1;
        }
        Some((steps - 1).min(u32::from(u8::MAX)) as u8)
    }

    /// Runtime population-group byte at compiled definition offset `+0x2e`.
    /// `FUN_00478b90` copies it into byte `+0x05` of each kind-13 map-object
    /// record, where the city dispatcher uses it as the residence tier.
    pub fn source_population_group(&self) -> Option<u8> {
        self.properties
            .get("BGruppe")
            .and_then(|group| group.trim().parse().ok())
    }

    fn source_kind_code_for(kind: &str) -> Option<u8> {
        match kind {
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
        let speeds = self.source_path_speeds()?;
        Some(speeds.map(|speed| ((u16::from(speed) * 32 / 100).min(126)) as u8))
    }

    /// The authored four-entry nonzero byte `Wegspeed` table retained without
    /// the path-grid class conversion. Figure movement divides its runtime
    /// `Speed` by this selected raw value in `FUN_0044a690`.
    pub fn source_path_speeds(&self) -> Option<[u8; 4]> {
        let speeds: [u8; 4] = self
            .properties
            .get("Wegspeed")?
            .split(',')
            .map(str::trim)
            .map(str::parse::<u8>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?
            .try_into()
            .ok()?;
        speeds.into_iter().all(|speed| speed != 0).then_some(speeds)
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

    /// Write the compiled production scalars that haeuser.cod authors inside
    /// `Objekt: HAUS_PRODTYP` rather than at the top level of a `@Nummer:`
    /// block.
    ///
    /// The executable keeps one flat runtime record per definition:
    /// `FUN_00460750` stores `Maxlager` at `+0x30`, `Prodmenge` at `+0x24`,
    /// `Rohmenge` at `+0x26`, `Workmenge` at `+0x28`, `Maxnorohst` at `+0x36`,
    /// `Interval` at `+0x3a`, `Randwachs` at `+0x40` and the `Anicontflg` /
    /// `LagAniFlg` bits at `+0x47`, and its parser has no notion of these
    /// fields belonging to a nested object. Every producer in the shipped file
    /// authors them nested — the forester's `Maxlager: 10` / `Interval: 8` /
    /// `Rohmenge: 1` all sit inside its `HAUS_PRODTYP` — and the `@Nummer: 0`
    /// default block that `ObjFill: 0,MAXHAUS` copies into every slot does the
    /// same for `Rohmenge: 1`, `Prodmenge: 1`, `Maxlager: 4`, `Maxnorohst: 5`
    /// and `Randwachs: 100`.
    ///
    /// Reading them only at the top level left `source_scheduler_activity`
    /// dividing by a zero `Rohmenge` and comparing `storage_fill` against a
    /// zero `Maxlager`, which pins every producer's activity at 0.
    fn apply_nested_production_scalar(
        current: &mut BuildingDef,
        constants: &HashMap<String, i32>,
        key: &str,
        value: &str,
    ) {
        match key {
            "Maxlager" => {
                let authored = Self::eval(constants, value).clamp(0, 0x7ff);
                current.storage_animation_capacity = (authored as u16) << 5;
            }
            "Prodmenge" => {
                current.source_production_amount = Self::eval_scaled_32(constants, value);
            }
            "Rohmenge" => {
                current.source_raw_material_amount = Self::eval_scaled_32(constants, value);
            }
            "Workmenge" => {
                current.source_work_material_amount = Self::eval_scaled_32(constants, value);
            }
            "Maxnorohst" => {
                current.source_max_no_raw_material_count =
                    Self::eval(constants, value).clamp(0, i32::from(u16::MAX)) as u16;
            }
            "Interval" => {
                current.source_scheduler_interval =
                    Self::eval(constants, value).clamp(0, i32::from(u16::MAX)) as u16;
            }
            "Randwachs" => {
                let authored = Self::eval(constants, value).max(0) as u32;
                current.source_resource_growth_factor =
                    ((authored.saturating_mul(128) / 100).min(u32::from(u8::MAX))) as u8;
            }
            "Anicontflg" => {
                current.animation_continues = Self::eval(constants, value) & 1 != 0;
            }
            "LagAniFlg" => {
                current.storage_animation = Self::eval(constants, value) & 1 != 0;
            }
            _ => {}
        }
    }

    fn parse_text(text: &str) -> Result<CodFile, CodError> {
        let mut constants: HashMap<String, i32> = HashMap::new();
        // Builtin constants the executable's constant table supplies before it
        // parses haeuser.cod. The shipped file references `RADIUS_MARKT` and
        // `RADIUS_HQ` (marketplace and warehouse service radius) but never
        // defines them; the original engine seeds them from the binary. Values
        // are the same ones `anno_sim::data_bridge` recovered.
        // `1602_exe.c:66467-66468`: the executable registers both as
        // 0x10 before parsing the file (the earlier 30/22 recovery was
        // wrong; a 3×3 Kontor at radius 16 reproduces Exile's authored
        // house-coverage flags exactly).
        constants.insert("RADIUS_MARKT".to_string(), 16);
        constants.insert("RADIUS_HQ".to_string(), 16);
        let mut buildings: Vec<BuildingDef> = Vec::new();
        let mut building_by_nummer: HashMap<i32, BuildingDef> = HashMap::new();
        // `@field: +/-value` is evaluated by the executable parser against
        // the last value assigned to that field, before `ObjFill` copies are
        // applied to the next runtime record. Keep that state separate from
        // `current`, whose fields may be replaced by an ObjFill template.
        let mut directive_values: HashMap<String, i32> = HashMap::new();
        let mut current = BuildingDef::default();
        // Snapshot of the `@Nummer: 0` default block that the file closes
        // with `ObjFill: 0,MAXHAUS`. That two-argument form pre-fills every
        // definition slot `0..MAXHAUS`, so each later `@Nummer:` block is
        // authored sparsely on top of *this* record, never on top of the
        // block that happens to precede it.
        let mut template = BuildingDef::default();
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
                            if key == "Radius" {
                                current.source_transfer_radius = Self::eval(&constants, value)
                                    .clamp(0, i32::from(u8::MAX))
                                    as u8;
                            } else if key == "Kosten" {
                                // `Kosten: <active>, <stopped>` operating
                                // costs; the haeuser loader stores the pair
                                // at compiled definition `+0x2c`/`+0x2a`
                                // (`1602_exe.c:67140-67148`), read back by
                                // `FUN_00463140(def, active)` for the city
                                // maintenance accumulator `+0x1d8`.
                                let mut parts = value.split(',').map(str::trim);
                                let active = parts
                                    .next()
                                    .map(|part| Self::eval(&constants, part))
                                    .unwrap_or(0);
                                let stopped = parts
                                    .next()
                                    .map(|part| Self::eval(&constants, part))
                                    .unwrap_or(0);
                                current.source_operating_costs = (
                                    active.clamp(0, i32::from(u16::MAX)) as u16,
                                    stopped.clamp(0, i32::from(u16::MAX)) as u16,
                                );
                            } else if key == "Figuranz" {
                                current.source_transfer_figure_limit = Self::eval(&constants, value)
                                    .clamp(0, i32::from(u8::MAX))
                                    as u8;
                            }
                            Self::apply_nested_production_scalar(
                                &mut current,
                                &constants,
                                key,
                                value,
                            );
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

            // Handle ObjFill. The single-argument form `ObjFill: <Nummer>`
            // copies a previously emitted record; the two-argument form
            // `ObjFill: <first>,<count>` pre-fills a whole slot range with
            // the block just authored, which the shipped file uses exactly
            // once (`ObjFill: 0,MAXHAUS`) to install the global default
            // record every later definition starts from.
            if let Some(val_str) = line.strip_prefix("ObjFill:") {
                let mut args = val_str.split(',').map(str::trim);
                let src_expr = args.next().unwrap_or("");
                let is_range_fill = args.next().is_some();
                let src_nummer = Self::eval(&constants, src_expr);
                if let Some(source) = building_by_nummer.get(&src_nummer) {
                    let nummer = current.nummer;
                    current = source.clone();
                    current.nummer = nummer;
                    constants.insert("Nummer".to_string(), current.nummer);
                } else if is_range_fill {
                    template = current.clone();
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
                // `ObjFill: 0,MAXHAUS` pre-filled every definition slot with
                // the `@Nummer: 0` default record, so a block that does not
                // restate a property inherits it from that template and never
                // from the block that precedes it. Restart from the template
                // rather than carrying the finished record forward; a
                // following single-argument `ObjFill:` may still replace it
                // with a previously emitted definition.
                //
                // `@Nummer: +N` is a delta against the last assigned number,
                // which the template copy must not clobber.
                let previous_nummer = current.nummer;
                current = template.clone();
                current.nummer = previous_nummer;
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

            // `FUN_0046248d..0x004624c4` multiplies Maxenergy by 32 before
            // storing it at compiled building-definition offset `+0x64`.
            if let Some(val_str) = line.strip_prefix("Maxenergy:") {
                current.source_damage_threshold = Self::eval_scaled_32(&constants, val_str.trim());
                current
                    .properties
                    .insert("Maxenergy".to_string(), val_str.trim().to_string());
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

            // `FUN_00460750` stores AnimFrame at compiled definition offset
            // 0x7c. The STADTFLD draw loop combines it with the map command's
            // four-bit frame selector before applying AnimAdd.
            if let Some(val_str) = line.strip_prefix("AnimFrame:") {
                current.anim_frame = Self::eval(&constants, val_str.trim());
                continue;
            }

            // Parse AnimTime (ms per animation frame)
            if let Some(val_str) = line.strip_prefix("AnimTime:") {
                current.anim_time = Self::eval(&constants, val_str.trim());
                continue;
            }

            // `FUN_00460750` packs Anicontflg and LagAniFlg into byte 0x47
            // of the compiled definition. The map-cell frame state machine
            // reads bit 0 on activity transitions, while the draw loop reads
            // bit 2 for its storage-ratio selector branch.
            if let Some(val_str) = line.strip_prefix("Anicontflg:") {
                current.animation_continues = Self::eval(&constants, val_str.trim()) & 1 != 0;
                continue;
            }
            if let Some(val_str) = line.strip_prefix("LagAniFlg:") {
                current.storage_animation = Self::eval(&constants, val_str.trim()) & 1 != 0;
                continue;
            }

            // `FUN_00460750` stores Maxlager in map-cell units at compiled
            // definition offset 0x30: one authored unit equals 32 cells.
            if let Some(val_str) = line.strip_prefix("Maxlager:") {
                let authored = Self::eval(&constants, val_str.trim()).clamp(0, 0x7ff);
                current.storage_animation_capacity = (authored as u16) << 5;
                current
                    .properties
                    .insert("Maxlager".to_string(), authored.to_string());
                continue;
            }

            // `FUN_00460750` writes Posoffs directly to definition offset
            // 0x0c. `FUN_00451180` and the figure constructors then use that
            // field as the source map-cell terrain height.
            if let Some(val_str) = line.strip_prefix("Posoffs:") {
                current.source_position_offset = Self::eval(&constants, val_str.trim());
                current.properties.insert(
                    "Posoffs".to_string(),
                    current.source_position_offset.to_string(),
                );
                continue;
            }

            // The production parser multiplies each decimal amount by 32
            // before writing the three u16 map-cell scalars. `0.5` and
            // `0.75` therefore remain representable as 16 and 24 rather
            // than being truncated by the simulation's integer good units.
            if let Some(val_str) = line.strip_prefix("Prodmenge:") {
                current.source_production_amount = Self::eval_scaled_32(&constants, val_str);
                current.properties.insert(
                    "Prodmenge".to_string(),
                    (current.source_production_amount as f64 / 32.0).to_string(),
                );
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Rohmenge:") {
                current.source_raw_material_amount = Self::eval_scaled_32(&constants, val_str);
                current.properties.insert(
                    "Rohmenge".to_string(),
                    (current.source_raw_material_amount as f64 / 32.0).to_string(),
                );
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Workmenge:") {
                current.source_work_material_amount = Self::eval_scaled_32(&constants, val_str);
                current.properties.insert(
                    "Workmenge".to_string(),
                    (current.source_work_material_amount as f64 / 32.0).to_string(),
                );
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Maxnorohst:") {
                current.source_max_no_raw_material_count =
                    Self::eval(&constants, val_str.trim()).clamp(0, i32::from(u16::MAX)) as u16;
                current.properties.insert(
                    "Maxnorohst".to_string(),
                    current.source_max_no_raw_material_count.to_string(),
                );
                continue;
            }
            if let Some(val_str) = line.strip_prefix("Interval:") {
                current.source_scheduler_interval =
                    Self::eval(&constants, val_str.trim()).clamp(0, i32::from(u16::MAX)) as u16;
                current.properties.insert(
                    "Interval".to_string(),
                    current.source_scheduler_interval.to_string(),
                );
                continue;
            }
            // `FUN_0046124d..0x0046128c` stores Randwachs as an unsigned
            // 128-scale byte at compiled definition offset `+0x40`.
            if let Some(val_str) = line.strip_prefix("Randwachs:") {
                let authored = Self::eval(&constants, val_str.trim()).max(0) as u32;
                current.source_resource_growth_factor =
                    ((authored.saturating_mul(128) / 100).min(u32::from(u8::MAX))) as u8;
                current.properties.insert(
                    "Randwachs".to_string(),
                    current.source_resource_growth_factor.to_string(),
                );
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

    /// Evaluate a COD decimal in the source map-cell's 1/32-unit scale.
    fn eval_scaled_32(constants: &HashMap<String, i32>, expr: &str) -> u16 {
        let expr = expr.trim();
        let value = if let Ok(value) = expr.parse::<f64>() {
            value
        } else {
            Self::eval(constants, expr) as f64
        };
        (value * 32.0).round().clamp(0.0, f64::from(u16::MAX)) as u16
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

    /// Resolve one compiled definition **slot**.
    ///
    /// The runtime table is 0x88-byte records laid out in `@Nummer` order, so
    /// the `def ± 0x88` steps taken by `FUN_0047c830` (harvest), `FUN_0047ca80`
    /// (the growth sweep, via `def+0x10c` = next record's `Gfx`) and
    /// `FUN_0047c920` (drought) are exactly `@Nummer ± 1`. A `Gfx ± 1` step is
    /// **not** the same thing: adjacent records' `Gfx` values are not adjacent
    /// — a Weizen group runs `GFXROHST+0`, `+5`, `+48`, because each record
    /// reserves `AnimAnz` consecutive sprite slots.
    ///
    /// The parser emits the `@Nummer: 0` default block and the real `@Nummer:
    /// 0` terrain record under the same number; the compiled table keeps only
    /// the last writer of a slot, so this searches from the end.
    pub fn building_index_by_nummer(&self, nummer: i32) -> Option<usize> {
        self.buildings.iter().rposition(|b| b.nummer == nummer)
    }

    /// [`Self::building_index_by_nummer`] resolved to the definition itself.
    pub fn building_by_nummer(&self, nummer: i32) -> Option<&BuildingDef> {
        self.building_index_by_nummer(nummer)
            .and_then(|index| self.buildings.get(index))
    }

    fn building_index_by_source_id(&self, source_id: i32) -> Option<usize> {
        self.buildings.iter().position(|b| b.source_id == source_id)
    }

    /// Resolve the first compiled `HAUS_PRODTYP Kind: WOHNUNG` definition
    /// for one BGruppe. The haeuser loader stores this first matching record
    /// at `DAT_0061fa84 + BGruppe * 0x48`; `FUN_0047bbc0` and
    /// `FUN_0047c080` use the same table for residential tier transitions.
    pub fn source_population_group_building(&self, group: u8) -> Option<&BuildingDef> {
        self.buildings.iter().find(|building| {
            building.source_production_kind_code() == Some(13)
                && building.source_population_group() == Some(group)
        })
    }

    /// Resolve the concrete BGruppe housing variant selected from a source
    /// `rand()` draw. The transition paths use the base definition's
    /// `RandAnz` and `RandAdd` with the same contiguous compiled-record
    /// layout as the terminal ruin selector.
    pub fn source_population_group_variant(
        &self,
        group: u8,
        rand_value: u16,
    ) -> Option<&BuildingDef> {
        let base_index = self.buildings.iter().position(|building| {
            building.source_production_kind_code() == Some(13)
                && building.source_population_group() == Some(group)
        })?;
        let base = self.buildings.get(base_index)?;
        let variant_count = usize::from(base.rand_anz.max(1));
        let variant = (usize::from(rand_value) % variant_count) * usize::from(base.rand_add);
        self.buildings.get(base_index + variant).or(Some(base))
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
    fn source_path_speeds_require_four_nonzero_bytes() {
        let mut building = BuildingDef::default();
        building
            .properties
            .insert("Wegspeed".into(), "100,120,170,100".into());
        assert_eq!(building.source_path_speeds(), Some([100, 120, 170, 100]));

        building
            .properties
            .insert("Wegspeed".into(), "100,0,170,100".into());
        assert_eq!(building.source_path_speeds(), None);
    }

    #[test]
    fn parses_anim_frame_for_source_sprite_selection() {
        let cod = CodFile::parse(
            b"@Nummer: 1\nId: 20001\nGfx: 120\nAnimAnz: 4\nAnimAdd: 6\nAnimFrame: 3\n",
        )
        .expect("parse plaintext COD");

        assert_eq!(cod.buildings.len(), 1);
        assert_eq!(cod.buildings[0].anim_anz, 4);
        assert_eq!(cod.buildings[0].anim_add, 6);
        assert_eq!(cod.buildings[0].anim_frame, 3);
    }

    #[test]
    fn parses_source_cell_animation_flags() {
        let cod = CodFile::parse(
            b"HIGHBODEN = 20\n@Nummer: 1\nId: 20001\nPosoffs: HIGHBODEN\nAnicontflg: 1\nLagAniFlg: 1\nMaxlager: 5\nProdmenge: 0.75\nRohmenge: 0.5\nWorkmenge: 2\nMaxnorohst: 9\nInterval: 7\nRandwachs: 75\n",
        )
        .expect("parse plaintext COD");

        assert!(cod.buildings[0].animation_continues);
        assert!(cod.buildings[0].storage_animation);
        assert_eq!(cod.buildings[0].source_position_offset, 20);
        assert_eq!(cod.buildings[0].storage_animation_capacity, 160);
        assert_eq!(cod.buildings[0].source_production_amount, 24);
        assert_eq!(cod.buildings[0].source_raw_material_amount, 16);
        assert_eq!(cod.buildings[0].source_work_material_amount, 64);
        assert_eq!(cod.buildings[0].source_max_no_raw_material_count, 9);
        assert_eq!(cod.buildings[0].source_scheduler_interval, 7);
        assert_eq!(cod.buildings[0].source_resource_growth_factor, 96);
    }

    #[test]
    fn plantation_raw_ware_and_worker_definition_follow_haus_prodtyp() {
        let cod = CodFile::parse(
            b"@Nummer: 1\nKind: GEBAEUDE\nObjekt: HAUS_PRODTYP\nKind: PLANTAGE\nRohstoff: GETREIDE\nFigurnr: MAEHER\nEndObj;\n",
        )
        .expect("parse plantation COD");

        assert_eq!(cod.buildings[0].source_raw_resource_ware_slot(), Some(0x2d));
        assert_eq!(
            cod.buildings[0].source_plantation_worker_definition(),
            Some(0x60)
        );
    }

    #[test]
    fn maxenergy_compiles_to_the_source_damage_threshold() {
        let cod = CodFile::parse(b"@Nummer: 1\nMaxenergy: 50\n").expect("parse plaintext COD");

        assert_eq!(cod.buildings[0].source_damage_threshold, 1_600);
    }

    #[test]
    fn parses_symbolic_type11_radius_and_figure_count() {
        let cod = CodFile::parse(
            b"RADIUS_MARKT = 16\n@Nummer: 1\nKind: GEBAEUDE\nObjekt: HAUS_PRODTYP\nKind: MARKT\nRadius: RADIUS_MARKT\nFiguranz: 2\nEndObj;\n",
        )
        .expect("parse plaintext COD");

        assert_eq!(cod.buildings[0].source_transfer_radius, 16);
        assert_eq!(cod.buildings[0].source_transfer_figure_limit, 2);
    }

    #[test]
    fn parses_population_group_for_kind_thirteen_runtime_records() {
        let cod = CodFile::parse(
            b"@Nummer: 1\nObjekt: HAUS_PRODTYP\nKind: WOHNUNG\nBGruppe: 3\nEndObj;\n",
        )
        .expect("parse plaintext COD");

        assert_eq!(cod.buildings[0].source_population_group(), Some(3));
    }

    #[test]
    fn population_group_housing_resolver_uses_first_nested_wohnung_and_variant_stride() {
        let cod = CodFile::parse(
            b"@Nummer: 1\nId: 21001\nRandAnz: 3\nRandAdd: 1\nObjekt: HAUS_PRODTYP\nKind: WOHNUNG\nBGruppe: 1\nEndObj;\n@Nummer: 2\nId: 21002\nObjekt: HAUS_PRODTYP\nKind: WOHNUNG\nBGruppe: 1\nEndObj;\n@Nummer: 3\nId: 21003\nObjekt: HAUS_PRODTYP\nKind: WOHNUNG\nBGruppe: 1\nEndObj;\n@Nummer: 4\nId: 21004\nObjekt: HAUS_PRODTYP\nKind: WOHNUNG\nBGruppe: 2\nEndObj;\n",
        )
        .expect("parse plaintext COD");

        assert_eq!(
            cod.source_population_group_building(1)
                .map(|building| building.source_id),
            Some(21001)
        );
        assert_eq!(
            cod.source_population_group_variant(1, 4)
                .map(|building| building.source_id),
            Some(21002)
        );
        assert_eq!(
            cod.source_population_group_variant(2, 0)
                .map(|building| building.source_id),
            Some(21004)
        );
    }

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
        assert_eq!(cod.buildings[1].source_position_offset, 20);
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
        assert!(
            cod.buildings
                .iter()
                .all(|building| building.anim_frame == 0),
            "the shipped haeuser.cod corpus keeps AnimFrame at its inherited zero phase"
        );
        // Superseded expectation: "the shipped haeuser.cod corpus declares no
        // LagAniFlg definitions" (`all(|b| !b.storage_animation)`). It only
        // held because `LagAniFlg` is authored inside `Objekt: HAUS_PRODTYP`
        // and the parser read the key at the top level only. The file does
        // declare it, on the Steinbruch (`Id: 20513`) and the forester
        // (`Id: 22001`).
        assert_eq!(
            cod.buildings
                .iter()
                .filter(|building| building.storage_animation)
                .count(),
            2,
            "LagAniFlg is authored inside Objekt: HAUS_PRODTYP on two definitions"
        );
        // The compiled production scalars live in the same nested object. The
        // forester authors `Maxlager: 10` / `Interval: 8` / `Rohmenge: 1` and
        // inherits `Prodmenge: 1` from the `@Nummer: 0` default block.
        let forester = cod
            .buildings
            .iter()
            .find(|building| building.properties.get("Id").is_some_and(|id| id == "22001"))
            .expect("forester source definition");
        assert_eq!(forester.storage_animation_capacity, 320);
        assert_eq!(forester.source_scheduler_interval, 8);
        assert_eq!(forester.source_raw_material_amount, 32);
        assert_eq!(forester.source_production_amount, 32);
        assert_eq!(forester.source_max_no_raw_material_count, 8);
        // The iron smelter is the stock two-input producer: `Rohstoff:
        // EISENERZ` with `Workstoff: HOLZ` and a nested `Workmenge: 1`.
        let smelter = cod
            .buildings
            .iter()
            .find(|building| building.properties.get("Id").is_some_and(|id| id == "20521"))
            .expect("iron smelter source definition");
        assert_eq!(smelter.source_work_material_amount, 32);
        assert_eq!(smelter.source_raw_resource_ware_slot(), Some(0x02));
        assert_eq!(smelter.source_work_material_ware_slot(), Some(0x17));
        assert!(
            cod.buildings.iter().any(|building| building
                .source_production_kind_code()
                .is_some_and(|kind| matches!(kind, 1..=8))
                && building.source_raw_material_amount != 0),
            "producers keep the nested Rohmenge the source scheduler divides by"
        );
        let market = cod
            .buildings
            .iter()
            .find(|building| {
                building
                    .properties
                    .get("ProdKind")
                    .is_some_and(|kind| kind == "MARKT")
            })
            .expect("MARKT source definition");
        assert_eq!(market.source_transfer_figure_limit, 2);
        assert!(market.source_transfer_radius > 0);

        // Should have ~500 buildings
        assert!(
            cod.buildings.len() >= 490,
            "expected ~500 buildings, got {}",
            cod.buildings.len()
        );

        // `Nahrung: 1.3` is a file-level GAMEEINSTELLUNGEN entry authored
        // before `Objekt: HAUS` opens, and `Steuer:` belongs to the five
        // `Objekt: BGRUPPE` records. Neither is a HAUS property. They used
        // to surface on every building definition only because the parser
        // carried one accumulating record from the file preamble through
        // every `@Nummer:` block; the `ObjFill: 0,MAXHAUS` template restart
        // drops them. Keep that as a tripwire against the leak returning.
        assert!(
            cod.buildings.iter().all(|building| {
                !building.properties.contains_key("Nahrung")
                    && !building.properties.contains_key("Steuer")
            }),
            "preamble/BGRUPPE properties must not leak onto HAUS definitions"
        );

        let ruin_cases = [
            (270, 8),   // RUINE_KONTOR_1
            (271, 9),   // ObjFill: BASE, then @Ruinenr: +1
            (272, 10),  // next @Ruinenr: +1 directive value
            (273, 11),  // next @Ruinenr: +1 directive value
            (274, 0),   // RUINE_HOLZ
            (275, 0),   // RUINE_HOLZ
            (276, 2),   // RUINE_STEIN
            (277, 2),   // RUINE_STEIN
            // Nr 356 (a beach wall) is the last block to author
            // `Ruinenr: NORUINE`. The stone gates and stone watchtower that
            // follow restate no `Ruinenr`, so they take the template's
            // `RUINE_STEIN`; the previous 255 here was that beach wall's
            // value leaking three definitions forward.
            (357, 2), // stone gate, template RUINE_STEIN
            (358, 2), // stone gate, template RUINE_STEIN
            (359, 2), // stone watchtower, template RUINE_STEIN
            (356, 255), // the authored NORUINE beach wall itself
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

    /// `ObjFill: 0,MAXHAUS` pre-fills every definition slot with the
    /// `@Nummer: 0` default record, so a sparse block inherits unrestated
    /// properties from that template — not from the block above it.
    #[test]
    fn definitions_restart_from_the_objfill_template_not_the_previous_block() {
        // Two consecutive blocks: the first is fully specified, the second
        // restates only `Holz`. Everything else must fall back to the
        // template (which itself declares no HAUS_BAUKOST at all), so the
        // second block must not inherit the first block's Money/Werkzeug/
        // Ziegel/Kanon, nor its HAUS_PRODTYP output good.
        let cod = CodFile::parse(
            b"MAXHAUS = 500\nObjekt: HAUS\n\
              @Nummer: 0\nId: 0\nKind: UNUSED\nSize: 1, 1\n\
              Objekt: HAUS_PRODTYP\nKind: UNUSED\nWare: NOWARE\nEndObj;\n\
              ObjFill: 0,MAXHAUS\n\
              @Nummer: +1\nId: 20001\nKind: GEBAEUDE\nSize: 2, 2\n\
              Objekt: HAUS_PRODTYP\nKind: WEIDETIER\nWare: WOLLE\nKosten: 25, 5\nEndObj;\n\
              Objekt: HAUS_BAUKOST\nWerkzeug: 2\nHolz: 4\nZiegel: 7\nKanon: 1\nMoney: 200\nEndObj;\n\
              @Nummer: +1\nId: 20002\nKind: GEBAEUDE\n\
              Objekt: HAUS_BAUKOST\nHolz: 3\nEndObj;\n",
        )
        .expect("parse plaintext COD");

        let costs = |building: &BuildingDef| {
            ["Money", "Holz", "Werkzeug", "Ziegel", "Kanon"]
                .map(|key| building.properties.get(key).cloned())
        };
        let farm = cod.building_by_source_id(20001).expect("farm definition");
        let hut = cod.building_by_source_id(20002).expect("hut definition");

        assert_eq!(
            costs(farm),
            [
                Some("200".into()),
                Some("4".into()),
                Some("2".into()),
                Some("7".into()),
                Some("1".into()),
            ]
        );
        // Only `Holz` is authored on the second block.
        assert_eq!(costs(hut), [None, Some("3".into()), None, None, None]);
        // ...and the rest of the record falls back to the template too.
        assert_eq!(hut.properties.get("Ware").map(String::as_str), Some("NOWARE"));
        assert_eq!(
            hut.properties.get("ProdKind").map(String::as_str),
            Some("UNUSED")
        );
        assert_eq!(hut.source_operating_costs, (0, 0));
        assert_eq!(hut.size, (1, 1));
        assert_eq!(farm.source_operating_costs, (25, 5));
    }

    /// The shipped resource groups, walked through the 0x88-byte record
    /// stride. Each `ROHSTOFF` record's `@Nummer - 1` is its `ROHSTWACHS`
    /// growth record; the seven crops add a `Doerrflg` record at `+1`.
    /// `FUN_00462d50` compiles each growth record's `Interval` / `AnimAnz`
    /// into a bucket index (`1602_exe.c:68880-68889`).
    #[test]
    fn shipped_resource_groups_resolve_through_the_record_stride() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/haeuser.cod");
        let Ok(data) = std::fs::read(&path) else {
            println!("Skipping test: data corpus not found");
            return;
        };
        let cod = CodFile::parse(&data).expect("parse haeuser.cod");

        // `Id: IDROHST+1` is ripe Weizen. Its group is `GFXROHST+0` / `+5` /
        // `+48`, i.e. `Gfx` gaps of -5 and +43 from the ripe record — a port
        // that stepped `Gfx ± 1` would land on another crop's sprites.
        let weizen = cod
            .building_by_source_id(cod.constants["IDROHST"] + 1)
            .expect("ripe Weizen");
        assert_eq!(weizen.kind, "WALD");
        assert_eq!(weizen.source_kind_code(), Some(10));
        assert_eq!(weizen.source_production_kind_code(), Some(9));
        assert_eq!(weizen.source_ware_slot(), Some(0x2d));
        let growth = cod
            .building_by_nummer(weizen.nummer - 1)
            .expect("Weizen growth record");
        let dry = cod
            .building_by_nummer(weizen.nummer + 1)
            .expect("Weizen withered record");
        assert_eq!(growth.source_production_kind_code(), Some(10));
        assert_eq!(dry.source_production_kind_code(), Some(10));
        assert!(!growth.source_resource_is_dry());
        assert!(dry.source_resource_is_dry());
        assert_eq!(weizen.gfx - growth.gfx, 5);
        assert_eq!(dry.gfx - weizen.gfx, 43);
        assert_eq!(growth.anim_anz, 5);
        assert_eq!(growth.anim_time, 0, "AnimTime: TIMENEVER is registered as 0");
        assert_eq!(growth.source_scheduler_interval, 175);
        assert_eq!(growth.source_growth_bucket(), Some(0));
        // The ripe record keeps its own `+0x3a`; only kind 10 is rewritten.
        assert_eq!(weizen.source_growth_bucket(), None);

        // Forest: `Interval: 900` over `AnimAnz: 5` is 180 000 ms per phase,
        // the largest `40000 + 7000k` that fits, k = 20.
        let forest_growth = cod
            .building_by_source_id(cod.constants["IDROHST"] + 2)
            .expect("Wald growth record");
        assert_eq!(forest_growth.source_production_kind_code(), Some(10));
        assert_eq!(forest_growth.source_scheduler_interval, 900);
        assert_eq!(forest_growth.anim_anz, 5);
        assert_eq!(forest_growth.source_growth_bucket(), Some(20));
        let forest_ripe = cod
            .building_by_nummer(forest_growth.nummer + 1)
            .expect("ripe forest");
        assert_eq!(forest_ripe.source_ware_slot(), Some(0x35));
        assert_eq!(forest_ripe.kind, "WALD");
        // Forest is a pair: the record after the ripe one is the next
        // variant's growth record, not a withered field.
        assert!(!cod
            .building_by_nummer(forest_ripe.nummer + 1)
            .expect("next record")
            .source_resource_is_dry());

        // Pasture ships ripe as outer `Kind: BODEN`, which is why the harvest
        // guard had to move off the outer map kind.
        let pasture_growth = cod
            .building_by_source_id(cod.constants["IDWEIDE"])
            .expect("IDWEIDE growth record");
        assert_eq!(pasture_growth.kind, "BODEN");
        assert_eq!(pasture_growth.source_scheduler_interval, 300);
        assert_eq!(pasture_growth.anim_anz, 1);
        // 300 000 ms per phase saturates the loop; `FUN_0047c760` clamps 32
        // to 31, the 257 000 ms clock.
        assert_eq!(pasture_growth.source_growth_bucket(), Some(32));
        let pasture_ripe = cod
            .building_by_nummer(pasture_growth.nummer + 1)
            .expect("ripe pasture");
        assert_eq!(pasture_ripe.kind, "BODEN");
        assert_eq!(pasture_ripe.source_kind_code(), Some(11));
        assert_eq!(pasture_ripe.source_production_kind_code(), Some(9));
        assert_eq!(pasture_ripe.source_ware_slot(), Some(0x34));

        // Exactly seven `Doerrflg: 1` records, all of them crops.
        assert_eq!(
            cod.buildings
                .iter()
                .filter(|building| building.source_resource_is_dry())
                .count(),
            7
        );
    }

    /// Shipped-corpus counterpart of the test above: `haeuser.cod` authors
    /// the pioneer hut's HAUS_BAUKOST as `Holz: 3` alone, immediately after a
    /// sheep farm that costs 200 gold / 2 tools / 4 wood. Before the template
    /// restart the hut reported the farm's 200 gold.
    #[test]
    fn shipped_build_costs_are_the_authored_ones() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/haeuser.cod");
        let Ok(data) = std::fs::read(&path) else {
            println!("Skipping test: {path:?} not found");
            return;
        };
        let cod = CodFile::parse(&data).expect("failed to parse COD");
        let costs = |building: &BuildingDef| {
            ["Money", "Holz", "Werkzeug", "Ziegel", "Kanon"].map(|key| {
                building
                    .properties
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0)
            })
        };

        // BGruppe 0 residence (Wohnhäuser I, Nr 413): `Holz: 3` only.
        let pioneer = cod
            .source_population_group_building(0)
            .expect("BGruppe 0 residence");
        assert_eq!(pioneer.nummer, 413);
        assert_eq!(costs(pioneer), [0, 3, 0, 0, 0]);
        // The preceding sheep farm, whose costs used to leak into it.
        let sheep_farm = cod
            .building_by_source_id(cod.constants["IDFARM"] + 14)
            .expect("sheep farm");
        assert_eq!(costs(sheep_farm), [200, 4, 2, 0, 0]);

        // Marketplace (Nr 467): no `Ziegel` line; the chapel-tier building
        // above it costs 19 bricks and used to donate them.
        let market = cod
            .buildings
            .iter()
            .find(|building| {
                building
                    .properties
                    .get("ProdKind")
                    .is_some_and(|kind| kind == "MARKT")
            })
            .expect("MARKT source definition");
        assert_eq!(costs(market), [200, 10, 4, 0, 0]);

        // Definitions with no HAUS_BAUKOST block at all are free: the
        // template declares none. Both ruins below cost nothing.
        let wohn_ruin = cod
            .building_by_source_id(cod.constants["IDWOHN"])
            .expect("residence ruin");
        assert_eq!(costs(wohn_ruin), [0, 0, 0, 0, 0]);
        let market_ruin = cod
            .building_by_source_id(cod.constants["IDDIVERS"] + 22)
            .expect("destroyed marketplace");
        assert_eq!(costs(market_ruin), [0, 0, 0, 0, 0]);

        // `ObjFill: <Nummer>` still copies a previously emitted record, so
        // the six random variants after the pioneer hut keep its 3 wood.
        for offset in 1..=6 {
            let variant = cod
                .building_by_source_id(pioneer.source_id + offset)
                .unwrap_or_else(|| panic!("pioneer variant +{offset}"));
            assert_eq!(costs(variant), [0, 3, 0, 0, 0], "variant +{offset}");
        }

        // Residences produce nothing; the leak used to give the pioneer hut
        // the sheep farm's WOLLE output and a WORKSTOFF input.
        assert_eq!(pioneer.properties.get("Ware").map(String::as_str), Some("NOWARE"));
        assert!(!pioneer.properties.contains_key("Workstoff"));
        // The palm-forest terrain tiles (Nr 488 authors no HAUS_BAUKOST,
        // Nr 489..=498 `ObjFill:` it) are free terrain; they used to charge
        // the 50 bricks of the last building above them in the file.
        for nummer in 488..=498 {
            let palm = cod
                .buildings
                .iter()
                .find(|building| building.nummer == nummer)
                .unwrap_or_else(|| panic!("palm forest Nr={nummer}"));
            assert_eq!(palm.kind, "WALD");
            assert_eq!(costs(palm), [0, 0, 0, 0, 0], "Nr={nummer}");
        }
        // ...while the grass/forest tiles that *do* author `Money: 5`
        // (Nr 81 `HAUSWACHS`) keep it, as do the definitions filled from it.
        let grass = cod
            .buildings
            .iter()
            .find(|building| building.nummer == 81)
            .expect("Nr 81");
        assert_eq!(grass.kind, "WALD");
        assert_eq!(costs(grass), [5, 0, 0, 0, 0]);
    }

    #[test]
    fn source_kind6_policy_slots_follow_definition_and_special_fallback() {
        let soldier_def = BuildingDef {
            source_id: SOURCE_DEFINITION_ID_BASE + 7,
            properties: HashMap::from([("Ware".into(), "wMUSKETIER3, 0.5".into())]),
            ..Default::default()
        };
        let cod = CodFile {
            constants: HashMap::new(),
            buildings: vec![soldier_def],
        };

        assert_eq!(source_ware_slot("RIND"), Some(0x07));
        assert_eq!(source_ware_slot("wPIONIER4"), Some(0x2c));
        assert_eq!(
            cod.source_kind6_policy_ware_slots([7, 0x26ad, 0xffff, 0, 0, 0, 0, 0]),
            [0x23, 0x19, 0, 0, 0, 0, 0, 0,]
        );
    }
}
