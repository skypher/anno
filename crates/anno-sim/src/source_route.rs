//! Ship route bytecode used by the original figure scheduler.
//!
//! `FUN_00455a20` writes a route at live ship-record offset `0x124`.
//! `FUN_0046cf70` encodes each run as `(direction << 4) | length` and
//! terminates the program with `0xc1`; `FUN_00456270` decodes the same
//! directions. The kind-1/3 ship handler calls the encoder with a maximum
//! run length of two.

use anno_formats::cod::BuildingDef as CodBuilding;
use anno_formats::szs::{Island, IslandTile};
use std::collections::HashMap;

/// The normal caller radius stored by `FUN_00452370` and
/// `FUN_004568b0` before they invoke the source ship-route builders.
pub const SOURCE_SHIP_NORMAL_ROUTE_RADIUS: usize = 0x50;

/// The short target-proximity retry radius stored by `FUN_00452370` before
/// it invokes `FUN_00455a20`.
pub const SOURCE_SHIP_SHORT_RETRY_ROUTE_RADIUS: usize = 0x28;

/// Persisted source route-window selection for a ship route request.
///
/// The executable stores the selected radius in the caller-owned route
/// record, then passes it unchanged as `param_4` to `FUN_00455a20` or
/// `FUN_00456920`. The `ShortTargetRetry` transition is made by the timed
/// target-proximity branch in `FUN_00452370`; a failed path search alone does
/// not select it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceShipRouteWindow {
    Normal,
    ShortTargetRetry,
}

impl Default for SourceShipRouteWindow {
    fn default() -> Self {
        Self::Normal
    }
}

impl SourceShipRouteWindow {
    /// Radius passed to the local source path-grid constructor.
    pub const fn radius(self) -> usize {
        match self {
            Self::Normal => SOURCE_SHIP_NORMAL_ROUTE_RADIUS,
            Self::ShortTargetRetry => SOURCE_SHIP_SHORT_RETRY_ROUTE_RADIUS,
        }
    }
}

/// Source route terminator written by `FUN_0046cf70`.
pub const SOURCE_ROUTE_TERMINATOR: u8 = 0xc1;

/// Maximum number of route bytes the kind-1/3 ship handler requests from
/// `FUN_0046cf70` before its terminator.
pub const SOURCE_SHIP_ROUTE_CAPACITY: usize = 100;

/// Maximum repeated direction encoded per byte by the kind-1/3 handler.
pub const SOURCE_SHIP_ROUTE_RUN_LIMIT: u8 = 2;

/// Source cardinal-step cost for a low-seven-bit path-grid metadata class.
/// `FUN_0046f8a0` initializes `DAT_005db8a0` with these values.
pub fn source_cardinal_cost(metadata: u8) -> u32 {
    match metadata & 0x7f {
        0..=31 => 0x40,
        32..=126 => u32::from(metadata & 0x7f) * 2,
        127 => 0x1f8,
        _ => unreachable!("metadata is masked to seven bits"),
    }
}

/// Source diagonal-step cost for a low-seven-bit path-grid metadata class.
/// `FUN_0046f8a0` initializes `DAT_005bb280` with these values.
pub fn source_diagonal_cost(metadata: u8) -> u32 {
    match metadata & 0x7f {
        0..=31 => 0x5b,
        32..=126 => (0xb60 + 0x5b * (u32::from(metadata & 0x7f) - 32)) / 32,
        127 => 0x2cc,
        _ => unreachable!("metadata is masked to seven bits"),
    }
}

/// A source direction together with the map-cost class that controls whether
/// adjacent steps are allowed to share one route byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceRouteStep {
    /// `FUN_0046cf70` direction, in `1..=8`.
    pub direction: u8,
    /// The low seven bits of the path-grid metadata. The source compressor
    /// only merges consecutive steps with equal metadata.
    pub metadata: u8,
}

/// Decode one source route direction into its world-coordinate delta.
pub fn source_direction_delta(direction: u8) -> Option<(i32, i32)> {
    match direction {
        1 => Some((0, -1)),
        2 => Some((1, -1)),
        3 => Some((1, 0)),
        4 => Some((1, 1)),
        5 => Some((0, 1)),
        6 => Some((-1, 1)),
        7 => Some((-1, 0)),
        8 => Some((-1, -1)),
        _ => None,
    }
}

/// Return the zero-based source compass code emitted by `FUN_00454050` for a
/// raw target delta. This is distinct from the one-based route-step encoding
/// accepted by [`source_direction_delta`]: `0..=7` means
/// north, north-east, east, south-east, south, south-west, west, north-west.
///
/// Category-6 dispatch calls this after clamping its doubled-coordinate target
/// footprint in `FUN_00458d80`; its exact tie rules are therefore preserved
/// here rather than inferred from the route-step encoder.
pub fn source_target_direction(dx: i32, dy: i32) -> u8 {
    let dx = i64::from(dx);
    let dy = i64::from(dy);
    if dy < 0 {
        let north = -dy;
        if dx < 0 {
            let west = -dx;
            if north < west {
                if -2 * dy < west {
                    return 6;
                }
            } else if -2 * dx < north {
                return 0;
            }
            return 7;
        }
        if north < dx {
            // The source branch is `dx < 2 * north -> diagonal`, so the exact
            // 2:1 boundary falls through to the axial return.
            if -2 * dy <= dx {
                return 2;
            }
        } else if 2 * dx < north {
            return 0;
        }
        return 1;
    }

    if dx >= 0 {
        if dy < dx {
            if 2 * dy < dx {
                return 2;
            }
        } else if 2 * dx < dy {
            return 4;
        }
        return 3;
    }

    let west = -dx;
    if dy < west {
        if 2 * dy < west {
            return 6;
        }
    } else if -2 * dx < dy {
        return 4;
    }
    5
}

/// Expand predecessor-route steps into the successive source-grid positions.
/// Returns `None` when a step contains an invalid source direction or would
/// overflow the coordinate representation.
pub fn source_route_positions(
    start: (i32, i32),
    steps: &[SourceRouteStep],
) -> Option<Vec<(i32, i32)>> {
    let mut position = start;
    let mut positions = Vec::with_capacity(steps.len());
    for step in steps {
        let (dx, dy) = source_direction_delta(step.direction)?;
        position = (position.0.checked_add(dx)?, position.1.checked_add(dy)?);
        positions.push(position);
    }
    Some(positions)
}

/// Decode a `FUN_0046cf70` bytecode program into its individual directions.
///
/// `FUN_00456270` stops on any `0xc?` byte. `FUN_0046cf70` itself writes
/// `0xc1`, exposed above as [`SOURCE_ROUTE_TERMINATOR`].
pub fn decode_source_route(program: &[u8]) -> Result<Vec<u8>, SourceRouteError> {
    let mut steps = Vec::new();
    let mut terminated = false;

    for &instruction in program {
        if instruction & 0xf0 == 0xc0 {
            terminated = true;
            break;
        }

        let direction = instruction >> 4;
        let length = instruction & 0x0f;
        if source_direction_delta(direction).is_none() || length == 0 {
            return Err(SourceRouteError::InvalidInstruction(instruction));
        }
        steps.extend(std::iter::repeat_n(direction, length as usize));
    }

    if terminated {
        Ok(steps)
    } else {
        Err(SourceRouteError::MissingTerminator)
    }
}

/// Encode source-route steps using the run grouping of `FUN_0046cf70`.
///
/// The caller supplies the source path-grid metadata because source code only
/// groups adjacent steps when their low-seven-bit metadata is equal.
pub fn encode_source_route(
    steps: &[SourceRouteStep],
    max_run_length: u8,
    capacity: usize,
) -> Result<Vec<u8>, SourceRouteError> {
    if max_run_length == 0 || max_run_length > 0x0f {
        return Err(SourceRouteError::InvalidRunLimit(max_run_length));
    }
    if capacity == 0 {
        return Err(SourceRouteError::CapacityExceeded);
    }

    let mut program = Vec::new();
    let mut index = 0;
    while index < steps.len() {
        let step = steps[index];
        if source_direction_delta(step.direction).is_none() {
            return Err(SourceRouteError::InvalidDirection(step.direction));
        }

        let mut length = 1usize;
        while length < max_run_length as usize
            && index + length < steps.len()
            && steps[index + length].direction == step.direction
            && (steps[index + length].metadata & 0x7f) == (step.metadata & 0x7f)
        {
            length += 1;
        }

        if program.len() + 1 >= capacity {
            return Err(SourceRouteError::CapacityExceeded);
        }
        program.push((step.direction << 4) | length as u8);
        index += length;
    }

    program.push(SOURCE_ROUTE_TERMINATOR);
    Ok(program)
}

/// Encode a route with the fixed output-buffer behavior of `FUN_0046cf70`.
///
/// The source writes as many complete runs as fit before its reserved
/// terminator byte, then writes `0xc1`; it does not reject a long route.
/// Callers with a source-owned bounded route field use this form.
pub fn encode_source_route_truncated(
    steps: &[SourceRouteStep],
    max_run_length: u8,
    capacity: usize,
) -> Result<Vec<u8>, SourceRouteError> {
    if max_run_length == 0 || max_run_length > 0x0f {
        return Err(SourceRouteError::InvalidRunLimit(max_run_length));
    }
    if capacity == 0 {
        return Err(SourceRouteError::CapacityExceeded);
    }

    let mut program = Vec::new();
    let mut index = 0;
    while index < steps.len() {
        let step = steps[index];
        if source_direction_delta(step.direction).is_none() {
            return Err(SourceRouteError::InvalidDirection(step.direction));
        }

        let mut length = 1usize;
        while length < max_run_length as usize
            && index + length < steps.len()
            && steps[index + length].direction == step.direction
            && (steps[index + length].metadata & 0x7f) == (step.metadata & 0x7f)
        {
            length += 1;
        }
        if program.len() + 1 >= capacity {
            break;
        }
        program.push((step.direction << 4) | length as u8);
        index += length;
    }
    program.push(SOURCE_ROUTE_TERMINATOR);
    Ok(program)
}

/// Encode a bounded plantation-worker route with `FUN_00472b60`'s grouping
/// rule. That routine merges adjacent identical directions regardless of the
/// path-grid metadata class, unlike `FUN_0046cf70`.
pub fn encode_source_direction_route_truncated(
    steps: &[SourceRouteStep],
    max_run_length: u8,
    capacity: usize,
) -> Result<Vec<u8>, SourceRouteError> {
    if max_run_length == 0 || max_run_length > 0x0f {
        return Err(SourceRouteError::InvalidRunLimit(max_run_length));
    }
    if capacity == 0 {
        return Err(SourceRouteError::CapacityExceeded);
    }

    let mut program = Vec::new();
    let mut index = 0;
    while index < steps.len() {
        let step = steps[index];
        if source_direction_delta(step.direction).is_none() {
            return Err(SourceRouteError::InvalidDirection(step.direction));
        }

        let mut length = 1usize;
        while length < max_run_length as usize
            && index + length < steps.len()
            && steps[index + length].direction == step.direction
        {
            length += 1;
        }
        if program.len() + 1 >= capacity {
            break;
        }
        program.push((step.direction << 4) | length as u8);
        index += length;
    }
    program.push(SOURCE_ROUTE_TERMINATOR);
    Ok(program)
}

/// Route-program validation or capacity failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRouteError {
    InvalidDirection(u8),
    InvalidInstruction(u8),
    InvalidRunLimit(u8),
    MissingTerminator,
    CapacityExceeded,
}

/// A two-byte-per-cell path grid matching the storage used by
/// `FUN_0046c7d0`: predecessor direction followed by path metadata.
#[derive(Debug, Clone)]
pub struct SourcePathGrid {
    origin: (i32, i32),
    width: usize,
    height: usize,
    cells: Vec<SourcePathCell>,
}

#[derive(Debug, Clone, Copy)]
struct SourcePathCell {
    direction: u8,
    metadata: u8,
}

/// Search failure from the fixed-cost wave expansion in `FUN_0046c7d0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePathSearchError {
    OutOfBounds,
    NoRoute,
}

/// A blocked-cell callback decision from `FUN_0046c7d0`.
///
/// The executable calls its callback only when a due frontier cell has the
/// high metadata bit set. `Block` leaves that cell unexpanded, `Expand`
/// continues through it, and `AdvanceFrontier` discards the remaining current
/// LIFO frontier while retaining cells already queued for the next one.
/// `Complete` represents a callback that terminates the grid search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePathBlockedCellDecision {
    Block,
    Expand,
    AdvanceFrontier,
    Complete,
}

/// A source-grid search completion, including the exact predecessor route
/// produced before the callback stopped `FUN_0046c7d0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePathSearchResult {
    /// World-space cell at which the blocked-cell callback completed.
    pub position: (i32, i32),
    /// `local_2c` passed to the callback by `FUN_0046c7d0`.
    pub elapsed_cost: u32,
    /// Predecessor directions from the supplied start cell to `position`.
    pub steps: Vec<SourceRouteStep>,
}

/// A world-space target rectangle prepared by `FUN_004451a0` before a ship
/// route callback runs. Its dimensions are positive source footprint sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePathTargetRect {
    pub origin: (i32, i32),
    pub width: usize,
    pub height: usize,
}

/// The four-byte live target descriptor passed to source entity route and
/// movement builders. `FUN_00445400` constructs kind `0x37` for a world-map
/// cell; `FUN_00444900` and `FUN_00444af0` decode its packed coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SourceTargetDescriptor {
    bytes: [u8; 4],
}

/// The two route preparations selected by `FUN_00455a20` after it resolves
/// the target descriptor's owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceShipTargetRouteBranch {
    /// `FUN_0046dde0` and `LAB_0046c670`.
    Direct { approach_radius: i32 },
    /// `FUN_0046e350` and `LAB_0046c750`.
    Threshold { approach_radius: i32, limit: u32 },
}

/// A static island-map target resolved from a kind-`0x32` or kind-`0x33`
/// descriptor. `FUN_004451a0` obtains the footprint from the target cell's
/// oriented definition; `FUN_00444100` obtains the source owner from that
/// same cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceResolvedStaticTarget {
    pub target: SourcePathTargetRect,
    pub owner: u8,
}

/// One active entry in an island's eight-slot map-object table. The source
/// stores these records at `island + 0xac + slot * 4`; kinds `0x35` and
/// `0x36` name an entry by its island and slot bytes.
///
/// The table is live state, not an `INSELHAUS` field. Callers must therefore
/// supply entries extracted from the corresponding source map-object state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceDynamicMapObject {
    pub island: u8,
    pub slot: u8,
    pub owner: u8,
    pub local_position: (u8, u8),
}

/// The eight-entry dynamic map-object table attached to one source island.
/// `FUN_00468ce0` selects the first zero entry; `FUN_00468ed0` clears an
/// entry when its object is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDynamicMapObjectTable {
    island: u8,
    slots: [Option<SourceDynamicMapObject>; Self::SLOT_COUNT],
}

impl SourceDynamicMapObjectTable {
    /// Number of source map-object pointers at `island + 0xac`.
    pub const SLOT_COUNT: usize = 8;

    /// Create the initially empty table for one source island record.
    pub const fn new(island: u8) -> Self {
        Self {
            island,
            slots: [None; Self::SLOT_COUNT],
        }
    }

    /// Allocate the first free source slot and retain its live object record.
    pub fn allocate(
        &mut self,
        owner: u8,
        local_position: (u8, u8),
    ) -> Option<SourceDynamicMapObject> {
        let slot = self.slots.iter().position(Option::is_none)? as u8;
        let object = SourceDynamicMapObject {
            island: self.island,
            slot,
            owner,
            local_position,
        };
        self.slots[slot as usize] = Some(object);
        Some(object)
    }

    /// Read one source slot without changing its allocation state.
    pub fn object(&self, slot: u8) -> Option<SourceDynamicMapObject> {
        self.slots.get(slot as usize).copied().flatten()
    }

    /// Restore one occupied source slot from persistent live-object state.
    /// The record must belong to this island and may not overwrite an entry.
    pub fn insert(&mut self, object: SourceDynamicMapObject) -> bool {
        if object.island != self.island {
            return false;
        }
        let Some(slot) = self.slots.get_mut(object.slot as usize) else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(object);
        true
    }

    /// Clear a source slot and return the live record that occupied it.
    pub fn release(&mut self, slot: u8) -> Option<SourceDynamicMapObject> {
        self.slots.get_mut(slot as usize)?.take()
    }

    /// Iterate live objects in the source table's ascending slot order.
    pub fn objects(&self) -> impl Iterator<Item = SourceDynamicMapObject> + '_ {
        self.slots.iter().flatten().copied()
    }
}

/// A kind-`0x35` or kind-`0x36` target resolved through a supplied live
/// map-object entry. `0x35` expands to the static cell's oriented footprint;
/// `0x36` uses the same origin and the source default `1 x 1` footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceResolvedDynamicTarget {
    pub target: SourcePathTargetRect,
    pub owner: u8,
}

impl SourceTargetDescriptor {
    /// The descriptor kind written by `FUN_00445400`.
    pub const WORLD_COORDINATE_KIND: u8 = 0x37;
    /// The packed coordinate kind written by `FUN_004453d0`.
    pub const FIXED_POINT_COORDINATE_KIND: u8 = 0x38;

    /// Construct the exact kind-`0x37` descriptor for a source world cell.
    /// Each coordinate occupies twelve bits in the live descriptor.
    pub fn from_world_coordinate(x: i32, y: i32) -> Option<Self> {
        let x = u16::try_from(x).ok()?;
        let y = u16::try_from(y).ok()?;
        (x <= 0x0fff && y <= 0x0fff).then_some(Self {
            bytes: [
                Self::WORLD_COORDINATE_KIND,
                ((y >> 8) as u8) << 4 | ((x >> 8) as u8 & 0x0f),
                x as u8,
                y as u8,
            ],
        })
    }

    /// Construct a kind-`0x37` descriptor for a type-4 figure's raw route
    /// coordinate. `FUN_00444af0` doubles kind `0x37` coordinates before
    /// handing them to the land route builder, so type-4 callers must pack
    /// the underlying world cell rather than the raw doubled waypoint.
    pub fn from_source_land_route_coordinate(x: i32, y: i32) -> Option<Self> {
        (x % 2 == 0 && y % 2 == 0).then(|| Self::from_world_coordinate(x / 2, y / 2))?
    }

    /// Preserve a live descriptor read from a source route record. Resolution
    /// of object-backed kinds is deliberately separate because it needs the
    /// corresponding source object tables.
    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self { bytes }
    }

    /// Construct the kind-`0x34` static island-cell descriptor written by
    /// `FUN_004458f0`. The type-4 native idle branch stores the selected
    /// local map cell in this form rather than as a packed coordinate.
    pub const fn from_source_kind34_island_cell(island: u8, x: u8, y: u8) -> Self {
        Self::from_bytes([0x34, island, x, y])
    }

    /// Return the raw four-byte live descriptor without changing its
    /// representation.
    pub const fn bytes(self) -> [u8; 4] {
        self.bytes
    }

    /// Return the descriptor kind stored in byte zero.
    pub const fn kind(self) -> u8 {
        self.bytes[0]
    }

    /// Decode a kind-`0x37` target through `FUN_00444900`'s coordinate
    /// branch. Other descriptor kinds require their live object tables.
    pub fn world_coordinate(self) -> Option<(i32, i32)> {
        (self.kind() == Self::WORLD_COORDINATE_KIND).then(|| {
            self.packed_coordinate()
                .expect("world coordinate descriptor has packed coordinates")
        })
    }

    /// Decode either packed-coordinate form handled by `FUN_00444af0`.
    ///
    /// Kind `0x37` expands the coordinate to the route grid by a factor of
    /// two; kind `0x38` retains this packed coordinate directly. The type-4
    /// scenario loader preserves the latter form.
    pub fn packed_coordinate(self) -> Option<(i32, i32)> {
        matches!(
            self.kind(),
            Self::WORLD_COORDINATE_KIND | Self::FIXED_POINT_COORDINATE_KIND
        )
        .then(|| {
            let x = (u16::from(self.bytes[1] & 0x0f) << 8) | u16::from(self.bytes[2]);
            let y = (u16::from(self.bytes[1] >> 4) << 8) | u16::from(self.bytes[3]);
            (i32::from(x), i32::from(y))
        })
    }

    /// Resolve the raw type-4 route coordinate returned by
    /// `FUN_00444af0`. Kind `0x37` doubles its packed world cell, while kind
    /// `0x38` retains its packed raw figure coordinate.
    pub fn source_land_route_coordinate(self) -> Option<(i32, i32)> {
        let (x, y) = self.packed_coordinate()?;
        match self.kind() {
            Self::WORLD_COORDINATE_KIND => Some((x.checked_mul(2)?, y.checked_mul(2)?)),
            Self::FIXED_POINT_COORDINATE_KIND => Some((x, y)),
            _ => None,
        }
    }

    /// Resolve the source fallback `[x, y, 1, 1]` footprint used by
    /// `FUN_004451a0` for a coordinate descriptor.
    pub fn target_rect(self) -> Option<SourcePathTargetRect> {
        self.world_coordinate()
            .and_then(|position| SourcePathTargetRect::new(position, 1, 1))
    }

    /// Resolve the static island-map kinds `0x32`, `0x33`, and `0x34` through the
    /// scenario's preserved map cell. The descriptor's second byte is the
    /// source island slot, followed by local x/y bytes. `FUN_00463830`
    /// swaps the compiled definition's dimensions for odd orientations.
    /// This is the undoubled map-grid footprint returned by `FUN_004451a0`.
    pub fn resolve_static_island_target(
        self,
        islands: &[Island],
        definitions: &[CodBuilding],
    ) -> Option<SourceResolvedStaticTarget> {
        if !matches!(self.kind(), 0x32 | 0x33 | 0x34) {
            return None;
        }

        let island = islands
            .iter()
            .find(|island| island.number == self.bytes[1])?;
        let local_position = (self.bytes[2], self.bytes[3]);
        let tile = island
            .tiles
            .iter()
            .copied()
            .find(|tile| (tile.x, tile.y) == local_position)?;
        let definition = definitions
            .iter()
            .find(|definition| definition.source_id == tile.source_id())?;
        let (width, height) = source_footprint_size(definition.size, tile.orientation);
        let origin = (
            i32::from(island.x_pos) + i32::from(local_position.0),
            i32::from(island.y_pos) + i32::from(local_position.1),
        );
        Some(SourceResolvedStaticTarget {
            target: SourcePathTargetRect::new(origin, usize::from(width), usize::from(height))?,
            owner: tile.source_owner(),
        })
    }

    /// Resolve the doubled raw-grid footprint returned by `FUN_00444fe0` for
    /// a type-4 land route. Static kinds `0x32`, `0x33`, and `0x34` share
    /// this conversion; the source doubles both the map-grid origin and the
    /// oriented footprint dimensions.
    pub fn resolve_static_island_land_target(
        self,
        islands: &[Island],
        definitions: &[CodBuilding],
    ) -> Option<SourceResolvedStaticTarget> {
        let resolved = self.resolve_static_island_target(islands, definitions)?;
        let origin = (
            resolved.target.origin.0.checked_mul(2)?,
            resolved.target.origin.1.checked_mul(2)?,
        );
        let width = resolved.target.width.checked_mul(2)?;
        let height = resolved.target.height.checked_mul(2)?;
        Some(SourceResolvedStaticTarget {
            target: SourcePathTargetRect::new(origin, width, height)?,
            owner: resolved.owner,
        })
    }

    /// Resolve object-backed kinds `0x35` and `0x36` through their source
    /// island-table entry. `FUN_00444900` reads the entry's local position;
    /// `FUN_00444100` reads its owner. `FUN_004451a0` uses the corresponding
    /// static cell's oriented footprint for `0x35`, while `0x36` falls through
    /// to the default unit footprint.
    pub fn resolve_dynamic_map_object_target(
        self,
        objects: &[SourceDynamicMapObject],
        islands: &[Island],
        definitions: &[CodBuilding],
    ) -> Option<SourceResolvedDynamicTarget> {
        if !matches!(self.kind(), 0x35 | 0x36) {
            return None;
        }

        let object = objects
            .iter()
            .copied()
            .find(|object| object.island == self.bytes[1] && object.slot == self.bytes[2])?;
        let island = islands
            .iter()
            .find(|island| island.number == object.island)?;
        let origin = (
            i32::from(island.x_pos) + i32::from(object.local_position.0),
            i32::from(island.y_pos) + i32::from(object.local_position.1),
        );
        let (width, height) = if self.kind() == 0x35 {
            let tile = island
                .tiles
                .iter()
                .copied()
                .find(|tile| (tile.x, tile.y) == object.local_position)?;
            let definition = definitions
                .iter()
                .find(|definition| definition.source_id == tile.source_id())?;
            source_footprint_size(definition.size, tile.orientation)
        } else {
            (1, 1)
        };
        Some(SourceResolvedDynamicTarget {
            target: SourcePathTargetRect::new(origin, usize::from(width), usize::from(height))?,
            owner: object.owner,
        })
    }

    /// `FUN_00455a20` has no owner-resolution switch arm for kind `0x37`,
    /// so it takes its threshold branch: `FUN_0046e350(..., 5)` followed by
    /// `LAB_0046c750` with limit zero.
    pub const fn threshold_route_parameters(self) -> Option<(i32, u32)> {
        match self.kind() {
            Self::WORLD_COORDINATE_KIND => Some((5, 0)),
            _ => None,
        }
    }

    /// Select the `FUN_00455a20` target branch after the caller has resolved
    /// the descriptor owner through `FUN_00444100`. `nation_relation` is the
    /// byte read from `DAT_005b7770[(current_owner * 0x50 + target_owner) *
    /// 8]`; the executable tests precisely for value `3`.
    ///
    /// `direct_approach_radius` is the caller's `Shotradius >> 3` value. The
    /// threshold branch has a fixed radius five and target-kind-specific
    /// `LAB_0046c750` limit.
    pub fn select_fun_00455a20_branch(
        self,
        current_owner: u8,
        resolved_owner: Option<u8>,
        nation_relation: Option<u8>,
        direct_approach_radius: i32,
    ) -> SourceShipTargetRouteBranch {
        let owner_sensitive = matches!(self.kind(), 1..=6 | 0x32 | 0x33);
        if owner_sensitive {
            if let Some(target_owner) = resolved_owner {
                if target_owner != current_owner && nation_relation != Some(3) {
                    return SourceShipTargetRouteBranch::Direct {
                        approach_radius: direct_approach_radius,
                    };
                }
            }
        }

        let limit = match self.kind() {
            1 | 2 | 3 | 0x32 | 0x33 | 0x34 | 0x35 => 2,
            _ => 0,
        };
        SourceShipTargetRouteBranch::Threshold {
            approach_radius: 5,
            limit,
        }
    }
}

/// Mutable target callback state laid out beside the local target point in
/// the ship-route callers. The executable stores a selected point, a best
/// candidate, and the weighted metric used to compare candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePathTargetCallbackState {
    pub selected: (i32, i32),
    pub candidate: (i32, i32),
    pub best_distance: u32,
}

impl SourcePathTargetCallbackState {
    /// Initialize the state with the caller's pre-search weighted distance.
    pub const fn new(selected: (i32, i32), initial_best_distance: u32) -> Self {
        Self {
            selected,
            candidate: selected,
            best_distance: initial_best_distance,
        }
    }
}

/// Source ship target callback behavior selected by the two caller branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePathTargetCallback {
    /// `LAB_0046c670`: record the reached callback cell and stop the grid.
    Direct,
    /// `LAB_0046c750`: keep expanding high-metadata cells, recording closer
    /// candidates, until the callback metric is within this source threshold.
    Threshold { limit: u32 },
}

/// The two target-approach ray variants selected by the source ship callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTargetApproachRayMode {
    /// `FUN_0046dde0`: skip the ray origin and stop before a `0x0d`
    /// direction marker.
    StopAtDirection13,
    /// `FUN_0046e350`: mark every rasterized cell, including the origin.
    MarkWholeRay,
}

impl SourcePathTargetCallback {
    /// Evaluate one high-metadata callback cell exactly as the selected source
    /// callback does. `elapsed_cost` is `FUN_0046c7d0`'s `local_2c`; neither
    /// source target callback uses it in its distance metric.
    pub fn decide(
        self,
        position: (i32, i32),
        elapsed_cost: u32,
        state: &mut SourcePathTargetCallbackState,
    ) -> SourcePathBlockedCellDecision {
        match self {
            Self::Direct => {
                state.candidate = position;
                state.best_distance = source_target_metric(position, state.selected);
                SourcePathBlockedCellDecision::Complete
            }
            Self::Threshold { limit } => {
                let _ = elapsed_cost;
                let distance = source_target_metric(position, state.selected);
                if distance <= limit {
                    state.candidate = position;
                    state.best_distance = distance;
                    SourcePathBlockedCellDecision::Complete
                } else {
                    if distance < state.best_distance {
                        state.candidate = position;
                        state.best_distance = distance;
                    }
                    SourcePathBlockedCellDecision::Expand
                }
            }
        }
    }
}

/// The `max(Δx, Δy) + floor(min(Δx, Δy) / 4)` metric written by both target
/// callbacks at runtime context offset `+0x20`.
pub fn source_target_metric(position: (i32, i32), selected: (i32, i32)) -> u32 {
    let dx = position.0.abs_diff(selected.0);
    let dy = position.1.abs_diff(selected.1);
    dx.max(dy) + dx.min(dy) / 4
}

/// Reproduce category-6's target-footprint gate in `FUN_00458d80` after the
/// descriptor has been resolved through `FUN_00444fe0`. Both inputs are raw
/// doubled coordinates. The source clamps `position` to the target rectangle,
/// accepts only metric at most `floor(shot_radius / 4)`, and returns its
/// zero-based direction code from `FUN_00454050`.
pub fn source_kind6_target_direction(
    position: (i32, i32),
    target: SourcePathTargetRect,
    shot_radius: u16,
) -> Option<u8> {
    let selected = target.nearest_point(position);
    (source_target_metric(position, selected) <= u32::from(shot_radius) / 4)
        .then(|| source_target_direction(selected.0 - position.0, selected.1 - position.1))
}

impl SourcePathTargetRect {
    /// Construct a source target rectangle. Empty source footprints do not
    /// reach either `FUN_00443380` or `FUN_0046d680`.
    pub fn new(origin: (i32, i32), width: usize, height: usize) -> Option<Self> {
        (width != 0 && height != 0 && i32::try_from(width).is_ok() && i32::try_from(height).is_ok())
            .then_some(Self {
                origin,
                width,
                height,
            })
    }

    /// Select the target point clamped to this rectangle, exactly as
    /// `FUN_00443380` does before the `LAB_0046c670` callback path.
    pub fn nearest_point(self, position: (i32, i32)) -> (i32, i32) {
        let max_x = self.origin.0.saturating_add(self.width as i32 - 1);
        let max_y = self.origin.1.saturating_add(self.height as i32 - 1);
        (
            position.0.clamp(self.origin.0, max_x),
            position.1.clamp(self.origin.1, max_y),
        )
    }

    /// Whether a world-space cell belongs to this source footprint.
    pub fn contains(self, position: (i32, i32)) -> bool {
        position.0 >= self.origin.0
            && position.1 >= self.origin.1
            && position.0 < self.origin.0.saturating_add(self.width as i32)
            && position.1 < self.origin.1.saturating_add(self.height as i32)
    }

    /// Select the central target cell on the side facing `position`, exactly
    /// as `FUN_004433d0` does before the `LAB_0046c750` callback path.
    pub fn center_point_toward(self, position: (i32, i32)) -> (i32, i32) {
        let x = self.center_axis_toward(self.origin.0, self.width, position.0, false);
        let y = self.center_axis_toward(self.origin.1, self.height, position.1, true);
        (x, y)
    }

    fn center_axis_toward(self, origin: i32, length: usize, position: i32, y_axis: bool) -> i32 {
        let lower_center = origin.saturating_add((length as i32 - 1) / 2);
        let upper_center = lower_center + (length as i32 - 1) % 2;
        if if y_axis {
            position > lower_center
        } else {
            lower_center < position
        } {
            upper_center
        } else {
            lower_center
        }
    }
}

/// Input failure while projecting static INSELHAUS records into the source
/// path grid used by `FUN_0046f230`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePathGridBuildError {
    InvalidMovementType(usize),
}

/// Test the dynamic overlay condition used by `FUN_0046f230`.
///
/// The executable reads `permissions[definition.kind_code]` and, unless the
/// requested owner is the wildcard 7, requires the static cell's owner code
/// to match. Fixed terrain kinds bypass this condition inside the same
/// routine.
pub fn source_path_eligible(
    definition: &CodBuilding,
    permissions: &[u8],
    requested_owner: u8,
    tile: IslandTile,
) -> bool {
    definition
        .source_kind_code()
        .and_then(|kind_code| permissions.get(kind_code as usize))
        .is_some_and(|&permission| permission != 0)
        && (requested_owner == 7 || requested_owner == tile.source_owner())
}

impl SourcePathGrid {
    /// Allocate a source path grid with the world-space origin and dimensions
    /// supplied to `FUN_0046c630`.
    pub fn new(origin: (i32, i32), width: usize, height: usize) -> Self {
        Self {
            origin,
            width,
            height,
            cells: vec![
                SourcePathCell {
                    direction: 0,
                    metadata: 0,
                };
                width.saturating_mul(height)
            ],
        }
    }

    /// Clear predecessor directions while preserving the low-seven-bit map
    /// classes and blocked metadata, as `FUN_0046d5e0` does for its region.
    pub fn clear_directions(&mut self) {
        for cell in &mut self.cells {
            cell.direction = 0;
        }
    }

    /// Project static INSELHAUS definitions into the two-byte path grid.
    ///
    /// `FUN_0046f230` first marks each destination cell with direction
    /// `0x0c`. It then overlays every definition footprint whose kind is
    /// fixed-path terrain or whose caller-selected permission-and-owner
    /// condition holds.
    ///
    /// The grid origin must be `(island.x_pos, island.y_pos)` and its
    /// dimensions must be the island's local width and height. Definitions
    /// without a four-entry `Wegspeed` property are skipped, matching the
    /// absence of a path-class overlay for an unresolved source record.
    pub fn populate_static_island_cells<F>(
        &mut self,
        island: &Island,
        definitions: &[CodBuilding],
        movement_type: usize,
        mut path_eligible: F,
    ) -> Result<(), SourcePathGridBuildError>
    where
        F: FnMut(&CodBuilding, IslandTile) -> bool,
    {
        if movement_type >= 4 {
            return Err(SourcePathGridBuildError::InvalidMovementType(movement_type));
        }

        for cell in &mut self.cells {
            cell.direction = 0x0c;
        }

        for &tile in &island.tiles {
            let Some(definition) = definitions
                .iter()
                .find(|definition| definition.source_id == tile.source_id())
            else {
                continue;
            };
            let Some(path_classes) = definition.source_path_classes() else {
                continue;
            };

            let eligible = path_eligible(definition, tile);
            if !source_fixed_path_kind(definition) && !eligible {
                continue;
            }

            let (footprint_width, footprint_height) =
                source_footprint_size(definition.size, tile.orientation);
            let Some(last_x) = tile.x.checked_add(footprint_width) else {
                continue;
            };
            let Some(last_y) = tile.y.checked_add(footprint_height) else {
                continue;
            };
            if usize::from(last_x) > self.width || usize::from(last_y) > self.height {
                continue;
            }

            let metadata = path_classes[movement_type] | (u8::from(eligible) << 7);
            for y in tile.y as usize..last_y as usize {
                for x in tile.x as usize..last_x as usize {
                    let index = y * self.width + x;
                    self.cells[index] = SourcePathCell {
                        direction: 0,
                        metadata,
                    };
                }
            }
        }

        Ok(())
    }

    /// Set a cell's source path metadata. The high bit is the source blocked
    /// flag; the lower seven bits select the cost-table entry.
    pub fn set_metadata(&mut self, position: (i32, i32), metadata: u8) -> bool {
        let Some(index) = self.index(position) else {
            return false;
        };
        self.cells[index].metadata = metadata;
        true
    }

    /// Restrict traversal to one source temporary-grid rectangle. The caller
    /// supplies the world-space origin and positive dimensions passed to the
    /// source grid constructor; cells outside receive direction marker `0xc`
    /// and therefore cannot enter the frontier.
    pub fn block_outside_rect(&mut self, origin: (i32, i32), width: usize, height: usize) -> bool {
        let (Ok(width), Ok(height)) = (i32::try_from(width), i32::try_from(height)) else {
            return false;
        };
        let (Some(end_x), Some(end_y)) =
            (origin.0.checked_add(width), origin.1.checked_add(height))
        else {
            return false;
        };
        for y in 0..self.height {
            for x in 0..self.width {
                let position = (self.origin.0 + x as i32, self.origin.1 + y as i32);
                if position.0 < origin.0
                    || position.0 >= end_x
                    || position.1 < origin.1
                    || position.1 >= end_y
                {
                    self.cells[y * self.width + x].direction = 0x0c;
                }
            }
        }
        true
    }

    /// Apply the symmetric radius clip generated by `FUN_00404d70` and used
    /// by `FUN_00471280`. The source runs it after overlaying a type-12
    /// worker's root footprint, so it writes only direction marker `0x0c` and
    /// leaves path metadata intact.
    pub fn block_outside_source_radius_mask(
        &mut self,
        radius: usize,
        root_width: usize,
        root_height: usize,
    ) -> bool {
        if radius <= 1 {
            return true;
        }
        let root_width = root_width.max(1);
        let root_height = root_height.max(1);
        let Some(expected_width) = radius
            .checked_mul(2)
            .and_then(|margin| margin.checked_add(root_width))
        else {
            return false;
        };
        let Some(expected_height) = radius
            .checked_mul(2)
            .and_then(|margin| margin.checked_add(root_height))
        else {
            return false;
        };
        if self.width != expected_width || self.height != expected_height {
            return false;
        }

        self.block_outside_source_radius_window(
            self.origin,
            self.width,
            self.height,
            radius,
            0,
            root_height - 1,
        )
    }

    /// Run the same `FUN_00471280` carve over a grid that is **larger** than
    /// the source's scratch window.
    ///
    /// `FUN_004704d0` / `FUN_004706e0` allocate a window that is exactly
    /// `extra_x + 1 + radius * 2` by `extra_y + 1 + radius * 2`
    /// (`1602_exe.c:61664-61671`), so in the original the window and the grid
    /// are the same object and `FUN_00471280` indexes from its corner. A port
    /// that floods a whole-island grid has to be told where that window sits,
    /// which is what `window_origin` supplies; `extra_x`/`extra_y` are the
    /// source's own `param_2`/`param_3`, the `(size - 1) & 1` parity bits of
    /// the requesting root's oriented footprint.
    ///
    /// Only the direction byte is written, exactly as `FUN_00471340`
    /// (`1602_exe.c:80140`) does: a goal bit already stamped into the
    /// metadata survives the carve, and the cell is unreachable because the
    /// wave cannot step onto direction `0x0c`. Cells outside the window are
    /// the caller's business — in the source they do not exist.
    pub fn block_outside_source_radius_window(
        &mut self,
        window_origin: (i32, i32),
        window_width: usize,
        window_height: usize,
        radius: usize,
        extra_x: usize,
        extra_y: usize,
    ) -> bool {
        // `if (1 < (int)param_1)` at `1602_exe.c:80113` — radius 0 and 1 keep
        // the raw rectangle.
        if radius <= 1 {
            return true;
        }

        for (offset, half_width) in source_radius_profile(radius).into_iter().enumerate() {
            let left = radius.saturating_sub(half_width);
            // `iVar1 = extra_x + radius + table[offset]` is the last retained
            // column, and `FUN_00471340` starts blocking at `iVar1 + 1`.
            let right_exclusive = radius + extra_x + half_width + 1;
            for row in [radius - offset, radius + extra_y + offset] {
                if row >= window_height {
                    continue;
                }
                let y = window_origin.1 + row as i32;
                for column in (0..left).chain(right_exclusive..window_width) {
                    let position = (window_origin.0 + column as i32, y);
                    if let Some(index) = self.index(position) {
                        self.cells[index].direction = 0x0c;
                    }
                }
            }
        }
        true
    }

    /// Set one source direction byte without changing the cell's path
    /// metadata. `FUN_0046f460` uses this when it overlays a static map cell
    /// into its temporary type-4 target-selection grid.
    pub fn set_direction_marker(&mut self, position: (i32, i32), direction: u8) -> bool {
        let Some(index) = self.index(position) else {
            return false;
        };
        self.cells[index].direction = direction;
        true
    }

    /// Set a path-grid cell selected by the static-map overlay. This clears
    /// its `0x0c` direction blocker exactly as `FUN_0046f000` does after it
    /// accepts a fixed or permission-matched source map object.
    pub fn set_traversable_cell(&mut self, position: (i32, i32), metadata: u8) -> bool {
        let Some(index) = self.index(position) else {
            return false;
        };
        self.cells[index] = SourcePathCell {
            direction: 0,
            metadata,
        };
        true
    }

    /// Read a cell's source path metadata.
    pub fn metadata(&self, position: (i32, i32)) -> Option<u8> {
        self.index(position).map(|index| self.cells[index].metadata)
    }

    /// Mark a cell with source direction marker `0xc`, as
    /// `FUN_0046f6d0`/`FUN_0046d900` do for path blockers.
    pub fn mark_direction_blocker(&mut self, position: (i32, i32)) -> bool {
        self.set_direction_marker(position, 0x0c)
    }

    /// Rasterize a source direction-13 segment as `FUN_0046dd30` does. The
    /// caller supplies already-clipped endpoints; this adapter rejects a
    /// segment unless both endpoints are inside this grid.
    pub fn mark_direction_13_segment(&mut self, start: (i32, i32), end: (i32, i32)) -> bool {
        if self.index(start).is_none() || self.index(end).is_none() {
            return false;
        }
        Self::source_raster_segment(start, end, |position| {
            let index = self
                .index(position)
                .expect("a raster between in-bounds source-grid endpoints stays in bounds");
            self.cells[index].direction = 0x0d;
        });
        true
    }

    /// Overlay one candidate figure accepted by `FUN_00453e50`'s dynamic
    /// mask. Only live categories 1 through 3 receive this direction-13
    /// footprint; category 4 land figures and later categories leave the
    /// temporary grid unchanged. `FUN_0046d9d0` uses the wide, five-cell
    /// branches selected by `FUN_0046c630(..., 1)` in that caller.
    pub fn overlay_source_candidate_footprint(
        &mut self,
        figure_kind: u8,
        position: (i32, i32),
        direction: u8,
    ) -> bool {
        if !(1..=3).contains(&figure_kind) {
            return false;
        }

        let center = (
            position.0.saturating_sub(self.origin.0),
            position.1.saturating_sub(self.origin.1),
        );
        let mut changed = false;
        let mut mark = |start, end| {
            changed |= self.mark_direction_13_segment_clipped(start, end);
        };
        match direction {
            0 | 4 => mark((center.0, center.1 - 2), (center.0, center.1 + 2)),
            1 | 5 => {
                mark((center.0 + 2, center.1 - 2), (center.0 - 2, center.1 + 2));
                mark((center.0 + 1, center.1 - 2), (center.0 - 2, center.1 + 1));
            }
            2 | 6 => mark((center.0 - 2, center.1), (center.0 + 2, center.1)),
            3 | 7 => {
                mark((center.0 - 2, center.1 - 2), (center.0 + 2, center.1 + 2));
                mark((center.0 - 2, center.1 - 1), (center.0 + 2, center.1 + 1));
            }
            _ => return false,
        }
        changed
    }

    /// `FUN_0046db80` clips a direction-13 segment to the local temporary
    /// grid before forwarding it to `FUN_0046dd30`. This keeps a candidate
    /// at the boundary from losing the in-bounds portion of its footprint.
    fn mark_direction_13_segment_clipped(
        &mut self,
        mut start: (i32, i32),
        mut end: (i32, i32),
    ) -> bool {
        let max_x = self.width as i32 - 1;
        let max_y = self.height as i32 - 1;
        let edge_bits = |point: (i32, i32)| {
            (u8::from(point.0 < 0))
                | (u8::from(point.0 > max_x) << 1)
                | (u8::from(point.1 < 0) << 2)
                | (u8::from(point.1 > max_y) << 3)
        };
        let start_bits = edge_bits(start);
        let end_bits = edge_bits(end);
        if start_bits | end_bits == 0 {
            return self.mark_direction_13_segment(start, end);
        }
        if start_bits & end_bits != 0 {
            return false;
        }

        let delta_x = end.0 - start.0;
        let delta_y = end.1 - start.1;
        if start_bits & 1 != 0 {
            start.1 -= delta_y * start.0 / delta_x;
            start.0 = 0;
        } else if start_bits & 2 != 0 {
            start.0 -= max_x;
            start.1 -= delta_y * start.0 / delta_x;
            start.0 = max_x;
        }
        if start.1 < 0 {
            start.0 -= delta_x * start.1 / delta_y;
            start.1 = 0;
        } else if start.1 > max_y {
            start.1 -= max_y;
            start.0 -= delta_x * start.1 / delta_y;
            start.1 = max_y;
        }

        if end_bits & 1 != 0 {
            end.1 -= delta_y * end.0 / delta_x;
            end.0 = 0;
        } else if end_bits & 2 != 0 {
            end.0 -= max_x;
            end.1 -= delta_y * end.0 / delta_x;
            end.0 = max_x;
        }
        if end.1 < 0 {
            end.0 -= delta_x * end.1 / delta_y;
            end.1 = 0;
        } else if end.1 > max_y {
            end.1 -= max_y;
            end.0 -= delta_x * end.1 / delta_y;
            end.1 = max_y;
        }

        if start == end {
            false
        } else {
            self.mark_direction_13_segment(start, end)
        }
    }

    /// `FUN_0046e8b0`'s direct direction-13 test. It rejects endpoints outside
    /// the local grid, requires the source's ceiling-quarter distance bound,
    /// and scans every raster cell after `start`. A direction-13 marker at
    /// `end` is permitted; every earlier direction-13 marker rejects the ray.
    pub fn direction_13_ray_clear(&self, start: (i32, i32), end: (i32, i32), radius: u32) -> bool {
        if self.index(start).is_none() || self.index(end).is_none() {
            return false;
        }
        let dx = start.0.abs_diff(end.0);
        let dy = start.1.abs_diff(end.1);
        let source_distance = dx.max(dy) + (dx.min(dy) + 3) / 4;
        if source_distance > radius {
            return false;
        }

        let mut clear = true;
        Self::source_raster_segment(start, end, |position| {
            if position != start
                && position != end
                && self
                    .index(position)
                    .is_some_and(|index| self.cells[index].direction == 0x0d)
            {
                clear = false;
            }
        });
        clear
    }

    /// The inclusive Bresenham traversal shared by `FUN_0046dd30` and
    /// `FUN_0046e8b0`. Its initial error is `floor(major / 2)`, matching the
    /// source's signed divide-by-two sequence.
    fn source_raster_segment<F>(start: (i32, i32), end: (i32, i32), mut visit: F)
    where
        F: FnMut((i32, i32)),
    {
        let dx = start.0.abs_diff(end.0);
        let dy = start.1.abs_diff(end.1);
        let step_x = if end.0 >= start.0 { 1 } else { -1 };
        let step_y = if end.1 >= start.1 { 1 } else { -1 };
        let mut position = start;
        visit(position);

        if dx > dy {
            let mut error = i64::from(dx) / 2;
            for _ in 0..dx {
                position.0 += step_x;
                error -= i64::from(dy);
                if error < 0 {
                    error += i64::from(dx);
                    position.1 += step_y;
                }
                visit(position);
            }
        } else {
            let mut error = i64::from(dy) / 2;
            for _ in 0..dy {
                position.1 += step_y;
                error -= i64::from(dx);
                if error < 0 {
                    error += i64::from(dy);
                    position.0 += step_x;
                }
                visit(position);
            }
        }
    }

    /// Mark a target rectangle as high-metadata callback cells. This is the
    /// clipped cell loop of `FUN_0046d680`: it clears predecessor directions
    /// inside the target footprint and sets metadata bit `0x80`, preserving
    /// the source path-cost class in the low seven bits.
    pub fn mark_target_region(&mut self, target: SourcePathTargetRect) -> bool {
        let max_x = target.origin.0.saturating_add(target.width as i32);
        let max_y = target.origin.1.saturating_add(target.height as i32);
        let start_x = target.origin.0.max(self.origin.0);
        let start_y = target.origin.1.max(self.origin.1);
        let end_x = max_x.min(self.origin.0 + self.width as i32);
        let end_y = max_y.min(self.origin.1 + self.height as i32);
        if start_x >= end_x || start_y >= end_y {
            return false;
        }

        for y in start_y..end_y {
            for x in start_x..end_x {
                let index = self.index((x, y)).expect("target clip is in bounds");
                self.cells[index].direction = 0;
                self.cells[index].metadata |= 0x80;
            }
        }
        true
    }

    /// Assign one metadata byte to every clipped target cell. Like
    /// `mark_target_region`, this clears predecessor directions first; callers
    /// use it when the source object supplies both the callback bit and path
    /// cost class for its entire oriented footprint.
    pub fn set_target_region_metadata(
        &mut self,
        target: SourcePathTargetRect,
        metadata: u8,
    ) -> bool {
        let max_x = target.origin.0.saturating_add(target.width as i32);
        let max_y = target.origin.1.saturating_add(target.height as i32);
        let start_x = target.origin.0.max(self.origin.0);
        let start_y = target.origin.1.max(self.origin.1);
        let end_x = max_x.min(self.origin.0 + self.width as i32);
        let end_y = max_y.min(self.origin.1 + self.height as i32);
        if start_x >= end_x || start_y >= end_y {
            return false;
        }

        for y in start_y..end_y {
            for x in start_x..end_x {
                let index = self.index((x, y)).expect("target clip is in bounds");
                self.cells[index].direction = 0;
                self.cells[index].metadata = metadata;
            }
        }
        true
    }

    /// Open the requesting object's own footprint, the way `FUN_004710b0`
    /// (`1602_exe.c:80003-80093`) does between the window raster and the
    /// flood.
    ///
    /// Both transfer searches run it on the figure's own tile —
    /// `FUN_00459150` at `1602_exe.c:61677` for the type-8 carrier and
    /// `FUN_004596b0` at `1602_exe.c:61878` for the type-11 city cart — and it
    /// is what lets a root leave its own tiles at all. Without it a MARKT or
    /// KONTOR is walled in by its own footprint: `FUN_004704d0` /
    /// `FUN_004706e0` stamp every cell of a production-kind root that belongs
    /// to the searching settlement with the goal bit, and a goal cell ends the
    /// wave instead of expanding it, so only the single start cell would ever
    /// be traversable.
    ///
    /// The source writes `direction = 0` and `metadata = 0x28` verbatim
    /// (`:80048-80049`), i.e. it drops the goal bit and pins the cost class
    /// rather than reading the object's own `Wegspeed`.
    ///
    /// The `(def[0x6b] & 1) != 0` variant at `:80059-80081`, which opens only
    /// the footprint cells whose **backing** terrain record is an outer kind
    /// in `10..=12`, is not reproduced: the port carries no `+0x6b` byte yet.
    /// Taking the unconditional branch is the permissive direction — it can
    /// only open cells the source would also have opened for a definition
    /// without that flag.
    pub fn open_source_object_footprint(&mut self, target: SourcePathTargetRect) -> bool {
        let max_x = target.origin.0.saturating_add(target.width as i32);
        let max_y = target.origin.1.saturating_add(target.height as i32);
        let start_x = target.origin.0.max(self.origin.0);
        let start_y = target.origin.1.max(self.origin.1);
        let end_x = max_x.min(self.origin.0 + self.width as i32);
        let end_y = max_y.min(self.origin.1 + self.height as i32);
        if start_x >= end_x || start_y >= end_y {
            return false;
        }

        for y in start_y..end_y {
            for x in start_x..end_x {
                let index = self.index((x, y)).expect("footprint clip is in bounds");
                self.cells[index] = SourcePathCell {
                    direction: 0,
                    metadata: 0x28,
                };
            }
        }
        true
    }

    /// Mark the target-approach rays generated by `FUN_0046dde0` or
    /// `FUN_0046e350`. Both routines fan four side rays and twelve diagonal
    /// rays from the target footprint; their only difference is the raster
    /// policy selected by `mode`.
    pub fn mark_target_approach_rays(
        &mut self,
        target: SourcePathTargetRect,
        radius: i32,
        mode: SourceTargetApproachRayMode,
    ) {
        let left = target.origin.0.saturating_sub(self.origin.0);
        let top = target.origin.1.saturating_sub(self.origin.1);
        let width = target.width as i32;
        let height = target.height as i32;
        let right = left.saturating_add(width - 1);
        let bottom = top.saturating_add(height - 1);

        for x in (left..=right).rev() {
            self.mark_target_ray((x, top), (x, top - radius), mode);
        }
        for x in (left..=right).rev() {
            self.mark_target_ray((x, bottom), (x, bottom + radius), mode);
        }
        for y in (top..=bottom).rev() {
            self.mark_target_ray((left, y), (left - radius, y), mode);
        }
        for y in (top..=bottom).rev() {
            self.mark_target_ray((right, y), (right + radius, y), mode);
        }

        let (near, diagonal, far) = if radius == 1 {
            (0, 1, 1)
        } else {
            (
                radius.saturating_mul(0x57) / 0x100,
                radius.saturating_mul(0xc2) / 0x100,
                radius.saturating_mul(0xf0) / 0x100,
            )
        };
        for (start, end) in [
            ((left, top), (left - near, top - far)),
            ((left, top), (left - diagonal, top - diagonal)),
            ((left, top), (left - far, top - near)),
            ((right, top), (left + near, top - far)),
            ((right, top), (left + diagonal, top - diagonal)),
            ((right, top), (left + far, top - near)),
            ((right, bottom), (left + near, top + far)),
            ((right, bottom), (left + diagonal, top + diagonal)),
            ((right, bottom), (left + far, top + near)),
            ((left, bottom), (left - near, top + far)),
            ((left, bottom), (left - diagonal, top + diagonal)),
            ((left, bottom), (left - far, top + near)),
        ] {
            self.mark_target_ray(start, end, mode);
        }
    }

    fn mark_target_ray(
        &mut self,
        start: (i32, i32),
        end: (i32, i32),
        mode: SourceTargetApproachRayMode,
    ) {
        let max_x = self.width as i32 - 1;
        let max_y = self.height as i32 - 1;
        let start_code = source_ray_outcode(start, max_x, max_y);
        let end_code = source_ray_outcode(end, max_x, max_y);
        if start_code + end_code == 0 {
            self.raster_target_ray(start, end, mode);
            return;
        }
        if start_code & end_code != 0 {
            return;
        }

        let (mut x0, mut y0) = start;
        let (mut x1, mut y1) = end;
        let dx = x1 - x0;
        let dy = y1 - y0;

        if start_code & 1 != 0 {
            let previous_x = x0;
            x0 = 0;
            y0 -= dy * previous_x / dx;
        } else if start_code & 2 != 0 {
            let overflow = x0 - max_x;
            x0 = max_x;
            y0 -= dy * overflow / dx;
        }
        if y0 < 0 {
            let previous_y = y0;
            y0 = 0;
            x0 -= dx * previous_y / dy;
        } else if y0 > max_y {
            let overflow = y0 - max_y;
            y0 = max_y;
            x0 -= dx * overflow / dy;
        }

        if end_code & 1 != 0 {
            let previous_x = x1;
            x1 = 0;
            y1 -= dy * previous_x / dx;
        } else if end_code & 2 != 0 {
            let overflow = x1 - max_x;
            x1 = max_x;
            y1 -= dy * overflow / dx;
        }
        if y1 < 0 {
            let previous_y = y1;
            y1 = 0;
            x1 -= dx * previous_y / dy;
        } else if y1 > max_y {
            let overflow = y1 - max_y;
            y1 = max_y;
            x1 -= dx * overflow / dy;
        }

        if (x0, y0) != (x1, y1) && (0..=max_x).contains(&x0) && (0..=max_x).contains(&x1) {
            self.raster_target_ray((x0, y0), (x1, y1), mode);
        }
    }

    fn raster_target_ray(
        &mut self,
        start: (i32, i32),
        end: (i32, i32),
        mode: SourceTargetApproachRayMode,
    ) {
        let (mut x, mut y) = start;
        if mode == SourceTargetApproachRayMode::MarkWholeRay {
            self.mark_high_metadata_local(x, y);
        }

        let dx = (end.0 - x).abs();
        let dy = (end.1 - y).abs();
        let step_x = if x <= end.0 { 1 } else { -1 };
        let step_y = if y <= end.1 { 1 } else { -1 };
        if dy < dx {
            let mut error = dx / 2;
            for _ in 0..dx {
                x += step_x;
                error -= dy;
                if error < 0 {
                    error += dx;
                    y += step_y;
                }
                if mode == SourceTargetApproachRayMode::StopAtDirection13
                    && self.direction_local(x, y) == Some(0x0d)
                {
                    break;
                }
                self.mark_high_metadata_local(x, y);
            }
        } else {
            let mut error = dy / 2;
            for _ in 0..dy {
                y += step_y;
                error -= dx;
                if error < 0 {
                    error += dy;
                    x += step_x;
                }
                if mode == SourceTargetApproachRayMode::StopAtDirection13
                    && self.direction_local(x, y) == Some(0x0d)
                {
                    break;
                }
                self.mark_high_metadata_local(x, y);
            }
        }
    }

    fn mark_high_metadata_local(&mut self, x: i32, y: i32) {
        let position = (self.origin.0 + x, self.origin.1 + y);
        let index = self.index(position).expect("clipped ray cell is in bounds");
        self.cells[index].metadata |= 0x80;
    }

    fn direction_local(&self, x: i32, y: i32) -> Option<u8> {
        let position = (self.origin.0 + x, self.origin.1 + y);
        self.index(position)
            .map(|index| self.cells[index].direction)
    }

    fn is_reached_predecessor_marker(&self, position: (i32, i32)) -> bool {
        self.index(position).is_some_and(|index| {
            let direction = self.cells[index].direction;
            direction != 0 && !(0x0b..=0x0d).contains(&direction)
        })
    }

    /// Return whether a cell has no source direction marker. A nonzero marker
    /// is impassable to `FUN_0046c7d0`.
    pub fn is_direction_clear(&self, position: (i32, i32)) -> Option<bool> {
        self.index(position)
            .map(|index| self.cells[index].direction == 0)
    }

    /// Construct the centered `2r + 1` square initialized by
    /// `FUN_0046c630`. Cells outside this backing static grid retain the
    /// source constructor's zero direction/metadata.
    pub fn source_window(&self, center: (i32, i32), radius: usize) -> Option<Self> {
        let diameter = radius.checked_mul(2)?.checked_add(1)?;
        let radius = i32::try_from(radius).ok()?;
        let origin = (center.0.checked_sub(radius)?, center.1.checked_sub(radius)?);
        let mut window = Self::new(origin, diameter, diameter);

        for y in 0..diameter {
            for x in 0..diameter {
                let position = (origin.0 + x as i32, origin.1 + y as i32);
                let Some(source_index) = self.index(position) else {
                    continue;
                };
                let destination_index = y * diameter + x;
                window.cells[destination_index] = self.cells[source_index];
            }
        }
        Some(window)
    }

    /// Overlay the ship-route direction markers written by `FUN_0046f6d0`
    /// for one active island. `IslandTile` records are already individual
    /// static-map cells, so their local coordinates are translated by the
    /// island's source world origin before clipping them to this path window.
    pub fn overlay_source_ship_route_blockers(
        &mut self,
        island: &Island,
        definitions: &[CodBuilding],
    ) {
        let definitions_by_source_id: HashMap<i32, &CodBuilding> = definitions
            .iter()
            .map(|definition| (definition.source_id, definition))
            .collect();

        for &tile in &island.tiles {
            let Some(definition) = definitions_by_source_id.get(&tile.source_id()) else {
                continue;
            };
            let Some(direction) = source_static_map_direction(definition, tile) else {
                continue;
            };
            let position = (
                i32::from(island.x_pos) + i32::from(tile.x),
                i32::from(island.y_pos) + i32::from(tile.y),
            );
            if let Some(index) = self.index(position) {
                self.cells[index].direction = direction;
            }
        }
    }

    /// Run the fixed-cost expansion in `FUN_0046c7d0`.
    ///
    /// The callback is consulted only for a due cell whose metadata high bit
    /// is set. `Block` reproduces a zero source callback return, `Expand`
    /// reproduces a nonzero return, and `Complete` models the source caller
    /// clearing its grid-state flag from that callback. Call
    /// [`Self::clear_directions`] and overlay any direction-marker blockers
    /// before each search, matching the source callers' setup sequence.
    pub fn search_with_blocked_cell_callback<F>(
        &mut self,
        start: (i32, i32),
        mut blocked_cell: F,
    ) -> Result<SourcePathSearchResult, SourcePathSearchError>
    where
        F: FnMut((i32, i32), u32) -> SourcePathBlockedCellDecision,
    {
        let Some(start_index) = self.index(start) else {
            return Err(SourcePathSearchError::OutOfBounds);
        };
        self.cells[start_index].direction = 0x0b;
        self.cells[start_index].metadata &= 0x7f;

        let mut current = vec![(start, 0x40_i32)];
        let mut elapsed_cost = 0;
        loop {
            let mut next = Vec::new();

            for &(position, cost) in current.iter().rev() {
                let remaining = cost - 0x40;
                if remaining > 0 {
                    next.push((position, remaining));
                    continue;
                }

                let index = self
                    .index(position)
                    .expect("frontier position is in bounds");
                if self.cells[index].metadata & 0x80 != 0 {
                    match blocked_cell(position, elapsed_cost) {
                        SourcePathBlockedCellDecision::Block => continue,
                        SourcePathBlockedCellDecision::Complete => {
                            return Ok(SourcePathSearchResult {
                                position,
                                elapsed_cost,
                                steps: self.trace_steps(start, position),
                            });
                        }
                        SourcePathBlockedCellDecision::AdvanceFrontier => break,
                        SourcePathBlockedCellDecision::Expand => {}
                    }
                }

                let metadata = self.cells[index].metadata;
                self.enqueue_neighbours(position, metadata, &mut next);
            }

            if next.is_empty() {
                return Err(SourcePathSearchError::NoRoute);
            }
            current = next;
            elapsed_cost += 0x40;
        }
    }

    /// Reproduce `FUN_00471c50`'s built-in high-metadata selector. Unlike
    /// `FUN_0046c7d0`'s callback form, this records every candidate in the
    /// current fixed-cost band and returns the source scan's final selection
    /// only after its following `0x40` boundary.
    pub fn search_source_high_metadata_target(
        &mut self,
        start: (i32, i32),
        completion_delay: u32,
    ) -> Result<SourcePathSearchResult, SourcePathSearchError> {
        let Some(start_index) = self.index(start) else {
            return Err(SourcePathSearchError::OutOfBounds);
        };
        self.cells[start_index].direction = 0x0b;
        self.cells[start_index].metadata &= 0x7f;

        let mut current = vec![(start, 0x40_i32)];
        let mut elapsed_cost = 0_u32;
        let mut selected = None;
        loop {
            let mut next = Vec::new();
            let mut stop_band = false;

            for &(position, cost) in current.iter().rev() {
                let remaining = cost - 0x40;
                if remaining > 0 {
                    next.push((position, remaining));
                    continue;
                }

                let index = self
                    .index(position)
                    .expect("frontier position is in bounds");
                if self.cells[index].metadata & 0x80 != 0 {
                    // The source scan finishes the current fixed-cost band
                    // even once the completion window is reached, so a later
                    // candidate in the same band replaces this one.
                    selected = Some(position);
                    if completion_delay.saturating_add(0x40) <= elapsed_cost {
                        stop_band = true;
                    }
                }

                let metadata = self.cells[index].metadata;
                self.enqueue_neighbours(position, metadata, &mut next);
            }

            if next.is_empty() {
                return selected
                    .map(|position| SourcePathSearchResult {
                        position,
                        elapsed_cost,
                        steps: self.trace_steps(start, position),
                    })
                    .ok_or(SourcePathSearchError::NoRoute);
            }
            elapsed_cost = elapsed_cost.saturating_add(0x40);
            if selected.is_some() && completion_delay.saturating_add(0x40) < elapsed_cost {
                let position = selected.expect("selection was checked above");
                return Ok(SourcePathSearchResult {
                    position,
                    elapsed_cost,
                    steps: self.trace_steps(start, position),
                });
            }
            if stop_band {
                let position = selected.expect("high-metadata cell stopped the band");
                return Ok(SourcePathSearchResult {
                    position,
                    elapsed_cost,
                    steps: self.trace_steps(start, position),
                });
            }
            current = next;
        }
    }

    /// Run `FUN_0046c7d0` to a specific destination.
    ///
    /// The source routine reaches a destination by marking it blocked and
    /// completing from its blocked-cell callback. This adapter supplies that
    /// callback contract while preserving the destination metadata.
    pub fn route_to(
        &mut self,
        start: (i32, i32),
        goal: (i32, i32),
    ) -> Result<Vec<SourceRouteStep>, SourcePathSearchError> {
        let Some(goal_index) = self.index(goal) else {
            return Err(SourcePathSearchError::OutOfBounds);
        };
        if self.index(start).is_none() {
            return Err(SourcePathSearchError::OutOfBounds);
        }
        if start == goal {
            return Ok(Vec::new());
        }

        let goal_metadata = self.cells[goal_index].metadata;
        self.cells[goal_index].metadata |= 0x80;
        let result = self
            .search_with_blocked_cell_callback(start, |position, _| {
                if position == goal {
                    SourcePathBlockedCellDecision::Complete
                } else {
                    SourcePathBlockedCellDecision::Block
                }
            })
            .map(|result| result.steps);
        self.cells[goal_index].metadata = goal_metadata;
        result
    }

    /// Route a source target region through the high-metadata callback
    /// mechanism used by `FUN_0046d680`: target cells become callback cells
    /// and the first reached target cell terminates the fixed-cost wave.
    ///
    /// The source ship callers use a `[x, y, 1, 1]` fallback footprint when
    /// target-map resolution fails, which is the form consumed by the ocean
    /// adapter. The caller-specific `LAB_0046c750` acceptance threshold is
    /// intentionally not inferred by this generic fallback helper.
    pub fn route_to_target_region(
        &mut self,
        start: (i32, i32),
        target: SourcePathTargetRect,
    ) -> Result<Vec<SourceRouteStep>, SourcePathSearchError> {
        self.route_to_direct_target(start, target, 0)
    }

    /// Route through the direct source target callback branch in
    /// `FUN_00455a20`: `FUN_0046d680`, then `FUN_0046dde0` with
    /// `Shotradius >> 3`, then `LAB_0046c670`.
    ///
    /// The direct callback completes on the first high-metadata cell it
    /// reaches, including an approach-ray cell outside the target footprint.
    pub fn route_to_direct_target(
        &mut self,
        start: (i32, i32),
        target: SourcePathTargetRect,
        approach_radius: i32,
    ) -> Result<Vec<SourceRouteStep>, SourcePathSearchError> {
        if self.index(start).is_none() {
            return Err(SourcePathSearchError::OutOfBounds);
        }
        if target.contains(start) {
            return Ok(Vec::new());
        }
        self.mark_target_region(target);
        self.mark_target_approach_rays(
            target,
            approach_radius,
            SourceTargetApproachRayMode::StopAtDirection13,
        );

        let selected = target.nearest_point(start);
        let initial_best_distance = source_target_metric(start, selected);
        let mut callback_state =
            SourcePathTargetCallbackState::new(selected, initial_best_distance);

        self.search_with_blocked_cell_callback(start, |position, elapsed_cost| {
            SourcePathTargetCallback::Direct.decide(position, elapsed_cost, &mut callback_state)
        })
        .map(|result| result.steps)
    }

    /// Run the threshold source target branch used by `FUN_00455a20` and
    /// `FUN_00456920`: target-region marking, `FUN_0046e350`, center-point
    /// selection, then `LAB_0046c750`.
    ///
    /// The caller owns both `approach_radius` and `limit`; those values vary
    /// by target descriptor kind in the executable.
    pub fn search_threshold_target(
        &mut self,
        start: (i32, i32),
        target: SourcePathTargetRect,
        approach_radius: i32,
        limit: u32,
    ) -> Result<SourcePathSearchResult, SourcePathSearchError> {
        if self.index(start).is_none() {
            return Err(SourcePathSearchError::OutOfBounds);
        }
        self.mark_target_region(target);
        self.mark_target_approach_rays(
            target,
            approach_radius,
            SourceTargetApproachRayMode::MarkWholeRay,
        );

        let selected = target.center_point_toward(start);
        let mut callback_state =
            SourcePathTargetCallbackState::new(selected, source_target_metric(start, selected));
        self.search_with_blocked_cell_callback(start, |position, elapsed_cost| {
            SourcePathTargetCallback::Threshold { limit }.decide(
                position,
                elapsed_cost,
                &mut callback_state,
            )
        })
    }

    /// Select the nearest reached predecessor marker for an out-of-window
    /// target, exactly as `FUN_0046d1e0` does before the caller consults
    /// `FUN_0046eb20`. `source_world_width` is runtime `DAT_005b6128`; the
    /// source initializes its strict best-distance bound to twice this value.
    ///
    /// A target already inside this local grid returns `None`, matching the
    /// source's early exit. The caller must still apply the source
    /// island-object predicate before turning the selected marker into a
    /// route endpoint.
    pub fn nearest_reached_marker(
        &self,
        target: (i32, i32),
        source_world_width: i32,
    ) -> Option<(i32, i32)> {
        let local_target_x = target.0.checked_sub(self.origin.0)?;
        let local_target_y = target.1.checked_sub(self.origin.1)?;
        let width = i32::try_from(self.width).ok()?;
        let height = i32::try_from(self.height).ok()?;

        let (boundary_x, target_inside_x) = if local_target_x < 0 {
            (0, false)
        } else if local_target_x < width {
            (local_target_x, true)
        } else {
            (width - 1, false)
        };
        let (boundary_y, target_inside_y) = if local_target_y < 0 {
            (0, false)
        } else if local_target_y < height {
            (local_target_y, true)
        } else {
            (height - 1, false)
        };
        if target_inside_x && target_inside_y {
            return None;
        }

        let mut best_distance = u32::try_from(source_world_width).ok()?.saturating_mul(2);
        let mut best = None;
        for y in 0..height {
            let position = (self.origin.0 + boundary_x, self.origin.1 + y);
            if self.is_reached_predecessor_marker(position) {
                let distance = source_target_metric(position, target);
                if distance < best_distance {
                    best_distance = distance;
                    best = Some(position);
                }
            }
        }
        for x in 0..width {
            let position = (self.origin.0 + x, self.origin.1 + boundary_y);
            if self.is_reached_predecessor_marker(position) {
                let distance = source_target_metric(position, target);
                if distance < best_distance {
                    best_distance = distance;
                    best = Some(position);
                }
            }
        }
        best
    }

    /// Trace the predecessor program to a caller-selected reached marker.
    /// Source ship callers obtain such a marker through
    /// [`Self::nearest_reached_marker`]; type-11 cart selection records its
    /// accepted callback cell directly.
    pub fn steps_to_reached_marker(
        &self,
        start: (i32, i32),
        marker: (i32, i32),
    ) -> Option<Vec<SourceRouteStep>> {
        (self.index(start).is_some() && self.is_reached_predecessor_marker(marker))
            .then(|| self.trace_steps(start, marker))
    }

    fn enqueue_neighbours(
        &mut self,
        position: (i32, i32),
        metadata: u8,
        next: &mut Vec<((i32, i32), i32)>,
    ) {
        let (x, y) = position;
        for (direction, dx, dy, diagonal) in [
            (8, -1, -1, true),
            (2, 1, -1, true),
            (6, -1, 1, true),
            (4, 1, 1, true),
            (1, 0, -1, false),
            (5, 0, 1, false),
            (7, -1, 0, false),
            (3, 1, 0, false),
        ] {
            let destination = (x + dx, y + dy);
            let Some(destination_index) = self.index(destination) else {
                continue;
            };
            if self.cells[destination_index].direction != 0 {
                continue;
            }

            if diagonal {
                let horizontal = self.index((x + dx, y));
                let vertical = self.index((x, y + dy));
                let clear_corners =
                    horizontal
                        .zip(vertical)
                        .is_some_and(|(horizontal, vertical)| {
                            self.cells[horizontal].direction == 0
                                && self.cells[vertical].direction == 0
                        });
                if !clear_corners {
                    continue;
                }
            }

            self.cells[destination_index].direction = direction;
            let step_cost = if diagonal {
                source_diagonal_cost(metadata)
            } else {
                source_cardinal_cost(metadata)
            } as i32;
            next.push((destination, step_cost));
        }
    }

    fn trace_steps(&self, start: (i32, i32), goal: (i32, i32)) -> Vec<SourceRouteStep> {
        let mut reversed = Vec::new();
        let mut position = goal;

        while position != start && reversed.len() < self.cells.len() {
            let index = self.index(position).expect("trace position is in bounds");
            let direction = self.cells[index].direction;
            let Some((dx, dy)) = source_direction_delta(direction) else {
                break;
            };
            position = (position.0 - dx, position.1 - dy);
            let predecessor = self
                .index(position)
                .expect("source predecessor is in bounds");
            reversed.push(SourceRouteStep {
                direction,
                metadata: self.cells[predecessor].metadata,
            });
        }

        reversed.reverse();
        reversed
    }

    fn index(&self, position: (i32, i32)) -> Option<usize> {
        let x = position.0.checked_sub(self.origin.0)?;
        let y = position.1.checked_sub(self.origin.1)?;
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return None;
        }
        Some(y as usize * self.width + x as usize)
    }
}

/// `FUN_00404d70` fills one radius row in the `DAT_005b7460` table at source
/// initialization. Each entry is the horizontal half-width retained at that
/// vertical offset by `FUN_00471280`.
fn source_radius_profile(radius: usize) -> Vec<usize> {
    let mut profile = vec![0; radius + 1];
    let mut i_var4 = 0_usize;
    let mut u_var7 = radius * radius;
    let mut u_var5 = radius.saturating_sub(1) * radius;
    let mut i_var6 = 0_usize;
    let mut i_var2 = radius;
    let mut twice_radius = radius * 2;

    loop {
        profile[i_var4] = i_var2;
        u_var7 = u_var7.wrapping_sub(1 + i_var6);
        let mut i_var3 = i_var2;
        if u_var7 <= u_var5 {
            i_var3 = i_var2.saturating_sub(1);
            profile[i_var2] = i_var4;
            twice_radius = twice_radius.wrapping_sub(2);
            u_var5 = u_var5.wrapping_sub(twice_radius);
        }
        i_var4 += 1;
        i_var6 += 2;
        i_var2 = i_var3;
        if i_var4 > i_var3 {
            break;
        }
    }
    if radius == 1 {
        profile[1] = 1;
    }
    profile
}

fn source_footprint_size(size: (i32, i32), orientation: u8) -> (u8, u8) {
    let width = u8::try_from(size.0.max(0)).unwrap_or(u8::MAX);
    let height = u8::try_from(size.1.max(0)).unwrap_or(u8::MAX);
    if orientation & 1 == 0 {
        (width, height)
    } else {
        (height, width)
    }
}

/// Cohen-Sutherland-style source outcode used by `FUN_0046e0e0` and
/// `FUN_0046e650` before they rasterize an approach ray.
fn source_ray_outcode(position: (i32, i32), max_x: i32, max_y: i32) -> u8 {
    let mut code = 0;
    if position.0 < 0 {
        code |= 1;
    } else if position.0 > max_x {
        code |= 2;
    }
    if position.1 < 0 {
        code |= 4;
    } else if position.1 > max_y {
        code |= 8;
    }
    code
}

/// The unconditional switch arm of `FUN_0046f230` (`1602_exe.c:78435-78441`),
/// which is the same "fixed path terrain" set the type-8 transfer wave walks
/// — see [`crate::island_map::source_transfer_wave_opens_ground_kind`].
fn source_fixed_path_kind(definition: &CodBuilding) -> bool {
    definition
        .source_kind_code()
        .is_some_and(crate::island_map::source_transfer_wave_opens_ground_kind)
}

/// Return the source direction marker that `FUN_0046f460`
/// (`1602_exe.c:78548-78554`) and `FUN_0046f6d0` (`:78640-78646`) write for
/// one static map cell. `None` is the explicit `MEER`/`KIRCHE` pass-through.
///
/// Both routines carry the same "fixed path terrain" arm as the type-8
/// transfer wave, so the walkable branch defers to
/// [`crate::island_map::source_transfer_wave_opens_ground_kind`].
pub(crate) fn source_static_map_direction(
    definition: &CodBuilding,
    tile: IslandTile,
) -> Option<u8> {
    let kind = definition.source_kind_code()?;
    if kind == 19 {
        return None;
    }

    if crate::island_map::source_transfer_wave_opens_ground_kind(kind) {
        return Some(0x0c);
    }

    match kind {
        3 if tile.source_id()
            == definition.source_id + definition.size.0.saturating_mul(definition.size.1) / 2 =>
        {
            Some(0x0c)
        }
        _ if definition.no_shot
            && (kind != 10
                || (definition.anim_anz & !1)
                    <= (i32::from(tile.orientation >> 2 & 0x0f) << 1)) =>
        {
            Some(0x0d)
        }
        _ => Some(0x0c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_radius_profile_matches_fun_00404d70_initialization() {
        assert_eq!(source_radius_profile(0), vec![0]);
        assert_eq!(source_radius_profile(1), vec![1, 1]);
        assert_eq!(source_radius_profile(5), vec![5, 5, 5, 4, 3, 2]);
        assert_eq!(source_radius_profile(8), vec![8, 8, 8, 7, 7, 6, 5, 4, 2]);
    }

    #[test]
    fn source_radius_mask_blocks_corners_but_preserves_the_profile_edge() {
        let mut grid = SourcePathGrid::new((0, 0), 11, 11);
        for y in 0..11 {
            for x in 0..11 {
                assert!(grid.set_traversable_cell((x, y), 32));
            }
        }

        assert!(grid.block_outside_source_radius_mask(5, 1, 1));
        assert!(grid.route_to((5, 5), (3, 0)).is_ok());
        assert!(grid.route_to((5, 5), (2, 0)).is_err());
    }

    /// `FUN_004710b0` (`1602_exe.c:80043-80058`) writes `direction = 0` and
    /// `metadata = 0x28` over the requesting object's footprint. The goal bit
    /// the raster left there has to go, or the wave dies on its own building.
    #[test]
    fn opening_the_requesting_footprint_clears_its_goal_bit_and_pins_cost_0x28() {
        let mut grid = SourcePathGrid::new((0, 0), 4, 1);
        for x in 0..4 {
            // What `FUN_004706e0` writes for a cell that is both traversable
            // and a goal: `Wegspeed & 0x7f | goal << 7`.
            assert!(grid.set_traversable_cell((x, 0), 0x20 | 0x80));
        }

        assert!(grid.open_source_object_footprint(
            SourcePathTargetRect::new((0, 0), 2, 1).expect("2x1 footprint")
        ));

        assert_eq!(grid.metadata((0, 0)), Some(0x28));
        assert_eq!(grid.metadata((1, 0)), Some(0x28));
        assert_eq!(grid.metadata((2, 0)), Some(0xa0));
        assert_eq!(grid.metadata((3, 0)), Some(0xa0));
    }

    /// The behavioural half: a root whose own footprint is goal-marked cannot
    /// leave its anchor tile, because `FUN_0046c7d0`'s blocked-cell branch
    /// never expands. Reopening the footprint is what restores the route.
    #[test]
    fn a_goal_marked_footprint_walls_the_requesting_root_in_until_it_is_reopened() {
        let build = || {
            let mut grid = SourcePathGrid::new((0, 0), 4, 1);
            for x in 0..4 {
                grid.set_traversable_cell((x, 0), 0x20 | 0x80);
            }
            grid
        };
        let reached = |grid: &mut SourcePathGrid| {
            let mut seen = Vec::new();
            let _ = grid.search_with_blocked_cell_callback((0, 0), |position, _| {
                seen.push(position);
                SourcePathBlockedCellDecision::Block
            });
            seen
        };

        // Untouched, the wave stops on the root's own second tile and the
        // search ends there: `Block` does not expand a goal cell.
        assert_eq!(reached(&mut build()), vec![(1, 0)]);

        // Reopened, the same wave walks through the footprint and offers the
        // first cell outside it instead.
        let mut opened = build();
        opened.open_source_object_footprint(
            SourcePathTargetRect::new((0, 0), 2, 1).expect("2x1 footprint"),
        );
        assert_eq!(reached(&mut opened), vec![(2, 0)]);
    }

    #[test]
    fn high_metadata_selector_finishes_the_source_scan_band() {
        let mut grid = SourcePathGrid::new((0, 0), 5, 1);
        assert!(grid.set_metadata((1, 0), 0x80));
        assert!(grid.set_metadata((3, 0), 0x80));

        let result = grid.search_source_high_metadata_target((2, 0), 0).unwrap();

        // The source's stack scan visits the right candidate first, then
        // retains the left candidate as the final selection in that band.
        assert_eq!(result.position, (1, 0));
    }

    #[test]
    fn source_ship_route_window_exposes_the_two_caller_radii() {
        assert_eq!(SourceShipRouteWindow::Normal.radius(), 0x50);
        assert_eq!(SourceShipRouteWindow::ShortTargetRetry.radius(), 0x28);
        assert_eq!(
            SourceShipRouteWindow::default(),
            SourceShipRouteWindow::Normal
        );
    }

    #[test]
    fn coordinate_target_descriptor_matches_fun_00445400_and_fun_00444900() {
        let descriptor = SourceTargetDescriptor::from_world_coordinate(0x345, 0xabc).unwrap();
        assert_eq!(descriptor.bytes(), [0x37, 0xa3, 0x45, 0xbc]);
        assert_eq!(descriptor.kind(), 0x37);
        assert_eq!(descriptor.world_coordinate(), Some((0x345, 0xabc)));
        assert_eq!(
            descriptor.target_rect(),
            SourcePathTargetRect::new((0x345, 0xabc), 1, 1)
        );
        assert_eq!(descriptor.threshold_route_parameters(), Some((5, 0)));
        assert_eq!(
            descriptor.select_fun_00455a20_branch(2, None, None, 9),
            SourceShipTargetRouteBranch::Threshold {
                approach_radius: 5,
                limit: 0,
            }
        );
        assert!(SourceTargetDescriptor::from_world_coordinate(0x1000, 0).is_none());
        assert!(SourceTargetDescriptor::from_world_coordinate(-1, 0).is_none());
    }

    #[test]
    fn fixed_point_target_descriptor_matches_fun_004453d0_and_fun_00444af0() {
        let descriptor = SourceTargetDescriptor::from_bytes([0x38, 0x22, 0xec, 0x4c]);
        assert_eq!(
            descriptor.kind(),
            SourceTargetDescriptor::FIXED_POINT_COORDINATE_KIND
        );
        assert_eq!(descriptor.packed_coordinate(), Some((0x2ec, 0x24c)));
        assert_eq!(
            descriptor.source_land_route_coordinate(),
            Some((0x2ec, 0x24c))
        );
        assert_eq!(descriptor.world_coordinate(), None);
        assert_eq!(descriptor.target_rect(), None);
    }

    #[test]
    fn land_route_coordinate_matches_fun_00444af0_kind_scaling() {
        let descriptor =
            SourceTargetDescriptor::from_source_land_route_coordinate(0x68a, 0x1578).unwrap();
        assert_eq!(descriptor.bytes(), [0x37, 0xa3, 0x45, 0xbc]);
        assert_eq!(
            descriptor.source_land_route_coordinate(),
            Some((0x68a, 0x1578))
        );
        assert!(SourceTargetDescriptor::from_source_land_route_coordinate(1, 0).is_none());
    }

    #[test]
    fn kind34_island_cell_descriptor_matches_fun_004458f0() {
        assert_eq!(
            SourceTargetDescriptor::from_source_kind34_island_cell(10, 24, 21).bytes(),
            [0x34, 10, 24, 21]
        );
    }

    #[test]
    fn target_descriptor_branch_matches_fun_00455a20_owner_cases() {
        let island_target = SourceTargetDescriptor::from_bytes([0x32, 4, 9, 7]);
        assert_eq!(
            island_target.select_fun_00455a20_branch(2, Some(4), Some(1), 3),
            SourceShipTargetRouteBranch::Direct { approach_radius: 3 }
        );
        assert_eq!(
            island_target.select_fun_00455a20_branch(2, Some(4), Some(3), 3),
            SourceShipTargetRouteBranch::Threshold {
                approach_radius: 5,
                limit: 2,
            }
        );
        assert_eq!(
            island_target.select_fun_00455a20_branch(2, Some(2), Some(1), 3),
            SourceShipTargetRouteBranch::Threshold {
                approach_radius: 5,
                limit: 2,
            }
        );

        let unresolvable_target = SourceTargetDescriptor::from_bytes([0x39, 0, 0, 0]);
        assert_eq!(
            unresolvable_target.select_fun_00455a20_branch(2, None, None, 3),
            SourceShipTargetRouteBranch::Threshold {
                approach_radius: 5,
                limit: 0,
            }
        );
    }

    #[test]
    fn static_island_target_descriptor_resolves_source_footprint_and_owner() {
        let island = Island {
            number: 4,
            width: 32,
            height: 32,
            x_pos: 100,
            y_pos: 200,
            fertilities: [7; 8],
            tiles: vec![IslandTile {
                building_id: 3,
                x: 9,
                y: 7,
                orientation: 1,
                anim_count: 0x80,
                flags: 1,
            }],
            city: None,
        };
        let definitions = [CodBuilding {
            source_id: 0x4e23,
            size: (2, 4),
            ..Default::default()
        }];
        let expected = Some(SourceResolvedStaticTarget {
            target: SourcePathTargetRect::new((109, 207), 4, 2).unwrap(),
            owner: 6,
        });
        let expected_land = Some(SourceResolvedStaticTarget {
            target: SourcePathTargetRect::new((218, 414), 8, 4).unwrap(),
            owner: 6,
        });
        for kind in [0x32, 0x33, 0x34] {
            let descriptor = SourceTargetDescriptor::from_bytes([kind, 4, 9, 7]);
            assert_eq!(
                descriptor.resolve_static_island_target(&[island.clone()], &definitions),
                expected
            );
            assert_eq!(
                descriptor.resolve_static_island_land_target(&[island.clone()], &definitions),
                expected_land
            );
        }
        assert!(SourceTargetDescriptor::from_bytes([0x35, 4, 9, 7])
            .resolve_static_island_target(&[], &definitions)
            .is_none());
    }

    #[test]
    fn dynamic_map_object_target_descriptor_uses_live_slot_position_and_kind_footprint() {
        let island = Island {
            number: 4,
            width: 32,
            height: 32,
            x_pos: 100,
            y_pos: 200,
            fertilities: [7; 8],
            tiles: vec![IslandTile {
                building_id: 3,
                x: 9,
                y: 7,
                orientation: 1,
                anim_count: 0,
                flags: 0,
            }],
            city: None,
        };
        let definitions = [CodBuilding {
            source_id: 0x4e23,
            size: (2, 4),
            ..Default::default()
        }];
        let object = SourceDynamicMapObject {
            island: 4,
            slot: 6,
            owner: 2,
            local_position: (9, 7),
        };

        assert_eq!(
            SourceTargetDescriptor::from_bytes([0x35, 4, 6, 0]).resolve_dynamic_map_object_target(
                &[object],
                &[island.clone()],
                &definitions
            ),
            Some(SourceResolvedDynamicTarget {
                target: SourcePathTargetRect::new((109, 207), 4, 2).unwrap(),
                owner: 2,
            })
        );
        assert_eq!(
            SourceTargetDescriptor::from_bytes([0x36, 4, 6, 0]).resolve_dynamic_map_object_target(
                &[object],
                &[island],
                &[]
            ),
            Some(SourceResolvedDynamicTarget {
                target: SourcePathTargetRect::new((109, 207), 1, 1).unwrap(),
                owner: 2,
            })
        );
        assert!(SourceTargetDescriptor::from_bytes([0x35, 4, 7, 0])
            .resolve_dynamic_map_object_target(&[object], &[], &definitions)
            .is_none());
    }

    #[test]
    fn dynamic_map_object_table_reuses_the_first_released_source_slot() {
        let mut table = SourceDynamicMapObjectTable::new(4);
        let first = table.allocate(2, (9, 7)).unwrap();
        let second = table.allocate(3, (5, 6)).unwrap();

        assert_eq!(first.slot, 0);
        assert_eq!(second.slot, 1);
        assert_eq!(table.object(0), Some(first));
        assert_eq!(table.release(0), Some(first));

        let replacement = table.allocate(6, (11, 12)).unwrap();
        assert_eq!(replacement.slot, 0);
        assert_eq!(
            table.objects().collect::<Vec<_>>(),
            vec![replacement, second]
        );
    }

    #[test]
    fn dynamic_map_object_table_fills_all_eight_source_slots() {
        let mut table = SourceDynamicMapObjectTable::new(4);
        for slot in 0..SourceDynamicMapObjectTable::SLOT_COUNT {
            let object = table.allocate(2, (slot as u8, 0)).unwrap();
            assert_eq!(object.slot, slot as u8);
            assert_eq!(object.island, 4);
        }

        assert!(table.allocate(2, (9, 7)).is_none());
        assert_eq!(
            table.objects().count(),
            SourceDynamicMapObjectTable::SLOT_COUNT
        );
    }

    #[test]
    fn target_rectangle_helpers_match_source_callback_setup() {
        let target = SourcePathTargetRect::new((10, 20), 4, 3).unwrap();
        assert_eq!(target.nearest_point((8, 22)), (10, 22));
        assert_eq!(target.nearest_point((20, 30)), (13, 22));
        assert_eq!(target.center_point_toward((8, 20)), (11, 21));
        assert_eq!(target.center_point_toward((20, 30)), (12, 21));

        let mut grid = SourcePathGrid::new((9, 19), 5, 5);
        grid.set_metadata((10, 20), 0x2a);
        grid.mark_direction_blocker((10, 20));
        assert!(grid.mark_target_region(target));
        assert_eq!(grid.metadata((10, 20)), Some(0xaa));
        assert_eq!(grid.is_direction_clear((10, 20)), Some(true));
        assert_eq!(grid.metadata((13, 22)), Some(0x80));
        assert_eq!(grid.metadata((9, 19)), Some(0));
    }

    #[test]
    fn target_region_metadata_covers_the_entire_clipped_footprint() {
        let mut grid = SourcePathGrid::new((9, 19), 5, 5);
        let target = SourcePathTargetRect::new((8, 20), 3, 2).unwrap();

        assert!(grid.set_target_region_metadata(target, 0xa6));
        assert_eq!(grid.metadata((9, 20)), Some(0xa6));
        assert_eq!(grid.metadata((10, 20)), Some(0xa6));
        assert_eq!(grid.metadata((9, 21)), Some(0xa6));
        assert_eq!(grid.metadata((10, 21)), Some(0xa6));
        assert_eq!(grid.metadata((11, 20)), Some(0));
    }

    #[test]
    fn target_region_callback_completes_on_first_reached_target_cell() {
        let mut grid = SourcePathGrid::new((0, 0), 5, 1);
        let target = SourcePathTargetRect::new((2, 0), 2, 1).unwrap();

        let steps = grid.route_to_target_region((0, 0), target).unwrap();
        assert_eq!(
            steps.iter().map(|step| step.direction).collect::<Vec<_>>(),
            vec![3, 3]
        );
        assert_eq!(grid.metadata((2, 0)), Some(0x80));
        assert_eq!(grid.metadata((3, 0)), Some(0x80));
    }

    #[test]
    fn direct_target_callback_completes_on_the_first_approach_ray_cell() {
        let target = SourcePathTargetRect::new((5, 1), 1, 1).unwrap();

        let mut no_approach = SourcePathGrid::new((0, 0), 7, 3);
        let direct_steps = no_approach
            .route_to_direct_target((0, 1), target, 0)
            .unwrap();
        assert_eq!(direct_steps.len(), 5);

        let mut with_approach = SourcePathGrid::new((0, 0), 7, 3);
        let approach_steps = with_approach
            .route_to_direct_target((0, 1), target, 2)
            .unwrap();
        assert_eq!(approach_steps.len(), 3);
        assert_eq!(with_approach.metadata((3, 1)), Some(0x80));
    }

    #[test]
    fn threshold_target_search_uses_whole_rays_and_the_center_callback() {
        let target = SourcePathTargetRect::new((5, 1), 1, 1).unwrap();

        let mut grid = SourcePathGrid::new((0, 0), 7, 3);
        let result = grid.search_threshold_target((0, 1), target, 2, 2).unwrap();
        assert_eq!(result.position, (3, 1));
        assert_eq!(result.steps.len(), 3);
        assert_eq!(grid.metadata((5, 1)), Some(0x80));

        let mut target_only = SourcePathGrid::new((0, 0), 7, 3);
        let result = target_only
            .search_threshold_target((0, 1), target, 2, 0)
            .unwrap();
        assert_eq!(result.position, (5, 1));
        assert_eq!(result.steps.len(), 5);
    }

    #[test]
    fn nearest_reached_marker_matches_fun_0046d1e0_boundary_scan() {
        let mut grid = SourcePathGrid::new((10, 20), 5, 4);
        for (position, direction) in [
            ((14, 20), 3),
            ((14, 23), 3),
            ((12, 20), 3),
            ((12, 21), 3),
            ((11, 21), 0x0b),
        ] {
            let index = grid.index(position).unwrap();
            grid.cells[index].direction = direction;
        }

        // The vertical boundary scan precedes the horizontal scan, so the
        // equal-cost top-right marker wins the source's strict comparison.
        assert_eq!(grid.nearest_reached_marker((20, 21), 100), Some((14, 20)));
        assert_eq!(grid.nearest_reached_marker((12, 21), 100), None);
        assert_eq!(grid.nearest_reached_marker((12, 15), 100), Some((12, 20)));
    }

    #[test]
    fn reached_marker_trace_requires_a_source_predecessor_direction() {
        let mut grid = SourcePathGrid::new((0, 0), 3, 1);
        for position in [(1, 0), (2, 0)] {
            let index = grid.index(position).unwrap();
            grid.cells[index].direction = 3;
        }

        assert_eq!(
            grid.steps_to_reached_marker((0, 0), (2, 0)),
            Some(vec![
                SourceRouteStep {
                    direction: 3,
                    metadata: 0,
                },
                SourceRouteStep {
                    direction: 3,
                    metadata: 0,
                },
            ])
        );
        assert_eq!(grid.steps_to_reached_marker((0, 0), (0, 0)), None);
    }

    #[test]
    fn target_callbacks_match_lab_0046c670_and_lab_0046c750() {
        let mut direct = SourcePathTargetCallbackState::new((10, 20), 999);
        assert_eq!(
            SourcePathTargetCallback::Direct.decide((13, 24), 0, &mut direct),
            SourcePathBlockedCellDecision::Complete
        );
        assert_eq!(direct.candidate, (13, 24));
        assert_eq!(direct.best_distance, 4);

        let mut threshold = SourcePathTargetCallbackState::new((50, 10), 60);
        assert_eq!(
            SourcePathTargetCallback::Threshold { limit: 3 }.decide((99, 20), 0, &mut threshold),
            SourcePathBlockedCellDecision::Expand
        );
        assert_eq!(threshold.candidate, (99, 20));
        assert_eq!(threshold.best_distance, 51);
        assert_eq!(
            SourcePathTargetCallback::Threshold { limit: 3 }.decide((48, 11), 48, &mut threshold),
            SourcePathBlockedCellDecision::Complete
        );
        assert_eq!(threshold.candidate, (48, 11));
        assert_eq!(threshold.best_distance, 2);
    }

    #[test]
    fn target_approach_rays_preserve_the_two_source_raster_policies() {
        let target = SourcePathTargetRect::new((3, 3), 1, 1).unwrap();

        let mut stop_at_direction_13 = SourcePathGrid::new((0, 0), 7, 7);
        let stop_index = stop_at_direction_13.index((2, 3)).unwrap();
        stop_at_direction_13.cells[stop_index].direction = 0x0d;
        stop_at_direction_13.mark_target_approach_rays(
            target,
            1,
            SourceTargetApproachRayMode::StopAtDirection13,
        );
        assert_eq!(stop_at_direction_13.metadata((3, 3)), Some(0));
        assert_eq!(stop_at_direction_13.metadata((2, 3)), Some(0));
        assert_eq!(stop_at_direction_13.metadata((3, 2)), Some(0x80));

        let mut mark_whole_ray = SourcePathGrid::new((0, 0), 7, 7);
        let mark_index = mark_whole_ray.index((2, 3)).unwrap();
        mark_whole_ray.cells[mark_index].direction = 0x0d;
        mark_whole_ray.mark_target_approach_rays(
            target,
            1,
            SourceTargetApproachRayMode::MarkWholeRay,
        );
        assert_eq!(mark_whole_ray.metadata((3, 3)), Some(0x80));
        assert_eq!(mark_whole_ray.metadata((2, 3)), Some(0x80));
    }

    #[test]
    fn target_approach_rays_clip_against_the_path_grid() {
        let mut grid = SourcePathGrid::new((0, 0), 4, 4);
        let target = SourcePathTargetRect::new((0, 0), 1, 1).unwrap();

        grid.mark_target_approach_rays(target, 5, SourceTargetApproachRayMode::MarkWholeRay);

        assert_eq!(grid.metadata((0, 0)), Some(0x80));
        assert_eq!(grid.metadata((1, 0)), Some(0x80));
        assert_eq!(grid.metadata((0, 1)), Some(0x80));
        assert_eq!(grid.metadata((3, 3)), Some(0x80));
    }

    #[test]
    fn direction_13_ray_matches_fun_0046dd30_and_fun_0046e8b0() {
        let mut grid = SourcePathGrid::new((0, 0), 7, 3);
        assert!(grid.mark_direction_13_segment((3, 2), (3, 2)));
        assert!(grid.mark_direction_13_segment((6, 2), (6, 2)));

        // The source direct-ray metric uses ceil(minor / 4), unlike the
        // candidate producer's floor-based metric.
        assert!(!grid.direction_13_ray_clear((0, 1), (4, 2), 4));
        assert!(!grid.direction_13_ray_clear((0, 1), (4, 2), 5));

        // An intermediate direction-13 cell rejects the segment, whereas a
        // marker exactly at the selected endpoint is allowed.
        assert!(grid.direction_13_ray_clear((4, 2), (6, 2), 2));
        assert!(grid.direction_13_ray_clear((0, 0), (6, 2), 7));
        assert!(!grid.direction_13_ray_clear((-1, 0), (1, 0), 2));
    }

    #[test]
    fn source_candidate_footprint_matches_fun_0046d9d0_wide_branches() {
        let mut grid = SourcePathGrid::new((0, 0), 7, 7);
        assert!(grid.overlay_source_candidate_footprint(1, (3, 3), 2));
        for x in 1..=5 {
            assert_eq!(grid.cells[grid.index((x, 3)).unwrap()].direction, 0x0d);
        }

        let mut edge = SourcePathGrid::new((0, 0), 5, 5);
        assert!(edge.overlay_source_candidate_footprint(3, (0, 1), 1));
        assert_eq!(edge.cells[edge.index((1, 0)).unwrap()].direction, 0x0d);
        assert_eq!(edge.cells[edge.index((0, 1)).unwrap()].direction, 0x0d);

        let mut corner = SourcePathGrid::new((0, 0), 5, 5);
        assert!(!corner.overlay_source_candidate_footprint(3, (0, 0), 1));

        assert!(!edge.overlay_source_candidate_footprint(4, (2, 2), 2));
        assert!(!edge.overlay_source_candidate_footprint(1, (2, 2), 8));
    }

    #[test]
    fn source_directions_match_fun_00456270() {
        assert_eq!(source_direction_delta(1), Some((0, -1)));
        assert_eq!(source_direction_delta(2), Some((1, -1)));
        assert_eq!(source_direction_delta(3), Some((1, 0)));
        assert_eq!(source_direction_delta(4), Some((1, 1)));
        assert_eq!(source_direction_delta(5), Some((0, 1)));
        assert_eq!(source_direction_delta(6), Some((-1, 1)));
        assert_eq!(source_direction_delta(7), Some((-1, 0)));
        assert_eq!(source_direction_delta(8), Some((-1, -1)));
        assert_eq!(source_direction_delta(0), None);
    }

    #[test]
    fn source_target_direction_matches_fun_00454050_axes_diagonals_and_ties() {
        assert_eq!(source_target_direction(0, -1), 0);
        assert_eq!(source_target_direction(1, -1), 1);
        assert_eq!(source_target_direction(1, 0), 2);
        assert_eq!(source_target_direction(1, 1), 3);
        assert_eq!(source_target_direction(0, 1), 4);
        assert_eq!(source_target_direction(-1, 1), 5);
        assert_eq!(source_target_direction(-1, 0), 6);
        assert_eq!(source_target_direction(-1, -1), 7);

        // Source comparisons are strict: 2:1 is axial, while the matching
        // diagonal boundary remains diagonal. Even zero follows the final
        // south-east return in the original branch structure.
        assert_eq!(source_target_direction(2, -1), 2);
        assert_eq!(source_target_direction(1, -2), 1);
        assert_eq!(source_target_direction(1, -3), 0);
        assert_eq!(source_target_direction(0, 0), 3);
    }

    #[test]
    fn source_kind6_target_direction_clamps_raw_footprint_and_applies_quarter_radius() {
        let target = SourcePathTargetRect::new((10, 10), 4, 2).unwrap();

        // The nearest raw point is (10, 11), four cells east. Runtime shot
        // radius 16 permits metric four; radius 12 permits only three.
        assert_eq!(source_kind6_target_direction((6, 11), target, 16), Some(2));
        assert_eq!(source_kind6_target_direction((6, 11), target, 12), None);

        // A position inside the footprint clamps to itself; `FUN_00454050`
        // returns its source zero-delta direction value.
        assert_eq!(source_kind6_target_direction((11, 11), target, 0), Some(3));
    }

    #[test]
    fn source_route_positions_expand_predecessor_directions() {
        let steps = [
            SourceRouteStep {
                direction: 3,
                metadata: 32,
            },
            SourceRouteStep {
                direction: 1,
                metadata: 32,
            },
        ];
        assert_eq!(
            source_route_positions((10, 20), &steps),
            Some(vec![(11, 20), (11, 19)])
        );
        assert_eq!(
            source_route_positions(
                (0, 0),
                &[SourceRouteStep {
                    direction: 0,
                    metadata: 0,
                }]
            ),
            None
        );
    }

    #[test]
    fn path_cost_tables_match_fun_0046f8a0() {
        for metadata in [0, 31] {
            assert_eq!(source_cardinal_cost(metadata), 0x40);
            assert_eq!(source_diagonal_cost(metadata), 0x5b);
        }
        assert_eq!(source_cardinal_cost(32), 64);
        assert_eq!(source_cardinal_cost(33), 66);
        assert_eq!(source_cardinal_cost(126), 252);
        assert_eq!(source_diagonal_cost(32), 91);
        assert_eq!(source_diagonal_cost(33), 93);
        assert_eq!(source_diagonal_cost(34), 96);
        assert_eq!(source_diagonal_cost(126), 358);
        assert_eq!(source_cardinal_cost(127), 504);
        assert_eq!(source_diagonal_cost(127), 716);
        // The source indexes both tables with the low seven metadata bits.
        assert_eq!(source_cardinal_cost(0xa1), source_cardinal_cost(0x21));
        assert_eq!(source_diagonal_cost(0xff), source_diagonal_cost(0x7f));
    }

    #[test]
    fn source_route_round_trips_kind_one_three_program() {
        let steps = [
            SourceRouteStep {
                direction: 3,
                metadata: 7,
            },
            SourceRouteStep {
                direction: 3,
                metadata: 7,
            },
            SourceRouteStep {
                direction: 3,
                metadata: 7,
            },
            SourceRouteStep {
                direction: 5,
                metadata: 12,
            },
            SourceRouteStep {
                direction: 5,
                metadata: 12,
            },
            SourceRouteStep {
                direction: 1,
                metadata: 1,
            },
        ];

        let program = encode_source_route(
            &steps,
            SOURCE_SHIP_ROUTE_RUN_LIMIT,
            SOURCE_SHIP_ROUTE_CAPACITY,
        )
        .unwrap();
        assert_eq!(program, [0x32, 0x31, 0x52, 0x11, 0xc1]);
        assert_eq!(
            decode_source_route(&program).unwrap(),
            vec![3, 3, 3, 5, 5, 1]
        );
    }

    #[test]
    fn metadata_prevents_source_run_coalescing() {
        let steps = [
            SourceRouteStep {
                direction: 2,
                metadata: 0x81,
            },
            SourceRouteStep {
                direction: 2,
                metadata: 1,
            },
            SourceRouteStep {
                direction: 2,
                metadata: 2,
            },
        ];

        let program = encode_source_route(&steps, 15, 100).unwrap();
        assert_eq!(program, [0x22, 0x21, 0xc1]);
    }

    #[test]
    fn plantation_route_merges_matching_directions_across_metadata_classes() {
        let steps = [
            SourceRouteStep {
                direction: 2,
                metadata: 0x81,
            },
            SourceRouteStep {
                direction: 2,
                metadata: 1,
            },
            SourceRouteStep {
                direction: 2,
                metadata: 2,
            },
        ];

        assert_eq!(
            encode_source_direction_route_truncated(&steps, 15, 12),
            Ok(vec![0x23, SOURCE_ROUTE_TERMINATOR])
        );
    }

    #[test]
    fn bounded_source_route_keeps_the_terminator_after_the_last_fitting_run() {
        let steps = [
            SourceRouteStep {
                direction: 1,
                metadata: 1,
            },
            SourceRouteStep {
                direction: 2,
                metadata: 1,
            },
            SourceRouteStep {
                direction: 3,
                metadata: 1,
            },
        ];
        assert_eq!(
            encode_source_route_truncated(&steps, 15, 3),
            Ok(vec![0x11, 0x21, SOURCE_ROUTE_TERMINATOR])
        );
    }

    #[test]
    fn malformed_programs_do_not_decode_as_motion() {
        assert_eq!(
            decode_source_route(&[0x20, SOURCE_ROUTE_TERMINATOR]),
            Err(SourceRouteError::InvalidInstruction(0x20))
        );
        assert_eq!(
            decode_source_route(&[0xd1]),
            Err(SourceRouteError::InvalidInstruction(0xd1))
        );
        assert_eq!(decode_source_route(&[0xc2]), Ok(Vec::new()));
        assert_eq!(
            decode_source_route(&[0x11]),
            Err(SourceRouteError::MissingTerminator)
        );
    }

    #[test]
    fn fixed_cost_wave_search_writes_source_predecessor_route() {
        let mut grid = SourcePathGrid::new((10, 20), 3, 1);
        grid.set_metadata((10, 20), 7);
        grid.set_metadata((11, 20), 19);
        grid.set_metadata((12, 20), 31);

        let steps = grid.route_to((10, 20), (12, 20)).unwrap();
        assert_eq!(
            steps,
            vec![
                SourceRouteStep {
                    direction: 3,
                    metadata: 7,
                },
                SourceRouteStep {
                    direction: 3,
                    metadata: 19,
                },
            ]
        );
        assert_eq!(
            encode_source_route(
                &steps,
                SOURCE_SHIP_ROUTE_RUN_LIMIT,
                SOURCE_SHIP_ROUTE_CAPACITY
            ),
            Ok(vec![0x31, 0x31, SOURCE_ROUTE_TERMINATOR])
        );
        assert_eq!(grid.metadata((12, 20)), Some(31));
    }

    #[test]
    fn fixed_cost_wave_search_respects_source_direction_blockers() {
        let mut grid = SourcePathGrid::new((0, 0), 3, 3);
        assert!(grid.mark_direction_blocker((1, 0)));

        let steps = grid.route_to((0, 0), (2, 0)).unwrap();
        let mut position = (0, 0);
        for step in steps {
            let (dx, dy) = source_direction_delta(step.direction).unwrap();
            position = (position.0 + dx, position.1 + dy);
            assert_ne!(position, (1, 0));
        }
        assert_eq!(position, (2, 0));
    }

    #[test]
    fn fixed_cost_wave_search_rejects_metadata_blocked_corridor() {
        let mut grid = SourcePathGrid::new((0, 0), 3, 1);
        grid.set_metadata((1, 0), 0x80);

        assert_eq!(
            grid.route_to((0, 0), (2, 0)),
            Err(SourcePathSearchError::NoRoute)
        );
    }

    #[test]
    fn source_wave_calls_blocked_cells_in_reverse_frontier_order() {
        let mut grid = SourcePathGrid::new((0, 0), 3, 1);
        assert!(grid.set_metadata((0, 0), 0x80));
        assert!(grid.set_metadata((2, 0), 0x80));
        let mut visits = Vec::new();

        assert_eq!(
            grid.search_with_blocked_cell_callback((1, 0), |position, elapsed_cost| {
                visits.push((position, elapsed_cost));
                SourcePathBlockedCellDecision::Block
            }),
            Err(SourcePathSearchError::NoRoute)
        );
        assert_eq!(visits, [((2, 0), 0x40), ((0, 0), 0x40)]);
    }

    #[test]
    fn source_wave_completion_uses_high_metadata_callback() {
        let mut grid = SourcePathGrid::new((0, 0), 3, 1);
        assert!(grid.set_metadata((2, 0), 0x80));

        let result = grid
            .search_with_blocked_cell_callback((0, 0), |position, elapsed_cost| {
                assert_eq!(position, (2, 0));
                assert_eq!(elapsed_cost, 0x80);
                SourcePathBlockedCellDecision::Complete
            })
            .unwrap();

        assert_eq!(result.position, (2, 0));
        assert_eq!(result.elapsed_cost, 0x80);
        assert_eq!(
            result.steps,
            [
                SourceRouteStep {
                    direction: 3,
                    metadata: 0,
                },
                SourceRouteStep {
                    direction: 3,
                    metadata: 0,
                },
            ]
        );
    }

    #[test]
    fn source_wave_advance_frontier_skips_current_cells_but_keeps_queued_work() {
        let mut grid = SourcePathGrid::new((0, 0), 4, 3);
        assert!(grid.set_metadata((1, 0), 0x80));
        assert!(grid.set_metadata((3, 0), 0x80));
        let mut visits = Vec::new();

        let result = grid
            .search_with_blocked_cell_callback((1, 1), |position, _| {
                visits.push(position);
                if position == (1, 0) {
                    SourcePathBlockedCellDecision::AdvanceFrontier
                } else {
                    SourcePathBlockedCellDecision::Complete
                }
            })
            .unwrap();

        assert_eq!(visits, [(1, 0), (3, 0)]);
        assert_eq!(result.position, (3, 0));
    }

    #[test]
    fn fun_0046c7d0_allows_diagonal_past_high_metadata_corners() {
        let mut grid = SourcePathGrid::new((0, 0), 2, 2);
        assert!(grid.set_metadata((1, 0), 0x80));
        assert!(grid.set_metadata((0, 1), 0x80));

        assert_eq!(
            grid.route_to((0, 0), (1, 1)),
            Ok(vec![SourceRouteStep {
                direction: 4,
                metadata: 0,
            }])
        );
    }

    #[test]
    fn static_island_overlay_uses_source_ids_rotated_footprints_and_classes() {
        let island = Island {
            number: 0,
            width: 3,
            height: 2,
            x_pos: 10,
            y_pos: 20,
            fertilities: [7; 8],
            tiles: vec![
                IslandTile {
                    building_id: 1,
                    x: 0,
                    y: 0,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                },
                IslandTile {
                    building_id: 2,
                    x: 1,
                    y: 0,
                    orientation: 1,
                    anim_count: 0x80,
                    flags: 0,
                },
            ],
            city: None,
        };
        let mut ground = CodBuilding {
            source_id: 0x4e21,
            kind: "BODEN".to_string(),
            ..Default::default()
        };
        ground
            .properties
            .insert("Wegspeed".to_string(), "100,100,100,100".to_string());
        let mut eligible_building = CodBuilding {
            source_id: 0x4e22,
            kind: "HANDWERK".to_string(),
            size: (2, 1),
            ..Default::default()
        };
        eligible_building
            .properties
            .insert("Wegspeed".to_string(), "50,50,50,50".to_string());
        let mut permissions = [0_u8; 38];
        permissions[1] = 1;

        let mut grid = SourcePathGrid::new((10, 20), 3, 2);
        grid.populate_static_island_cells(
            &island,
            &[ground, eligible_building],
            3,
            |definition, tile| source_path_eligible(definition, &permissions, 2, tile),
        )
        .unwrap();

        assert_eq!(grid.metadata((10, 20)), Some(32));
        assert_eq!(grid.metadata((11, 20)), Some(0x90));
        assert_eq!(grid.metadata((11, 21)), Some(0x90));
        assert_eq!(grid.metadata((12, 20)), Some(0));
        assert_eq!(grid.cells[0].direction, 0);
        assert_eq!(grid.cells[1].direction, 0);
        assert_eq!(grid.cells[2].direction, 0x0c);
    }

    #[test]
    fn static_island_overlay_rejects_missing_movement_class() {
        let island = Island {
            number: 0,
            width: 1,
            height: 1,
            x_pos: 0,
            y_pos: 0,
            fertilities: [7; 8],
            tiles: Vec::new(),
            city: None,
        };
        let mut grid = SourcePathGrid::new((0, 0), 1, 1);

        assert_eq!(
            grid.populate_static_island_cells(&island, &[], 4, |_, _| false),
            Err(SourcePathGridBuildError::InvalidMovementType(4))
        );
    }

    #[test]
    fn ship_route_overlay_matches_fun_0046f6d0_markers() {
        let island = Island {
            number: 0,
            width: 5,
            height: 1,
            x_pos: 10,
            y_pos: 20,
            fertilities: [7; 8],
            tiles: vec![
                IslandTile {
                    building_id: 0,
                    x: 0,
                    y: 0,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                },
                IslandTile {
                    building_id: 1,
                    x: 1,
                    y: 0,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                },
                IslandTile {
                    building_id: 2,
                    x: 2,
                    y: 0,
                    orientation: 0b0000_1000,
                    anim_count: 0,
                    flags: 0,
                },
                IslandTile {
                    building_id: 3,
                    x: 3,
                    y: 0,
                    orientation: 0b0000_0100,
                    anim_count: 0,
                    flags: 0,
                },
                IslandTile {
                    building_id: 4,
                    x: 4,
                    y: 0,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                },
            ],
            city: None,
        };
        let definitions = [
            CodBuilding {
                source_id: 0x4e20,
                kind: "MEER".to_string(),
                ..Default::default()
            },
            CodBuilding {
                source_id: 0x4e21,
                kind: "BODEN".to_string(),
                ..Default::default()
            },
            CodBuilding {
                source_id: 0x4e22,
                kind: "WALD".to_string(),
                no_shot: true,
                anim_anz: 4,
                ..Default::default()
            },
            CodBuilding {
                source_id: 0x4e23,
                kind: "WALD".to_string(),
                no_shot: true,
                anim_anz: 4,
                ..Default::default()
            },
            CodBuilding {
                source_id: 0x4e24,
                kind: "TOR".to_string(),
                size: (3, 3),
                ..Default::default()
            },
        ];
        let mut grid = SourcePathGrid::new((10, 20), 5, 1);

        grid.overlay_source_ship_route_blockers(&island, &definitions);

        assert_eq!(grid.cells[0].direction, 0, "MEER stays passable");
        assert_eq!(grid.cells[1].direction, 0x0c, "fixed kind marker");
        assert_eq!(grid.cells[2].direction, 0x0d, "variant reaches AnimAnz");
        assert_eq!(grid.cells[3].direction, 0x0c, "variant below AnimAnz");
        assert_eq!(grid.cells[4].direction, 0x0c, "non-center kind three cell");

        let central_gate = CodBuilding {
            source_id: 0x4e30,
            kind: "TOR".to_string(),
            size: (3, 3),
            ..Default::default()
        };
        let center_tile = IslandTile {
            building_id: 0x10 + 4,
            x: 0,
            y: 0,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        };
        assert_eq!(
            source_static_map_direction(&central_gate, center_tile),
            Some(0x0c)
        );
    }

    #[test]
    fn source_window_keeps_static_markers_inside_the_callers_square() {
        let mut grid = SourcePathGrid::new((0, 0), 5, 1);
        grid.mark_direction_blocker((0, 0));
        grid.mark_direction_blocker((4, 0));

        let window = grid.source_window((2, 0), 1).unwrap();

        assert_eq!(window.is_direction_clear((1, 0)), Some(true));
        assert_eq!(window.is_direction_clear((2, 0)), Some(true));
        assert_eq!(window.is_direction_clear((3, 0)), Some(true));
        assert_eq!(window.is_direction_clear((0, 0)), None);
        assert_eq!(window.is_direction_clear((4, 0)), None);
    }
}
