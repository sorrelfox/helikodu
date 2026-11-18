// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The loudness values of the files in the queue, read from their tags when they are
//! about to play.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helikodu_library::read_file_details;
use helikodu_model::player::{Loudness, PlaybackItem};

use super::config::ReplayGainMode;

/// What the tags of one file say about its loudness and its album.
#[derive(Debug, Clone, PartialEq)]
struct Tagged {
    loudness: Loudness,
    /// The album title and album artists, when the file has an album title.
    album: Option<(String, Vec<String>)>,
}

/// Reads and remembers the loudness tags of queued files, by path.
#[derive(Debug)]
pub(super) struct LoudnessTags {
    mode: ReplayGainMode,
    read: HashMap<PathBuf, Tagged>,
}

impl LoudnessTags {
    pub(super) fn new(mode: ReplayGainMode) -> Self {
        Self {
            mode,
            read: HashMap::new(),
        }
    }

    /// Forgets what was read, for a new queue.
    pub(super) fn clear(&mut self) {
        self.read.clear();
    }

    /// `item`, which plays the file at `path`, with its loudness values and whether
    /// it plays as part of its album: whether the file before or after it in the
    /// queue (`neighbours`) has the same album. Without loudness normalization the
    /// item stays as it is and no tags are read.
    pub(super) fn complete(
        &mut self,
        path: &Path,
        neighbours: &[Option<PathBuf>; 2],
        item: PlaybackItem,
    ) -> PlaybackItem {
        if self.mode == ReplayGainMode::Off {
            return item;
        }
        let tagged = self.tags(path);
        let album_context = self.mode == ReplayGainMode::Auto
            && tagged.album.is_some()
            && neighbours
                .iter()
                .flatten()
                .any(|neighbour| self.tags(neighbour).album == tagged.album);
        item.with_loudness(tagged.loudness, album_context)
    }

    /// The tags of the file at `path`; nothing for a file whose tags cannot be read
    /// (playing it reports the problem, if there is one).
    fn tags(&mut self, path: &Path) -> Tagged {
        self.read
            .entry(path.to_path_buf())
            .or_insert_with(|| match read_file_details(path) {
                Ok(details) => Tagged {
                    loudness: details.loudness,
                    album: details
                        .tags
                        .album
                        .map(|album| (album, details.tags.album_artists)),
                },
                Err(_) => Tagged {
                    loudness: Loudness::UNKNOWN,
                    album: None,
                },
            })
            .clone()
    }
}
