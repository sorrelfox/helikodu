// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the dialogs need from the core besides the library lists and the
//! player: the settings store, the output devices, the library folders and
//! their scans, tag editing, and the facts for Supported formats and About.
//!
//! [`Services`] is implemented over the running service (`service.rs`) and
//! over invented content (`sample.rs`). Methods marked "blocking" are called
//! on worker threads only; the others are quick and may be called on the main
//! thread.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use helikodu_model::audio::{
    BackendSelection, DeviceId, DeviceInfo, DeviceKey, DeviceSelection, OutputBackendKind,
    OutputMode, OutputSelection,
};
pub use helikodu_model::settings::Settings;
pub use helikodu_service::formats::{
    Availability, Decoder, DecoderStatus, FamilyStatus, FormatFamily, FormatReport,
};

use super::{AudioFormat, BackendError, CoverKey, TrackKey};

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
    /// Analysis data, in bytes.
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
}

impl TagEdit {
    /// Whether anything is changed in the files themselves (the rating is
    /// kept in the library).
    pub fn touches_files(&self) -> bool {
        !self.fields.is_empty() || self.cover.is_some() || self.lyrics.is_some()
    }

    /// Whether anything is changed at all.
    pub fn is_empty(&self) -> bool {
        !self.touches_files() && self.rating.is_none()
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
    /// Saves `edit`, file by file. Blocking.
    fn write_tags(&self, edit: &TagEdit) -> Vec<WriteResult>;

    /// What this build can play.
    fn formats(&self) -> FormatReport;
    /// Versions of the libraries in use.
    fn versions(&self) -> Versions;
}
