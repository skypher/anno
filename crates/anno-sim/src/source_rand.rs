//! Source-shaped pseudo-random stream.
//!
//! The Windows executable seeds the C runtime generator with
//! `srand(GetTickCount())` during startup (`1602_exe.c:106312-106313`)
//! and its decoded gameplay paths call `rand()`. The shipped binary was
//! built against the Microsoft C runtime, whose `rand()` is the familiar
//! 32-bit LCG returning the upper 15 bits.

/// Deterministic seed used by tests and non-interactive simulation
/// construction. The live game seeds from uptime milliseconds.
pub const DEFAULT_SOURCE_RAND_SEED: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceRand {
    state: u32,
}

impl SourceRand {
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    pub fn from_get_tick_count() -> Self {
        Self::new(get_tick_count_seed())
    }

    /// Return the next Microsoft C runtime `rand()` value, 0..=32767.
    pub fn next(&mut self) -> u16 {
        self.state = self.state.wrapping_mul(214013).wrapping_add(2531011);
        ((self.state >> 16) & 0x7fff) as u16
    }
}

impl Default for SourceRand {
    fn default() -> Self {
        Self::new(DEFAULT_SOURCE_RAND_SEED)
    }
}

/// Uptime-millisecond seed matching the shape of Windows `GetTickCount`.
/// Linux hosts use `/proc/uptime`; other environments fall back to wall
/// time, still truncated to the same 32-bit wrapping domain.
pub fn get_tick_count_seed() -> u32 {
    if let Ok(uptime) = std::fs::read_to_string("/proc/uptime") {
        if let Some(seconds) = uptime.split_whitespace().next() {
            if let Ok(seconds) = seconds.parse::<f64>() {
                return (seconds * 1000.0) as u64 as u32;
            }
        }
    }

    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(DEFAULT_SOURCE_RAND_SEED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msvc_rand_sequence_is_pinned() {
        let mut rng = SourceRand::new(1);
        let got: Vec<u16> = (0..5).map(|_| rng.next()).collect();
        assert_eq!(got, vec![41, 18467, 6334, 26500, 19169]);
    }

    #[test]
    fn zero_seed_matches_crt_sequence() {
        let mut rng = SourceRand::new(0);
        assert_eq!(rng.next(), 38);
    }
}
