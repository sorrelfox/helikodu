// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a worker read from a file, turned into the library's track records.

use helikodu_meta::cue::VirtualTrack;
use helikodu_meta::tags::{self as meta_tags, Role};
use helikodu_meta::worker::FileReport;
use helikodu_model::audio::SampleRate;

use super::filter::{FileClass, is_audiobook};
use super::walk::Found;
use crate::catalog::{AudioProperties, FileStamp, RootId, TrackKind, TrackRecord, TrackTags};
use crate::stats::Rating;

/// The tracks of the file `found`, read by a worker into `report`: one per track of
/// its CUE sheet, or one for the whole file.
///
/// The file's stamp is the one the walk saw, so a file that changed while it was
/// being read is read again by the next scan. Chapters are not tracks.
pub(crate) fn tracks_of(root: RootId, found: &Found, report: &FileReport) -> Vec<TrackRecord> {
    let file = FileStamp {
        size: found.stamp.size,
        mtime_ns: Some(found.stamp.mtime_ns),
        inode: Some(found.stamp.inode),
        quick_hash: report.quick_hash.clone(),
    };
    let kind = match found.class {
        FileClass::Video => TrackKind::AudioFromVideo,
        _ if found.path.file_name().is_some_and(is_audiobook) => TrackKind::Audiobook,
        _ => TrackKind::Music,
    };
    let audio = audio_properties(&report.tags.audio);
    let base = TrackRecord {
        kind,
        audio,
        ..TrackRecord::new(root, &found.path, file)
    };
    // A track that starts after the end of the file (a sheet written for a longer
    // rip) is left out; without any track left, the file is one track.
    let length = base.audio.duration;
    let sheet_tracks: Vec<TrackRecord> = report
        .cue
        .iter()
        .flat_map(|cue| &cue.tracks)
        .filter(|track| length.is_none_or(|length| track.start().as_duration() < length))
        .map(|track| cue_track(&base, track))
        .collect();
    if !sheet_tracks.is_empty() {
        return sheet_tracks;
    }
    let tags = &report.tags.tags;
    vec![TrackRecord {
        tags: track_tags(tags),
        rating_from_tags: rating(tags),
        ..base
    }]
}

fn cue_track(base: &TrackRecord, track: &VirtualTrack) -> TrackRecord {
    let start = track.start();
    let end = track.end();
    let mut audio = base.audio.clone();
    if let Some(file_length) = audio.duration {
        let until = end.map_or(file_length, |end| end.as_duration().min(file_length));
        audio.duration = Some(until.saturating_sub(start.as_duration()));
    }
    let frames = audio
        .sample_rate
        .and_then(SampleRate::new)
        .map(|rate| (start.frames_at(rate), end.map(|end| end.frames_at(rate))));
    TrackRecord {
        sub_index: track.number,
        frames,
        audio,
        tags: track_tags(&track.tags),
        rating_from_tags: rating(&track.tags),
        ..base.clone()
    }
}

fn audio_properties(audio: &meta_tags::AudioProperties) -> AudioProperties {
    AudioProperties {
        duration: audio.duration,
        sample_rate: audio.sample_rate,
        channels: audio.channels,
        bit_depth: audio.bit_depth.and_then(|bits| u16::try_from(bits).ok()),
        bitrate: audio.bitrate_kbps.map(|kbps| kbps.saturating_mul(1000)),
        codec: audio.codec.clone(),
        container: audio.container.clone(),
    }
}

/// The fields the catalogue keeps. A credited artist without a list of artists
/// stands for the list.
fn track_tags(tags: &meta_tags::TrackTags) -> TrackTags {
    let list_or = |list: &Vec<String>, single: &Option<String>| {
        if list.is_empty() {
            single.iter().cloned().collect()
        } else {
            list.clone()
        }
    };
    TrackTags {
        title: tags.title.clone(),
        title_sort: tags.title_sort.clone(),
        artists: list_or(&tags.artists, &tags.artist),
        album_artists: list_or(&tags.album_artists, &tags.album_artist),
        composers: tags
            .credits_with(Role::Composer)
            .map(|credit| credit.name.clone())
            .collect(),
        album: tags.album.clone(),
        genres: tags.genres.clone(),
        track_no: tags.track_number,
        disc_no: tags.disc_number,
        date: tags.date.clone(),
        year: tags.year(),
    }
}

fn rating(tags: &meta_tags::TrackTags) -> Option<Rating> {
    tags.rating
        .and_then(|rating| Rating::from_half_stars(rating.half_stars()))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::time::Duration;

    use helikodu_meta::cue::CueOrigin;
    use helikodu_meta::tags::{Credit, FileTags, TagSource};
    use helikodu_meta::worker::CueReport;
    use helikodu_model::audio::MediaTime;
    use helikodu_model::library::{MediaSource, SourceSection};

    use super::*;
    use crate::scan::walk::Stamp;

    fn report(cue: Option<CueReport>) -> FileReport {
        FileReport {
            size: 10,
            quick_hash: vec![9; 16],
            tags: FileTags {
                tags: meta_tags::TrackTags {
                    title: Some("Whole".into()),
                    artist: Some("A feat. B".into()),
                    album_artists: vec!["A".into()],
                    credits: vec![Credit {
                        role: Role::Composer,
                        name: "C".into(),
                        instrument: None,
                    }],
                    date: Some("1999-02-03".into()),
                    rating: meta_tags::Rating::from_half_stars(8),
                    ..Default::default()
                },
                audio: meta_tags::AudioProperties {
                    duration: Some(Duration::from_secs(100)),
                    sample_rate: Some(44_100),
                    bitrate_kbps: Some(320),
                    bit_depth: Some(16),
                    ..Default::default()
                },
                source: TagSource::Lofty,
                fallback_reason: None,
                charset_repaired: false,
            },
            cue,
            chapters: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn found(name: &str, class: FileClass) -> Found {
        Found {
            path: format!("/m/{name}").into(),
            stamp: Stamp {
                size: 10,
                mtime_ns: 5,
                inode: 7,
            },
            class,
        }
    }

    #[test]
    fn a_whole_file() {
        let records = tracks_of(RootId(1), &found("a.m4b", FileClass::Audio), &report(None));
        let [record] = records.as_slice() else {
            panic!("{records:?}");
        };
        assert_eq!(record.sub_index, 0);
        assert_eq!(record.kind, TrackKind::Audiobook);
        assert_eq!(record.tags.artists, ["A feat. B"]);
        assert_eq!(record.tags.composers, ["C"]);
        assert_eq!(record.tags.year, Some(1999));
        assert_eq!(record.audio.bitrate, Some(320_000));
        assert_eq!(record.rating_from_tags, Rating::from_half_stars(8));
        assert_eq!(record.file.inode, Some(7));
        assert_eq!(record.file.quick_hash, vec![9; 16]);
    }

    #[test]
    fn cue_tracks_split_the_file() {
        let cd = NonZeroU32::new(75).unwrap();
        let track = |number: u32, start: u64, end: Option<u64>| VirtualTrack {
            number,
            source: MediaSource::file("/m/a.flac").with_section(SourceSection::Span {
                start: MediaTime::new(start * 75, cd),
                end: end.map(|e| MediaTime::new(e * 75, cd)),
            }),
            tags: meta_tags::TrackTags {
                title: Some(format!("T{number}")),
                ..Default::default()
            },
            pregap_start: None,
            pre_emphasis: false,
        };
        let cue = CueReport {
            origin: CueOrigin::External,
            sheet: Some("/m/a.cue".into()),
            tracks: vec![
                track(1, 0, Some(40)),
                track(2, 40, None),
                track(3, 100, None),
            ],
            warnings: Vec::new(),
        };
        let records = tracks_of(
            RootId(1),
            &found("a.mkv", FileClass::Video),
            &report(Some(cue)),
        );
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].sub_index, 1);
        assert_eq!(records[0].frames, Some((0, Some(40 * 44_100))));
        assert_eq!(records[0].audio.duration, Some(Duration::from_secs(40)));
        assert_eq!(records[1].frames, Some((40 * 44_100, None)));
        assert_eq!(records[1].audio.duration, Some(Duration::from_secs(60)));
        assert_eq!(records[1].tags.title.as_deref(), Some("T2"));
        assert_eq!(records[1].kind, TrackKind::AudioFromVideo);
    }
}
