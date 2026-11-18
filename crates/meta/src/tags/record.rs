// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The format-independent record of a track's tags and audio properties.

use std::time::Duration;

use super::rating::Rating;

/// What a person did on a track. The roles follow MusicBrainz Picard's tag names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// A track artist (`ARTISTS`, or `ARTIST` when there is no list).
    Artist,
    /// An album artist (`ALBUMARTISTS`, or `ALBUMARTIST`).
    AlbumArtist,
    /// A composer.
    Composer,
    /// A conductor.
    Conductor,
    /// A lyricist.
    Lyricist,
    /// A writer (music and lyrics, when not told apart).
    Writer,
    /// An arranger.
    Arranger,
    /// A remixer.
    Remixer,
    /// A producer.
    Producer,
    /// An engineer.
    Engineer,
    /// A mixing engineer.
    Mixer,
    /// A DJ mixer.
    DjMixer,
    /// A performer, possibly with an instrument or vocal part.
    Performer,
}

/// One credited person (or group) on a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    /// What they did.
    pub role: Role,
    /// The name as tagged.
    pub name: String,
    /// The instrument or vocal part of a performer, such as `guitar` or `lead vocals`.
    pub instrument: Option<String>,
}

/// MusicBrainz identifiers, as written by Picard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MusicBrainzIds {
    /// The recording (Vorbis `MUSICBRAINZ_TRACKID`, ID3 `UFID:http://musicbrainz.org`).
    pub recording: Option<String>,
    /// The track on the release (`MUSICBRAINZ_RELEASETRACKID`).
    pub release_track: Option<String>,
    /// The release (`MUSICBRAINZ_ALBUMID`).
    pub release: Option<String>,
    /// The release group (`MUSICBRAINZ_RELEASEGROUPID`).
    pub release_group: Option<String>,
    /// The track artists, in the order of [`TrackTags::artists`].
    pub artists: Vec<String>,
    /// The album artists, in the order of [`TrackTags::album_artists`].
    pub album_artists: Vec<String>,
    /// The works.
    pub works: Vec<String>,
}

impl MusicBrainzIds {
    /// Whether no identifier is set.
    pub fn is_empty(&self) -> bool {
        self.recording.is_none()
            && self.release_track.is_none()
            && self.release.is_none()
            && self.release_group.is_none()
            && self.artists.is_empty()
            && self.album_artists.is_empty()
            && self.works.is_empty()
    }
}

/// Loudness tags as stored in the file, not yet normalized to one reference.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReplayGainTags {
    /// `REPLAYGAIN_TRACK_GAIN` in dB, relative to the ReplayGain reference.
    pub track_gain_db: Option<f64>,
    /// `REPLAYGAIN_TRACK_PEAK`, linear.
    pub track_peak: Option<f64>,
    /// `REPLAYGAIN_ALBUM_GAIN` in dB.
    pub album_gain_db: Option<f64>,
    /// `REPLAYGAIN_ALBUM_PEAK`, linear.
    pub album_peak: Option<f64>,
    /// Opus `R128_TRACK_GAIN`: Q7.8 fixed point (dB × 256) relative to −23 LUFS.
    pub r128_track_gain: Option<i16>,
    /// Opus `R128_ALBUM_GAIN`: Q7.8 fixed point relative to −23 LUFS.
    pub r128_album_gain: Option<i16>,
}

impl ReplayGainTags {
    /// Whether any value is set.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The track gain relative to the ReplayGain 2.0 reference (−18 LUFS): the
    /// `REPLAYGAIN_TRACK_GAIN` tag, or else the Opus `R128_TRACK_GAIN` tag converted.
    pub fn track_gain_rg2_db(&self) -> Option<f64> {
        self.track_gain_db
            .or(self.r128_track_gain.map(r128_to_replaygain_db))
    }

    /// The album gain relative to the ReplayGain 2.0 reference, like
    /// [`track_gain_rg2_db`](Self::track_gain_rg2_db).
    pub fn album_gain_rg2_db(&self) -> Option<f64> {
        self.album_gain_db
            .or(self.r128_album_gain.map(r128_to_replaygain_db))
    }
}

/// An Opus `R128_*_GAIN` value (Q7.8, relative to −23 LUFS) as a gain relative to
/// the ReplayGain 2.0 reference of −18 LUFS.
pub fn r128_to_replaygain_db(q7_8: i16) -> f64 {
    f64::from(q7_8) / 256.0 + 5.0
}

/// Every tag of a track that the library uses, independent of the tag format.
///
/// Text is trimmed and never empty. Multi-valued fields keep the order of the file.
/// Fields the record has no place for are kept in [`extra`](Self::extra) under
/// Picard's internal name (`bpm`, `isrc`, `barcode`, `media`, …) or, for fields
/// Picard does not know, under the tag's own key in lower case.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackTags {
    /// The title.
    pub title: Option<String>,
    /// The title for sorting (`TITLESORT`, `TSOT`, `sonm`).
    pub title_sort: Option<String>,
    /// The track artist as credited, such as `A feat. B` (`ARTIST`, `TPE1`).
    pub artist: Option<String>,
    /// The individual track artists (`ARTISTS`; otherwise every `ARTIST` value).
    pub artists: Vec<String>,
    /// The track artist for sorting.
    pub artist_sort: Option<String>,
    /// The album title.
    pub album: Option<String>,
    /// The album title for sorting.
    pub album_sort: Option<String>,
    /// The album artist as credited (`ALBUMARTIST`, `TPE2`, `aART`).
    pub album_artist: Option<String>,
    /// The individual album artists (`ALBUMARTISTS`; otherwise every `ALBUMARTIST` value).
    pub album_artists: Vec<String>,
    /// The album artist for sorting.
    pub album_artist_sort: Option<String>,
    /// The composer for sorting.
    pub composer_sort: Option<String>,
    /// The track number.
    pub track_number: Option<u32>,
    /// The number of tracks on the disc.
    pub track_total: Option<u32>,
    /// The disc number.
    pub disc_number: Option<u32>,
    /// The number of discs.
    pub disc_total: Option<u32>,
    /// The disc subtitle.
    pub disc_subtitle: Option<String>,
    /// The release date as tagged: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`, possibly with a time.
    pub date: Option<String>,
    /// The original release date, or only its year.
    pub original_date: Option<String>,
    /// The genres.
    pub genres: Vec<String>,
    /// The people credited on the track other than the artists and album artists
    /// (composers, conductors, performers, …), in tag order within each role.
    pub credits: Vec<Credit>,
    /// The record labels.
    pub labels: Vec<String>,
    /// The catalogue numbers.
    pub catalog_numbers: Vec<String>,
    /// The MusicBrainz release types, such as `album` and `live`.
    pub release_types: Vec<String>,
    /// MusicBrainz identifiers.
    pub musicbrainz: MusicBrainzIds,
    /// The comment without a description (`COMMENT`, `COMM`, `©cmt`).
    pub comment: Option<String>,
    /// Whether the file carries lyrics (unsynchronized or synchronized).
    pub has_lyrics: bool,
    /// Loudness tags.
    pub replay_gain: ReplayGainTags,
    /// The rating found in the tags, on the half-star scale.
    pub rating: Option<Rating>,
    /// The compilation flag (`COMPILATION`, `TCMP`, `cpil`).
    pub compilation: Option<bool>,
    /// The content group (`GROUPING`, `TIT1` or `GRP1`, `©grp`).
    pub grouping: Option<String>,
    /// The classical work.
    pub work: Option<String>,
    /// The movement name.
    pub movement: Option<String>,
    /// The movement number.
    pub movement_number: Option<u32>,
    /// The number of movements.
    pub movement_total: Option<u32>,
    /// Every other field, as key and value pairs; a key may repeat.
    pub extra: Vec<(String, String)>,
}

impl TrackTags {
    /// The year of [`date`](Self::date).
    pub fn year(&self) -> Option<i32> {
        self.date.as_deref().and_then(leading_year)
    }

    /// The year of [`original_date`](Self::original_date).
    pub fn original_year(&self) -> Option<i32> {
        self.original_date.as_deref().and_then(leading_year)
    }

    /// The credits with `role`, in tag order.
    pub fn credits_with(&self, role: Role) -> impl Iterator<Item = &Credit> {
        self.credits.iter().filter(move |c| c.role == role)
    }

    /// Every person credited on the track, the artists and album artists first.
    pub fn people(&self) -> impl Iterator<Item = (Role, &str)> {
        self.artists
            .iter()
            .map(|a| (Role::Artist, a.as_str()))
            .chain(
                self.album_artists
                    .iter()
                    .map(|a| (Role::AlbumArtist, a.as_str())),
            )
            .chain(self.credits.iter().map(|c| (c.role, c.name.as_str())))
    }

    /// Every value of `key` in [`extra`](Self::extra), compared without regard to ASCII case.
    pub fn extra_values<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> {
        self.extra
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// The year at the start of a date such as `1999`, `1999-04-01` or `1999-04-01T10:00`.
fn leading_year(date: &str) -> Option<i32> {
    let digits: String = date
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.len() != 4 {
        return None;
    }
    digits.parse().ok().filter(|y| *y > 0)
}

/// The audio properties reported with the tags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioProperties {
    /// The duration.
    pub duration: Option<Duration>,
    /// The sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// The number of channels.
    pub channels: Option<u16>,
    /// The bits per sample of lossless and PCM formats.
    pub bit_depth: Option<u32>,
    /// The audio bitrate in kbit/s (the overall bitrate when the audio bitrate is unknown).
    pub bitrate_kbps: Option<u32>,
    /// The codec, in lower case: `mp3`, `flac`, `aac`, `alac`, `opus`, `vorbis`, …
    pub codec: Option<String>,
    /// The container, in lower case: `mpeg`, `flac`, `mp4`, `ogg`, `matroska`, …
    pub container: Option<String>,
}

/// Which reader produced the tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagSource {
    /// The pure-Rust tag library. These tags can be written back.
    Lofty,
    /// FFmpeg, for formats the tag library does not cover or cannot parse. These
    /// tags are read-only.
    Ffmpeg,
    /// The format's own decoder (tracker modules, game music, SID tunes, MIDI
    /// files), which reads the text the file carries. These tags are read-only.
    Decoder,
}

/// The result of reading one file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileTags {
    /// The tags.
    pub tags: TrackTags,
    /// The audio properties.
    pub audio: AudioProperties,
    /// Which reader produced them.
    pub source: TagSource,
    /// Why the tag library was not used, when [`source`](Self::source) is FFmpeg.
    pub fallback_reason: Option<String>,
    /// Whether some text was decoded from a legacy charset or repaired from mojibake.
    pub charset_repaired: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_come_from_the_start_of_the_date() {
        let mut t = TrackTags {
            date: Some("1999-04-01".into()),
            original_date: Some("1969".into()),
            ..TrackTags::default()
        };
        assert_eq!(t.year(), Some(1999));
        assert_eq!(t.original_year(), Some(1969));
        t.date = Some("99".into());
        assert_eq!(t.year(), None);
        t.date = Some("2004T10".into());
        assert_eq!(t.year(), Some(2004));
    }

    #[test]
    fn r128_gains_are_moved_to_the_replaygain_reference() {
        let rg = ReplayGainTags {
            r128_track_gain: Some(-2560),
            album_gain_db: Some(-3.0),
            r128_album_gain: Some(0),
            ..ReplayGainTags::default()
        };
        assert_eq!(rg.track_gain_rg2_db(), Some(-5.0));
        assert_eq!(rg.album_gain_rg2_db(), Some(-3.0));
        assert!(!rg.is_empty());
    }
}
