//! COD file parser for game data (haeuser.cod, figuren.cod, text.cod).
//!
//! COD files are text-based configuration files with a simple key-value structure,
//! possibly encrypted with a simple XOR cipher.

use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CodError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A parsed COD file containing named object groups.
#[derive(Debug)]
pub struct CodFile {
    pub objects: Vec<CodObject>,
}

/// A single object definition from a COD file.
#[derive(Debug, Clone)]
pub struct CodObject {
    pub name: String,
    pub properties: HashMap<String, String>,
    pub children: Vec<CodObject>,
}

impl CodFile {
    /// Parse a COD file from raw bytes.
    /// COD files may be XOR-encrypted or plain text.
    pub fn parse(data: &[u8]) -> Result<Self, CodError> {
        // Try to detect if the file is encrypted
        // The encryption is a simple byte-by-byte transform
        let text = Self::decrypt_if_needed(data);
        let objects = Self::parse_text(&text);
        Ok(CodFile { objects })
    }

    fn decrypt_if_needed(data: &[u8]) -> String {
        // COD files use a simple encryption: each byte is transformed
        // If the first few bytes look like printable ASCII, it's plain text
        if data.len() > 4 && data[..4].iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace()) {
            return String::from_utf8_lossy(data).into_owned();
        }

        // Try XOR decryption with the known key pattern
        let mut decrypted = Vec::with_capacity(data.len());
        for (i, &byte) in data.iter().enumerate() {
            // The encryption uses a rotating key based on position
            let key = (i as u8).wrapping_add(0x72);
            decrypted.push(byte ^ key);
        }

        // Check if decryption produced readable text
        if decrypted.iter().take(20).all(|&b| b.is_ascii() || b == 0) {
            String::from_utf8_lossy(&decrypted).into_owned()
        } else {
            // Fall back to raw interpretation
            String::from_utf8_lossy(data).into_owned()
        }
    }

    fn parse_text(text: &str) -> Vec<CodObject> {
        let mut objects = Vec::new();
        let mut current: Option<CodObject> = None;
        let mut depth = 0u32;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }

            if line.starts_with("@BEGIN") {
                if depth == 0 {
                    let name = line.strip_prefix("@BEGIN").unwrap_or("").trim();
                    current = Some(CodObject {
                        name: name.to_string(),
                        properties: HashMap::new(),
                        children: Vec::new(),
                    });
                }
                depth += 1;
            } else if line.starts_with("@END") {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(obj) = current.take() {
                        objects.push(obj);
                    }
                }
            } else if let Some(ref mut obj) = current {
                // Parse key=value or key:value pairs
                if let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':')) {
                    obj.properties
                        .insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }

        objects
    }
}
