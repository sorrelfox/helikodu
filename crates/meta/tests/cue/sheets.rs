// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! External sheets: parsing, charsets, resolution and hostile input.

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use helikodu_meta::cue::{
    CueOptions, CueSheet, CueTime, CueWarningKind, EncodingSource, MAX_SHEET_BYTES, PathRejection,
    VirtualTrack, read_cue,
};
use helikodu_meta::tags::Role;
use helikodu_model::audio::{MediaTime, SampleRate};

use crate::media::folder;

/// An album ripped by EAC to one file with gaps appended to the previous track.
const EAC_SINGLE_FILE: &str = r#"REM GENRE "Progressive Rock"
REM DATE 1977
REM DISCID 860B640B
REM COMMENT "ExactAudioCopy v1.6"
REM REPLAYGAIN_ALBUM_GAIN -7.89 dB
REM REPLAYGAIN_ALBUM_PEAK 0.988525
CATALOG 0077774644020
PERFORMER "The Example Band"
TITLE "Animals Revisited"
FILE "The Example Band - Animals Revisited.flac" WAVE
  TRACK 01 AUDIO
    TITLE "Wings (Part One)"
    PERFORMER "The Example Band"
    ISRC GBAYE7700001
    REM REPLAYGAIN_TRACK_GAIN -5.12 dB
    REM REPLAYGAIN_TRACK_PEAK 0.912
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Dogs"
    FLAGS DCP PRE
    INDEX 00 01:24:65
    INDEX 01 01:25:40
  TRACK 03 AUDIO
    TITLE "Pigs (Three Different Ones)"
    PERFORMER "Guest Singer"
    SONGWRITER "R. Writer"
    REM COMPOSER "R. Writer"
    INDEX 00 18:28:10
    INDEX 01 18:30:00
"#;

fn span(track: &VirtualTrack) -> (MediaTime, Option<MediaTime>) {
    (track.start(), track.end())
}

fn frames(n: u64) -> MediaTime {
    MediaTime::from_cue_frames(n)
}

fn kinds(warnings: &[helikodu_meta::cue::CueWarning]) -> Vec<CueWarningKind> {
    warnings.iter().map(|w| w.kind.clone()).collect()
}

fn check_eac_album(sheet: &CueSheet, folder: &Path) {
    assert!(sheet.warnings.is_empty(), "{:?}", sheet.warnings);
    let resolution = sheet.resolve(folder);
    assert!(resolution.warnings.is_empty(), "{:?}", resolution.warnings);
    let tracks = resolution.tracks;
    assert_eq!(tracks.len(), 3);
    let file = folder.join("The Example Band - Animals Revisited.flac");
    assert!(tracks.iter().all(|t| t.source.path == file));

    // INDEX 01 starts a track; the next track's INDEX 01 ends it.
    assert_eq!(span(&tracks[0]), (frames(0), Some(frames(85 * 75 + 40))));
    assert_eq!(
        span(&tracks[1]),
        (frames(85 * 75 + 40), Some(frames(1110 * 75)))
    );
    assert_eq!(span(&tracks[2]), (frames(1110 * 75), None));
    // INDEX 00 is exposed as the pregap.
    assert_eq!(tracks[0].pregap_start, None);
    assert_eq!(tracks[1].pregap_start, Some(frames(84 * 75 + 65)));
    assert_eq!(tracks[2].pregap_start, Some(frames(1108 * 75 + 10)));
    assert!(tracks[1].pre_emphasis && !tracks[0].pre_emphasis);

    let first = &tracks[0].tags;
    assert_eq!(first.title.as_deref(), Some("Wings (Part One)"));
    assert_eq!(first.album.as_deref(), Some("Animals Revisited"));
    assert_eq!(first.album_artist.as_deref(), Some("The Example Band"));
    assert_eq!(first.artist.as_deref(), Some("The Example Band"));
    assert_eq!(first.track_number, Some(1));
    assert_eq!(first.track_total, Some(3));
    assert_eq!(first.date.as_deref(), Some("1977"));
    assert_eq!(first.genres, ["Progressive Rock"]);
    assert_eq!(first.comment.as_deref(), Some("ExactAudioCopy v1.6"));
    assert_eq!(first.replay_gain.album_gain_db, Some(-7.89));
    assert_eq!(first.replay_gain.album_peak, Some(0.988525));
    assert_eq!(first.replay_gain.track_gain_db, Some(-5.12));
    assert_eq!(first.replay_gain.track_peak, Some(0.912));
    assert_eq!(
        first.extra_values("isrc").collect::<Vec<_>>(),
        ["GBAYE7700001"]
    );
    assert_eq!(
        first.extra_values("barcode").collect::<Vec<_>>(),
        ["0077774644020"]
    );
    assert_eq!(
        first.extra_values("discid").collect::<Vec<_>>(),
        ["860B640B"]
    );

    let second = &tracks[1].tags;
    assert_eq!(second.replay_gain.track_gain_db, None);
    assert_eq!(second.artist.as_deref(), Some("The Example Band"));
    assert_eq!(second.extra_values("isrc").count(), 0);

    let third = &tracks[2].tags;
    assert_eq!(third.artist.as_deref(), Some("Guest Singer"));
    assert_eq!(third.album_artist.as_deref(), Some("The Example Band"));
    let credits: Vec<(Role, &str)> = third
        .credits
        .iter()
        .map(|c| (c.role, c.name.as_str()))
        .collect();
    assert_eq!(
        credits,
        [(Role::Writer, "R. Writer"), (Role::Composer, "R. Writer")]
    );
}

#[test]
fn eac_single_file_album_with_pregaps() {
    let sheet = CueSheet::parse(EAC_SINGLE_FILE.as_bytes(), &CueOptions::default()).unwrap();
    assert_eq!(sheet.encoding.name(), "UTF-8");
    assert_eq!(
        sheet.tracks[1]
            .index(0)
            .map(|i| i.time.to_string())
            .as_deref(),
        Some("01:24:65")
    );
    check_eac_album(&sheet, Path::new("/music/album"));
}

#[test]
fn crlf_and_byte_order_mark() {
    let crlf = EAC_SINGLE_FILE.replace('\n', "\r\n");
    let sheet = CueSheet::parse(crlf.as_bytes(), &CueOptions::default()).unwrap();
    check_eac_album(&sheet, Path::new("/music/album"));

    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend_from_slice(crlf.as_bytes());
    let sheet = CueSheet::parse(&bom, &CueOptions::default()).unwrap();
    assert_eq!(sheet.encoding.source, EncodingSource::ByteOrderMark);
    check_eac_album(&sheet, Path::new("/music/album"));

    let mut utf16 = vec![0xFF, 0xFE];
    for unit in crlf.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    let sheet = CueSheet::parse(&utf16, &CueOptions::default()).unwrap();
    assert_eq!(sheet.encoding.name(), "UTF-16LE");
    check_eac_album(&sheet, Path::new("/music/album"));
}

#[test]
fn spans_are_exact_at_any_sample_rate() {
    let sheet = CueSheet::parse(EAC_SINGLE_FILE.as_bytes(), &CueOptions::default()).unwrap();
    let tracks = sheet.resolve(Path::new("/a")).tracks;
    let start = tracks[1].start();
    assert_eq!(start.ticks(), 85 * 75 + 40);
    assert_eq!(start.ticks_per_second().get(), 75);
    for (hz, per_frame) in [(44_100, 588), (48_000, 640), (88_200, 1176), (96_000, 1280)] {
        let rate = SampleRate::new(hz).unwrap();
        assert_eq!(start.frames_at(rate), (85 * 75 + 40) * per_frame, "{hz}");
        assert_eq!(
            start,
            MediaTime::from_frames((85 * 75 + 40) * per_frame, rate)
        );
    }
    // The largest time a sheet can hold converts without overflow.
    let long = CueTime::parse("999999:59:74").unwrap();
    assert_eq!(long.to_media_time().ticks(), 999_999 * 4500 + 59 * 75 + 74);
}

#[test]
fn multi_file_with_gaps_appended_to_the_previous_file() {
    let text = r#"PERFORMER "Artist"
TITLE "Album"
FILE "01 - One.flac" WAVE
  TRACK 01 AUDIO
    TITLE "One"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Two"
    INDEX 00 03:58:12
FILE "02 - Two.flac" WAVE
    INDEX 01 00:00:00
  TRACK 03 AUDIO
    TITLE "Three"
    INDEX 01 04:10:00
FILE "CD2\03 - Four.ape" WAVE
  TRACK 04 AUDIO
    TITLE "Four"
    INDEX 00 00:00:00
    INDEX 01 00:00:33
"#;
    let sheet = CueSheet::parse(text.as_bytes(), &CueOptions::default()).unwrap();
    assert!(sheet.warnings.is_empty(), "{:?}", sheet.warnings);
    let resolution = sheet.resolve(Path::new("/m"));
    assert!(resolution.warnings.is_empty(), "{:?}", resolution.warnings);
    let t = resolution.tracks;
    let paths: Vec<&[u8]> = t
        .iter()
        .map(|t| t.source.path.as_os_str().as_bytes())
        .collect();
    assert_eq!(
        paths,
        [
            b"/m/01 - One.flac".as_slice(),
            b"/m/02 - Two.flac",
            b"/m/02 - Two.flac",
            b"/m/CD2/03 - Four.ape",
        ]
    );
    // Track 1 is the last of its file and plays the gap before track 2.
    assert_eq!(span(&t[0]), (frames(0), None));
    assert_eq!(t[0].pregap_start, None);
    // Track 2's pregap is in another file, so it is not exposed.
    assert_eq!(span(&t[1]), (frames(0), Some(frames(250 * 75))));
    assert_eq!(t[1].pregap_start, None);
    assert_eq!(span(&t[2]), (frames(250 * 75), None));
    assert_eq!(span(&t[3]), (frames(33), None));
    assert_eq!(t[3].pregap_start, Some(frames(0)));
    assert_eq!(t[3].tags.track_total, Some(4));
}

#[test]
fn missing_index_01() {
    let text = "FILE a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 00 01:00:00\nTRACK 03 AUDIO\nTITLE \"No index\"\nTRACK 04 AUDIO\nINDEX 01 02:00:00\nTRACK 05 MODE1/2352\nINDEX 01 03:00:00\n";
    let sheet = CueSheet::parse(text.as_bytes(), &CueOptions::default()).unwrap();
    let resolution = sheet.resolve(Path::new("/m"));
    assert_eq!(
        resolution
            .warnings
            .iter()
            .map(|w| (w.line, w.kind.clone()))
            .collect::<Vec<_>>(),
        [
            (Some(4), CueWarningKind::MissingIndex01(2)),
            (Some(6), CueWarningKind::NoIndex(3)),
        ]
    );
    let numbers: Vec<u32> = resolution.tracks.iter().map(|t| t.number).collect();
    assert_eq!(
        numbers,
        [1, 2, 4],
        "data tracks and tracks without index are left out"
    );
    assert_eq!(
        span(&resolution.tracks[1]),
        (frames(4500), Some(frames(9000)))
    );
    assert_eq!(resolution.tracks[0].tags.track_total, Some(4));
}

#[test]
fn tracks_out_of_order_are_left_out() {
    let text = "FILE a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:10:00\nTRACK 02 AUDIO\nINDEX 01 00:05:00\nTRACK 03 AUDIO\nINDEX 01 00:20:00\nINDEX 00 00:30:00\n";
    let sheet = CueSheet::parse(text.as_bytes(), &CueOptions::default()).unwrap();
    let resolution = sheet.resolve(Path::new("/m"));
    assert_eq!(
        kinds(&resolution.warnings),
        [
            CueWarningKind::OutOfOrder(2),
            CueWarningKind::PregapAfterStart(3)
        ]
    );
    assert_eq!(resolution.tracks.len(), 2);
    assert_eq!(
        span(&resolution.tracks[0]),
        (frames(750), Some(frames(1500)))
    );
}

#[test]
fn hostile_paths_are_rejected() {
    let text = r#"FILE "../../../etc/passwd" WAVE
TRACK 01 AUDIO
INDEX 01 00:00:00
FILE "/etc/shadow" WAVE
TRACK 02 AUDIO
INDEX 01 00:00:00
FILE "http://example.org/a.mp3" MP3
TRACK 03 AUDIO
INDEX 01 00:00:00
FILE "C:\Users\me\Music\a.wav" WAVE
TRACK 04 AUDIO
INDEX 01 00:00:00
FILE "sub\..\..\escape.wav" WAVE
TRACK 05 AUDIO
INDEX 01 00:00:00
FILE "\\server\share\a.wav" WAVE
TRACK 06 AUDIO
INDEX 01 00:00:00
FILE "ok.wav" WAVE
TRACK 07 AUDIO
INDEX 01 00:00:00
"#;
    let sheet = CueSheet::parse(text.as_bytes(), &CueOptions::default()).unwrap();
    let resolution = sheet.resolve(Path::new("/music/album"));
    let reasons: Vec<(Option<usize>, PathRejection)> = resolution
        .warnings
        .iter()
        .map(|w| match &w.kind {
            CueWarningKind::RejectedPath { reason, .. } => (w.line, *reason),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        reasons,
        [
            (Some(1), PathRejection::EscapesFolder),
            (Some(4), PathRejection::Absolute),
            (Some(7), PathRejection::Url),
            (Some(10), PathRejection::Absolute),
            (Some(13), PathRejection::EscapesFolder),
            (Some(16), PathRejection::Absolute),
        ]
    );
    assert_eq!(resolution.tracks.len(), 1);
    assert_eq!(
        resolution.tracks[0].source.path,
        Path::new("/music/album/ok.wav")
    );
    // The same holds when reading from disk.
    let dir = folder("hostile");
    fs::write(dir.join("hostile.cue"), text).unwrap();
    let read = read_cue(dir.join("hostile.cue"), &CueOptions::default()).unwrap();
    assert_eq!(read.tracks.len(), 1);
    assert!(read.tracks.iter().all(|t| t.source.path.starts_with(&dir)));
}

/// `text` in the charset `label`.
fn encoded(text: &str, label: &str) -> Vec<u8> {
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).expect("known label");
    let (bytes, _, errors) = encoding.encode(text);
    assert!(!errors);
    bytes.into_owned()
}

const CYRILLIC: &str = "REM GENRE \"Рок\"\nREM DATE 1988\nPERFORMER \"Кино\"\nTITLE \"Группа крови\"\nFILE \"Кино - Группа крови.flac\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"Группа крови\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    TITLE \"Закрой за мной дверь, я ухожу\"\n    INDEX 01 04:46:00\n  TRACK 03 AUDIO\n    TITLE \"Война\"\n    INDEX 01 09:01:50\n";

#[test]
fn cp1251_sheet_with_a_utf8_file_name_on_disk() {
    let dir = folder("cp1251-utf8");
    fs::write(dir.join("album.cue"), encoded(CYRILLIC, "windows-1251")).unwrap();
    fs::write(dir.join("Кино - Группа крови.flac"), b"").unwrap();
    let read = read_cue(dir.join("album.cue"), &CueOptions::default()).unwrap();
    let sheet = read.sheet.as_ref().unwrap();
    assert_eq!(sheet.encoding.name(), "windows-1251");
    assert_eq!(sheet.encoding.source, EncodingSource::Detected);
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    assert_eq!(read.tracks.len(), 3);
    assert_eq!(
        read.tracks[1].tags.title.as_deref(),
        Some("Закрой за мной дверь, я ухожу")
    );
    assert_eq!(read.tracks[0].tags.artist.as_deref(), Some("Кино"));
    assert_eq!(read.tracks[0].tags.genres, ["Рок"]);
    assert_eq!(
        read.tracks[0].source.path,
        dir.join("Кино - Группа крови.flac")
    );
}

#[test]
fn cp1251_sheet_with_a_cp1251_file_name_on_disk() {
    let dir = folder("cp1251-raw");
    fs::write(dir.join("album.cue"), encoded(CYRILLIC, "windows-1251")).unwrap();
    let raw_name = encoded("Кино - Группа крови.flac", "windows-1251");
    let raw_path = dir.join(std::ffi::OsStr::from_bytes(&raw_name));
    fs::write(&raw_path, b"").unwrap();
    let read = read_cue(dir.join("album.cue"), &CueOptions::default()).unwrap();
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    assert_eq!(read.tracks[0].source.path, raw_path);
    assert!(
        read.tracks[0].source.path.to_str().is_none(),
        "kept as bytes"
    );
}

#[test]
fn a_chosen_charset_overrides_the_guess() {
    let bytes = encoded(CYRILLIC, "koi8-r");
    let options = CueOptions {
        legacy_charset: Some("koi8-r".into()),
    };
    let sheet = CueSheet::parse(&bytes, &options).unwrap();
    assert_eq!(sheet.encoding.source, EncodingSource::Chosen);
    assert_eq!(sheet.title.as_deref(), Some("Группа крови"));
    let unknown = CueOptions {
        legacy_charset: Some("klingon".into()),
    };
    assert!(CueSheet::parse(&bytes, &unknown).is_err());
}

#[test]
fn shift_jis_sheet() {
    let text = "PERFORMER \"荒井由実\"\r\nTITLE \"ひこうき雲\"\r\nFILE \"荒井由実 - ひこうき雲.wav\" WAVE\r\n  TRACK 01 AUDIO\r\n    TITLE \"ひこうき雲\"\r\n    INDEX 01 00:00:00\r\n  TRACK 02 AUDIO\r\n    TITLE \"恋のスーパー・パラシューター\"\r\n    INDEX 01 03:58:00\r\n  TRACK 03 AUDIO\r\n    TITLE \"きっと言える\"\r\n    INDEX 01 07:05:12\r\n";
    let dir = folder("shift-jis");
    fs::write(dir.join("a.cue"), encoded(text, "shift_jis")).unwrap();
    fs::write(dir.join("荒井由実 - ひこうき雲.wav"), b"").unwrap();
    let read = read_cue(dir.join("a.cue"), &CueOptions::default()).unwrap();
    assert_eq!(read.sheet.as_ref().unwrap().encoding.name(), "Shift_JIS");
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    let titles: Vec<&str> = read
        .tracks
        .iter()
        .filter_map(|t| t.tags.title.as_deref())
        .collect();
    assert_eq!(
        titles,
        ["ひこうき雲", "恋のスーパー・パラシューター", "きっと言える"]
    );
    assert_eq!(
        read.tracks[0].tags.album_artist.as_deref(),
        Some("荒井由実")
    );
}

#[test]
fn missing_files_are_substituted_or_reported() {
    let dir = folder("substitute");
    let text = "FILE \"Album.wav\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE \"other.WAV\" WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00\nFILE \"gone.wav\" WAVE\nTRACK 03 AUDIO\nINDEX 01 00:00:00\n";
    fs::write(dir.join("a.cue"), text).unwrap();
    fs::write(dir.join("Album.flac"), b"").unwrap();
    fs::write(dir.join("Album.txt"), b"").unwrap();
    fs::write(dir.join("other.wav"), b"").unwrap();
    let read = read_cue(dir.join("a.cue"), &CueOptions::default()).unwrap();
    assert_eq!(read.tracks.len(), 3);
    assert_eq!(read.tracks[0].source.path, dir.join("Album.flac"));
    assert_eq!(read.tracks[1].source.path, dir.join("other.wav"));
    assert_eq!(read.tracks[2].source.path, dir.join("gone.wav"));
    assert_eq!(
        kinds(&read.warnings),
        [
            CueWarningKind::FileSubstituted {
                name: "Album.wav".into(),
                found: "Album.flac".into()
            },
            CueWarningKind::FileSubstituted {
                name: "other.WAV".into(),
                found: "other.wav".into()
            },
            CueWarningKind::FileNotFound("gone.wav".into()),
        ]
    );
}

#[test]
fn malformed_and_hostile_input_never_panics() {
    let mut inputs: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"\0\0\0\0".to_vec(),
        b"\xff\xfe".to_vec(),
        b"\xef\xbb\xbf".to_vec(),
        b"TRACK".to_vec(),
        b"FILE".to_vec(),
        b"FILE \"".to_vec(),
        b"INDEX 01 99:99:99".to_vec(),
        b"TRACK 01 AUDIO\nINDEX 01 00:00:00".to_vec(),
        b"FILE a WAVE\nTRACK 99999999999999999999 AUDIO".to_vec(),
        b"FILE a WAVE\nTRACK 01 AUDIO\nINDEX 01 18446744073709551615:00:00".to_vec(),
        b"REM \"\nREM GENRE \"\nTITLE \"\"\"\n".to_vec(),
        "FILE \"\u{feff}é\" WAVE\nTRACK ١ AUDIO\n"
            .as_bytes()
            .to_vec(),
        vec![b'\r'; 10_000],
        (0..=255u8).cycle().take(70_000).collect(),
    ];
    let mut long = b"FILE a.wav WAVE\nTRACK 01 AUDIO\nTITLE \"".to_vec();
    long.extend(std::iter::repeat_n(b'x', 100_000));
    inputs.push(long);
    for input in &inputs {
        let sheet = CueSheet::parse(input, &CueOptions::default()).unwrap();
        let resolution = sheet.resolve(Path::new("/m"));
        assert!(
            resolution
                .tracks
                .iter()
                .all(|t| t.source.path.starts_with("/m"))
        );
    }
    let too_large = vec![b' '; MAX_SHEET_BYTES + 1];
    assert!(CueSheet::parse(&too_large, &CueOptions::default()).is_err());
    let dir = folder("too-large");
    fs::write(dir.join("big.cue"), &too_large).unwrap();
    assert!(read_cue(dir.join("big.cue"), &CueOptions::default()).is_err());
}

#[test]
fn warnings_are_bounded() {
    let text = "FROB\n".repeat(10_000);
    let sheet = CueSheet::parse(text.as_bytes(), &CueOptions::default()).unwrap();
    assert!(sheet.warnings.len() <= 257);
    assert_eq!(
        sheet.warnings.last().map(|w| &w.kind),
        Some(&CueWarningKind::TooManyWarnings)
    );
    assert_eq!(
        sheet.warnings[0].to_string(),
        "line 1: unknown command \"FROB\""
    );
}
