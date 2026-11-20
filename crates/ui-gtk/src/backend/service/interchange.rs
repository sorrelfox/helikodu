// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Imports, the export of the user data and statistics carried to replacement
//! files over the running service.
//!
//! The file of the source chosen last is read once and kept, so that changing
//! an option or adding a folder that moved only plans again; the plan made last
//! is kept for "Import". Both are dropped when the Import window closes.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use helikodu_model::library::TrackId;
use helikodu_service::Service;
use helikodu_service::library::interchange::{
    ForeignLibrary, ImportError, ImportPlan, ImportRunId, ImportSource, UserDataFile,
    default_sources, read_source,
};
use helikodu_service::library::{LibraryError, MusicLibrary};

use super::super::interchange::{
    ImportFrom, ImportPreview, ImportProblem, ImportRunRow, InterchangeServices, MergeOptions,
    Player, PlayerSource, ReplacedTrack, run_source,
};
use super::super::{BackendError, TrackKey};
use super::error;
use crate::i18n::tr;

/// A source as it was read.
#[derive(Debug)]
enum Read {
    Player(ImportSource, ForeignLibrary),
    Data(UserDataFile),
}

#[derive(Debug, Default)]
struct Cache {
    read: Option<(ImportFrom, Arc<Read>)>,
    plan: Option<(u64, ImportPlan)>,
    next: u64,
}

/// [`InterchangeServices`] over the running service.
#[derive(Debug)]
pub(super) struct ServiceInterchange {
    pub(super) service: Arc<Service>,
    cache: Mutex<Cache>,
}

impl ServiceInterchange {
    pub(super) fn new(service: Arc<Service>) -> Self {
        Self {
            service,
            cache: Mutex::new(Cache::default()),
        }
    }

    fn music(&self) -> Result<&MusicLibrary, BackendError> {
        self.service
            .library()
            .map(|l| l.library())
            .ok_or_else(|| BackendError(tr("Helikodu runs without a library.")))
    }

    fn announce(&self) {
        if let Some(library) = self.service.library() {
            library.announce_change();
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The source `from`, read now or kept from before.
    fn read(&self, from: &ImportFrom) -> Result<Arc<Read>, ImportProblem> {
        if let Some((kept, read)) = &self.cache().read
            && kept == from
        {
            return Ok(read.clone());
        }
        let read = match from {
            ImportFrom::Player { player, path } => {
                let source = ImportSource {
                    player: *player,
                    path: path.clone(),
                };
                let library = read_source(&source).map_err(problem)?;
                Read::Player(source, library)
            }
            ImportFrom::DataFile(path) => {
                Read::Data(MusicLibrary::read_user_data(path).map_err(|e| match e {
                    LibraryError::Import(e) => problem(e),
                    other => ImportProblem::Failed(other.to_string()),
                })?)
            }
        };
        let read = Arc::new(read);
        self.cache().read = Some((from.clone(), read.clone()));
        Ok(read)
    }
}

/// Why a source cannot be read, for the window.
fn problem(error: ImportError) -> ImportProblem {
    match error {
        ImportError::Newer { .. } => ImportProblem::Newer,
        ImportError::Format { reason, .. } => ImportProblem::Unreadable(reason),
        ImportError::Io { source, .. } => ImportProblem::Unreadable(source.to_string()),
        other => ImportProblem::Failed(other.to_string()),
    }
}

impl InterchangeServices for ServiceInterchange {
    fn players(&self) -> Vec<PlayerSource> {
        let found = default_sources();
        Player::ALL
            .into_iter()
            .map(|player| PlayerSource {
                player,
                found: found
                    .iter()
                    .find(|s| s.player == player)
                    .map(|s| s.path.clone()),
            })
            .collect()
    }

    fn plan_import(
        &self,
        from: &ImportFrom,
        options: &MergeOptions,
    ) -> Result<ImportPreview, ImportProblem> {
        let music = self
            .music()
            .map_err(|e| ImportProblem::Failed(e.0.clone()))?;
        let read = self.read(from)?;
        let plan = match &*read {
            Read::Player(source, library) => music.plan_source_import(source, library, options),
            Read::Data(file) => music.plan_user_data(file, options),
        }
        .map_err(|e| ImportProblem::Failed(e.to_string()))?;
        let earlier = music.import_runs().ok().and_then(|runs| {
            runs.into_iter()
                .filter(|r| !r.undone && r.source == plan.source)
                .map(|r| r.at)
                .max()
        });
        let mut cache = self.cache();
        cache.next += 1;
        let number = cache.next;
        let preview = ImportPreview {
            from: from.clone(),
            files: plan.files.clone(),
            saved: plan.saved.clone(),
            matching: plan.matching,
            totals: plan.totals,
            playlists: plan.playlists.clone(),
            ambiguous: plan.ambiguous.clone(),
            unmatched: plan.unmatched.clone(),
            field_conflicts: plan.field_conflicts.clone(),
            notes: plan.notes.clone(),
            earlier,
            empty: plan.is_empty(),
            plan: number,
        };
        cache.plan = Some((number, plan));
        Ok(preview)
    }

    fn apply_import(&self, plan: u64) -> Result<i64, BackendError> {
        let kept = self.cache().plan.take();
        let Some((number, plan_kept)) = kept else {
            return Err(BackendError(tr("Nothing to import.")));
        };
        if number != plan {
            self.cache().plan = Some((number, plan_kept));
            return Err(BackendError(tr("Nothing to import.")));
        }
        let run = self.music()?.apply_import(&plan_kept);
        self.announce();
        Ok(run.map_err(error)?.get())
    }

    fn import_runs(&self) -> Result<Vec<ImportRunRow>, BackendError> {
        Ok(self
            .music()?
            .import_runs()
            .map_err(error)?
            .into_iter()
            .map(|run| {
                let (player, path) = run_source(run.kind, &run.source);
                ImportRunRow {
                    id: run.id.get(),
                    kind: run.kind,
                    player,
                    path,
                    at: run.at,
                    undone: run.undone,
                    undone_at: run.undone_at,
                    tracks: run.summary.as_ref().map_or(run.tracks, |s| s.totals.tracks),
                    summary: run.summary,
                }
            })
            .collect())
    }

    fn undo_import(&self, run: i64) -> Result<(), BackendError> {
        let undone = self.music()?.undo_import(ImportRunId::new(run));
        self.announce();
        undone.map_err(|e| match e {
            LibraryError::Import(ImportError::NotNewest) => {
                BackendError(tr("Undo the newer imports first."))
            }
            other => error(other),
        })
    }

    fn export_user_data(&self, path: &Path) -> Result<(), BackendError> {
        self.music()?
            .export_user_data_to(path)
            .map(|_| ())
            .map_err(error)
    }

    fn replaced_tracks(&self) -> Result<Vec<ReplacedTrack>, BackendError> {
        let mut tracks: Vec<ReplacedTrack> = Vec::new();
        for c in self.music()?.carry_candidates().map_err(error)? {
            let new = (TrackKey(c.new.get()), c.new_path.clone());
            if let Some(track) = tracks.iter_mut().find(|t| t.old.0 == c.old.get()) {
                track.new.push(new);
                continue;
            }
            tracks.push(ReplacedTrack {
                old: TrackKey(c.old.get()),
                old_path: c.old_path,
                title: c.title,
                artist: c.artist,
                plays: c.stats.play_count,
                rating: c.stats.rating.map(|r| r.half_stars()),
                history: c.history,
                playlists: c.playlists,
                new: vec![new],
            });
        }
        Ok(tracks)
    }

    fn carry_stats(&self, pairs: &[(TrackKey, TrackKey)]) -> Result<i64, BackendError> {
        let pairs: Vec<(TrackId, TrackId)> = pairs
            .iter()
            .map(|(old, new)| (TrackId::new(old.0), TrackId::new(new.0)))
            .collect();
        let run = self.music()?.carry_stats_of(&pairs);
        self.announce();
        Ok(run.map_err(error)?.get())
    }

    fn forget(&self) {
        let mut cache = self.cache();
        cache.read = None;
        cache.plan = None;
    }
}
