//! TCP transport layer replacing DirectPlay.
//!
//! The original Maxnet.dll uses DirectPlay (DPLAYX.dll) vtable calls:
//!   offset 0x14: CreatePlayer
//!   offset 0x18: CreateGroup + AddPlayerToGroup
//!   offset 0x1C: DeletePlayerFromGroup
//!   offset 0x24: DestroyPlayer
//!   offset 0x2C: EnumSessions
//!   offset 0x34: EnumPlayers (for session listing)
//!   offset 0x38: GetCaps (get send buffer size, capped to 0x4000)
//!   offset 0x54: GetPlayerName
//!   offset 0x60: Open (join session)
//!   offset 0x64: Receive
//!   offset 0x68: Send
//!   offset 0x10: Close
//!
//! The receive thread (FUN_10002920 → LAB_10001530 → FUN_10002c60) loops calling
//! Receive on the DirectPlay object and dispatches messages through the switch
//! statement in FUN_10002c60.
//!
//! This module replaces DirectPlay with TCP sockets. The host listens for
//! connections; clients connect to the host. Messages use the same wire format
//! as the original protocol.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, SocketAddr};
use crate::protocol::{MessageHeader, NetMessage, MAX_PLAYERS, MAX_SEND_BUFFER};
use crate::session::{Session, SessionEvent};

/// Default port for Anno 1602 multiplayer.
pub const DEFAULT_PORT: u16 = 2300;

/// Connection to a remote peer.
struct PeerConnection {
    stream: TcpStream,
    player_id: i32,
    recv_buffer: Vec<u8>,
    /// Reassembly buffer for fragmented messages (from 0x7DC handler).
    frag_buffer: Option<Vec<u8>>,
    frag_expected_size: usize,
}

impl PeerConnection {
    fn new(stream: TcpStream, player_id: i32) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            player_id,
            recv_buffer: Vec::with_capacity(MAX_SEND_BUFFER),
            frag_buffer: None,
            frag_expected_size: 0,
        })
    }

    /// Try to read available data from the stream.
    fn read_available(&mut self) -> io::Result<usize> {
        let mut tmp = [0u8; 4096];
        match self.stream.read(&mut tmp) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::ConnectionReset, "peer disconnected")),
            Ok(n) => {
                self.recv_buffer.extend_from_slice(&tmp[..n]);
                Ok(n)
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Try to parse complete messages from the receive buffer.
    fn drain_messages(&mut self) -> Vec<NetMessage> {
        let mut messages = Vec::new();
        loop {
            match NetMessage::decode(&self.recv_buffer) {
                Some((msg, consumed)) => {
                    self.recv_buffer.drain(..consumed);
                    messages.push(msg);
                }
                None => break,
            }
        }
        messages
    }

    /// Send a message to this peer.
    fn send(&mut self, msg: &NetMessage) -> io::Result<()> {
        let data = msg.encode();
        // Fragment if needed (matching MaxnetSendConfirm's fragmentation at 0x4000)
        if data.len() <= MAX_SEND_BUFFER {
            self.stream.write_all(&data)?;
        } else {
            // Send first chunk
            self.stream.write_all(&data[..MAX_SEND_BUFFER])?;
            // Send remaining as continuation fragments
            let mut offset = MAX_SEND_BUFFER;
            while offset < data.len() {
                let chunk_size = (data.len() - offset).min(MAX_SEND_BUFFER - MessageHeader::SIZE);
                let frag = NetMessage::new(
                    crate::protocol::MessageId::FragContinuation,
                    data[offset..offset + chunk_size].to_vec(),
                );
                self.stream.write_all(&frag.encode())?;
                offset += chunk_size;
            }
        }
        self.stream.flush()
    }
}

/// Host-side multiplayer server.
pub struct NetHost {
    listener: TcpListener,
    peers: Vec<PeerConnection>,
    session: Session,
    next_player_id: i32,
    events: Vec<SessionEvent>,
}

impl NetHost {
    /// Create a new host listening on the given address.
    pub fn bind(addr: SocketAddr, session_name: &str) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;

        let mut session = Session::new(session_name);
        session.is_host = true;
        session.active = true;

        // Host is always player 0
        session.add_player(0, "Host", "H");
        session.local_player_idx = Some(0);

        Ok(Self {
            listener,
            peers: Vec::new(),
            session,
            next_player_id: 1,
            events: Vec::new(),
        })
    }

    /// Poll for new connections and incoming messages.
    pub fn poll(&mut self) -> Vec<SessionEvent> {
        self.events.clear();
        self.accept_connections();
        self.receive_messages();
        std::mem::take(&mut self.events)
    }

    /// Accept pending connections.
    fn accept_connections(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    if self.peers.len() >= MAX_PLAYERS - 1 {
                        // Session full — drop connection
                        drop(stream);
                        continue;
                    }
                    let player_id = self.next_player_id;
                    self.next_player_id += 1;

                    match PeerConnection::new(stream, player_id) {
                        Ok(peer) => {
                            let name = format!("Player{}", player_id);
                            if let Some(slot) = self.session.add_player(player_id, &name, &format!("P{}", player_id)) {
                                self.events.push(SessionEvent::PlayerJoined {
                                    slot,
                                    player_id,
                                    name: name.clone(),
                                });
                            }
                            self.peers.push(peer);
                        }
                        Err(_) => {}
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    /// Receive and dispatch messages from all peers.
    fn receive_messages(&mut self) {
        let mut disconnected = Vec::new();

        for i in 0..self.peers.len() {
            match self.peers[i].read_available() {
                Ok(_) => {
                    let messages = self.peers[i].drain_messages();
                    let player_id = self.peers[i].player_id;
                    for msg in messages {
                        self.dispatch_message(player_id, &msg);
                    }
                }
                Err(_) => {
                    disconnected.push(i);
                }
            }
        }

        // Remove disconnected peers (reverse order to preserve indices)
        for &i in disconnected.iter().rev() {
            let player_id = self.peers[i].player_id;
            self.peers.remove(i);
            if let Some(slot) = self.session.remove_player(player_id) {
                self.events.push(SessionEvent::PlayerLeft { slot, player_id });
            }
            if !self.session.has_enough_players() {
                self.events.push(SessionEvent::SessionEnded);
            }
        }
    }

    /// Dispatch a received message based on command ID.
    /// Mirrors the switch statement in FUN_10002c60.
    fn dispatch_message(&mut self, from_player: i32, msg: &NetMessage) {
        match crate::protocol::MessageId::from_u32(msg.header.command_id) {
            Some(crate::protocol::MessageId::GameData) => {
                self.events.push(SessionEvent::GameData {
                    from_player,
                    data: msg.payload.clone(),
                });
                // Forward to all other peers (host acts as relay)
                self.broadcast_except(msg, from_player);
            }
            Some(crate::protocol::MessageId::Pause) => {
                if let Some(idx) = self.session.find_player(from_player) {
                    self.session.set_pause(idx);
                    self.events.push(SessionEvent::PauseChanged {
                        paused: true,
                        pause_mask: self.session.pause_mask,
                    });
                    self.broadcast(msg);
                }
            }
            Some(crate::protocol::MessageId::Resume) => {
                if let Some(idx) = self.session.find_player(from_player) {
                    self.session.clear_pause(idx);
                    self.events.push(SessionEvent::PauseChanged {
                        paused: self.session.is_paused(),
                        pause_mask: self.session.pause_mask,
                    });
                    self.broadcast(msg);
                }
            }
            Some(crate::protocol::MessageId::ChatMessage) => {
                let text = String::from_utf8_lossy(&msg.payload)
                    .trim_end_matches('\0')
                    .to_string();
                self.events.push(SessionEvent::Chat {
                    from_player,
                    text,
                });
                self.broadcast_except(msg, from_player);
            }
            Some(crate::protocol::MessageId::PlayerDisconnect) => {
                if let Some(slot) = self.session.remove_player(from_player) {
                    self.events.push(SessionEvent::PlayerLeft {
                        slot,
                        player_id: from_player,
                    });
                    self.broadcast(msg);
                }
            }
            _ => {}
        }
    }

    /// Send a message to all peers.
    fn broadcast(&mut self, msg: &NetMessage) {
        for peer in &mut self.peers {
            let _ = peer.send(msg);
        }
    }

    /// Send a message to all peers except the given player.
    fn broadcast_except(&mut self, msg: &NetMessage, except_player: i32) {
        for peer in &mut self.peers {
            if peer.player_id != except_player {
                let _ = peer.send(msg);
            }
        }
    }

    /// Send a message from the host to all peers.
    pub fn send_to_all(&mut self, msg: &NetMessage) {
        self.broadcast(msg);
    }

    /// Send a message to a specific player.
    pub fn send_to(&mut self, player_id: i32, msg: &NetMessage) {
        if let Some(peer) = self.peers.iter_mut().find(|p| p.player_id == player_id) {
            let _ = peer.send(msg);
        }
    }

    /// Get current session state.
    pub fn session(&self) -> &Session {
        &self.session
    }
}

/// Client-side connection to a host.
pub struct NetClient {
    connection: PeerConnection,
    session: Session,
    events: Vec<SessionEvent>,
}

impl NetClient {
    /// Connect to a host.
    pub fn connect(addr: SocketAddr, _player_name: &str) -> io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        let connection = PeerConnection::new(stream, 0)?; // ID assigned by host

        let mut session = Session::new("Remote");
        session.is_host = false;

        Ok(Self {
            connection,
            session,
            events: Vec::new(),
        })
    }

    /// Poll for incoming messages.
    pub fn poll(&mut self) -> Vec<SessionEvent> {
        self.events.clear();

        match self.connection.read_available() {
            Ok(_) => {
                let messages = self.connection.drain_messages();
                for msg in messages {
                    self.dispatch_message(&msg);
                }
            }
            Err(_) => {
                self.events.push(SessionEvent::Disconnected {
                    reason: "Connection lost".to_string(),
                });
            }
        }

        std::mem::take(&mut self.events)
    }

    fn dispatch_message(&mut self, msg: &NetMessage) {
        match crate::protocol::MessageId::from_u32(msg.header.command_id) {
            Some(crate::protocol::MessageId::GameData) => {
                self.events.push(SessionEvent::GameData {
                    from_player: 0, // From host
                    data: msg.payload.clone(),
                });
            }
            Some(crate::protocol::MessageId::Pause) => {
                self.events.push(SessionEvent::PauseChanged {
                    paused: true,
                    pause_mask: 0,
                });
            }
            Some(crate::protocol::MessageId::Resume) => {
                self.events.push(SessionEvent::PauseChanged {
                    paused: false,
                    pause_mask: 0,
                });
            }
            Some(crate::protocol::MessageId::ChatMessage) => {
                let text = String::from_utf8_lossy(&msg.payload)
                    .trim_end_matches('\0')
                    .to_string();
                self.events.push(SessionEvent::Chat {
                    from_player: 0,
                    text,
                });
            }
            Some(crate::protocol::MessageId::PlayerDisconnect) => {
                self.events.push(SessionEvent::SessionEnded);
            }
            _ => {}
        }
    }

    /// Send a message to the host.
    pub fn send(&mut self, msg: &NetMessage) -> io::Result<()> {
        self.connection.send(msg)
    }

    /// Get current session state.
    pub fn session(&self) -> &Session {
        &self.session
    }
}
