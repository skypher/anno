//! Unit formations.
//!
//! RE source: `figuren.cod` `Objekt: FORMATION` block at the head of
//! the file. Three named formations (`FORM_HORI`, `FORM_VERT`,
//! `FORM_QUAD`) with up to 21 indexed offsets each. The original
//! game arranges a selected group of military units into one of
//! these patterns; offset 0 sits on the player's clicked tile and
//! each subsequent unit takes the next-numbered offset.
//!
//! Binary cite: `1602_exe.c:44503` registers the keyword `FORMATION`,
//! `:44537` enumerates `&PTR_s_FORM_HORI_00498ee4 .. 0x498ef0`
//! (three string-constant slots = three formation types).
//!
//! Offsets below transcribed verbatim from `figuren.cod` so the
//! shapes match the original.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formation {
    /// Horizontal line — file fans left/right with two flanking
    /// rows behind/ahead.
    Hori,
    /// Vertical line — column with two flanking files.
    Vert,
    /// 5x5 square block centred on the leader.
    Quad,
}

/// `FORM_HORI` offsets (`figuren.cod` Nummer:FORM_HORI):
const HORI: &[(i32, i32)] = &[
    ( 0,  0), (-1,  0), ( 1,  0), (-2,  0), ( 2,  0), (-3,  0), ( 3,  0),
    ( 0,  1), (-1,  1), ( 1,  1), (-2,  1), ( 2,  1), (-3,  1), ( 3,  1),
    ( 0, -1), (-1, -1), ( 1, -1), (-2, -1), ( 2, -1), (-3, -1), ( 3, -1),
];

/// `FORM_VERT` offsets:
const VERT: &[(i32, i32)] = &[
    ( 0,  0), ( 0, -1), ( 0,  1), ( 0, -2), ( 0,  2), ( 0, -3), ( 0,  3),
    ( 1,  0), ( 1, -1), ( 1,  1), ( 1, -2), ( 1,  2), ( 1, -3), ( 1,  3),
    (-1,  0), (-1, -1), (-1,  1), (-1, -2), (-1,  2), (-1, -3), (-1,  3),
];

/// `FORM_QUAD` offsets:
const QUAD: &[(i32, i32)] = &[
    ( 0,  0), (-1,  0), ( 1,  0), ( 0, -1), ( 0,  1),
    (-1, -1), ( 1, -1), ( 1,  1), (-1,  1),
    (-2, -1), (-2,  0), (-2,  1),
    (-1, -2), ( 0, -2), ( 1, -2),
    ( 2, -1), ( 2,  0), ( 2,  1),
    (-1,  2), ( 0,  2), ( 1,  2),
];

impl Formation {
    pub fn offsets(self) -> &'static [(i32, i32)] {
        match self {
            Formation::Hori => HORI,
            Formation::Vert => VERT,
            Formation::Quad => QUAD,
        }
    }

    /// Resolve the destination tile for the `i`th unit in the
    /// formation, anchored at `(anchor_x, anchor_y)`. Units past the
    /// formation's offset count clamp to the last entry rather than
    /// piling on the leader.
    pub fn place(self, i: usize, anchor_x: i32, anchor_y: i32) -> (i32, i32) {
        let offs = self.offsets();
        let last = offs.len().saturating_sub(1);
        let (dx, dy) = offs[i.min(last)];
        (anchor_x + dx, anchor_y + dy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hori_has_21_offsets_and_anchor_first() {
        assert_eq!(Formation::Hori.offsets().len(), 21);
        assert_eq!(Formation::Hori.place(0, 50, 50), (50, 50));
    }

    #[test]
    fn vert_anchor_then_steps_north() {
        let f = Formation::Vert;
        assert_eq!(f.place(0, 10, 10), (10, 10));
        assert_eq!(f.place(1, 10, 10), (10, 9));
        assert_eq!(f.place(2, 10, 10), (10, 11));
    }

    #[test]
    fn quad_clamps_past_capacity() {
        let f = Formation::Quad;
        let last = f.offsets().last().copied().unwrap();
        assert_eq!(f.place(99, 0, 0), last);
    }
}
