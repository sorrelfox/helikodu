// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `export.toml`: the converter's own presets and last choices, and the sync
//! destinations. Kept apart from `settings.toml`, since the tools write it as they
//! are used. A missing or broken file gives the defaults.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::naming::NamingPattern;
use super::presets::Preset;
use super::sync::SyncDestination;
use crate::files::write_atomic;

/// The name of the file in the configuration folder.
pub const EXPORT_FILE: &str = "export.toml";

/// The converter's last choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConverterChoices {
    /// The preset, by its identifier.
    pub preset: String,
    /// The folder converted files go to.
    pub folder: Option<PathBuf>,
    /// The naming pattern.
    pub pattern: String,
    /// The characters names may hold, by [`NameStyle::id`](super::NameStyle::id).
    pub names: String,
    /// Whether tags and covers are copied into the new files.
    pub copy_tags: bool,
    /// Whether files that are already there are replaced.
    pub replace: bool,
    /// The number of files converted at a time.
    pub workers: usize,
}

impl Default for ConverterChoices {
    fn default() -> Self {
        Self {
            preset: "opus-128".to_owned(),
            folder: None,
            pattern: NamingPattern::DEFAULT.to_owned(),
            names: "portable".to_owned(),
            copy_tags: true,
            replace: false,
            workers: 4,
        }
    }
}

/// Everything in `export.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportState {
    /// The presets the user made, in order.
    #[serde(rename = "preset")]
    pub presets: Vec<Preset>,
    /// The converter's last choices.
    pub converter: ConverterChoices,
    /// The sync destinations, in order.
    #[serde(rename = "destination")]
    pub destinations: Vec<SyncDestination>,
}

impl ExportState {
    /// The built-in presets followed by the user's.
    pub fn all_presets(&self) -> Vec<Preset> {
        let mut all = Preset::builtins();
        all.extend(self.presets.iter().cloned());
        all
    }

    /// The preset with `id`, built-in or the user's.
    pub fn preset(&self, id: &str) -> Option<Preset> {
        self.all_presets().into_iter().find(|p| p.id == id)
    }

    /// A free identifier for a new preset of the user's.
    pub fn new_preset_id(&self) -> String {
        let n = self
            .presets
            .iter()
            .filter_map(|p| p.id.strip_prefix("own-")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        format!("own-{}", n + 1)
    }

    /// A free identifier for a new destination.
    pub fn new_destination_id(&self) -> String {
        let n = self
            .destinations
            .iter()
            .filter_map(|d| d.id.strip_prefix("dest-")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        format!("dest-{}", n + 1)
    }
}

/// Reads and writes `export.toml`.
#[derive(Debug, Clone)]
pub struct ExportStore {
    path: PathBuf,
}

impl ExportStore {
    /// The store for the file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The store in the configuration folder `config`.
    pub fn in_folder(config: &Path) -> Self {
        Self::new(config.join(EXPORT_FILE))
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the file holds, or the defaults.
    pub fn load(&self) -> ExportState {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Saves `state` by atomic replace.
    pub fn save(&self, state: &ExportState) -> io::Result<()> {
        let text = toml::to_string(state).map_err(io::Error::other)?;
        write_atomic(&self.path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("helikodu-export-{}", std::process::id()));
        let store = ExportStore::in_folder(&dir);
        assert_eq!(store.load(), ExportState::default());
        let mut state = ExportState::default();
        let mut own = Preset::builtins()[0].clone();
        own.id = state.new_preset_id();
        own.name = "Mine".into();
        state.presets.push(own);
        state.destinations.push(SyncDestination::new(
            state.new_destination_id(),
            "Phone".into(),
            dir.join("phone"),
        ));
        state.converter.workers = 2;
        store.save(&state).unwrap();
        assert_eq!(store.load(), state);
        assert_eq!(state.new_preset_id(), "own-2");
        assert_eq!(state.preset("own-1").unwrap().name, "Mine");
        let _ = std::fs::remove_dir_all(dir);
    }
}
