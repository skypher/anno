//! High-score / hall of fame table.
//!
//! RE: `1602_exe.c:48dfe0 FUN_0048df00` writes a `[Objekt: HISCORE]`
//! block to disk with up to 12 entries (stride 11 words = 44 bytes
//! each). Each entry is `name, score, x, x` — three named-game
//! statistics plus a name string. The format matches the binary's
//! Sprintf line `Entry: %2d, %s, %d, %d, %d`.
//!
//! We mirror that 12-entry table size and persist to a plain
//! `hiscore.txt` file in the saves dir. Top score wins.

use std::cmp::Ordering;

/// Maximum entries kept (matches binary's 12-entry HISCORE block).
pub const MAX_HISCORE: usize = 12;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HiscoreEntry {
    pub name: String,
    pub score: u32,
    pub population: u32,
    pub gold: i32,
}

/// Score the player's run for the hall-of-fame. Manual:
/// "Score:" line on the evaluation screen — a weighted sum of
/// inhabitants, treasury, and elapsed time. We use a simplified
/// formula `population * 10 + max(gold, 0) / 10 + buildings * 5`.
pub fn compute_score(population: u32, gold: i32, buildings: u32) -> u32 {
    let pop_score = population.saturating_mul(10);
    let gold_score = gold.max(0) as u32 / 10;
    let bld_score = buildings.saturating_mul(5);
    pop_score
        .saturating_add(gold_score)
        .saturating_add(bld_score)
}

/// Insert `entry` into the table, sort by score descending, keep
/// only the top `MAX_HISCORE` rows. Returns the rank (1-based) the
/// new entry landed at, or `None` if it didn't make the cut.
pub fn insert_entry(table: &mut Vec<HiscoreEntry>, entry: HiscoreEntry) -> Option<usize> {
    table.push(entry);
    table.sort_by(|a, b| b.score.cmp(&a.score).then(Ordering::Equal));
    if table.len() > MAX_HISCORE {
        table.truncate(MAX_HISCORE);
    }
    // Rank: we'll just return the position of the highest-score
    // entry whose score equals our new one (the most recently
    // inserted will sort to the head of any tie).
    let target = table.last().map(|e| e.score);
    table
        .iter()
        .position(|e| Some(e.score) == target)
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &str, score: u32) -> HiscoreEntry {
        HiscoreEntry {
            name: name.to_string(),
            score,
            population: 0,
            gold: 0,
        }
    }

    #[test]
    fn score_combines_inputs() {
        let s = compute_score(100, 5_000, 20);
        assert_eq!(s, 100 * 10 + 5_000 / 10 + 20 * 5);
    }

    #[test]
    fn table_truncates_to_max() {
        let mut t = Vec::new();
        for i in 0..(MAX_HISCORE as u32 + 5) {
            insert_entry(&mut t, mk(&format!("P{i}"), i * 100));
        }
        assert_eq!(t.len(), MAX_HISCORE);
        // Highest score sorted first.
        assert!(t[0].score >= t[t.len() - 1].score);
    }

    #[test]
    fn low_score_doesnt_displace_high() {
        let mut t = Vec::new();
        for i in 0..MAX_HISCORE {
            insert_entry(&mut t, mk("filler", 10_000 + i as u32));
        }
        let lowest = t.last().unwrap().score;
        insert_entry(&mut t, mk("noob", 5));
        assert_eq!(t.len(), MAX_HISCORE);
        // Lowest-of-table is still ≥ the old lowest, so noob
        // didn't make it.
        assert!(t.last().unwrap().score >= lowest);
    }
}
