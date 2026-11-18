// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lyrics in the tags of the file: ID3v2 `SYLT` (synced) and `USLT`, the
//! Vorbis comments `LYRICS` and `UNSYNCEDLYRICS`, the MP4 `©lyr` atom and the
//! APEv2 `Lyrics` item, read with lofty; for the formats lofty lacks, the
//! lyrics FFmpeg reports. FFmpeg itself writes lyrics into MP3 files as a
//! `TXXX` frame named `USLT` (or `lyrics-<language>`); those count too. Text lyrics that are LRC come back synced.

use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use helikodu_ffmpeg_bridge::{MediaFile, StreamChoice};
use lofty::aac::AacFile;
use lofty::ape::{ApeFile, ApeTag};
use lofty::config::{ParseOptions, ParsingMode};
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{
    Frame, Id3v2Tag, SyncTextContentType, SynchronizedTextFrame, TimestampFormat,
};
use lofty::iff::aiff::AiffFile;
use lofty::iff::wav::WavFile;
use lofty::mp4::{AtomData, AtomIdent, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::musepack::MpcFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OpusFile, SpeexFile, VorbisFile};
use lofty::probe::Probe;
use lofty::tag::ItemValue;
use lofty::wavpack::WavPackFile;

use super::lrc::MAX_LINES;
use super::{EmbeddedKind, LyricLine, Lyrics, LyricsError, LyricsSource, WordTiming};

/// The longest text lyrics taken from a tag.
const MAX_TEXT_BYTES: usize = 1 << 20;
/// The longest line built from `SYLT` fragments.
const MAX_SYLT_LINE_BYTES: usize = 4096;

/// What the tags hold, before choosing.
#[derive(Default)]
struct Found {
    synced: Option<Lyrics>,
    text: Option<(String, EmbeddedKind)>,
}

impl Found {
    fn text(&mut self, text: &str, kind: EmbeddedKind) {
        if self.text.is_none() && !text.trim().is_empty() && text.len() <= MAX_TEXT_BYTES {
            self.text = Some((text.to_owned(), kind));
        }
    }

    fn into_lyrics(self) -> Vec<Lyrics> {
        let mut out: Vec<Lyrics> = self.synced.into_iter().collect();
        if let Some((text, kind)) = self.text
            && let Some(lyrics) = Lyrics::from_text(&text, LyricsSource::Embedded(kind))
        {
            out.push(lyrics);
        }
        out
    }
}

/// The lyrics in the tags of the file at `path`: the `SYLT` frame first, then
/// the text lyrics (synced when they are LRC). Empty when there are none.
pub fn read_embedded(path: &Path) -> Result<Vec<Lyrics>, LyricsError> {
    let mut file = File::open(path)?;
    let file_type = Probe::new(BufReader::new(&mut file))
        .guess_file_type()
        .map_err(|e| LyricsError::Tags(e.to_string()))?
        .file_type();
    let mut found = Found::default();
    match file_type {
        Some(FileType::Mpeg) => {
            let f: MpegFile = parse(&mut file)?;
            id3(&mut found, f.id3v2());
            ape(&mut found, f.ape());
        }
        Some(FileType::Aac) => id3(&mut found, parse::<AacFile>(&mut file)?.id3v2()),
        Some(FileType::Wav) => id3(&mut found, parse::<WavFile>(&mut file)?.id3v2()),
        Some(FileType::Aiff) => id3(&mut found, parse::<AiffFile>(&mut file)?.id3v2()),
        Some(FileType::Flac) => {
            let f: FlacFile = parse(&mut file)?;
            vorbis(&mut found, f.vorbis_comments());
            id3(&mut found, f.id3v2());
        }
        Some(FileType::Opus) => vorbis(
            &mut found,
            Some(parse::<OpusFile>(&mut file)?.vorbis_comments()),
        ),
        Some(FileType::Vorbis) => {
            vorbis(
                &mut found,
                Some(parse::<VorbisFile>(&mut file)?.vorbis_comments()),
            );
        }
        Some(FileType::Speex) => {
            vorbis(
                &mut found,
                Some(parse::<SpeexFile>(&mut file)?.vorbis_comments()),
            );
        }
        Some(FileType::Mp4) => {
            let f: Mp4File = parse(&mut file)?;
            if let Some(atom) = f
                .ilst()
                .and_then(|ilst| ilst.get(&AtomIdent::Fourcc(*b"\xa9lyr")))
            {
                for data in atom.data() {
                    if let AtomData::UTF8(s) | AtomData::UTF16(s) = data {
                        found.text(s, EmbeddedKind::Mp4);
                    }
                }
            }
        }
        Some(FileType::Ape) => ape(&mut found, parse::<ApeFile>(&mut file)?.ape()),
        Some(FileType::Mpc) => ape(&mut found, parse::<MpcFile>(&mut file)?.ape()),
        Some(FileType::WavPack) => ape(&mut found, parse::<WavPackFile>(&mut file)?.ape()),
        _ => ffmpeg(&mut found, file)?,
    }
    Ok(found.into_lyrics())
}

fn parse<F: AudioFile>(file: &mut File) -> Result<F, LyricsError> {
    file.seek(SeekFrom::Start(0))?;
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(false)
        .parsing_mode(ParsingMode::BestAttempt);
    crate::tags::guard_parse(|| F::read_from(&mut BufReader::new(&mut *file), options))
        .map_err(LyricsError::Tags)?
        .map_err(|e| LyricsError::Tags(e.to_string()))
}

fn id3(found: &mut Found, tag: Option<&Id3v2Tag>) {
    let Some(tag) = tag else { return };
    for frame in tag {
        match frame {
            Frame::UnsynchronizedText(f) => found.text(&f.content, EmbeddedKind::Id3Unsynced),
            // FFmpeg writes its `lyrics` key as `TXXX:USLT` or `TXXX:lyrics-<language>`.
            Frame::UserText(f) if lyrics_description(&f.description) => {
                found.text(&f.content, EmbeddedKind::Id3Unsynced);
            }
            Frame::Binary(f) if f.id().as_str() == "SYLT" && found.synced.is_none() => {
                if let Ok(sylt) = SynchronizedTextFrame::parse(&f.data, f.flags()) {
                    found.synced = synced_from_sylt(&sylt);
                }
            }
            _ => {}
        }
    }
}

/// Whether a `TXXX` frame with this description holds lyrics.
fn lyrics_description(description: &str) -> bool {
    let d = description.trim();
    ["USLT", "LYRICS", "UNSYNCEDLYRICS", "UNSYNCED LYRICS"]
        .iter()
        .any(|name| d.eq_ignore_ascii_case(name))
        || d.get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("lyrics-"))
}

fn vorbis(found: &mut Found, comments: Option<&VorbisComments>) {
    let Some(comments) = comments else { return };
    // `LYRICS` before `UNSYNCEDLYRICS`, whatever their order in the file.
    for wanted in ["LYRICS", "UNSYNCEDLYRICS", "UNSYNCED LYRICS"] {
        for (key, value) in comments.items() {
            if key.eq_ignore_ascii_case(wanted) {
                found.text(value, EmbeddedKind::VorbisComment);
            }
        }
    }
}

fn ape(found: &mut Found, tag: Option<&ApeTag>) {
    let Some(tag) = tag else { return };
    for item in tag {
        if let ItemValue::Text(text) = item.value()
            && item.key().eq_ignore_ascii_case("lyrics")
        {
            found.text(text, EmbeddedKind::Ape);
        }
    }
}

/// The formats lofty does not read (WMA, Matroska, …): FFmpeg names a `USLT`
/// frame `lyrics-<language>` and an ASF one `WM/Lyrics`.
fn ffmpeg(found: &mut Found, file: File) -> Result<(), LyricsError> {
    let media = MediaFile::open_file(file).map_err(|e| LyricsError::Tags(e.to_string()))?;
    let tags = match media.choose_stream(StreamChoice::Auto).ok() {
        Some(index) => media.tags_with_stream(index),
        None => media.tags().clone(),
    };
    for (key, value) in tags.iter() {
        let lower = key.to_ascii_lowercase();
        if lower == "lyrics"
            || lower.starts_with("lyrics-")
            || lower == "wm/lyrics"
            || lower == "unsyncedlyrics"
        {
            found.text(value, EmbeddedKind::Other);
        }
    }
    Ok(())
}

/// The lines of a `SYLT` frame with millisecond times. Taggers write either one
/// entry per line, or one per word or syllable with a line break at the start
/// of the entry that begins a line; both give lines here, the second with its
/// word times.
fn synced_from_sylt(frame: &SynchronizedTextFrame<'_>) -> Option<Lyrics> {
    if frame.timestamp_format != TimestampFormat::MS
        || !matches!(
            frame.content_type,
            SyncTextContentType::Lyrics | SyncTextContentType::TextTranscription
        )
    {
        return None;
    }
    let fragments = frame
        .content
        .iter()
        .any(|(_, text)| text.starts_with(['\n', '\r']));
    let mut raw: Vec<LyricLine> = Vec::new();
    for (time, text) in frame.content.iter().take(MAX_LINES * 8) {
        let time = Duration::from_millis(u64::from(*time));
        let new_line = !fragments || raw.is_empty() || text.starts_with(['\n', '\r']);
        let body = if fragments {
            text.trim_start_matches(['\n', '\r'])
        } else {
            text.as_str()
        };
        if new_line {
            if raw.len() >= MAX_LINES {
                break;
            }
            raw.push(LyricLine {
                time: Some(time),
                text: String::new(),
                words: Vec::new(),
                verse_start: false,
            });
        }
        if let Some(line) = raw.last_mut()
            && line.text.len() < MAX_SYLT_LINE_BYTES
        {
            if fragments && !body.is_empty() {
                line.words.push(WordTiming {
                    time,
                    start: line.text.len(),
                });
            }
            line.text.push_str(body);
        }
    }
    // Empty entries are pauses: the next line starts a verse.
    let mut lines: Vec<LyricLine> = Vec::with_capacity(raw.len());
    let mut gap = false;
    for mut line in raw {
        let lead = line.text.len() - line.text.trim_start().len();
        line.text = line.text.trim().to_owned();
        if line.text.is_empty() {
            gap = !lines.is_empty();
            continue;
        }
        let len = line.text.len();
        for word in &mut line.words {
            word.start = word.start.saturating_sub(lead).min(len);
        }
        line.words.retain(|w| w.start < len);
        line.verse_start = gap;
        gap = false;
        lines.push(line);
    }
    lines.sort_by_key(|l| l.time);
    (!lines.is_empty()).then(|| Lyrics {
        lines,
        synced: true,
        source: LyricsSource::Embedded(EmbeddedKind::Id3Synced),
        metadata: super::LrcMetadata {
            language: std::str::from_utf8(&frame.language).ok().map(str::to_owned),
            ..super::LrcMetadata::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::TextEncoding;

    fn frame(content: &[(u32, &str)]) -> SynchronizedTextFrame<'static> {
        SynchronizedTextFrame::new(
            TextEncoding::UTF8,
            *b"eng",
            TimestampFormat::MS,
            SyncTextContentType::Lyrics,
            None,
            content.iter().map(|(t, s)| (*t, (*s).to_owned())).collect(),
        )
    }

    #[test]
    fn sylt_with_one_entry_per_line() {
        let lyrics = synced_from_sylt(&frame(&[
            (1000, "one"),
            (2500, "two"),
            (3000, ""),
            (4000, "three"),
        ]))
        .expect("lines");
        let lines: Vec<_> = lyrics
            .lines
            .iter()
            .map(|l| (l.time, l.text.as_str(), l.verse_start))
            .collect();
        assert_eq!(
            lines,
            [
                (Some(Duration::from_millis(1000)), "one", false),
                (Some(Duration::from_millis(2500)), "two", false),
                (Some(Duration::from_millis(4000)), "three", true),
            ]
        );
        assert_eq!(lyrics.metadata.language.as_deref(), Some("eng"));
    }

    #[test]
    fn sylt_with_syllables() {
        let lyrics = synced_from_sylt(&frame(&[
            (1000, "Salt "),
            (1300, "on "),
            (1600, "the "),
            (1900, "win"),
            (2100, "dow"),
            (3000, "\nThe "),
            (3400, "kettle"),
        ]))
        .expect("lines");
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "Salt on the window");
        assert_eq!(lyrics.lines[0].words.len(), 5);
        assert_eq!(lyrics.lines[1].text, "The kettle");
        assert_eq!(lyrics.lines[1].time, Some(Duration::from_millis(3000)));
    }

    #[test]
    fn sylt_in_mpeg_frames_is_not_used() {
        let mut f = frame(&[(10, "x")]);
        f.timestamp_format = TimestampFormat::MPEG;
        assert!(synced_from_sylt(&f).is_none());
    }
}
