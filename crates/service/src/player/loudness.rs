// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The loudness values of the files in the queue, read from their tags when they are
//! about to play; the library's measurements, where it has current ones, win over
//! the tags.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helikodu_library::{MeasuredLoudness, read_file_details};
use helikodu_model::player::{Loudness, PlaybackItem};

/// What the tags of one file say about its loudness and its album.
#[derive(Debug, Clone, PartialEq)]
struct Tagged {
    loudness: Loudness,
    /// The album title and album artists, when the file has an album title.
    album: Option<(String, Vec<String>)>,
}

/// Reads and remembers the loudness tags of queued files, by path.
///
/// The tags are read whatever the loudness mode, so that a change of the mode while
/// playing ([`PlayerCommand::ConfigureAudio`](super::PlayerCommand::ConfigureAudio))
/// finds the values of the item that plays.
#[derive(Debug, Default)]
pub(super) struct LoudnessTags {
    read: HashMap<PathBuf, Tagged>,
}

impl LoudnessTags {
    /// Forgets what was read, for a new queue.
    pub(super) fn clear(&mut self) {
        self.read.clear();
    }

    /// `item`, which plays the file at `path`, with its loudness values, its album
    /// (from the tags, for the crossfade policy) and whether it plays as part of its
    /// album: whether the file before or after it in the queue (`neighbours`) has the
    /// same album. The engine looks at that only in the automatic loudness mode.
    ///
    /// `measured` is the library's current measurement of the track, if it has
    /// one: its values win over those of the tags, which fill what it lacks.
    pub(super) fn complete(
        &mut self,
        path: &Path,
        neighbours: &[Option<PathBuf>; 2],
        item: PlaybackItem,
        measured: Option<&MeasuredLoudness>,
    ) -> PlaybackItem {
        let tagged = self.tags(path);
        let album_context = tagged.album.is_some()
            && neighbours
                .iter()
                .flatten()
                .any(|neighbour| self.tags(neighbour).album == tagged.album);
        let loudness = match measured {
            Some(measured) => measured.over_tags(tagged.loudness),
            None => tagged.loudness,
        };
        // The album as a key: title and album artists, apart by a unit separator.
        let album = tagged.album.map(|(title, artists)| {
            std::iter::once(title)
                .chain(artists)
                .collect::<Vec<_>>()
                .join("\u{1f}")
        });
        item.with_loudness(loudness, album_context)
            .with_album(album)
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

#[cfg(test)]
mod tests {
    use helikodu_library::AlbumMeasurement;
    use helikodu_model::library::{MediaSource, TrackId};
    use helikodu_model::player::ItemId;

    use super::*;

    #[test]
    fn playback_prefers_measured_values_over_tags() {
        let path = PathBuf::from("/nonexistent/track.flac");
        let mut loudness = LoudnessTags::default();
        let tagged = Loudness {
            track_gain_db: Some(-2.0),
            track_peak: Some(0.5),
            album_gain_db: Some(-3.0),
            album_peak: Some(0.6),
            true_peak: false,
        };
        loudness.read.insert(
            path.clone(),
            Tagged {
                loudness: tagged,
                album: None,
            },
        );
        let item = || PlaybackItem::new(ItemId::new(1), MediaSource::file(&path));
        let neighbours = [None, None];

        let from_tags = loudness.complete(&path, &neighbours, item(), None);
        assert_eq!(from_tags.loudness, tagged);

        let measured = MeasuredLoudness {
            track: TrackId::new(7),
            version: 1,
            integrated_lufs: Some(-9.5),
            range_lu: 4.0,
            sample_peak: 0.97,
            true_peak: 1.04,
            album: Some(AlbumMeasurement {
                integrated_lufs: Some(-10.0),
                range_lu: 5.0,
                sample_peak: 0.98,
                true_peak: 1.08,
            }),
        };
        let played = loudness.complete(&path, &neighbours, item(), Some(&measured));
        assert_eq!(
            played.loudness,
            Loudness {
                track_gain_db: Some(-8.5),
                track_peak: Some(1.04),
                album_gain_db: Some(-8.0),
                album_peak: Some(1.08),
                true_peak: true,
            }
        );
    }
}
