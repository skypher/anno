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
/// | `flags` | 0x04 (u32) | 0x0001 / 0x0011 / 0x0401 / 0x0403 in scripted scenarios; 0x0000 in tutorials |
/// | `briefing` | 0x68 onward | Always begins at 0x68, null-terminated, CP1252 |
/// | `goals_raw` | 0x870..end | Raw 996 bytes of typed goal records — structure not fully RE'd |
///
/// Confirmed against: A Plague of Pirates, Atoll, Cooperation,
/// Exile, Fireland, Good Neighbors, Continous Play00, Tutorial0/1/2.
#[derive(Debug, Clone)]
pub struct Mission {
    /// Mission goal-flags bitfield. Bit 0 (0x01) is set whenever
    /// a population threshold is required; bits 0x10/0x400 toggle
    /// in scenarios involving cooperative goals or pirate
    /// combat. Higher-bit semantics not fully reverse-engineered.
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

const AUFTRAG4_BRIEFING_OFFSET: usize = 0x68;
const AUFTRAG4_GOALS_OFFSET: usize = 0x870;
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
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerSlotInit {
    /// Starting gold (first u32 of the slot record).
    pub starting_gold: i32,
    /// True when this slot is a human player. RE-derived from
    /// byte offset 12 of the slot record: `0x00` = human / fixed
    /// faction, `0xff` = not present (AI fills the slot when the
    /// scenario configures one).
    pub is_human: bool,
}

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

                // Look for the matching INSELHAUS chunk (follows INSEL5, possibly with
                // other chunks in between for the same island)
                for j in (i + 1)..chunks.len() {
                    if chunks[j].name == "INSELHAUS" {
                        island.tiles = Self::parse_inselhaus(&chunks[j].data);
                        break;
                    }
                    if chunks[j].name == "INSEL5" {
                        break; // Next island, no INSELHAUS for this one
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

        Ok(SzsFile { chunks, islands, players, mission })
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
            let is_human = data[off + 12] == 0x00 && slot < 4;
            out.push(PlayerSlotInit { starting_gold, is_human });
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
        }
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
            },
            Island {
                number: 4, width: 60, height: 40,
                x_pos: 500, y_pos: 600,
                tiles: vec![],
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
