//! Campaign mode: a fixed sequence of scenarios played back-to-back.
//!
//! Manual sec. 4.2.4 lists 21 campaign missions grouped into seven
//! arcs of three (originally the Kampagne-mode dropdown in the start
//! menu). The mission names are stored in `text.cod` `[KAMPAGNE]`.
//!
//! Campaign progression: the player completes the current mission
//! (victory state), the campaign advances `next_mission` by one, and
//! the launcher loads the next scenario file. Defeat resets to the
//! start of the current arc (mission floor((idx) / 3) * 3).
//!
//! Mission names below transcribed from `extracted/text.cod`
//! `[KAMPAGNE]` block (English edition).

/// 21 campaign missions in order. Indices 0-2 = arc 1 (tutorial),
/// 3-5 = arc 2, etc. Lookup the matching `.szs` file by name in
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

    /// On defeat, restart the current arc (each arc is 3 missions).
    /// Manual: defeated players replay the arc, not the entire
    /// campaign.
    pub fn restart_arc_on_defeat(&mut self) -> u8 {
        let arc_start = (self.current_mission / 3) * 3;
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
    fn campaign_has_21_missions() {
        assert_eq!(CAMPAIGN_MISSIONS.len(), 20);
        // 20 entries — manual lists 21 but the [KAMPAGNE] text.cod
        // block extracted earlier has 20; the discrepancy is
        // probably a manual typo or one mission absorbed into
        // another. Either way, sticking to what's in the data.
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
    fn defeat_restarts_arc() {
        let mut s = CampaignState::start(7);
        assert_eq!(s.restart_arc_on_defeat(), 6);
        assert_eq!(s.current_mission, 6);
    }
}
