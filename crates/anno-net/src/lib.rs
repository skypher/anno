//! Anno 1602 multiplayer networking.
//!
//! Reverse-engineered from Maxnet.dll, which wraps DirectPlay (DPLAYX.dll)
//! for peer-to-peer multiplayer. This crate reimplements the protocol using
//! TCP sockets instead of DirectPlay.
//!
//! # Protocol overview (from Maxnet.dll decompilation)
//!
//! Maxnet.dll provides a thin abstraction over DirectPlay with these key features:
//!
//! - **Session management**: Create/join sessions with up to 4 players + host
//! - **Message passing**: Two modes — fire-and-forget (`MaxnetSend`) and
//!   confirmed delivery (`MaxnetSendConfirm`) with ACK-based flow control
//! - **Synchronization**: Pause/Run commands broadcast to all players
//!   (message IDs 0x7D1/0x7D2) with per-player pause bitmask
//! - **Player management**: 4 player slots tracked by DirectPlay player ID,
//!   with long name + short name identification
//! - **Large message fragmentation**: Messages exceeding the DirectPlay
//!   guaranteed-send buffer size are split into chunks (0x7DC continuation)
//!   and reassembled on the receiver side
//!
//! # Message format
//!
//! Every message starts with a header:
//! ```text
//! [u32 command_id] [u32 total_size] [payload...]
//! ```
//!
//! The `command_id` field uses these values (from the receive handler switch):
//!
//! | ID     | Name              | Description |
//! |--------|-------------------|-------------|
//! | 0x7D0  | GameData          | Application game data (forwarded to game callback) |
//! | 0x7D1  | Pause             | Player requests pause |
//! | 0x7D2  | Resume            | Player resumes |
//! | 0x7D5  | (ignored)         | No-op in receive handler |
//! | 0x7D6  | (ignored)         | No-op in receive handler |
//! | 0x7D7  | Ack               | Confirmed-send acknowledgement |
//! | 0x7D8  | PlayerSync        | Broadcasts player readiness states (0x2C bytes) |
//! | 0x7D9  | SessionInfo       | Session metadata sync |
//! | 0x7DA  | (ignored)         | No-op in receive handler |
//! | 0x7DB  | ChatMessage       | Chat message forwarded via PostMessage |
//! | 0x7DC  | FragContinuation  | Continuation fragment for large messages |
//! | 0x7DD  | PlayerDisconnect  | Player leaving notification |
//!
//! # NetGroupStruct
//!
//! The shared session state struct is 0x1BC (444) bytes and contains:
//! - Byte 0: struct size marker (0x1BC)
//! - Byte 8: session flags
//! - Bytes 0x1C-0x15F: 4 player slot entries (0x54 bytes each)
//!   - Each slot: [player_id: i32] [player_num: i32] [long_name: [u8; 0x34]] [short_name: [u8; 0x20]]
//! - Various session parameters copied from MaxnetCreate args

pub mod protocol;
pub mod session;
pub mod transport;
