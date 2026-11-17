// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A parsed CUE sheet, as close to the text as is useful.

use std::fmt;

use helikodu_model::audio::MediaTime;

use super::text::SheetEncoding;
use super::warning::CueWarning;

/// A time in a CUE sheet: minutes, seconds and frames, of which there are 75 per
/// second. Converts exactly to a [`MediaTime`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CueTime {
    frames: u64,
}

impl CueTime {
    /// The number of frames in a second.
    pub const FRAMES_PER_SECOND: u64 = 75;

    /// The start of the file.
    pub const ZERO: Self = Self { frames: 0 };

    /// A time of `frames` frames.
    pub const fn from_frames(frames: u64) -> Self {
        Self { frames }
    }

    /// `minutes:seconds:frames`, or `None` when the seconds are 60 or more or the
    /// frames 75 or more.
    pub fn from_msf(minutes: u64, seconds: u8, frames: u8) -> Option<Self> {
        if seconds >= 60 || u64::from(frames) >= Self::FRAMES_PER_SECOND {
            return None;
        }
        let total = minutes
            .checked_mul(60 * Self::FRAMES_PER_SECOND)?
            .checked_add(u64::from(seconds) * Self::FRAMES_PER_SECOND + u64::from(frames))?;
        Some(Self { frames: total })
    }

    /// Parses `mm:ss:ff`. The minutes may have up to six digits; the seconds and
    /// frames one or two.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(':');
        let (m, s, f) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let digits = |p: &str, max: usize| {
            !p.is_empty() && p.len() <= max && p.bytes().all(|b| b.is_ascii_digit())
        };
        if !digits(m, 6) || !digits(s, 2) || !digits(f, 2) {
            return None;
        }
        Self::from_msf(m.parse().ok()?, s.parse().ok()?, f.parse().ok()?)
    }

    /// The number of frames.
    pub const fn frames(self) -> u64 {
        self.frames
    }

    /// The same time as a [`MediaTime`] with 75 ticks per second.
    pub const fn to_media_time(self) -> MediaTime {
        MediaTime::from_cue_frames(self.frames)
    }
}

impl From<CueTime> for MediaTime {
    fn from(time: CueTime) -> Self {
        time.to_media_time()
    }
}

impl fmt::Display for CueTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fps = Self::FRAMES_PER_SECOND;
        write!(
            f,
            "{:02}:{:02}:{:02}",
            self.frames / (60 * fps),
            self.frames / fps % 60,
            self.frames % fps
        )
    }
}

/// A `REM` line with a key, such as `REM GENRE Rock` or `REM DATE 1999`, or one of
/// the CD-TEXT keywords some programs write without `REM` (`COMPOSER`, `ARRANGER`,
/// `GENRE`, `MESSAGE`, `UPC_EAN`, `DISC_ID`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remark {
    /// The key in upper case.
    pub key: String,
    /// The value, without its quotes.
    pub value: String,
}

/// A `FILE` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueFile {
    /// The file name as decoded text.
    pub name: String,
    /// The file name as the sheet's own charset encodes it. Differs from the UTF-8
    /// bytes of [`name`](Self::name) when the sheet is in a legacy charset and
    /// the file names on disk kept that charset.
    pub raw_name: Vec<u8>,
    /// The file type (`WAVE`, `MP3`, `AIFF`, `BINARY`, `MOTOROLA`, …) in upper case.
    pub file_type: Option<String>,
    /// The line of the `FILE` command, counted from 1.
    pub line: usize,
}

/// The `FLAGS` of a track.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CueFlags {
    /// `DCP`: digital copy permitted.
    pub digital_copy_permitted: bool,
    /// `4CH`: four-channel audio.
    pub four_channel: bool,
    /// `PRE`: the audio has pre-emphasis.
    pub pre_emphasis: bool,
    /// `SCMS`: serial copy management system.
    pub serial_copy_management: bool,
}

/// An `INDEX` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CueIndex {
    /// The index number: 0 for the pregap, 1 for the track start, up to 99.
    pub number: u32,
    /// The position in the file.
    pub time: CueTime,
    /// The file the position is in, as an index into [`CueSheet::files`]: the
    /// last `FILE` before the `INDEX` line. A track's indexes may lie in different
    /// files (a pregap at the end of the previous file).
    pub file: Option<usize>,
    /// The line, counted from 1.
    pub line: usize,
}

/// A `TRACK` and the commands that follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueTrack {
    /// The track number.
    pub number: u32,
    /// The data type in upper case: `AUDIO`, or a data mode such as `MODE1/2352`.
    pub data_type: String,
    /// The file in effect at the `TRACK` line, as an index into [`CueSheet::files`].
    pub file: Option<usize>,
    /// `TITLE`.
    pub title: Option<String>,
    /// `PERFORMER`.
    pub performer: Option<String>,
    /// `SONGWRITER`.
    pub songwriter: Option<String>,
    /// `ISRC`.
    pub isrc: Option<String>,
    /// `FLAGS`.
    pub flags: CueFlags,
    /// `PREGAP`: silence to play before the track, not stored in the file.
    pub pregap: Option<CueTime>,
    /// `POSTGAP`: silence to play after the track, not stored in the file.
    pub postgap: Option<CueTime>,
    /// The `INDEX` lines, in sheet order.
    pub indexes: Vec<CueIndex>,
    /// The track's `REM` lines.
    pub remarks: Vec<Remark>,
    /// The line of the `TRACK` command, counted from 1.
    pub line: usize,
}

impl CueTrack {
    /// Whether this is an audio track.
    pub fn is_audio(&self) -> bool {
        self.data_type == "AUDIO"
    }

    /// The index with `number`.
    pub fn index(&self, number: u32) -> Option<&CueIndex> {
        self.indexes.iter().find(|i| i.number == number)
    }

    /// The first value of the remark `key` (upper case).
    pub fn remark(&self, key: &str) -> Option<&str> {
        remark(&self.remarks, key)
    }
}

/// A parsed CUE sheet. Parsing never fails on content: whatever could not be
/// understood is left out and reported in [`warnings`](Self::warnings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueSheet {
    /// `CATALOG`: the media catalogue number (UPC/EAN).
    pub catalog: Option<String>,
    /// `CDTEXTFILE`.
    pub cd_text_file: Option<String>,
    /// The album `TITLE`.
    pub title: Option<String>,
    /// The album `PERFORMER`.
    pub performer: Option<String>,
    /// The album `SONGWRITER`.
    pub songwriter: Option<String>,
    /// The `REM` lines before the first track.
    pub remarks: Vec<Remark>,
    /// The `FILE` lines.
    pub files: Vec<CueFile>,
    /// The tracks, in sheet order.
    pub tracks: Vec<CueTrack>,
    /// How the sheet's bytes were decoded.
    pub encoding: SheetEncoding,
    /// What the parser skipped or repaired, with line numbers.
    pub warnings: Vec<CueWarning>,
}

impl CueSheet {
    /// The first value of the album-level remark `key` (upper case).
    pub fn remark(&self, key: &str) -> Option<&str> {
        remark(&self.remarks, key)
    }

    /// The audio tracks.
    pub fn audio_tracks(&self) -> impl Iterator<Item = &CueTrack> {
        self.tracks.iter().filter(|t| t.is_audio())
    }
}

fn remark<'a>(remarks: &'a [Remark], key: &str) -> Option<&'a str> {
    remarks
        .iter()
        .find(|r| r.key == key)
        .map(|r| r.value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_parse_and_print() {
        let t = CueTime::parse("03:25:74").expect("valid");
        assert_eq!(t.frames(), 3 * 60 * 75 + 25 * 75 + 74);
        assert_eq!(t.to_string(), "03:25:74");
        assert_eq!(
            CueTime::parse("120:00:00").map(CueTime::frames),
            Some(540_000)
        );
        assert_eq!(CueTime::parse("0:0:0"), Some(CueTime::ZERO));
        for bad in [
            "",
            "1:2",
            "1:2:3:4",
            "00:60:00",
            "00:00:75",
            "a:00:00",
            "-1:00:00",
            "00:000:00",
            "1234567:00:00",
            "00: 1:00",
        ] {
            assert_eq!(CueTime::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn times_are_exact_media_times() {
        // 1 minute, 2 seconds and 37 frames: 4687 frames of 1/75 s.
        let t = CueTime::parse("01:02:37").expect("valid");
        let media = t.to_media_time();
        assert_eq!(media.ticks(), 4687);
        assert_eq!(media.ticks_per_second().get(), 75);
        // 588 samples per frame at 44.1 kHz, with no rounding.
        let rate = helikodu_model::audio::SampleRate::new(44_100).expect("valid rate");
        assert_eq!(media.frames_at(rate), 4687 * 588);
        assert_eq!(media, MediaTime::from_frames(4687 * 588, rate));
        // 37/75 s is 493.33 ms: no whole number of milliseconds equals it.
        assert!(media > MediaTime::from_millis(62_493));
        assert!(media < MediaTime::from_millis(62_494));
    }
}
