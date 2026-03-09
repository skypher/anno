//! Message protocol definitions.
//!
//! Reverse-engineered from the Maxnet.dll receive handler (FUN_10002c60)
//! and send functions (MaxnetSend, MaxnetSendConfirm).

/// Maximum number of player slots (from decompiled player slot iteration: 4 iterations of 0x54-byte structs).
pub const MAX_PLAYERS: usize = 4;

/// Size of the NetGroupStruct in bytes (0x1BC = 444, from FUN_10002840 / MaxnetClrNetGroupStruct).
pub const NET_GROUP_STRUCT_SIZE: usize = 0x1BC;

/// Maximum guaranteed send buffer size (0x4000 = 16384 bytes, from FUN_10002920 cap).
pub const MAX_SEND_BUFFER: usize = 0x4000;

/// Player slot entry size in bytes (0x54 = 84, from player iteration stride).
pub const PLAYER_SLOT_SIZE: usize = 0x54;

/// Long name max length (0x34 = 52 bytes, from lstrcpynA calls).
pub const LONG_NAME_LEN: usize = 0x34;

/// Short name max length (0x20 = 32 bytes, from lstrcpynA calls).
pub const SHORT_NAME_LEN: usize = 0x20;

/// Message command IDs from the receive handler switch statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MessageId {
    /// Application game data — forwarded to game callback (default case in switch).
    GameData = 0x7D0,
    /// Player requests pause (sets bit in pause bitmask).
    Pause = 0x7D1,
    /// Player resumes (clears bit in pause bitmask).
    Resume = 0x7D2,
    /// Confirmed-send acknowledgement.
    Ack = 0x7D7,
    /// Player readiness/sync state broadcast (0x2C bytes payload).
    PlayerSync = 0x7D8,
    /// Session info metadata sync.
    SessionInfo = 0x7D9,
    /// Chat message (forwarded via PostMessage to game window).
    ChatMessage = 0x7DB,
    /// Continuation fragment for large messages split across packets.
    FragContinuation = 0x7DC,
    /// Player disconnect notification.
    PlayerDisconnect = 0x7DD,
}

impl MessageId {
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0x7D0 => Some(Self::GameData),
            0x7D1 => Some(Self::Pause),
            0x7D2 => Some(Self::Resume),
            0x7D7 => Some(Self::Ack),
            0x7D8 => Some(Self::PlayerSync),
            0x7D9 => Some(Self::SessionInfo),
            0x7DB => Some(Self::ChatMessage),
            0x7DC => Some(Self::FragContinuation),
            0x7DD => Some(Self::PlayerDisconnect),
            _ => None,
        }
    }
}

/// Wire message header: every message starts with [command_id: u32] [total_size: u32].
#[derive(Debug, Clone)]
pub struct MessageHeader {
    pub command_id: u32,
    pub total_size: u32,
}

impl MessageHeader {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; 8] {
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&self.command_id.to_le_bytes());
        buf[4..8].copy_from_slice(&self.total_size.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8; 8]) -> Self {
        Self {
            command_id: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            total_size: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        }
    }
}

/// A complete network message.
#[derive(Debug, Clone)]
pub struct NetMessage {
    pub header: MessageHeader,
    pub payload: Vec<u8>,
}

impl NetMessage {
    /// Create a new message.
    pub fn new(command_id: MessageId, payload: Vec<u8>) -> Self {
        let total_size = (MessageHeader::SIZE + payload.len()) as u32;
        Self {
            header: MessageHeader {
                command_id: command_id as u32,
                total_size,
            },
            payload,
        }
    }

    /// Create a game data message (the most common type).
    pub fn game_data(payload: Vec<u8>) -> Self {
        Self::new(MessageId::GameData, payload)
    }

    /// Create a pause message.
    pub fn pause() -> Self {
        Self::new(MessageId::Pause, Vec::new())
    }

    /// Create a resume message.
    pub fn resume() -> Self {
        Self::new(MessageId::Resume, Vec::new())
    }

    /// Create a player disconnect message with player readiness state.
    pub fn player_disconnect(player_ready_state: u32, player_id: u32) -> Self {
        let mut payload = Vec::with_capacity(8);
        payload.extend_from_slice(&player_ready_state.to_le_bytes());
        payload.extend_from_slice(&player_id.to_le_bytes());
        Self::new(MessageId::PlayerDisconnect, payload)
    }

    /// Create a chat message.
    pub fn chat(text: &str) -> Self {
        let mut payload = text.as_bytes().to_vec();
        payload.push(0); // null terminator
        Self::new(MessageId::ChatMessage, payload)
    }

    /// Encode to bytes for transmission.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(MessageHeader::SIZE + self.payload.len());
        buf.extend_from_slice(&self.header.encode());
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Decode from a byte buffer. Returns (message, bytes_consumed).
    pub fn decode(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < MessageHeader::SIZE {
            return None;
        }
        let header = MessageHeader::decode(buf[0..8].try_into().ok()?);
        let total = header.total_size as usize;
        if buf.len() < total {
            return None;
        }
        let payload = buf[MessageHeader::SIZE..total].to_vec();
        Some((
            Self { header, payload },
            total,
        ))
    }
}

/// Confirmed send state — tracks acknowledgements from peers.
///
/// From MaxnetSendConfirm: sets high bit (0x80000000) on command_id for
/// confirmed sends. Receiver sends back Ack (0x7D7). Sender waits on
/// event with 15-second timeout (WaitForSingleObject 15000ms).
#[derive(Debug, Clone)]
pub struct ConfirmState {
    /// Number of acks expected.
    pub expected_acks: u32,
    /// Number of acks received so far.
    pub received_acks: u32,
    /// Whether confirmation is required (MaxnetSetConfirm / MaxnetClrConfirm).
    pub confirm_required: bool,
}

impl ConfirmState {
    pub fn new() -> Self {
        Self {
            expected_acks: 0,
            received_acks: 0,
            confirm_required: false,
        }
    }

    pub fn is_confirmed(&self) -> bool {
        self.received_acks >= self.expected_acks
    }
}

/// Player sync data broadcast (MessageId::PlayerSync, 0x2C = 44 bytes).
///
/// From FUN_10002790: sends 4 pairs of (player_ready_state, player_id)
/// pulled from the global player state array at DAT_10008198.
#[derive(Debug, Clone)]
pub struct PlayerSyncData {
    pub players: [(u32, u32); MAX_PLAYERS], // (ready_state, player_id)
}

impl PlayerSyncData {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32);
        for (ready, id) in &self.players {
            buf.extend_from_slice(&ready.to_le_bytes());
            buf.extend_from_slice(&id.to_le_bytes());
        }
        buf
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 32 {
            return None;
        }
        let mut players = [(0u32, 0u32); MAX_PLAYERS];
        for i in 0..MAX_PLAYERS {
            let off = i * 8;
            let ready = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
            let id = u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]]);
            players[i] = (ready, id);
        }
        Some(Self { players })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip() {
        let msg = NetMessage::game_data(vec![1, 2, 3, 4]);
        let encoded = msg.encode();
        let (decoded, consumed) = NetMessage::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.header.command_id, MessageId::GameData as u32);
        assert_eq!(decoded.payload, vec![1, 2, 3, 4]);
    }

    #[test]
    fn pause_resume_messages() {
        let pause = NetMessage::pause();
        assert_eq!(pause.header.command_id, MessageId::Pause as u32);
        assert_eq!(pause.header.total_size, 8); // header only

        let resume = NetMessage::resume();
        assert_eq!(resume.header.command_id, MessageId::Resume as u32);
    }

    #[test]
    fn player_sync_roundtrip() {
        let sync = PlayerSyncData {
            players: [(1, 100), (2, 200), (0, 0), (0, 0)],
        };
        let encoded = sync.encode();
        let decoded = PlayerSyncData::decode(&encoded).unwrap();
        assert_eq!(decoded.players[0], (1, 100));
        assert_eq!(decoded.players[1], (2, 200));
    }

    #[test]
    fn disconnect_message() {
        let msg = NetMessage::player_disconnect(42, 7);
        let (decoded, _) = NetMessage::decode(&msg.encode()).unwrap();
        assert_eq!(decoded.header.command_id, MessageId::PlayerDisconnect as u32);
        assert_eq!(decoded.payload.len(), 8);
    }

    #[test]
    fn message_id_from_u32() {
        assert_eq!(MessageId::from_u32(0x7D0), Some(MessageId::GameData));
        assert_eq!(MessageId::from_u32(0x7DD), Some(MessageId::PlayerDisconnect));
        assert_eq!(MessageId::from_u32(0x999), None);
    }
}
