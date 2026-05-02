//! Persistent user settings: master/music/SFX volume and the default zoom.
//!
//! Stored as a tiny `name = value` text file at
//! `$XDG_CONFIG_HOME/anno/settings.toml` (or `$HOME/.config/anno/...`).
//! Roll-your-own parser to avoid pulling in `toml` for four scalars.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub master_volume: u8,  // 0..=100
    pub music_volume: u8,   // 0..=100
    pub sfx_volume: u8,     // 0..=100
    pub default_zoom: u8,   // 1..=8 (display zoom level)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            master_volume: 70,
            music_volume: 60,
            sfx_volume: 80,
            default_zoom: 2,
        }
    }
}

impl Settings {
    pub fn config_path() -> PathBuf {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(".config")
            });
        base.join("anno").join("settings.toml")
    }

    pub fn load_default() -> Self {
        Self::load(&Self::config_path()).unwrap_or_default()
    }

    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut s = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let (k, v) = line.split_once('=')?;
            let k = k.trim();
            let v = v.trim();
            match k {
                "master_volume" => s.master_volume = v.parse().unwrap_or(s.master_volume),
                "music_volume"  => s.music_volume  = v.parse().unwrap_or(s.music_volume),
                "sfx_volume"    => s.sfx_volume    = v.parse().unwrap_or(s.sfx_volume),
                "default_zoom"  => s.default_zoom  = v.parse().unwrap_or(s.default_zoom),
                _ => {} // forward-compat: ignore unknown keys
            }
        }
        Some(s)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = format!(
            "# Anno settings\n\
             master_volume = {}\n\
             music_volume = {}\n\
             sfx_volume = {}\n\
             default_zoom = {}\n",
            self.master_volume, self.music_volume,
            self.sfx_volume, self.default_zoom,
        );
        std::fs::write(path, body)
    }

    pub fn save_default(&self) -> std::io::Result<()> {
        self.save(&Self::config_path())
    }

    /// Adjust setting at slot `idx` by `delta` (clamped to its valid range).
    pub fn adjust(&mut self, idx: usize, delta: i32) {
        let bump = |v: u8, max: u8| -> u8 {
            (v as i32 + delta).clamp(0, max as i32) as u8
        };
        match idx {
            0 => self.master_volume = bump(self.master_volume, 100),
            1 => self.music_volume  = bump(self.music_volume, 100),
            2 => self.sfx_volume    = bump(self.sfx_volume, 100),
            3 => self.default_zoom  = bump(self.default_zoom.max(1), 8).max(1),
            _ => {}
        }
    }

    pub fn label(idx: usize) -> &'static str {
        match idx {
            0 => "Master volume",
            1 => "Music volume",
            2 => "SFX volume",
            3 => "Default zoom",
            _ => "?",
        }
    }

    pub fn value(&self, idx: usize) -> u8 {
        match idx {
            0 => self.master_volume,
            1 => self.music_volume,
            2 => self.sfx_volume,
            3 => self.default_zoom,
            _ => 0,
        }
    }

    pub const COUNT: usize = 4;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_defaults() {
        let s = Settings::default();
        let path = std::env::temp_dir().join("anno_settings_test.toml");
        s.save(&path).unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(s, loaded);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn adjust_clamps_to_range() {
        let mut s = Settings::default();
        s.adjust(0, -200);
        assert_eq!(s.master_volume, 0);
        s.adjust(0, 9999);
        assert_eq!(s.master_volume, 100);
        s.adjust(3, -50);
        assert!(s.default_zoom >= 1);
    }

    #[test]
    fn unknown_keys_ignored() {
        let path = std::env::temp_dir().join("anno_settings_unknown.toml");
        std::fs::write(&path,
            "master_volume = 42\nfuture_key = nope\nmusic_volume = 33\n"
        ).unwrap();
        let s = Settings::load(&path).unwrap();
        assert_eq!(s.master_volume, 42);
        assert_eq!(s.music_volume, 33);
        std::fs::remove_file(&path).ok();
    }
}
