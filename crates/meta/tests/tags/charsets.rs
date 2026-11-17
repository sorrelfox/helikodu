// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Legacy charsets in ID3v1 fields and ID3v2.3 Latin-1 frames.

use std::borrow::Cow;
use std::path::Path;

use helikodu_meta::tags::{ReadOptions, read_tags};
use lofty::TextEncoding;
use lofty::config::WriteOptions;
use lofty::id3::v1::Id3v1Tag;
use lofty::id3::v2::{Frame, FrameId, Id3v2Tag, TextInformationFrame};
use lofty::tag::TagExt;

use crate::media::{as_latin1, encode};

/// An MP3 file without tags, named `name`.
fn untagged_mp3(name: &str) -> Option<std::path::PathBuf> {
    encode(name, "libmp3lame", &["-id3v2_version", "0"], "mp3")
}

/// Writes an ID3v2.3 tag of Latin-1 frames holding `fields` as they are.
fn write_latin1_id3v23(path: &Path, fields: &[(&str, &str)]) {
    let mut tag = Id3v2Tag::new();
    for (id, text) in fields {
        let id = FrameId::new(Cow::Owned((*id).to_owned())).expect("frame ID");
        tag.insert(Frame::Text(TextInformationFrame::new(
            id,
            TextEncoding::Latin1,
            (*text).to_owned(),
        )));
    }
    let mut options = WriteOptions::default();
    options.use_id3v23(true);
    tag.save_to_path(path, options).expect("write ID3v2.3");
}

#[test]
fn cp1251_in_id3v23_latin1_frames() {
    let Some(path) = untagged_mp3("cp1251-id3v23.mp3") else {
        return;
    };
    write_latin1_id3v23(
        &path,
        &[
            ("TIT2", &as_latin1("Группа крови", "windows-1251")),
            ("TPE1", &as_latin1("Кино", "windows-1251")),
            ("TALB", &as_latin1("Группа крови", "windows-1251")),
            ("TCON", "Rock"),
        ],
    );
    let file = read_tags(&path, &ReadOptions::default()).expect("read");
    assert!(file.charset_repaired);
    assert_eq!(file.tags.title.as_deref(), Some("Группа крови"));
    assert_eq!(file.tags.artist.as_deref(), Some("Кино"));
    assert_eq!(file.tags.album.as_deref(), Some("Группа крови"));
    assert_eq!(file.tags.genres, ["Rock"]);

    let raw = ReadOptions {
        repair_charsets: false,
        ..ReadOptions::default()
    };
    let file = read_tags(&path, &raw).expect("read");
    assert!(!file.charset_repaired);
    assert_eq!(
        file.tags.artist.as_deref(),
        Some(as_latin1("Кино", "windows-1251").as_str())
    );
}

#[test]
fn shift_jis_in_id3v1() {
    let Some(path) = untagged_mp3("sjis-id3v1.mp3") else {
        return;
    };
    let tag = Id3v1Tag {
        title: Some(as_latin1("ひこうき雲", "shift_jis")),
        artist: Some(as_latin1("荒井由実", "shift_jis")),
        album: Some(as_latin1("ひこうき雲", "shift_jis")),
        year: Some(1973),
        track_number: Some(1),
        genre: Some(13),
        ..Id3v1Tag::default()
    };
    tag.save_to_path(&path, WriteOptions::default())
        .expect("write ID3v1");
    let file = read_tags(&path, &ReadOptions::default()).expect("read");
    assert!(file.charset_repaired);
    assert_eq!(file.tags.title.as_deref(), Some("ひこうき雲"));
    assert_eq!(file.tags.artist.as_deref(), Some("荒井由実"));
    assert_eq!(file.tags.year(), Some(1973));
    assert_eq!(file.tags.track_number, Some(1));
    assert_eq!(file.tags.genres, ["Pop"]);
}

#[test]
fn gbk_with_a_chosen_charset() {
    let Some(path) = untagged_mp3("gbk-id3v23.mp3") else {
        return;
    };
    write_latin1_id3v23(
        &path,
        &[
            ("TIT2", &as_latin1("甜蜜蜜", "gbk")),
            ("TPE1", &as_latin1("邓丽君", "gbk")),
        ],
    );
    let options = ReadOptions {
        legacy_charset: Some("gbk".into()),
        ..ReadOptions::default()
    };
    let file = read_tags(&path, &options).expect("read");
    assert_eq!(file.tags.title.as_deref(), Some("甜蜜蜜"));
    assert_eq!(file.tags.artist.as_deref(), Some("邓丽君"));

    let bad = ReadOptions {
        legacy_charset: Some("no-such-charset".into()),
        ..ReadOptions::default()
    };
    assert!(read_tags(&path, &bad).is_err());
}

#[test]
fn western_latin1_is_left_alone() {
    let Some(path) = untagged_mp3("latin1-western.mp3") else {
        return;
    };
    write_latin1_id3v23(
        &path,
        &[
            ("TIT2", "Ace of Spades"),
            ("TPE1", "Motörhead"),
            ("TALB", "Ágætis byrjun"),
            ("TPE2", "Hüsker Dü"),
            ("TCOM", "Françoise Hardy"),
        ],
    );
    let file = read_tags(&path, &ReadOptions::default()).expect("read");
    assert!(!file.charset_repaired);
    assert_eq!(file.tags.artist.as_deref(), Some("Motörhead"));
    assert_eq!(file.tags.album.as_deref(), Some("Ágætis byrjun"));
    assert_eq!(file.tags.album_artist.as_deref(), Some("Hüsker Dü"));
}

#[test]
fn utf8_bytes_in_a_latin1_frame() {
    let Some(path) = untagged_mp3("utf8-in-latin1.mp3") else {
        return;
    };
    write_latin1_id3v23(
        &path,
        &[
            ("TIT2", &as_latin1("Hoppípolla", "utf-8")),
            ("TPE1", &as_latin1("Sigur Rós", "utf-8")),
        ],
    );
    let file = read_tags(&path, &ReadOptions::default()).expect("read");
    assert!(file.charset_repaired);
    assert_eq!(file.tags.title.as_deref(), Some("Hoppípolla"));
    assert_eq!(file.tags.artist.as_deref(), Some("Sigur Rós"));
}
