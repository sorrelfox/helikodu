// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a worker read from a file, turned into the library's track records.

use helikodu_meta::cue::VirtualTrack;
use helikodu_meta::tags::{self as meta_tags, Role};
use helikodu_meta::worker::FileReport;
use helikodu_model::audio::SampleRate;

use super::filter::{FileClass, is_audiobook};
use super::walk::Found;
use crate::catalog::{
    ArtistRole, AudioProperties, CatalogTags, Credit, FileStamp, ReplayGain, RootId, TrackCovers,
    TrackKind, TrackRecord, TrackSection, TrackTags,
};
use crate::stats::Rating;

/// The tracks of the file `found`, read by a worker into `report`: one per track of
/// its CUE sheet; or else, when the container has two chapters or more, one per
/// chapter; or else one for the whole file.
///
/// The file's stamp is the one the walk saw, so a file that changed while it was
/// being read is read again by the next scan. With `covers`, the covers of the
/// report replace the stored ones (the worker was asked for them).
pub(crate) fn tracks_of(
    root: RootId,
    found: &Found,
    report: &FileReport,
    covers: bool,
) -> Vec<TrackRecord> {
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
        covers: covers.then(|| TrackCovers {
            embedded: report.cover.clone(),
            folder: report.folder_cover.clone(),
        }),
        ..TrackRecord::new(root, &found.path, file)
    };
    // A track that starts after the end of the file (a sheet written for a longer
    // rip) is left out; without any track left, the file is one track.
    let length = base.audio.duration;
    let within =
        |track: &&VirtualTrack| length.is_none_or(|length| track.start().as_duration() < length);
    let sheet_tracks: Vec<TrackRecord> = report
        .cue
        .iter()
        .flat_map(|cue| &cue.tracks)
        .filter(within)
        .map(|track| section_track(&base, track, TrackSection::CueSheet))
        .collect();
    if !sheet_tracks.is_empty() {
        return sheet_tracks;
    }
    let chapters: Vec<TrackRecord> = report
        .chapters
        .iter()
        .filter(within)
        .map(|chapter| section_track(&base, chapter, TrackSection::Chapter))
        .collect();
    if chapters.len() > 1 {
        return chapters;
    }
    let tags = &report.tags.tags;
    vec![TrackRecord {
        tags: track_tags(tags),
        catalog: catalog_tags(tags),
        rating_from_tags: rating(tags),
        replay_gain: replay_gain(tags),
        ..base
    }]
}

/// A track that is a part of the file: a CUE sheet track or a chapter.
fn section_track(base: &TrackRecord, track: &VirtualTrack, section: TrackSection) -> TrackRecord {
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
        section,
        audio,
        tags: track_tags(&track.tags),
        catalog: catalog_tags(&track.tags),
        rating_from_tags: rating(&track.tags),
        replay_gain: replay_gain(&track.tags),
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

/// The tags of the album and artist model.
fn catalog_tags(tags: &meta_tags::TrackTags) -> CatalogTags {
    let mb = &tags.musicbrainz;
    CatalogTags {
        artist_credit: tags.artist.clone(),
        artist_sort: tags.artist_sort.clone(),
        album_artist_credit: tags.album_artist.clone(),
        album_artist_sort: tags.album_artist_sort.clone(),
        album_sort: tags.album_sort.clone(),
        composer_sort: tags.composer_sort.clone(),
        credits: tags
            .credits
            .iter()
            .filter_map(|credit| {
                Some(Credit {
                    role: role(credit.role)?,
                    name: credit.name.clone(),
                    instrument: credit.instrument.clone(),
                })
            })
            .collect(),
        compilation: tags.compilation,
        release_types: tags.release_types.clone(),
        release_mbid: mb.release.clone(),
        release_group_mbid: mb.release_group.clone(),
        artist_mbids: mb.artists.clone(),
        album_artist_mbids: mb.album_artists.clone(),
        track_total: tags.track_total,
        disc_total: tags.disc_total,
        disc_subtitle: tags.disc_subtitle.clone(),
        original_year: tags.original_year(),
        work: tags.work.clone(),
        movement: tags.movement.clone(),
        movement_no: tags.movement_number,
        has_lyrics: tags.has_lyrics,
    }
}

/// The library's role for a credit; the artists, album artists and composers have
/// their own fields.
fn role(role: Role) -> Option<ArtistRole> {
    Some(match role {
        Role::Artist | Role::AlbumArtist | Role::Composer => return None,
        Role::Conductor => ArtistRole::Conductor,
        Role::Lyricist => ArtistRole::Lyricist,
        Role::Writer => ArtistRole::Writer,
        Role::Arranger => ArtistRole::Arranger,
        Role::Remixer => ArtistRole::Remixer,
        Role::Producer => ArtistRole::Producer,
        Role::Engineer => ArtistRole::Engineer,
        Role::Mixer => ArtistRole::Mixer,
        Role::DjMixer => ArtistRole::DjMixer,
        Role::Performer => ArtistRole::Performer,
    })
}

/// The loudness values, relative to the ReplayGain 2.0 reference: the
/// `REPLAYGAIN_*` tags, or else the Opus `R128_*_GAIN` tags converted.
fn replay_gain(tags: &meta_tags::TrackTags) -> ReplayGain {
    let rg = &tags.replay_gain;
    ReplayGain {
        track_gain_db: rg.track_gain_rg2_db(),
        track_peak: rg.track_peak,
        album_gain_db: rg.album_gain_rg2_db(),
        album_peak: rg.album_peak,
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
                    replay_gain: meta_tags::ReplayGainTags {
                        track_gain_db: Some(-7.5),
                        track_peak: Some(0.98),
                        r128_album_gain: Some(-512),
                        ..Default::default()
                    },
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
            cover: None,
            folder_cover: None,
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
        let records = tracks_of(
            RootId(1),
            &found("a.m4b", FileClass::Audio),
            &report(None),
            true,
        );
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
        assert_eq!(record.replay_gain.track_gain_db, Some(-7.5));
        assert_eq!(record.replay_gain.track_peak, Some(0.98));
        // R128 −2 dB relative to −23 LUFS is +3 dB relative to −18 LUFS.
        assert_eq!(record.replay_gain.album_gain_db, Some(3.0));
        assert_eq!(record.replay_gain.album_peak, None);
        assert_eq!(record.covers, Some(TrackCovers::default()));
        let without = tracks_of(
            RootId(1),
            &found("a.flac", FileClass::Audio),
            &report(None),
            false,
        );
        assert_eq!(without[0].covers, None);
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
            true,
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
