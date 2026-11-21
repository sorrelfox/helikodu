// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the dialogs need from the core besides the library lists and the
//! player: the settings store, the output devices, the library folders and
//! their scans, tag editing, copies and the duplicate finder with the desktop
//! trash, and the facts for Supported formats and About.
//!
//! [`Services`] is implemented over the running service (`service.rs`) and
//! over invented content (`sample.rs`). Methods marked "blocking" are called
//! on worker threads only; the others are quick and may be called on the main
//! thread.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub use helikodu_model::audio::{
    BackendSelection, DeviceId, DeviceInfo, DeviceKey, DeviceSelection, OutputBackendKind,
    OutputMode, OutputSelection,
};
pub use helikodu_model::settings::Settings;
pub use helikodu_service::formats::{
    Availability, Decoder, DecoderStatus, FamilyStatus, FormatFamily, FormatReport,
};
pub use helikodu_service::jobs::verify::{VerifiedFormat, VerifyProblem, VerifyReport};
pub use helikodu_service::library::{AnalysisProgress, AnalysisState, PauseReason};
pub use helikodu_service::library::{
    CancelToken, Change, CharacterSet, Destination, FileRunId, FileRunSummary, FoundFile,
    MissingFile, MoveOutcome, MoveProblem, MoveRequest, NamingPattern, OrganizeOptions,
    PatternError, PatternProblem, PlannedMove, RelinkOutcome, RelinkProblem, RunKind, RunReport,
    SavedPattern,
};
pub use helikodu_service::library::{
    CopyFile, CopyGroup, DuplicateCounts, DuplicateEntry, DuplicateGroup, FileId, KeepRule, MatchBy,
};
pub use helikodu_service::library::{
    ItemAction, ItemKind, ItemResult, ManagedItem, ManagedOperation, ManagedOptions,
    ManagedOutcome, ManagedPlan, ManagedReport, ManagedRunId, ManagedRunSummary, SkipReason,
    Transfer,
};

use super::{AudioFormat, BackendError, CoverKey, TrackEqualizer, TrackKey};

/// How a library folder is kept current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderState {
    /// Changes are noticed as they happen.
    Watched,
    /// It cannot be watched and is scanned again every so many minutes.
    Polled(u32),
    /// Only scanned when asked.
    Manual,
    /// Its drive is not connected.
    NotConnected,
}

/// One library folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFolder {
    /// The folder.
    pub path: PathBuf,
    /// How it is kept current.
    pub state: FolderState,
    /// The tracks in it, when known.
    pub tracks: Option<usize>,
}

/// Where a scan stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanStatus {
    /// The folder or file being read.
    pub current: Option<PathBuf>,
    /// The files read so far.
    pub done: usize,
    /// The files to read, once the folders are walked.
    pub total: Option<usize>,
    /// The scan is over.
    pub finished: bool,
    /// Why it failed, if it did.
    pub error: Option<String>,
}

impl ScanStatus {
    /// The share done, from 0 to 1, once it is known.
    pub fn fraction(&self) -> Option<f64> {
        match self.total {
            Some(0) => Some(1.0),
            Some(total) => Some((self.done as f64 / total as f64).clamp(0.0, 1.0)),
            None => None,
        }
    }
}

/// The size of a cache or store, as far as it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StorageUsage {
    /// Cover thumbnails, in bytes.
    pub covers: Option<u64>,
    /// The waveforms of the seek bar, in bytes.
    pub analysis: Option<u64>,
    /// Logs, in bytes.
    pub logs: Option<u64>,
    /// Undo history, in bytes.
    pub undo: Option<u64>,
}

/// The library database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    /// Its file.
    pub path: PathBuf,
    /// Its size in bytes.
    pub size: Option<u64>,
}

/// The backups of the library database, for Settings › Advanced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupsInfo {
    /// The backups folder.
    pub folder: PathBuf,
    /// How many backups are kept.
    pub kept: usize,
    /// Whether one is made by itself every day.
    pub daily: bool,
    /// When the newest backup was made.
    pub latest: Option<SystemTime>,
    /// When the database was last checked, and what was found wrong, if anything.
    pub check: Option<(SystemTime, Option<String>)>,
    /// Whether a backup waits to be put back at the next start.
    pub restore_pending: bool,
    /// Whether the database can only be read.
    pub read_only: bool,
}

/// A crash report not shown yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashNotice {
    /// The report file.
    pub path: PathBuf,
    /// The folder of the reports.
    pub folder: PathBuf,
    /// The panic message.
    pub summary: String,
    /// The whole report.
    pub text: String,
}

/// A tag field the editor shows in its form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TagField {
    /// The title.
    Title,
    /// The track artists.
    Artist,
    /// The album artists.
    AlbumArtist,
    /// The album.
    Album,
    /// The track number.
    Track,
    /// The number of tracks.
    TrackTotal,
    /// The disc number.
    Disc,
    /// The number of discs.
    DiscTotal,
    /// The year or date.
    Year,
    /// The genres.
    Genre,
    /// The composers.
    Composer,
    /// The comment.
    Comment,
    /// Any other field, by its name in the file.
    Custom(String),
}

impl TagField {
    /// The fields of the form, in its order.
    pub const FORM: [Self; 12] = [
        Self::Title,
        Self::Artist,
        Self::AlbumArtist,
        Self::Album,
        Self::Track,
        Self::TrackTotal,
        Self::Disc,
        Self::DiscTotal,
        Self::Year,
        Self::Genre,
        Self::Composer,
        Self::Comment,
    ];

    /// Whether the field holds several values (shown as chips).
    pub fn is_list(&self) -> bool {
        matches!(self, Self::Genre)
    }
}

/// Facts about one file, for the File info tab.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileFacts {
    /// Its size in bytes.
    pub size: Option<u64>,
    /// When it last changed, as the desktop writes dates.
    pub modified: Option<String>,
    /// Its audio format.
    pub format: AudioFormat,
    /// Its length.
    pub duration: Option<Duration>,
    /// The encoder named in its tags.
    pub encoder: Option<String>,
    /// Its tag formats, such as `Vorbis comments` or `ID3v2.4`.
    pub tag_formats: Vec<String>,
    /// How many pictures it carries.
    pub pictures: usize,
    /// Track gain in dB.
    pub track_gain_db: Option<f64>,
    /// Track peak.
    pub track_peak: Option<f64>,
    /// Album gain in dB.
    pub album_gain_db: Option<f64>,
    /// Album peak.
    pub album_peak: Option<f64>,
    /// When it entered the library.
    pub added: Option<String>,
    /// When it was last played.
    pub last_played: Option<String>,
    /// How often it was played.
    pub plays: Option<u64>,
    /// How often it was skipped.
    pub skips: Option<u64>,
    /// What the track analysis found.
    pub analysis: AnalysisFacts,
}

/// What the track analysis found for one track, for the File info tab.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum AnalysisFacts {
    /// It was not analysed, or its file changed since.
    #[default]
    NotYet,
    /// Its file could not be read for the analysis.
    Failed,
    /// The analysis.
    Analysed(Arc<helikodu_model::analysis::TrackAnalysis>),
}

/// Another player whose library was found on this computer, for the offer to
/// import its plays and ratings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundPlayer {
    /// The short name the import uses: `rhythmbox`, `strawberry`, …
    pub id: String,
    /// The player's name, as it calls itself.
    pub name: String,
}

/// One track as the tag editor opens it.
#[derive(Debug, Clone, PartialEq)]
pub struct EditTrack {
    /// The track.
    pub key: TrackKey,
    /// Its file.
    pub path: PathBuf,
    /// The values of the form's fields; a missing field has no value.
    pub fields: BTreeMap<TagField, Vec<String>>,
    /// The other fields, by their names in the file.
    pub other: Vec<(String, String)>,
    /// The form's fields as the file stores them, where that differs from
    /// `fields` because garbled text was repaired on reading.
    pub raw: BTreeMap<TagField, Vec<String>>,
    /// The rating in whole stars.
    pub rating: Option<u8>,
    /// The front cover.
    pub cover: Option<CoverKey>,
    /// A stable value that picks the generated cover.
    pub cover_seed: u64,
    /// The lyrics in the tags.
    pub lyrics: Option<String>,
    /// Why its tags cannot be written, if they cannot.
    pub read_only: Option<String>,
    /// Facts for the File info tab.
    pub facts: FileFacts,
}

impl EditTrack {
    /// The values of `field`.
    pub fn values(&self, field: &TagField) -> &[String] {
        self.fields.get(field).map_or(&[], Vec::as_slice)
    }

    /// The file name.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// A new front cover, or none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverEdit {
    /// This image file (JPEG or PNG) becomes the front cover.
    Set(PathBuf),
    /// The front cover is removed.
    Remove,
}

/// The changes the tag editor saves into a selection of tracks. A field that
/// is not named keeps each file's value; an empty list of values clears it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TagEdit {
    /// The tracks.
    pub tracks: Vec<TrackKey>,
    /// New values, field by field.
    pub fields: BTreeMap<TagField, Vec<String>>,
    /// A new cover.
    pub cover: Option<CoverEdit>,
    /// New lyrics; empty removes them.
    pub lyrics: Option<String>,
    /// A new rating in whole stars, or none.
    pub rating: Option<Option<u8>>,
    /// Values that differ per track (from the tag tools), field by field; a
    /// field named in `fields` replaces them.
    pub per_track: BTreeMap<TrackKey, BTreeMap<TagField, Vec<String>>>,
    /// Whether a new cover stays out of the files ("Keep the cover: Inside
    /// each file" unticked).
    pub cover_not_inside: bool,
    /// Whether a new cover is also saved as `cover.jpg` (or `cover.png`) in
    /// the folder of each file, where no such file is yet.
    pub cover_in_folder: bool,
}

impl TagEdit {
    /// Whether anything is changed in the files themselves (the rating is
    /// kept in the library).
    pub fn touches_files(&self) -> bool {
        !self.fields.is_empty()
            || (self.cover.is_some() && !self.cover_not_inside)
            || self.lyrics.is_some()
            || self.per_track.values().any(|f| !f.is_empty())
    }

    /// The field values written into `track`: its own values from the tag
    /// tools, then the values for all.
    pub fn fields_for(&self, track: TrackKey) -> BTreeMap<TagField, Vec<String>> {
        let mut fields = self.per_track.get(&track).cloned().unwrap_or_default();
        for (field, values) in &self.fields {
            fields.insert(field.clone(), values.clone());
        }
        fields
    }

    /// Whether anything is changed at all.
    pub fn is_empty(&self) -> bool {
        !self.touches_files()
            && self.rating.is_none()
            && !(self.cover.is_some() && self.cover_in_folder)
    }
}

/// What happened to one file of a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    /// The track.
    pub track: TrackKey,
    /// Its file name.
    pub file: String,
    /// Why it failed, if it did.
    pub error: Option<String>,
}

/// What undoing or doing again a tag edit did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TagStepResult {
    /// The files of the edit.
    pub files: usize,
    /// The files left as they were, by name, and why.
    pub problems: Vec<(String, String)>,
}

/// What a step of the shared undo history (tag edits and file runs) did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryStepResult {
    /// A tag edit was undone or done again.
    Tags(TagStepResult),
    /// A run of the file organizer (or a file renamed) was undone or done again.
    Files {
        /// What kind of run.
        kind: RunKind,
        /// What happened to its files.
        report: RunReport,
    },
}

/// How many tracks have loudness values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoudnessCoverage {
    /// The tracks whose files are present.
    pub tracks: usize,
    /// Those with values from their tags or measured.
    pub with_values: usize,
}

/// The tracks a loudness measurement covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoudnessScope {
    /// These tracks; an album gets album values when all its tracks are
    /// among them.
    Tracks(Vec<TrackKey>),
    /// Every track without a current measurement ("Measure the rest").
    Rest,
}

/// Where a loudness measurement stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoudnessStatus {
    /// The tracks measured (or failed) so far.
    pub done: usize,
    /// The tracks to measure, once known.
    pub total: Option<usize>,
    /// The measurement is over.
    pub finished: bool,
    /// The tracks that could not be measured.
    pub failed: usize,
    /// It was stopped before the end.
    pub cancelled: bool,
    /// Why it failed, if it did.
    pub error: Option<String>,
}

impl LoudnessStatus {
    /// The share done, from 0 to 1, once it is known.
    pub fn fraction(&self) -> Option<f64> {
        match self.total {
            Some(0) => Some(1.0),
            Some(total) => Some((self.done as f64 / total as f64).clamp(0.0, 1.0)),
            None => None,
        }
    }
}

/// What the limiter did, for its editor.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LimiterReading {
    /// The gain reduction now, in dB (0 or positive).
    pub reduction_db: f64,
    /// The largest reduction in this track, in dB.
    pub largest_db: f64,
    /// How often it limited in this track.
    pub times: u64,
}

/// Versions of the libraries in use, for About and the diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Versions {
    /// FFmpeg.
    pub ffmpeg: Option<String>,
    /// PipeWire.
    pub pipewire: Option<String>,
    /// SQLite.
    pub sqlite: Option<String>,
}

/// Why a file asked to go to the trash stayed where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashProblem {
    /// It changed since the library read it, or is no longer identical to the
    /// main copy.
    Changed,
    /// It is the same file as another path in the library (a hard link, a
    /// folder mounted twice).
    SameFile,
    /// It is the main copy of its group.
    MainCopy,
    /// It is missing.
    Missing,
    /// The trash did not take it, or the library could not check it; the text
    /// says why.
    Failed(String),
}

/// What happened to one file asked to go to the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashResult {
    /// The file.
    pub file: FileId,
    /// Its path, if the library knew it.
    pub path: Option<PathBuf>,
    /// Why it stayed, or `None` when it is in the trash now.
    pub problem: Option<TrashProblem>,
}

/// The files a check of files reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyScope {
    /// Every file of the library.
    Library,
    /// The library files in this folder and below.
    Folder(PathBuf),
    /// The files of these tracks.
    Tracks(Vec<TrackKey>),
}

/// How a long file operation reports its progress: done and total.
pub type FileProgress<'a> = &'a (dyn Fn(usize, usize) + Sync);

/// What a plan of the managed folder takes in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedSource {
    /// These files and folders from outside the library.
    Add(Vec<PathBuf>),
    /// The library's own files outside the managed folder.
    Gather,
}

/// The dialogs' access to the core. See the module documentation.
pub trait Services: fmt::Debug + Send + Sync {
    /// The settings as they are now.
    fn settings(&self) -> Arc<Settings>;
    /// Changes the settings with `change`, validates and saves them. Nothing is
    /// changed when the result is an error; its text names the problem.
    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) -> Result<(), BackendError>;
    /// The settings file, if there is one.
    fn settings_file(&self) -> Option<PathBuf>;

    /// The output backends this build has that play sound.
    fn output_backends(&self) -> Vec<OutputBackendKind>;
    /// The backend `selection` stands for now.
    fn resolve_backend(&self, selection: BackendSelection) -> OutputBackendKind;
    /// The devices of `backend`. Blocking.
    fn devices(&self, backend: OutputBackendKind) -> Result<Vec<DeviceInfo>, BackendError>;
    /// Where the player plays now.
    fn output(&self) -> OutputSelection;
    /// Moves playback to `selection`.
    fn set_output(&self, selection: OutputSelection);

    /// The library folders. Blocking.
    fn library_folders(&self) -> Result<Vec<LibraryFolder>, BackendError>;
    /// Scans `folders`, adding them to the library; every library folder when
    /// empty. Progress arrives as [`Update::Scan`](super::Update::Scan).
    fn scan(&self, folders: &[PathBuf]) -> Result<(), BackendError>;
    /// The library database.
    fn database(&self) -> Option<DatabaseInfo>;
    /// The sizes of the caches and stores. Blocking.
    fn storage(&self) -> StorageUsage;

    /// The tracks `tracks` as the tag editor shows them. Blocking.
    fn edit_tracks(&self, tracks: &[TrackKey]) -> Result<Vec<EditTrack>, BackendError>;
    /// Saves `edit`, file by file, as one step of the tag undo journal.
    /// Blocking.
    fn write_tags(&self, edit: &TagEdit) -> Vec<WriteResult>;
    /// The bytes of the front cover inside the file of `track`, as the tag
    /// editor opened it. Blocking.
    fn cover_picture(&self, track: TrackKey) -> Option<Vec<u8>>;
    /// Undoes the newest saved tag edit, or with `redo` does the newest undone
    /// one again; `None` when there is none. Blocking.
    fn step_tags(&self, redo: bool) -> Result<Option<TagStepResult>, BackendError>;
    /// Undoes the newest tag edit or file run, whichever came last, or with
    /// `redo` does the one undone last again; `None` when there is none.
    /// Blocking.
    fn step_history(&self, redo: bool) -> Result<Option<HistoryStepResult>, BackendError> {
        Ok(self.step_tags(redo)?.map(HistoryStepResult::Tags))
    }

    /// What this build can play.
    fn formats(&self) -> FormatReport;
    /// Versions of the libraries in use.
    fn versions(&self) -> Versions;

    /// How many tracks have loudness values, and how many tracks there are.
    /// Blocking.
    fn loudness_coverage(&self) -> Option<LoudnessCoverage>;
    /// Starts measuring the loudness of `scope` in the background, stopping
    /// an earlier measurement. Progress arrives as
    /// [`Update::Loudness`](super::Update::Loudness). Blocking: it waits for an
    /// earlier measurement to stop.
    fn measure_loudness(&self, scope: LoudnessScope) -> Result<(), BackendError>;
    /// Stops the running loudness measurement; what it finished stays stored.
    /// Blocking.
    fn stop_loudness(&self);
    /// Whether a loudness measurement runs.
    fn is_measuring_loudness(&self) -> bool;
    /// How far the track analysis of the library is, and what the background
    /// analysis does; `None` without a library. Later changes arrive as
    /// [`Update::Analysis`](super::Update::Analysis). Blocking.
    fn analysis_progress(&self) -> Option<AnalysisProgress> {
        None
    }
    /// What the limiter did lately, when the player reports it.
    fn limiter_activity(&self) -> Option<LimiterReading>;

    /// The group of byte-identical copies of the file of `track`, the main
    /// copy first; `None` when it has no copies. Blocking.
    fn copies_of(&self, track: TrackKey) -> Result<Option<CopyGroup>, BackendError>;
    /// Makes `file` the main copy of its group. Blocking.
    fn set_main_copy(&self, file: FileId) -> Result<(), BackendError>;
    /// Moves `files`, which the owner confirmed, to the desktop trash, never
    /// deleting one: copies of a group (`copies`, checked to be identical to
    /// the main copy right before) or files the duplicate finder found by
    /// another measure. Each file's result says whether it went. Blocking.
    fn trash_files(&self, files: &[FileId], copies: bool)
    -> Result<Vec<TrashResult>, BackendError>;
    /// The groups of duplicates by `by`, with the files `keep` suggests moving
    /// to the trash marked. Blocking.
    fn find_duplicates(
        &self,
        by: MatchBy,
        keep: KeepRule,
    ) -> Result<Vec<DuplicateGroup>, BackendError>;
    /// How many groups each measure of the duplicate finder finds. Blocking.
    fn duplicate_counts(&self) -> Result<DuplicateCounts, BackendError>;

    /// The converter, folder sync and the cover manager.
    fn export(&self) -> Arc<dyn super::export::ExportServices>;
    /// The fields the user defines, for Your fields.
    fn user_fields(&self) -> Arc<dyn super::fields::FieldServices> {
        Arc::new(super::fields::NoFields)
    }
    /// The organizer's preview of the files of `tracks` (the whole library when
    /// `None`). Blocking.
    fn plan_organize(
        &self,
        tracks: Option<&[TrackKey]>,
        options: &OrganizeOptions,
    ) -> Result<Vec<PlannedMove>, BackendError> {
        let _ = (tracks, options);
        Err(no_files())
    }
    /// Moves `moves` as one run of the undo journal; with `companions`, covers
    /// and booklets go along. Announces the change. Blocking, for a thread of
    /// its own.
    fn organize(
        &self,
        moves: &[MoveRequest],
        pattern: Option<&str>,
        companions: bool,
        progress: FileProgress<'_>,
        cancel: &CancelToken,
    ) -> Result<RunReport, BackendError> {
        let _ = (moves, pattern, companions, progress, cancel);
        Err(no_files())
    }
    /// The newest run of the undo journal. Blocking.
    fn last_file_run(&self) -> Result<Option<FileRunSummary>, BackendError> {
        Ok(None)
    }
    /// Undoes (or with `redo` does again) the newest run. Announces the change.
    /// Blocking, for a thread of its own.
    fn step_file_run(
        &self,
        run: FileRunId,
        redo: bool,
        progress: FileProgress<'_>,
    ) -> Result<RunReport, BackendError> {
        let _ = (run, redo, progress);
        Err(no_files())
    }
    /// Renames the file of `track` within its folder, as a run that can be
    /// undone. Announces the change. Blocking.
    fn rename_track_file(&self, track: TrackKey, name: &str) -> Result<RunReport, BackendError> {
        let _ = (track, name);
        Err(no_files())
    }
    /// Moves the files of `tracks`, which the owner confirmed, to the desktop
    /// trash, each after checking that it is unchanged and not the same file as
    /// another library path. Blocking.
    fn trash_tracks(&self, tracks: &[TrackKey]) -> Result<Vec<TrashResult>, BackendError> {
        let _ = tracks;
        Err(no_files())
    }
    /// The organizer's saved patterns. Blocking.
    fn naming_patterns(&self) -> Result<Vec<SavedPattern>, BackendError> {
        Ok(Vec::new())
    }
    /// Saves a pattern (a new one without `id`); returns its id. Blocking.
    fn save_naming_pattern(
        &self,
        id: Option<i64>,
        name: &str,
        pattern: &str,
    ) -> Result<i64, BackendError> {
        let _ = (id, name, pattern);
        Err(no_files())
    }
    /// Removes a saved pattern. Blocking.
    fn delete_naming_pattern(&self, id: i64) -> Result<(), BackendError> {
        let _ = id;
        Err(no_files())
    }
    /// The missing files, oldest first. Blocking.
    fn missing_files(&self) -> Result<Vec<MissingFile>, BackendError> {
        Ok(Vec::new())
    }
    /// Looks for the missing `files` in `folder` (every library folder when
    /// `None`). Blocking, for a thread of its own.
    fn find_missing(
        &self,
        files: &[FileId],
        folder: Option<&std::path::Path>,
        cancel: &CancelToken,
    ) -> Result<Vec<FoundFile>, BackendError> {
        let _ = (files, folder, cancel);
        Ok(Vec::new())
    }
    /// Relinks the missing files that were found. Announces the change.
    /// Blocking.
    fn relink_missing(&self, found: &[FoundFile]) -> Result<Vec<RelinkOutcome>, BackendError> {
        let _ = found;
        Err(no_files())
    }
    /// Removes missing files from the library. Announces the change. Blocking.
    fn remove_missing(&self, files: &[FileId]) -> Result<usize, BackendError> {
        let _ = files;
        Err(no_files())
    }
    /// The files a check of `scope` reads. Blocking.
    fn verify_targets(&self, scope: &VerifyScope) -> Result<Vec<PathBuf>, BackendError> {
        let _ = scope;
        Ok(Vec::new())
    }
    /// Decodes `paths` from start to end, handing each report to `found` as it
    /// is ready. Blocking, for a thread of its own.
    fn verify_files(
        &self,
        paths: &[PathBuf],
        cancelled: &(dyn Fn() -> bool + Sync),
        found: &(dyn Fn(usize, VerifyReport) + Sync),
    ) {
        let _ = (paths, cancelled, found);
    }

    /// Turns the managed folder on at `folder`, or moves it there; a folder
    /// outside every library folder becomes a library folder and is scanned.
    /// Returns whether it did. Blocking.
    fn set_managed_folder(&self, folder: &Path) -> Result<bool, BackendError> {
        let _ = folder;
        Err(no_files())
    }
    /// How many library files are outside the managed folder. Blocking.
    fn managed_files_outside(&self) -> Result<usize, BackendError> {
        Err(no_files())
    }
    /// The plan of adding `source` to the managed folder, copying or moving
    /// by `transfer`. Changes nothing. Blocking, for a thread of its own.
    fn plan_managed(
        &self,
        source: &ManagedSource,
        transfer: Transfer,
        cancel: &CancelToken,
    ) -> Result<ManagedPlan, BackendError> {
        let _ = (source, transfer, cancel);
        Err(no_files())
    }
    /// Carries out `plan`; stops between files when `cancel` is set, and the
    /// run is then cut short. Announces the change. Blocking, for a thread of
    /// its own.
    fn apply_managed(
        &self,
        plan: &ManagedPlan,
        progress: FileProgress<'_>,
        cancel: &CancelToken,
    ) -> Result<ManagedReport, BackendError> {
        let _ = (plan, progress, cancel);
        Err(no_files())
    }
    /// The run of the managed folder that was cut short, if there is one.
    /// Blocking.
    fn interrupted_managed_run(&self) -> Result<Option<ManagedRunSummary>, BackendError> {
        Ok(None)
    }
    /// Continues the run `run`, which was cut short. Announces the change.
    /// Blocking, for a thread of its own.
    fn resume_managed_run(
        &self,
        run: ManagedRunId,
        progress: FileProgress<'_>,
        cancel: &CancelToken,
    ) -> Result<ManagedReport, BackendError> {
        let _ = (run, progress, cancel);
        Err(no_files())
    }
    /// Rolls back the run `run`, which was cut short. Announces the change.
    /// Blocking, for a thread of its own.
    fn roll_back_managed_run(
        &self,
        run: ManagedRunId,
        progress: FileProgress<'_>,
    ) -> Result<ManagedReport, BackendError> {
        let _ = (run, progress);
        Err(no_files())
    }

    /// The playback flags of a library track (the tag editor's Playback
    /// group); `None` when the track is not in the library. Blocking.
    fn track_flags(&self, track: TrackKey) -> Option<helikodu_model::player::TrackFlags> {
        let _ = track;
        None
    }

    /// The equalizer of the library track `track`, if it has one of its own.
    /// Blocking.
    fn track_equalizer(&self, track: TrackKey) -> Result<Option<TrackEqualizer>, BackendError> {
        let _ = track;
        Ok(None)
    }

    /// Gives each of `tracks` `equalizer`, or with `None` takes their own
    /// equalizers away; it applies from the next time a track starts.
    /// Blocking.
    fn set_track_equalizers(
        &self,
        tracks: &[TrackKey],
        equalizer: Option<&TrackEqualizer>,
    ) -> Result<(), BackendError> {
        let _ = (tracks, equalizer);
        Err(no_files())
    }

    /// Whether any of `tracks` has an equalizer of its own. Blocking.
    fn any_track_equalizer(&self, tracks: &[TrackKey]) -> bool {
        let _ = tracks;
        false
    }

    /// The backups of the library database; `None` without a library.
    fn backups(&self) -> Option<BackupsInfo> {
        None
    }

    /// Makes a backup now. Blocking.
    fn back_up_now(&self) -> Result<BackupsInfo, BackendError> {
        Err(no_files())
    }

    /// Checks the backup at `backup` and stages it to be put back at the next
    /// start. Blocking.
    fn stage_restore(&self, backup: &Path) -> Result<(), BackendError> {
        let _ = backup;
        Err(no_files())
    }

    /// The folder of the log files.
    fn log_folder(&self) -> Option<PathBuf> {
        None
    }

    /// What the window should say once it is shown: about the library database,
    /// as translated sentences.
    fn start_notices(&self) -> Vec<String> {
        Vec::new()
    }

    /// The newest crash report not shown yet.
    fn unseen_crash_report(&self) -> Option<CrashNotice> {
        None
    }

    /// Records that the crash reports were shown.
    fn mark_crash_reports_seen(&self) {}

    /// Imports from other players, the export of the user data, and statistics
    /// carried to replacement files.
    fn interchange(&self) -> Arc<dyn super::interchange::InterchangeServices> {
        Arc::new(super::interchange::NoInterchange)
    }
    /// The players whose libraries are found on this computer, if the offer to
    /// import from them was not made yet; empty once it was. Blocking.
    fn import_offer(&self) -> Vec<FoundPlayer> {
        Vec::new()
    }

    /// Remembers that the offer to import was made, so that it is not made
    /// again.
    fn mark_import_offered(&self) {}
}

/// The error of a file operation without a library.
fn no_files() -> BackendError {
    BackendError(crate::i18n::tr("Helikodu runs without a library."))
}
