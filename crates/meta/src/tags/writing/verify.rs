// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checking a written file before it replaces the original: it must read back
//! with the same audio, and every change must read back as written.

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use crate::covers::{CoverHash, CoverLimits, read_embedded_covers};
use crate::tags::reading::{LoftyReader, ReadOptions, TagReader};
use crate::tags::record::{AudioProperties, TrackTags};

use super::WriteError;
use super::changes::{CoverChange, FieldKey, Normalized, TagField};
use super::editor::SkippedChange;
use super::formats;
use super::values::{field_values, read_back_form, same_values};

/// The options of the reads that verify and plan a write: the text as stored.
pub(crate) fn plain_read_options() -> ReadOptions {
    ReadOptions {
        repair_charsets: false,
        legacy_charset: None,
    }
}

fn failed(reason: impl Into<String>) -> WriteError {
    WriteError::VerificationFailed {
        reason: reason.into(),
        in_place: false,
    }
}

/// Checks the written file at `path` against the audio read before the write and
/// against `changes`, leaving out what the main tag `skipped`.
pub(crate) fn verify(
    path: &Path,
    before: &AudioProperties,
    changes: &Normalized,
    skipped: &[SkippedChange],
) -> Result<(), WriteError> {
    let read = LoftyReader
        .read_file(File::open(path)?, &plain_read_options())
        .map_err(|e| failed(format!("the written file cannot be read: {e}")))?;
    same_audio(before, &read.audio)?;
    let tags = &read.tags;
    let was_skipped = |what: &str| skipped.iter().any(|s| s.what == what);
    for (key, values) in &changes.fields {
        if was_skipped(key.name()) || pair_partner_skipped(key, &was_skipped) {
            continue;
        }
        let expected = read_back_form(key, values);
        let found = field_values(tags, key);
        if !same_values(key, &expected, &found) && !list_fallback(key, &expected, tags) {
            return Err(failed(format!(
                "{} reads back as {found:?} instead of {expected:?}",
                key.name()
            )));
        }
    }
    if let Some(lyrics) = &changes.lyrics
        && !was_skipped("lyrics")
    {
        let found = formats::lyrics(&mut File::open(path)?)?;
        let normalize = |t: &str| t.replace("\r\n", "\n").trim_end().to_owned();
        let same = match (lyrics, &found) {
            (Some(expected), Some(found)) => normalize(expected) == normalize(found),
            (None, None) => !tags.has_lyrics,
            _ => false,
        };
        if !same {
            return Err(failed("the lyrics do not read back as written"));
        }
    }
    if let Some(Some(rating)) = changes.rating
        && !was_skipped("rating")
        && tags.rating != Some(rating)
    {
        return Err(failed(format!(
            "the rating reads back as {:?} instead of {rating}",
            tags.rating
        )));
    }
    if let Some(cover) = &changes.cover
        && !was_skipped("cover")
    {
        verify_cover(path, cover)?;
    }
    Ok(())
}

/// A change of a number is written together with its total, so a skipped pair
/// skips both.
fn pair_partner_skipped(key: &FieldKey, was_skipped: &dyn Fn(&str) -> bool) -> bool {
    let partner = match key {
        FieldKey::Known(TagField::TrackTotal) => TagField::TrackNumber,
        FieldKey::Known(TagField::DiscTotal) => TagField::DiscNumber,
        FieldKey::Known(TagField::MovementTotal) => TagField::MovementNumber,
        _ => return false,
    };
    was_skipped(partner.picard_name())
}

/// An artist field that was cleared still reads the other one: the credited
/// artist is the list joined when there is no `ARTIST`, and the list is the
/// `ARTIST` values when there is no `ARTISTS`.
fn list_fallback(key: &FieldKey, expected: &[String], tags: &TrackTags) -> bool {
    if !expected.is_empty() {
        return false;
    }
    let (single, list) = match key {
        FieldKey::Known(TagField::Artist | TagField::Artists) => (&tags.artist, &tags.artists),
        FieldKey::Known(TagField::AlbumArtist | TagField::AlbumArtists) => {
            (&tags.album_artist, &tags.album_artists)
        }
        _ => return false,
    };
    single.as_deref() == Some(list.join("; ").as_str())
}

fn same_audio(before: &AudioProperties, after: &AudioProperties) -> Result<(), WriteError> {
    if before.sample_rate != after.sample_rate
        || before.channels != after.channels
        || before.codec != after.codec
    {
        return Err(failed("the audio properties changed"));
    }
    let close = match (before.duration, after.duration) {
        (Some(a), Some(b)) => {
            let difference = a.abs_diff(b);
            difference <= Duration::from_millis(100) + a / 100
        }
        (a, b) => a.is_none() == b.is_none(),
    };
    if close {
        Ok(())
    } else {
        Err(failed("the length of the audio changed"))
    }
}

fn verify_cover(path: &Path, change: &CoverChange) -> Result<(), WriteError> {
    let covers = read_embedded_covers(path, &CoverLimits::default())
        .map_err(|e| failed(format!("the pictures cannot be read: {e}")))?;
    let ok = match change {
        CoverChange::SetFront(bytes) => covers
            .front()
            .and_then(|c| c.hash().ok())
            .is_some_and(|hash| hash == CoverHash::of(bytes)),
        CoverChange::RemoveFront => !covers
            .covers
            .iter()
            .any(|c| c.source().picture_type().is_none_or(|t| t.0 == 3)),
        CoverChange::RemoveAll => covers.covers.is_empty() && covers.skipped.is_empty(),
    };
    if ok {
        Ok(())
    } else {
        Err(failed("the pictures do not read back as written"))
    }
}
