// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generator playlists and the tracks that sound alike, over the running
//! service.
//!
//! The library keeps generators in a table of their own; the window lists
//! them among the playlists. Their keys are their numbers with
//! [`GENERATOR_KEYS`] added, so that they never meet a playlist's key. A
//! generator is listed in its folder after the playlists there, by name.

use std::time::{SystemTime, UNIX_EPOCH};

use helikodu_model::library::TrackId;
use helikodu_service::library::{
    AnalysisOutcome, Generator, GeneratorError, GeneratorId, GeneratorSource, GeneratorStep,
    GeneratorSteps, LibraryError, MusicLibrary, SimilarTrack, SimilaritySeed, TrackSummary,
};

use super::{BackendError, ServiceLibrary, error, person, track_id};
use crate::backend::generators::{SIMILAR_FLOOR, too_few_analysed};
use crate::backend::{
    AlbumKey, GeneratorInfo, GeneratorRun, GeneratorTrack, NotYet, PlaylistKey, PlaylistRow,
    PlaylistType, SimilarFound, SimilarRow, SimilarSeed, StepProblem,
};
use crate::i18n::tr;

/// Added to a generator's number to make its key.
pub(super) const GENERATOR_KEYS: u64 = 1 << 48;

/// The most tracks given with their format and cover.
const FULL: usize = 2000;

/// The key of the generator `id`.
pub(super) fn generator_key(id: GeneratorId) -> PlaylistKey {
    PlaylistKey(GENERATOR_KEYS + u64::try_from(id.get()).unwrap_or(0))
}

/// The generator a key stands for, if it stands for one.
pub(super) fn generator_id(key: PlaylistKey) -> Option<GeneratorId> {
    key.0
        .checked_sub(GENERATOR_KEYS)
        .and_then(|n| i64::try_from(n).ok())
        .map(GeneratorId::from_raw)
}

/// A seed for the random sort that differs from run to run.
fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
}

/// Why a step cannot run, in the window's words.
fn problem(error: &GeneratorError) -> StepProblem {
    match error {
        GeneratorError::UnknownPlaylist(name) => StepProblem::MissingPlaylist(name.clone()),
        GeneratorError::Syntax(e) => StepProblem::Query(e.to_string()),
        GeneratorError::Query(e) => StepProblem::Query(e.to_string()),
        other => StepProblem::Other(other.to_string()),
    }
}

/// The step of `steps` that `error` is about, when a run did not say: the
/// first that names the missing playlist, else the source.
fn step_of(steps: &GeneratorSteps, error: &GeneratorError) -> usize {
    let GeneratorError::UnknownPlaylist(name) = error else {
        return 0;
    };
    if matches!(&steps.source, GeneratorSource::Playlist(n) if n.eq_ignore_ascii_case(name)) {
        return 0;
    }
    steps
        .steps
        .iter()
        .position(|s| matches!(s, GeneratorStep::Exclude(n) if n.eq_ignore_ascii_case(name)))
        .map_or(0, |i| i + 1)
}

fn library_error(error: LibraryError) -> GeneratorError {
    match error {
        LibraryError::Generator(e) => e,
        other => GeneratorError::Unknown(other.to_string()),
    }
}

impl ServiceLibrary {
    /// The generator playlists as sidebar rows, put into `rows` after what is in
    /// their folders.
    pub(super) fn add_generator_rows(
        &self,
        music: &MusicLibrary,
        rows: &mut Vec<PlaylistRow>,
    ) -> Result<(), BackendError> {
        let generators = music.generators().map_err(error)?;
        let keys = super::lock(&self.playlists);
        for generator in generators {
            let parent = generator.parent.and_then(|folder| {
                keys.iter()
                    .find(|(_, id)| **id == folder)
                    .map(|(key, _)| *key)
            });
            let (at, depth) = match parent.and_then(|p| rows.iter().position(|r| r.key == p)) {
                Some(folder) => {
                    let depth = rows[folder].depth + 1;
                    // After the last row inside the folder.
                    let end = rows[folder + 1..]
                        .iter()
                        .position(|r| r.depth < depth)
                        .map_or(rows.len(), |n| folder + 1 + n);
                    (end, depth)
                }
                None => (rows.len(), 0),
            };
            let key = generator_key(generator.id);
            let (items, duration) = match &generator.tracks {
                Some(tracks) => {
                    let summaries = music.summaries_of(tracks).map_err(error)?;
                    let length = summaries.iter().filter_map(|t| t.duration).sum();
                    (summaries.len(), length)
                }
                None => (0, std::time::Duration::ZERO),
            };
            rows.insert(
                at,
                PlaylistRow {
                    key,
                    name: generator.name.clone(),
                    items,
                    duration,
                    modified: generator.generated.or(Some(generator.modified)),
                    kind: PlaylistType::Generator,
                    parent: parent.filter(|_| depth > 0),
                    depth,
                    ..PlaylistRow::default()
                },
            );
        }
        Ok(())
    }

    fn stored_generator(&self, key: PlaylistKey) -> Result<Generator, BackendError> {
        let id = generator_id(key).ok_or_else(|| BackendError(tr("The playlist is gone.")))?;
        self.library()?
            .generator(id)
            .map_err(error)?
            .ok_or_else(|| BackendError(tr("The playlist is gone.")))
    }

    pub(super) fn generator_info(
        &self,
        key: PlaylistKey,
    ) -> Result<Option<GeneratorInfo>, BackendError> {
        let Some(id) = generator_id(key) else {
            return Ok(None);
        };
        Ok(self
            .library()?
            .generator(id)
            .map_err(error)?
            .map(|g| GeneratorInfo {
                key,
                name: g.name,
                steps: g.steps,
                on_open: g.on_open,
            }))
    }

    pub(super) fn new_generator(
        &self,
        name: &str,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        let parent = parent.map(|p| self.playlist_id(p)).transpose()?;
        let music = self.library()?;
        let id = music
            .create_generator(name, &GeneratorSteps::default(), true)
            .map_err(error)?;
        if parent.is_some() {
            music.move_generator(id, parent).map_err(error)?;
        }
        Ok(generator_key(id))
    }

    /// The rows of `tracks`, in order, with what the steps may sort by.
    fn generator_track_rows(
        &self,
        music: &MusicLibrary,
        tracks: &[TrackId],
    ) -> Result<Vec<GeneratorTrack>, BackendError> {
        let summaries: Vec<TrackSummary> = music.summaries_of(tracks).map_err(error)?;
        Ok(summaries
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let brief = self.summary_row(track);
                if i >= FULL {
                    return GeneratorTrack {
                        row: brief,
                        ..GeneratorTrack::default()
                    };
                }
                let stats = music.stats(track.id).ok();
                let row = self
                    .track_row(music, brief.key, brief.album, brief.copies)
                    .unwrap_or(brief);
                GeneratorTrack {
                    row,
                    last_played: stats.as_ref().and_then(|s| s.last_played),
                    skips: stats.map_or(0, |s| u32::try_from(s.skip_count).unwrap_or(u32::MAX)),
                }
            })
            .collect())
    }

    pub(super) fn run_generator(
        &self,
        key: PlaylistKey,
        again: bool,
    ) -> Result<GeneratorRun, BackendError> {
        let generator = self.stored_generator(key)?;
        let music = self.library()?;
        let make = again || generator.on_open || generator.tracks.is_none();
        if make {
            match music.regenerate(generator.id, seed()) {
                Ok(tracks) => {
                    let made = music
                        .generator(generator.id)
                        .map_err(error)?
                        .and_then(|g| g.generated);
                    return Ok(GeneratorRun {
                        tracks: self.generator_track_rows(music, &tracks)?,
                        counts: Vec::new(),
                        problem: None,
                        made,
                        fresh: true,
                    });
                }
                Err(e) => {
                    let e = library_error(e);
                    let step = step_of(&generator.steps, &e);
                    let tracks = generator.tracks.clone().unwrap_or_default();
                    let mut run = GeneratorRun {
                        tracks: self.generator_track_rows(music, &tracks)?,
                        problem: Some((step, problem(&e))),
                        made: generator.generated,
                        ..GeneratorRun::default()
                    };
                    // Leave out the tracks that have left the library.
                    run.tracks.retain(|t| !t.row.missing);
                    return Ok(run);
                }
            }
        }
        let tracks = music.open_generator(generator.id, seed()).map_err(error)?;
        Ok(GeneratorRun {
            tracks: self.generator_track_rows(music, &tracks)?,
            made: generator.generated,
            ..GeneratorRun::default()
        })
    }

    pub(super) fn trial(
        &self,
        key: PlaylistKey,
        steps: &GeneratorSteps,
    ) -> Result<GeneratorRun, BackendError> {
        let name = self.stored_generator(key).ok().map(|g| g.name);
        let music = self.library()?;
        let trial = music.try_generator(steps, seed(), name.as_deref());
        Ok(GeneratorRun {
            tracks: self.generator_track_rows(music, &trial.tracks)?,
            counts: trial.counts,
            problem: trial.failure.map(|(step, e)| (step, problem(&e))),
            made: None,
            fresh: true,
        })
    }

    pub(super) fn change_generator(
        &self,
        key: PlaylistKey,
        steps: Option<&GeneratorSteps>,
        on_open: Option<bool>,
    ) -> Result<(), BackendError> {
        let generator = self.stored_generator(key)?;
        let music = self.library()?;
        let steps = steps.filter(|s| **s != generator.steps);
        if steps.is_none() && on_open.is_none_or(|o| o == generator.on_open) {
            return Ok(());
        }
        music
            .update_generator(generator.id, None, steps, on_open)
            .map_err(error)?;
        if steps.is_some() {
            // New steps drop the last result: make it at once. A step that
            // cannot run shows when the generator is opened.
            let _ = music.regenerate(generator.id, seed());
        }
        Ok(())
    }

    pub(super) fn rename_generator(
        &self,
        key: PlaylistKey,
        name: &str,
    ) -> Result<(), BackendError> {
        let generator = self.stored_generator(key)?;
        self.library()?
            .update_generator(generator.id, Some(name), None, None)
            .map_err(error)
    }

    pub(super) fn delete_generator(&self, key: PlaylistKey) -> Result<(), BackendError> {
        let generator = self.stored_generator(key)?;
        self.library()?
            .remove_generator(generator.id)
            .map(|_| ())
            .map_err(error)
    }

    pub(super) fn move_generator_to(
        &self,
        key: PlaylistKey,
        parent: Option<PlaylistKey>,
    ) -> Result<(), BackendError> {
        let generator = self.stored_generator(key)?;
        let parent = parent.map(|p| self.playlist_id(p)).transpose()?;
        self.library()?
            .move_generator(generator.id, parent)
            .map_err(error)
    }

    /// A copy of the generator with the same steps, in the same folder.
    pub(super) fn duplicate_generator(
        &self,
        key: PlaylistKey,
        name: &str,
    ) -> Result<PlaylistKey, BackendError> {
        let generator = self.stored_generator(key)?;
        let music = self.library()?;
        let id = music
            .create_generator(name, &generator.steps, generator.on_open)
            .map_err(error)?;
        if generator.parent.is_some() {
            music.move_generator(id, generator.parent).map_err(error)?;
        }
        Ok(generator_key(id))
    }

    /// The last result of a generator, as playlist entries.
    pub(super) fn generator_items(
        &self,
        key: PlaylistKey,
    ) -> Result<Vec<crate::backend::TrackRow>, BackendError> {
        let generator = self.stored_generator(key)?;
        let music = self.library()?;
        let tracks = generator.tracks.unwrap_or_default();
        Ok(music
            .summaries_of(&tracks)
            .map_err(error)?
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut row = self.summary_row(t);
                row.entry = Some(i as u64);
                row
            })
            .collect())
    }

    /// Writes the last result of a generator to a playlist file.
    pub(super) fn export_generator(
        &self,
        key: PlaylistKey,
        path: &std::path::Path,
    ) -> Result<usize, BackendError> {
        let generator = self.stored_generator(key)?;
        let music = self.library()?;
        let tracks = generator.tracks.unwrap_or_default();
        // Through a playlist made for the moment, which the library writes in
        // every format it knows.
        let temporary = music
            .create_playlist(&format!("{} {}", generator.name, seed()))
            .map_err(error)?;
        let written = music
            .add_to_playlist(temporary, tracks, None)
            .map_err(error)
            .and_then(|_| {
                music
                    .export_playlist(temporary, path)
                    .map(|r| r.entries)
                    .map_err(error)
            });
        music.delete_playlist(temporary).map_err(error)?;
        written
    }

    pub(super) fn similar(
        &self,
        seed: SimilarSeed,
        limit: usize,
    ) -> Result<SimilarFound, BackendError> {
        let music = self.library()?;
        let (service_seed, own) = match seed {
            SimilarSeed::Track(track) => (
                SimilaritySeed::Track(track_id(track)),
                vec![track_id(track)],
            ),
            SimilarSeed::Album(album) => {
                let id = self.album_id(album)?;
                let tracks = self.album_track_ids(music, album)?;
                (SimilaritySeed::Album(id), tracks)
            }
            SimilarSeed::Artist(artist) => {
                let tracks = music
                    .sorted_track_ids(
                        helikodu_service::library::TrackFilter::Artist(person(artist), None),
                        &[],
                    )
                    .map_err(error)?
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                (SimilaritySeed::Artist(person(artist)), tracks)
            }
        };
        // Whether any track of the seed has sound features: compared with
        // itself, it has a value.
        let analysed = own
            .iter()
            .any(|t| matches!(music.track_similarity(*t, *t), Ok(Some(_))));
        if !analysed {
            let unreadable = !own.is_empty()
                && own.iter().all(|t| {
                    matches!(
                        music.track_analysis(*t),
                        Ok(Some(a)) if a.current && matches!(a.outcome, AnalysisOutcome::Failed(_))
                    )
                });
            return Ok(SimilarFound::NotYet(if unreadable {
                NotYet::Unreadable
            } else {
                NotYet::NotAnalysed
            }));
        }
        let coverage = music.analysis_coverage().map_err(error)?;
        if too_few_analysed(coverage.analyzed, coverage.tracks) {
            return Ok(SimilarFound::NotYet(NotYet::TooFew(coverage.analyzed)));
        }
        let found: Vec<SimilarTrack> = music
            .similar_tracks(service_seed, limit + 1)
            .map_err(error)?
            .into_iter()
            .filter(|t| t.similarity >= SIMILAR_FLOOR)
            .collect();
        let more = found.len() > limit;
        let ids: Vec<TrackId> = found.iter().take(limit).map(|t| t.track).collect();
        let rows = self.generator_track_rows(music, &ids)?;
        Ok(SimilarFound::Tracks {
            rows: rows
                .into_iter()
                .zip(found.iter())
                .map(|(t, s)| SimilarRow {
                    row: t.row,
                    score: s.similarity,
                })
                .collect(),
            more,
        })
    }

    fn album_id(
        &self,
        album: AlbumKey,
    ) -> Result<helikodu_service::library::AlbumId, BackendError> {
        super::lock(&self.ids)
            .get(&album)
            .copied()
            .ok_or_else(|| BackendError(tr("The album is gone.")))
    }

    fn album_track_ids(
        &self,
        music: &MusicLibrary,
        album: AlbumKey,
    ) -> Result<Vec<TrackId>, BackendError> {
        let _ = music;
        Ok(crate::backend::LibrarySource::album(self, album)?
            .map(|d| d.tracks().map(|t| track_id(t.key)).collect())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_keys_never_meet_playlist_keys() {
        let key = generator_key(GeneratorId::from_raw(7));
        assert_eq!(generator_id(key), Some(GeneratorId::from_raw(7)));
        assert_eq!(generator_id(PlaylistKey(7)), None);
    }
}
