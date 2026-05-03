//! Player-issued commands. Used by the multiplayer layer to ship intent
//! from clients to the authoritative host instead of relying purely on
//! state-snapshot replication.
//!
//! Wire format: client encodes one of these via `bincode` and prefixes the
//! payload with [`COMMAND_TAG`] so the host can distinguish it from
//! anything else flowing over the same `NetMessage::GameData` channel.

use crate::combat::Diplomacy;
use crate::types::Good;

/// Magic prefix byte. Snapshots don't use a tag (see `Simulation::snapshot`),
/// so only client → host commands carry this.
pub const COMMAND_TAG: u8 = 0x43; // 'C'

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Command {
    /// Adjust a tier's tax rate on player slot `player`.
    SetTaxRate { player: u8, tier: u8, rate: u8 },
    /// Set bilateral relation between `a` and `b` (kept symmetric).
    SetDiplomacy { a: u8, b: u8, state: Diplomacy },
    /// Buy `qty` of `good` at the current market price (deducts gold).
    Buy { player: u8, good: Good, qty: u16 },
    /// Sell `qty` of `good` at the current market price (credits gold).
    Sell { player: u8, good: Good, qty: u16 },
    /// Send a gift of gold from `from` to `to`. Anno 1602 manual:
    /// `Pay tribute` action in the diplomacy panel — players may
    /// transfer gold to another player at any time. We allow only
    /// non-negative amounts and clamp to the sender's balance.
    GiftGold { from: u8, to: u8, amount: i32 },
    /// Send a gift of `qty` `good` from `from` to `to` via their
    /// active warehouses. Drains from the sender's first matching
    /// warehouse, deposits into the recipient's first matching one.
    GiftGoods { from: u8, to: u8, good: Good, qty: u16 },
    /// Market-wagon trade: transfer `qty` of `good` between two of
    /// `player`'s active warehouses (by index). Manual section 8.2
    /// "Trading with other cities on your island": the KARREN
    /// figure (figuren.cod `Nummer: KARREN`, `Maxtrag: 6`) is the
    /// player-driven overland transport between warehouses on the
    /// same island chain. We model the gameplay effect (goods move
    /// between warehouses) without yet simulating the wagon's walk;
    /// per-trip `qty` is clamped to KARREN's 6-good capacity.
    DispatchCart {
        player: u8,
        from_warehouse: u16,
        to_warehouse: u16,
        good: Good,
        qty: u16,
    },
}

impl Command {
    /// Encode with the magic tag prefix ready for `NetMessage::game_data`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![COMMAND_TAG];
        if let Ok(b) = bincode::serialize(self) {
            out.extend_from_slice(&b);
        }
        out
    }

    /// Try to decode a command from a tag-prefixed payload. Returns None if
    /// the payload doesn't start with `COMMAND_TAG` or fails bincode.
    pub fn decode(payload: &[u8]) -> Option<Self> {
        if payload.first().copied() != Some(COMMAND_TAG) {
            return None;
        }
        bincode::deserialize(&payload[1..]).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_tax_command() {
        let c = Command::SetTaxRate { player: 0, tier: 2, rate: 96 };
        let encoded = c.encode();
        assert_eq!(encoded[0], COMMAND_TAG);
        let back = Command::decode(&encoded).expect("decode");
        match back {
            Command::SetTaxRate { player, tier, rate } => {
                assert_eq!((player, tier, rate), (0, 2, 96));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn rejects_untagged_payload() {
        // A bincoded SaveState begins with its version u32 = small number,
        // not COMMAND_TAG, so decode must refuse it.
        let snap_payload = vec![10u8, 0, 0, 0, 0, 0, 0, 0];
        assert!(Command::decode(&snap_payload).is_none());
    }

    #[test]
    fn round_trip_buy_sell() {
        let buy = Command::Buy { player: 0, good: Good::Wood, qty: 25 };
        let back = Command::decode(&buy.encode()).unwrap();
        if let Command::Buy { qty, .. } = back {
            assert_eq!(qty, 25);
        } else { panic!(); }
    }
}
