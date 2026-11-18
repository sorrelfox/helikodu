// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tag writing.
//!
//! [`write_tags`] writes a [`TagChanges`] into one file through lofty: fields set
//! or cleared (several values where a field takes them), custom fields, the front
//! cover, the lyrics and the rating. Each field is written under the key
//! MusicBrainz Picard uses, which is the mapping table of the reader in reverse,
//! and every other key that the reader takes for the same field is removed.
//! Everything the editor does not touch stays: other frames, other keys, other
//! pictures, the order of the rest.
//!
//! - **Formats.** MP3, ADTS AAC, WAV and AIFF (ID3v2; ID3v2.4 for a new tag unless
//!   ID3v2.3 is chosen, and an existing tag keeps its version), FLAC, Ogg Vorbis,
//!   Opus and Speex (Vorbis comments), MP4 (`ilst`), WavPack, Monkey's Audio and
//!   Musepack (APEv2). APEv2 and ID3v1 tags on MP3, RIFF `INFO` in WAV and AIFF
//!   text chunks are kept in step where they can hold a field. Matroska, ASF,
//!   DSF, DFF, TAK, TTA and anything else lofty does not write are
//!   [`WriteError::ReadOnlyFormat`].
//! - **Safe saving.** The edit goes into a copy in the same folder, which is read
//!   back and checked (same audio, every change as written), flushed to disk, and
//!   renamed over the original. A failure at any step removes the copy and leaves
//!   the original untouched. Files above [`WriteOptions::in_place_above`] (and
//!   files with several hard links, which a rename would split) are changed in
//!   place instead, which [`WriteMode::InPlace`] reports.
//! - **Stale files.** A write given the [`EditStamp`] recorded when the file was
//!   read refuses a file whose size or modification time differs
//!   ([`WriteError::ChangedSinceRead`]); it checks again just before the rename.
//! - **Selections.** [`EditableFile`] is a file as the editor loads it,
//!   [`SelectionView`] the shared or mixed value of every field over several, and
//!   [`write_batch`] writes many files with progress, cancellation and a result
//!   for each.

mod ape;
mod batch;
mod changes;
mod editable;
mod editor;
mod formats;
mod id3v2;
mod mp4;
mod safe_save;
mod secondary;
mod selection;
mod values;
mod verify;
mod vorbis;

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub use batch::{BatchItem, BatchProgress, BatchReport, write_batch};
pub use changes::{CoverChange, FieldChange, FieldKey, TagChanges, TagField};
pub use editable::{EditableFile, load_for_editing, read_lyrics};
pub use editor::SkippedChange;
pub use formats::Id3Version;
pub use safe_save::{EditStamp, is_temporary_name, remove_stale_temporaries};
pub use selection::{SelectionView, Shared};
pub use values::field_values;

use super::reading::{FfmpegReader, LoftyReader, TagReader};
use super::record::FileTags;
use changes::Normalized;
use editor::Plan;
use safe_save::Temporary;
use verify::{plain_read_options, verify};

/// The default size above which tags are changed in place: 2 GiB.
pub const DEFAULT_IN_PLACE_ABOVE: u64 = 2 << 30;

/// How tags are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// The ID3v2 version of new ID3v2 tags; existing tags keep theirs.
    pub id3_version: Id3Version,
    /// Files larger than this, in bytes, are changed in place instead of being
    /// copied and replaced.
    pub in_place_above: u64,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            id3_version: Id3Version::V2_4,
            in_place_above: DEFAULT_IN_PLACE_ABOVE,
        }
    }
}

/// Why a file was changed in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InPlaceReason {
    /// It is larger than [`WriteOptions::in_place_above`].
    Large,
    /// It has several hard links, which replacing it would separate.
    HardLinked,
}

/// How a file was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteMode {
    /// Nothing was to be changed; the file was not touched.
    Unchanged,
    /// A verified copy replaced the file.
    Replaced,
    /// The file was changed in place: a crash during the write could have
    /// damaged it.
    InPlace(InPlaceReason),
}

/// The result of writing one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    /// The file written (symbolic links resolved).
    pub path: PathBuf,
    /// How it was written.
    pub mode: WriteMode,
    /// Its stamp after the write, for a following edit.
    pub stamp: EditStamp,
    /// The changes the file's tag could not take; the rest was written.
    pub skipped: Vec<SkippedChange>,
    /// The version of the ID3v2 tag written, for files whose main tag is ID3v2.
    pub id3_version: Option<Id3Version>,
}

/// Why tags could not be written. The file is unchanged unless the error says
/// otherwise.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The file could not be read or written.
    #[error("cannot access the file: {0}")]
    Io(#[from] io::Error),
    /// The format's tags can be read but not written.
    #[error("read-only format: tags of this format cannot be written")]
    ReadOnlyFormat,
    /// The file's size or modification time differs from when it was read.
    #[error("the file changed since it was read")]
    ChangedSinceRead,
    /// The tag library could not parse the file.
    #[error("the tag library cannot parse this file: {0}")]
    Parse(String),
    /// The tag library could not write the tags.
    #[error("the tags could not be written: {0}")]
    Save(String),
    /// A picture could not be prepared.
    #[error("the picture could not be written: {0}")]
    Picture(&'static str),
    /// A value that cannot be written.
    #[error("{value:?} cannot be written to {field}: {reason}")]
    InvalidValue {
        /// The field's name.
        field: String,
        /// The value.
        value: String,
        /// Why not.
        reason: &'static str,
    },
    /// A cover that cannot be embedded.
    #[error("the cover cannot be embedded: {0}")]
    InvalidCover(&'static str),
    /// The written file did not read back as expected.
    #[error("the written file did not check out: {reason}")]
    VerificationFailed {
        /// What differed.
        reason: String,
        /// Whether the file was changed in place, so that it now holds the
        /// unexpected result; otherwise the original is untouched.
        in_place: bool,
    },
    /// The batch was cancelled before this file was written.
    #[error("cancelled")]
    Cancelled,
}

/// How a file of `size` bytes with metadata `metadata` would be written.
fn mode_for(metadata: &fs::Metadata, options: &WriteOptions) -> WriteMode {
    if metadata.len() > options.in_place_above {
        WriteMode::InPlace(InPlaceReason::Large)
    } else if metadata.nlink() > 1 {
        WriteMode::InPlace(InPlaceReason::HardLinked)
    } else {
        WriteMode::Replaced
    }
}

/// How the file at `path` would be written with `options`: replaced, or changed
/// in place. A player can wait for the end of a track before an in-place write
/// to the file it is playing.
pub fn planned_mode(path: impl AsRef<Path>, options: &WriteOptions) -> io::Result<WriteMode> {
    Ok(mode_for(&fs::metadata(path)?, options))
}

/// Whether the tags of the file at `path` can be written: lofty knows its format
/// and parses it.
pub fn is_writable(path: impl AsRef<Path>) -> io::Result<bool> {
    let path = path.as_ref();
    let mut file = File::open(path)?;
    let known = match formats::writable_type(&mut file) {
        Ok(found) => found.is_some(),
        Err(WriteError::Io(e)) => return Err(e),
        Err(_) => false,
    };
    Ok(known && read_for_writing(&file, path).is_ok())
}

/// The tags of an open file as lofty reads them. A file lofty cannot parse but
/// FFmpeg can (a format lofty mistakes for another by its content, such as
/// Matroska) is a read-only format, as the reader treats it.
fn read_for_writing(file: &File, path: &Path) -> Result<FileTags, WriteError> {
    let error = match LoftyReader.read_file(file.try_clone()?, &plain_read_options()) {
        Ok(read) => return Ok(read),
        Err(e) => e,
    };
    let ffmpeg_reads = FfmpegReader
        .read_file(File::open(path)?, &plain_read_options())
        .is_ok();
    Err(if ffmpeg_reads {
        WriteError::ReadOnlyFormat
    } else {
        WriteError::Parse(error.to_string())
    })
}

/// Writes `changes` into the file at `path`.
///
/// With `expected`, the stamp recorded when the file was read, a file that
/// changed since is refused with [`WriteError::ChangedSinceRead`]. The values are
/// checked before the file is touched ([`WriteError::InvalidValue`],
/// [`WriteError::InvalidCover`]).
pub fn write_tags(
    path: impl AsRef<Path>,
    changes: &TagChanges,
    expected: Option<EditStamp>,
    options: &WriteOptions,
) -> Result<WriteOutcome, WriteError> {
    write_checked(path.as_ref(), changes, expected, options, &|_| Ok(()))
}

/// [`write_tags`] with an extra check of the written file, for tests.
pub(crate) fn write_checked(
    path: &Path,
    changes: &TagChanges,
    expected: Option<EditStamp>,
    options: &WriteOptions,
    extra_check: &dyn Fn(&Path) -> Result<(), WriteError>,
) -> Result<WriteOutcome, WriteError> {
    let normalized = changes.normalized()?;
    let target = fs::canonicalize(path)?;
    let metadata = fs::metadata(&target)?;
    let stamp = EditStamp::from_metadata(&metadata)?;
    if expected.is_some_and(|e| e != stamp) {
        return Err(WriteError::ChangedSinceRead);
    }
    let mut original = File::open(&target)?;
    let file_type = formats::writable_type(&mut original)?.ok_or(WriteError::ReadOnlyFormat)?;
    if changes.is_empty() {
        return Ok(WriteOutcome {
            path: target,
            mode: WriteMode::Unchanged,
            stamp,
            skipped: Vec::new(),
            id3_version: None,
        });
    }
    let before = read_for_writing(&original, &target)?;
    let plan = Plan::new(&normalized, &before.tags);
    let mode = mode_for(&metadata, options);
    let edited = match mode {
        WriteMode::InPlace(_) => {
            drop(original);
            let mut file = OpenOptions::new().read(true).write(true).open(&target)?;
            let edited = formats::edit(&mut file, file_type, &plan, options.id3_version)?;
            file.sync_all()?;
            drop(file);
            check(
                &target,
                &before.audio,
                &normalized,
                &edited.skipped,
                extra_check,
            )
            .map_err(|e| match e {
                WriteError::VerificationFailed { reason, .. } => WriteError::VerificationFailed {
                    reason,
                    in_place: true,
                },
                other => other,
            })?;
            edited
        }
        WriteMode::Replaced | WriteMode::Unchanged => {
            let mut copy = Temporary::copy_of(&target, &metadata)?;
            let edited = formats::edit(&mut copy.file, file_type, &plan, options.id3_version)?;
            copy.file.sync_all()?;
            check(
                copy.path(),
                &before.audio,
                &normalized,
                &edited.skipped,
                extra_check,
            )?;
            if EditStamp::of(&target)? != stamp {
                return Err(WriteError::ChangedSinceRead);
            }
            copy.replace(&target)?;
            edited
        }
    };
    Ok(WriteOutcome {
        stamp: EditStamp::of(&target)?,
        path: target,
        mode,
        skipped: edited.skipped,
        id3_version: edited.id3_version,
    })
}

fn check(
    path: &Path,
    before: &super::record::AudioProperties,
    changes: &Normalized,
    skipped: &[SkippedChange],
    extra_check: &dyn Fn(&Path) -> Result<(), WriteError>,
) -> Result<(), WriteError> {
    verify(path, before, changes, skipped)?;
    extra_check(path)
}

#[cfg(test)]
mod tests;
