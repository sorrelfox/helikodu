// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The formats lofty reads: field mapping, multiple values, ratings, loudness tags
//! and audio properties.

use std::borrow::Cow;
use std::fs::File;
use std::path::Path;
use std::time::Duration;

use helikodu_meta::tags::{FileTags, ReadOptions, Role, TagSource, TrackTags, read_tags};
use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::flac::FlacFile;
use lofty::id3::v2::{
    ExtendedTextFrame, Frame, FrameId, Id3v2Tag, KeyValueFrame, PopularimeterFrame,
    UniqueFileIdentifierFrame,
};
use lofty::mp4::{Atom, AtomData, AtomIdent, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::ogg::tag::VorbisComments;
use lofty::tag::TagExt;

use crate::media::{encode, metadata};

const ALBUM_ID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
const RECORDING_ID: &str = "11111111-2222-3333-4444-555555555555";
const ARTIST_A_ID: &str = "aaaaaaaa-0000-0000-0000-000000000001";
const ARTIST_B_ID: &str = "bbbbbbbb-0000-0000-0000-000000000002";

fn read(path: &Path) -> FileTags {
    read_tags(path, &ReadOptions::default()).expect("tags can be read")
}

fn credits(tags: &TrackTags, role: Role) -> Vec<(String, Option<String>)> {
    tags.credits_with(role)
        .map(|c| (c.name.clone(), c.instrument.clone()))
        .collect()
}

fn assert_about_one_second(file: &FileTags) {
    let d = file.audio.duration.expect("a duration");
    assert!(
        d > Duration::from_millis(950) && d < Duration::from_millis(1100),
        "duration {d:?}"
    );
}

fn edit_vorbis_comments(path: &Path, edit: impl FnOnce(&mut VorbisComments)) {
    let flac = FlacFile::read_from(&mut File::open(path).expect("open"), ParseOptions::new())
        .expect("lofty reads the FLAC file");
    let mut comments = flac.vorbis_comments().cloned().unwrap_or_default();
    edit(&mut comments);
    comments
        .save_to_path(path, WriteOptions::default())
        .expect("lofty writes the comments");
}

fn edit_id3v2(path: &Path, v23: bool, edit: impl FnOnce(&mut Id3v2Tag)) {
    let mp3 = MpegFile::read_from(&mut File::open(path).expect("open"), ParseOptions::new())
        .expect("lofty reads the MP3 file");
    let mut tag = mp3.id3v2().cloned().unwrap_or_default();
    edit(&mut tag);
    let mut options = WriteOptions::default();
    options.use_id3v23(v23);
    tag.save_to_path(path, options)
        .expect("lofty writes the ID3v2 tag");
}

fn frame_id(id: &str) -> FrameId<'static> {
    FrameId::new(Cow::Owned(id.to_owned())).expect("a valid frame ID")
}

#[test]
fn flac_with_picard_vorbis_comments() {
    let args = metadata(&[
        ("title", "Teen Town"),
        ("artist", "Weather Report"),
        ("album", "Heavy Weather"),
        ("album_artist", "Weather Report"),
        ("track", "3/8"),
        ("disc", "1/1"),
        ("date", "1977-03-01"),
        ("ORIGINALDATE", "1977"),
        ("MUSICBRAINZ_ALBUMID", ALBUM_ID),
        ("MUSICBRAINZ_TRACKID", RECORDING_ID),
        ("REPLAYGAIN_TRACK_GAIN", "-6.50 dB"),
        ("REPLAYGAIN_TRACK_PEAK", "0.988525"),
        ("REPLAYGAIN_ALBUM_GAIN", "-7.10 dB"),
        ("REPLAYGAIN_ALBUM_PEAK", "1.000000"),
        ("FMPS_RATING", "0.7"),
        ("COMPILATION", "0"),
        ("BPM", "136"),
        ("ISRC", "USCO10000001"),
    ]);
    let Some(path) = encode("picard.flac", "flac", &args, "flac") else {
        return;
    };
    edit_vorbis_comments(&path, |c| {
        for (k, v) in [
            ("ARTISTS", "Jaco Pastorius"),
            ("ARTISTS", "Joe Zawinul"),
            ("MUSICBRAINZ_ARTISTID", ARTIST_A_ID),
            ("MUSICBRAINZ_ARTISTID", ARTIST_B_ID),
            ("GENRE", "Jazz"),
            ("GENRE", "Fusion"),
            ("COMPOSER", "Jaco Pastorius"),
            ("PERFORMER", "Jaco Pastorius (bass)"),
            ("PERFORMER", "Alex Acuña (drums)"),
            ("LABEL", "Columbia"),
            ("CATALOGNUMBER", "PC 34418"),
            ("RELEASETYPE", "album"),
            ("WORK", "Teen Town"),
            ("MOVEMENTNAME", "Part one"),
            ("MOVEMENT", "1"),
            ("MOVEMENTTOTAL", "2"),
            ("LYRICS", "(instrumental)"),
            ("MY_OWN_FIELD", "kept"),
        ] {
            c.push(k.to_owned(), v.to_owned());
        }
    });

    let file = read(&path);
    assert_eq!(file.source, TagSource::Lofty);
    assert!(file.fallback_reason.is_none());
    assert!(!file.charset_repaired);
    let t = &file.tags;
    assert_eq!(t.title.as_deref(), Some("Teen Town"));
    assert_eq!(t.artist.as_deref(), Some("Weather Report"));
    assert_eq!(t.artists, ["Jaco Pastorius", "Joe Zawinul"]);
    assert_eq!(t.album.as_deref(), Some("Heavy Weather"));
    assert_eq!(t.album_artists, ["Weather Report"]);
    assert_eq!((t.track_number, t.track_total), (Some(3), Some(8)));
    assert_eq!((t.disc_number, t.disc_total), (Some(1), Some(1)));
    assert_eq!(t.date.as_deref(), Some("1977-03-01"));
    assert_eq!(t.year(), Some(1977));
    assert_eq!(t.original_year(), Some(1977));
    assert_eq!(t.genres, ["Jazz", "Fusion"]);
    assert_eq!(
        credits(t, Role::Composer),
        [("Jaco Pastorius".into(), None)]
    );
    assert_eq!(
        credits(t, Role::Performer),
        [
            ("Jaco Pastorius".into(), Some("bass".into())),
            ("Alex Acuña".into(), Some("drums".into()))
        ]
    );
    assert_eq!(t.labels, ["Columbia"]);
    assert_eq!(t.catalog_numbers, ["PC 34418"]);
    assert_eq!(t.release_types, ["album"]);
    assert_eq!(t.musicbrainz.release.as_deref(), Some(ALBUM_ID));
    assert_eq!(t.musicbrainz.recording.as_deref(), Some(RECORDING_ID));
    assert_eq!(t.musicbrainz.artists, [ARTIST_A_ID, ARTIST_B_ID]);
    assert_eq!(t.replay_gain.track_gain_db, Some(-6.5));
    assert_eq!(t.replay_gain.track_peak, Some(0.988525));
    assert_eq!(t.replay_gain.album_gain_db, Some(-7.1));
    assert_eq!(t.replay_gain.album_peak, Some(1.0));
    assert_eq!(t.rating.map(|r| r.half_stars()), Some(7));
    assert_eq!(t.compilation, Some(false));
    assert_eq!(t.work.as_deref(), Some("Teen Town"));
    assert_eq!(t.movement.as_deref(), Some("Part one"));
    assert_eq!((t.movement_number, t.movement_total), (Some(1), Some(2)));
    assert!(t.has_lyrics);
    assert_eq!(t.extra_values("bpm").collect::<Vec<_>>(), ["136"]);
    assert_eq!(t.extra_values("isrc").collect::<Vec<_>>(), ["USCO10000001"]);
    assert_eq!(t.extra_values("my_own_field").collect::<Vec<_>>(), ["kept"]);

    let a = &file.audio;
    assert_eq!(a.sample_rate, Some(44_100));
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.bit_depth, Some(16));
    assert_eq!(a.codec.as_deref(), Some("flac"));
    assert_eq!(a.container.as_deref(), Some("flac"));
    assert!(a.bitrate_kbps.is_some());
    assert_about_one_second(&file);
}

#[test]
fn mp3_with_id3v24_frames() {
    let args = metadata(&[
        ("title", "Kid A"),
        ("artist", "Radiohead"),
        ("album", "Kid A"),
        ("album_artist", "Radiohead"),
        ("track", "4/10"),
        ("disc", "1/1"),
        ("date", "2000-10-02"),
        ("genre", "Electronic"),
        ("composer", "Thom Yorke"),
        ("MusicBrainz Album Id", ALBUM_ID),
        ("REPLAYGAIN_TRACK_GAIN", "-4.20 dB"),
    ]);
    let mut all: Vec<String> = vec!["-id3v2_version".into(), "4".into()];
    all.extend(args);
    let Some(path) = encode("id3v24.mp3", "libmp3lame", &all, "mp3") else {
        return;
    };
    edit_id3v2(&path, false, |tag| {
        tag.insert(Frame::Popularimeter(PopularimeterFrame::new(
            "Windows Media Player 9 Series",
            196,
            3,
        )));
        tag.insert(Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new(
            "http://musicbrainz.org",
            RECORDING_ID.as_bytes().to_vec(),
        )));
        tag.insert(Frame::UserText(ExtendedTextFrame::new(
            TextEncoding::UTF8,
            "ARTISTS",
            "Thom Yorke\0Jonny Greenwood",
        )));
        tag.insert(Frame::KeyValue(KeyValueFrame::new(
            frame_id("TMCL"),
            TextEncoding::UTF8,
            vec![(Cow::Borrowed("guitar"), Cow::Borrowed("Jonny Greenwood"))],
        )));
        tag.insert(Frame::KeyValue(KeyValueFrame::new(
            frame_id("TIPL"),
            TextEncoding::UTF8,
            vec![
                (Cow::Borrowed("producer"), Cow::Borrowed("Nigel Godrich")),
                (Cow::Borrowed("engineer"), Cow::Borrowed("Gerard Navarro")),
            ],
        )));
        tag.insert(Frame::Text(lofty::id3::v2::TextInformationFrame::new(
            frame_id("TCON"),
            TextEncoding::UTF8,
            "Electronic\0Art Rock",
        )));
    });

    let file = read(&path);
    assert_eq!(file.source, TagSource::Lofty);
    let t = &file.tags;
    assert_eq!(t.title.as_deref(), Some("Kid A"));
    assert_eq!(t.artist.as_deref(), Some("Radiohead"));
    assert_eq!(t.artists, ["Thom Yorke", "Jonny Greenwood"]);
    assert_eq!(t.album_artist.as_deref(), Some("Radiohead"));
    assert_eq!((t.track_number, t.track_total), (Some(4), Some(10)));
    assert_eq!(t.date.as_deref(), Some("2000-10-02"));
    assert_eq!(t.genres, ["Electronic", "Art Rock"]);
    assert_eq!(credits(t, Role::Composer), [("Thom Yorke".into(), None)]);
    assert_eq!(
        credits(t, Role::Performer),
        [("Jonny Greenwood".into(), Some("guitar".into()))]
    );
    assert_eq!(credits(t, Role::Producer), [("Nigel Godrich".into(), None)]);
    assert_eq!(
        credits(t, Role::Engineer),
        [("Gerard Navarro".into(), None)]
    );
    assert_eq!(t.musicbrainz.release.as_deref(), Some(ALBUM_ID));
    assert_eq!(t.musicbrainz.recording.as_deref(), Some(RECORDING_ID));
    assert_eq!(t.replay_gain.track_gain_db, Some(-4.2));
    assert_eq!(t.rating.map(|r| r.half_stars()), Some(8));

    let a = &file.audio;
    assert_eq!(a.codec.as_deref(), Some("mp3"));
    assert_eq!(a.sample_rate, Some(44_100));
    assert_eq!(a.channels, Some(2));
    assert!(a.bitrate_kbps.is_some_and(|b| b > 8));
}

#[test]
fn popm_ratings_by_player() {
    let cases = [
        ("musicbee", "MusicBee", 186, Some(7)),
        ("quodlibet", "quodlibet@lists.sacredchao.net", 153, Some(6)),
        ("foobar", "no@email", 64, Some(4)),
        ("unrated", "Windows Media Player 9 Series", 0, None),
    ];
    for (name, email, value, expected) in cases {
        let Some(path) = encode(
            &format!("popm-{name}.mp3"),
            "libmp3lame",
            &[] as &[&str],
            "mp3",
        ) else {
            return;
        };
        edit_id3v2(&path, true, |tag| {
            tag.insert(Frame::Popularimeter(PopularimeterFrame::new(
                email, value, 0,
            )));
        });
        let file = read(&path);
        assert_eq!(file.tags.rating.map(|r| r.half_stars()), expected, "{name}");
    }
}

#[test]
fn id3v23_grouping_and_work() {
    let Some(path) = encode("grouping.mp3", "libmp3lame", &[] as &[&str], "mp3") else {
        return;
    };
    edit_id3v2(&path, true, |tag| {
        for (id, value) in [("TIT1", "Symphonies"), ("TIT2", "Allegro")] {
            tag.insert(Frame::Text(lofty::id3::v2::TextInformationFrame::new(
                frame_id(id),
                TextEncoding::UTF16,
                value,
            )));
        }
    });
    let t = read(&path).tags;
    assert_eq!(t.grouping.as_deref(), Some("Symphonies"));
    assert_eq!(t.work, None);

    edit_id3v2(&path, false, |tag| {
        tag.insert(Frame::Text(lofty::id3::v2::TextInformationFrame::new(
            frame_id("GRP1"),
            TextEncoding::UTF8,
            "Favourites",
        )));
    });
    let t = read(&path).tags;
    assert_eq!(t.grouping.as_deref(), Some("Favourites"));
    assert_eq!(t.work.as_deref(), Some("Symphonies"));
}

#[test]
fn m4a_with_ilst_and_freeform_atoms() {
    let args = metadata(&[
        ("title", "So What"),
        ("artist", "Miles Davis"),
        ("album", "Kind of Blue"),
        ("album_artist", "Miles Davis"),
        ("track", "1/5"),
        ("disc", "1/1"),
        ("date", "1959"),
        ("genre", "Jazz"),
        ("composer", "Miles Davis"),
        ("comment", "Take 3"),
        ("grouping", "Modal"),
    ]);
    let Some(path) = encode("ilst.m4a", "aac", &args, "ipod") else {
        return;
    };
    let mp4 = Mp4File::read_from(&mut File::open(&path).expect("open"), ParseOptions::new())
        .expect("lofty reads the M4A file");
    let mut ilst = mp4.ilst().cloned().unwrap_or_default();
    let freeform = |name: &str| AtomIdent::Freeform {
        mean: Cow::Borrowed("com.apple.iTunes"),
        name: Cow::Owned(name.to_owned()),
    };
    ilst.insert(Atom::new(
        freeform("MusicBrainz Track Id"),
        AtomData::UTF8(RECORDING_ID.to_owned()),
    ));
    ilst.insert(Atom::new(
        freeform("MusicBrainz Album Id"),
        AtomData::UTF8(ALBUM_ID.to_owned()),
    ));
    ilst.insert(Atom::new(
        freeform("replaygain_album_gain"),
        AtomData::UTF8("-8.00 dB".to_owned()),
    ));
    ilst.insert(
        Atom::from_collection(
            freeform("ARTISTS"),
            vec![
                AtomData::UTF8("Miles Davis".to_owned()),
                AtomData::UTF8("John Coltrane".to_owned()),
            ],
        )
        .expect("two values"),
    );
    ilst.insert(Atom::new(AtomIdent::Fourcc(*b"cpil"), AtomData::Bool(true)));
    ilst.save_to_path(&path, WriteOptions::default())
        .expect("lofty writes the ilst");

    let file = read(&path);
    let t = &file.tags;
    assert_eq!(t.title.as_deref(), Some("So What"));
    assert_eq!(t.artist.as_deref(), Some("Miles Davis"));
    assert_eq!(t.artists, ["Miles Davis", "John Coltrane"]);
    assert_eq!(t.album_artist.as_deref(), Some("Miles Davis"));
    assert_eq!((t.track_number, t.track_total), (Some(1), Some(5)));
    assert_eq!((t.disc_number, t.disc_total), (Some(1), Some(1)));
    assert_eq!(t.year(), Some(1959));
    assert_eq!(t.genres, ["Jazz"]);
    assert_eq!(t.comment.as_deref(), Some("Take 3"));
    assert_eq!(t.grouping.as_deref(), Some("Modal"));
    assert_eq!(t.compilation, Some(true));
    assert_eq!(t.musicbrainz.recording.as_deref(), Some(RECORDING_ID));
    assert_eq!(t.musicbrainz.release.as_deref(), Some(ALBUM_ID));
    assert_eq!(t.replay_gain.album_gain_db, Some(-8.0));
    assert_eq!(file.audio.codec.as_deref(), Some("aac"));
    assert_eq!(file.audio.container.as_deref(), Some("mp4"));
    assert_about_one_second(&file);
}

#[test]
fn opus_r128_gains() {
    let args = metadata(&[
        ("title", "Quiet"),
        ("R128_TRACK_GAIN", "-512"),
        ("R128_ALBUM_GAIN", "256"),
    ]);
    let Some(path) = encode("r128.opus", "libopus", &args, "opus") else {
        return;
    };
    let file = read(&path);
    let rg = file.tags.replay_gain;
    assert_eq!(rg.r128_track_gain, Some(-512));
    assert_eq!(rg.r128_album_gain, Some(256));
    assert_eq!(rg.track_gain_rg2_db(), Some(3.0));
    assert_eq!(rg.album_gain_rg2_db(), Some(6.0));
    assert_eq!(file.audio.codec.as_deref(), Some("opus"));
    assert_eq!(file.audio.container.as_deref(), Some("ogg"));
}

#[test]
fn wav_with_riff_info() {
    let args = metadata(&[
        ("title", "Test tone"),
        ("artist", "Signal"),
        ("genre", "Test"),
    ]);
    let Some(path) = encode("info.wav", "pcm_s16le", &args, "wav") else {
        return;
    };
    let file = read(&path);
    assert_eq!(file.tags.title.as_deref(), Some("Test tone"));
    assert_eq!(file.tags.artist.as_deref(), Some("Signal"));
    assert_eq!(file.tags.genres, ["Test"]);
    assert_eq!(file.audio.bit_depth, Some(16));
    assert_eq!(file.audio.codec.as_deref(), Some("pcm"));
    assert_about_one_second(&file);
}
