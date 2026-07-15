//! Shared source figure-event registry at `DAT_00505e38`.
//!
//! `FUN_00443080` probes this 2,096-entry table for an existing
//! `(x, y, owner)` event. `FUN_00443110` then claims the first empty entry in
//! the same 48-entry probe range. `FUN_0044b140` uses that claim for every
//! kind-12 civilian figure, but does not rewrite the entry's owner byte;
//! `FUN_00443520` releases it by restoring only both coordinates to `-1`.

use crate::source_route::{
    encode_source_direction_route_truncated, encode_source_route_truncated, SourceRouteStep,
    SOURCE_ROUTE_TERMINATOR,
};

/// One source figure-event table entry. The source considers an entry free
/// exactly when both coordinate words are `-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceFigureEventSlot {
    /// Source byte `+0x00`, initialized from compiled `Radius` by the
    /// type-8/type-11 constructors after generic figure allocation.
    pub route_radius: u8,
    pub x: i16,
    pub y: i16,
    /// Source words `+0x08/+0x0a`, the current type-17 terrain-event target.
    /// `FUN_0044bd00` initializes both to `-1`; `FUN_0045c270` writes a
    /// selected terrain cell and `FUN_0045c3f0` restores the sentinels.
    #[serde(default = "source_event_target_sentinel")]
    pub target_x: i16,
    #[serde(default = "source_event_target_sentinel")]
    pub target_y: i16,
    /// Source byte `+0x01`, written as `0xff` by the shared release branch
    /// in `FUN_00443520` and otherwise retained by the kind-12 allocator.
    pub lifecycle: u8,
    pub owner: u8,
    /// Source byte `+0x02`, initialized to zero by `FUN_00443110` and used as
    /// a route-program cursor by kind-specific figure handlers.
    pub route_cursor: u8,
    /// Source byte `+0x14`. `FUN_0044b140` preserves its low nibble and sets
    /// the high nibble to `0xc0` before it encodes a route.
    pub state: u8,
    /// Source word `+0x28`, the 1/32-good quantity accumulated by a
    /// type-11 `FUN_00459400` cart and passed to `FUN_0047d940` on arrival.
    pub transfer_amount_fixed: u16,
    /// Source byte `+0x2a`. Type-17 terrain figures begin with `0x34` and
    /// switch to `0x35` after their accumulated source path cost reaches
    /// `0x40`.
    #[serde(default)]
    pub resource_ware_slot: u8,
    /// Source bytes `+0x14..=+0x1f`, written by `FUN_0046cf70` after a
    /// successful kind-12 grid search. The first byte is also `state` while
    /// `route_cursor` is zero.
    pub route_program: [u8; SourceFigureEventRegistry::KIND12_ROUTE_CAPACITY],
}

const fn source_event_target_sentinel() -> i16 {
    -1
}

impl Default for SourceFigureEventSlot {
    fn default() -> Self {
        Self {
            route_radius: 0,
            x: -1,
            y: -1,
            target_x: -1,
            target_y: -1,
            lifecycle: 0,
            owner: 0,
            route_cursor: 0,
            state: 0,
            transfer_amount_fixed: 0,
            resource_ware_slot: 0,
            route_program: [0; SourceFigureEventRegistry::KIND12_ROUTE_CAPACITY],
        }
    }
}

impl SourceFigureEventSlot {
    pub const fn is_free(self) -> bool {
        self.x == -1 && self.y == -1
    }
}

/// Physical source table shared by map-anchored figure categories.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceFigureEventRegistry {
    slots: Vec<SourceFigureEventSlot>,
}

impl Default for SourceFigureEventRegistry {
    fn default() -> Self {
        Self {
            slots: vec![SourceFigureEventSlot::default(); Self::SLOT_COUNT],
        }
    }
}

impl SourceFigureEventRegistry {
    /// Source table bound `0x830` used by `FUN_00443080` and `FUN_00443110`.
    pub const SLOT_COUNT: usize = 0x830;
    /// The source lookup scans its hash position plus at most `0x30` entries.
    pub const PROBE_LENGTH: usize = 0x30;
    /// `FUN_0044b140` passes this output capacity to `FUN_0046cf70`.
    pub const KIND12_ROUTE_CAPACITY: usize = 0x0c;
    /// `FUN_0044b140` passes this run cap to `FUN_0046cf70`.
    pub const KIND12_ROUTE_RUN_LIMIT: u8 = 0x0f;

    /// `FUN_004430f0`: coordinates are hashed independently of the owner.
    pub const fn source_index(x: i16, y: i16) -> usize {
        ((x as usize & 0x3f) * 0x20) + (y as usize & 0x1f)
    }

    fn probe_range(x: i16, y: i16) -> std::ops::Range<usize> {
        let start = Self::source_index(x, y);
        let end = start
            .saturating_add(Self::PROBE_LENGTH)
            .min(Self::SLOT_COUNT);
        start..end
    }

    /// Return the source slot of an occupied event with this exact coordinate
    /// and owner, matching `FUN_00443080`.
    pub fn lookup(&self, x: i16, y: i16, owner: u8) -> Option<u16> {
        Self::probe_range(x, y).find_map(|slot| {
            let entry = self.slots[slot];
            (entry.x == x && entry.y == y && entry.owner == owner).then_some(slot as u16)
        })
    }

    /// Prepare a kind-12 source event candidate after proving no matching
    /// coordinate/owner record exists. This is `FUN_00443080` followed by
    /// `FUN_00443110`; the entry remains coordinate-free until the generic
    /// figure allocator has succeeded.
    ///
    /// The kind-12 allocator writes x/y after it claims the free slot, but
    /// leaves byte `+0x2b` untouched. Its lookup key can therefore differ
    /// from the entry's retained owner byte after a prior category used this
    /// slot.
    pub fn prepare_kind12_if_absent(&mut self, x: i16, y: i16, owner: u8) -> Option<u16> {
        if self.lookup(x, y, owner).is_some() {
            return None;
        }
        let slot = Self::probe_range(x, y).find(|&slot| self.slots[slot].is_free())?;
        let entry = &mut self.slots[slot];
        entry.route_cursor = 0;
        entry.state = 0xc0;
        entry.route_program[0] = 0xc0;
        Some(slot as u16)
    }

    /// Prepare the shared event entry used by a type-8 transfer figure.
    /// `FUN_0044ab60` uses the same lookup and free-entry constructor as type
    /// 12 before its generic figure record has been allocated. Type 11 uses
    /// the same free-entry constructor only after its separate multiplicity
    /// count has admitted another cart.
    pub fn prepare_transfer_if_absent(&mut self, x: i16, y: i16, owner: u8) -> Option<u16> {
        self.prepare_kind12_if_absent(x, y, owner)
    }

    /// `FUN_0044af10` counts every occupied event with the transfer root's
    /// exact coordinate and map owner before `FUN_0044ad50` claims a free
    /// slot. The compiled `Figuranz` value supplies this bound.
    pub fn prepare_transfer_with_limit(
        &mut self,
        x: i16,
        y: i16,
        owner: u8,
        limit: u8,
    ) -> Option<u16> {
        let matching_entries = Self::probe_range(x, y)
            .filter(|&slot| {
                let entry = self.slots[slot];
                entry.x == x && entry.y == y && entry.owner == owner
            })
            .count();
        if matching_entries >= usize::from(limit) {
            return None;
        }
        let slot = Self::probe_range(x, y).find(|&slot| self.slots[slot].is_free())?;
        let entry = &mut self.slots[slot];
        entry.route_cursor = 0;
        entry.state = 0xc0;
        entry.route_program[0] = 0xc0;
        Some(slot as u16)
    }

    /// Write the prepared candidate's coordinates after `FUN_00446ca0` has
    /// returned a live kind-12 figure. A prepared candidate still has both
    /// free sentinels and may therefore be activated only once.
    pub fn activate_kind12(&mut self, slot: u16, x: i16, y: i16) -> bool {
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if !entry.is_free() {
            return false;
        }
        entry.x = x;
        entry.y = y;
        true
    }

    /// Publish a type-8 or type-11 transfer event after its generic figure
    /// allocator has succeeded. Those constructors set source byte `+0x01`
    /// to one, rewrite the map-owner byte, and store the compiled route
    /// radius, unlike kind 12.
    pub fn activate_transfer(
        &mut self,
        slot: u16,
        x: i16,
        y: i16,
        owner: u8,
        route_radius: u8,
    ) -> bool {
        if !self.activate_kind12(slot, x, y) {
            return false;
        }
        let entry = &mut self.slots[usize::from(slot)];
        entry.route_radius = route_radius;
        entry.lifecycle = 1;
        entry.owner = owner;
        entry.transfer_amount_fixed = 0;
        true
    }

    /// Claim and publish the `FUN_0044bd00` entry for a generic type-17
    /// terrain figure. Its lookup owner is the fixed source selector seven,
    /// distinct from the category-six byte written at source offset `+0x00`.
    pub fn prepare_terrain_event_if_absent(&mut self, x: i16, y: i16) -> Option<u16> {
        self.prepare_kind12_if_absent(x, y, 7)
    }

    /// Complete type-17 terrain-event construction after generic-figure pool
    /// allocation succeeds. This mirrors the writes in `FUN_0044bd00`.
    pub fn activate_terrain_event(&mut self, slot: u16, x: i16, y: i16) -> bool {
        if !self.activate_kind12(slot, x, y) {
            return false;
        }
        let entry = &mut self.slots[usize::from(slot)];
        entry.route_radius = 6;
        entry.lifecycle = 0;
        entry.owner = 7;
        entry.target_x = -1;
        entry.target_y = -1;
        entry.transfer_amount_fixed = 0;
        entry.resource_ware_slot = 0x34;
        true
    }

    /// Claim a kind-12 source event slot and activate its coordinates. This
    /// convenience operation is appropriate when generic figure-pool
    /// allocation is already known to succeed.
    pub fn allocate_kind12_if_absent(&mut self, x: i16, y: i16, owner: u8) -> Option<u16> {
        let slot = self.prepare_kind12_if_absent(x, y, owner)?;
        self.activate_kind12(slot, x, y).then_some(slot)
    }

    /// Write the bounded route program used by `FUN_0044b140` after its
    /// callback search succeeds. A valid source route always has an event
    /// slot first; a stale slot is rejected here rather than allocated anew.
    pub fn write_kind12_route(&mut self, slot: u16, steps: &[SourceRouteStep]) -> bool {
        let Ok(program) = encode_source_route_truncated(
            steps,
            Self::KIND12_ROUTE_RUN_LIMIT,
            Self::KIND12_ROUTE_CAPACITY,
        ) else {
            return false;
        };
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        entry.route_cursor = 0;
        entry.route_program[..program.len()].copy_from_slice(&program);
        entry.state = entry.route_program[0];
        true
    }

    /// Write the plantation-worker route generated by `FUN_0045b200`.
    /// Its `FUN_00472b60` encoder coalesces equal directions without
    /// considering the preceding cell's metadata class.
    pub fn write_plantation_route(&mut self, slot: u16, steps: &[SourceRouteStep]) -> bool {
        let Ok(program) = encode_source_direction_route_truncated(
            steps,
            Self::KIND12_ROUTE_RUN_LIMIT,
            Self::KIND12_ROUTE_CAPACITY,
        ) else {
            return false;
        };
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        entry.route_cursor = 0;
        entry.route_program[..program.len()].copy_from_slice(&program);
        entry.state = entry.route_program[0];
        true
    }

    /// `FUN_0045c270` encodes a type-17 terrain route with the same
    /// `FUN_00472b60` direction-only program as a plantation worker.
    pub fn write_terrain_route(&mut self, slot: u16, steps: &[SourceRouteStep]) -> bool {
        self.write_plantation_route(slot, steps)
    }

    /// Set the terrain target selected by `FUN_00471c50` and retain its
    /// source path cost when the active resource selector is `0x34`.
    pub fn set_terrain_target(&mut self, slot: u16, target: (i16, i16), path_cost: u16) -> bool {
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        entry.target_x = target.0;
        entry.target_y = target.1;
        if entry.resource_ware_slot == 0x34 {
            entry.transfer_amount_fixed = entry.transfer_amount_fixed.saturating_add(path_cost);
        }
        true
    }

    /// `FUN_0045c3f0` drops the active terrain target and restores the route
    /// terminator while retaining the event's resource selector and cost.
    pub fn clear_terrain_target(&mut self, slot: u16) -> bool {
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        entry.target_x = -1;
        entry.target_y = -1;
        entry.route_cursor = 0;
        entry.state = 0xc0;
        entry.route_program[0] = 0xc0;
        true
    }

    /// Apply the type-17 harvest postcondition from `FUN_0045bfc0`.
    pub fn finish_terrain_harvest(&mut self, slot: u16) -> bool {
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        entry.lifecycle = 0;
        if entry.transfer_amount_fixed < 0x40 {
            entry.resource_ware_slot = 0x34;
        } else {
            entry.transfer_amount_fixed = 0;
            entry.resource_ware_slot = 0x35;
        }
        true
    }

    /// Write the type-8/type-11 route program used by `FUN_0045c8b0` and
    /// consumed by the type-11 `FUN_00459400` dispatcher. The program uses
    /// the same bounded `FUN_0046cf70` representation as kind 12.
    pub fn write_transfer_route(&mut self, slot: u16, steps: &[SourceRouteStep]) -> bool {
        self.write_kind12_route(slot, steps)
    }

    /// Synchronize source word `+0x28` after a type-11 cart has collected
    /// from its supplier. The source keeps this quantity in the shared event
    /// record, rather than in the generic figure, until terminal delivery.
    pub fn set_transfer_amount_fixed(&mut self, slot: u16, amount: u16) -> bool {
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        entry.transfer_amount_fixed = amount;
        true
    }

    /// Number of cell steps preserved by the event slot's bounded program.
    /// This follows the source decoder's `0xc?` terminator rule.
    pub fn kind12_route_step_count(&self, slot: u16) -> Option<usize> {
        let entry = self.slots.get(usize::from(slot))?;
        let mut steps = 0usize;
        for &instruction in &entry.route_program {
            if instruction & 0xf0 == 0xc0 {
                return Some(steps);
            }
            let direction = instruction >> 4;
            let length = instruction & 0x0f;
            if !(1..=8).contains(&direction) || length == 0 {
                return None;
            }
            steps = steps.saturating_add(usize::from(length));
        }
        None
    }

    /// Whether `FUN_00459f40` will take its terminal deletion branch before
    /// attempting another movement segment.
    pub fn kind12_is_terminal(&self, slot: u16) -> Option<bool> {
        self.slots
            .get(usize::from(slot))
            .map(|entry| entry.state & 0xf0 == 0xc0)
    }

    /// Synchronize the source route-byte cursor after the expanded movement
    /// representation has completed `completed_steps` cells. The source
    /// advances this cursor only after a whole encoded run finishes.
    pub fn set_kind12_route_progress(&mut self, slot: u16, completed_steps: usize) -> bool {
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        let mut cursor = 0usize;
        let mut consumed = 0usize;
        while cursor < entry.route_program.len() {
            let instruction = entry.route_program[cursor];
            if instruction & 0xf0 == 0xc0 {
                break;
            }
            let direction = instruction >> 4;
            let length = instruction & 0x0f;
            if !(1..=8).contains(&direction) || length == 0 {
                return false;
            }
            let run_end = consumed.saturating_add(usize::from(length));
            if completed_steps < run_end {
                break;
            }
            consumed = run_end;
            cursor += 1;
        }
        let Some(&state) = entry.route_program.get(cursor) else {
            return false;
        };
        entry.route_cursor = cursor as u8;
        entry.state = state;
        true
    }

    /// Synchronize a type-8/type-11 event program after its linked figure
    /// completes expanded source-grid steps.
    pub fn set_transfer_route_progress(&mut self, slot: u16, completed_steps: usize) -> bool {
        self.set_kind12_route_progress(slot, completed_steps)
    }

    /// Update source byte `+0x01` for handlers whose figure lifecycle has a
    /// distinct intermediate state. `FUN_0045afd0` advances a plantation
    /// worker from lifecycle one to two after it reaches its selected raw
    /// resource, then consumes the resource on the next handler pass.
    pub fn set_lifecycle(&mut self, slot: u16, lifecycle: u8) -> bool {
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        entry.lifecycle = lifecycle;
        true
    }

    /// Release the coordinate pair exactly as the kind-12 branch in
    /// `FUN_00443520`, preserving the remaining event bytes except source
    /// lifecycle byte `+0x01`, which becomes `0xff`. Invalid indices are not
    /// source slots and are ignored.
    pub fn release(&mut self, slot: u16) -> bool {
        let Some(entry) = self.slots.get_mut(usize::from(slot)) else {
            return false;
        };
        if entry.is_free() {
            return false;
        }
        entry.x = -1;
        entry.y = -1;
        entry.lifecycle = 0xff;
        true
    }

    pub fn slot(&self, slot: u16) -> Option<SourceFigureEventSlot> {
        self.slots.get(usize::from(slot)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_probes_the_source_coordinate_window() {
        let mut registry = SourceFigureEventRegistry::default();
        assert_eq!(SourceFigureEventRegistry::source_index(65, 34), 0x22);

        let first = registry.allocate_kind12_if_absent(65, 34, 2).unwrap();
        assert_eq!(first, 0x22);
        assert_eq!(registry.lookup(65, 34, 0), Some(first));
        assert_eq!(registry.lookup(65, 34, 2), None);
        assert_eq!(registry.slot(first).unwrap().state, 0xc0);
        assert_eq!(registry.slot(first).unwrap().route_program[0], 0xc0);

        let other_owner = registry.allocate_kind12_if_absent(65, 34, 2).unwrap();
        assert_eq!(other_owner, first + 1);

        let matching_owner = registry.allocate_kind12_if_absent(66, 34, 0).unwrap();
        assert!(registry.allocate_kind12_if_absent(66, 34, 0).is_none());
        assert_eq!(registry.lookup(66, 34, 0), Some(matching_owner));
    }

    #[test]
    fn release_restores_the_source_free_coordinate_sentinel() {
        let mut registry = SourceFigureEventRegistry::default();
        let slot = registry.allocate_kind12_if_absent(7, 9, 0).unwrap();
        assert!(registry.release(slot));
        let released = registry.slot(slot).unwrap();
        assert!(released.is_free());
        assert_eq!(released.lifecycle, 0xff);
        assert_eq!(released.owner, 0);
        assert_eq!(released.state, 0xc0);
        assert!(!registry.release(slot));
        assert_eq!(registry.allocate_kind12_if_absent(7, 9, 0), Some(slot));
    }

    #[test]
    fn transfer_activation_sets_its_lifecycle_and_map_owner() {
        let mut registry = SourceFigureEventRegistry::default();
        let slot = registry.prepare_transfer_if_absent(7, 9, 3).unwrap();
        assert!(registry.slot(slot).unwrap().is_free());
        assert!(registry.activate_transfer(slot, 7, 9, 3, 16));
        assert_eq!(
            registry.slot(slot),
            Some(SourceFigureEventSlot {
                route_radius: 16,
                x: 7,
                y: 9,
                target_x: -1,
                target_y: -1,
                lifecycle: 1,
                owner: 3,
                route_cursor: 0,
                state: 0xc0,
                transfer_amount_fixed: 0,
                resource_ware_slot: 0,
                route_program: [0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            })
        );
        assert!(registry.set_lifecycle(slot, 2));
        assert_eq!(registry.slot(slot).unwrap().lifecycle, 2);
        assert!(registry.set_transfer_amount_fixed(slot, 65));
        assert_eq!(registry.slot(slot).unwrap().transfer_amount_fixed, 65);
    }

    #[test]
    fn terrain_event_tracks_native_target_and_resource_lifecycle() {
        let mut registry = SourceFigureEventRegistry::default();
        let slot = registry.prepare_terrain_event_if_absent(7, 9).unwrap();
        assert!(registry.activate_terrain_event(slot, 7, 9));
        assert_eq!(registry.lookup(7, 9, 7), Some(slot));
        assert_eq!(
            registry.slot(slot),
            Some(SourceFigureEventSlot {
                route_radius: 6,
                x: 7,
                y: 9,
                target_x: -1,
                target_y: -1,
                lifecycle: 0,
                owner: 7,
                route_cursor: 0,
                state: 0xc0,
                transfer_amount_fixed: 0,
                resource_ware_slot: 0x34,
                route_program: [0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            })
        );
        assert!(registry.set_terrain_target(slot, (11, 13), 0x40));
        assert!(registry.write_terrain_route(
            slot,
            &[SourceRouteStep {
                direction: 3,
                metadata: 0x20,
            }]
        ));
        assert!(registry.set_lifecycle(slot, 1));
        assert!(registry.finish_terrain_harvest(slot));
        assert_eq!(registry.slot(slot).unwrap().resource_ware_slot, 0x35);
        assert_eq!(registry.slot(slot).unwrap().transfer_amount_fixed, 0);
        assert!(registry.clear_terrain_target(slot));
        let entry = registry.slot(slot).unwrap();
        assert_eq!(
            (entry.target_x, entry.target_y, entry.state),
            (-1, -1, 0xc0)
        );
    }

    #[test]
    fn transfer_release_preserves_noncoordinate_event_state() {
        let mut registry = SourceFigureEventRegistry::default();
        let slot = registry.prepare_transfer_if_absent(7, 9, 3).unwrap();
        assert!(registry.activate_transfer(slot, 7, 9, 3, 16));
        assert!(registry.write_transfer_route(
            slot,
            &[SourceRouteStep {
                direction: 3,
                metadata: 0,
            }],
        ));
        assert!(registry.set_transfer_amount_fixed(slot, 129));

        assert!(registry.release(slot));
        assert_eq!(
            registry.slot(slot),
            Some(SourceFigureEventSlot {
                route_radius: 16,
                x: -1,
                y: -1,
                target_x: -1,
                target_y: -1,
                lifecycle: 0xff,
                owner: 3,
                route_cursor: 0,
                state: 0x31,
                transfer_amount_fixed: 129,
                resource_ware_slot: 0,
                route_program: [0x31, SOURCE_ROUTE_TERMINATOR, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,],
            })
        );
    }

    #[test]
    fn type11_transfer_admission_counts_matching_live_entries_up_to_figuranz() {
        let mut registry = SourceFigureEventRegistry::default();
        let first = registry.prepare_transfer_with_limit(7, 9, 3, 2).unwrap();
        assert!(registry.activate_transfer(first, 7, 9, 3, 2));
        let second = registry.prepare_transfer_with_limit(7, 9, 3, 2).unwrap();
        assert!(registry.activate_transfer(second, 7, 9, 3, 2));

        assert_eq!(registry.prepare_transfer_with_limit(7, 9, 3, 2), None);
        assert!(registry.prepare_transfer_with_limit(7, 9, 4, 2).is_some());
        assert!(registry.release(first));
        assert!(registry.prepare_transfer_with_limit(7, 9, 3, 2).is_some());
    }

    #[test]
    fn kind12_route_uses_the_source_bounded_program_layout() {
        let mut registry = SourceFigureEventRegistry::default();
        let slot = registry.allocate_kind12_if_absent(7, 9, 0).unwrap();
        let steps = [
            SourceRouteStep {
                direction: 3,
                metadata: 4,
            },
            SourceRouteStep {
                direction: 3,
                metadata: 4,
            },
            SourceRouteStep {
                direction: 5,
                metadata: 7,
            },
        ];

        assert!(registry.write_kind12_route(slot, &steps));
        let entry = registry.slot(slot).unwrap();
        assert_eq!(entry.route_cursor, 0);
        assert_eq!(entry.state, 0x32);
        assert_eq!(
            entry.route_program[..3],
            [0x32, 0x51, SOURCE_ROUTE_TERMINATOR]
        );
        assert_eq!(registry.kind12_route_step_count(slot), Some(3));
        assert!(registry.set_kind12_route_progress(slot, 1));
        assert_eq!(registry.slot(slot).unwrap().route_cursor, 0);
        assert!(registry.set_kind12_route_progress(slot, 2));
        assert_eq!(registry.slot(slot).unwrap().state, 0x51);
        assert!(registry.set_kind12_route_progress(slot, 3));
        assert!(registry.kind12_is_terminal(slot).unwrap());
    }
}
