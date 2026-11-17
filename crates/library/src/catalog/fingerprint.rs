// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The fingerprint that finds a track's identity again after it left the library.

use super::track_record::TrackRecord;

/// What identifies a track independently of its path: the quick hash of its file and
/// its audio properties.
///
/// Two tracks with the same fingerprint are the same recording in the same file
/// content. A re-encoded or re-ripped file has another fingerprint, so it gets a new
/// identity; moving its statistics over is an explicit action.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackFingerprint {
    /// The quick hash of the file: its size and its first and last 64 KiB.
    pub content_hash: Vec<u8>,
    /// The audio properties, encoded (see [`TrackFingerprint::audio_key`]).
    audio_key: Vec<u8>,
}

/// The version of the audio key's layout, its first byte.
const AUDIO_KEY_VERSION: u8 = 1;

impl TrackFingerprint {
    /// The fingerprint of `record`.
    pub fn of(record: &TrackRecord) -> Self {
        let mut key = Vec::with_capacity(48);
        key.push(AUDIO_KEY_VERSION);
        key.extend_from_slice(&record.sub_index.to_le_bytes());
        let (start, end) = record.frames.unwrap_or((0, None));
        key.extend_from_slice(&start.to_le_bytes());
        key.extend_from_slice(&end.unwrap_or(u64::MAX).to_le_bytes());
        let audio = &record.audio;
        let duration_ms = audio.duration.map_or(u64::MAX, |d| {
            u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
        });
        key.extend_from_slice(&duration_ms.to_le_bytes());
        key.extend_from_slice(&audio.sample_rate.unwrap_or(0).to_le_bytes());
        key.extend_from_slice(&audio.channels.unwrap_or(0).to_le_bytes());
        Self {
            content_hash: record.file.quick_hash.clone(),
            audio_key: key,
        }
    }

    /// The audio properties as stored in `identities.audio_key`: a version byte, then
    /// the sub-track number, the first and last frame, the duration in milliseconds,
    /// the sample rate and the channel count, little-endian.
    pub fn audio_key(&self) -> &[u8] {
        &self.audio_key
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::catalog::{AudioProperties, FileStamp, RootId};

    fn record(sub_index: u32, duration_ms: u64) -> TrackRecord {
        let mut record = TrackRecord::new(
            RootId(1),
            "/music/a.flac",
            FileStamp {
                size: 10,
                mtime_ns: None,
                inode: None,
                quick_hash: vec![1, 2, 3],
            },
        );
        record.sub_index = sub_index;
        record.audio = AudioProperties {
            duration: Some(Duration::from_millis(duration_ms)),
            sample_rate: Some(44_100),
            channels: Some(2),
            ..AudioProperties::default()
        };
        record
    }

    #[test]
    fn the_path_does_not_matter() {
        let a = record(0, 1000);
        let mut b = record(0, 1000);
        b.path = "/elsewhere/b.flac".into();
        b.tags.title = Some("Renamed".to_owned());
        assert_eq!(TrackFingerprint::of(&a), TrackFingerprint::of(&b));
    }

    #[test]
    fn sub_tracks_and_audio_differ() {
        let base = TrackFingerprint::of(&record(0, 1000));
        assert_ne!(base, TrackFingerprint::of(&record(1, 1000)));
        assert_ne!(base, TrackFingerprint::of(&record(0, 1001)));
        let mut other_content = record(0, 1000);
        other_content.file.quick_hash = vec![9];
        assert_ne!(base, TrackFingerprint::of(&other_content));
    }
}
