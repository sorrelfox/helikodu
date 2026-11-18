// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lyrics through the public API: LRC files as they are found in the wild,
//! lyrics in the tags of files made by the `ffmpeg` tool (a `SYLT` frame added
//! with lofty), files beside the track, and the choice between them. A test
//! whose tool or encoder is missing is skipped with a message, not failed.

#![forbid(unsafe_code)]

mod media;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use helikodu_meta::lyrics::{
    EmbeddedKind, LocalLyrics, Lyrics, LyricsProvider, LyricsQuery, LyricsSource, SidecarKind,
    read_embedded, read_sidecar, sidecar_paths,
};
use helikodu_meta::tags::{ReadOptions, read_tags};
use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::id3::v2::{
    BinaryFrame, Frame, FrameId, SyncTextContentType, SynchronizedTextFrame, TimestampFormat,
    UnsynchronizedTextFrame,
};
use lofty::mpeg::MpegFile;
use lofty::tag::TagExt;

use crate::media::{dir, encode, metadata};

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn texts(lyrics: &Lyrics) -> Vec<&str> {
    lyrics.lines.iter().map(|l| l.text.as_str()).collect()
}

/// A fresh folder for one test.
fn folder(name: &str) -> PathBuf {
    let d = dir().join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("create the folder");
    d
}

fn encoded(text: &str, label: &str) -> Vec<u8> {
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).expect("known label");
    let (bytes, _, errors) = encoding.encode(text);
    assert!(!errors);
    bytes.into_owned()
}

fn sidecar(track: &Path) -> Lyrics {
    let found = sidecar_paths(track);
    let (path, kind) = found.first().expect("a lyrics file");
    read_sidecar(path, *kind, None)
        .expect("read")
        .expect("lines")
}

#[test]
fn an_lrc_file_with_bom_crlf_and_offset() {
    let d = folder("bom-crlf");
    let track = d.join("01 Shoreline.flac");
    fs::write(&track, b"").expect("track");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(
        b"[ti:Shoreline, 6 a.m.]\r\n[offset:+200]\r\n\r\n[00:46.00]The tide comes in\r\n\
          [00:58.20][02:38.00]Grey light\r\n[01:11.00]\r\n[01:12.50]I count the boats\r\n",
    );
    fs::write(d.join("01 Shoreline.lrc"), bytes).expect("lrc");
    let lyrics = sidecar(&track);
    assert!(lyrics.synced);
    assert_eq!(
        texts(&lyrics),
        [
            "The tide comes in",
            "Grey light",
            "I count the boats",
            "Grey light"
        ]
    );
    let times: Vec<_> = lyrics.lines.iter().map(|l| l.time).collect();
    assert_eq!(
        times,
        [
            Some(ms(45_800)),
            Some(ms(58_000)),
            Some(ms(72_300)),
            Some(ms(157_800))
        ]
    );
    assert!(lyrics.lines[2].verse_start);
    assert_eq!(lyrics.metadata.offset_ms, 200);
    assert_eq!(lyrics.metadata.title.as_deref(), Some("Shoreline, 6 a.m."));
    match &lyrics.source {
        LyricsSource::Sidecar { kind, charset, .. } => {
            assert_eq!(*kind, SidecarKind::Lrc);
            assert_eq!(*charset, "UTF-8");
        }
        other => panic!("not a file: {other:?}"),
    }
}

#[test]
fn a_cp1251_lrc_file_and_a_utf16_text_file() {
    let d = folder("charsets");
    let track = d.join("Кино.mp3");
    fs::write(&track, b"").expect("track");
    let text = "[ar:Кино]\n[00:10.00]Белый день, тёмная ночь\n[00:15.00]Звезда по имени Солнце\n";
    fs::write(d.join("Кино.lrc"), encoded(text, "windows-1251")).expect("lrc");
    let lyrics = sidecar(&track);
    assert_eq!(
        texts(&lyrics),
        ["Белый день, тёмная ночь", "Звезда по имени Солнце"]
    );
    assert_eq!(lyrics.metadata.artist.as_deref(), Some("Кино"));
    assert!(matches!(
        lyrics.source,
        LyricsSource::Sidecar {
            charset: "windows-1251",
            ..
        }
    ));

    let other = d.join("Song.ogg");
    fs::write(&other, b"").expect("track");
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "Первая строка\r\nВторая строка\r\n".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(d.join("Song.txt"), bytes).expect("txt");
    let lyrics = sidecar(&other);
    assert!(!lyrics.synced);
    assert_eq!(texts(&lyrics), ["Первая строка", "Вторая строка"]);
}

#[test]
fn a_huge_lyrics_file_is_refused() {
    let d = folder("huge");
    let track = d.join("a.flac");
    fs::write(&track, b"").expect("track");
    let path = d.join("a.lrc");
    fs::write(&path, vec![b'x'; 2 << 20]).expect("lrc");
    assert!(read_sidecar(&path, SidecarKind::Lrc, None).is_err());
}

/// Adds `frame` to the ID3v2 tag of the MP3 file at `path`.
fn add_id3_frame(path: &Path, frame: Frame<'static>) {
    let mp3 = MpegFile::read_from(
        &mut fs::File::open(path).expect("open"),
        ParseOptions::new(),
    )
    .expect("lofty reads the file");
    let mut tag = mp3.id3v2().cloned().unwrap_or_default();
    tag.insert(frame);
    tag.save_to_path(path, WriteOptions::default())
        .expect("lofty writes the tag");
}

#[test]
fn ffmpeg_lyrics_in_an_mp3_file() {
    // FFmpeg writes `TXXX:USLT`.
    let args = metadata(&[("title", "x"), ("lyrics", "First line\nSecond line")]);
    let Some(path) = encode("txxx.mp3", "libmp3lame", &args, "mp3") else {
        return;
    };
    let found = read_embedded(&path).expect("tags");
    assert_eq!(found.len(), 1);
    assert_eq!(texts(&found[0]), ["First line", "Second line"]);
    // The scanner sees them too.
    let tags = read_tags(&path, &ReadOptions::default()).expect("read the tags");
    assert!(tags.tags.has_lyrics);
}

#[test]
fn uslt_in_an_mp3_file() {
    let Some(path) = encode(
        "uslt.mp3",
        "libmp3lame",
        &metadata(&[("title", "x")]),
        "mp3",
    ) else {
        return;
    };
    add_id3_frame(
        &path,
        Frame::UnsynchronizedText(UnsynchronizedTextFrame::new(
            TextEncoding::UTF8,
            *b"eng",
            "",
            "First line\r\nSecond line",
        )),
    );
    let found = read_embedded(&path).expect("tags");
    assert_eq!(found.len(), 1);
    assert!(!found[0].synced);
    assert_eq!(texts(&found[0]), ["First line", "Second line"]);
    assert_eq!(
        found[0].source,
        LyricsSource::Embedded(EmbeddedKind::Id3Unsynced)
    );
}

#[test]
fn sylt_wins_over_uslt() {
    let Some(path) = encode(
        "sylt.mp3",
        "libmp3lame",
        &metadata(&[("lyrics", "Plain words")]),
        "mp3",
    ) else {
        return;
    };
    let sylt = SynchronizedTextFrame::new(
        TextEncoding::UTF8,
        *b"eng",
        TimestampFormat::MS,
        SyncTextContentType::Lyrics,
        None,
        vec![
            (1_000, "Timed one".to_owned()),
            (2_500, "Timed two".to_owned()),
        ],
    );
    let bytes = sylt.as_bytes(WriteOptions::default()).expect("frame bytes");
    let id = FrameId::new(std::borrow::Cow::Borrowed("SYLT")).expect("frame id");
    add_id3_frame(&path, Frame::Binary(BinaryFrame::new(id, bytes)));

    let found = read_embedded(&path).expect("tags");
    assert_eq!(found.len(), 2);
    assert!(found[0].synced);
    assert_eq!(texts(&found[0]), ["Timed one", "Timed two"]);
    assert_eq!(found[0].lines[1].time, Some(ms(2_500)));
    assert_eq!(
        found[1].source,
        LyricsSource::Embedded(EmbeddedKind::Id3Unsynced)
    );
    let best = LocalLyrics::default()
        .find(&LyricsQuery { path: &path })
        .expect("lyrics")
        .expect("found");
    assert_eq!(texts(&best), ["Timed one", "Timed two"]);
}

#[test]
fn lrc_in_a_flac_lyrics_comment() {
    let args = metadata(&[("LYRICS", "[00:01.00]One\n[00:02.00]Two")]);
    let Some(path) = encode("vorbis.flac", "flac", &args, "flac") else {
        return;
    };
    let found = read_embedded(&path).expect("tags");
    assert_eq!(found.len(), 1);
    assert!(found[0].synced);
    assert_eq!(texts(&found[0]), ["One", "Two"]);
    assert_eq!(
        found[0].source,
        LyricsSource::Embedded(EmbeddedKind::VorbisComment)
    );
}

#[test]
fn unsyncedlyrics_in_an_opus_file() {
    let args = metadata(&[("UNSYNCEDLYRICS", "Opus words")]);
    let Some(path) = encode("vorbis.opus", "libopus", &args, "opus") else {
        return;
    };
    let found = read_embedded(&path).expect("tags");
    assert_eq!(texts(&found[0]), ["Opus words"]);
}

#[test]
fn lyr_in_an_mp4_file() {
    let args = metadata(&[("lyrics", "Line A\n\nLine B")]);
    let Some(path) = encode("lyr.m4a", "aac", &args, "ipod") else {
        return;
    };
    let found = read_embedded(&path).expect("tags");
    assert_eq!(found.len(), 1);
    assert_eq!(texts(&found[0]), ["Line A", "Line B"]);
    assert!(found[0].lines[1].verse_start);
    assert_eq!(found[0].source, LyricsSource::Embedded(EmbeddedKind::Mp4));
}

#[test]
fn the_choice_between_files_and_tags() {
    let args = metadata(&[("LYRICS", "[00:01.00]From the tags")]);
    let Some(encoded) = encode("choice.flac", "flac", &args, "flac") else {
        return;
    };
    let d = folder("choice");
    let track = d.join("choice.flac");
    fs::copy(&encoded, &track).expect("copy");
    let provider = LocalLyrics::default();
    let query = LyricsQuery { path: &track };
    // Synced tags win over a plain file.
    fs::write(d.join("choice.txt"), "From a text file\n").expect("txt");
    let best = provider.find(&query).expect("lyrics").expect("found");
    assert_eq!(texts(&best), ["From the tags"]);
    // A synced file beside the track wins over synced tags.
    fs::write(d.join("choice.lrc"), "[00:01.00]From the file\n").expect("lrc");
    let all = provider.find_all(&query).expect("lyrics");
    let order: Vec<_> = all.iter().map(|l| l.lines[0].text.as_str()).collect();
    assert_eq!(
        order,
        ["From the file", "From the tags", "From a text file"]
    );
}

#[test]
fn a_file_without_lyrics() {
    let Some(path) = encode("none.flac", "flac", &metadata(&[("title", "x")]), "flac") else {
        return;
    };
    assert!(read_embedded(&path).expect("tags").is_empty());
    assert!(
        LocalLyrics::default()
            .find(&LyricsQuery { path: &path })
            .expect("no error")
            .is_none()
    );
}
