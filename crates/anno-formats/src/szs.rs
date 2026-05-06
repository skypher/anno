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

/// Island metadata from an INSEL5 chunk.
#[derive(Debug, Clone)]
pub struct Island {
    pub number: u8,
    pub width: u8,
    pub height: u8,
    pub x_pos: u16,
    pub y_pos: u16,
    pub tiles: Vec<IslandTile>,
    /// Optional city info from the matching `STADT4` chunk that
    /// follows this island's INSELHAUS in chunk order. Populated
    /// only when the island carries a settled town.
    pub city: Option<City>,
}

/// One ship record from the SHIP4 chunk (436 bytes per slot).
///
/// Cross-scenario sample (Tutorial0 = 1 record, Continous Play00
/// = 4, Cooperation = 7, Atoll = 8, A Plague of Pirates = 19)
/// confirms the chunk is always exactly `N * 436` bytes. The
/// fields decoded here are the ones needed to reconstruct
/// initial ship layouts; remaining bytes (cargo manifest, AI
/// state, route table) are preserved on the raw chunk.
#[derive(Debug, Clone)]
pub struct Ship {
    /// Ship name as displayed in the original game UI (e.g.
    /// "Carnera", "Seehind", "Palstek"). 28-byte slot, CP1252,
    /// null-terminated.
    pub name: String,
    /// Spawn position in island-grid coordinates. u16 x at
    /// record offset 28, u16 y at record offset 30.
    pub x: u16,
    pub y: u16,
    /// Owning player slot (0 = human, 1..=3 = AI rivals,
    /// 5 = native faction). Audit of 418 ship records across
    /// the shipping corpus surfaces only values {0, 1, 2, 3, 5}
    /// at byte offset 0x4B — slot 4 (free trader) and slot 6
    /// (pirate) never carry static SHIP4 records, presumably
    /// because their fleets spawn dynamically at runtime.
    pub owner: u8,
    /// Ship class byte at record offset 0x48. Audit surfaces
    /// exactly 5 distinct values across all shipping content:
    /// 0x15, 0x17, 0x19, 0x1B, 0x1F — one per ship type
    /// (small trader / large trader / small warship / large
    /// warship / pirate ship). The mapping to figuren.cod's
    /// HANDEL1/HANDEL2/KRIEG1/KRIEG2/PIRAT entries hasn't
    /// been verified against a binary function yet; the raw
    /// byte is exposed for downstream interpretation.
    pub ship_class: u8,
    /// Up to 7 cargo manifest slots at record offsets 0x174,
    /// 0x17C, 0x184, 0x18C, 0x194, 0x19C, 0x1A4 (stride 8 with
    /// the +4 word always zero). Cross-scenario audit:
    ///
    ///   * High 16 bits of each non-zero u32 are clean
    ///     multiples of 32 — most commonly 0x0640 (= 1600).
    ///     These look like quantities in a 100-unit-per-ton
    ///     scale (16 tons = 1600 units).
    ///   * Low 16 bits cluster around a small set of values
    ///     (20 distinct across the corpus) — likely a good /
    ///     ware identifier, but the encoding doesn't match a
    ///     simple small-int good_id (values up to ~10000
    ///     occur), so semantics aren't pinned to a binary
    ///     function yet.
    ///
    /// The raw u32 array is exposed so downstream callers can
    /// surface ship cargo without committing to a specific
    /// (good, qty) decode.
    pub cargo_slots: [u32; 7],
}

const SHIP4_RECORD_BYTES: usize = 436;
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
    pub name: String,
}

/// A single tile/building record from INSELHAUS (8 bytes).
#[derive(Debug, Clone, Copy)]
pub struct IslandTile {
    pub building_id: u16,
    pub x: u8,
    pub y: u8,
    pub orientation: u8,
    pub anim_count: u8,
    pub flags: u16,
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
}

/// Scenario-level metadata (mission #, player range, difficulty
/// ranking) extracted from the four `SZENE_*` chunks at the top
/// of every shipping `.szs` file. Each field is `Option<u32>`
/// because tutorial scenarios omit the player-count chunks (they
/// are implicit single-player) and standalone "Continous Play"
/// maps omit the mission number.
///
/// Cross-scenario sample shows `SZENE_RANKING` 0 in tutorials,
/// 2-3 in scripted missions; `SZENE_MISSNR` is the campaign
/// slot index visible in the original mission picker.
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
            if off + 4 > self.goals_raw.len() { return 0; }
            u32::from_le_bytes([
                self.goals_raw[off], self.goals_raw[off + 1],
                self.goals_raw[off + 2], self.goals_raw[off + 3],
            ])
        };
        let triple = |start: usize| -> Option<PopulationGoal> {
            let total = read_u32(start);
            if total == 0 { return None; }
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
            primary:   (self.flags & MISSION_FLAG_POPULATION  != 0).then(|| triple(0)).flatten(),
            secondary: (self.flags & MISSION_FLAG_POPULATION2 != 0).then(|| triple(3)).flatten(),
            tertiary:  (self.flags & MISSION_FLAG_POPULATION3 != 0).then(|| triple(6)).flatten(),
            cooperative_population: coop_active
                .then(|| read_u32(18))
                .filter(|&v| v > 0),
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
    0x0000_0080 | 0x0000_0200 | 0x0000_1000 |
    0x0000_4000 | 0x0000_8000 | 0x0001_0000;

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
    /// Seven u32 values at slot offsets 0xC0, 0xC8, … 0xF0
    /// (stride 8; padding +4 uniformly zero). Sourced from the
    /// runtime player struct's `+0xF0` array in
    /// `1602_exe.c::FUN_00478160:85440`. Cross-scenario audit
    /// surfaces a similar 0/3 pattern to `relationships` but
    /// with a different masking — Tutorial0 slot 0 has
    /// `[3, 3, 3, 3, 3, 0, 3]` here vs `[0, 0, 0, 0, 3, 3, 3]`
    /// in `relationships`. Concrete semantics aren't yet RE'd.
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
    /// Seven u32 values at slot offsets 0x140, 0x148, … 0x170
    /// (stride 8; the upper four bytes between each element are
    /// uniformly zero across all 434 surveyed slots). Each row
    /// has one entry per player slot, so this is almost certainly
    /// the per-slot relationship matrix the engine seeds into
    /// the diplomacy table at scenario load.
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
                Ok(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
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
                        "INSELHAUS" => island.tiles =
                            Self::parse_inselhaus(&chunks[j].data),
                        "STADT4" => island.city =
                            Self::parse_stadt4(&chunks[j].data),
                        "INSEL5" => break, // next island
                        _ => {}
                    }
                }

                islands.push(island);
            }
            i += 1;
        }

        // Extract per-slot player init from the PLAYER4 chunk.
        let players = chunks.iter()
            .find(|c| c.name == "PLAYER4")
            .map(|c| Self::parse_player4(&c.data))
            .unwrap_or_default();

        let mission = chunks.iter()
            .find(|c| c.name == "AUFTRAG4")
            .and_then(|c| Self::parse_auftrag4(&c.data));

        let read_u32 = |name: &str| -> Option<u32> {
            chunks.iter()
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

        let ships = chunks.iter()
            .find(|c| c.name == "SHIP4")
            .map(|c| Self::parse_ship4(&c.data))
            .unwrap_or_default();

        Ok(SzsFile { chunks, islands, players, mission, scenario, ships })
    }

    fn parse_ship4(data: &[u8]) -> Vec<Ship> {
        let count = data.len() / SHIP4_RECORD_BYTES;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let off = i * SHIP4_RECORD_BYTES;
            let name_bytes = &data[off..off + SHIP4_NAME_BYTES];
            let name_end = name_bytes.iter().position(|&b| b == 0)
                .unwrap_or(SHIP4_NAME_BYTES);
            let name: String = name_bytes[..name_end]
                .iter().map(|&b| char::from(b)).collect();
            let x = u16::from_le_bytes([data[off + 28], data[off + 29]]);
            let y = u16::from_le_bytes([data[off + 30], data[off + 31]]);
            let ship_class = if off + 0x49 <= data.len() { data[off + 0x48] } else { 0 };
            let owner      = if off + 0x4C <= data.len() { data[off + 0x4B] } else { 0 };
            let mut cargo_slots = [0u32; 7];
            for (i, slot) in cargo_slots.iter_mut().enumerate() {
                let o = off + 0x174 + i * 8;
                if o + 4 <= data.len() {
                    *slot = u32::from_le_bytes([
                        data[o], data[o + 1], data[o + 2], data[o + 3],
                    ]);
                }
            }
            out.push(Ship { name, x, y, owner, ship_class, cargo_slots });
        }
        out
    }

    fn parse_auftrag4(data: &[u8]) -> Option<Mission> {
        if data.len() < AUFTRAG4_TOTAL_BYTES { return None; }
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
        Some(Mission { flags, briefing, goals_raw })
    }

    fn parse_player4(data: &[u8]) -> Vec<PlayerSlotInit> {
        let mut out = Vec::new();
        for slot in 0..PLAYER4_MAX_SLOTS {
            let off = slot * PLAYER4_SLOT_BYTES;
            if off + 16 > data.len() { break; }
            let starting_gold = i32::from_le_bytes([
                data[off], data[off + 1], data[off + 2], data[off + 3],
            ]);
            // Byte 12 = 0x00 (active player / fixed faction) vs
            // 0xff (slot inactive — AI fills it on game start).
            let state_byte = data[off + 4];
            let color_idx  = data[off + 7];
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
            } else { 0 };
            let slot_u32_0x34 = if off + 0x38 <= data.len() {
                u32::from_le_bytes([
                    data[off + 0x34], data[off + 0x35],
                    data[off + 0x36], data[off + 0x37],
                ])
            } else { 0 };
            let read_array = |start: usize| -> [u32; 7] {
                let mut arr = [0u32; 7];
                for (i, slot_val) in arr.iter_mut().enumerate() {
                    let o = off + start + i * 8;
                    if o + 4 <= data.len() {
                        *slot_val = u32::from_le_bytes([
                            data[o], data[o + 1],
                            data[o + 2], data[o + 3],
                        ]);
                    }
                }
                arr
            };
            let relations_0xc0 = read_array(0xC0);
            let relationships  = read_array(0x140);
            let events_0x1c0   = read_array(0x1C0);
            let name_off = off + PLAYER4_NAME_OFFSET;
            let name = if name_off + PLAYER4_NAME_BYTES <= data.len() {
                let name_bytes = &data[name_off..name_off + PLAYER4_NAME_BYTES];
                let end = name_bytes.iter().position(|&b| b == 0)
                    .unwrap_or(PLAYER4_NAME_BYTES);
                name_bytes[..end].iter().map(|&b| char::from(b)).collect()
            } else {
                String::new()
            };
            out.push(PlayerSlotInit {
                starting_gold, state_byte, color_idx, slot_byte12,
                ai_active, name, slot_u32_0x34,
                relations_0xc0, relationships, events_0x1c0,
                slot_u16_0x18,
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
        Island {
            number: data[0],
            width: data[1],
            height: data[2],
            x_pos: u16::from_le_bytes([data[4], data[5]]),
            y_pos: u16::from_le_bytes([data[6], data[7]]),
            tiles: Vec::new(),
            city: None,
        }
    }

    fn parse_stadt4(data: &[u8]) -> Option<City> {
        if data.len() < 0xa8 { return None; }
        let island_index = data[0];
        let owner_slot = data[2];
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
        Some(City { island_index, owner_slot, name })
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

#[cfg(test)]
mod tests {
    use super::*;

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
                        building_id: 1234, x: 5, y: 7,
                        orientation: 1, anim_count: 0, flags: 0,
                    },
                    IslandTile {
                        building_id: 42, x: 9, y: 9,
                        orientation: 0, anim_count: 2, flags: 1,
                    },
                ],
                city: None,
            },
            Island {
                number: 4, width: 60, height: 40,
                x_pos: 500, y_pos: 600,
                tiles: vec![],
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
        assert!(!szs.islands[0].tiles.is_empty(), "First island should have tiles");
    }

    #[test]
    fn player4_extracts_starting_gold() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
            .join("extracted/Szenes/Tutorial0.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Tutorial0");
        assert!(szs.players.len() == 7,
            "PLAYER4 chunk yields exactly 7 slots, got {}", szs.players.len());
        // Tutorial scenarios start with non-zero gold so a player
        // can actually do anything; the binary's editor shows this
        // is configurable per-slot.
        let slot0 = szs.players[0].starting_gold;
        assert!(slot0 > 0,
            "tutorial slot 0 starting_gold should be positive (got {})",
            slot0);
        // Slot 4 is the free trader (1602_exe.c:83179) — every
        // surveyed scenario gives it 1 000 000 gold.
        assert_eq!(szs.players[4].starting_gold, 1_000_000,
            "slot 4 (free trader) should have 1M gold");
        // Slot 6 is the pirate faction.
        assert_eq!(szs.players[6].starting_gold, 5_000,
            "slot 6 (pirates) should have 5 000 gold");
        // Tutorial0 ships the default German male player name.
        assert_eq!(szs.players[0].name, "Wilfried",
            "slot 0 player name should be the default 'Wilfried'");
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
            assert!(szs.players[slot].ai_active,
                "Tutorial0 slot {slot} should be ai_active");
        }
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
                .parent().unwrap().parent().unwrap()
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
                assert_eq!(szs.players[slot].slot_u16_0x18, expected[slot],
                    "{scenario} slot {slot} byte 0x18");
            }
        }
    }

    #[test]
    fn player4_relationships_table_matches_observed_pattern() {
        // Tutorial0 / Plague / Atoll all share the canonical
        // diplomacy seed shown in `PlayerSlotInit::relationships`.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
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
            assert_eq!(szs.players[slot].relationships,
                [0, 0, 0, 0, 3, 3, 3],
                "active slot {slot} relationship row");
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
                .parent().unwrap().parent().unwrap()
                .join(format!("extracted/Szenes/{stem}.szs"));
            std::fs::read(&path).ok().and_then(|d| SzsFile::parse(&d).ok())
        };

        if let Some(t) = load("Tutorial0") {
            // Tutorial0 leaves 0x1C0 entirely zero.
            for slot in 0..7 {
                assert_eq!(t.players[slot].events_0x1c0, [0; 7],
                    "Tutorial0 slot {slot} 0x1C0 array");
            }
            // 0xC0 array — slot 0 has 3s everywhere except
            // position 5 (natives).
            assert_eq!(t.players[0].relations_0xc0,
                [3, 3, 3, 3, 3, 0, 3]);
            // Slot 5 (natives) — only positions 4, 5 = 3.
            assert_eq!(t.players[5].relations_0xc0,
                [0, 0, 0, 0, 3, 3, 0]);
        }

        if let Some(m) = load("The Magnate0") {
            // Magnate0 events log: slot N (1..=3, the AI rivals)
            // carries N entries of `(N << 8) | 2` against rivals
            // 1..=N. Slots 0 / 4 / 5 stay empty; slot 6 (pirates)
            // carries a different encoding [0x301, 0x303, …].
            assert_eq!(m.players[0].events_0x1c0, [0; 7],
                "slot 0 (player) has no events");
            assert_eq!(m.players[1].events_0x1c0,
                [0x102, 0, 0, 0, 0, 0, 0]);
            assert_eq!(m.players[2].events_0x1c0,
                [0x102, 0x202, 0, 0, 0, 0, 0]);
            assert_eq!(m.players[3].events_0x1c0,
                [0x102, 0x202, 0x302, 0, 0, 0, 0]);
            assert_eq!(m.players[4].events_0x1c0, [0; 7],
                "slot 4 (trader) has no events");
            assert_eq!(m.players[5].events_0x1c0, [0; 7],
                "slot 5 (natives) has no events");
            assert_eq!(m.players[6].events_0x1c0,
                [0x301, 0x303, 0, 0, 0, 0, 0],
                "slot 6 (pirates) carries the distinct encoding");
        }
    }

    #[test]
    fn player4_ai_active_skips_disabled_rivals() {
        // Exile pre-configures but disables some AI rivals via
        // byte 0x0d == 0x01. The audit run counts 21 such slots
        // across shipping content; Exile is one of them.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
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
        assert!(!szs.players[1].ai_active,
            "Exile slot 1 has byte 0x0d == 0x01 → AI disabled");
        assert_eq!(szs.players[3].state_byte, 0x0c);
        assert!(!szs.players[3].ai_active,
            "Exile slot 3 has byte 0x0d == 0x01 → AI disabled");
    }

    #[test]
    fn player4_slot_u32_0x34_grows_with_difficulty() {
        // Magnate0 ships a difficulty-tiered AI roster: stronger
        // rivals carry strictly larger 0x34 bitsets. This is the
        // strongest cross-scenario signal that the field encodes
        // an AI feature/unlock mask.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
            .join("extracted/Szenes/The Magnate0.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Magnate0");
        assert_eq!(szs.players[0].slot_u32_0x34, 0x0000_0003,
            "slot 0 (player) baseline mask");
        assert_eq!(szs.players[1].slot_u32_0x34, 0x003F_C00F,
            "slot 1 (easy AI) mid-tier mask");
        assert_eq!(szs.players[2].slot_u32_0x34, 0x0FFF_C33F,
            "slot 2 (harder AI) wide mask");
        assert_eq!(szs.players[3].slot_u32_0x34, 0x0FFF_C33F,
            "slot 3 (harder AI) wide mask");
        // Strict monotone growth across rivals — 0 ⊂ 1 ⊂ 2.
        let masks: Vec<u32> = (0..4).map(|i| szs.players[i].slot_u32_0x34).collect();
        assert!(masks[0] & masks[1] == masks[0],
            "slot 1 mask is a superset of slot 0");
        assert!(masks[1] & masks[2] == masks[1],
            "slot 2 mask is a superset of slot 1");
    }

    #[test]
    fn stadt4_extracts_city_name() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
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
        let city = szs.islands.iter()
            .find_map(|i| i.city.as_ref())
            .expect("at least one island has a STADT4 city");
        assert_eq!(city.name, "Larrach");
        // Larrach is the player's main settlement in Plague,
        // so it belongs to slot 0. The previous test asserted
        // `owner == 1` against the byte at offset 0, which is
        // actually the island_index — Larrach sits on island 1
        // because Plague's island 0 is an unused sentinel.
        assert_eq!(city.island_index, 1,
            "Larrach is on Plague's island #1 (after the sentinel)");
        assert_eq!(city.owner_slot, 0,
            "Larrach is the player's main settlement");
    }

    #[test]
    fn stadt4_multi_city_scenario_distinguishes_island_from_owner() {
        // New Horizons2 places cities on multiple islands with
        // distinct owner_slots — this is the test that motivated
        // separating the two fields.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
            .join("extracted/Szenes/New Horizons2.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse New Horizons2");
        let cities: Vec<&City> = szs.islands.iter()
            .filter_map(|i| i.city.as_ref())
            .filter(|c| !c.name.is_empty())
            .collect();
        let by_name = |n: &str| cities.iter()
            .find(|c| c.name == n)
            .copied();
        // "Jaricho" sits on island 21 with owner_slot 6 (pirate).
        if let Some(c) = by_name("Jaricho") {
            assert_eq!(c.island_index, 21);
            assert_eq!(c.owner_slot, 6,
                "Jaricho is the pirate stronghold (slot 6)");
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
            assert!(c.owner_slot <= 6,
                "owner_slot must be a valid PLAYER4 slot index, got {}", c.owner_slot);
        }
    }

    #[test]
    fn scenario_meta_extracts_szene_chunks() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
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
            .parent().unwrap().parent().unwrap()
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
        // Tutorial0's lone ship is the human player's small
        // trader: owner = slot 0, ship_class one of the five
        // observed values {0x15, 0x17, 0x19, 0x1B, 0x1F}.
        assert_eq!(szs.ships[0].owner, 0,
            "Tutorial0 starting ship is owned by the human player");
        assert!(matches!(szs.ships[0].ship_class,
                         0x15 | 0x17 | 0x19 | 0x1B | 0x1F),
            "ship_class falls within the observed shipping-corpus set, got 0x{:02X}",
            szs.ships[0].ship_class);
    }

    #[test]
    fn ship4_cargo_slots_carry_three_loaded_entries_in_tutorial0() {
        // Tutorial0 starts the player with one ship loaded with
        // three goods. Audit surfaces the raw u32 cargo entries
        // 0x03C00003, 0x03C00011, 0x03200033 at slot 0 of the
        // SHIP4 record's cargo manifest (high16 = quantity,
        // low16 = good identifier of unknown encoding).
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
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
        assert_eq!(szs.ships[0].cargo_slots,
            [62915075, 62916561, 52429875, 0, 0, 0, 0],
            "Tutorial0 starting cargo");
        // The high 16 bits of each non-zero entry should be a
        // quantity-style multiple of 32 in the observed range.
        for slot in &szs.ships[0].cargo_slots {
            if *slot == 0 { continue; }
            let high = (slot >> 16) as u16;
            assert!(high % 32 == 0,
                "high16 should be a multiple of 32, got 0x{high:04X}");
            assert!(high > 0 && high <= 4000,
                "high16 should fall within observed range, got {high}");
        }
    }

    #[test]
    fn ship4_owner_distribution_covers_player_and_ai() {
        // Plague of Pirates has 19 ships across multiple owners,
        // so it's the best test of the owner-byte interpretation.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
            .join("extracted/Szenes/A Plague of Pirates.szs");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                println!("Skipping: {path:?} not found");
                return;
            }
        };
        let szs = SzsFile::parse(&data).expect("parse Plague");
        let mut owners: std::collections::BTreeSet<u8> =
            std::collections::BTreeSet::new();
        for s in &szs.ships {
            owners.insert(s.owner);
        }
        assert!(owners.contains(&0),
            "Plague includes at least one human-owned ship");
        // The full surveyed set across the corpus is {0,1,2,3,5}
        // — Plague should cover at least 0 and one rival.
        let has_rival = owners.iter().any(|&o| matches!(o, 1 | 2 | 3 | 5));
        assert!(has_rival,
            "Plague should also include a non-player owner; got {owners:?}");
    }

    #[test]
    fn auftrag4_flag_bits_decode() {
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
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
        assert!(m.flags & MISSION_FLAG_POPULATION != 0,
            "Plague must have population bit");
        assert!(m.flags & MISSION_FLAG_PIRATE != 0,
            "Plague must have pirate bit");
        // Good Neighbors: pop + cooperative neighbour goal.
        let m = load("Good Neighbors.szs").expect("Good Neighbors");
        assert!(m.flags & MISSION_FLAG_POPULATION != 0);
        assert!(m.flags & MISSION_FLAG_COOPERATIVE != 0);
        assert!(m.flags & MISSION_FLAG_PIRATE == 0,
            "Good Neighbors has no pirate combat");
        // Tutorials carry no flags.
        let m = load("Tutorial0.szs").expect("Tutorial0");
        assert_eq!(m.flags, 0);
    }

    #[test]
    fn mission_goals_decode_per_scenario() {
        let scenes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() { return; }
        let load = |name: &str| -> Option<Mission> {
            SzsFile::parse(&std::fs::read(scenes.join(name)).ok()?).ok()?.mission
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
            .parent().unwrap().parent().unwrap()
            .join("extracted/Szenes");
        if !scenes.exists() { return; }
        let load = |name: &str| -> Option<Mission> {
            SzsFile::parse(&std::fs::read(scenes.join(name)).ok()?).ok()?.mission
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
            .parent().unwrap().parent().unwrap()
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
        assert!(mission.briefing.starts_with("You have managed to lead"),
            "briefing text wrong: {:?}", &mission.briefing);
        assert!(mission.briefing.contains("5,000 inhabitants"),
            "briefing should mention the 5 000 inhabitants goal");
        // Primary pop threshold is the first u32 of the goals
        // region; for Plague this is 0x1388 = 5000.
        let pop = u32::from_le_bytes([
            mission.goals_raw[0], mission.goals_raw[1],
            mission.goals_raw[2], mission.goals_raw[3],
        ]);
        assert_eq!(pop, 5000, "goals_raw[0..4] should encode 5 000 pop");
    }
}
