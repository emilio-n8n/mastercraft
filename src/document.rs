//! MasterCraft project document.
//!
//! Only editing settings are persisted. Audio samples live outside the JSON
//! project file; see the `EditState` history in [`crate::app`] for the reason.

use serde::{Deserialize, Serialize};

/// The persisted part of a MasterCraft project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Document title.
    pub title: String,
    /// Path of the WAV the samples were loaded from, when known.
    pub source_path: Option<String>,
    /// Gain in decibels applied by the last processing run.
    pub gain_db: f32,
    /// Whether peak normalisation is enabled.
    pub normalize: bool,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            title: "Untitled".to_owned(),
            source_path: None,
            gain_db: 0.0,
            normalize: false,
        }
    }
}

impl Document {
    /// Built-in demonstration settings, paired with a generated sine buffer.
    #[must_use]
    pub fn sample() -> Self {
        Self {
            title: "MasterCraft demo".to_owned(),
            source_path: None,
            gain_db: 0.0,
            normalize: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let document = Document {
            title: "Take 03".to_owned(),
            source_path: Some("audio/take-03.wav".to_owned()),
            gain_db: -3.5,
            normalize: true,
        };
        let json = serde_json::to_string(&document).expect("serialize");
        let decoded: Document = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, document);
    }

    #[test]
    fn defaults_are_neutral() {
        let document = Document::default();
        assert_eq!(document.title, "Untitled");
        assert_eq!(document.source_path, None);
        assert_eq!(document.gain_db, 0.0);
        assert!(!document.normalize);
    }

    #[test]
    fn json_round_trip_without_source() {
        let document = Document::default();
        let json = serde_json::to_string(&document).expect("serialize");
        assert!(json.contains("\"source_path\":null"));
        let decoded: Document = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, document);
    }
}
