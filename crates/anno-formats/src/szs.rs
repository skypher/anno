//! SZS/SZM save file and scenario parser.
//!
//! Anno 1602 save files use a chunk-based binary format:
//!   - Each chunk: 16-byte name (null-padded) + 4-byte LE size + data
//!   - Islands are stored as INSEL5 (metadata) + INSELHAUS (tile records) pairs
//!   - INSELHAUS records are 8 bytes each: building_id(u16) + x(u8) + y(u8) + 4 bytes flags

use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SzsError {
    #[error("file too small")]
    TooSmall,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A raw chunk from the save file.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub name: String,
    pub data: Vec<u8>,
}

/// One of the seven climate-dependent crop fertilities the
/// engine recognises. Values match the order of the
/// `[ROHST]` section in `editor.cod` and are echoed by the
/// `[ROHSTFELD]` (raw-resource field) section:
///
///   0 = Grain      (KORN)
///   1 = Tobacco    (TABAK)
///   2 = Spices     (GEWUERZE)
///   3 = Sugarcane  (ZUCKER / ZUCKERROHR)
///   4 = Cotton     (BAUMWOLLE)
///   5 = Vines      (WEIN)
///   6 = Cocoa      (KAKAO)
///
/// 7 is the sentinel "grazing land / no special crop"
/// (matches editor.cod's "Grazing land" entry at the same
/// position). 93% of fertility slots in shipping content
/// carry 7, leaving 0..=6 to mark which one or two specific
/// crops a fertile island supports.
///
/// `[ROHSTFELD]` extends this ladder past 7 with non-crop
/// resource markers — 8 = Forest, 9 = Stones, 10 = Ore,
/// 11 = Wild game, 12 = Fishing grounds — but no shipping
/// `.szs` carries a fertility byte > 7, so we treat them as
/// the sentinel here and leave them for future RE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Fertility {
    Grain = 0,
    Tobacco = 1,
    Spices = 2,
    Sugarcane = 3,
    Cotton = 4,
    Vines = 5,
    Cocoa = 6,
}

impl Fertility {
    /// Decode a raw INSEL5 fertility byte. Returns `None` for
    /// the sentinel value 7 (grazing land / no crop) and any
    /// out-of-range value.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Fertility::Grain),
            1 => Some(Fertility::Tobacco),
            2 => Some(Fertility::Spices),
            3 => Some(Fertility::Sugarcane),
            4 => Some(Fertility::Cotton),
            5 => Some(Fertility::Vines),
            6 => Some(Fertility::Cocoa),
            _ => None,
        }
    }
}

/// Island metadata from an INSEL5 chunk.
#[derive(Debug, Clone)]
pub struct Island {
    pub number: u8,
    pub width: u8,
    pub height: u8,
    pub x_pos: u16,
    pub y_pos: u16,
    /// Eight fertility bytes at INSEL5 offsets 0x0C..0x14.
    /// The mapping is pinned by the `[ROHST]` section of
    /// `editor.cod`: 0=Grain, 1=Tobacco, 2=Spices,
    /// 3=Sugarcane, 4=Cotton, 5=Vines, 6=Cocoa, 7=Grazing
    /// land (sentinel "no specific crop here").
    ///
    /// 93% of bytes are 7 across 546 islands; most islands
    /// fill one or two slots with 0..=6 to flag specific
    /// fertilities. Use `Fertility::from_byte` to decode each
    /// entry into the typed enum (returning `None` for the
    /// sentinel).
    pub fertilities: [u8; 8],
    pub tiles: Vec<IslandTile>,
    /// Optional city info from the matching `STADT4` chunk that
    /// follows this island's INSELHAUS in chunk order. Populated
    /// only when the island carries a settled town.
    pub city: Option<City>,
}

/// The 8-byte `INSEL5` resource record consumed by `FUN_0046aff0`.
///
/// The source only reads byte 0 as the ware selector, byte 4 as the
/// availability state, and bytes 6..8 as a remaining-amount threshold for
/// `FUN_0046b0a0`; retaining the complete record keeps the scenario data
/// available to source-backed callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IslandSourceResourceRecord {
    pub raw: [u8; 8],
}

impl IslandSourceResourceRecord {
    pub const fn ware(self) -> u8 {
        self.raw[0]
    }

    pub const fn availability_state(self) -> u8 {
        self.raw[4]
    }

    pub const fn remaining_amount(self) -> u16 {
        u16::from_le_bytes([self.raw[6], self.raw[7]])
    }
}

/// Runtime island-resource inputs serialized in an `INSEL5` record.
/// `FUN_0046aff0` converts these fields into the 0/64/128 source-resource
/// strength used by raw-cell replacement, while `FUN_004684a0` also consumes
/// `attenuation` after applying a cell's compiled `Randwachs` factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IslandSourceResourceState {
    /// `INSEL5[0x1A]`, capped by the eight serialized slots.
    pub record_count: u8,
    /// `INSEL5[0x1C..0x5C]`, eight records with stride eight.
    pub records: [IslandSourceResourceRecord; 8],
    /// `INSEL5[0x5C..0x60]`, the crop-resource bitmask. The loader
    /// (`0x00469f96..0x0046a001`) reloads this word into runtime `+0x5c` with
    /// `|= 0x1181` forced on, so wares `0x2d`/`0x34`/`0x35`/`0x39` (grain and
    /// the attenuation-exempt grass/tree/fish) always read available; the OR
    /// is applied by [`resource_strength`], not stored here.
    pub crop_flags: u32,
    /// `INSEL5[0x64]`, the season/parity byte the loader copies to runtime
    /// `+0x1c`. `FUN_0046aff0` compares it against 0 and 1 to pick which crop
    /// triple is fertile; `FUN_0046b3e0` reads the same byte for the
    /// `FRAU`/`ADEL` terrain-event definition.
    pub parity: u8,
    /// `INSEL5[0x6c..0x70]`, copied to runtime `+0x60` and compared with
    /// `DAT_005b6040` by `FUN_0046b3e0` to choose its raw-to-dry scan before
    /// the source begins the attenuation-decay and dry-to-raw branch.
    pub transition_deadline_ticks: u32,
    /// `INSEL5[0x66]`, the mutable factor subtracted by `FUN_004684a0` for
    /// non-grass, non-tree, and non-fish resources.
    pub attenuation: u8,
}

impl IslandSourceResourceState {
    /// Exact 0/64/128 result of `FUN_0046aff0` for one raw ware.
    pub fn resource_strength(self, ware: u8) -> u8 {
        if !(0x2d..=0x3a).contains(&ware) {
            let mut partial_strength = 0;
            for record in self
                .records
                .iter()
                .take(usize::from(self.record_count).min(8))
            {
                if record.ware() != ware {
                    continue;
                }
                match record.availability_state() {
                    0 => return 0x80,
                    1 => partial_strength = 0x40,
                    _ => {}
                }
            }
            return partial_strength;
        }

        // The loader forces `0x1181` on before `FUN_0046aff0` reads the mask.
        if (self.crop_flags | 0x1181) & (1_u32 << u32::from(ware - 0x2d)) != 0 {
            return 0x80;
        }
        if self.parity == 0 {
            return if matches!(ware, 0x2e | 0x30 | 0x32) {
                0x40
            } else {
                0
            };
        }
        if self.parity == 1 && matches!(ware, 0x2f | 0x31 | 0x33) {
            0x40
        } else {
            0
        }
    }
}

impl Island {
    /// Active (non-sentinel) fertilities decoded into the
    /// typed enum. Yields at most 8 entries; preserves the
    /// slot order so callers can correlate with the binary's
    /// internal indexing.
    pub fn active_fertilities(&self) -> Vec<Fertility> {
        self.fertilities
            .iter()
            .filter_map(|&b| Fertility::from_byte(b))
            .collect()
    }
}

/// One of the five ship types `SHIP4` records track. The values are source
/// executable definition IDs: the pointer table registered by
/// `FUN_00441210` at `0x00498b98` maps 0x15/0x17/0x19/0x1B/0x1F to
/// HANDEL1/HANDEL2/KRIEG1/KRIEG2/PIRAT, respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ShipClass {
    /// HANDEL1 — small trade ship (raw byte 0x15).
    SmallTrader = 0x15,
    /// HANDEL2 — large trade ship (0x17).
    LargeTrader = 0x17,
    /// KRIEG1 — small warship (0x19).
    SmallWarship = 0x19,
    /// KRIEG2 — large warship (0x1B).
    LargeWarship = 0x1B,
    /// PIRAT — pirate ship (0x1F).
    PirateShip = 0x1F,
}

impl ShipClass {
    /// Decode a raw ship-class byte. Returns `None` for any
    /// value not in the observed shipping-corpus set
    /// `{0x15, 0x17, 0x19, 0x1B, 0x1F}`.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x15 => Some(ShipClass::SmallTrader),
            0x17 => Some(ShipClass::LargeTrader),
            0x19 => Some(ShipClass::SmallWarship),
            0x1B => Some(ShipClass::LargeWarship),
            0x1F => Some(ShipClass::PirateShip),
            _ => None,
        }
    }

    /// Symbolic `figuren.cod` definition named by the source executable's
    /// compiled definition table for this SHIP4 class.
    pub fn source_figure_name(self) -> &'static str {
        match self {
            ShipClass::SmallTrader => "HANDEL1",
            ShipClass::LargeTrader => "HANDEL2",
            ShipClass::SmallWarship => "KRIEG1",
            ShipClass::LargeWarship => "KRIEG2",
            ShipClass::PirateShip => "PIRAT",
        }
    }

    /// True when this ship class is a combat-capable warship
    /// (small / large warship or pirate). Used by callers to
    /// route SHIP4 records to the simulation's MilitaryUnit
    /// path versus the TradeShip path.
    pub fn is_warship(self) -> bool {
        matches!(
            self,
            ShipClass::SmallWarship | ShipClass::LargeWarship | ShipClass::PirateShip
        )
    }

    /// Per-cargo-slot maximum quantity for this ship class.
    /// `FUN_00448120` stores the source 1/32-good quantity in cargo bits
    /// `8..=21`; the SHIP4 manifest preserves the same packed entry.
    /// Audit-derived from the shipping corpus (`probe_cargo_per_class`):
    ///
    ///   SmallTrader   1600
    ///   LargeTrader   1600
    ///   SmallWarship  1600
    ///   LargeWarship  1600
    ///   PirateShip     800 (smaller hold, 8 t/slot)
    pub fn cargo_slot_max_units(self) -> u16 {
        match self {
            ShipClass::PirateShip => 800,
            _ => 1600,
        }
    }
}

/// One ship record from the SHIP4 chunk (436 bytes per slot).
///
/// Cross-scenario sample (Tutorial0 = 1 record, Continous Play00
/// = 4, Cooperation = 7, Atoll = 8, A Plague of Pirates = 19)
/// confirms the chunk is always exactly `N * 436` bytes. The
/// fields decoded here are the ones needed to reconstruct
/// initial ship layouts; remaining bytes (cargo manifest, AI
/// state, route table) are preserved in `Ship::raw_record`.
#[derive(Debug, Clone)]
pub struct Ship {
    /// Complete 436-byte SHIP4 source record. The typed fields below expose
    /// audited projections; remaining bytes retain authored runtime state
    /// for source-backed consumers.
    pub raw_record: [u8; SHIP4_RECORD_BYTES],
    /// Ship name as displayed in the original game UI (e.g.
    /// "Carnera", "Seehind", "Palstek"). 28-byte slot, CP1252,
    /// null-terminated.
    pub name: String,
    /// Spawn position in world tile coordinates (u16 x at
    /// record offset 28, u16 y at offset 30). Audit of 418
    /// records confirms the values span the full 0..~330
    /// range typical of Anno 1602 maps (Geraldine = (301, 237),
    /// Defender = (311, 86), Tutorial0 Seehind = (210, 128)),
    /// not the per-island 0..~50 range. Callers spawning these
    /// into the simulation use them as `MilitaryUnit::tile_x`
    /// or `TradeShip::world_x` directly.
    pub x: u16,
    pub y: u16,
    /// Owning player slot (0 = human, 1..=3 = AI rivals,
    /// 5 = native faction). Audit of 418 ship records across
    /// the shipping corpus surfaces only values {0, 1, 2, 3, 5}
    /// at byte offset 0x4B — slot 4 (free trader) and slot 6
    /// (pirate) never carry static SHIP4 records, presumably
    /// because their fleets spawn dynamically at runtime.
    ///
    /// Notably, every `ShipClass::PirateShip` record in shipping
    /// content carries `owner == 5` (the NATIVE slot) — the
    /// PIRAT figure is the visual hull used by the hostile
    /// native faction, not by the dedicated pirate slot 6.
    /// Slot 6's pirates only ship dynamically from the pirate
    /// Kontor at runtime. Crosstab: 27/27 PirateShip records
    /// → owner 5; 0/418 SHIP4 records → owner 6.
    pub owner: u8,
    /// Full source figure-definition ID at record offsets 0x48..0x49.
    /// `0x0045f550` passes this little-endian word as the second argument
    /// to `FUN_00446ca0` when allocating the live source figure
    /// (`0x0045f79a..0x0045f7a4`). It indexes the executable's compiled
    /// figure-definition table, not the declaration order in figuren.cod.
    pub figure_definition_id: u16,
    /// Low byte of [`Ship::figure_definition_id`] at record offset 0x48.
    /// Audit surfaces
    /// exactly 5 distinct values across all shipping content:
    /// 0x15, 0x17, 0x19, 0x1B, 0x1F — one per ship type
    /// (small trader / large trader / small warship / large
    /// warship / pirate ship). The mapping to figuren.cod's
    /// HANDEL1/HANDEL2/KRIEG1/KRIEG2/PIRAT entries is exposed
    /// through `ShipClass`.
    pub ship_class: u8,
    /// Serialized energy at record offset 0x3C. The source
    /// SHIP4 loader copies it to the live ship record and caps
    /// it by the figure definition's `Maxenergy` value before
    /// the ship enters the entity scheduler (`0x0045f550`,
    /// `0x0045f700..0x0045f70d`).
    pub stored_energy: u16,
    /// Live-ship slot selected by the scenario at record offset
    /// 0x46. The source loader uses this as the index into its
    /// 0x218-byte ship-record array (`0x0045f5c3..0x0045f5da`).
    pub runtime_slot: u16,
    /// Source figure category at record offset 0x4A. The SHIP4
    /// loader passes it as the first argument when creating the
    /// runtime figure (`0x0045f79a..0x0045f7a4`).
    pub figure_kind: u8,
    /// Candidate-list key at record offset 0x4D. The SHIP4 loader writes
    /// this byte to live figure offset `+0x01` before `FUN_00453da0` uses it
    /// to select the source candidate list (`0x0045f7c0..0x0045f7c5`). It
    /// is independent from the faction at record offset 0x4B.
    pub candidate_list_key: u8,
    /// Source movement direction at record offset 0x50. The SHIP4 loader
    /// writes this to the live figure direction byte (`0x0045f7c6..0x0045f7cb`),
    /// separately from the renderer heading at 0x42.
    pub source_direction: u8,
    /// Initial figure-animation state at record offset 0x4E.
    /// After allocating the stationary source figure, the
    /// SHIP4 loader passes this value to `0x00446d90`, which
    /// selects the animation and resets its frame counters
    /// (`0x0045f7d8..0x0045f7ff`).
    pub animation_state: u8,
    /// (See `ShipClass::from_byte` for the typed decode.)
    /// Compass-heading byte at record offset 0x42. Raw value
    /// ranges 0..14 across the corpus; 95% are even. Likely a
    /// `(heading × 2) + frame_phase` packing — use
    /// `Ship::heading()` for the typed 0..7 cardinal direction. The SHIP4
    /// loader also copies the raw byte to its shared category-1/2/3 slot at
    /// `+0x1a2`, where `FUN_00454250` reads it as a score-state tier.
    pub heading_byte: u8,
    /// Up to 7 packed cargo entries at record offsets 0x175, 0x17D, 0x185,
    /// 0x18D, 0x195, 0x19D, 0x1A5 (stride 8; the trailing byte of each 8-byte
    /// group is zero). `FUN_00448120` decodes the low byte as the source ware,
    /// bits `8..=21` as its exact 1/32-good quantity, and bits `22..=31` as
    /// entry metadata. The raw array remains available because source special
    /// wares are not all represented by the local `Good` enum.
    pub cargo_slots: [u32; 7],
}

/// One source land figure from a `SOLDAT3` record.
///
/// `FUN_0045fac0` loads 68-byte `SOLDAT3` records into the source's
/// 400-entry land-figure pool. Its `0x44`-byte branch passes byte `0x16`
/// to `FUN_00446ca0` as the figure kind, then restores the island, owner,
/// and direction from bytes `0x17`, `0x18`, and `0x1b` respectively.
#[derive(Debug, Clone)]
pub struct LandFigure {
    /// Full source record retained for later state extraction.
    pub raw_record: [u8; SOLDAT3_RECORD_BYTES],
    /// World x coordinate at record offset 0x00.
    pub x: u16,
    /// World y coordinate at record offset 0x02.
    pub y: u16,
    /// Current source energy at record offset 0x04. `FUN_0045fac0` copies
    /// this word to the type-4 runtime slot at `+0x0a`; `FUN_00454250`
    /// reads it when scoring combat candidates.
    pub source_energy: u16,
    /// Compiled source figure-definition ID at record offset 0x06.
    pub figure_definition_id: u16,
    /// Type-4 runtime slot at record offset 0x08. `FUN_0045fac0` stores
    /// the allocated source figure at this slot in its 400-entry table.
    pub runtime_slot: u16,
    /// Four-byte type-4 origin descriptor copied by `FUN_0045fac0` from
    /// record offsets 0x0a..0x0d. `FUN_00456d00` uses it as the native
    /// idle-branch anchor.
    pub origin_descriptor: [u8; 4],
    /// Type-4 route-search radius at record offset 0x0b. `FUN_0045fac0`
    /// writes this byte to the first field of the live per-slot record;
    /// `FUN_00456d00` passes it to `FUN_004581f0`, which builds a centered
    /// `2r + 1` raw-coordinate route grid.
    pub route_radius: u8,
    /// Source figure kind at record offset 0x16.
    pub figure_kind: u8,
    /// Source island record at offset 0x17.
    pub island_id: u8,
    /// Owning player slot at offset 0x18.
    pub owner: u8,
    /// Source movement direction at record offset 0x1b.
    pub direction: u8,
    /// Initial animation state passed to `FUN_00446d90` at record offset
    /// 0x19.
    pub animation_state: u8,
    /// Type-4 alternate-state selector at record offset `0x1c`.
    /// `FUN_0045fac0` copies it to runtime offset `+0x126`; when the live
    /// target clears, `FUN_00458190` advances it through the two descriptors
    /// in `state_payload`.
    pub state_selector: u8,
    /// Four-byte type-4 descriptor copied by `FUN_0045fac0` from record
    /// offsets 0x12..0x15 into the live per-slot state read by
    /// `FUN_00456d00`.
    pub state_descriptor: [u8; 4],
    /// Low two source state bits loaded at record offset 0x1d.
    pub state_flags: u8,
    /// Type-4 per-slot state copied by `FUN_0045fac0` from record offsets
    /// 0x1e..0x25 into `DAT_0051c7a0`.
    pub state_payload: [u8; 8],
}

/// Bytes per source `SOLDAT3` record. `FUN_0045fac0` selects this format
/// by the `SOLDAT3` chunk name and advances its input cursor by 0x44 bytes.
pub const SOLDAT3_RECORD_BYTES: usize = 0x44;

/// Authored figure family selected by a type-4 `SOLDAT3` definition ID.
///
/// `FUN_00441210` registers the executable's figure-name table at
/// `DAT_00498b98`: IDs 1..=16 name the four player soldier ladders and
/// IDs 33..=36 name the native `SPEER` ladder. `SOLDAT3` stores one of
/// those compiled IDs at record offset 0x06.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LandFigureFamily {
    Infantry,
    Cavalry,
    Musketeer,
    Cannoneer,
    NativeSpearman,
}

/// Resolved type-4 `SOLDAT3` figure definition.
///
/// `variant` is the one-based suffix of the source `figuren.cod` name:
/// for example, definition ID 3 is `SOLDAT3`, and definition ID 33 is
/// `SPEER1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LandFigureDefinition {
    pub id: u16,
    pub family: LandFigureFamily,
    pub variant: u8,
}

impl LandFigureDefinition {
    /// Decode a compiled type-4 figure-definition ID from the executable's
    /// `DAT_00498b98` name table.
    pub const fn from_id(id: u16) -> Option<Self> {
        let (family, first_id) = match id {
            1..=4 => (LandFigureFamily::Infantry, 1),
            5..=8 => (LandFigureFamily::Cavalry, 5),
            9..=12 => (LandFigureFamily::Musketeer, 9),
            13..=16 => (LandFigureFamily::Cannoneer, 13),
            33..=36 => (LandFigureFamily::NativeSpearman, 33),
            _ => return None,
        };
        Some(Self {
            id,
            family,
            variant: (id - first_id + 1) as u8,
        })
    }

    /// Symbolic `figuren.cod` definition named by this compiled ID.
    pub const fn source_figure_name(self) -> &'static str {
        match (self.family, self.variant) {
            (LandFigureFamily::Infantry, 1) => "SOLDAT1",
            (LandFigureFamily::Infantry, 2) => "SOLDAT2",
            (LandFigureFamily::Infantry, 3) => "SOLDAT3",
            (LandFigureFamily::Infantry, 4) => "SOLDAT4",
            (LandFigureFamily::Cavalry, 1) => "KAVALERIE1",
            (LandFigureFamily::Cavalry, 2) => "KAVALERIE2",
            (LandFigureFamily::Cavalry, 3) => "KAVALERIE3",
            (LandFigureFamily::Cavalry, 4) => "KAVALERIE4",
            (LandFigureFamily::Musketeer, 1) => "MUSKETIER1",
            (LandFigureFamily::Musketeer, 2) => "MUSKETIER2",
            (LandFigureFamily::Musketeer, 3) => "MUSKETIER3",
            (LandFigureFamily::Musketeer, 4) => "MUSKETIER4",
            (LandFigureFamily::Cannoneer, 1) => "KANONIER1",
            (LandFigureFamily::Cannoneer, 2) => "KANONIER2",
            (LandFigureFamily::Cannoneer, 3) => "KANONIER3",
            (LandFigureFamily::Cannoneer, 4) => "KANONIER4",
            (LandFigureFamily::NativeSpearman, 1) => "SPEER1",
            (LandFigureFamily::NativeSpearman, 2) => "SPEER2",
            (LandFigureFamily::NativeSpearman, 3) => "SPEER3",
            (LandFigureFamily::NativeSpearman, 4) => "SPEER4",
            _ => panic!("valid LandFigureDefinition has variant 1 through 4"),
        }
    }

    /// Authored `Speed:` compiled into the type-4 figure record at `+0x10`
    /// after multiplication by 0.0001. The four visual variants inherit the
    /// movement properties from their numbered base definition.
    pub const fn source_move_speed(self) -> u16 {
        match self.family {
            LandFigureFamily::Infantry => 260,
            LandFigureFamily::Cavalry => 400,
            LandFigureFamily::Musketeer => 210,
            LandFigureFamily::Cannoneer => 230,
            LandFigureFamily::NativeSpearman => 280,
        }
    }

    /// `Speedtyp:` index used to select the terrain `Wegspeed` divisor.
    /// Infantry, musketeers, and native spearmen inherit the base value 0;
    /// cavalry supplies 1 and cannon 2 in `figuren.cod`.
    pub const fn source_speed_type(self) -> u8 {
        match self.family {
            LandFigureFamily::Cavalry => 1,
            LandFigureFamily::Cannoneer => 2,
            LandFigureFamily::Infantry
            | LandFigureFamily::Musketeer
            | LandFigureFamily::NativeSpearman => 0,
        }
    }

    /// Authored `Maxstepcnt:` passed as the `FUN_0046cf70` run-compression
    /// limit by `FUN_004581f0`. This counts raw doubled route-grid cells,
    /// not INSEL5 cells.
    pub const fn source_max_step_count(self) -> u8 {
        match self.family {
            LandFigureFamily::Cannoneer => 3,
            LandFigureFamily::Musketeer => 2,
            LandFigureFamily::Infantry
            | LandFigureFamily::Cavalry
            | LandFigureFamily::NativeSpearman => 1,
        }
    }

    /// Authored `Maxenergy:` compiled by `FUN_00441210` at definition offset
    /// `+0x3c`. `FUN_00454250` reads this field while scoring a live type-4
    /// candidate; visual variants inherit the numbered base value.
    pub const fn source_max_energy(self) -> u16 {
        match self.family {
            LandFigureFamily::Infantry => 20,
            LandFigureFamily::Cavalry | LandFigureFamily::NativeSpearman => 18,
            LandFigureFamily::Musketeer => 15,
            LandFigureFamily::Cannoneer => 12,
        }
    }

    /// `Maxenergy:` in the source runtime's compiled units. `FUN_00441210`
    /// multiplies the parsed value by the executable constant `3.0` at
    /// `0x00441a4c` before storing it at definition offset `+0x3c`; the
    /// `SOLDAT3 +0x04` corpus and `FUN_00454250` use this same scale.
    pub const fn source_runtime_energy_cap(self) -> u16 {
        self.source_max_energy() * 3
    }

    /// Authored `Shotradius:` compiled by `FUN_00441210` at definition
    /// offset `+0x4a`. `FUN_00454250` uses it as the candidate approach
    /// threshold and `FUN_00453e50` uses the corresponding route radius.
    /// The source records fractional melee radii, so this remains a float.
    pub const fn source_shot_radius(self) -> f32 {
        match self.family {
            LandFigureFamily::Infantry | LandFigureFamily::Cavalry => 0.75,
            LandFigureFamily::Musketeer => 4.0,
            LandFigureFamily::Cannoneer => 7.0,
            LandFigureFamily::NativeSpearman => 1.0,
        }
    }

    /// `Shotradius:` in the source runtime units. `FUN_00441210` multiplies
    /// the parsed value by the executable constant `2.0` at `0x0044170c`
    /// and converts toward zero before storing the unsigned definition field
    /// at `+0x4a`; `FUN_00454250` consumes that field directly.
    pub const fn source_runtime_shot_radius(self) -> u16 {
        match self.family {
            LandFigureFamily::Infantry | LandFigureFamily::Cavalry => 1,
            LandFigureFamily::Musketeer => 8,
            LandFigureFamily::Cannoneer => 14,
            LandFigureFamily::NativeSpearman => 2,
        }
    }

    /// `Hitpoint:` in the runtime units consumed by `FUN_00454250`.
    /// `FUN_00441210` applies the same `3.0` multiplier at `0x00441abb` as
    /// its `Maxenergy:` loader branch, then converts toward zero before
    /// storing the unsigned definition field.
    pub const fn source_runtime_hit_points(self) -> u16 {
        match self.family {
            LandFigureFamily::Infantry => 3,
            LandFigureFamily::Cavalry => 4,
            LandFigureFamily::Musketeer => 7,
            LandFigureFamily::Cannoneer => 21,
            LandFigureFamily::NativeSpearman => 3,
        }
    }

    /// `Worktime:` stored as a source `float` at compiled definition offset
    /// `+0x14` by `FUN_00441210`. `FUN_00454250` divides the energy-adjusted
    /// hit-point term by this value before converting it toward zero.
    pub const fn source_runtime_work_time(self) -> f32 {
        match self.family {
            LandFigureFamily::Infantry | LandFigureFamily::NativeSpearman => 0.8,
            LandFigureFamily::Cavalry => 1.0,
            LandFigureFamily::Musketeer => 2.0,
            LandFigureFamily::Cannoneer => 4.5,
        }
    }

    /// `Shottime:` converted by `FUN_00441210` to the 100-ms delay units at
    /// compiled definition offset `+0x50`. The category-6 executor passes
    /// this integer directly to its deferred-event allocator.
    pub const fn source_shot_delay_ticks(self) -> u32 {
        match self.family {
            LandFigureFamily::Infantry
            | LandFigureFamily::Cannoneer
            | LandFigureFamily::NativeSpearman => 6,
            LandFigureFamily::Cavalry | LandFigureFamily::Musketeer => 7,
        }
    }

    /// `Drehtime:` at runtime record offset `+0x18`. None of the five
    /// type-4 land families declares this field in `figuren.cod`, so the
    /// loader retains its zero-initialized value. `FUN_00457ce0` consequently
    /// starts their route command without a turn-only update.
    pub const fn source_turn_time(self) -> f32 {
        let _ = self;
        0.0
    }
}

/// The four compiled definition fields consumed by `FUN_00454250` when it
/// scores a combat candidate. `FUN_00441210` assigns definition IDs from the
/// executable name table at `0x00498b98`; the loader multiplies authored
/// `Maxenergy:` and `Hitpoint:` by `3.0`, and `Shotradius:` by `2.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceCombatDefinition {
    pub id: u16,
    pub source_figure_name: &'static str,
    pub runtime_energy_cap: u16,
    pub runtime_hit_points: u16,
    pub runtime_shot_radius: u16,
    pub runtime_work_time: f32,
    /// Compiled `Shottime:` at definition offset `+0x50`, expressed in the
    /// source queue's 100-ms `DAT_005b6040` ticks. `FUN_00447880` passes this
    /// directly to `FUN_00478a60` when it queues a category-6 hit.
    pub runtime_shot_delay_ticks: u32,
    /// Compiled `Shotfignr:` at definition offset `+0x48`. `FUN_00447880`
    /// reads this after executing a category-6 action and creates a kind-15
    /// figure only when it is nonzero.
    pub runtime_shot_figure_id: Option<u16>,
}

/// Compiled source definition field `Preis` at offset `+0x54`. The
/// `FUN_00441210` name table has 119 entries; only the six ship base
/// definitions assign this field in `figuren.cod`, while their `ObjFill`
/// variants inherit the same value and every other entry remains zero.
pub const fn source_figure_purchase_price(id: u16) -> u32 {
    match id {
        0x15 | 0x16 => 1_000,
        0x17 | 0x18 => 1_800,
        0x19 | 0x1a => 1_300,
        0x1b | 0x1c => 2_400,
        0x1d | 0x1e => 2_000,
        0x1f | 0x20 => 2_000,
        _ => 0,
    }
}

/// `FUN_00422030`'s candidate cost: `(Preis >> 7) × live energy`.
pub const fn source_figure_purchase_cost(id: u16, source_energy: u16) -> u32 {
    (source_figure_purchase_price(id) >> 7) * source_energy as u32
}

impl SourceCombatDefinition {
    /// Resolve every combat-capable compiled figure ID currently identified
    /// in the source definition table. IDs without a score-bearing figure
    /// definition deliberately return `None`.
    pub const fn from_id(id: u16) -> Option<Self> {
        if let Some(definition) = LandFigureDefinition::from_id(id) {
            return Some(Self {
                id,
                source_figure_name: definition.source_figure_name(),
                runtime_energy_cap: definition.source_runtime_energy_cap(),
                runtime_hit_points: definition.source_runtime_hit_points(),
                runtime_shot_radius: definition.source_runtime_shot_radius(),
                runtime_work_time: definition.source_runtime_work_time(),
                runtime_shot_delay_ticks: definition.source_shot_delay_ticks(),
                runtime_shot_figure_id: None,
            });
        }

        let definition = match id {
            0x15 => ("HANDEL1", 150, 6, 14, 5.0, 10, Some(113)),
            0x16 => ("HANDELD1", 150, 6, 14, 5.0, 10, Some(113)),
            0x17 => ("HANDEL2", 240, 6, 14, 5.0, 10, Some(112)),
            0x18 => ("HANDELD2", 240, 6, 14, 5.0, 10, Some(112)),
            0x19 => ("KRIEG1", 195, 6, 14, 5.0, 10, Some(113)),
            0x1a => ("KRIEGD1", 195, 6, 14, 5.0, 10, Some(113)),
            0x1b => ("KRIEG2", 360, 6, 14, 5.0, 10, Some(112)),
            0x1c => ("KRIEGD2", 360, 6, 14, 5.0, 10, Some(112)),
            0x1d => ("HANDLER", 285, 6, 14, 5.0, 10, Some(112)),
            0x1e => ("HANDLERD", 285, 6, 14, 5.0, 10, Some(112)),
            0x1f => ("PIRAT", 285, 6, 14, 5.0, 10, Some(112)),
            0x20 => ("PIRATD", 285, 6, 14, 5.0, 10, Some(112)),
            0x25 => ("TRADER1", 42, 2, 8, 2.0, 6, None),
            0x26 => ("KANONTURM", 72, 12, 15, 3.0, 10, Some(114)),
            0x28 => ("PIRATTURM", 72, 12, 15, 3.0, 10, Some(115)),
            _ => return None,
        };
        Some(Self {
            id,
            source_figure_name: definition.0,
            runtime_energy_cap: definition.1,
            runtime_hit_points: definition.2,
            runtime_shot_radius: definition.3,
            runtime_work_time: definition.4,
            runtime_shot_delay_ticks: definition.5,
            runtime_shot_figure_id: definition.6,
        })
    }
}

/// The compiled kind-15 definition fields consumed by `FUN_00447f00` after a
/// source category-6 action. The executable rotates `Fahnoffs.x` by the
/// selected direction and adds `Fahnoffs.z` to the launcher's live height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceShotFigureDefinition {
    pub id: u16,
    pub source_figure_name: &'static str,
    /// Authored `Worktime:` copied to the kind-15 live record at `+0x14` by
    /// `FUN_00447f00`. The generic figure update consumes it using the
    /// record's `0.02` step amount.
    pub runtime_work_time: f32,
    pub runtime_fahnoffs_x: f32,
    pub runtime_fahnoffs_z: f32,
}

impl SourceShotFigureDefinition {
    /// Resolve the four `Shotfignr:` targets used by the recovered ship and
    /// tower definitions. The IDs are direct indices in the executable's
    /// `figuren.cod` name table at `0x00498b98`.
    pub const fn from_id(id: u16) -> Option<Self> {
        let (source_figure_name, runtime_work_time, runtime_fahnoffs_x, runtime_fahnoffs_z) =
            match id {
                112 => ("KANONSHOT1", 0.96, 0.5, 4.0),
                113 => ("KANONSHOT2", 0.96, 0.35, 4.0),
                114 => ("KANONSHOTTURM", 0.96, 0.5, 2.0),
                115 => ("KANONSHOTTURM2", 0.96, 0.5, 0.8),
                _ => return None,
            };
        Some(Self {
            id,
            source_figure_name,
            runtime_work_time,
            runtime_fahnoffs_x,
            runtime_fahnoffs_z,
        })
    }
}

impl LandFigure {
    /// Resolve this type-4 figure's executable definition ID.
    pub const fn definition(&self) -> Option<LandFigureDefinition> {
        LandFigureDefinition::from_id(self.figure_definition_id)
    }
}

impl Ship {
    /// Decode the raw `ship_class` byte into a typed `ShipClass`.
    /// Returns `None` if the byte falls outside the observed
    /// shipping-corpus set — useful for callers that want to
    /// short-circuit on malformed scenarios.
    pub fn class(&self) -> Option<ShipClass> {
        ShipClass::from_byte(self.ship_class)
    }

    /// Compass heading 0..7 (N, NE, E, SE, S, SW, W, NW)
    /// derived from byte 0x42 of the SHIP4 record. The raw
    /// byte ranges 0..14 with 95% of records carrying an
    /// even value; the binary appears to pack `heading × 2 +
    /// frame_phase`, so `heading_byte / 2` yields the cardinal
    /// direction. Renderers can feed this straight to the
    /// existing `TradeShip::heading` / sprite-rotation logic.
    pub fn heading(&self) -> u8 {
        self.heading_byte / 2
    }

    /// Eight raw SHIP4 slots at `0x132 + 8i` (spanning `0x132..0x172`, before
    /// the cargo block at `0x175`). These feed the category-6 combat-selection
    /// path resolved by `resolve_ship_kind6_policy_slots`; the runtime consumer
    /// is `FUN_00458e60`, which tests the low-byte item IDs while deciding
    /// whether a category-6 figure may target the ship.
    ///
    /// The exact disk offset is NOT independently confirmed against a SHIP4
    /// disk-record loader in the decompiled dump (the load path that expands
    /// these into runtime state is not present in `1602_exe.c`), and the field
    /// is all-zero in every shipped scenario, so it has no observable effect
    /// today. `0x132` is retained because it is the only pre-cargo placement:
    /// `0x174` (an earlier speculative alternative) would span `0x174..0x1b4`
    /// and collide byte-for-byte with the cargo slots at `0x175`.
    pub fn source_kind6_policy_raw_slots(&self) -> [u64; 8] {
        std::array::from_fn(|index| {
            let offset = 0x132 + index * 8;
            u64::from_le_bytes(
                self.raw_record[offset..offset + 8]
                    .try_into()
                    .expect("fixed SHIP4 policy slot lies inside its record"),
            )
        })
    }

    /// Bytes copied by `FUN_0045f550` from `SHIP4 + 0x2c` into the shared
    /// category record at `+0x18`. `FUN_00445650` uses them to construct a
    /// category-1/2/3 target descriptor for category-6 combat selection.
    pub const fn source_kind6_target_descriptor_payload(&self) -> [u8; 2] {
        [self.raw_record[0x2c], self.raw_record[0x2d]]
    }
}

pub const SHIP4_RECORD_BYTES: usize = 436;
const SHIP4_NAME_BYTES: usize = 28;

/// Per-island city info parsed from a STADT4 chunk (168 bytes).
///
/// Layout (verified by `cargo run --example audit_stadt4_bytes`
/// across 245 city records — owner-distribution survey shows
/// byte 0 ranges 0..=33 across the corpus, matching island
/// indexes, while byte 0x02 ranges 0..=6 matching the seven
/// player slots):
///
/// ```text
/// byte 0x00  island_index — which island carries this city
///            (an index into the scenario's island list, 0..N)
/// byte 0x02  owner_slot — which player slot owns the city
///            (0=human, 1..=3=AI rivals, 4=trader,
///             5=natives, 6=pirates), matching PLAYER4's
///             slot numbering
/// 0x87..0xa7 null-terminated CP1252 city name
/// 0x78..0x86 fifteen 0x80 sentinel bytes (semantics TBD)
/// ```
///
/// Cross-scenario sample: New Horizons2 places "Jaricho" on
/// island 21 with owner_slot 6 (pirates), "Radolfsell" on
/// island 19 with owner_slot 5 (natives), confirming the
/// island-vs-slot split.
#[derive(Debug, Clone)]
pub struct City {
    /// Island index this city sits on (0..N where N is the
    /// number of islands in the scenario). Was previously
    /// mis-labelled as `owner`.
    pub island_index: u8,
    /// Player slot owning the city (matches PLAYER4 slot
    /// numbering). 0 for the player's main settlement; AI
    /// rivals 1..=3; reserved factions 4..=6.
    pub owner_slot: u8,
    /// Per-tier inhabitant counts at record offsets 0x5C, 0x60,
    /// 0x64, 0x68, 0x6C (five u32, stride 4). Binary-confirmed:
    /// the city loader `FUN_00484af0` copies exactly these five
    /// dwords (`edi = record + 0x5C`, 5 iterations) into the
    /// runtime city record at `+0x220`. Values form a class
    /// pyramid ascending index 0 → 4 — e.g. "Metropolis" carries
    /// `[2, 108, 143, 800, 4440]`, so index 0 (0x5C) is the
    /// smallest, highest tier and index 4 (0x6C) the largest,
    /// lowest tier. Empty placeholder cities leave the array
    /// all-zero.
    pub tier_population: [u32; 5],
    pub name: String,
}

/// A single tile/building record from INSELHAUS (8 bytes).
///
/// `anim_count` audit across 333,354 tiles surfaces:
///   * values 0..=6 cover 81% of tiles (typical animation
///     frame index, max 6 matches the longest animations in
///     haeuser.cod's `AnimAnz`)
///   * values 64..=68 cover 6% — high-bit-set patterns
///     suggesting (frame_idx, flag) packing where bits 6/7
///     encode something else (under-construction overlay?)
///   * value 192 (= bits 6+7 set) on 1% of tiles
///
/// `flags` audit (`cargo run --example audit_inselhaus_flags`)
/// across 333,354 tiles in 62 shipping `.szs` files surfaces:
///
///   * bit 0 (0x0001):  2% set — sparse, likely a true flag
///                      (under-construction / damaged-marker)
///   * bits 1..=7:      45-58% set each — looks like a per-tile
///                      randomization seed the engine uses to
///                      pick between the building's animation
///                      / rotation variants
///   * bit 8 (0x0100):  7% set — second sparse flag candidate
///
/// 224 distinct values across the corpus, dominated by the
/// 0x0040..0x007E range (bits 1-6 set in various combinations).
/// The semantic decode of bits 0 / 8 hasn't been pinned to a
/// specific binary function yet.
#[derive(Debug, Clone, Copy, Default)]
pub struct IslandTile {
    /// Low 16 bits of the source definition ID. The INSELHAUS loader at
    /// `0x004685af` adds [`INSELHAUS_SOURCE_ID_BASE`] and resolves the
    /// compiled definition; this is not a STADTFLD sprite index.
    pub building_id: u16,
    pub x: u8,
    pub y: u8,
    pub orientation: u8,
    pub anim_count: u8,
    pub flags: u16,
}

/// Constant added to each INSELHAUS u16 before the original loader resolves
/// its compiled building definition at `0x004685af`.
pub const INSELHAUS_SOURCE_ID_BASE: i32 = 0x4e20;

impl IslandTile {
    /// Resolved haeuser.cod source ID used by the original INSELHAUS loader.
    pub fn source_id(self) -> i32 {
        INSELHAUS_SOURCE_ID_BASE + i32::from(self.building_id)
    }

    /// Owner code consumed by `FUN_00464370` while loading this INSELHAUS
    /// record. The loader reads bits 14..=16 of the four-byte word beginning
    /// at record offset 4; those are anim_count bits 6..=7 followed by flags
    /// bit 0.
    pub fn source_owner(self) -> u8 {
        (self.anim_count >> 6) | ((self.flags as u8 & 1) << 2)
    }

    /// Object owner passed as `param_7` to `FUN_00465170` by the
    /// INSELHAUS loader. It reads bits 22..=25 of the record word, which
    /// correspond to bits 6..=9 of the parsed flags field.
    pub fn source_dynamic_object_owner(self) -> u8 {
        ((self.flags >> 6) & 0x0f) as u8
    }
}

/// Parsed save/scenario file.
#[derive(Debug)]
pub struct SzsFile {
    pub chunks: Vec<Chunk>,
    pub islands: Vec<Island>,
    /// Per-slot setup parsed from the `PLAYER4` chunk. Up to 7
    /// entries (slots 0-6 matching our diplomacy layout). Empty
    /// when no PLAYER4 chunk is present.
    pub players: Vec<PlayerSlotInit>,
    /// Mission briefing & goal data parsed from `AUFTRAG4`.
    /// `None` for files without that chunk (rare; even tutorial
    /// scenarios carry an AUFTRAG4 with all-zero goal flags).
    pub mission: Option<Mission>,
    /// Scenario metadata parsed from the four `SZENE_*` chunks
    /// (`MISSNR`, `PLAYERMIN`, `PLAYERMAX`, `RANKING`). Each is
    /// a single u32; absent chunks come back as `None`.
    pub scenario: ScenarioMeta,
    /// Initial ship layout parsed from the SHIP4 chunk. Empty
    /// when the scenario contains no ships.
    pub ships: Vec<Ship>,
    /// Initial land figures parsed from the SOLDAT3 chunk. Source shipping
    /// scenarios use kind-4 entries for their authored land soldiers.
    pub land_figures: Vec<LandFigure>,
}

/// Scenario-level metadata (mission #, player range, difficulty
/// ranking) extracted from the four `SZENE_*` chunks at the top
/// of every shipping `.szs` file. Each field is `Option<u32>`
/// because tutorial scenarios omit the player-count chunks (they
/// are implicit single-player) and standalone "Continous Play"
/// maps omit the mission number.
///
/// `SZENE_RANKING` is the scenario difficulty / ranking
/// rating, audit-confirmed across 62 shipping `.szs` files:
///
///   0 = Tutorial / very easy (Tutorial0, One lone Settlement,
///       The End of a long Trip — 8 scenarios)
///   1 = Easy (Peaceful Reign, The Trial, No Surplus of Land —
///       3 scenarios)
///   2 = Medium (Atoll, Continous Play 00..02, Cooperation,
///       The Continent — 42 scenarios; the bulk of shipping
///       content)
///   3 = Hard (A Plague of Pirates, New Horizons0, On His
///       Majesty's Service 1..2 — 9 scenarios)
///
/// `SZENE_MISSNR` is the campaign slot index visible in the
/// original mission picker (0 = Tutorial0/Tutorial1, 1 =
/// Continous Play 00/01, etc.).
#[derive(Debug, Clone, Default)]
pub struct ScenarioMeta {
    pub mission_nr: Option<u32>,
    pub player_min: Option<u32>,
    pub player_max: Option<u32>,
    pub ranking: Option<u32>,
}

/// Mission metadata extracted from the `AUFTRAG4` chunk.
///
/// The chunk is always 2244 bytes and is dispatched by the
/// engine's chunk loader (`s_AUFTRAG4` reachable via the dispatch
/// in `1602_exe.c` around offset 0x40dxxx; see siblings
/// `s_AUFTRAG`/`s_AUFTRAG2` at 0x498270/0x49827c). The decompiled
/// loader is opaque (label-based dispatch), so this struct only
/// surfaces fields verified across multiple shipping scenarios:
///
/// | Field | Offset | Cross-scenario evidence |
/// |---|---|---|
/// | `flags` | 0x04 (u32) | bit-flag corpus surveyed across all 60 shipping `.szs` files |
/// | `briefing` | 0x68 onward | Always begins at 0x68, null-terminated, CP1252 |
/// | `goals_raw` | 0x870..end | Raw 996 bytes of typed goal records — structure not fully RE'd |
///
/// Mission-flag bit assignments (deduced by cross-referencing
/// the 60-scenario corpus against each scenario's briefing text):
///
///   bit 0 (`MISSION_FLAG_POPULATION`)  — primary population goal
///                                         active; goals_raw[0..4]
///                                         is the threshold
///   bit 4 (`MISSION_FLAG_COOPERATIVE`) — cooperative neighbour-
///                                         assist goal (Good Neighbors,
///                                         Alliance)
///   bit 8 (`MISSION_FLAG_RANKING`)     — secondary "ranking" /
///                                         tier-headcount sub-goal
///   bit 9                              — competitive (1v1) ranking
///                                         (Competition.szs only)
///   bit 10 (`MISSION_FLAG_PIRATE`)     — must defeat pirates /
///                                         survive raid waves
///                                         (Plague, Pirata, Fortress)
///
/// Tutorials and Continous-Play templates carry `flags = 0`.
/// Higher-bit semantics (0x1000 / 0x4000 / 0x10000 / 0x1c000)
/// remain not yet RE'd.
///
/// Confirmed against the full Szenes/ directory: 60 `.szs` files.
#[derive(Debug, Clone)]
pub struct Mission {
    /// Mission goal-flags bitfield. See [`MISSION_FLAG_*`] constants
    /// in this module.
    pub flags: u32,
    /// Briefing text shown before the scenario starts. Decoded
    /// from CP1252 with the trailing nulls stripped.
    pub briefing: String,
    /// Goal-record region (offsets 0x870..0x8c4). First u32 is
    /// reliably the primary population threshold; remaining bytes
    /// encode tier indices and secondary goals whose layout is
    /// scenario-flag dependent.
    pub goals_raw: Vec<u8>,
}

impl Mission {
    /// Decode the goal numbers each `MISSION_FLAG_*` bit
    /// references. Cross-scenario survey of all 60 shipping
    /// `.szs` files showed the layout:
    ///
    ///   goals_raw u32  0 = primary population threshold
    ///   goals_raw u32  1 = primary tier index (0..=4)
    ///   goals_raw u32 18 = cooperative-neighbour population
    ///                       (when `MISSION_FLAG_COOPERATIVE`)
    ///
    /// Other slots (u32s 2..=7) hold per-tier sub-goals — e.g.
    /// "500 of these must be Merchants" — but the per-flag
    /// layout there isn't fully stable across scenarios, so
    /// they're left to callers via `goals_raw`.
    pub fn goals(&self) -> MissionGoals {
        let read_u32 = |i: usize| -> u32 {
            let off = i * 4;
            if off + 4 > self.goals_raw.len() {
                return 0;
            }
            u32::from_le_bytes([
                self.goals_raw[off],
                self.goals_raw[off + 1],
                self.goals_raw[off + 2],
                self.goals_raw[off + 3],
            ])
        };
        let triple = |start: usize| -> Option<PopulationGoal> {
            let total = read_u32(start);
            if total == 0 {
                return None;
            }
            let tier_raw = read_u32(start + 1);
            let tier = if (1..=4).contains(&tier_raw) {
                Some(tier_raw as u8)
            } else {
                None
            };
            Some(PopulationGoal {
                total,
                tier,
                at_tier: read_u32(start + 2),
            })
        };
        let coop_active = self.flags & MISSION_FLAG_COOPERATIVE != 0;
        MissionGoals {
            primary: (self.flags & MISSION_FLAG_POPULATION != 0)
                .then(|| triple(0))
                .flatten(),
            secondary: (self.flags & MISSION_FLAG_POPULATION2 != 0)
                .then(|| triple(3))
                .flatten(),
            tertiary: (self.flags & MISSION_FLAG_POPULATION3 != 0)
                .then(|| triple(6))
                .flatten(),
            cooperative_population: coop_active.then(|| read_u32(18)).filter(|&v| v > 0),
            cooperative_tier: coop_active
                .then(|| read_u32(19))
                .filter(|&v| (1..=4).contains(&v))
                .map(|v| v as u8),
        }
    }
}

const AUFTRAG4_BRIEFING_OFFSET: usize = 0x68;
const AUFTRAG4_GOALS_OFFSET: usize = 0x870;

/// Set when a population threshold is the primary mission goal.
/// Triple at `goals_raw` u32 0..=2 = (total, tier, at-tier count).
pub const MISSION_FLAG_POPULATION: u32 = 1 << 0;

/// Set when the scenario carries a SECOND population threshold
/// (often a second city you must build). Triple at goals_raw
/// u32 3..=5 = (total, tier, at-tier count). Used by On His
/// Majesty's Service3, The Search for Gold, Exile, etc.
pub const MISSION_FLAG_POPULATION2: u32 = 1 << 1;

/// Set when the scenario carries a THIRD population threshold.
/// Triple at goals_raw u32 6..=8. Used by The Continent and
/// New Horizons1 (three cities of 5000 / 500 respectively).
pub const MISSION_FLAG_POPULATION3: u32 = 1 << 2;

/// Set when the scenario carries a cooperative-neighbour goal
/// (Good Neighbors, The Alliance). Triple at goals_raw u32 18,19.
pub const MISSION_FLAG_COOPERATIVE: u32 = 1 << 4;

/// Set when there is a "ranking" sub-goal — typically a
/// per-tier headcount requirement on top of the primary
/// population threshold.
pub const MISSION_FLAG_RANKING: u32 = 1 << 8;

/// Set when the scenario involves a pirate / hostile-faction
/// combat goal (Plague of Pirates, Pirata, Fortress, Exile,
/// Dark Clouds on the Horizon, To Each his Own).
pub const MISSION_FLAG_PIRATE: u32 = 1 << 10;

/// Mission flag bits observed in shipping scenarios but whose
/// binary semantics are not yet reverse-engineered. Audit script:
/// `cargo run --example audit_mission_flags -p anno-formats`.
/// All carry NO data in `goals_raw` — pure flag-only objectives,
/// presumably evaluated against simulation state by 1602.exe's
/// goal-check function.
///
/// | bit  | scenarios                               | briefing hint |
/// |------|-----------------------------------------|---------------|
/// | 0x80 | The Magnate2                            | "spice monopoly… aura of peace and prosperity" |
/// | 0x200 | Competition                            | "first competitors do arrive" — defend |
/// | 0x1000 | Magnate0, Quest for Ore, Trust no one1 | wealth/treasury/tools-and-weapons |
/// | 0x4000 | Magnate1, Monopoly                    | settlement / cocoa-tobacco markets |
/// | 0x8000 | Monopoly                              | second monopoly slot |
/// | 0x10000 | On His Majesty's Service0, Monopoly  | "rare resources" |
pub const MISSION_FLAG_OBSERVED_UNMODELLED: u32 =
    0x0000_0080 | 0x0000_0200 | 0x0000_1000 | 0x0000_4000 | 0x0000_8000 | 0x0001_0000;

/// One population requirement: total inhabitants, optional tier,
/// and how many of those total must be at that tier. Cross-
/// scenario evidence: Cooperation `[2000, 4, 1300]` = "2000
/// total inhabitants, of which 1300 must be Aristocrat tier".
/// Plague `[5000, 0, 0]` = "5000 total, no tier requirement".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PopulationGoal {
    /// Total population threshold for this goal slot.
    pub total: u32,
    /// Tier index (1..=4 ⇒ Settler..Aristocrat). `None` for
    /// "any tier" — the original encodes that as raw 0 in u32 1.
    pub tier: Option<u8>,
    /// How many of `total` must be at `tier`. When `tier` is
    /// `None` this is just the population subtotal echo.
    pub at_tier: u32,
}

/// Decoded mission-goal numbers. Each field is `Option<…>`
/// because not every flag bit is set in every scenario; callers
/// should read these together with `Mission::flags`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MissionGoals {
    /// Primary population goal (`MISSION_FLAG_POPULATION`)
    /// — triple at goals_raw u32 0..=2.
    pub primary: Option<PopulationGoal>,
    /// Secondary population goal (`MISSION_FLAG_POPULATION2`)
    /// — triple at goals_raw u32 3..=5. Typically a second
    /// settlement the player must establish.
    pub secondary: Option<PopulationGoal>,
    /// Tertiary population goal (`MISSION_FLAG_POPULATION3`)
    /// — triple at goals_raw u32 6..=8. Used by The Continent
    /// and New Horizons1 (three cities of 5000 / 500).
    pub tertiary: Option<PopulationGoal>,
    /// Cooperative neighbour-population requirement
    /// (`MISSION_FLAG_COOPERATIVE`). Stored at goals_raw u32 18
    /// (chunk offset 0x8B8). Good Neighbors / New Horizons2 /
    /// Alliance all use this slot.
    pub cooperative_population: Option<u32>,
    /// Tier index the cooperative neighbour must reach. Stored at
    /// goals_raw u32 19. The Alliance / New Horizons2 pin this
    /// to 3 (Merchant); Good Neighbors leaves it at 0.
    pub cooperative_tier: Option<u8>,
}

/// Convenience accessor on `MissionGoals` for the legacy callers
/// that just want the primary total.
impl MissionGoals {
    pub fn primary_population(&self) -> Option<u32> {
        self.primary.map(|p| p.total)
    }
    pub fn primary_tier(&self) -> Option<u8> {
        self.primary.and_then(|p| p.tier)
    }
}
const AUFTRAG4_TOTAL_BYTES: usize = 2244;

/// One player-slot record parsed from the SZS PLAYER4 chunk.
/// 1072 bytes per slot in the original (= 0xa0 stride confirmed
/// from `1602_exe.c` `&DAT_005b7680`). We only extract fields
/// we know how to interpret; the raw blob is preserved on the
/// `Chunk` for callers that need more.
///
/// Cross-scenario sample (A Plague of Pirates / Atoll /
/// Competition / Continuous Play 00-02) confirms a consistent
/// per-slot layout. Slots 4..=6 are reserved for special
/// factions: slot 4 = free trader (1 000 000 gold, matching
/// `1602_exe.c:83179` `s_Trader_d`), slot 5 = native faction
/// (50 000), slot 6 = pirates (5 000).
#[derive(Debug, Clone, Default)]
pub struct PlayerSlotInit {
    /// Starting gold (first u32 of the slot record).
    pub starting_gold: i32,
    /// Faction-state code at byte offset 4 of the slot record.
    /// Cross-scenario sample (six shipping `.szs` files) gives a
    /// stable per-slot mapping:
    ///
    ///   0x00 → human (slot 0 in every scenario)
    ///   0x0c → AI rival (slots 1..=3)
    ///   0x0d → free trader (slot 4)
    ///   0x0e → natives (slot 5)
    ///   0x0b → pirates (slot 6)
    ///
    /// These are FACTION-KIND codes, not the post-load runtime
    /// `PlayerState` (whose `Empty` / `Defeated` values may share
    /// numeric encoding with `0x0e` etc. but appear in a different
    /// context). Stored raw; downstream code can interpret.
    pub state_byte: u8,
    /// Colour / portrait index at byte offset 7 of the slot record.
    /// Slots 0..=3 carry the player-chosen colour (typically 0
    /// for slot 0 in single-player templates); slots 4..=6 carry
    /// the reserved-faction portraits — trader 6, native 4,
    /// pirate 5.
    pub color_idx: u8,
    /// Raw value of byte 12 of the slot record. Cross-scenario
    /// sample shows this isn't a clean `is_human` flag — Atoll
    /// (single-player free-for-all) sets it to `0x00` for slots
    /// 0 / 4 / 5 / 6 and `0xff` for unused AI slots, while
    /// Tutorial0 leaves every slot at `0xff` and Plague of
    /// Pirates puts `0x00` on slots 0..=3. Most plausible
    /// interpretation: `0x00` = "scenario explicitly configured
    /// this slot" / `0xff` = default fill — but the binary's
    /// PLAYER4 reader hasn't been traced so the semantic stays
    /// raw for now.
    pub slot_byte12: u8,
    /// Whether `1602_exe.c::FUN_00473c50` will populate this
    /// slot at scenario load time. The decompiled chunk reader
    /// at line 82622 includes a slot only when:
    ///
    /// ```text
    /// state_byte == 0x00                  // human / special
    ///   OR (state_byte == 0x0c            // AI rival
    ///       AND slot_byte_0x0d == 0x00)
    /// ```
    ///
    /// `slot_byte_0x0d == 0x01` therefore means "AI rival pre-
    /// configured but disabled in the scenario." The audit run
    /// counts 21 such slots across the shipping corpus (Exile,
    /// New Horizons2, etc.). For non-AI slots the byte is
    /// always 0, so this flag effectively gates AI rivals
    /// only.
    pub ai_active: bool,
    /// Player display name. Verified at byte offset 0x3C0 of
    /// each slot record (a 16-byte CP1252 null-terminated field):
    /// Tutorial0 / Cooperation = "Wilfried" (the default German
    /// male player name); Atoll = "Namenlos" (German for
    /// "Nameless"); custom-named scenarios store whatever the
    /// editor was asked to call the player.
    pub name: String,
    /// Little-endian u32 starting at byte offset 0x34 of the slot
    /// record. Cross-scenario sample (60 shipping `.szs` files via
    /// `cargo run --example audit_player4_bytes`):
    ///
    ///   * Plague-of-Pirates scripts always assign 0x0000_0003 to
    ///     the AI rivals (slots 1..=3) and 0 to the special
    ///     factions (slots 4..=6).
    ///   * Continuous-Play / Tutorial0 leave every active slot at
    ///     0 and clamp the unused native + pirate slots (5, 6) to
    ///     0xFFFF_FFFF.
    ///   * Difficulty-tiered scripts grow the mask with the AI
    ///     index — Magnate0 has slot 0 = 0x0000_0003, slot 1 =
    ///     0x003F_C00F, slots 2/3 = 0x0FFF_C33F, suggesting a
    ///     bitset that widens for stronger opponents.
    ///   * Cooperation / Good Neighbors set the same mask on
    ///     every team slot (0x003F_C00F and 0x007F_CFFF
    ///     respectively), so the field is per-slot, not per-side.
    ///
    /// Binary semantics aren't yet RE'd from `1602_exe.c`; the
    /// raw u32 is exposed so callers can correlate it with
    /// observed AI behaviour (e.g. a tech-unlock mask).
    pub slot_u32_0x34: u32,
    /// PLAYER4 u16 `+0x9c`, the upper bound applied by
    /// `FUN_00423710` to this player's controller figure capacity.
    pub controller_figure_capacity_limit_0x9c: u16,
    /// Little-endian u16 at slot offset 0x18..0x1A.
    /// `1602_exe.c::FUN_00478160:85417` reads/writes this as a
    /// `*(u16*)` — the high byte is always 0 in shipping
    /// content, so the effective range is 0x0000..0x0007. Audit
    /// (62 shipping `.szs` × 7 slots = 434 samples) finds 21
    /// scenarios with non-zero values per slot:
    ///
    /// ```text
    /// Atoll                    [1, 0, 0, 0, 0, 0, 0]
    /// Exile                    [1, 1, 1, 1, 0, 0, 0]
    /// New Horizons0            [2, 1, 0, 0, 0, 0, 0]
    /// On His Majesty's Service0 [0, 6, 2, 0, 0, 0, 0]
    /// The Magnate2             [2, 5, 6, 6, 0, 0, 0]
    /// Trust no one2            [3, 0, 7, 5, 0, 7, 0]
    /// ```
    ///
    /// The values track per-scenario AI difficulty (Magnate2 is
    /// the hardest "Magnate" tier and carries the largest
    /// numbers on its rivals), so this is most likely an AI
    /// personality / portrait index. Concrete semantics aren't
    /// pinned to a binary function yet, so the raw u16 is
    /// exposed for downstream callers.
    pub slot_u16_0x18: u16,
    /// Runtime player dword `DAT_005b76f0`, saved at PLAYER4 offset `0x1c`.
    /// `FUN_00475c60` enables its four population and city-strength policy
    /// deductions through bits `0x10`, `0x80`, `0x100`, and `0x200`.
    pub diplomacy_policy_flags_0x1c: u32,
    /// PLAYER4 u16 `+0x20`, copied from `DAT_005b7720` by
    /// `FUN_00478160`. The population policy at `FUN_00475c60` compares it
    /// with the peer's total residents.
    pub diplomacy_peer_population_threshold_0x20: u16,
    /// PLAYER4 u16 `+0x22`, copied from `DAT_005b7722`. The corresponding
    /// policy compares it with this player's total residents.
    pub diplomacy_own_population_threshold_0x22: u16,
    /// PLAYER4 byte `+0x24`, copied from `DAT_005b76ff`. This is the
    /// player-city strength target used by policy bit `0x100`.
    pub diplomacy_own_city_strength_0x24: u8,
    /// PLAYER4 byte `+0x25`, copied from `DAT_005b7700`. This is the
    /// peer-city strength target used by policy bit `0x200`.
    pub diplomacy_peer_city_strength_0x25: u8,
    /// Seven u16 values at slot offsets `0x40, 0x42, ..., 0x4c`.
    /// `FUN_00478160` copies these from runtime `DAT_005b7730`; the
    /// `FUN_00475c60` diplomacy score adds the directed value to its caller
    /// contribution before applying the population curve.
    pub diplomacy_base_0x40: [u16; 7],
    /// Seven u16 values at slot offsets `0x60, 0x62, ..., 0x6c`.
    /// `FUN_00478160` copies these from `DAT_005b7740`; `FUN_00475c60`
    /// applies the same population curve to this directed scale term.
    pub diplomacy_scale_0x60: [u16; 7],
    /// Seven u32 values at slot offsets `0x80, 0x84, ..., 0x98`
    /// (stride 4, contiguous — verified against both the loader
    /// `FUN_00477912` and the writer `FUN_00478160`, which each
    /// walk this block one dword at a time). These are the
    /// directed runtime `DAT_005b7750` activity counters used by
    /// `FUN_00477390` and deducted by `FUN_00475c60`. Uniformly
    /// zero across the shipping corpus (runtime state, not
    /// authored).
    pub diplomacy_activity_0x80: [u32; 7],
    /// Seven u32 values at slot offsets 0xC0, 0xC8, … 0xF0
    /// (stride 8; padding +4 uniformly zero). `FUN_00478160`
    /// copies these from the directed `DAT_005b7770` table, which
    /// `FUN_0045cd20` reads to exclude a candidate only for code
    /// `3`. Cross-scenario audit surfaces a similar 0/3 pattern to
    /// `relationships` but with a different masking — Tutorial0
    /// slot 0 has `[3, 3, 3, 3, 3, 0, 3]` here versus
    /// `[0, 0, 0, 0, 3, 3, 3]` in `relationships`.
    pub relations_0xc0: [u32; 7],
    /// Seven u32 values at slot offsets 0x1C0, 0x1C8, … 0x1F0
    /// (stride 8). Sourced from the runtime player struct's
    /// `+0x170` array in `1602_exe.c::FUN_00478160:85447`. The
    /// values are NOT 0/3 like the other tables — Magnate0
    /// rival N (1..=3) has N entries of `(N << 8) | 2`
    /// against rivals 1..=N (so AI 1 holds `[0x102]`, AI 2
    /// holds `[0x102, 0x202]`, AI 3 holds
    /// `[0x102, 0x202, 0x302]`). The pirate slot (6) carries
    /// a distinct `[0x301, 0x303, …]` encoding. Suggests an
    /// `(slot << 8) | event_type` per-slot event log.
    /// Tutorial0 leaves it all-zero.
    pub events_0x1c0: [u32; 7],
    /// Seven u32 values at slot offsets 0x140, 0x148, … 0x170.
    /// `FUN_00478160` copies these from `DAT_005b77b0`, the
    /// directed attitude table handled by `FUN_00476130` / event
    /// `0x30`. The upper four bytes between each element are
    /// uniformly zero across all 434 surveyed slots.
    ///
    /// Cross-scenario pattern (Tutorial0 / Plague / Atoll all
    /// agree, with Magnate0 modulating only the AI rows):
    ///
    /// ```text
    /// row 0 (player):  [0, 0, 0, 0, 3, 3, 3]
    /// row 1..=3 (AIs): [0, 0, 0, 0, 3, 3, 3]   // same as player
    /// row 4 (trader):  [3, 3, 3, 3, 0, 0, 0]
    /// row 5 (natives): [0, 0, 0, 0, 0, 3, 0]   // only self
    /// row 6 (pirates): [3, 3, 3, 3, 0, 0, 0]   // same as trader
    /// ```
    ///
    /// Values are limited to 0 or 3 in the shipping content;
    /// concrete diplomacy semantics ("0 = at war", "3 = neutral
    /// pact") aren't pinned to a binary function yet, so the
    /// raw u32 array is exposed for downstream interpretation.
    pub relationships: [u32; 7],
}

const PLAYER4_NAME_OFFSET: usize = 0x3C0;
const PLAYER4_NAME_BYTES: usize = 16;

const PLAYER4_SLOT_BYTES: usize = 1072;
const PLAYER4_MAX_SLOTS: usize = 7;

const CHUNK_HEADER_SIZE: usize = 20;

/// Write a single chunk (16-byte zero-padded name + 4-byte LE size + body).
fn write_chunk(out: &mut Vec<u8>, name: &str, body: &[u8]) {
    let mut name_bytes = [0u8; 16];
    let bytes = name.as_bytes();
    let n = bytes.len().min(16);
    name_bytes[..n].copy_from_slice(&bytes[..n]);
    out.extend_from_slice(&name_bytes);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
}

impl SzsFile {
    /// Return the u16 installed at island-runtime offset `+0x18` by the
    /// `INSEL5` loader at `0x00469d60`. The loader overwrites the serialized
    /// word at `0x62` for widths through `0x6e` before copying it into the
    /// runtime record.
    pub fn island_source_runtime_classification(&self, island_index: usize) -> u16 {
        let Some(data) = self
            .chunks
            .iter()
            .filter(|chunk| chunk.name == "INSEL5" && chunk.data.len() >= 8)
            .nth(island_index)
            .map(|chunk| chunk.data.as_slice())
        else {
            return 0;
        };

        let serialized = data
            .get(0x62..0x64)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .unwrap_or(0);
        source_runtime_island_classification(data[1], serialized)
    }

    /// Decode the resource inputs paired with the `island_index`th parsed
    /// `INSEL5` island. Short editor-generated records carry the zero state.
    pub fn island_source_resource_state(&self, island_index: usize) -> IslandSourceResourceState {
        let Some(data) = self
            .chunks
            .iter()
            .filter(|chunk| chunk.name == "INSEL5" && chunk.data.len() >= 8)
            .nth(island_index)
            .map(|chunk| chunk.data.as_slice())
        else {
            return IslandSourceResourceState::default();
        };

        let mut state = IslandSourceResourceState::default();
        if data.len() >= 0x1b {
            state.record_count = data[0x1a].min(8);
        }
        for (index, record) in state.records.iter_mut().enumerate() {
            // The loader (`0x0046a004`) assembles each runtime record from two
            // four-byte serialized halves: the selector word at `0x1c + 8r`
            // (runtime `+0x20`) and the availability word at `0x28 + 8r`
            // (runtime `+0x24`). The two halves overlap adjacent records in the
            // packed chunk, so they cannot be read as one contiguous slice.
            let selector_offset = 0x1c + index * 8;
            let availability_offset = 0x28 + index * 8;
            if data.len() >= selector_offset + 4 {
                record.raw[0..4].copy_from_slice(&data[selector_offset..selector_offset + 4]);
            }
            if data.len() >= availability_offset + 4 {
                record.raw[4..8].copy_from_slice(&data[availability_offset..availability_offset + 4]);
            }
        }
        if data.len() >= 0x60 {
            state.crop_flags = u32::from_le_bytes(data[0x5c..0x60].try_into().expect("slice size"));
        }
        if data.len() > 0x64 {
            state.parity = data[0x64];
        }
        if data.len() > 0x66 {
            state.attenuation = data[0x66];
        }
        if data.len() >= 0x70 {
            state.transition_deadline_ticks =
                u32::from_le_bytes(data[0x6c..0x70].try_into().expect("slice size"));
        }
        state
    }

    pub fn parse(data: &[u8]) -> Result<Self, SzsError> {
        if data.len() < CHUNK_HEADER_SIZE {
            return Err(SzsError::TooSmall);
        }

        let mut chunks = Vec::new();
        let mut pos = 0;

        while pos + CHUNK_HEADER_SIZE <= data.len() {
            // Read 16-byte name
            let name_bytes = &data[pos..pos + 16];
            let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(16);
            let name = match std::str::from_utf8(&name_bytes[..name_end]) {
                Ok(s)
                    if !s.is_empty()
                        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
                {
                    s.to_string()
                }
                _ => {
                    pos += 1;
                    continue;
                }
            };

            // Read 4-byte size
            let size = u32::from_le_bytes([
                data[pos + 16],
                data[pos + 17],
                data[pos + 18],
                data[pos + 19],
            ]) as usize;

            if pos + CHUNK_HEADER_SIZE + size > data.len() {
                break;
            }

            let chunk_data = data[pos + CHUNK_HEADER_SIZE..pos + CHUNK_HEADER_SIZE + size].to_vec();
            chunks.push(Chunk {
                name,
                data: chunk_data,
            });

            pos += CHUNK_HEADER_SIZE + size;
        }

        // Extract islands by pairing INSEL5 + INSELHAUS chunks
        let mut islands = Vec::new();
        let mut i = 0;
        while i < chunks.len() {
            if chunks[i].name == "INSEL5" && chunks[i].data.len() >= 8 {
                let mut island = Self::parse_insel5(&chunks[i].data);

                // Look for the matching INSELHAUS / STADT4 chunks
                // (follow INSEL5, possibly with other chunks in
                // between for the same island).
                for j in (i + 1)..chunks.len() {
                    match chunks[j].name.as_str() {
                        "INSELHAUS" => island.tiles = Self::parse_inselhaus(&chunks[j].data),
                        "STADT4" => island.city = Self::parse_stadt4(&chunks[j].data),
                        "INSEL5" => break, // next island
                        _ => {}
                    }
                }

                islands.push(island);
            }
            i += 1;
        }

        // Extract per-slot player init from the PLAYER4 chunk.
        let players = chunks
            .iter()
            .find(|c| c.name == "PLAYER4")
            .map(|c| Self::parse_player4(&c.data))
            .unwrap_or_default();

        let mission = chunks
            .iter()
            .find(|c| c.name == "AUFTRAG4")
            .and_then(|c| Self::parse_auftrag4(&c.data));

        let read_u32 = |name: &str| -> Option<u32> {
            chunks
                .iter()
                .find(|c| c.name == name)
                .filter(|c| c.data.len() >= 4)
                .map(|c| u32::from_le_bytes([c.data[0], c.data[1], c.data[2], c.data[3]]))
        };
        let scenario = ScenarioMeta {
            mission_nr: read_u32("SZENE_MISSNR"),
            player_min: read_u32("SZENE_PLAYERMIN"),
            player_max: read_u32("SZENE_PLAYERMAX"),
            ranking: read_u32("SZENE_RANKING"),
        };

        let ships = chunks
            .iter()
            .find(|c| c.name == "SHIP4")
            .map(|c| Self::parse_ship4(&c.data))
            .unwrap_or_default();

        let land_figures = chunks
            .iter()
            .find(|c| c.name == "SOLDAT3")
            .map(|c| Self::parse_soldat3(&c.data))
            .unwrap_or_default();

        Ok(SzsFile {
            chunks,
            islands,
            players,
            mission,
            scenario,
            ships,
            land_figures,
        })
    }

    fn parse_soldat3(data: &[u8]) -> Vec<LandFigure> {
        data.chunks_exact(SOLDAT3_RECORD_BYTES)
            .map(|record| {
                let mut raw_record = [0u8; SOLDAT3_RECORD_BYTES];
                raw_record.copy_from_slice(record);
                LandFigure {
                    raw_record,
                    x: u16::from_le_bytes([record[0x00], record[0x01]]),
                    y: u16::from_le_bytes([record[0x02], record[0x03]]),
                    source_energy: u16::from_le_bytes([record[0x04], record[0x05]]),
                    figure_definition_id: u16::from_le_bytes([record[0x06], record[0x07]]),
                    runtime_slot: u16::from_le_bytes([record[0x08], record[0x09]]),
                    origin_descriptor: record[0x0a..0x0e]
                        .try_into()
                        .expect("SOLDAT3 origin descriptor has fixed width"),
                    route_radius: record[0x0b],
                    figure_kind: record[0x16],
                    island_id: record[0x17],
                    owner: record[0x18],
                    direction: record[0x1b],
                    animation_state: record[0x19],
                    state_selector: record[0x1c],
                    state_descriptor: record[0x12..0x16]
                        .try_into()
                        .expect("SOLDAT3 state descriptor has fixed width"),
                    state_flags: record[0x1d] & 3,
                    state_payload: record[0x1e..0x26]
                        .try_into()
                        .expect("SOLDAT3 state payload has fixed width"),
                }
            })
            .collect()
    }

    fn parse_ship4(data: &[u8]) -> Vec<Ship> {
        let count = data.len() / SHIP4_RECORD_BYTES;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let off = i * SHIP4_RECORD_BYTES;
            let mut raw_record = [0u8; SHIP4_RECORD_BYTES];
            raw_record.copy_from_slice(&data[off..off + SHIP4_RECORD_BYTES]);
            let name_bytes = &data[off..off + SHIP4_NAME_BYTES];
            let name_end = name_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(SHIP4_NAME_BYTES);
            let name: String = name_bytes[..name_end]
                .iter()
                .map(|&b| char::from(b))
                .collect();
            let x = u16::from_le_bytes([data[off + 28], data[off + 29]]);
            let y = u16::from_le_bytes([data[off + 30], data[off + 31]]);
            let stored_energy = u16::from_le_bytes([data[off + 0x3c], data[off + 0x3d]]);
            let runtime_slot = u16::from_le_bytes([data[off + 0x46], data[off + 0x47]]);
            let figure_definition_id = u16::from_le_bytes([data[off + 0x48], data[off + 0x49]]);
            let ship_class = figure_definition_id as u8;
            let figure_kind = if off + 0x4b <= data.len() {
                data[off + 0x4a]
            } else {
                0
            };
            let owner = if off + 0x4C <= data.len() {
                data[off + 0x4B]
            } else {
                0
            };
            let animation_state = if off + 0x4f <= data.len() {
                data[off + 0x4e]
            } else {
                0
            };
            let candidate_list_key = if off + 0x4e <= data.len() {
                data[off + 0x4d]
            } else {
                0
            };
            let source_direction = if off + 0x51 <= data.len() {
                data[off + 0x50]
            } else {
                0
            };
            let heading_byte = if off + 0x43 <= data.len() {
                data[off + 0x42]
            } else {
                0
            };
            let mut cargo_slots = [0u32; 7];
            for (i, slot) in cargo_slots.iter_mut().enumerate() {
                // The packed cargo entry begins at record offset 0x175 (stride
                // 8). Decoded as `FUN_00448120` does — ware in the low byte,
                // the 1/32-good quantity in bits 8..=21 — this yields valid
                // wares and 32-aligned quantities; reading a byte early (0x174)
                // shifts an invalid ware into the low byte and corrupts both.
                let o = off + 0x175 + i * 8;
                if o + 4 <= data.len() {
                    *slot = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                }
            }
            out.push(Ship {
                raw_record,
                name,
                x,
                y,
                owner,
                figure_definition_id,
                ship_class,
                stored_energy,
                runtime_slot,
                figure_kind,
                candidate_list_key,
                source_direction,
                animation_state,
                heading_byte,
                cargo_slots,
            });
        }
        out
    }

    fn parse_auftrag4(data: &[u8]) -> Option<Mission> {
        if data.len() < AUFTRAG4_TOTAL_BYTES {
            return None;
        }
        let flags = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);

        // Briefing text: CP1252, null-terminated, starts at 0x68.
        let text_start = AUFTRAG4_BRIEFING_OFFSET;
        let text_end = data[text_start..AUFTRAG4_GOALS_OFFSET]
            .iter()
            .position(|&b| b == 0)
            .map(|n| text_start + n)
            .unwrap_or(AUFTRAG4_GOALS_OFFSET);
        let briefing: String = data[text_start..text_end]
            .iter()
            .map(|&b| char::from(b))
            .collect();

        let goals_raw = data[AUFTRAG4_GOALS_OFFSET..AUFTRAG4_TOTAL_BYTES].to_vec();
        Some(Mission {
            flags,
            briefing,
            goals_raw,
        })
    }

    fn parse_player4(data: &[u8]) -> Vec<PlayerSlotInit> {
        let mut out = Vec::new();
        for slot in 0..PLAYER4_MAX_SLOTS {
            let off = slot * PLAYER4_SLOT_BYTES;
            if off + 16 > data.len() {
                break;
            }
            let starting_gold =
                i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
            // Byte 12 = 0x00 (active player / fixed faction) vs
            // 0xff (slot inactive — AI fills it on game start).
            let state_byte = data[off + 4];
            let color_idx = data[off + 7];
            let slot_byte12 = data[off + 12];
            // 1602_exe.c FUN_00473c50:82622 includes the slot
            // only when byte 0x0d is 0 for AI rivals (state_byte
            // == 0x0c). For human / special-faction slots the
            // gate doesn't apply, so we report `true` there.
            let ai_active = if state_byte == 0x0c {
                data[off + 13] == 0x00
            } else {
                true
            };
            let slot_u16_0x18 = if off + 0x1A <= data.len() {
                u16::from_le_bytes([data[off + 0x18], data[off + 0x19]])
            } else {
                0
            };
            let slot_u32_0x34 = if off + 0x38 <= data.len() {
                u32::from_le_bytes([
                    data[off + 0x34],
                    data[off + 0x35],
                    data[off + 0x36],
                    data[off + 0x37],
                ])
            } else {
                0
            };
            let read_u16 = |start: usize| -> u16 {
                let o = off + start;
                if o + 2 <= data.len() {
                    u16::from_le_bytes([data[o], data[o + 1]])
                } else {
                    0
                }
            };
            let read_u32 = |start: usize| -> u32 {
                let o = off + start;
                if o + 4 <= data.len() {
                    u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
                } else {
                    0
                }
            };
            let read_array = |start: usize| -> [u32; 7] {
                let mut arr = [0u32; 7];
                for (i, slot_val) in arr.iter_mut().enumerate() {
                    let o = off + start + i * 8;
                    if o + 4 <= data.len() {
                        *slot_val =
                            u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                    }
                }
                arr
            };
            let read_u16_array = |start: usize| -> [u16; 7] {
                let mut arr = [0u16; 7];
                for (i, slot_val) in arr.iter_mut().enumerate() {
                    let o = off + start + i * 2;
                    if o + 2 <= data.len() {
                        *slot_val = u16::from_le_bytes([data[o], data[o + 1]]);
                    }
                }
                arr
            };
            let diplomacy_base_0x40 = read_u16_array(0x40);
            let diplomacy_scale_0x60 = read_u16_array(0x60);
            // Both the loader (`FUN_00477912`, spilled pointer `[esp+0x1c]`
            // advanced by 4) and the writer (`FUN_00478160`, output cursor
            // `[esp+0x14]` advanced by 4) lay this array out as seven
            // contiguous u32 at 0x80..0x9C — stride 4, not the stride 8 used
            // by the 0xC0/0x140/0x1C0 tables.
            let diplomacy_activity_0x80 = {
                let mut arr = [0u32; 7];
                for (i, slot_val) in arr.iter_mut().enumerate() {
                    let o = off + 0x80 + i * 4;
                    if o + 4 <= data.len() {
                        *slot_val =
                            u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                    }
                }
                arr
            };
            let relations_0xc0 = read_array(0xC0);
            let relationships = read_array(0x140);
            let events_0x1c0 = read_array(0x1C0);
            let name_off = off + PLAYER4_NAME_OFFSET;
            let name = if name_off + PLAYER4_NAME_BYTES <= data.len() {
                let name_bytes = &data[name_off..name_off + PLAYER4_NAME_BYTES];
                let end = name_bytes
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(PLAYER4_NAME_BYTES);
                name_bytes[..end].iter().map(|&b| char::from(b)).collect()
            } else {
                String::new()
            };
            out.push(PlayerSlotInit {
                starting_gold,
                state_byte,
                color_idx,
                slot_byte12,
                ai_active,
                name,
                slot_u32_0x34,
                controller_figure_capacity_limit_0x9c: read_u16(0x9c),
                relations_0xc0,
                relationships,
                events_0x1c0,
                slot_u16_0x18,
                diplomacy_policy_flags_0x1c: read_u32(0x1c),
                diplomacy_peer_population_threshold_0x20: read_u16(0x20),
                diplomacy_own_population_threshold_0x22: read_u16(0x22),
                diplomacy_own_city_strength_0x24: data.get(off + 0x24).copied().unwrap_or(0),
                diplomacy_peer_city_strength_0x25: data.get(off + 0x25).copied().unwrap_or(0),
                diplomacy_base_0x40,
                diplomacy_scale_0x60,
                diplomacy_activity_0x80,
            });
        }
        out
    }

    /// Encode an SZS file from a list of `Island`s. Generates one
    /// `INSEL5` + `INSELHAUS` chunk pair per island. The result round-
    /// trips through `SzsFile::parse` for the islands payload (other
    /// chunks aren't reconstructed since this writer is intended for the
    /// scenario-editor flow, not full save fidelity).
    pub fn encode_islands(islands: &[Island]) -> Vec<u8> {
        let mut out = Vec::new();
        for island in islands {
            // INSEL5 chunk: 8-byte body matching the parser.
            let mut body = Vec::with_capacity(8);
            body.push(island.number);
            body.push(island.width);
            body.push(island.height);
            body.push(0); // padding byte
            body.extend_from_slice(&island.x_pos.to_le_bytes());
            body.extend_from_slice(&island.y_pos.to_le_bytes());
            write_chunk(&mut out, "INSEL5", &body);

            // INSELHAUS chunk: tile records.
            let mut tile_body = Vec::with_capacity(island.tiles.len() * 8);
            for t in &island.tiles {
                tile_body.extend_from_slice(&t.building_id.to_le_bytes());
                tile_body.push(t.x);
                tile_body.push(t.y);
                tile_body.push(t.orientation);
                tile_body.push(t.anim_count);
                tile_body.extend_from_slice(&t.flags.to_le_bytes());
            }
            write_chunk(&mut out, "INSELHAUS", &tile_body);
        }
        out
    }

    fn parse_insel5(data: &[u8]) -> Island {
        let mut fertilities = [0x07u8; 8];
        if data.len() >= 0x14 {
            fertilities.copy_from_slice(&data[0x0C..0x14]);
        }
        Island {
            number: data[0],
            width: data[1],
            height: data[2],
            x_pos: u16::from_le_bytes([data[4], data[5]]),
            y_pos: u16::from_le_bytes([data[6], data[7]]),
            fertilities,
            tiles: Vec::new(),
            city: None,
        }
    }

    fn parse_stadt4(data: &[u8]) -> Option<City> {
        if data.len() < 0xa8 {
            return None;
        }
        let island_index = data[0];
        let owner_slot = data[2];
        let read_u32 = |off: usize| {
            u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
        };
        // The city loader `FUN_00484af0` copies five per-tier population
        // dwords from record offsets 0x5C..0x70 (stride 4) into the runtime
        // city record at `+0x220` (`0x00484c65..0x00484cbb`, edi = record + 1
        // + 0x5b). Reading from 0x60 drops the first tier and appends a
        // spurious trailing zero.
        let tier_population = [
            read_u32(0x5C),
            read_u32(0x60),
            read_u32(0x64),
            read_u32(0x68),
            read_u32(0x6C),
        ];
        let name_start = 0x87;
        let name_end = data[name_start..]
            .iter()
            .position(|&b| b == 0)
            .map(|n| name_start + n)
            .unwrap_or(data.len());
        let name: String = data[name_start..name_end]
            .iter()
            .map(|&b| char::from(b))
            .collect();
        Some(City {
            island_index,
            owner_slot,
            tier_population,
            name,
        })
    }

    fn parse_inselhaus(data: &[u8]) -> Vec<IslandTile> {
        let record_size = 8;
        let count = data.len() / record_size;
        let mut tiles = Vec::with_capacity(count);

        let mut cursor = Cursor::new(data);
        for _ in 0..count {
            let building_id = cursor.read_u16::<LittleEndian>().unwrap_or(0);
            let x = cursor.read_u8().unwrap_or(0);
            let y = cursor.read_u8().unwrap_or(0);
            let orientation = cursor.read_u8().unwrap_or(0);
            let anim_count = cursor.read_u8().unwrap_or(0);
            let flags = cursor.read_u16::<LittleEndian>().unwrap_or(0);

            tiles.push(IslandTile {
                building_id,
                x,
                y,
                orientation,
                anim_count,
                flags,
            });
        }

        tiles
    }
}

/// Exact `INSEL5` classification rewrite at `0x00469e50`.
pub const fn source_runtime_island_classification(island_width: u8, serialized: u16) -> u16 {
    match island_width {
        0..=0x20 => 0,
        0x21..=0x2a => 1,
        0x2b..=0x37 => 2,
        0x38..=0x4b => 3,
        0x4c..=0x6e => 4,
        _ => serialized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soldat3_records_preserve_source_kind_owner_and_island() {
        let mut record = [0u8; SOLDAT3_RECORD_BYTES];
        record[0x00..0x02].copy_from_slice(&0x1234u16.to_le_bytes());
        record[0x02..0x04].copy_from_slice(&0x5678u16.to_le_bytes());
        record[0x04..0x06].copy_from_slice(&0x0011u16.to_le_bytes());
        record[0x06..0x08].copy_from_slice(&0x1f0au16.to_le_bytes());
        record[0x08..0x0a].copy_from_slice(&0x0042u16.to_le_bytes());
        record[0x0a..0x0e].copy_from_slice(&[0x33, 9, 8, 7]);
        record[0x16] = 4;
        record[0x17] = 9;
        record[0x18] = 3;
        record[0x19] = 5;
        record[0x12..0x16].copy_from_slice(&[9, 8, 7, 6]);
        record[0x1b] = 7;
        record[0x1c] = 1;
        record[0x1d] = 0xfe;
        record[0x1e..0x26].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

        let mut bytes = Vec::new();
        write_chunk(&mut bytes, "SOLDAT3", &record);
        let parsed = SzsFile::parse(&bytes).expect("SOLDAT3 chunk parses");

        assert_eq!(parsed.land_figures.len(), 1);
        let figure = &parsed.land_figures[0];
        assert_eq!((figure.x, figure.y), (0x1234, 0x5678));
        assert_eq!(figure.source_energy, 0x0011);
        assert_eq!(figure.figure_definition_id, 0x1f0a);
        assert_eq!(figure.runtime_slot, 0x0042);
        assert_eq!(figure.origin_descriptor, [0x33, 9, 8, 7]);
        assert_eq!(figure.route_radius, 9);
        assert_eq!(figure.figure_kind, 4);
        assert_eq!(figure.island_id, 9);
        assert_eq!(figure.owner, 3);
        assert_eq!(figure.direction, 7);
        assert_eq!(figure.animation_state, 5);
        assert_eq!(figure.state_selector, 1);
        assert_eq!(figure.state_descriptor, [9, 8, 7, 6]);
        assert_eq!(figure.state_flags, 2);
        assert_eq!(figure.state_payload, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(figure.raw_record, record);
    }

    #[test]
    fn land_figure_definition_decodes_executable_type_four_table() {
        let cases = [
            (1, LandFigureFamily::Infantry, 1, "SOLDAT1"),
            (2, LandFigureFamily::Infantry, 2, "SOLDAT2"),
            (3, LandFigureFamily::Infantry, 3, "SOLDAT3"),
            (4, LandFigureFamily::Infantry, 4, "SOLDAT4"),
            (5, LandFigureFamily::Cavalry, 1, "KAVALERIE1"),
            (6, LandFigureFamily::Cavalry, 2, "KAVALERIE2"),
            (7, LandFigureFamily::Cavalry, 3, "KAVALERIE3"),
            (8, LandFigureFamily::Cavalry, 4, "KAVALERIE4"),
            (9, LandFigureFamily::Musketeer, 1, "MUSKETIER1"),
            (10, LandFigureFamily::Musketeer, 2, "MUSKETIER2"),
            (11, LandFigureFamily::Musketeer, 3, "MUSKETIER3"),
            (12, LandFigureFamily::Musketeer, 4, "MUSKETIER4"),
            (13, LandFigureFamily::Cannoneer, 1, "KANONIER1"),
            (14, LandFigureFamily::Cannoneer, 2, "KANONIER2"),
            (15, LandFigureFamily::Cannoneer, 3, "KANONIER3"),
            (16, LandFigureFamily::Cannoneer, 4, "KANONIER4"),
            (33, LandFigureFamily::NativeSpearman, 1, "SPEER1"),
            (34, LandFigureFamily::NativeSpearman, 2, "SPEER2"),
            (35, LandFigureFamily::NativeSpearman, 3, "SPEER3"),
            (36, LandFigureFamily::NativeSpearman, 4, "SPEER4"),
        ];

        for (id, family, variant, name) in cases {
            let definition = LandFigureDefinition::from_id(id).expect("known type-4 ID");
            assert_eq!(definition.id, id);
            assert_eq!(definition.family, family);
            assert_eq!(definition.variant, variant);
            assert_eq!(definition.source_figure_name(), name);
        }
        assert_eq!(LandFigureDefinition::from_id(0), None);
        assert_eq!(LandFigureDefinition::from_id(17), None);
        assert_eq!(LandFigureDefinition::from_id(32), None);
        assert_eq!(LandFigureDefinition::from_id(37), None);
    }

    #[test]
    fn land_figure_motion_properties_match_authored_bases() {
        let cases = [
            (1, 260, 0, 1, 20, 60, 3, 0.8, 0.75, 1, 0.0),
            (5, 400, 1, 1, 18, 54, 4, 1.0, 0.75, 1, 0.0),
            (9, 210, 0, 2, 15, 45, 7, 2.0, 4.0, 8, 0.0),
            (13, 230, 2, 3, 12, 36, 21, 4.5, 7.0, 14, 0.0),
            (33, 280, 0, 1, 18, 54, 3, 0.8, 1.0, 2, 0.0),
        ];
        for (
            id,
            speed,
            speed_type,
            max_step_count,
            max_energy,
            runtime_energy,
            runtime_hit_points,
            runtime_work_time,
            shot_radius,
            runtime_shot_radius,
            turn_time,
        ) in cases
        {
            let definition = LandFigureDefinition::from_id(id).unwrap();
            assert_eq!(definition.source_move_speed(), speed);
            assert_eq!(definition.source_speed_type(), speed_type);
            assert_eq!(definition.source_max_step_count(), max_step_count);
            assert_eq!(definition.source_max_energy(), max_energy);
            assert_eq!(definition.source_runtime_energy_cap(), runtime_energy);
            assert_eq!(definition.source_runtime_hit_points(), runtime_hit_points);
            assert_eq!(definition.source_runtime_work_time(), runtime_work_time);
            assert_eq!(definition.source_shot_radius(), shot_radius);
            assert_eq!(definition.source_runtime_shot_radius(), runtime_shot_radius);
            assert_eq!(definition.source_turn_time(), turn_time);
        }
    }

    #[test]
    fn source_combat_definitions_match_the_compiled_figure_table() {
        let cases = [
            (1, "SOLDAT1", 60, 3, 1, 0.8, 6, None),
            (9, "MUSKETIER1", 45, 7, 8, 2.0, 7, None),
            (13, "KANONIER1", 36, 21, 14, 4.5, 6, None),
            (0x15, "HANDEL1", 150, 6, 14, 5.0, 10, Some(113)),
            (0x16, "HANDELD1", 150, 6, 14, 5.0, 10, Some(113)),
            (0x19, "KRIEG1", 195, 6, 14, 5.0, 10, Some(113)),
            (0x1c, "KRIEGD2", 360, 6, 14, 5.0, 10, Some(112)),
            (0x1d, "HANDLER", 285, 6, 14, 5.0, 10, Some(112)),
            (0x1f, "PIRAT", 285, 6, 14, 5.0, 10, Some(112)),
            (0x21, "SPEER1", 54, 3, 2, 0.8, 6, None),
            (0x25, "TRADER1", 42, 2, 8, 2.0, 6, None),
            (0x26, "KANONTURM", 72, 12, 15, 3.0, 10, Some(114)),
            (0x28, "PIRATTURM", 72, 12, 15, 3.0, 10, Some(115)),
        ];

        for (
            id,
            name,
            energy,
            hit_points,
            shot_radius,
            work_time,
            shot_delay_ticks,
            shot_figure_id,
        ) in cases
        {
            assert_eq!(
                SourceCombatDefinition::from_id(id),
                Some(SourceCombatDefinition {
                    id,
                    source_figure_name: name,
                    runtime_energy_cap: energy,
                    runtime_hit_points: hit_points,
                    runtime_shot_radius: shot_radius,
                    runtime_work_time: work_time,
                    runtime_shot_delay_ticks: shot_delay_ticks,
                    runtime_shot_figure_id: shot_figure_id,
                })
            );
        }
        assert_eq!(SourceCombatDefinition::from_id(0), None);
        assert_eq!(SourceCombatDefinition::from_id(0x27), None);
        assert_eq!(SourceCombatDefinition::from_id(0x77), None);
    }

    #[test]
    fn source_figure_purchase_cost_matches_compiled_preis_field() {
        assert_eq!(source_figure_purchase_price(0x15), 1_000);
        assert_eq!(source_figure_purchase_price(0x16), 1_000);
        assert_eq!(source_figure_purchase_price(0x1b), 2_400);
        assert_eq!(source_figure_purchase_price(0x20), 2_000);
        assert_eq!(source_figure_purchase_price(0x25), 0);
        assert_eq!(source_figure_purchase_cost(0x15, 150), 1_050);
        assert_eq!(source_figure_purchase_cost(0x1b, 360), 6_480);
    }

    #[test]
    fn source_shot_definitions_match_compiled_fahnoffs() {
        assert_eq!(
            SourceShotFigureDefinition::from_id(112),
            Some(SourceShotFigureDefinition {
                id: 112,
                source_figure_name: "KANONSHOT1",
                runtime_work_time: 0.96,
                runtime_fahnoffs_x: 0.5,
                runtime_fahnoffs_z: 4.0,
            })
        );
        assert_eq!(
            SourceShotFigureDefinition::from_id(113),
            Some(SourceShotFigureDefinition {
                id: 113,
                source_figure_name: "KANONSHOT2",
                runtime_work_time: 0.96,
                runtime_fahnoffs_x: 0.35,
                runtime_fahnoffs_z: 4.0,
            })
        );
        assert_eq!(
            SourceShotFigureDefinition::from_id(114),
            Some(SourceShotFigureDefinition {
                id: 114,
                source_figure_name: "KANONSHOTTURM",
                runtime_work_time: 0.96,
                runtime_fahnoffs_x: 0.5,
                runtime_fahnoffs_z: 2.0,
            })
        );
        assert_eq!(
            SourceShotFigureDefinition::from_id(115),
            Some(SourceShotFigureDefinition {
                id: 115,
                source_figure_name: "KANONSHOTTURM2",
                runtime_work_time: 0.96,
                runtime_fahnoffs_x: 0.5,
                runtime_fahnoffs_z: 0.8,
            })
        );
        assert_eq!(SourceShotFigureDefinition::from_id(111), None);
    }

    #[test]
    fn soldat3_world_positions_and_fixed_targets_stay_on_declared_islands() {
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            println!("Skipping: scenes dir not found");
            return;
        }

        let mut scenarios = 0;
        let mut figures = 0;
        let mut fixed_targets = 0;
        let mut fixed_target_rows = Vec::new();
        let mut native_idle_anchors = 0;
        for entry in std::fs::read_dir(&scenes)
            .unwrap()
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if !path
                .extension()
                .map(|extension| extension.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(parsed) => parsed,
                Err(_) => continue,
            };
            if parsed.land_figures.is_empty() {
                continue;
            }
            scenarios += 1;
            for figure in &parsed.land_figures {
                figures += 1;
                let island = parsed
                    .islands
                    .iter()
                    .find(|island| island.number == figure.island_id)
                    .unwrap_or_else(|| {
                        panic!(
                            "{:?}: type-4 figure slot {} references missing island {}",
                            path.file_name(),
                            figure.runtime_slot,
                            figure.island_id
                        )
                    });
                let local_x = i32::from(figure.x) / 2 - i32::from(island.x_pos);
                let local_y = i32::from(figure.y) / 2 - i32::from(island.y_pos);
                assert!(
                    (0..i32::from(island.width)).contains(&local_x)
                        && (0..i32::from(island.height)).contains(&local_y),
                    "{:?}: type-4 slot {} world ({}, {}) is outside island {} local {}x{} at ({}, {})",
                    path.file_name(),
                    figure.runtime_slot,
                    figure.x,
                    figure.y,
                    island.number,
                    island.width,
                    island.height,
                    local_x,
                    local_y
                );

                if figure.state_descriptor[0] == 0x38 {
                    fixed_targets += 1;
                    fixed_target_rows.push((
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .expect("scenario filename is UTF-8")
                            .to_owned(),
                        figure.runtime_slot,
                        figure.figure_definition_id,
                        figure.owner,
                        figure.direction,
                        figure.state_descriptor,
                    ));
                    let target_x = i32::from(
                        (u16::from(figure.state_descriptor[1] & 0x0f) << 8)
                            | u16::from(figure.state_descriptor[2]),
                    );
                    let target_y = i32::from(
                        (u16::from(figure.state_descriptor[1] >> 4) << 8)
                            | u16::from(figure.state_descriptor[3]),
                    );
                    let target_local_x = target_x / 2 - i32::from(island.x_pos);
                    let target_local_y = target_y / 2 - i32::from(island.y_pos);
                    assert!(
                        (0..i32::from(island.width)).contains(&target_local_x)
                            && (0..i32::from(island.height)).contains(&target_local_y),
                        "{:?}: type-4 slot {} fixed target ({target_x}, {target_y}) is outside island {} local ({target_local_x}, {target_local_y})",
                        path.file_name(),
                        figure.runtime_slot,
                        island.number,
                    );
                }

                if matches!(
                    figure.definition().map(|definition| definition.family),
                    Some(LandFigureFamily::NativeSpearman)
                ) {
                    native_idle_anchors += 1;
                    match figure.origin_descriptor[0] {
                        0x33 | 0x34 => {
                            assert_eq!(
                                figure.origin_descriptor[1],
                                island.number,
                                "{:?}: native slot {} static anchor island differs from figure island",
                                path.file_name(),
                                figure.runtime_slot,
                            );
                            assert!(
                                figure.origin_descriptor[2] < island.width
                                    && figure.origin_descriptor[3] < island.height,
                                "{:?}: native slot {} static anchor ({}, {}) is outside island {}",
                                path.file_name(),
                                figure.runtime_slot,
                                figure.origin_descriptor[2],
                                figure.origin_descriptor[3],
                                island.number,
                            );
                        }
                        0x38 => {
                            let anchor_x = i32::from(
                                (u16::from(figure.origin_descriptor[1] & 0x0f) << 8)
                                    | u16::from(figure.origin_descriptor[2]),
                            );
                            let anchor_y = i32::from(
                                (u16::from(figure.origin_descriptor[1] >> 4) << 8)
                                    | u16::from(figure.origin_descriptor[3]),
                            );
                            let anchor_local_x = anchor_x / 2 - i32::from(island.x_pos);
                            let anchor_local_y = anchor_y / 2 - i32::from(island.y_pos);
                            assert!(
                                (0..i32::from(island.width)).contains(&anchor_local_x)
                                    && (0..i32::from(island.height)).contains(&anchor_local_y),
                                "{:?}: native slot {} packed anchor ({anchor_x}, {anchor_y}) is outside island {} local ({anchor_local_x}, {anchor_local_y})",
                                path.file_name(),
                                figure.runtime_slot,
                                island.number,
                            );
                        }
                        kind => panic!(
                            "{:?}: native slot {} uses unsupported idle-anchor kind {kind:#x}",
                            path.file_name(),
                            figure.runtime_slot,
                        ),
                    }
                }
            }
        }

        assert_eq!(scenarios, 23, "SOLDAT3 corpus scenario count changed");
        assert_eq!(figures, 972, "SOLDAT3 corpus figure count changed");
        assert_eq!(fixed_targets, 2, "fixed-point target count changed");
        fixed_target_rows.sort_unstable();
        assert_eq!(
            fixed_target_rows,
            vec![
                (
                    "On His Majesty's Service0.szs".to_owned(),
                    119,
                    14,
                    1,
                    1,
                    [0x38, 0x22, 0xec, 0x4c],
                ),
                (
                    "On His Majesty's Service0.szs".to_owned(),
                    129,
                    14,
                    1,
                    6,
                    [0x38, 0x22, 0xea, 0x42],
                ),
            ]
        );
        assert_eq!(native_idle_anchors, 67, "native idle-anchor count changed");
    }

    #[test]
    fn inselhaus_id_uses_executable_definition_base() {
        let tile = IslandTile {
            building_id: 0x01ab,
            x: 0,
            y: 0,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        };

        assert_eq!(tile.source_id(), 0x4e20 + 0x01ab);
        assert_eq!(tile.source_owner(), 0);

        let owned = IslandTile {
            anim_count: 0x80,
            flags: 1,
            ..tile
        };
        assert_eq!(owned.source_owner(), 6);

        let dynamic_owner = IslandTile {
            flags: 6 << 6,
            ..tile
        };
        assert_eq!(dynamic_owner.source_dynamic_object_owner(), 6);
    }

    #[test]
    fn round_trip_encoded_islands() {
        let islands = vec![
            Island {
                number: 3,
                width: 50,
                height: 30,
                x_pos: 100,
                y_pos: 200,
                tiles: vec![
                    IslandTile {
                        building_id: 1234,
                        x: 5,
                        y: 7,
                        orientation: 1,
                        anim_count: 0,
                        flags: 0,
                    },
                    IslandTile {
                        building_id: 42,
                        x: 9,
                        y: 9,
                        orientation: 0,
                        anim_count: 2,
                        flags: 1,
                    },
                ],
                fertilities: [7; 8],
                city: None,
            },
            Island {
                number: 4,
                width: 60,
                height: 40,
                x_pos: 500,
                y_pos: 600,
                tiles: vec![],
                fertilities: [7; 8],
                city: None,
            },
        ];
        let bytes = SzsFile::encode_islands(&islands);
        let parsed = SzsFile::parse(&bytes).expect("parse");
        assert_eq!(parsed.islands.len(), 2);
        let i0 = &parsed.islands[0];
        assert_eq!(i0.number, 3);
        assert_eq!(i0.width, 50);
        assert_eq!(i0.height, 30);
        assert_eq!(i0.x_pos, 100);
        assert_eq!(i0.y_pos, 200);
        assert_eq!(i0.tiles.len(), 2);
        assert_eq!(i0.tiles[0].building_id, 1234);
        assert_eq!(i0.tiles[0].orientation, 1);
        let i1 = &parsed.islands[1];
        assert_eq!(i1.number, 4);
        assert!(i1.tiles.is_empty());
    }

    #[test]
    fn parse_scenario() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/Atoll.szs");

        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping test: {path:?} not found");
                return;
            }
        };

        let szs = SzsFile::parse(&data).expect("failed to parse SZS");

        println!("Chunks: {}", szs.chunks.len());
        for chunk in &szs.chunks {
            if chunk.name == "INSEL5" || chunk.name == "INSELHAUS" {
                println!("  {} size={}", chunk.name, chunk.data.len());
            }
        }

        println!("\nIslands: {}", szs.islands.len());
        for island in &szs.islands {
            println!(
                "  Island {} at ({},{}) size {}x{} tiles={}",
                island.number,
                island.x_pos,
                island.y_pos,
                island.width,
                island.height,
                island.tiles.len()
            );
        }

        assert!(szs.islands.len() > 5, "Atoll should have many islands");
        assert!(
            !szs.islands[0].tiles.is_empty(),
            "First island should have tiles"
        );
    }

    #[test]
    fn player4_extracts_controller_figure_capacity_limits() {
        let mut data = vec![0; PLAYER4_SLOT_BYTES * PLAYER4_MAX_SLOTS];
        data[0x9c..0x9e].copy_from_slice(&12_u16.to_le_bytes());
        let second_slot = PLAYER4_SLOT_BYTES + 0x9c;
        data[second_slot..second_slot + 2].copy_from_slice(&27_u16.to_le_bytes());

        let players = SzsFile::parse_player4(&data);
        assert_eq!(players[0].controller_figure_capacity_limit_0x9c, 12);
        assert_eq!(players[1].controller_figure_capacity_limit_0x9c, 27);
    }

    #[test]
    fn player4_extracts_starting_gold() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/Tutorial0.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Tutorial0");
        assert!(
            szs.players.len() == 7,
            "PLAYER4 chunk yields exactly 7 slots, got {}",
            szs.players.len()
        );
        // Tutorial scenarios start with non-zero gold so a player
        // can actually do anything; the binary's editor shows this
        // is configurable per-slot.
        let slot0 = szs.players[0].starting_gold;
        assert!(
            slot0 > 0,
            "tutorial slot 0 starting_gold should be positive (got {})",
            slot0
        );
        // Slot 4 is the free trader (1602_exe.c:83179) — every
        // surveyed scenario gives it 1 000 000 gold.
        assert_eq!(
            szs.players[4].starting_gold, 1_000_000,
            "slot 4 (free trader) should have 1M gold"
        );
        // Slot 6 is the pirate faction.
        assert_eq!(
            szs.players[6].starting_gold, 5_000,
            "slot 6 (pirates) should have 5 000 gold"
        );
        // Tutorial0 ships the default German male player name.
        assert_eq!(
            szs.players[0].name, "Wilfried",
            "slot 0 player name should be the default 'Wilfried'"
        );
        // Faction-state byte: 0 = human, 0x0c = AI, 0x0d = trader,
        // 0x0e = native, 0x0b = pirate (cross-scenario verified).
        assert_eq!(szs.players[0].state_byte, 0x00, "slot 0 = human");
        assert_eq!(szs.players[1].state_byte, 0x0c, "slot 1 = AI rival");
        assert_eq!(szs.players[4].state_byte, 0x0d, "slot 4 = trader");
        assert_eq!(szs.players[5].state_byte, 0x0e, "slot 5 = native");
        assert_eq!(szs.players[6].state_byte, 0x0b, "slot 6 = pirate");

        // The 0x34 u32 is currently a raw bitfield exposed for
        // future RE work. Tutorial0 has every value at 0 except
        // the unused native + pirate slots, which are clamped to
        // 0xFFFF_FFFF (matches the audit-script output).
        assert_eq!(szs.players[0].slot_u32_0x34, 0);
        assert_eq!(szs.players[5].slot_u32_0x34, 0xFFFF_FFFF);
        assert_eq!(szs.players[6].slot_u32_0x34, 0xFFFF_FFFF);

        // ai_active mirrors `1602_exe.c::FUN_00473c50`'s slot
        // filter. Tutorial0 has byte 0x0d == 0x00 for every AI
        // rival (slots 1..=3), so all four are `true`.
        for slot in 0..7 {
            assert!(
                szs.players[slot].ai_active,
                "Tutorial0 slot {slot} should be ai_active"
            );
        }
    }

    #[test]
    fn player4_secondary_string_at_0x400_is_empty_in_corpus() {
        // FUN_00473cc9 reads a null-terminated CP1252 string at
        // each PLAYER4 slot's offset 0x400 and copies it to the
        // runtime player struct + 0x34 (likely an AI personality
        // / scripted-behaviour identifier). Audit confirms the
        // field is empty (first byte 0x00) in every slot of
        // every shipping `.szs`. This invariant gates the future
        // semantic decode: when a custom scenario writes a
        // non-empty value here, the test will fail and the
        // reader can grow to expose the field.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            println!("Skipping: scenes dir not found");
            return;
        }
        let mut scanned_slots = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            // Inspect the raw PLAYER4 chunk for byte at slot+0x400.
            let Some(p4) = parsed.chunks.iter().find(|c| c.name == "PLAYER4") else {
                continue;
            };
            for slot in 0..7 {
                let off = slot * 1072 + 0x400;
                if off >= p4.data.len() {
                    continue;
                }
                assert_eq!(
                    p4.data[off],
                    0,
                    "{:?} slot {slot}: PLAYER4 byte 0x400 should be NUL (empty string), got 0x{:02X}",
                    path.file_stem().unwrap(),
                    p4.data[off]
                );
                scanned_slots += 1;
            }
        }
        assert!(scanned_slots > 0, "audit must scan at least one slot");
    }

    #[test]
    fn player4_byte_05_is_slot_index_echo_corpus_wide() {
        // FUN_00478160:85404 writes `local_8` (the iteration
        // counter, 0..6) into chunk[5] of each slot.
        // Equivalent assertion: every PLAYER4 slot's byte 5
        // matches its slot index. Corpus-wide invariant.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let Some(p4) = parsed.chunks.iter().find(|c| c.name == "PLAYER4") else {
                continue;
            };
            for slot in 0..7 {
                let off = slot * 1072 + 5;
                if off >= p4.data.len() {
                    continue;
                }
                assert_eq!(
                    p4.data[off],
                    slot as u8,
                    "{:?} slot {slot}: byte 0x05 should echo slot index",
                    path.file_stem().unwrap()
                );
                total += 1;
            }
        }
        assert!(total > 0);
    }

    #[test]
    fn player4_byte_06_is_constant_one_corpus_wide() {
        // byte 0x06 == 0x01 in every PLAYER4 slot of every
        // shipping `.szs`. The binary writes
        // (undefined1)DAT_005bafdc here at FUN_00478160:85405
        // — a "record-version / valid-slot" marker.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let Some(p4) = parsed.chunks.iter().find(|c| c.name == "PLAYER4") else {
                continue;
            };
            for slot in 0..7 {
                let off = slot * 1072 + 6;
                if off >= p4.data.len() {
                    continue;
                }
                assert_eq!(
                    p4.data[off],
                    0x01,
                    "{:?} slot {slot}: byte 0x06 should be the 0x01 record marker",
                    path.file_stem().unwrap()
                );
                total += 1;
            }
        }
        assert!(total > 0);
    }

    #[test]
    fn szene_ranking_is_in_range_0_to_3() {
        // Audit shows RANKING ∈ {0, 1, 2, 3} across all 62
        // shipping `.szs`. Tutorial0 → 0, Plague → 3, etc.
        // A new scenario with RANKING > 3 (or < 0 if signed)
        // would break our scenario picker UI.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if let Some(r) = parsed.scenario.ranking {
                assert!(
                    r <= 3,
                    "{:?} has RANKING {} > 3",
                    path.file_stem().unwrap(),
                    r
                );
                total += 1;
            }
        }
        assert!(total > 0);
    }

    #[test]
    fn stadt4_sparse_region_carries_native_pirate_stockpile() {
        // STADT4 offsets 0x18, 0x1C, 0x20, 0x24 (four u32) are
        // zero on every player/AI city but uniformly 200 on
        // native + pirate settlements (Jaricho, Citaltepetl,
        // Uga Bunga, Manakaru, ...). The pattern almost
        // certainly seeds the hostile-faction stockpile.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut natives_seen = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for chunk in parsed.chunks.iter().filter(|c| c.name == "STADT4") {
                if chunk.data.len() < 0x28 {
                    continue;
                }
                let r = |o: usize| {
                    u32::from_le_bytes([
                        chunk.data[o],
                        chunk.data[o + 1],
                        chunk.data[o + 2],
                        chunk.data[o + 3],
                    ])
                };
                let v = [r(0x18), r(0x1C), r(0x20), r(0x24)];
                let owner = chunk.data[2];
                if v == [0; 4] {
                    // player/AI city — fine.
                } else {
                    // Non-zero must be (200,200,200,200) AND
                    // owner must be a non-active faction
                    // (5 native or 6 pirate).
                    assert_eq!(
                        v,
                        [200; 4],
                        "{:?}: STADT4 0x18..0x28 stockpile must be [200; 4], got {v:?}",
                        path.file_stem().unwrap()
                    );
                    assert!(
                        owner == 5 || owner == 6,
                        "{:?}: stockpile only on native/pirate, owner={owner}",
                        path.file_stem().unwrap()
                    );
                    natives_seen += 1;
                }
            }
        }
        assert!(
            natives_seen > 0,
            "audit must include at least one native/pirate stockpiled city"
        );
    }

    #[test]
    fn stadt4_byte_04_always_zero_byte_05_settlement_stage() {
        // Audit (`probe stadt4 head region`) shows STADT4
        // byte 0x04 is uniformly 0 across the corpus; byte
        // 0x05 carries the "settlement stage" indicator with
        // values 0x11..0x1F (17..31) only on cities with
        // pre-seeded population. The u32 at 0x04 therefore
        // reads as `(byte_0x05 << 8)`, matching the
        // 4352/4608/5120/6912/7936 values seen in the audit.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for chunk in parsed.chunks.iter().filter(|c| c.name == "STADT4") {
                if chunk.data.len() < 8 {
                    continue;
                }
                let b4 = chunk.data[4];
                let b5 = chunk.data[5];
                // byte 0x04: typically 0 in shipping content,
                // small values (≤ 0x1F) in scenarios that
                // pre-configure the stage marker.
                assert!(
                    b4 <= 0x1F,
                    "{:?}: STADT4 byte 0x04 = {b4:#04x} unexpectedly large",
                    path.file_stem().unwrap()
                );
                // Stage marker: either 0 (empty/uninhabited) or
                // 0x11..=0x1F (occupied tier in shipping corpus).
                assert!(
                    b5 == 0 || (0x11..=0x1F).contains(&b5),
                    "{:?}: STADT4 byte 0x05 = {b5:#04x} out of range",
                    path.file_stem().unwrap()
                );
                total += 1;
            }
        }
        assert!(total > 100);
    }

    #[test]
    fn insel5_byte_5d_constant_0x11_with_one_outlier() {
        // INSEL5 byte 0x5D is 0x11 (=17) on 545 of 546 corpus
        // islands and 0x51 on a single outlier. Pin the
        // overwhelming majority and document the outlier.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total = 0;
        let mut outliers = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for chunk in parsed.chunks.iter().filter(|c| c.name == "INSEL5") {
                if chunk.data.len() < 0x60 {
                    continue;
                }
                let b = chunk.data[0x5D];
                if b != 0x11 {
                    outliers += 1;
                    assert_eq!(b, 0x51, "unexpected INSEL5 byte 0x5D outlier: {b:#04x}");
                }
                total += 1;
            }
        }
        assert!(total > 100);
        assert!(outliers <= 1, "only one corpus-wide 0x5D outlier expected");
    }

    #[test]
    fn insel5_byte_03_only_nonzero_in_trust_no_one0() {
        // Audit shows INSEL5 byte 0x03 == 0 for all islands
        // except Trust no one0's six islands (all of them set
        // to 0x02). Plausible interpretation: a scenario-wide
        // "ruined / starts-colonized" flag the editor stamps
        // on every island in that scenario, since Trust-no-
        // one0 is one of the Pirata-style scenarios where the
        // entire map starts in an unusual state.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total_islands = 0;
        let mut nonzero = Vec::new();
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            for chunk in parsed.chunks.iter().filter(|c| c.name == "INSEL5") {
                if chunk.data.len() < 4 {
                    continue;
                }
                total_islands += 1;
                if chunk.data[3] != 0 {
                    nonzero.push((stem.clone(), chunk.data[3]));
                }
            }
        }
        assert!(total_islands > 0);
        // Only Trust no one0 should hit the non-zero branch.
        for (scen, val) in &nonzero {
            assert_eq!(
                scen, "Trust no one0",
                "unexpected INSEL5 byte 0x03 outlier: {scen} value 0x{val:02X}"
            );
            assert_eq!(
                *val, 2,
                "Trust no one0 should consistently use byte 0x03 = 2"
            );
        }
        assert!(
            !nonzero.is_empty(),
            "Trust no one0 must contribute the documented outliers"
        );
    }

    #[test]
    fn ship4_heading_byte_packs_compass_direction() {
        // 95% of corpus records carry an even heading_byte
        // (heading × 2). Ship::heading() halves it to recover
        // the 0..=7 cardinal direction.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total = 0;
        let mut max_heading = 0u8;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for s in &parsed.ships {
                let h = s.heading();
                assert!(
                    h <= 7,
                    "{:?} ship {:?} heading {} > 7",
                    path.file_stem().unwrap(),
                    s.name,
                    h
                );
                if h > max_heading {
                    max_heading = h;
                }
                total += 1;
            }
        }
        assert!(total > 0);
        assert!(
            max_heading > 0,
            "audit must include at least one non-N heading"
        );
    }

    #[test]
    fn player4_slot_u32_0x34_trader_always_zero() {
        // Audit-derived corpus invariant: PLAYER4 slot 4 (the
        // free trader) carries `slot_u32_0x34 == 0` across all
        // 62 shipping `.szs` files. Slot 5 (native) and slot 6
        // (pirate) each carry only 2 distinct values
        // (typically 0 or 0xFFFFFFFF). The active player + AI
        // rivals (slots 0..=3) vary widely (12-20 distinct
        // values each) since this field encodes per-slot AI
        // unlocks / starting-state mask.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut slot4_seen = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if let Some(p) = parsed.players.get(4) {
                assert_eq!(
                    p.slot_u32_0x34,
                    0,
                    "{:?}: slot 4 (trader) slot_u32_0x34 must be 0, got 0x{:08X}",
                    path.file_stem().unwrap(),
                    p.slot_u32_0x34
                );
                slot4_seen += 1;
            }
        }
        assert!(slot4_seen > 50);
    }

    #[test]
    fn player4_relationship_arrays_carry_diplomacy_codes_0_3() {
        // Corpus invariant: PLAYER4 0xC0 and 0x140 arrays
        // (7 u32 stride-8 each) carry values from {0, 1, 2, 3}
        // across all 62 shipping `.szs` files. 0 and 3 are the
        // two dominant codes (peace / default-state); 1 and 2
        // appear as rare outliers in scenarios that pre-set
        // mid-state diplomacy (e.g., Magnate2's pre-cooled
        // relationships). Concrete semantics aren't yet pinned
        // (TaskList #115); this invariant guards the value
        // range against corruption.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total_slots = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for player in &parsed.players {
                for v in player.relations_0xc0 {
                    assert!(
                        v <= 3,
                        "{:?}: PLAYER4 0xC0 entry {v} > 3",
                        path.file_stem().unwrap()
                    );
                }
                for v in player.relationships {
                    assert!(
                        v <= 3,
                        "{:?}: PLAYER4 0x140 entry {v} > 3",
                        path.file_stem().unwrap()
                    );
                }
                total_slots += 1;
            }
        }
        assert!(total_slots > 100);
    }

    #[test]
    fn ship4_byte_4a_correlates_strictly_with_owner() {
        // Audit shows byte 0x4A == 0x01 for every record with
        // owner ∈ {0, 1, 2, 3} (player + AI rivals) and
        // 0x4A == 0x03 for every record with owner == 5
        // (native faction, including PIRAT-figure ships).
        // Pin the correlation as a corpus invariant — gives
        // us a redundant cross-check on the owner byte.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let Some(s4) = parsed.chunks.iter().find(|c| c.name == "SHIP4") else {
                continue;
            };
            for record in s4.data.chunks_exact(436) {
                if record.len() < 0x4C {
                    continue;
                }
                let b4a = record[0x4A];
                let owner = record[0x4B];
                let want = match owner {
                    0..=3 => 0x01,
                    5 => 0x03,
                    _ => unreachable!("owner {owner} unexpected"),
                };
                assert_eq!(
                    b4a,
                    want,
                    "{:?}: SHIP4 byte 0x4A {:#04x} doesn't match owner {} pattern",
                    path.file_stem().unwrap(),
                    b4a,
                    owner
                );
                total += 1;
            }
        }
        assert!(total > 0);
    }

    #[test]
    fn ship4_byte_41_constant_80_corpus_wide() {
        // byte 0x41 == 80 in every SHIP4 record across the
        // corpus. Likely a record-format / sprite-anchor
        // constant the engine never varies.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let Some(s4) = parsed.chunks.iter().find(|c| c.name == "SHIP4") else {
                continue;
            };
            for record in s4.data.chunks_exact(436) {
                if record.len() < 0x42 {
                    continue;
                }
                assert_eq!(
                    record[0x41],
                    80,
                    "{:?}: SHIP4 byte 0x41 should be 80, got {}",
                    path.file_stem().unwrap(),
                    record[0x41]
                );
                total += 1;
            }
        }
        assert!(total > 0);
    }

    #[test]
    fn auftrag4_chunk_size_is_one_mission_in_shipping_corpus() {
        // The binary's encoder FUN_00478380 allocates 0x2310 =
        // 4 × 0x8C4 = up to 4 mission slots and writes
        // `iVar7 * 0x8C4` bytes (active count). Across all 62
        // shipping `.szs` files we observe exactly 1 mission
        // per AUFTRAG4 chunk. New scenarios with multi-mission
        // chunks will fail this invariant — at which point our
        // `Mission::from_chunks` reader needs to grow N-mission
        // support (see also TaskList #128).
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            println!("Skipping: scenes dir not found");
            return;
        }
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if let Some(c) = parsed.chunks.iter().find(|c| c.name == "AUFTRAG4") {
                assert_eq!(
                    c.data.len(),
                    0x8C4,
                    "{:?}: AUFTRAG4 chunk size {} ≠ 1 × 0x8C4",
                    path.file_stem().unwrap(),
                    c.data.len()
                );
                total += 1;
            }
        }
        assert!(total > 0, "audit must scan at least one scenario");
    }

    #[test]
    fn player4_state_byte_layout_is_corpus_invariant() {
        // Every shipping `.szs` carries exactly the same
        // state_byte sequence at PLAYER4: slot 0 = 0x00 (human),
        // slots 1..=3 = 0x0C (AI rival), slot 4 = 0x0D (free
        // trader), slot 5 = 0x0E (native), slot 6 = 0x0B
        // (pirate). The slot index and state_byte are
        // therefore redundant — every scenario uses the same
        // PLAYER4 ordering. New scenarios that violate this
        // invariant should fail this test loudly.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            println!("Skipping: scenes dir not found");
            return;
        }
        let want: [u8; 7] = [0x00, 0x0C, 0x0C, 0x0C, 0x0D, 0x0E, 0x0B];
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let szs = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for (i, p) in szs.players.iter().enumerate().take(7) {
                assert_eq!(
                    p.state_byte,
                    want[i],
                    "{:?} slot {i} has state_byte 0x{:02X}, want 0x{:02X}",
                    path.file_stem().unwrap(),
                    p.state_byte,
                    want[i]
                );
            }
            total += 1;
        }
        assert!(total > 0, "audit must scan at least one scenario");
    }

    #[test]
    fn player4_byte_0x18_carries_per_slot_index() {
        // Magnate2 ships values 0x05/0x06/0x06 on its three AI
        // rivals — the most distinctive non-zero per-slot row
        // in the corpus. Plague of Pirates is a control sample
        // (all zero).
        for (scenario, expected) in &[
            ("A Plague of Pirates", [0u16; 7]),
            // Atoll: only slot 0 carries 0x01.
            ("Atoll", [1, 0, 0, 0, 0, 0, 0]),
            ("The Magnate2", [2, 5, 6, 6, 0, 0, 0]),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(format!("extracted/Szenes/{scenario}.szs"));
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(_) => {
                    println!("Skipping: {path:?} not found");
                    continue;
                }
            };
            let szs = SzsFile::parse(&data).expect("parse");
            for slot in 0..7 {
                assert_eq!(
                    szs.players[slot].slot_u16_0x18, expected[slot],
                    "{scenario} slot {slot} byte 0x18"
                );
            }
        }
    }

    #[test]
    fn player4_relationships_table_matches_observed_pattern() {
        // Tutorial0 / Plague / Atoll all share the canonical
        // diplomacy seed shown in `PlayerSlotInit::relationships`.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/Tutorial0.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Tutorial0");
        // Active rows (player + AIs): zero against active slots,
        // 3 against trader/native/pirate.
        for slot in 0..=3 {
            assert_eq!(
                szs.players[slot].relationships,
                [0, 0, 0, 0, 3, 3, 3],
                "active slot {slot} relationship row"
            );
        }
        // Trader: mirror image — 3 against actives, 0 against
        // specials.
        assert_eq!(szs.players[4].relationships, [3, 3, 3, 3, 0, 0, 0]);
        // Natives: only self-position = 3.
        assert_eq!(szs.players[5].relationships, [0, 0, 0, 0, 0, 3, 0]);
        // Pirates: same shape as trader.
        assert_eq!(szs.players[6].relationships, [3, 3, 3, 3, 0, 0, 0]);
    }

    #[test]
    fn player4_companion_arrays_match_binary_encoder() {
        // FUN_00478160 writes three 7-element stride-8 u32 arrays
        // at chunk offsets 0xC0, 0x140, 0x1C0. Tutorial0 leaves
        // 0x1C0 entirely zero; Magnate0 fills it with an
        // (slot << 8) | type per-slot event log whose Nth slot
        // carries N entries against rivals 1..=N.
        let load = |stem: &str| -> Option<SzsFile> {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(format!("extracted/Szenes/{stem}.szs"));
            std::fs::read(&path)
                .ok()
                .and_then(|d| SzsFile::parse(&d).ok())
        };

        if let Some(t) = load("Tutorial0") {
            // Tutorial0 leaves 0x1C0 entirely zero.
            for slot in 0..7 {
                assert_eq!(
                    t.players[slot].events_0x1c0, [0; 7],
                    "Tutorial0 slot {slot} 0x1C0 array"
                );
            }
            // 0xC0 array — slot 0 has 3s everywhere except
            // position 5 (natives).
            assert_eq!(t.players[0].relations_0xc0, [3, 3, 3, 3, 3, 0, 3]);
            // Slot 5 (natives) — only positions 4, 5 = 3.
            assert_eq!(t.players[5].relations_0xc0, [0, 0, 0, 0, 3, 3, 0]);
        }

        if let Some(m) = load("The Magnate0") {
            // Magnate0 events log: slot N (1..=3, the AI rivals)
            // carries N entries of `(N << 8) | 2` against rivals
            // 1..=N. Slots 0 / 4 / 5 stay empty; slot 6 (pirates)
            // carries a different encoding [0x301, 0x303, …].
            assert_eq!(
                m.players[0].events_0x1c0, [0; 7],
                "slot 0 (player) has no events"
            );
            assert_eq!(m.players[1].events_0x1c0, [0x102, 0, 0, 0, 0, 0, 0]);
            assert_eq!(m.players[2].events_0x1c0, [0x102, 0x202, 0, 0, 0, 0, 0]);
            assert_eq!(m.players[3].events_0x1c0, [0x102, 0x202, 0x302, 0, 0, 0, 0]);
            assert_eq!(
                m.players[4].events_0x1c0, [0; 7],
                "slot 4 (trader) has no events"
            );
            assert_eq!(
                m.players[5].events_0x1c0, [0; 7],
                "slot 5 (natives) has no events"
            );
            assert_eq!(
                m.players[6].events_0x1c0,
                [0x301, 0x303, 0, 0, 0, 0, 0],
                "slot 6 (pirates) carries the distinct encoding"
            );
        }
    }

    #[test]
    fn player4_ai_active_skips_disabled_rivals() {
        // Exile pre-configures but disables some AI rivals via
        // byte 0x0d == 0x01. The audit run counts 21 such slots
        // across shipping content; Exile is one of them.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/Exile.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Exile");
        // Slots 1 and 3 carry `state_byte == 0x0c` (AI) but
        // byte 0x0d == 0x01, so the binary skips them.
        assert_eq!(szs.players[1].state_byte, 0x0c);
        assert!(
            !szs.players[1].ai_active,
            "Exile slot 1 has byte 0x0d == 0x01 → AI disabled"
        );
        assert_eq!(szs.players[3].state_byte, 0x0c);
        assert!(
            !szs.players[3].ai_active,
            "Exile slot 3 has byte 0x0d == 0x01 → AI disabled"
        );
    }

    #[test]
    fn player4_slot_u32_0x34_grows_with_difficulty() {
        // Magnate0 ships a difficulty-tiered AI roster: stronger
        // rivals carry strictly larger 0x34 bitsets. This is the
        // strongest cross-scenario signal that the field encodes
        // an AI feature/unlock mask.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/The Magnate0.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Magnate0");
        assert_eq!(
            szs.players[0].slot_u32_0x34, 0x0000_0003,
            "slot 0 (player) baseline mask"
        );
        assert_eq!(
            szs.players[1].slot_u32_0x34, 0x003F_C00F,
            "slot 1 (easy AI) mid-tier mask"
        );
        assert_eq!(
            szs.players[2].slot_u32_0x34, 0x0FFF_C33F,
            "slot 2 (harder AI) wide mask"
        );
        assert_eq!(
            szs.players[3].slot_u32_0x34, 0x0FFF_C33F,
            "slot 3 (harder AI) wide mask"
        );
        // Strict monotone growth across rivals — 0 ⊂ 1 ⊂ 2.
        let masks: Vec<u32> = (0..4).map(|i| szs.players[i].slot_u32_0x34).collect();
        assert!(
            masks[0] & masks[1] == masks[0],
            "slot 1 mask is a superset of slot 0"
        );
        assert!(
            masks[1] & masks[2] == masks[1],
            "slot 2 mask is a superset of slot 1"
        );
    }

    #[test]
    fn fertility_byte_maps_to_editor_cod_rohst_order() {
        // editor.cod's [ROHST] section pins the order:
        //   Grain / Tobacco / Spices / Sugarcane / Cotton /
        //   Vines / Cocoa / Grazing land
        assert_eq!(Fertility::from_byte(0), Some(Fertility::Grain));
        assert_eq!(Fertility::from_byte(1), Some(Fertility::Tobacco));
        assert_eq!(Fertility::from_byte(2), Some(Fertility::Spices));
        assert_eq!(Fertility::from_byte(3), Some(Fertility::Sugarcane));
        assert_eq!(Fertility::from_byte(4), Some(Fertility::Cotton));
        assert_eq!(Fertility::from_byte(5), Some(Fertility::Vines));
        assert_eq!(Fertility::from_byte(6), Some(Fertility::Cocoa));
        // 7 = sentinel, 8+ = invalid → None
        assert_eq!(Fertility::from_byte(7), None);
        assert_eq!(Fertility::from_byte(8), None);
    }

    #[test]
    fn insel5_source_resource_state_matches_fun_0046aff0_inputs() {
        let mut body = vec![0_u8; 0x74];
        body[0] = 4;
        body[1] = 12;
        body[2] = 10;
        body[0x1a] = 2;
        // Record selectors are the word at `0x1c + 8r`; the availability word
        // lives separately at `0x28 + 8r`. Record 0 is an available (state 0)
        // sub-crop `0x02` with remaining `0x20`; record 1 a partial (state 1)
        // sub-crop `0x03`.
        body[0x1c] = 0x02;
        body[0x28] = 0;
        body[0x2a] = 0x20;
        body[0x24] = 0x03;
        body[0x30] = 1;
        // Authored crop bit 2 (ware 0x2f); the loader still forces 0x1181 on.
        body[0x5c..0x60].copy_from_slice(&(1_u32 << 2).to_le_bytes());
        // Season/parity byte at 0x64 selects the odd fertile triple.
        body[0x64] = 1;
        body[0x66] = 0x40;
        // Transition deadline at 0x6c (the loader's runtime `+0x60`).
        body[0x6c..0x70].copy_from_slice(&12_345_u32.to_le_bytes());

        let mut encoded = Vec::new();
        write_chunk(&mut encoded, "INSEL5", &body);
        let parsed = SzsFile::parse(&encoded).expect("parse resource state");
        let state = parsed.island_source_resource_state(0);

        assert_eq!(state.record_count, 2);
        assert_eq!(state.records[0].ware(), 0x02);
        assert_eq!(state.records[0].availability_state(), 0);
        assert_eq!(state.records[0].remaining_amount(), 0x20);
        assert_eq!(state.records[1].ware(), 0x03);
        assert_eq!(state.records[1].availability_state(), 1);
        assert_eq!(state.parity, 1);
        assert_eq!(state.attenuation, 0x40);
        assert_eq!(state.transition_deadline_ticks, 12_345);
        // Sub-crop wares (< 0x2d) go through the record search.
        assert_eq!(state.resource_strength(0x02), 0x80);
        assert_eq!(state.resource_strength(0x03), 0x40);
        assert_eq!(state.resource_strength(0x04), 0);
        // Grain and the attenuation-exempt grass/tree/fish are forced on.
        assert_eq!(state.resource_strength(0x2d), 0x80);
        assert_eq!(state.resource_strength(0x35), 0x80);
        assert_eq!(state.resource_strength(0x39), 0x80);
        // The authored crop bit 2 pins ware 0x2f to full strength directly.
        assert_eq!(state.resource_strength(0x2f), 0x80);
        // Parity 1 makes the rest of the odd triple fertile (0x40) and the
        // even triple barren (0) through the season fallback.
        assert_eq!(state.resource_strength(0x31), 0x40);
        assert_eq!(state.resource_strength(0x33), 0x40);
        assert_eq!(state.resource_strength(0x2e), 0);
        assert_eq!(state.resource_strength(0x30), 0);
    }

    #[test]
    fn insel5_runtime_classification_matches_00469e50() {
        assert_eq!(source_runtime_island_classification(0x20, 9), 0);
        assert_eq!(source_runtime_island_classification(0x21, 9), 1);
        assert_eq!(source_runtime_island_classification(0x2a, 9), 1);
        assert_eq!(source_runtime_island_classification(0x2b, 9), 2);
        assert_eq!(source_runtime_island_classification(0x37, 9), 2);
        assert_eq!(source_runtime_island_classification(0x38, 9), 3);
        assert_eq!(source_runtime_island_classification(0x4b, 9), 3);
        assert_eq!(source_runtime_island_classification(0x4c, 9), 4);
        assert_eq!(source_runtime_island_classification(0x6e, 9), 4);
        assert_eq!(source_runtime_island_classification(0x6f, 9), 9);

        let mut body = vec![0_u8; 0x74];
        body[0] = 0x71;
        body[1] = 0x6f;
        body[0x62..0x64].copy_from_slice(&9_u16.to_le_bytes());
        let mut encoded = Vec::new();
        write_chunk(&mut encoded, "INSEL5", &body);
        let parsed = SzsFile::parse(&encoded).expect("parse INSEL5 classification");
        assert_eq!(parsed.island_source_runtime_classification(0), 9);
    }

    #[test]
    fn insel5_extracts_fertility_map() {
        // Atoll has 35 islands with varied fertility patterns.
        // The audit shows most islands carry the no-fertility
        // sentinel `[07; 8]` while a handful encode 1-2 active
        // fertility slots.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/Atoll.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Atoll");
        // At least one island has the all-default `[07; 8]`
        // pattern, and at least one has a non-default slot.
        let any_default = szs.islands.iter().any(|i| i.fertilities == [7; 8]);
        let any_active = szs
            .islands
            .iter()
            .any(|i| i.fertilities.iter().any(|&v| v != 7));
        assert!(
            any_default,
            "Atoll should include at least one fertility-free island"
        );
        assert!(
            any_active,
            "Atoll should include at least one fertile island"
        );
        // No fertility byte should exceed 7 (the binary's value
        // range is 0..=7 with 7 being the no-fertility sentinel).
        for i in &szs.islands {
            for &b in &i.fertilities {
                assert!(b <= 7, "fertility byte must be in 0..=7, got {b}");
            }
        }
    }

    #[test]
    fn stadt4_extracts_city_name() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/A Plague of Pirates.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Plague");
        // The first island chunk in Plague is a sentinel with no
        // STADT4 attached; the player's city ("Larrach") sits on
        // a later island. Find any city record and verify the name.
        let city = szs
            .islands
            .iter()
            .find_map(|i| i.city.as_ref())
            .expect("at least one island has a STADT4 city");
        assert_eq!(city.name, "Larrach");
        // Larrach is the player's main settlement in Plague,
        // so it belongs to slot 0. The previous test asserted
        // `owner == 1` against the byte at offset 0, which is
        // actually the island_index — Larrach sits on island 1
        // because Plague's island 0 is an unused sentinel.
        assert_eq!(
            city.island_index, 1,
            "Larrach is on Plague's island #1 (after the sentinel)"
        );
        assert_eq!(
            city.owner_slot, 0,
            "Larrach is the player's main settlement"
        );
    }

    #[test]
    fn stadt4_extracts_per_tier_population() {
        // Peaceful Reign's "Falkenstain" carries
        // [0, 0, 85, 800, 0] per the loader's 0x5C..0x6C dword
        // window; "Fraiburg" has [4, 8, 596, 0, 0] (its top-tier
        // count of 4 at 0x5C was dropped by the earlier 0x60
        // read). Empty placeholders stay all-zero.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/Peaceful Reign.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Peaceful Reign");
        let by_name = |n: &str| {
            szs.islands
                .iter()
                .filter_map(|i| i.city.as_ref())
                .find(|c| c.name == n)
        };
        if let Some(c) = by_name("Falkenstain") {
            assert_eq!(c.tier_population, [0, 0, 85, 800, 0]);
        }
        if let Some(c) = by_name("Fraiburg") {
            assert_eq!(c.tier_population, [4, 8, 596, 0, 0]);
        }
        // Total inhabitants across every populated city must be
        // strictly positive — sanity-check that the parser
        // captured at least one non-empty city.
        let total: u64 = szs
            .islands
            .iter()
            .filter_map(|i| i.city.as_ref())
            .flat_map(|c| c.tier_population.iter().map(|&v| v as u64))
            .sum();
        assert!(
            total > 0,
            "Peaceful Reign should have non-empty city populations"
        );
    }

    #[test]
    fn stadt4_multi_city_scenario_distinguishes_island_from_owner() {
        // New Horizons2 places cities on multiple islands with
        // distinct owner_slots — this is the test that motivated
        // separating the two fields.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/New Horizons2.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse New Horizons2");
        let cities: Vec<&City> = szs
            .islands
            .iter()
            .filter_map(|i| i.city.as_ref())
            .filter(|c| !c.name.is_empty())
            .collect();
        let by_name = |n: &str| cities.iter().find(|c| c.name == n).copied();
        // "Jaricho" sits on island 21 with owner_slot 6 (pirate).
        if let Some(c) = by_name("Jaricho") {
            assert_eq!(c.island_index, 21);
            assert_eq!(c.owner_slot, 6, "Jaricho is the pirate stronghold (slot 6)");
        }
        // "Radolfsell" — island 19, owner_slot 5 (natives).
        if let Some(c) = by_name("Radolfsell") {
            assert_eq!(c.island_index, 19);
            assert_eq!(c.owner_slot, 5);
        }
        // No city should have owner_slot > 6 (only seven slots
        // exist), and we expect the corpus invariant that
        // island_index varies independently of owner_slot.
        for c in &cities {
            assert!(
                c.owner_slot <= 6,
                "owner_slot must be a valid PLAYER4 slot index, got {}",
                c.owner_slot
            );
        }
    }

    #[test]
    fn scenario_meta_extracts_szene_chunks() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/A Plague of Pirates.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Plague");
        // Plague of Pirates: campaign mission #15, single-player,
        // ranking 3 (matches the in-game mission picker).
        assert_eq!(szs.scenario.mission_nr, Some(15));
        assert_eq!(szs.scenario.player_min, Some(1));
        assert_eq!(szs.scenario.player_max, Some(1));
        assert_eq!(szs.scenario.ranking, Some(3));
    }

    #[test]
    fn ship4_extracts_initial_ships() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/Tutorial0.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Tutorial0");
        // Tutorial0 has a single starting ship "Seehind" at
        // approximately (210, 128) in map coords.
        assert_eq!(szs.ships.len(), 1, "Tutorial0 has one starting ship");
        assert_eq!(szs.ships[0].name, "Seehind");
        assert_eq!(szs.ships[0].x, 0xd2);
        assert_eq!(szs.ships[0].y, 0x80);
        let ship = &szs.ships[0];
        assert_eq!(ship.raw_record.len(), SHIP4_RECORD_BYTES);
        assert_eq!(
            u16::from_le_bytes([ship.raw_record[28], ship.raw_record[29]]),
            ship.x
        );
        assert_eq!(
            u16::from_le_bytes([ship.raw_record[30], ship.raw_record[31]]),
            ship.y
        );
        assert_eq!(ship.raw_record[0x42], ship.heading_byte);
        assert_eq!(
            u16::from_le_bytes([ship.raw_record[0x3c], ship.raw_record[0x3d]]),
            ship.stored_energy
        );
        assert_eq!(
            u16::from_le_bytes([ship.raw_record[0x46], ship.raw_record[0x47]]),
            ship.runtime_slot
        );
        assert_eq!(
            u16::from_le_bytes([ship.raw_record[0x48], ship.raw_record[0x49]]),
            ship.figure_definition_id
        );
        assert_eq!(ship.figure_definition_id as u8, ship.ship_class);
        assert_eq!(ship.raw_record[0x4a], ship.figure_kind);
        assert_eq!(ship.raw_record[0x4d], ship.candidate_list_key);
        assert_eq!(ship.raw_record[0x50], ship.source_direction);
        assert_eq!(ship.raw_record[0x4e], ship.animation_state);
        assert_eq!(ship.raw_record[0x4b], ship.owner);
        assert_eq!(
            ship.source_kind6_target_descriptor_payload(),
            [ship.raw_record[0x2c], ship.raw_record[0x2d]]
        );
        assert_eq!(
            ship.source_kind6_policy_raw_slots()[0],
            u64::from_le_bytes(ship.raw_record[0x132..0x13a].try_into().unwrap())
        );
        assert_eq!(
            u32::from_le_bytes(ship.raw_record[0x175..0x179].try_into().unwrap()),
            ship.cargo_slots[0]
        );
        // Tutorial0's lone ship is the human player's small
        // trader: owner = slot 0, ship_class one of the five
        // observed values {0x15, 0x17, 0x19, 0x1B, 0x1F}.
        assert_eq!(
            szs.ships[0].owner, 0,
            "Tutorial0 starting ship is owned by the human player"
        );
        assert!(
            matches!(szs.ships[0].ship_class, 0x15 | 0x17 | 0x19 | 0x1B | 0x1F),
            "ship_class falls within the observed shipping-corpus set, got 0x{:02X}",
            szs.ships[0].ship_class
        );
    }

    #[test]
    fn ship4_cargo_slots_carry_three_loaded_entries_in_tutorial0() {
        // Tutorial0 starts the player with one ship loaded with three
        // goods. The packed u32 cargo entries at offset 0x175 are
        // 0x0003C002, 0x0003C007, 0x00032004. `FUN_00448120` decodes their
        // low byte as ware (0x02 iron ore, 0x07 meat, 0x04 wool) and bits
        // 8..=21 as the 1/32-good quantity (960, 960, 800 = 30/30/25 goods).
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/Tutorial0.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Tutorial0");
        assert_eq!(szs.ships.len(), 1);
        assert_eq!(
            szs.ships[0].cargo_slots,
            [0x0003C002, 0x0003C007, 0x00032004, 0, 0, 0, 0],
            "Tutorial0 starting cargo"
        );
        // The decoded quantity retains the source's 1/32-good alignment.
        for slot in &szs.ships[0].cargo_slots {
            if *slot == 0 {
                continue;
            }
            let quantity = ((slot >> 8) & 0x3fff) as u16;
            assert!(
                quantity % 32 == 0,
                "quantity should be a multiple of 32, got 0x{quantity:04X}"
            );
            assert!(
                quantity > 0 && quantity <= 4000,
                "quantity should fall within observed range, got {quantity}"
            );
        }
    }

    #[test]
    fn ship_class_decoder_pins_observed_byte_set() {
        // The five distinct values surveyed across the SHIP4
        // corpus map onto figuren.cod's ship-figure ladder in
        // ascending order of size/strength.
        assert_eq!(ShipClass::from_byte(0x15), Some(ShipClass::SmallTrader));
        assert_eq!(ShipClass::from_byte(0x17), Some(ShipClass::LargeTrader));
        assert_eq!(ShipClass::from_byte(0x19), Some(ShipClass::SmallWarship));
        assert_eq!(ShipClass::from_byte(0x1B), Some(ShipClass::LargeWarship));
        assert_eq!(ShipClass::from_byte(0x1F), Some(ShipClass::PirateShip));
        assert_eq!(ShipClass::SmallTrader.source_figure_name(), "HANDEL1");
        assert_eq!(ShipClass::LargeTrader.source_figure_name(), "HANDEL2");
        assert_eq!(ShipClass::SmallWarship.source_figure_name(), "KRIEG1");
        assert_eq!(ShipClass::LargeWarship.source_figure_name(), "KRIEG2");
        assert_eq!(ShipClass::PirateShip.source_figure_name(), "PIRAT");
        // Anything outside the set is None.
        assert_eq!(ShipClass::from_byte(0x00), None);
        assert_eq!(ShipClass::from_byte(0x16), None);
        assert_eq!(ShipClass::from_byte(0xFF), None);

        // Warship classification matches the combat-capable
        // half of the ladder.
        assert!(!ShipClass::SmallTrader.is_warship());
        assert!(!ShipClass::LargeTrader.is_warship());
        assert!(ShipClass::SmallWarship.is_warship());
        assert!(ShipClass::LargeWarship.is_warship());
        assert!(ShipClass::PirateShip.is_warship());
    }

    #[test]
    fn ship4_never_uses_owner_slot_6_in_corpus() {
        // Pirates (PLAYER4 slot 6) spawn dynamically from the
        // pirate Kontor; SHIP4 only carries static ships, and
        // every PirateShip-class record in the corpus is owned
        // by slot 5 (the hostile native faction, which uses
        // the PIRAT figure as its visual hull).
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            println!("Skipping: scenes dir not found");
            return;
        }
        let mut pirate_class_count = 0;
        let mut total = 0;
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let szs = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for s in &szs.ships {
                total += 1;
                assert_ne!(
                    s.owner,
                    6,
                    "{:?}: SHIP4 must not carry owner == 6 (pirates)",
                    path.file_stem().unwrap()
                );
                if s.class() == Some(ShipClass::PirateShip) {
                    assert_eq!(
                        s.owner,
                        5,
                        "{:?}: PirateShip-class records must be owner 5",
                        path.file_stem().unwrap()
                    );
                    pirate_class_count += 1;
                }
            }
        }
        assert!(
            pirate_class_count > 0,
            "corpus should include at least one PirateShip record"
        );
        assert!(total > 0, "corpus should include at least one SHIP4 record");
    }

    #[test]
    fn ship_class_decoder_runs_clean_on_corpus() {
        // Every shipping `.szs` file should yield a known
        // ShipClass for every static SHIP4 record. A failure
        // here means a new ship-class byte appeared in the
        // corpus and the enum needs to grow.
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            println!("Skipping: scenes dir not found");
            return;
        }
        for entry in std::fs::read_dir(&scenes).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .map(|s| s.eq_ignore_ascii_case("szs"))
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let szs = match SzsFile::parse(&bytes) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for s in &szs.ships {
                let stem = path.file_stem().unwrap().to_string_lossy();
                assert_eq!(
                    u16::from_le_bytes([s.raw_record[0x48], s.raw_record[0x49]]),
                    s.figure_definition_id,
                    "{stem}: ship \"{}\" has a detached figure-definition ID",
                    s.name
                );
                assert_eq!(
                    s.figure_definition_id as u8, s.ship_class,
                    "{stem}: ship \"{}\" has a detached ship-class projection",
                    s.name
                );
                assert!(
                    s.class().is_some(),
                    "{stem}: ship \"{}\" has unknown ship_class byte 0x{:02X}",
                    s.name,
                    s.ship_class
                );
            }
        }
    }

    #[test]
    fn ship4_owner_distribution_covers_player_and_ai() {
        // Plague of Pirates has 19 ships across multiple owners,
        // so it's the best test of the owner-byte interpretation.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/A Plague of Pirates.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Plague");
        let mut owners: std::collections::BTreeSet<u8> = std::collections::BTreeSet::new();
        for s in &szs.ships {
            owners.insert(s.owner);
        }
        assert!(
            owners.contains(&0),
            "Plague includes at least one human-owned ship"
        );
        // The full surveyed set across the corpus is {0,1,2,3,5}
        // — Plague should cover at least 0 and one rival.
        let has_rival = owners.iter().any(|&o| matches!(o, 1 | 2 | 3 | 5));
        assert!(
            has_rival,
            "Plague should also include a non-player owner; got {owners:?}"
        );
    }

    #[test]
    fn auftrag4_flag_bits_decode() {
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            println!("Skipping: {scenes:?} not found");
            return;
        }
        let load = |name: &str| -> Option<Mission> {
            let bytes = std::fs::read(scenes.join(name)).ok()?;
            SzsFile::parse(&bytes).ok()?.mission
        };
        // Plague of Pirates: pop goal AND pirate combat goal.
        let m = load("A Plague of Pirates.szs").expect("Plague present");
        assert!(
            m.flags & MISSION_FLAG_POPULATION != 0,
            "Plague must have population bit"
        );
        assert!(
            m.flags & MISSION_FLAG_PIRATE != 0,
            "Plague must have pirate bit"
        );
        // Good Neighbors: pop + cooperative neighbour goal.
        let m = load("Good Neighbors.szs").expect("Good Neighbors");
        assert!(m.flags & MISSION_FLAG_POPULATION != 0);
        assert!(m.flags & MISSION_FLAG_COOPERATIVE != 0);
        assert!(
            m.flags & MISSION_FLAG_PIRATE == 0,
            "Good Neighbors has no pirate combat"
        );
        // Tutorials carry no flags.
        let m = load("Tutorial0.szs").expect("Tutorial0");
        assert_eq!(m.flags, 0);
    }

    #[test]
    fn mission_goals_decode_per_scenario() {
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let load = |name: &str| -> Option<Mission> {
            SzsFile::parse(&std::fs::read(scenes.join(name)).ok()?)
                .ok()?
                .mission
        };
        // Plague of Pirates: 5 000 inhabitants (tier left
        // unspecified — the briefing only names a number).
        let g = load("A Plague of Pirates.szs").unwrap().goals();
        let p = g.primary.expect("Plague has primary");
        assert_eq!(p.total, 5_000);
        assert_eq!(p.tier, None);
        assert!(g.secondary.is_none());
        assert_eq!(g.cooperative_population, None);
        // Good Neighbors: 1000 / Merchant + 1000 in neighbour
        // (neighbour-tier unspecified).
        let g = load("Good Neighbors.szs").unwrap().goals();
        let p = g.primary.expect("Good Neighbors primary");
        assert_eq!(p.total, 1_000);
        assert_eq!(p.tier, Some(3));
        assert_eq!(g.cooperative_population, Some(1_000));
        assert_eq!(g.cooperative_tier, None);
        // The Alliance pins the cooperative neighbour at tier 3.
        let g = load("The Alliance.szs").unwrap().goals();
        assert_eq!(g.cooperative_population, Some(1_000));
        assert_eq!(g.cooperative_tier, Some(3));
        // Tutorial0: no flags → no decoded goals.
        let g = load("Tutorial0.szs").unwrap().goals();
        assert!(g.primary.is_none());
        assert!(g.secondary.is_none());
        assert_eq!(g.cooperative_population, None);
    }

    #[test]
    fn mission_goals_decode_secondary_and_tertiary() {
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() {
            return;
        }
        let load = |name: &str| -> Option<Mission> {
            SzsFile::parse(&std::fs::read(scenes.join(name)).ok()?)
                .ok()?
                .mission
        };
        // The Continent: flags 0x107 = POP + POP2 + POP3 + RANKING.
        // Three triples each [5000, 4, 5000] (5k Aristocrats per
        // city × three cities).
        let g = load("The Continent.szs").unwrap().goals();
        let p = g.primary.unwrap();
        let s = g.secondary.unwrap();
        let t = g.tertiary.unwrap();
        assert_eq!(p.total, 5_000);
        assert_eq!(p.tier, Some(4));
        assert_eq!(p.at_tier, 5_000);
        assert_eq!(s.total, 5_000);
        assert_eq!(t.total, 5_000);
        // Cooperation: Triple 0 = [2000, 4, 1300] meaning 2 000
        // total, of which 1 300 must be Aristocrats.
        let g = load("Cooperation.szs").unwrap().goals();
        let p = g.primary.unwrap();
        assert_eq!(p.total, 2_000);
        assert_eq!(p.tier, Some(4));
        assert_eq!(p.at_tier, 1_300);
        // Cooperation has POP only (no POP2/POP3 bits) — secondary
        // and tertiary stay None.
        assert!(g.secondary.is_none());
        assert!(g.tertiary.is_none());
    }

    #[test]
    fn auftrag4_extracts_briefing_and_flags() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/Szenes/A Plague of Pirates.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Plague");
        let mission = szs.mission.as_ref().expect("AUFTRAG4 present");
        // Plague of Pirates carries the pop-goal bit (0x01) plus a
        // pirate-mission high byte (0x04xx); the precise meaning of
        // the upper bits is not yet RE-confirmed, only their value.
        assert_eq!(mission.flags, 0x0401);
        // Spot-check that the briefing was decoded — full text begins
        // with "You have managed to lead..." and mentions the 5 000
        // population goal verbatim.
        assert!(
            mission.briefing.starts_with("You have managed to lead"),
            "briefing text wrong: {:?}",
            &mission.briefing
        );
        assert!(
            mission.briefing.contains("5,000 inhabitants"),
            "briefing should mention the 5 000 inhabitants goal"
        );
        // Primary pop threshold is the first u32 of the goals
        // region; for Plague this is 0x1388 = 5000.
        let pop = u32::from_le_bytes([
            mission.goals_raw[0],
            mission.goals_raw[1],
            mission.goals_raw[2],
            mission.goals_raw[3],
        ]);
        assert_eq!(pop, 5000, "goals_raw[0..4] should encode 5 000 pop");
    }
}
