// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The loudness scanner as a background job: "measure the selected tracks", "measure
//! this album", "measure the rest of the library".
//!
//! Tracks are decoded and measured in parallel (EBU R128 / ITU-R BS.1770-4, see
//! [`helikodu_engine::analysis`]) by a small pool of threads at a lowered priority.
//! The tracks of an album are measured together: once all of them are done, the
//! album is measured as one programme and the results of the album are stored in
//! the library at once. A track picked on its own, or an album of which only some
//! tracks were picked, gets track values only.
//!
//! The results go to the library database. Only when [`LoudnessRequest::write_tags`]
//! asks for it are they also written into the files' tags, as ReplayGain 2.0 values
//! (`REPLAYGAIN_TRACK_GAIN` `-6.30 dB`, `REPLAYGAIN_TRACK_PEAK` `0.988525`, and the
//! album pair; peaks are true peaks), in every format including Opus, where they
//! take precedence over `R128_*_GAIN` when read. Tracks of CUE sheets share their
//! file with other tracks, so their values stay in the library.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use helikodu_engine::analysis::{
    AnalysisError, MEASUREMENT_VERSION, TrackMeasurement, album_loudness, measure_source,
};
use helikodu_engine::decode::DecoderRegistry;
use helikodu_library::tag_editing::{BatchItem, TagChanges, TagField};
use helikodu_library::{
    AlbumId, AlbumMeasurement, CancelToken, LoudnessSelection, LoudnessTarget, MeasuredLoudness,
    TagWriteSettings,
};
use helikodu_model::library::TrackId;
use helikodu_model::settings::{AdvancedSettings, LibrarySettings};

use crate::library::{LibraryError, MusicLibrary, scan_options};

/// The nice value of the measuring threads: below interactive work, above idle.
const NICE: i32 = 10;

/// What to measure, and where the results go.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudnessRequest {
    /// The tracks.
    pub selection: LoudnessSelection,
    /// Whether the results are also written into the files' tags (the
    /// `loudness.write_measured_to_tags` setting). They always go to the library.
    pub write_tags: bool,
    /// The number of measuring threads; 0 picks half the processors, at least one.
    pub threads: usize,
    /// The `[library]` settings: the rules by which written files are read again.
    pub library: LibrarySettings,
    /// The `[advanced]` settings: how tags are written.
    pub advanced: AdvancedSettings,
    /// The scan worker program that reads written files again, when it is not
    /// beside the running program.
    pub worker_program: Option<PathBuf>,
}

impl LoudnessRequest {
    /// A request for `selection` with the default settings, results kept in the
    /// library only.
    pub fn new(selection: LoudnessSelection) -> Self {
        Self {
            selection,
            write_tags: false,
            threads: 0,
            library: LibrarySettings::default(),
            advanced: AdvancedSettings::default(),
            worker_program: None,
        }
    }

    /// "Measure the rest": every track without a current measurement.
    pub fn unmeasured() -> Self {
        Self::new(LoudnessSelection::Unmeasured {
            version: MEASUREMENT_VERSION,
        })
    }
}

/// What happened to the tags of a measured track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagOutcome {
    /// Writing tags was not asked for.
    NotAsked,
    /// The values were written into the file.
    Written,
    /// Not written, for the reason given (a CUE sheet track, silence).
    Skipped(String),
    /// Writing failed.
    Failed(String),
}

/// The result of one track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackResult {
    /// The track.
    pub track: TrackId,
    /// Its file.
    pub path: PathBuf,
    /// Its album.
    pub album: Option<AlbumId>,
    /// The values stored, or why the track could not be measured.
    pub measured: Result<MeasuredLoudness, String>,
    /// What happened to its tags.
    pub tags: TagOutcome,
}

/// What the job announces, in this order: `Started`, a `Progress` per track
/// measured (or failed), an `Stored` per album or lone track whose results were
/// stored, and `Finished`.
#[derive(Debug, Clone, PartialEq)]
pub enum LoudnessEvent {
    /// The tracks to measure are known.
    Started {
        /// How many.
        tracks: usize,
    },
    /// A track was measured, or failed.
    Progress {
        /// The tracks done so far, this one included.
        done: usize,
        /// All tracks.
        total: usize,
        /// The track.
        track: TrackId,
        /// Whether it was measured.
        ok: bool,
    },
    /// Results were stored in the library (and written to tags, if asked).
    Stored(Vec<TrackResult>),
    /// The job ended.
    Finished(LoudnessSummary),
}

/// How the job ended.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoudnessSummary {
    /// The tracks there were to measure.
    pub total: usize,
    /// Those measured and stored.
    pub measured: usize,
    /// Those that could not be measured.
    pub failed: usize,
    /// Those whose values were written into their files.
    pub tags_written: usize,
    /// Whether the job was cancelled before it finished.
    pub cancelled: bool,
    /// Every stored or failed track, in the order they were stored.
    pub results: Vec<TrackResult>,
    /// Why the job stopped early, when the library failed.
    pub error: Option<String>,
}

/// A loudness measurement running in the background.
#[derive(Debug)]
pub struct LoudnessJob {
    cancel: CancelToken,
    thread: JoinHandle<LoudnessSummary>,
}

impl LoudnessJob {
    /// Starts measuring. `events` is called on the job's thread.
    pub fn start(
        library: &MusicLibrary,
        request: LoudnessRequest,
        events: impl FnMut(LoudnessEvent) + Send + 'static,
    ) -> Result<Self, LibraryError> {
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let library = library.clone();
        let thread = thread::Builder::new()
            .name("helikodu-loudness".into())
            .spawn(move || run(&library, &request, &token, events))
            .map_err(|source| LibraryError::Io {
                context: "cannot start the loudness scanner".into(),
                source,
            })?;
        Ok(Self { cancel, thread })
    }

    /// Asks the job to stop. Albums and tracks finished so far stay stored.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether the job has ended.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Waits for the job to end.
    pub fn join(self) -> LoudnessSummary {
        self.thread.join().unwrap_or_else(|_| LoudnessSummary {
            error: Some("the loudness scanner failed".into()),
            ..LoudnessSummary::default()
        })
    }
}

/// Measures on the calling thread: what [`LoudnessJob`] runs.
pub fn run(
    library: &MusicLibrary,
    request: &LoudnessRequest,
    cancel: &CancelToken,
    mut events: impl FnMut(LoudnessEvent),
) -> LoudnessSummary {
    let mut summary = LoudnessSummary::default();
    let targets = match library.inner().loudness_targets(&request.selection) {
        Ok(targets) => targets,
        Err(error) => {
            summary.error = Some(error.to_string());
            events(LoudnessEvent::Finished(summary.clone()));
            return summary;
        }
    };
    summary.total = targets.len();
    events(LoudnessEvent::Started {
        tracks: targets.len(),
    });
    let groups = groups(&targets);
    let mut writer = request.write_tags.then(|| TagWriter::new(request));

    let threads = match request.threads {
        0 => thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1)),
        n => n,
    }
    .min(targets.len().max(1));
    let next = AtomicUsize::new(0);
    let (sender, results) = mpsc::channel::<(usize, Result<TrackMeasurement, AnalysisError>)>();
    let registry = DecoderRegistry::with_default_backends();

    thread::scope(|scope| {
        for index in 0..threads {
            let sender = sender.clone();
            let (registry, next, targets) = (&registry, &next, &targets);
            let spawned = thread::Builder::new()
                .name(format!("helikodu-loudness-{index}"))
                .spawn_scoped(scope, move || {
                    lower_priority();
                    loop {
                        if cancel.is_cancelled() {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(target) = targets.get(i) else {
                            break;
                        };
                        let source = target.source.media_source();
                        let result = measure_source(registry, &source, &|| cancel.is_cancelled());
                        if sender.send((i, result)).is_err() {
                            break;
                        }
                    }
                });
            if let Err(error) = spawned {
                if index == 0 {
                    summary.error = Some(format!("cannot start the measuring threads: {error}"));
                }
                break;
            }
        }
        drop(sender);

        let mut pending: Vec<Option<Result<TrackMeasurement, AnalysisError>>> =
            targets.iter().map(|_| None).collect();
        let mut remaining: Vec<usize> = groups.iter().map(Vec::len).collect();
        let group_of: Vec<usize> = {
            let mut of = vec![0; targets.len()];
            for (g, members) in groups.iter().enumerate() {
                for &i in members {
                    of[i] = g;
                }
            }
            of
        };
        let mut done = 0;
        for (i, result) in results {
            if matches!(result, Err(AnalysisError::Cancelled)) {
                continue;
            }
            done += 1;
            events(LoudnessEvent::Progress {
                done,
                total: targets.len(),
                track: targets[i].track,
                ok: result.is_ok(),
            });
            pending[i] = Some(result);
            let g = group_of[i];
            remaining[g] -= 1;
            if remaining[g] > 0 || summary.error.is_some() {
                continue;
            }
            let finished: Vec<(usize, Result<TrackMeasurement, AnalysisError>)> = groups[g]
                .iter()
                .filter_map(|&m| pending[m].take().map(|r| (m, r)))
                .collect();
            let album = album_of(&groups[g], &targets) && finished.iter().all(|(_, r)| r.is_ok());
            match finish_group(library, &targets, finished, album, writer.as_mut()) {
                Ok(stored) => {
                    for result in &stored {
                        match &result.measured {
                            Ok(_) => summary.measured += 1,
                            Err(_) => summary.failed += 1,
                        }
                        if result.tags == TagOutcome::Written {
                            summary.tags_written += 1;
                        }
                    }
                    summary.results.extend(stored.iter().cloned());
                    events(LoudnessEvent::Stored(stored));
                }
                Err(error) => {
                    summary.error = Some(error.to_string());
                    cancel.cancel();
                }
            }
        }
    });
    summary.cancelled = cancel.is_cancelled() && summary.error.is_none();
    events(LoudnessEvent::Finished(summary.clone()));
    summary
}

/// The targets in groups that are stored together: the tracks of one album, or
/// one track without an album.
fn groups(targets: &[LoudnessTarget]) -> Vec<Vec<usize>> {
    let mut albums: BTreeMap<AlbumId, usize> = BTreeMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, target) in targets.iter().enumerate() {
        match target.album {
            Some(album) => {
                let g = *albums.entry(album).or_insert_with(|| {
                    groups.push(Vec::new());
                    groups.len() - 1
                });
                groups[g].push(i);
            }
            None => groups.push(vec![i]),
        }
    }
    groups
}

/// Whether the group is a whole album, so that album values are measured.
fn album_of(group: &[usize], targets: &[LoudnessTarget]) -> bool {
    let first = &targets[group[0]];
    first.album.is_some() && group.len() == first.album_size
}

/// Measures the album if `album`, stores the group's results and writes tags.
fn finish_group(
    library: &MusicLibrary,
    targets: &[LoudnessTarget],
    finished: Vec<(usize, Result<TrackMeasurement, AnalysisError>)>,
    album: bool,
    writer: Option<&mut TagWriter>,
) -> Result<Vec<TrackResult>, LibraryError> {
    let album_values = if album {
        album_loudness(finished.iter().filter_map(|(_, r)| r.as_ref().ok())).map(|a| {
            AlbumMeasurement {
                integrated_lufs: a.integrated_lufs,
                range_lu: a.range_lu,
                sample_peak: a.sample_peak,
                true_peak: a.true_peak,
            }
        })
    } else {
        None
    };
    let mut results = Vec::new();
    let mut store = Vec::new();
    for (i, result) in finished {
        let target = &targets[i];
        let measured = result
            .map(|m| {
                let measured = MeasuredLoudness {
                    track: target.track,
                    version: MEASUREMENT_VERSION,
                    integrated_lufs: m.loudness.integrated_lufs,
                    range_lu: m.loudness.range_lu,
                    sample_peak: m.loudness.sample_peak,
                    true_peak: m.loudness.true_peak,
                    album: album_values,
                };
                store.push(measured);
                measured
            })
            .map_err(|error| error.to_string());
        results.push(TrackResult {
            track: target.track,
            path: target.source.path.clone(),
            album: target.album,
            measured,
            tags: TagOutcome::NotAsked,
        });
    }
    library.inner().store_loudness(store)?;
    if let Some(writer) = writer {
        writer.write(library, targets, &mut results)?;
    }
    Ok(results)
}

/// Writes measured values into tags.
struct TagWriter {
    settings: TagWriteSettings,
    library: LibrarySettings,
    worker_program: Option<PathBuf>,
}

impl TagWriter {
    fn new(request: &LoudnessRequest) -> Self {
        Self {
            settings: TagWriteSettings::from_settings(&request.advanced),
            library: request.library.clone(),
            worker_program: request.worker_program.clone(),
        }
    }

    /// Writes the measured values of `results` and restamps their measurements.
    fn write(
        &mut self,
        library: &MusicLibrary,
        targets: &[LoudnessTarget],
        results: &mut [TrackResult],
    ) -> Result<(), LibraryError> {
        let mut items = Vec::new();
        let mut item_of = Vec::new();
        for (index, result) in results.iter_mut().enumerate() {
            let Ok(measured) = &result.measured else {
                result.tags = TagOutcome::Skipped("not measured".into());
                continue;
            };
            let whole_file = targets
                .iter()
                .find(|t| t.track == result.track)
                .is_some_and(|t| t.whole_file);
            if !whole_file {
                result.tags = TagOutcome::Skipped("part of a file shared with other tracks".into());
                continue;
            }
            let Some(changes) = tag_changes(measured) else {
                result.tags = TagOutcome::Skipped("silence has no gain".into());
                continue;
            };
            items.push(BatchItem {
                path: result.path.clone(),
                expected: None,
                changes,
            });
            item_of.push(index);
        }
        if items.is_empty() {
            return Ok(());
        }
        let scan = scan_options(&self.library, self.worker_program.as_deref())?;
        let report = library.inner().write_tags(
            &items,
            &self.settings,
            &scan,
            &mut |_| {},
            &CancelToken::new(),
        )?;
        let mut written = Vec::new();
        for (index, (path, outcome)) in item_of.iter().zip(&report.batch.results) {
            let result = &mut results[*index];
            debug_assert_eq!(*path, result.path);
            result.tags = match outcome {
                Ok(_) => {
                    written.push(result.track);
                    TagOutcome::Written
                }
                Err(error) => TagOutcome::Failed(error.to_string()),
            };
        }
        library.inner().restamp_loudness(written)?;
        Ok(())
    }
}

/// The tag changes that carry `measured`, or `None` for silence.
fn tag_changes(measured: &MeasuredLoudness) -> Option<TagChanges> {
    let track_gain = measured.track_gain_db()?;
    let mut changes = TagChanges::new();
    changes.set_one(TagField::ReplayGainTrackGain, format_gain(track_gain));
    changes.set_one(
        TagField::ReplayGainTrackPeak,
        format_peak(measured.true_peak),
    );
    if let Some(album) = measured.album
        && let Some(gain) = album.gain_db()
    {
        changes.set_one(TagField::ReplayGainAlbumGain, format_gain(gain));
        changes.set_one(TagField::ReplayGainAlbumPeak, format_peak(album.true_peak));
    }
    Some(changes)
}

/// A gain as ReplayGain 2.0 writes it: `-6.30 dB`.
fn format_gain(db: f64) -> String {
    format!("{db:.2} dB")
}

/// A peak as ReplayGain 2.0 writes it: `0.988525`.
fn format_peak(peak: f64) -> String {
    format!("{peak:.6}")
}

/// Lowers the calling thread's scheduling priority; a failure leaves it as it is.
fn lower_priority() {
    let thread = rustix::thread::gettid();
    let _ = rustix::process::setpriority_process(Some(thread), NICE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_values_in_replaygain_2_terms() {
        let measured = MeasuredLoudness {
            track: TrackId::new(1),
            version: MEASUREMENT_VERSION,
            integrated_lufs: Some(-11.7),
            range_lu: 5.0,
            sample_peak: 0.9,
            true_peak: 1.0234567,
            album: Some(AlbumMeasurement {
                integrated_lufs: Some(-13.0),
                range_lu: 6.0,
                sample_peak: 0.95,
                true_peak: 1.1,
            }),
        };
        let changes = tag_changes(&measured).unwrap();
        let mut expected = TagChanges::new();
        expected.set_one(TagField::ReplayGainTrackGain, "-6.30 dB");
        expected.set_one(TagField::ReplayGainTrackPeak, "1.023457");
        expected.set_one(TagField::ReplayGainAlbumGain, "-5.00 dB");
        expected.set_one(TagField::ReplayGainAlbumPeak, "1.100000");
        assert_eq!(changes, expected);

        let silent = MeasuredLoudness {
            integrated_lufs: None,
            ..measured
        };
        assert!(tag_changes(&silent).is_none());
    }
}
