// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Every writable format: fields with several values, custom fields, numbers,
//! dates, credits, lyrics, covers and ratings written and read back; fields
//! cleared; what the editor did not touch kept; read-only formats refused.

use std::path::Path;

use helikodu_meta::covers::{CoverHash, CoverLimits, read_embedded_covers};
use helikodu_meta::tags::rating::Rating;
use helikodu_meta::tags::writing::{
    CoverChange, FieldKey, TagChanges, TagField, WriteError, WriteMode, WriteOptions, field_values,
    read_lyrics, write_tags,
};
use helikodu_meta::tags::{ReadOptions, TagSource, TrackTags, read_tags};

use crate::media::{Format, WRITABLE, encode, entries, folder, jpeg, metadata, png};

fn read(path: &Path) -> TrackTags {
    let file = read_tags(path, &ReadOptions::default()).expect("the file reads");
    assert_eq!(file.source, TagSource::Lofty, "{}", path.display());
    file.tags
}

fn values(tags: &TrackTags, key: impl Into<FieldKey>) -> Vec<String> {
    field_values(tags, &key.into())
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

fn custom() -> FieldKey {
    FieldKey::Custom("HELIKODU_TEST".into())
}

fn full_changes(cover: &[u8]) -> TagChanges {
    let mut c = TagChanges::new();
    c.set_one(TagField::Title, "Écoute — 聴く")
        .set(TagField::Artist, ["Alpha", "Beta"])
        .set(TagField::Artists, ["Alpha", "Beta"])
        .set_one(TagField::Album, "The Album")
        .set_one(TagField::AlbumArtist, "Various")
        .set_one(TagField::TrackNumber, "3")
        .set_one(TagField::TrackTotal, "12")
        .set_one(TagField::DiscNumber, "1")
        .set_one(TagField::DiscTotal, "2")
        .set_one(TagField::Date, "2001-05-03")
        .set_one(TagField::OriginalDate, "1999")
        .set(TagField::Genre, ["Rock", "Pop"])
        .set(TagField::Composer, ["Comp One", "Comp Two"])
        .set(TagField::Performer, ["Pat (guitar)", "Quinn"])
        .set_one(TagField::Producer, "Prod")
        .set_one(TagField::Conductor, "Cond")
        .set(TagField::Label, ["Label A", "Label B"])
        .set_one(TagField::CatalogNumber, "CAT-1")
        .set_one(TagField::Bpm, "120")
        .set_one(TagField::Key, "Am")
        .set_one(TagField::Mood, "Calm")
        .set_one(TagField::Isrc, "USRC17607839")
        .set_one(TagField::Comment, "A comment")
        .set_one(TagField::Grouping, "Group")
        .set_one(TagField::Compilation, "1")
        .set_one(
            TagField::MusicBrainzAlbumId,
            "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
        )
        .set_one(
            TagField::MusicBrainzRecordingId,
            "11111111-2222-3333-4444-555555555555",
        )
        .set(
            TagField::MusicBrainzArtistId,
            [
                "aaaaaaaa-0000-0000-0000-000000000001",
                "bbbbbbbb-0000-0000-0000-000000000002",
            ],
        )
        .set_one(TagField::ReplayGainTrackGain, "-6.50 dB")
        .set(custom(), ["first", "second"]);
    c.lyrics = Some(Some("Line one\nLine two".into()));
    c.rating = Some(Rating::from_half_stars(8));
    c.play_count = Some(Some(42));
    c.cover = Some(CoverChange::SetFront(cover.to_vec()));
    c
}

fn check_full(tags: &TrackTags, name: &str) {
    let expect = |key: FieldKey, want: &[&str]| {
        assert_eq!(
            values(tags, key.clone()),
            strings(want),
            "{name}: {}",
            key.name()
        );
    };
    expect(TagField::Title.into(), &["Écoute — 聴く"]);
    expect(TagField::Artist.into(), &["Alpha; Beta"]);
    expect(TagField::Artists.into(), &["Alpha", "Beta"]);
    expect(TagField::Album.into(), &["The Album"]);
    expect(TagField::AlbumArtist.into(), &["Various"]);
    expect(TagField::TrackNumber.into(), &["3"]);
    expect(TagField::TrackTotal.into(), &["12"]);
    expect(TagField::DiscNumber.into(), &["1"]);
    expect(TagField::DiscTotal.into(), &["2"]);
    expect(TagField::Date.into(), &["2001-05-03"]);
    expect(TagField::OriginalDate.into(), &["1999"]);
    expect(TagField::Genre.into(), &["Rock", "Pop"]);
    expect(TagField::Composer.into(), &["Comp One", "Comp Two"]);
    expect(TagField::Performer.into(), &["Pat (guitar)", "Quinn"]);
    expect(TagField::Producer.into(), &["Prod"]);
    expect(TagField::Conductor.into(), &["Cond"]);
    expect(TagField::Label.into(), &["Label A", "Label B"]);
    expect(TagField::CatalogNumber.into(), &["CAT-1"]);
    expect(TagField::Bpm.into(), &["120"]);
    expect(TagField::Key.into(), &["Am"]);
    expect(TagField::Mood.into(), &["Calm"]);
    expect(TagField::Isrc.into(), &["USRC17607839"]);
    expect(TagField::Comment.into(), &["A comment"]);
    expect(TagField::Grouping.into(), &["Group"]);
    expect(TagField::Compilation.into(), &["1"]);
    expect(
        TagField::MusicBrainzAlbumId.into(),
        &["0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"],
    );
    expect(
        TagField::MusicBrainzRecordingId.into(),
        &["11111111-2222-3333-4444-555555555555"],
    );
    expect(
        TagField::MusicBrainzArtistId.into(),
        &[
            "aaaaaaaa-0000-0000-0000-000000000001",
            "bbbbbbbb-0000-0000-0000-000000000002",
        ],
    );
    expect(TagField::ReplayGainTrackGain.into(), &["-6.50 dB"]);
    expect(custom(), &["first", "second"]);
    assert!(tags.has_lyrics, "{name}: lyrics");
    assert_eq!(tags.rating, Rating::from_half_stars(8), "{name}: rating");
    expect(FieldKey::Custom("FMPS_PLAYCOUNT".into()), &["42"]);
}

fn round_trip(format: Format) {
    let dir = folder(&format!("round-trip-{}", format.extension));
    let name = format!("a.{}", format.extension);
    let args = metadata(&[("title", "Old"), ("copyright", "Keep me")]);
    let Some(path) = encode(&dir, &name, format, &args) else {
        return;
    };
    let before = read_tags(&path, &ReadOptions::default()).expect("reads");
    // Not every muxer writes tags (ADTS has none); what there is must stay.
    let copyright = values(&before.tags, TagField::Copyright);
    let before = before.audio;
    let cover = png([200, 30, 30]);
    let outcome = write_tags(&path, &full_changes(&cover), None, &WriteOptions::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(outcome.mode, WriteMode::Replaced, "{name}");
    assert!(outcome.skipped.is_empty(), "{name}: {:?}", outcome.skipped);
    assert_eq!(
        entries(&dir),
        std::slice::from_ref(&name),
        "{name}: no temporary copy left"
    );

    let after = read_tags(&path, &ReadOptions::default()).expect("reads");
    assert_eq!(after.audio.sample_rate, before.sample_rate, "{name}");
    assert_eq!(after.audio.channels, before.channels, "{name}");
    let tags = after.tags;
    check_full(&tags, &name);
    assert_eq!(
        values(&tags, TagField::Copyright),
        copyright,
        "{name}: untouched field"
    );
    if format.extension == "flac" {
        assert_eq!(copyright, ["Keep me"]);
    }
    assert_eq!(
        read_lyrics(&path).expect("lyrics read").as_deref(),
        Some("Line one\nLine two"),
        "{name}"
    );
    let covers = read_embedded_covers(&path, &CoverLimits::default()).expect("pictures read");
    let front = covers.front().expect("a cover").hash().expect("hashed");
    assert_eq!(front, CoverHash::of(&cover), "{name}");

    // Replace the cover, clear fields, drop the lyrics, rating and a custom field.
    let other = jpeg([20, 20, 220]);
    let mut c = TagChanges::new();
    c.clear(TagField::Genre)
        .clear(TagField::Composer)
        .clear(TagField::Performer)
        .clear(TagField::TrackTotal)
        .clear(TagField::Date)
        .clear(TagField::Comment)
        .clear(custom())
        .set_one(TagField::Title, "Second");
    c.lyrics = Some(None);
    c.rating = Some(None);
    c.cover = Some(CoverChange::SetFront(other.clone()));
    write_tags(&path, &c, None, &WriteOptions::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let tags = read(&path);
    assert_eq!(values(&tags, TagField::Title), ["Second"], "{name}");
    for cleared in [
        FieldKey::from(TagField::Genre),
        TagField::Composer.into(),
        TagField::Performer.into(),
        TagField::TrackTotal.into(),
        TagField::Date.into(),
        TagField::Comment.into(),
        custom(),
    ] {
        assert!(
            values(&tags, cleared.clone()).is_empty(),
            "{name}: {} cleared",
            cleared.name()
        );
    }
    assert_eq!(
        values(&tags, TagField::TrackNumber),
        ["3"],
        "{name}: the number stays"
    );
    assert_eq!(
        values(&tags, TagField::Album),
        ["The Album"],
        "{name}: untouched"
    );
    assert!(!tags.has_lyrics, "{name}: lyrics removed");
    assert_eq!(tags.rating, None, "{name}: rating removed");
    let covers = read_embedded_covers(&path, &CoverLimits::default()).expect("pictures read");
    assert_eq!(covers.covers.len(), 1, "{name}: one picture");
    assert_eq!(
        covers.front().and_then(|c| c.hash().ok()),
        Some(CoverHash::of(&other))
    );

    let mut c = TagChanges::new();
    c.cover = Some(CoverChange::RemoveFront);
    write_tags(&path, &c, None, &WriteOptions::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let covers = read_embedded_covers(&path, &CoverLimits::default()).expect("pictures read");
    assert!(covers.covers.is_empty(), "{name}: cover removed");
}

#[test]
fn mp3_round_trip() {
    round_trip(WRITABLE[0]);
}

#[test]
fn adts_aac_round_trip() {
    round_trip(WRITABLE[1]);
}

#[test]
fn flac_round_trip() {
    round_trip(WRITABLE[2]);
}

#[test]
fn ogg_vorbis_round_trip() {
    round_trip(WRITABLE[3]);
}

#[test]
fn opus_round_trip() {
    round_trip(WRITABLE[4]);
}

#[test]
fn mp4_round_trip() {
    round_trip(WRITABLE[5]);
}

#[test]
fn wavpack_round_trip() {
    round_trip(WRITABLE[6]);
}

#[test]
fn wav_round_trip() {
    round_trip(WRITABLE[7]);
}

#[test]
fn aiff_round_trip() {
    round_trip(WRITABLE[8]);
}

#[test]
fn matroska_is_a_read_only_format() {
    let dir = folder("read-only");
    let format = Format {
        extension: "mka",
        encoder: "flac",
        muxer: "matroska",
    };
    let Some(path) = encode(&dir, "a.mka", format, &metadata(&[("title", "Old")])) else {
        return;
    };
    let before = std::fs::read(&path).expect("read");
    let mut c = TagChanges::new();
    c.set_one(TagField::Title, "New");
    let result = write_tags(&path, &c, None, &WriteOptions::default());
    assert!(
        matches!(result, Err(WriteError::ReadOnlyFormat)),
        "{result:?}"
    );
    assert_eq!(std::fs::read(&path).expect("read"), before);
    assert_eq!(entries(&dir), ["a.mka"]);
}

#[test]
fn opus_takes_r128_gains_and_other_formats_skip_them() {
    let dir = folder("r128");
    let Some(opus) = encode(&dir, "a.opus", WRITABLE[4], &[] as &[&str]) else {
        return;
    };
    let mut c = TagChanges::new();
    c.set_one(TagField::R128TrackGain, "-1280")
        .set_one(TagField::R128AlbumGain, "+256");
    let outcome = write_tags(&opus, &c, None, &WriteOptions::default()).expect("Opus writes");
    assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
    let tags = read(&opus);
    assert_eq!(values(&tags, TagField::R128TrackGain), strings(&["-1280"]));
    assert_eq!(values(&tags, TagField::R128AlbumGain), strings(&["256"]));
    assert_eq!(tags.replay_gain.r128_track_gain, Some(-1280));
    // −5 dB at −23 LUFS is 0 dB at the ReplayGain reference of −18 LUFS.
    let track = tags.replay_gain.track_gain_rg2_db().expect("a track gain");
    assert!(track.abs() < 1e-9, "{track}");

    c = TagChanges::new();
    c.clear(TagField::R128TrackGain);
    write_tags(&opus, &c, None, &WriteOptions::default()).expect("Opus writes");
    assert!(values(&read(&opus), TagField::R128TrackGain).is_empty());

    c = TagChanges::new();
    c.set_one(TagField::R128TrackGain, "1.5");
    assert!(matches!(
        write_tags(&opus, &c, None, &WriteOptions::default()),
        Err(WriteError::InvalidValue { .. })
    ));

    let Some(vorbis) = encode(&dir, "a.ogg", WRITABLE[3], &[] as &[&str]) else {
        return;
    };
    c = TagChanges::new();
    c.set_one(TagField::R128TrackGain, "-1280");
    let outcome = write_tags(&vorbis, &c, None, &WriteOptions::default()).expect("Vorbis writes");
    assert_eq!(outcome.skipped.len(), 1, "{:?}", outcome.skipped);
    assert!(values(&read(&vorbis), TagField::R128TrackGain).is_empty());
}
