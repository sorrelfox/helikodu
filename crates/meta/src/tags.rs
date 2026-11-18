// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tag reading and writing.
//!
//! [`read_tags`] reads a file's tags into a format-independent [`TrackTags`] record,
//! with the file's [`AudioProperties`]. The pure-Rust tag library lofty reads every
//! format it supports; FFmpeg reads the rest (Matroska, ASF/WMA, DSF, DFF, TAK,
//! TTA) and any file lofty cannot parse. Those tags are read-only.
//!
//! Keys are mapped to fields by one table per tag format (ID3v2, Vorbis comments,
//! APEv2, MP4 atoms, RIFF INFO and FFmpeg's dictionary), following MusicBrainz
//! Picard's tag mapping. Text in legacy charsets is repaired (see [`charset`]
//! for the rules), and ratings are read onto the half-star scale (see [`rating`]).
//!
//! [`writing`] writes tags back through lofty, with the same tables in reverse.

mod builder;
pub mod charset;
mod field;
mod gain;
mod mapping;
pub mod rating;
mod reading;
mod record;
pub mod writing;

pub use rating::Rating;
pub use reading::{
    FfmpegReader, LoftyReader, ReadOptions, TagError, TagReader, read_tags, read_tags_from_file,
};
pub use record::{
    AudioProperties, Credit, FileTags, MusicBrainzIds, ReplayGainTags, Role, TagSource, TrackTags,
    r128_to_replaygain_db,
};
pub use writing::{
    EditStamp, EditableFile, FieldKey, TagChanges, TagField, WriteError, WriteOptions,
    WriteOutcome, load_for_editing, write_tags,
};
