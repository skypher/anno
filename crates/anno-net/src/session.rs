//! Multiplayer session management.
//!
//! Reverse-engineered from MaxnetCreate, MaxnetSetPlayer, MaxnetSetNetGroup,
//! MaxnetGetNetGroupStruct, and the NetGroupStruct (0x1BC bytes).
//!
//! The original game uses a global state block at DAT_10008090 (the "session object")
//! which is 0x410 bytes. Key offsets:
//!   0x000: connected flag (DAT_100080d0)
//!   0x024: pause bitmask (DAT_100080b4)
//!   0x028: dialog HWND
//!   0x048: local player ID
//!   0x0B4: player count
//!   0x0B8: active player count
//!   0x0BC: own DirectPlay player ID
//!   0x0C8: session GUID
//!   0x108-0x25F: 4 player slots (0x54 bytes each)
//!   0x284: receive thread handle
//!   0x28C: receive buffer
//!   0x408: IDirectPlay interface pointer

use crate::protocol::MAX_PLAYERS;

/// Player information for a single slot.
#[derive(Debug, Clone)]
pub struct PlayerInfo {
    /// Player ID (from DirectPlay, now assigned by host).
    pub player_id: i32,
    /// Player number (0-3).
    pub player_num: i32,
    /// Long display name.
    pub long_name: String,
    /// Short name.
    pub short_name: String,
    /// Readiness state.
    pub ready_state: u32,
    /// Whether this slot is occupied.
    pub active: bool,
}

impl PlayerInfo {
    pub fn empty() -> Self {
        Self {
            player_id: -1,
            player_num: -1,
            long_name: String::new(),
            short_name: String::new(),
            ready_state: 0,
            active: false,
        }
    }

    pub fn new(player_id: i32, player_num: i32, long_name: &str, short_name: &str) -> Self {
        Self {
            player_id,
            player_num,
            long_name: long_name.to_string(),
            short_name: short_name.to_string(),
            ready_state: 0,
            active: true,
        }
    }
}

/// Session state — corresponds to the NetGroupStruct (0x1BC bytes).
///
/// From MaxnetClrNetGroupStruct (FUN_10002840):
/// - param_1[0] = 0x1BC (struct size marker)
/// - param_1[2] = 1 (session flags)
/// - param_1[7] = 0 initially
/// - param_1[8..10] = -1 (unused slots)
/// - param_1[0x1C..] = 4 player slot entries
#[derive(Debug, Clone)]
pub struct Session {
    /// Unique session ID.
    pub session_id: u32,
    /// Session name.
    pub name: String,
    /// Host player index.
    pub host_player: usize,
    /// Maximum players allowed.
    pub max_players: u32,
    /// Current player count.
    pub player_count: u32,
    /// Player slots.
    pub players: [PlayerInfo; MAX_PLAYERS],
    /// Per-player pause bitmask (from DAT_100080b4).
    /// Bit N set = player N is paused.
    pub pause_mask: u32,
    /// Whether the session is active.
    pub active: bool,
    /// Whether we are the host (created the session).
    pub is_host: bool,
    /// Local player index in the players array.
    pub local_player_idx: Option<usize>,
}

impl Session {
    pub fn new(name: &str) -> Self {
        Self {
            session_id: 0,
            name: name.to_string(),
            host_player: 0,
            max_players: MAX_PLAYERS as u32,
            player_count: 0,
            players: [
                PlayerInfo::empty(),
                PlayerInfo::empty(),
                PlayerInfo::empty(),
                PlayerInfo::empty(),
            ],
            pause_mask: 0,
            active: false,
            is_host: false,
            local_player_idx: None,
        }
    }

    /// Add a player to the first available slot. Returns slot index.
    pub fn add_player(&mut self, player_id: i32, long_name: &str, short_name: &str) -> Option<usize> {
        // From FUN_100025a0: iterates slots looking for player_id == -1 (empty)
        // or matching names, stores up to 4
        for i in 0..MAX_PLAYERS {
            if !self.players[i].active {
                self.players[i] = PlayerInfo::new(player_id, i as i32, long_name, short_name);
                self.player_count += 1;
                return Some(i);
            }
        }
        None
    }

    /// Remove a player by ID. Returns the slot index if found.
    ///
    /// From FUN_10002670: finds player by ID, zeros the slot, sets to -1,
    /// decrements player count. If < 2 players remain, signals disconnect.
    pub fn remove_player(&mut self, player_id: i32) -> Option<usize> {
        for i in 0..MAX_PLAYERS {
            if self.players[i].active && self.players[i].player_id == player_id {
                self.players[i] = PlayerInfo::empty();
                self.player_count = self.player_count.saturating_sub(1);
                // Clear pause bit for this player
                self.pause_mask &= !(1 << i);
                return Some(i);
            }
        }
        None
    }

    /// Find player slot by ID.
    pub fn find_player(&self, player_id: i32) -> Option<usize> {
        self.players.iter().position(|p| p.active && p.player_id == player_id)
    }

    /// Set pause for a player (from MaxnetPause: sets bit in bitmask).
    pub fn set_pause(&mut self, player_idx: usize) {
        if player_idx < MAX_PLAYERS {
            self.pause_mask |= 1 << player_idx;
        }
    }

    /// Clear pause for a player (from MaxnetRun: clears bit in bitmask).
    pub fn clear_pause(&mut self, player_idx: usize) {
        if player_idx < MAX_PLAYERS {
            self.pause_mask &= !(1 << player_idx);
        }
    }

    /// Check if the game is paused (any player has pause set).
    pub fn is_paused(&self) -> bool {
        self.pause_mask != 0
    }

    /// Check if we have enough players to play (>= 2 from FUN_100017d0 check).
    pub fn has_enough_players(&self) -> bool {
        self.player_count >= 2
    }
}

/// Events emitted by the session for the game to handle.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// A new player joined.
    PlayerJoined { slot: usize, player_id: i32, name: String },
    /// A player left.
    PlayerLeft { slot: usize, player_id: i32 },
    /// Game data received from a player.
    GameData { from_player: i32, data: Vec<u8> },
    /// Pause state changed.
    PauseChanged { paused: bool, pause_mask: u32 },
    /// Chat message received.
    Chat { from_player: i32, text: String },
    /// Session ended (host left or too few players).
    SessionEnded,
    /// Connection lost.
    Disconnected { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_add_remove_players() {
        let mut session = Session::new("Test Game");
        assert_eq!(session.player_count, 0);

        let idx = session.add_player(100, "Player1", "P1").unwrap();
        assert_eq!(idx, 0);
        assert_eq!(session.player_count, 1);

        let idx = session.add_player(200, "Player2", "P2").unwrap();
        assert_eq!(idx, 1);
        assert_eq!(session.player_count, 2);
        assert!(session.has_enough_players());

        session.remove_player(100);
        assert_eq!(session.player_count, 1);
        assert!(!session.has_enough_players());
    }

    #[test]
    fn session_max_players() {
        let mut session = Session::new("Full Game");
        for i in 0..MAX_PLAYERS {
            assert!(session.add_player(i as i32, &format!("P{}", i), &format!("p{}", i)).is_some());
        }
        assert!(session.add_player(99, "Extra", "E").is_none());
    }

    #[test]
    fn pause_bitmask() {
        let mut session = Session::new("Test");
        session.add_player(1, "P1", "p1");
        session.add_player(2, "P2", "p2");

        assert!(!session.is_paused());
        session.set_pause(0);
        assert!(session.is_paused());
        assert_eq!(session.pause_mask, 1);

        session.set_pause(1);
        assert_eq!(session.pause_mask, 3);

        session.clear_pause(0);
        assert_eq!(session.pause_mask, 2);
        assert!(session.is_paused());

        session.clear_pause(1);
        assert!(!session.is_paused());
    }
}
