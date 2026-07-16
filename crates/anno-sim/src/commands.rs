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
    /// Execute one directed source `FUN_004760e0` relationship event. The
    /// raw payload is applied by the `0x2f` handler to `DAT_005b7770`; it is
    /// not the separate `0x30` attitude-matrix event.
    ApplySourceRelationshipEvent { source: u8, target: u8, payload: u8 },
    /// Execute one directed source `FUN_00476130` attitude event. Its raw
    /// payload is applied by the separate `0x30` handler to `DAT_005b77b0`.
    ApplySourceAttitudeEvent { source: u8, target: u8, payload: u8 },
    /// Buy `qty` of `good` at the current market price (deducts gold).
    Buy { player: u8, good: Good, qty: u16 },
    /// Sell `qty` of `good` at the current market price (credits gold).
    Sell { player: u8, good: Good, qty: u16 },
    /// Send the selected tribute amount from `from` to `to`. Anno 1602 manual
    /// section 7.4 describes a diplomacy-panel tribute slider followed by a
    /// hand click; live UI callers must supply that sourced amount instead of
    /// inventing a fixed shortcut value.
    GiftGold { from: u8, to: u8, amount: i32 },
    /// Send a gift of `qty` `good` from `from` to `to` via their
    /// active warehouses. Drains from the sender's first matching
    /// warehouse, deposits into the recipient's first matching one.
    GiftGoods {
        from: u8,
        to: u8,
        good: Good,
        qty: u16,
    },
    /// Arm a naval unit with cannons (manual sec. 9.2.3 "Arming
    /// your ships"). `target_cannons` is the desired count; the
    /// command clamps to the ship class's `cannon_capacity`. Each
    /// cannon costs `Cannons` from the player's nearest active
    /// warehouse + 200 gold (manual: cannons are crafted-good
    /// expenditure plus an installation fee).
    ArmShip {
        player: u8,
        unit_index: u32,
        target_cannons: u8,
    },
    /// Native trade — deliver goods (manual sec. 8.6).
    /// Withdraws `qty` of `good` from any of `player`'s active
    /// warehouses and credits the corresponding native village's
    /// trade balance for that player. Refused if the village
    /// doesn't accept this good.
    NativeDeliver {
        player: u8,
        village_idx: u32,
        good: Good,
        qty: u16,
    },
    /// Native trade — withdraw goods (manual sec. 8.6).
    /// Spends the player's accumulated trade credit at the village
    /// to deposit `qty` of `good` into one of `player`'s active
    /// warehouses. Refused if the village doesn't offer this good
    /// or the credit balance is too low.
    NativeWithdraw {
        player: u8,
        village_idx: u32,
        good: Good,
        qty: u16,
    },
    /// Manually load goods onto a trade ship at a warehouse
    /// (manual sec. 5.3 + 8.3). Withdraws `qty` of `good` from
    /// `warehouse_idx` and adds it to `ship_idx`'s cargo. Refused
    /// if the ship isn't adjacent to the warehouse, or if cargo
    /// is full, or if warehouse stock is short.
    LoadShip {
        player: u8,
        ship_idx: u32,
        warehouse_idx: u32,
        good: Good,
        qty: u16,
    },
    /// Manually unload goods from a trade ship to a warehouse.
    UnloadShip {
        player: u8,
        ship_idx: u32,
        warehouse_idx: u32,
        good: Good,
        qty: u16,
    },
    /// Sell or scuttle a naval unit at the Werft (manual: ships
    /// can be sold or sunk if no longer needed). Refunds half the
    /// unit's gold cost back to the player and removes the unit.
    /// Refused for land units.
    SellShip { player: u8, unit_index: u32 },
    /// Set or clear a unit's patrol waypoint list (manual sec.
    /// 9.2.4). An empty list cancels patrol. Sets `target_x/y` to
    /// the first waypoint immediately so movement starts on the
    /// next tick.
    SetPatrol {
        player: u8,
        unit_index: u32,
        waypoints: Vec<(i32, i32)>,
    },
    /// Propose a trade agreement (manual sec. 7.2). Bilateral once
    /// concluded. Auto-accepted in single-player; auto-rejected if
    /// the players are at war or a recent trade agreement was
    /// broken between them.
    ProposeTradeAgreement { a: u8, b: u8 },
    /// Cancel an existing trade agreement. Sets the per-pair broken
    /// flag so the next proposal is auto-rejected until cleared
    /// (manual: "seldom possible to conclude a new trade agreement
    /// right after one has been broken").
    BreakTradeAgreement { a: u8, b: u8 },
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
        let c = Command::SetTaxRate {
            player: 0,
            tier: 2,
            rate: 96,
        };
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
        let buy = Command::Buy {
            player: 0,
            good: Good::Wood,
            qty: 25,
        };
        let back = Command::decode(&buy.encode()).unwrap();
        if let Command::Buy { qty, .. } = back {
            assert_eq!(qty, 25);
        } else {
            panic!();
        }
    }

    #[test]
    fn round_trip_source_relationship_event_command() {
        let command = Command::ApplySourceRelationshipEvent {
            source: 2,
            target: 4,
            payload: 1,
        };
        let back = Command::decode(&command.encode()).expect("decode source relationship event");
        match back {
            Command::ApplySourceRelationshipEvent {
                source,
                target,
                payload,
            } => {
                assert_eq!(source, 2);
                assert_eq!(target, 4);
                assert_eq!(payload, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_source_attitude_event_command() {
        let command = Command::ApplySourceAttitudeEvent {
            source: 2,
            target: 4,
            payload: 1,
        };
        let back = Command::decode(&command.encode()).expect("decode source attitude event");
        match back {
            Command::ApplySourceAttitudeEvent {
                source,
                target,
                payload,
            } => {
                assert_eq!(source, 2);
                assert_eq!(target, 4);
                assert_eq!(payload, 1);
            }
            _ => panic!("wrong variant"),
        }
    }
}
