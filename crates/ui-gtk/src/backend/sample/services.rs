// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialogs' access to an invented core: settings kept in memory, a few
//! made-up output devices, a library folder, tags made from the sample
//! library, a scan that ends at once, and copies and duplicates that are only
//! forgotten when moved "to the trash". Nothing touches the disk or a sound
//! device.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use helikodu_model::audio::{DeviceCapabilities, SampleFormat, SampleRate};

use super::super::services::{
    Availability, BackendSelection, DatabaseInfo, Decoder, DecoderStatus, DeviceId, DeviceInfo,
    DeviceSelection, EditTrack, FamilyStatus, FileFacts, FolderState, FormatFamily, FormatReport,
    LibraryFolder, OutputBackendKind, OutputMode, OutputSelection, ScanStatus, Services, Settings,
    StorageUsage, TagEdit, TagField, TagStepResult, Versions, WriteResult,
};
use super::super::services::{
    CancelToken, FileProgress, FileRunId, FileRunSummary, FoundFile, MissingFile, MoveRequest,
    OrganizeOptions, PlannedMove, RelinkOutcome, RunReport, SavedPattern, VerifyReport,
    VerifyScope,
};
use super::super::services::{
    CopyGroup, DuplicateCounts, DuplicateGroup, FileId, KeepRule, MatchBy, TrashResult,
};
use super::super::services::{LimiterReading, LoudnessCoverage, LoudnessScope, LoudnessStatus};
use super::super::{BackendError, OutputInfo, SignalState, TrackKey, Update};
use super::SampleLibrary;
use super::files::SampleFiles;

/// The tag edits of the sample tracks, field by field.
type Edits = HashMap<TrackKey, BTreeMap<TagField, Vec<String>>>;

#[derive(Debug)]
struct MemoryState {
    settings: Arc<Settings>,
    output: OutputSelection,
    roots: Vec<PathBuf>,
    edits: HashMap<TrackKey, BTreeMap<TagField, Vec<String>>>,
    /// The tag edits as they were before each save, and before each undo.
    tag_undo: Vec<(usize, Edits)>,
    tag_redo: Vec<(usize, Edits)>,
    ratings: HashMap<TrackKey, Option<u8>>,
    lyrics: HashMap<TrackKey, String>,
    covers_removed: Vec<TrackKey>,
    /// When "Back up now" was last used.
    backup_made: Option<std::time::SystemTime>,
}

/// Invented settings, devices, folders and tags. See the module documentation.
#[derive(Debug)]
pub struct MemoryServices {
    library: Option<Arc<SampleLibrary>>,
    updates: Option<async_channel::Sender<Update>>,
    state: Mutex<MemoryState>,
    files: SampleFiles,
}

impl Default for MemoryServices {
    fn default() -> Self {
        Self::new(None, None, Vec::new())
    }
}

/// The music folder of the sample library.
fn music_folder() -> PathBuf {
    gtk::glib::home_dir().join("Music")
}

impl MemoryServices {
    fn new(
        library: Option<Arc<SampleLibrary>>,
        updates: Option<async_channel::Sender<Update>>,
        roots: Vec<PathBuf>,
    ) -> Self {
        let mut settings = Settings::default();
        settings.library.folders.clone_from(&roots);
        Self {
            library,
            updates,
            files: SampleFiles::new(music_folder()),
            state: Mutex::new(MemoryState {
                settings: Arc::new(settings),
                output: OutputSelection::default(),
                roots,
                edits: HashMap::new(),
                tag_undo: Vec::new(),
                tag_redo: Vec::new(),
                ratings: HashMap::new(),
                lyrics: HashMap::new(),
                covers_removed: Vec::new(),
                backup_made: None,
            }),
        }
    }

    /// Services over `library` with one library folder, reporting to `updates`.
    pub(super) fn for_library(
        library: Arc<SampleLibrary>,
        updates: async_channel::Sender<Update>,
    ) -> Self {
        Self::new(Some(library), Some(updates), vec![music_folder()])
    }

    /// The same services with no library folder, as before the first run.
    #[must_use]
    pub fn empty(self) -> Self {
        {
            let mut state = self.lock();
            state.roots.clear();
            let mut settings = (*state.settings).clone();
            settings.library.folders.clear();
            state.settings = Arc::new(settings);
        }
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, update: Update) {
        if let Some(updates) = &self.updates {
            let _ = updates.try_send(update);
        }
    }

    fn device_name(&self, selection: &OutputSelection) -> String {
        let backend = self.resolve_backend(selection.backend);
        match &selection.device {
            DeviceSelection::SystemDefault => "System default".into(),
            DeviceSelection::Device(id) => sample_devices(backend)
                .into_iter()
                .find(|d| d.id == *id)
                .map_or_else(|| id.as_str().to_owned(), |d| d.description),
        }
    }
}

fn device(
    backend: OutputBackendKind,
    id: &str,
    description: &str,
    is_default: bool,
    rates: &[u32],
) -> DeviceInfo {
    DeviceInfo {
        backend,
        id: DeviceId::new(id).unwrap_or_else(|_| DeviceId::new("default").expect("a valid name")),
        description: description.into(),
        is_default,
        capabilities: DeviceCapabilities {
            sample_rates: rates.iter().filter_map(|&hz| SampleRate::new(hz)).collect(),
            formats: vec![
                SampleFormat::S16(helikodu_model::audio::Endianness::Little),
                SampleFormat::S32(helikodu_model::audio::Endianness::Little),
            ],
            channel_counts: vec![2],
        },
    }
}

fn sample_devices(backend: OutputBackendKind) -> Vec<DeviceInfo> {
    match backend {
        OutputBackendKind::Alsa => vec![
            device(
                backend,
                "hw:CARD=PCH,DEV=0",
                "Built-in Audio",
                true,
                &[44_100, 48_000, 96_000, 192_000],
            ),
            device(
                backend,
                "hw:CARD=Studio,DEV=0",
                "Studio Interface",
                false,
                &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 384_000],
            ),
            device(
                backend,
                "hw:CARD=HDMI,DEV=3",
                "HDMI / DisplayPort 1",
                false,
                &[32_000, 44_100, 48_000],
            ),
        ],
        _ => vec![
            device(
                backend,
                "alsa_output.pci-0000_00_1f.3.analog-stereo",
                "Built-in Audio",
                true,
                &[44_100, 48_000, 96_000, 192_000],
            ),
            device(
                backend,
                "alsa_output.usb-Studio_Interface-00.analog-stereo",
                "Studio Interface",
                false,
                &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 384_000],
            ),
            device(
                backend,
                "alsa_output.pci-0000_01_00.1.hdmi-stereo",
                "HDMI / DisplayPort 1",
                false,
                &[32_000, 44_100, 48_000],
            ),
        ],
    }
}

impl Services for MemoryServices {
    fn export(&self) -> Arc<dyn super::super::export::ExportServices> {
        Arc::new(super::export::SampleExport {
            library: self.library.clone(),
        })
    }

    fn settings(&self) -> Arc<Settings> {
        self.lock().settings.clone()
    }

    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) -> Result<(), BackendError> {
        let mut state = self.lock();
        let mut settings = (*state.settings).clone();
        change(&mut settings);
        settings
            .validate()
            .map_err(|e| BackendError(e.to_string()))?;
        let output = settings.output.selection();
        let moved = output != state.output;
        let duplicates = settings.library.duplicates != state.settings.library.duplicates;
        state.settings = Arc::new(settings);
        if moved {
            state.output = output.clone();
        }
        drop(state);
        if moved {
            self.set_output(output);
        }
        if duplicates {
            // As the service applies the change in the background: the copies
            // are read (or their tracks removed), then the end is announced.
            let total = self.library.as_ref().map_or(0, |l| l.tracks.len() / 9);
            self.emit(Update::Scan(ScanStatus {
                done: total / 2,
                total: Some(total),
                ..ScanStatus::default()
            }));
            self.emit(Update::DuplicatesApplied(Ok(total)));
            self.emit(Update::LibraryChanged);
        }
        Ok(())
    }

    fn settings_file(&self) -> Option<PathBuf> {
        Some(
            gtk::glib::user_config_dir()
                .join("helikodu")
                .join("settings.toml"),
        )
    }

    fn output_backends(&self) -> Vec<OutputBackendKind> {
        vec![OutputBackendKind::PipeWire, OutputBackendKind::Alsa]
    }

    fn resolve_backend(&self, selection: BackendSelection) -> OutputBackendKind {
        match selection {
            BackendSelection::Automatic => OutputBackendKind::PipeWire,
            BackendSelection::Backend(kind) => kind,
        }
    }

    fn devices(&self, backend: OutputBackendKind) -> Result<Vec<DeviceInfo>, BackendError> {
        match backend {
            OutputBackendKind::PipeWire | OutputBackendKind::Alsa => Ok(sample_devices(backend)),
            other => Err(BackendError(format!(
                "this build has no {} output",
                other.as_str()
            ))),
        }
    }

    fn output(&self) -> OutputSelection {
        self.lock().output.clone()
    }

    fn set_output(&self, selection: OutputSelection) {
        let name = self.device_name(&selection);
        {
            let mut state = self.lock();
            state.output = selection.clone();
            let mut settings = (*state.settings).clone();
            settings.output.backend = selection.backend;
            settings.output.device = selection.device.clone();
            settings.output.mode = selection.mode;
            state.settings = Arc::new(settings);
        }
        let signal = if selection.mode == OutputMode::Exclusive {
            SignalState::BitPerfect
        } else {
            SignalState::Idle
        };
        self.emit(Update::Output(OutputInfo {
            device: name,
            signal,
            sample_rate: None,
            bits: None,
        }));
    }

    fn library_folders(&self) -> Result<Vec<LibraryFolder>, BackendError> {
        let state = self.lock();
        let tracks = self
            .library
            .as_ref()
            .map(|l| l.albums.iter().map(|a| a.tracks.len()).sum());
        Ok(state
            .roots
            .iter()
            .enumerate()
            .map(|(i, path)| LibraryFolder {
                path: path.clone(),
                state: if !state.settings.library.watch {
                    FolderState::Manual
                } else if path.starts_with("/mnt") {
                    FolderState::Polled(state.settings.library.rescan_interval_min)
                } else {
                    FolderState::Watched
                },
                tracks: if i == 0 { tracks } else { Some(0) },
            })
            .collect())
    }

    fn scan(&self, folders: &[PathBuf]) -> Result<(), BackendError> {
        {
            let mut state = self.lock();
            for folder in folders {
                if !state.roots.contains(folder) {
                    state.roots.push(folder.clone());
                }
            }
        }
        let total = self
            .library
            .as_ref()
            .map_or(0, |l| l.albums.iter().map(|a| a.tracks.len()).sum());
        self.emit(Update::Scan(ScanStatus {
            current: folders.first().cloned(),
            done: total / 8,
            total: Some(total),
            finished: false,
            error: None,
        }));
        self.emit(Update::Scan(ScanStatus {
            current: None,
            done: total,
            total: Some(total),
            finished: true,
            error: None,
        }));
        self.emit(Update::LibraryChanged);
        Ok(())
    }

    fn database(&self) -> Option<DatabaseInfo> {
        Some(DatabaseInfo {
            path: gtk::glib::user_data_dir()
                .join("helikodu")
                .join("library.db"),
            size: Some(412 << 20),
        })
    }

    fn storage(&self) -> StorageUsage {
        StorageUsage {
            covers: Some(1_288_490_189),
            analysis: Some(310 << 20),
            logs: Some(12 << 20),
            undo: Some(38 << 20),
        }
    }

    fn edit_tracks(&self, tracks: &[TrackKey]) -> Result<Vec<EditTrack>, BackendError> {
        let Some(library) = &self.library else {
            return Ok(Vec::new());
        };
        let state = self.lock();
        let mut found = Vec::new();
        for key in tracks {
            for album in &library.albums {
                let Some(track) = album.tracks.iter().find(|t| t.key == *key) else {
                    continue;
                };
                let total = album.tracks.len();
                let number = track.number.unwrap_or(0);
                let mut fields = BTreeMap::new();
                let year = album.row.year.map(|y| y.to_string()).unwrap_or_default();
                for (field, value) in [
                    (TagField::Title, track.title.clone()),
                    (TagField::Artist, track.artist.clone()),
                    (TagField::AlbumArtist, album.row.artist.clone()),
                    (TagField::Album, album.row.title.clone()),
                    (TagField::Track, number.to_string()),
                    (TagField::TrackTotal, total.to_string()),
                    (TagField::Disc, track.disc.unwrap_or(1).to_string()),
                    (TagField::DiscTotal, "1".into()),
                    (TagField::Year, year.clone()),
                    (TagField::Genre, super::GENRES[album.genre].to_string()),
                    (TagField::Composer, album.row.artist.clone()),
                ] {
                    fields.insert(field, vec![value]);
                }
                if number % 2 == 1 {
                    fields.insert(TagField::Comment, vec!["Recorded live".into()]);
                }
                if let Some(edits) = state.edits.get(key) {
                    for (field, values) in edits {
                        if values.is_empty() {
                            fields.remove(field);
                        } else {
                            fields.insert(field.clone(), values.clone());
                        }
                    }
                }
                let path = track.path.clone().unwrap_or_else(|| {
                    album
                        .folder
                        .join(format!("{number:02} {}.flac", track.title))
                });
                let codec = album.row.format.codec.as_deref().unwrap_or_default();
                let tag_format = match codec {
                    "MP3" => "ID3v2.4",
                    "AAC" | "ALAC" => "MP4",
                    _ => "Vorbis comments",
                };
                found.push(EditTrack {
                    key: *key,
                    path,
                    fields,
                    raw: BTreeMap::new(),
                    other: vec![
                        ("LABEL".into(), "Saar Records".into()),
                        ("CATALOGNUMBER".into(), format!("SR-{:03}", album.row.key.0)),
                        ("REPLAYGAIN_ALBUM_GAIN".into(), "-5.82 dB".into()),
                    ],
                    rating: state.ratings.get(key).copied().unwrap_or(track.rating),
                    cover: if state.covers_removed.contains(key) {
                        None
                    } else {
                        album.row.cover
                    },
                    cover_seed: album.row.cover_seed,
                    lyrics: state.lyrics.get(key).cloned(),
                    read_only: None,
                    facts: FileFacts {
                        size: Some(28_400_000),
                        modified: Some("12 Oct 2026, 14:02".into()),
                        format: album.row.format.clone(),
                        duration: track.duration,
                        encoder: (codec == "FLAC").then(|| "reference libFLAC 1.4.3".into()),
                        tag_formats: vec![tag_format.into()],
                        pictures: 2,
                        track_gain_db: Some(-4.1),
                        track_peak: Some(0.92),
                        album_gain_db: Some(-5.8),
                        album_peak: Some(0.98),
                        added: Some("2 Mar 2024".into()),
                        last_played: Some("9 Nov 2026".into()),
                        plays: Some(u64::from(track.plays)),
                        skips: Some(1),
                    },
                });
                break;
            }
        }
        Ok(found)
    }

    fn write_tags(&self, edit: &TagEdit) -> Vec<WriteResult> {
        let names: HashMap<TrackKey, String> = self
            .edit_tracks(&edit.tracks)
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.key, t.file_name()))
            .collect();
        let mut state = self.lock();
        if edit.touches_files() {
            let before = state.edits.clone();
            state.tag_undo.push((edit.tracks.len(), before));
            state.tag_redo.clear();
        }
        edit.tracks
            .iter()
            .map(|key| {
                let entry = state.edits.entry(*key).or_default();
                for (field, values) in edit.fields_for(*key) {
                    entry.insert(field, values);
                }
                if let Some(rating) = edit.rating {
                    state.ratings.insert(*key, rating);
                }
                if let Some(lyrics) = &edit.lyrics {
                    state.lyrics.insert(*key, lyrics.clone());
                }
                if let Some(super::super::services::CoverEdit::Remove) = &edit.cover {
                    state.covers_removed.push(*key);
                }
                WriteResult {
                    track: *key,
                    file: names.get(key).cloned().unwrap_or_default(),
                    error: None,
                }
            })
            .collect()
    }

    fn cover_picture(&self, _track: TrackKey) -> Option<Vec<u8>> {
        None
    }

    fn step_tags(&self, redo: bool) -> Result<Option<TagStepResult>, BackendError> {
        let mut state = self.lock();
        let popped = if redo {
            state.tag_redo.pop()
        } else {
            state.tag_undo.pop()
        };
        let Some((files, edits)) = popped else {
            return Ok(None);
        };
        let now = std::mem::replace(&mut state.edits, edits);
        if redo {
            state.tag_undo.push((files, now));
        } else {
            state.tag_redo.push((files, now));
        }
        Ok(Some(TagStepResult {
            files,
            problems: Vec::new(),
        }))
    }

    fn formats(&self) -> FormatReport {
        let built = [Decoder::Ffmpeg, Decoder::Dsd, Decoder::Rustysynth];
        let soundfont = self.lock().settings.playback.decoders.soundfont.is_some();
        let availability = |decoder: Decoder| {
            if !built.contains(&decoder) {
                Availability::NotBuilt
            } else if decoder == Decoder::Rustysynth && !soundfont {
                Availability::NeedsSoundFont
            } else {
                Availability::Available
            }
        };
        FormatReport {
            decoders: Decoder::ALL
                .iter()
                .map(|&decoder| DecoderStatus {
                    decoder,
                    availability: availability(decoder),
                    version: match decoder {
                        Decoder::Ffmpeg => Some("9.0.2".into()),
                        _ => None,
                    },
                })
                .collect(),
            families: FormatFamily::ALL
                .iter()
                .map(|&family| {
                    let decoder = family
                        .decoders()
                        .iter()
                        .copied()
                        .find(|d| built.contains(d))
                        .unwrap_or(family.decoders()[0]);
                    FamilyStatus {
                        family,
                        decoder,
                        availability: availability(decoder),
                    }
                })
                .collect(),
        }
    }

    fn loudness_coverage(&self) -> Option<LoudnessCoverage> {
        let tracks = self
            .library
            .as_ref()
            .map_or(0, |l| l.albums.iter().map(|a| a.tracks.len()).sum());
        Some(LoudnessCoverage {
            tracks,
            with_values: tracks - tracks / 12,
        })
    }

    fn measure_loudness(&self, scope: LoudnessScope) -> Result<(), BackendError> {
        let total = match scope {
            LoudnessScope::Tracks(keys) => keys.len(),
            LoudnessScope::Rest => self
                .loudness_coverage()
                .map_or(0, |c| c.tracks - c.with_values),
        };
        self.emit(Update::Loudness(LoudnessStatus {
            total: Some(total),
            done: total / 3,
            ..LoudnessStatus::default()
        }));
        self.emit(Update::Loudness(LoudnessStatus {
            total: Some(total),
            done: total,
            finished: true,
            ..LoudnessStatus::default()
        }));
        self.emit(Update::LibraryChanged);
        Ok(())
    }

    fn stop_loudness(&self) {}

    fn is_measuring_loudness(&self) -> bool {
        false
    }

    fn limiter_activity(&self) -> Option<LimiterReading> {
        Some(LimiterReading {
            reduction_db: 0.0,
            largest_db: 1.8,
            times: 3,
        })
    }

    fn copies_of(&self, track: TrackKey) -> Result<Option<CopyGroup>, BackendError> {
        Ok(self
            .library
            .as_ref()
            .and_then(|l| l.sample_copies_of(track)))
    }

    fn set_main_copy(&self, file: FileId) -> Result<(), BackendError> {
        let library = self
            .library
            .as_ref()
            .ok_or_else(|| BackendError("there is no library".into()))?;
        library.set_sample_main_copy(file).map_err(BackendError)?;
        self.emit(Update::LibraryChanged);
        Ok(())
    }

    fn trash_files(
        &self,
        files: &[FileId],
        copies: bool,
    ) -> Result<Vec<TrashResult>, BackendError> {
        let library = self
            .library
            .as_ref()
            .ok_or_else(|| BackendError("there is no library".into()))?;
        let results = library.sample_trash(files, copies);
        self.emit(Update::LibraryChanged);
        Ok(results)
    }

    fn find_duplicates(
        &self,
        by: MatchBy,
        keep: KeepRule,
    ) -> Result<Vec<DuplicateGroup>, BackendError> {
        Ok(self
            .library
            .as_ref()
            .map(|l| l.sample_duplicates(by, keep))
            .unwrap_or_default())
    }

    fn duplicate_counts(&self) -> Result<DuplicateCounts, BackendError> {
        Ok(self
            .library
            .as_ref()
            .map(|l| l.sample_duplicate_counts())
            .unwrap_or_default())
    }

    fn plan_organize(
        &self,
        tracks: Option<&[TrackKey]>,
        options: &OrganizeOptions,
    ) -> Result<Vec<PlannedMove>, BackendError> {
        let _ = tracks;
        self.files.plan(options)
    }

    fn organize(
        &self,
        moves: &[MoveRequest],
        pattern: Option<&str>,
        companions: bool,
        progress: FileProgress<'_>,
        cancel: &CancelToken,
    ) -> Result<RunReport, BackendError> {
        let _ = (companions, cancel);
        let report = self.files.organize(moves, pattern);
        progress(moves.len(), moves.len());
        self.emit(Update::LibraryChanged);
        Ok(report)
    }

    fn last_file_run(&self) -> Result<Option<FileRunSummary>, BackendError> {
        Ok(self.files.last_run())
    }

    fn step_file_run(
        &self,
        run: FileRunId,
        redo: bool,
        progress: FileProgress<'_>,
    ) -> Result<RunReport, BackendError> {
        let report = self.files.step(run, redo)?;
        progress(report.outcomes.len(), report.outcomes.len());
        self.emit(Update::LibraryChanged);
        Ok(report)
    }

    fn rename_track_file(&self, track: TrackKey, name: &str) -> Result<RunReport, BackendError> {
        let report = self.files.rename(track, name)?;
        self.emit(Update::LibraryChanged);
        Ok(report)
    }

    fn trash_tracks(&self, tracks: &[TrackKey]) -> Result<Vec<TrashResult>, BackendError> {
        self.emit(Update::LibraryChanged);
        Ok(tracks
            .iter()
            .map(|t| TrashResult {
                file: FileId::from_raw(i64::try_from(t.0).unwrap_or(0)),
                path: self
                    .library
                    .as_ref()
                    .and_then(|l| l.track(*t))
                    .and_then(|r| r.path.clone()),
                problem: None,
            })
            .collect())
    }

    fn naming_patterns(&self) -> Result<Vec<SavedPattern>, BackendError> {
        Ok(self.files.patterns())
    }

    fn save_naming_pattern(
        &self,
        id: Option<i64>,
        name: &str,
        pattern: &str,
    ) -> Result<i64, BackendError> {
        Ok(self.files.save_pattern(id, name, pattern))
    }

    fn delete_naming_pattern(&self, id: i64) -> Result<(), BackendError> {
        self.files.delete_pattern(id);
        Ok(())
    }

    fn missing_files(&self) -> Result<Vec<MissingFile>, BackendError> {
        Ok(self.files.missing())
    }

    fn find_missing(
        &self,
        files: &[FileId],
        folder: Option<&std::path::Path>,
        cancel: &CancelToken,
    ) -> Result<Vec<FoundFile>, BackendError> {
        let _ = cancel;
        Ok(self.files.find(files, folder))
    }

    fn relink_missing(&self, found: &[FoundFile]) -> Result<Vec<RelinkOutcome>, BackendError> {
        let outcomes = self.files.relink(found);
        self.emit(Update::LibraryChanged);
        Ok(outcomes)
    }

    fn remove_missing(&self, files: &[FileId]) -> Result<usize, BackendError> {
        let removed = self.files.remove(files);
        self.emit(Update::LibraryChanged);
        Ok(removed)
    }

    fn verify_targets(&self, scope: &VerifyScope) -> Result<Vec<PathBuf>, BackendError> {
        let all = self.files.verify_targets();
        Ok(match scope {
            VerifyScope::Folder(folder) => {
                all.into_iter().filter(|p| p.starts_with(folder)).collect()
            }
            VerifyScope::Tracks(keys) => all.into_iter().take(keys.len()).collect(),
            VerifyScope::Library => all,
        })
    }

    fn verify_files(
        &self,
        paths: &[PathBuf],
        cancelled: &(dyn Fn() -> bool + Sync),
        found: &(dyn Fn(usize, VerifyReport) + Sync),
    ) {
        for (i, path) in paths.iter().enumerate() {
            if cancelled() {
                return;
            }
            found(i, SampleFiles::verify(path));
        }
    }

    fn track_flags(&self, track: TrackKey) -> Option<helikodu_model::player::TrackFlags> {
        let library = self.library.as_ref()?;
        library.track(track)?;
        let flags = library.flags.lock().unwrap_or_else(PoisonError::into_inner);
        Some(flags.get(&track).copied().unwrap_or_default())
    }

    fn backups(&self) -> Option<super::super::services::BackupsInfo> {
        self.library.as_ref()?;
        let made = self.lock().backup_made;
        Some(super::super::services::BackupsInfo {
            folder: gtk::glib::user_data_dir().join("helikodu").join("backups"),
            kept: 7,
            daily: true,
            latest: made,
            check: made.map(|at| (at, None)),
            restore_pending: false,
            read_only: false,
        })
    }

    fn back_up_now(&self) -> Result<super::super::services::BackupsInfo, BackendError> {
        if self.library.is_none() {
            return Err(BackendError(crate::i18n::tr(
                "Helikodu runs without a library.",
            )));
        }
        self.lock().backup_made = Some(std::time::SystemTime::now());
        self.backups()
            .ok_or_else(|| BackendError(crate::i18n::tr("Helikodu runs without a library.")))
    }

    fn versions(&self) -> Versions {
        Versions {
            ffmpeg: Some("9.0.2".into()),
            pipewire: Some("1.6.2".into()),
            sqlite: Some("3.46.1".into()),
        }
    }
}
