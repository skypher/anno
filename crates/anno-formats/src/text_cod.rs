//! Parse text.cod / editor.cod section-keyed string tables.
//!
//! These COD files use the byte-negation encryption (decrypted =
//! `(-byte) & 0xFF`) and a simple `[SECTION]` ... `[END]` layout
//! where each non-empty line is the next string in the section.
//!
//! Sections we care about:
//!
//!   * `[WARE]`       — canonical ware-name table the game UI
//!                       uses; index follows Anno's internal good
//!                       ID, not our local `Good` enum.
//!   * `[FIGKIND]`    — soldier-class display names (Infantryman,
//!                      Cavalryman, Musketeer, Artilleryman).
//!   * `[SHIPS]`      — ship-name pool the editor draws from.
//!   * `[STAEDTE]`    — city-name pool.
//!   * `[ROHSTFELD]`  — raw-resource field names (fertility ladder).
//!   * Other sections (e.g. `[GAME]`, `[MELDUNG]`) are display
//!     strings keyed by line-index inside the section.
//!
//! Useful for renderers that want to show authentic ware /
//! soldier / ship-name strings instead of our enum Debug names.

use std::collections::HashMap;

/// Parsed `[SECTION] name1\nname2\n...\n[END]` blocks.
#[derive(Debug, Clone, Default)]
pub struct TextCod {
    pub sections: HashMap<String, Vec<String>>,
}

impl TextCod {
    /// Decode the raw COD bytes (byte-negation if needed), then
    /// walk the `[SECTION]` ... `[END]` blocks. Each line inside
    /// a block becomes an entry. Empty lines are preserved so
    /// callers can index by line number.
    pub fn parse(data: &[u8]) -> Self {
        let plain = decrypt(data);
        let mut sections: HashMap<String, Vec<String>> = HashMap::new();
        let mut current: Option<String> = None;
        let mut buf: Vec<String> = Vec::new();
        for line in plain.lines() {
            let trimmed = line.trim_end();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                let tag = &trimmed[1..trimmed.len() - 1];
                if tag == "END" {
                    if let Some(name) = current.take() {
                        sections.insert(name, std::mem::take(&mut buf));
                    }
                } else {
                    if let Some(name) = current.take() {
                        sections.insert(name, std::mem::take(&mut buf));
                    }
                    current = Some(tag.to_string());
                    buf.clear();
                }
                continue;
            }
            if current.is_some() {
                buf.push(trimmed.to_string());
            }
        }
        // Flush the last section if it omitted its `[END]`.
        if let Some(name) = current.take() {
            sections.insert(name, std::mem::take(&mut buf));
        }
        Self { sections }
    }

    /// Lookup all strings in a named section.
    pub fn section(&self, name: &str) -> Option<&[String]> {
        self.sections.get(name).map(|v| v.as_slice())
    }

    /// Convenience: the ware-name table.
    pub fn wares(&self) -> Option<&[String]> {
        self.section("WARE")
    }
}

fn decrypt(data: &[u8]) -> String {
    let raw = if data.len() > 4
        && data[..4].iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
    {
        data.to_vec()
    } else {
        data.iter().map(|&b| (-(b as i16) & 0xFF) as u8).collect()
    };
    raw.iter().map(|&b| char::from(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_text_cod() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap()
            .join("extracted/text.cod");
        let Ok(bytes) = std::fs::read(&path) else {
            println!("Skipping: text.cod not found");
            return;
        };
        let t = TextCod::parse(&bytes);
        // [WARE] section: 24 wares + the "empty" sentinel at index 0.
        let wares = t.wares().expect("WARE section");
        assert!(wares.len() >= 24,
            "WARE table should have ≥24 entries, got {}", wares.len());
        // Index 0 is the sentinel; index 3 is gold (canonical).
        assert_eq!(wares[0], "empty");
        assert_eq!(wares[3], "gold");
        assert_eq!(wares[23], "wood");
        assert_eq!(wares[24], "bricks");

        // FIGKIND: soldier classes.
        let figs = t.section("FIGKIND").expect("FIGKIND section");
        assert!(figs.iter().any(|s| s == "Infantryman"));
        assert!(figs.iter().any(|s| s == "Cavalryman"));
        assert!(figs.iter().any(|s| s == "Musketeer"));
        assert!(figs.iter().any(|s| s == "Artilleryman"));

        // SHIPS pool covers known authored ship names.
        let ships = t.section("SHIPS").expect("SHIPS section");
        assert!(ships.iter().any(|s| s == "Defender"));
        assert!(ships.iter().any(|s| s == "Imperator"));
    }
}
