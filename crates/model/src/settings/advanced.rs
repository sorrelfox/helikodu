// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The advanced settings: the `[advanced]` table.

use serde::{Deserialize, Serialize};

use super::{SettingsError, check_range};

/// Tag writing, logs and developer options: the `[advanced]` table.
///
/// ```toml
/// [advanced]
/// id3_version = "2.4"                 # 2.3 | 2.4, for new ID3 tags in MP3 files
/// in_place_tag_write_above_mib = 2048 # larger files get their tags changed in place
/// write_ratings_to_tags = false
/// write_play_counts_to_tags = false
/// log_detail = "off"                  # off | normal | full (debug builds: normal)
/// developer_mode = false
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedSettings {
    /// The ID3v2 version of new tags in MP3 files. Unknown fields are kept either way.
    pub id3_version: Id3Version,
    /// Files larger than this, in MiB, have their tags changed in place instead of
    /// being written anew and replaced (1 to 1 048 576).
    pub in_place_tag_write_above_mib: u32,
    /// Whether ratings are also written to the tags of the files (applied from a
    /// later version).
    pub write_ratings_to_tags: bool,
    /// Whether play counts are also written to the tags of the files, in
    /// batches (not after every play).
    pub write_play_counts_to_tags: bool,
    /// How much the log records.
    pub log_detail: LogDetail,
    /// Whether the test outputs (null and WAV file) are offered.
    pub developer_mode: bool,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            id3_version: Id3Version::V2_4,
            in_place_tag_write_above_mib: 2048,
            write_ratings_to_tags: false,
            write_play_counts_to_tags: false,
            log_detail: LogDetail::default(),
            developer_mode: false,
        }
    }
}

impl AdvancedSettings {
    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_range(
            "advanced.in_place_tag_write_above_mib",
            self.in_place_tag_write_above_mib,
            1..=1 << 20,
        )
    }

    /// The size above which tags are changed in place, in bytes.
    pub fn in_place_tag_write_above(&self) -> u64 {
        u64::from(self.in_place_tag_write_above_mib) << 20
    }
}

/// An ID3v2 version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Id3Version {
    /// ID3v2.3, for old players.
    #[serde(rename = "2.3")]
    V2_3,
    /// ID3v2.4.
    #[default]
    #[serde(rename = "2.4")]
    V2_4,
}

/// How much the log records.
///
/// Release builds keep no log by default; debug builds record the normal
/// detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LogDetail {
    /// No log: nothing is written to disk.
    Off,
    /// Warnings and the main events.
    Normal,
    /// Everything, for a bug report.
    Full,
}

impl Default for LogDetail {
    fn default() -> Self {
        if cfg!(debug_assertions) {
            Self::Normal
        } else {
            Self::Off
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn the_design_defaults() {
        let advanced = AdvancedSettings::default();
        assert_eq!(advanced.id3_version, Id3Version::V2_4);
        assert_eq!(advanced.in_place_tag_write_above(), 2 << 30);
        assert_eq!(advanced.validate(), Ok(()));
    }

    #[test]
    fn versions_are_written_as_numbers() {
        let mut settings = Settings::default();
        settings.advanced.id3_version = Id3Version::V2_3;
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("id3_version = \"2.3\""), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
        settings.advanced.in_place_tag_write_above_mib = 0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn no_logs_round_trips() {
        let mut settings = Settings::default();
        settings.advanced.log_detail = LogDetail::Off;
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("log_detail = \"off\""), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
        let expected = if cfg!(debug_assertions) {
            LogDetail::Normal
        } else {
            LogDetail::Off
        };
        assert_eq!(LogDetail::default(), expected);
    }
}
