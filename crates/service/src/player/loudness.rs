// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The loudness values of the files in the queue, read from their tags when they are
//! about to play.

use std::collections::HashMap;

use helikodu_library::read_file_details;
use helikodu_model::player::{Loudness, PlaybackItem};

use super::config::ReplayGainMode;
use super::queue::Queue;

/// What the tags of one file say about its loudness and its album.
#[derive(Debug, Clone, PartialEq)]
struct Tagged {
    loudness: Loudness,
    /// The album title and album artists, when the file has an album title.
    album: Option<(String, Vec<String>)>,
}

/// Reads and remembers the loudness tags of queued files, by their index.
#[derive(Debug)]
pub(super) struct LoudnessTags {
    mode: ReplayGainMode,
    read: HashMap<usize, Tagged>,
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

    /// `item`, the file at `index` of `queue`, with its loudness values and whether it
    /// plays as part of its album. Without loudness normalization the item stays as
    /// it is and no tags are read.
    pub(super) fn complete(
        &mut self,
        queue: &Queue,
        index: usize,
        item: PlaybackItem,
    ) -> PlaybackItem {
        if self.mode == ReplayGainMode::Off {
            return item;
        }
        let loudness = self.tags(queue, index).loudness;
        let album_context = self.mode == ReplayGainMode::Auto
            && [index.checked_sub(1), index.checked_add(1)]
                .into_iter()
                .flatten()
                .any(|neighbour| self.same_album(queue, index, neighbour));
        item.with_loudness(loudness, album_context)
    }

    /// Whether the files at `a` and `b` have the same album title and album artists.
    fn same_album(&mut self, queue: &Queue, a: usize, b: usize) -> bool {
        if queue.get(b).is_none() {
            return false;
        }
        let first = self.tags(queue, a).album;
        let second = self.tags(queue, b).album;
        first.is_some() && first == second
    }

    /// The tags of the file at `index`; nothing for a file whose tags cannot be read
    /// (playing it reports the problem, if there is one).
    fn tags(&mut self, queue: &Queue, index: usize) -> Tagged {
        self.read
            .entry(index)
            .or_insert_with(|| {
                let details = queue
                    .get(index)
                    .and_then(|path| read_file_details(path).ok());
                match details {
                    Some(details) => Tagged {
                        loudness: details.loudness,
                        album: details
                            .tags
                            .album
                            .map(|album| (album, details.tags.album_artists)),
                    },
                    None => Tagged {
                        loudness: Loudness::UNKNOWN,
                        album: None,
                    },
                }
            })
            .clone()
    }
}
