// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the converter, folder sync and the cover manager need from the core:
//! presets and destinations, the plans of conversions and syncs, the jobs that
//! carry them out in the background, and the covers of the albums.
//!
//! [`ExportServices`] is implemented over the running service
//! (`service/export.rs`) and over invented content (`sample/export.rs`). Methods
//! marked "blocking" are called on worker threads only.

use std::fmt;
use std::path::{Path, PathBuf};

pub use helikodu_service::convert::{
    CharacterSet, ConvertEvent, ConvertSummary, ConverterChoices, EncodeFormat, ExportState,
    ItemOutcome, JobOptions, NamingPattern, PatternError, PatternProblem, PlannedAction, Preset,
    PresetQuality, SyncAction, SyncDestination, SyncFiles, SyncItem, SyncPlan, SyncSource,
};
pub use helikodu_service::cover_manager::{CoverResult, CoverSave, CoverShow, FolderPicture};

use super::{AlbumKey, BackendError, CoverKey, TrackKey};

/// Where a job's events go; called on the job's threads.
pub type JobEvents = Box<dyn FnMut(ConvertEvent) + Send>;

/// One track of a planned conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertRow {
    /// The track.
    pub track: TrackKey,
    /// Where it goes, relative to the destination folder.
    pub relative: PathBuf,
    /// Where it goes.
    pub target: PathBuf,
    /// What happens to it.
    pub action: PlannedAction,
    /// The size its file is expected to have, in bytes.
    pub expected_bytes: u64,
    /// Its length in seconds.
    pub seconds: f64,
}

/// One album as the cover manager lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverAlbum {
    /// The album.
    pub album: AlbumKey,
    /// Its title.
    pub title: String,
    /// Its artist.
    pub artist: String,
    /// Its year.
    pub year: Option<i32>,
    /// Its cover.
    pub cover: Option<CoverKey>,
    /// A stable value that picks the generated cover.
    pub cover_seed: u64,
    /// Its folder.
    pub folder: Option<PathBuf>,
    /// The groups it is in.
    pub groups: Vec<CoverShow>,
    /// The picture in its folder: the folder image, or for an album without a
    /// cover an image file found there.
    pub picture: Option<FolderPicture>,
    /// The size of its cover in pixels.
    pub cover_size: Option<(u32, u32)>,
    /// The audio files a picture goes into.
    pub files: usize,
}

impl CoverAlbum {
    /// Whether the album is in `group`.
    pub fn is_in(&self, group: CoverShow) -> bool {
        group == CoverShow::All || self.groups.contains(&group)
    }
}

/// What the cover manager does to an album.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverAction {
    /// Use this image file, saved as `save` says.
    Use {
        /// The image file.
        picture: PathBuf,
        /// Where it goes.
        save: CoverSave,
    },
    /// Save the picture inside the files as a file in the folder.
    SaveEmbedded,
    /// Remove the front cover from the files.
    RemoveFromFiles,
}

/// The tools' access to the core. See the module documentation.
pub trait ExportServices: Send + Sync + fmt::Debug {
    /// The presets the user made, the converter's last choices and the
    /// destinations.
    fn state(&self) -> ExportState;
    /// Stores `state`.
    fn save_state(&self, state: &ExportState) -> Result<(), BackendError>;
    /// Each format with the reason it cannot be written, or `None` when it can.
    fn encoders(&self) -> Vec<(EncodeFormat, Option<String>)>;

    /// What converting `tracks` with `preset` into `folder` would do. Blocking.
    fn plan_conversion(
        &self,
        tracks: &[TrackKey],
        preset: &Preset,
        folder: &Path,
        pattern: &NamingPattern,
        style: CharacterSet,
        replace: bool,
    ) -> Result<Vec<ConvertRow>, BackendError>;
    /// Starts converting `rows` in the background; the events carry the index of
    /// the row. Fails when a job runs already.
    fn start_conversion(
        &self,
        rows: &[ConvertRow],
        preset: &Preset,
        options: JobOptions,
        copy_tags: bool,
        events: JobEvents,
    ) -> Result<(), BackendError>;

    /// What syncing `destination` would do. Blocking.
    fn plan_sync(&self, destination: &SyncDestination) -> Result<SyncPlan, BackendError>;
    /// Starts syncing `destination` in the background, as [`plan_sync`]
    /// planned it then; the events carry the index of the plan's row.
    ///
    /// [`plan_sync`]: Self::plan_sync
    fn start_sync(
        &self,
        destination: &SyncDestination,
        workers: usize,
        events: JobEvents,
    ) -> Result<(), BackendError>;

    /// Stops the running job; what it finished stays.
    fn stop(&self);

    /// Every album with its covers. Blocking.
    fn cover_albums(&self) -> Result<Vec<CoverAlbum>, BackendError>;
    /// Does `action` to `album`, and reports each file. Blocking.
    fn apply_cover(
        &self,
        album: AlbumKey,
        action: &CoverAction,
    ) -> Result<Vec<CoverResult>, BackendError>;
}
