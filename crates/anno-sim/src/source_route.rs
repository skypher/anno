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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
/// high metadata bit set. A nonzero callback return expands that cell; the
/// caller separately clears a grid-state flag to stop the search. `Complete`
/// represents that paired callback-and-stop action without exposing the
/// executable's mutable grid header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePathBlockedCellDecision {
    Block,
    Expand,
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

    /// Read a cell's source path metadata.
    pub fn metadata(&self, position: (i32, i32)) -> Option<u8> {
        self.index(position).map(|index| self.cells[index].metadata)
    }

    /// Mark a cell with source direction marker `0xc`, as
    /// `FUN_0046f6d0`/`FUN_0046d900` do for path blockers.
    pub fn mark_direction_blocker(&mut self, position: (i32, i32)) -> bool {
        let Some(index) = self.index(position) else {
            return false;
        };
        self.cells[index].direction = 0x0c;
        true
    }

    /// Return whether a cell has no source direction marker. A nonzero marker
    /// is impassable to `FUN_0046c7d0`.
    pub fn is_direction_clear(&self, position: (i32, i32)) -> Option<bool> {
        self.index(position)
            .map(|index| self.cells[index].direction == 0)
    }

    /// Construct the centered `2r + 1` square initialized by
    /// `FUN_0046c630` in the ship-route callers. Cells outside this backing
    /// static grid retain the source constructor's zero direction/metadata.
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
            let Some(direction) = source_ship_route_direction(definition, tile) else {
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

fn source_footprint_size(size: (i32, i32), orientation: u8) -> (u8, u8) {
    let width = u8::try_from(size.0.max(0)).unwrap_or(u8::MAX);
    let height = u8::try_from(size.1.max(0)).unwrap_or(u8::MAX);
    if orientation & 1 == 0 {
        (width, height)
    } else {
        (height, width)
    }
}

fn source_fixed_path_kind(definition: &CodBuilding) -> bool {
    matches!(
        definition.source_kind_code(),
        Some(1 | 11 | 12 | 13 | 18 | 29 | 30)
    )
}

/// Return the source direction marker that `FUN_0046f6d0` writes for one
/// static map cell. `None` is the explicit `MEER`/`KIRCHE` pass-through.
fn source_ship_route_direction(definition: &CodBuilding, tile: IslandTile) -> Option<u8> {
    let kind = definition.source_kind_code()?;
    if kind == 19 {
        return None;
    }

    match kind {
        1 | 11 | 12 | 13 | 18 | 29 | 30 => Some(0x0c),
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
            source_ship_route_direction(&central_gate, center_tile),
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
