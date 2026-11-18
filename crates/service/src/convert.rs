// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The converter and folder sync: presets, naming patterns, the plan of a
//! conversion, the background job that converts and copies, and sync
//! destinations with their list of the files Helikodu put there.
//!
//! Encoding goes through the engine and the FFmpeg bridge, offline as decoding
//! is: the bridge writes to a file the job opened itself and refuses every other
//! open. See [`job`] for how files are written so that nothing half-written is
//! ever left behind.

pub mod job;
pub mod plan;
pub mod presets;
pub mod store;
pub mod sync;
mod tags;

pub use helikodu_engine::convert::{
    EncodeFormat, EncodeQuality, EncoderAvailability, TranscodeSettings, encoder_availability,
    supported_sample_rates,
};
/// Naming patterns, shared with the file organizer: one pattern gives one name
/// for one track in every tool.
pub use helikodu_library::{
    CharacterSet, FieldMap, FieldValues, NamingPattern, PatternError, PatternProblem,
    clean_file_name,
};
pub use job::{
    ConvertEvent, ConvertJob, ConvertSummary, ItemOutcome, JobAction, JobItem, JobOptions,
};
pub use plan::{PlannedAction, PlannedItem, SourceTrack, plan_conversion, source_tracks};
pub use presets::{Preset, PresetQuality};
pub use store::{ConverterChoices, EXPORT_FILE, ExportState, ExportStore};
pub use sync::{
    Manifest, ManifestEntry, ResolvedSource, SyncAction, SyncDestination, SyncFiles, SyncItem,
    SyncPlan, SyncSource, plan_sync, resolve, run_sync,
};

use helikodu_model::library::TrackId;

use crate::library::MusicLibrary;

/// The encoded front cover of `track` (JPEG or PNG), from its file or folder.
pub fn cover_of(library: &MusicLibrary, track: TrackId) -> Option<Vec<u8>> {
    tags::track_cover(library, track)
}
