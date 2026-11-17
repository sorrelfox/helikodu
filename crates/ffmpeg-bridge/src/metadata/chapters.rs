// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Chapters: MP4/M4B, Matroska, Ogg `CHAPTERxxx` and ID3v2 `CHAP`, as FFmpeg reads
//! them.

use helikodu_model::audio::MediaTime;

use crate::demux::Input;
use crate::ffi;
use crate::metadata::Tags;

/// One chapter of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    /// The container's chapter id.
    pub id: i64,
    /// Where the chapter starts.
    pub start: MediaTime,
    /// Where it ends, when the container says.
    pub end: Option<MediaTime>,
    /// The chapter's title, if it has one.
    pub title: Option<String>,
    /// All of the chapter's tags, the title included.
    pub tags: Tags,
}

/// Reads every chapter of an open input. Chapters with an invalid start are left out.
pub(crate) fn read(input: &Input) -> Vec<Chapter> {
    // SAFETY: fmt is open; `chapters` has `nb_chapters` entries.
    let chapters = unsafe {
        let n = (*input.fmt).nb_chapters as usize;
        if n == 0 || (*input.fmt).chapters.is_null() {
            &[]
        } else {
            std::slice::from_raw_parts((*input.fmt).chapters, n)
        }
    };
    chapters
        .iter()
        .filter(|c| !c.is_null())
        .filter_map(|&c| {
            // SAFETY: a valid chapter of the open context.
            let (id, time_base, start, end, metadata) =
                unsafe { ((*c).id, (*c).time_base, (*c).start, (*c).end, (*c).metadata) };
            // SAFETY: the chapter's dictionary is valid while the context is open.
            let tags = Tags::from_entries(unsafe { ffi::dict_entries(metadata) });
            let start = ffi::media_time(start, time_base)?;
            let end = ffi::media_time(end, time_base).filter(|e| *e >= start);
            Some(Chapter {
                id,
                start,
                end,
                title: tags.get("title").map(str::to_owned),
                tags,
            })
        })
        .collect()
}
