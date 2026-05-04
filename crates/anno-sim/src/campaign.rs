//! Campaign mode: a fixed sequence of scenarios played back-to-back.
//!
//! Mission names and arc boundaries are taken directly from
//! `extracted/text.cod` `[KAMPAGNE]` (English edition), where blank
//! lines separate the arcs. Decoding the block gives 20 missions
//! grouped into 6 arcs of sizes [4, 3, 3, 3, 3, 4] — note the
//! first and last arcs are length 4, not 3 (the manual's "seven
//! arcs of three" is mis-summarised; the data is authoritative).
//!
//! Campaign progression: the player completes the current mission
//! (victory state), the campaign advances `next_mission` by one,
//! and the launcher loads the next scenario file. Defeat resets to
//! the start of the current arc, computed from `ARC_STARTS`.

/// 20 campaign missions in order, transcribed from text.cod
/// `[KAMPAGNE]`. Lookup the matching `.szs` file by name in
/// `Szenes/` at load time.
pub const CAMPAIGN_MISSIONS: &[&str] = &[
    "Halfway there",
    "To Each his Island",
    "Appearance can be deceiving",
    "Hard Times",
    "Humility is a Virtue",
    "The Blinding",
    "The Thief",
    "Gold Rush",
    "Spice Monopoly",
    "Dangerous waters",
    "Revenge is sweet",
    "The saviour",
    "Quest for peace",
    "Break the Monopoly",
    "The new Empire",
    "Imperial Proclamation",
    "Veni, vidi, vici",
    "At all Costs",
    "The Deluge",
    "Close Quarters",
];

/// Starting mission index of each arc, derived from the blank-line
/// separators in `text.cod` `[KAMPAGNE]`. Missions in arc `i`
/// inclusive-span `ARC_STARTS[i]..ARC_STARTS[i+1]`, with the final
/// arc running to the end of `CAMPAIGN_MISSIONS`.
pub const ARC_STARTS: &[u8] = &[0, 4, 7, 10, 13, 16];

/// Index of the arc that contains `mission`. Linear scan over
/// `ARC_STARTS` (six entries — small constant).
fn arc_of(mission: u8) -> usize {
    let mut i = ARC_STARTS.len();
    while i > 0 {
        i -= 1;
        if mission >= ARC_STARTS[i] {
            return i;
        }
    }
    0
}

/// Campaign progression state. Saved with the scenario so resuming
/// a campaign-mode game restores the right next-mission pointer.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CampaignState {
    /// True when the player started a campaign (vs. one-off
    /// scenario play). Drives whether on-victory advances to the
    /// next mission or just shows the banner.
    pub active: bool,
    /// Index into `CAMPAIGN_MISSIONS` of the currently-loaded
    /// mission. 0..=20.
    pub current_mission: u8,
}

impl CampaignState {
    pub fn start(mission_idx: u8) -> Self {
        Self {
            active: true,
            current_mission: mission_idx.min(CAMPAIGN_MISSIONS.len() as u8 - 1),
        }
    }

    /// Advance to the next mission. Returns the new index, or `None`
    /// if the campaign is finished (player just beat mission 20).
    pub fn advance_on_victory(&mut self) -> Option<u8> {
        if !self.active {
            return None;
        }
        if (self.current_mission as usize) + 1 >= CAMPAIGN_MISSIONS.len() {
            self.active = false;
            return None;
        }
        self.current_mission += 1;
        Some(self.current_mission)
    }

    /// On defeat, restart the current arc. Arc starts come from
    /// the blank-line layout of `text.cod [KAMPAGNE]` (sizes
    /// [4, 3, 3, 3, 3, 4]); the first and sixth arcs are length 4
    /// rather than 3.
    pub fn restart_arc_on_defeat(&mut self) -> u8 {
        let arc_start = ARC_STARTS[arc_of(self.current_mission)];
        self.current_mission = arc_start;
        arc_start
    }

    pub fn current_name(&self) -> &'static str {
        CAMPAIGN_MISSIONS
            .get(self.current_mission as usize)
            .copied()
            .unwrap_or("(unknown mission)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campaign_has_20_missions_and_six_arcs() {
        assert_eq!(CAMPAIGN_MISSIONS.len(), 20);
        assert_eq!(ARC_STARTS, &[0, 4, 7, 10, 13, 16]);
    }

    #[test]
    fn advance_walks_through_campaign() {
        let mut s = CampaignState::start(0);
        for expected in 1..CAMPAIGN_MISSIONS.len() {
            let next = s.advance_on_victory();
            assert_eq!(next, Some(expected as u8));
        }
        // Past the last mission: campaign deactivates.
        assert_eq!(s.advance_on_victory(), None);
        assert!(!s.active);
    }

    #[test]
    fn defeat_restarts_to_correct_arc_start() {
        // Mission 3 (Hard Times) belongs to arc 0 (length-4 arc),
        // so a defeat there must restart at mission 0, not 3.
        let mut s = CampaignState::start(3);
        assert_eq!(s.restart_arc_on_defeat(), 0);
        assert_eq!(s.current_mission, 0);

        // Mission 7 (Gold Rush) is the start of arc 2.
        let mut s = CampaignState::start(7);
        assert_eq!(s.restart_arc_on_defeat(), 7);

        // Mission 19 (Close Quarters) is in arc 5 (16..=19); a
        // defeat in the final mission must restart at 16 even
        // though `(19 / 3) * 3 = 18` (the old buggy formula).
        let mut s = CampaignState::start(19);
        assert_eq!(s.restart_arc_on_defeat(), 16);
    }
}
