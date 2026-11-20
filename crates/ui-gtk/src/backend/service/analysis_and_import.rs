// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Two things the window reads besides the library lists: what the track
//! analysis found for a track (the File info tab), and the other players whose
//! libraries can be imported, offered once after the first scan.

use std::path::Path;
use std::sync::Arc;

use helikodu_model::library::TrackId;
use helikodu_service::library::interchange::default_sources;
use helikodu_service::library::{AnalysisOutcome, MusicLibrary};

use crate::backend::TrackKey;
use crate::backend::services::{AnalysisFacts, FoundPlayer};

/// The file in the state folder that records that the import was offered.
const IMPORT_OFFERED: &str = "import-offered";

/// What the analysis found for `key`: an analysis or a failure that is still
/// current (the file and the track's span as they were), else not yet.
pub(super) fn analysis_facts(music: &MusicLibrary, key: TrackKey) -> AnalysisFacts {
    match music.track_analysis(TrackId::new(key.0)) {
        Ok(Some(stored)) if stored.current => match stored.outcome {
            AnalysisOutcome::Analyzed(analysis) => AnalysisFacts::Analysed(Arc::new(analysis)),
            AnalysisOutcome::Failed(_) => AnalysisFacts::Failed,
        },
        _ => AnalysisFacts::NotYet,
    }
}

/// The players whose libraries are at their usual places, each once, unless
/// the offer was made already (recorded in the state folder `state`).
pub(super) fn import_offer(state: &Path) -> Vec<FoundPlayer> {
    if state.join(IMPORT_OFFERED).exists() {
        return Vec::new();
    }
    let mut found: Vec<FoundPlayer> = Vec::new();
    for source in default_sources() {
        let id = source.player.id();
        if !found.iter().any(|p| p.id == id) {
            found.push(FoundPlayer {
                id: id.to_owned(),
                name: source.player.name().to_owned(),
            });
        }
    }
    found
}

/// Records in the state folder `state` that the import was offered.
pub(super) fn mark_import_offered(state: &Path) {
    if std::fs::create_dir_all(state).is_ok()
        && let Err(e) = std::fs::write(state.join(IMPORT_OFFERED), b"")
    {
        log::warn!("cannot record that the import was offered: {e}");
    }
}
