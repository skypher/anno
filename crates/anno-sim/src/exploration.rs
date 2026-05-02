//! Per-island exploration / fog-of-war bitmap.
//!
//! Tracks which tiles the human player has revealed via line-of-sight from
//! their buildings, ships, and military units. Bits only flip true; once
//! explored, a tile stays known.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExplorationMap {
    pub island_id: u8,
    pub width: u16,
    pub height: u16,
    explored: Vec<bool>,
}

impl ExplorationMap {
    pub fn new(island_id: u8, width: u16, height: u16) -> Self {
        let size = width as usize * height as usize;
        Self {
            island_id,
            width,
            height,
            explored: vec![false; size],
        }
    }

    pub fn is_explored(&self, x: u16, y: u16) -> bool {
        if x >= self.width || y >= self.height { return false; }
        self.explored[y as usize * self.width as usize + x as usize]
    }

    /// Reveal a square of side `2r+1` centered on `(cx, cy)`.
    pub fn mark_radius(&mut self, cx: i32, cy: i32, r: i32) {
        let xmin = (cx - r).max(0);
        let ymin = (cy - r).max(0);
        let xmax = (cx + r).min(self.width as i32 - 1);
        let ymax = (cy + r).min(self.height as i32 - 1);
        for y in ymin..=ymax {
            for x in xmin..=xmax {
                let idx = y as usize * self.width as usize + x as usize;
                if idx < self.explored.len() {
                    self.explored[idx] = true;
                }
            }
        }
    }

    /// Fraction of the map explored, 0..=128 scale.
    pub fn coverage_128(&self) -> u8 {
        if self.explored.is_empty() { return 0; }
        let n = self.explored.iter().filter(|&&v| v).count() as u64;
        ((n * 128) / self.explored.len() as u64).min(128) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_radius_reveals_square() {
        let mut m = ExplorationMap::new(0, 10, 10);
        m.mark_radius(5, 5, 1);
        for y in 4..=6 {
            for x in 4..=6 {
                assert!(m.is_explored(x, y));
            }
        }
        assert!(!m.is_explored(7, 7));
    }

    #[test]
    fn mark_radius_clamps_at_edges() {
        let mut m = ExplorationMap::new(0, 5, 5);
        m.mark_radius(0, 0, 10);
        // Should mark all tiles, not crash.
        for y in 0..5 {
            for x in 0..5 {
                assert!(m.is_explored(x, y));
            }
        }
        assert_eq!(m.coverage_128(), 128);
    }

    #[test]
    fn explored_bits_only_flip_true() {
        let mut m = ExplorationMap::new(0, 5, 5);
        m.mark_radius(2, 2, 0);
        assert!(m.is_explored(2, 2));
        // No "unmark" API — once revealed, stays revealed.
        let cov_before = m.coverage_128();
        m.mark_radius(0, 0, 0);
        assert!(m.coverage_128() >= cov_before);
    }
}
