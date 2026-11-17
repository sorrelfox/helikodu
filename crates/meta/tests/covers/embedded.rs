// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Embedded pictures: each tag kind, picture-type selection, the FFmpeg fallback
//! and the limits.

use std::fs::{self, File};
use std::path::Path;

use helikodu_meta::covers::{
    CoverHash, CoverLimits, CoverSource, EmbeddedCovers, EmbeddedTag, PictureFormat, PictureType,
    SkipReason, read_embedded_covers,
};
use lofty::TextEncoding;
use lofty::ape::{ApeItem, ApeTag};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::flac::FlacFile;
use lofty::id3::v2::{AttachedPictureFrame, Frame, Id3v2Tag};
use lofty::ogg::{OggPictureStorage, OpusFile};
use lofty::picture::{MimeType, Picture, PictureInformation, PictureType as LoftyType};
use lofty::tag::{ItemValue, TagExt};

use crate::media::{GIF, fresh_dir, jpeg, plain, png, with_attached_picture};

fn read(path: &Path) -> EmbeddedCovers {
    read_embedded_covers(path, &CoverLimits::default()).expect("pictures can be read")
}

fn tag_of(covers: &EmbeddedCovers, i: usize) -> EmbeddedTag {
    match covers.covers[i].source() {
        CoverSource::Embedded { tag, .. } => *tag,
        other => panic!("not embedded: {other:?}"),
    }
}

fn picture(data: Vec<u8>, kind: LoftyType, description: &str) -> Picture {
    Picture::unchecked(data)
        .pic_type(kind)
        .mime_type(MimeType::Jpeg)
        .description(description.to_owned())
        .build()
}

fn write_id3v2(path: &Path, pictures: Vec<Picture>) {
    let mut tag = Id3v2Tag::new();
    for p in pictures {
        tag.insert(Frame::Picture(AttachedPictureFrame::new(
            TextEncoding::UTF8,
            p,
        )));
    }
    tag.save_to_path(path, WriteOptions::default())
        .expect("lofty writes the ID3v2 tag");
}

/// Checks a file with one attached front cover written by ffmpeg.
fn check_one_front_cover(name: &str, encoder: &str, expected_tag: EmbeddedTag, typed: bool) {
    let dir = fresh_dir("embedded-one");
    let cover = jpeg(64, 48, [200, 40, 40]);
    let cover_path = dir.join("cover-source.jpg");
    fs::write(&cover_path, &cover).expect("write the picture");
    let Some(path) = with_attached_picture(&dir, name, encoder, &cover_path, Some("Cover (front)"))
    else {
        return;
    };
    let covers = read(&path);
    assert_eq!(covers.covers.len(), 1, "{name}: {covers:?}");
    assert!(covers.skipped.is_empty(), "{name}: {covers:?}");
    assert_eq!(covers.fallback_reason, None, "{name}");
    assert_eq!(tag_of(&covers, 0), expected_tag, "{name}");
    let front = covers.front().expect("a front cover");
    let expected_type = typed.then_some(PictureType::FRONT_COVER);
    assert_eq!(front.source().picture_type(), expected_type, "{name}");
    assert_eq!(front.bytes().expect("bytes"), &cover[..], "{name}");
    assert_eq!(front.format().expect("format"), PictureFormat::Jpeg);
    assert_eq!(front.hash().expect("hash"), CoverHash::of(&cover));
    let info = front.probe(&CoverLimits::default()).expect("probe");
    assert_eq!((info.width, info.height), (64, 48));
}

#[test]
fn mp3_apic() {
    check_one_front_cover("apic.mp3", "libmp3lame", EmbeddedTag::Id3v2, true);
}

#[test]
fn flac_picture_block() {
    check_one_front_cover("picture.flac", "flac", EmbeddedTag::FlacPicture, true);
}

#[test]
fn m4a_covr() {
    // MP4 `covr` atoms carry no picture type.
    check_one_front_cover("covr.m4a", "aac", EmbeddedTag::Mp4Cover, false);
}

#[test]
fn opus_metadata_block_picture() {
    let dir = fresh_dir("embedded-opus");
    let Some(path) = plain(&dir, "picture.opus", "libopus") else {
        return;
    };
    let cover = png(40, 40, [20, 200, 20]);
    let mut file = OpusFile::read_from(&mut File::open(&path).expect("open"), ParseOptions::new())
        .expect("lofty reads the Opus file");
    let comments = file.vorbis_comments_mut();
    let p = Picture::unchecked(cover.clone())
        .pic_type(LoftyType::CoverFront)
        .mime_type(MimeType::Png)
        .build();
    let info = PictureInformation::from_picture(&p).expect("picture information");
    comments.insert_picture(p, Some(info)).expect("insert");
    comments
        .save_to_path(&path, WriteOptions::default())
        .expect("lofty writes the comments");

    let covers = read(&path);
    assert_eq!(covers.covers.len(), 1, "{covers:?}");
    assert_eq!(tag_of(&covers, 0), EmbeddedTag::VorbisComment);
    let front = covers.front().expect("a front cover");
    assert_eq!(
        front.source().picture_type(),
        Some(PictureType::FRONT_COVER)
    );
    assert_eq!(front.bytes().expect("bytes"), &cover[..]);
    assert_eq!(front.format().expect("format"), PictureFormat::Png);
}

#[test]
fn ape_cover_art_item() {
    let dir = fresh_dir("embedded-ape");
    let Some(path) = plain(&dir, "cover.wv", "wavpack") else {
        return;
    };
    let back = jpeg(32, 32, [0, 0, 200]);
    let front = jpeg(16, 16, [200, 200, 0]);
    let mut tag = ApeTag::new();
    for (key, data) in [("Cover Art (Back)", &back), ("Cover Art (Front)", &front)] {
        let mut value = b"cover.jpg\0".to_vec();
        value.extend_from_slice(data);
        tag.insert(ApeItem::new(key.to_owned(), ItemValue::Binary(value)).expect("item"));
    }
    tag.save_to_path(&path, WriteOptions::default())
        .expect("lofty writes the APE tag");

    let covers = read(&path);
    assert_eq!(covers.covers.len(), 2, "{covers:?}");
    let chosen = covers.front().expect("a front cover");
    match chosen.source() {
        CoverSource::Embedded {
            tag,
            picture_type,
            description,
            ..
        } => {
            assert_eq!(*tag, EmbeddedTag::Ape);
            assert_eq!(*picture_type, Some(PictureType::FRONT_COVER));
            assert_eq!(description.as_deref(), Some("cover.jpg"));
        }
        other => panic!("not embedded: {other:?}"),
    }
    assert_eq!(chosen.bytes().expect("bytes"), &front[..]);
}

#[test]
fn the_front_cover_wins_over_other_types() {
    let dir = fresh_dir("embedded-types");
    let Some(path) = plain(&dir, "types.mp3", "libmp3lame") else {
        return;
    };
    let back = jpeg(300, 300, [0, 0, 255]);
    let artist = jpeg(200, 200, [0, 255, 0]);
    let front = jpeg(20, 20, [255, 0, 0]);
    write_id3v2(
        &path,
        vec![
            picture(back, LoftyType::CoverBack, "back"),
            picture(artist, LoftyType::Artist, "artist"),
            picture(front.clone(), LoftyType::CoverFront, "front"),
        ],
    );
    let covers = read(&path);
    assert_eq!(covers.covers.len(), 3, "{covers:?}");
    let chosen = covers.front().expect("a cover");
    assert_eq!(
        chosen.source().picture_type(),
        Some(PictureType::FRONT_COVER)
    );
    assert_eq!(chosen.bytes().expect("bytes"), &front[..]);
    let types: Vec<_> = covers
        .covers
        .iter()
        .map(|c| c.source().picture_type())
        .collect();
    assert_eq!(
        types,
        [
            Some(PictureType::BACK_COVER),
            Some(PictureType(8)),
            Some(PictureType::FRONT_COVER)
        ]
    );
}

#[test]
fn without_a_front_cover_other_beats_the_back_cover() {
    let dir = fresh_dir("embedded-other");
    let Some(path) = plain(&dir, "other.mp3", "libmp3lame") else {
        return;
    };
    let back = jpeg(300, 300, [0, 0, 255]);
    let other = jpeg(30, 30, [9, 9, 9]);
    write_id3v2(
        &path,
        vec![
            picture(back, LoftyType::CoverBack, "back"),
            picture(other.clone(), LoftyType::Other, "other"),
        ],
    );
    let covers = read(&path);
    let chosen = covers.into_front().expect("a cover");
    assert_eq!(chosen.bytes().expect("bytes"), &other[..]);
}

#[test]
fn of_two_front_covers_the_larger_wins() {
    let dir = fresh_dir("embedded-two-fronts");
    let Some(path) = plain(&dir, "fronts.flac", "flac") else {
        return;
    };
    let small = jpeg(16, 16, [1, 2, 3]);
    let large = jpeg(128, 128, [3, 2, 1]);
    let mut file = FlacFile::read_from(&mut File::open(&path).expect("open"), ParseOptions::new())
        .expect("lofty reads the FLAC file");
    for (data, d) in [(small, "small"), (large.clone(), "large")] {
        let p = picture(data, LoftyType::CoverFront, d);
        let info = PictureInformation::from_picture(&p).expect("picture information");
        file.insert_picture(p, Some(info)).expect("insert");
    }
    file.save_to_path(&path, WriteOptions::default())
        .expect("lofty writes the FLAC file");

    let covers = read(&path);
    assert_eq!(covers.covers.len(), 2, "{covers:?}");
    assert_eq!(tag_of(&covers, 0), EmbeddedTag::FlacPicture);
    assert_eq!(
        covers.front().expect("a cover").bytes().expect("bytes"),
        &large[..]
    );
}

#[test]
fn format_comes_from_the_bytes_not_the_declared_type() {
    let dir = fresh_dir("embedded-sniff");
    let Some(path) = plain(&dir, "sniff.mp3", "libmp3lame") else {
        return;
    };
    // Declared JPEG, actually PNG; then a GIF; then no picture at all.
    let actual_png = png(10, 10, [5, 5, 5]);
    write_id3v2(
        &path,
        vec![
            picture(actual_png, LoftyType::CoverFront, "png"),
            picture(GIF.to_vec(), LoftyType::CoverBack, "gif"),
            picture(
                b"<html>not a picture</html>".to_vec(),
                LoftyType::Media,
                "html",
            ),
        ],
    );
    let covers = read(&path);
    assert_eq!(covers.covers.len(), 1, "{covers:?}");
    assert_eq!(
        covers.covers[0].format().expect("format"),
        PictureFormat::Png
    );
    let reasons: Vec<_> = covers.skipped.iter().map(|s| s.reason).collect();
    assert_eq!(
        reasons,
        [
            SkipReason::UnsupportedFormat(PictureFormat::Gif),
            SkipReason::UnknownFormat
        ]
    );
    // Indexes count skipped pictures too.
    match &covers.skipped[1].source {
        CoverSource::Embedded { index, .. } => assert_eq!(*index, 2),
        other => panic!("not embedded: {other:?}"),
    }
}

#[test]
fn byte_and_count_limits() {
    let dir = fresh_dir("embedded-limits");
    let Some(path) = plain(&dir, "limits.mp3", "libmp3lame") else {
        return;
    };
    let big = jpeg(256, 256, [100, 100, 100]);
    let small = [
        jpeg(8, 8, [1, 1, 1]),
        jpeg(8, 8, [2, 2, 2]),
        jpeg(8, 8, [3, 3, 3]),
    ];
    let limit = small.iter().map(Vec::len).max().expect("three") + 1;
    assert!(big.len() > limit);
    write_id3v2(
        &path,
        vec![
            picture(big, LoftyType::CoverFront, "big"),
            picture(small[0].clone(), LoftyType::Other, "a"),
            picture(small[1].clone(), LoftyType::Other, "b"),
            picture(small[2].clone(), LoftyType::Other, "c"),
        ],
    );
    let limits = CoverLimits {
        max_bytes: limit,
        max_count: 2,
        ..CoverLimits::default()
    };
    let covers = read_embedded_covers(&path, &limits).expect("pictures can be read");
    assert_eq!(covers.covers.len(), 2, "{covers:?}");
    let reasons: Vec<_> = covers.skipped.iter().map(|s| s.reason).collect();
    assert_eq!(reasons, [SkipReason::TooLarge, SkipReason::TooMany]);
    // The oversized front cover is not chosen; the first "other" picture is.
    let chosen = covers.front().expect("a cover");
    assert_eq!(chosen.bytes().expect("bytes"), &small[0][..]);
}

#[test]
fn matroska_through_ffmpeg() {
    let dir = fresh_dir("embedded-mka");
    let cover = jpeg(24, 24, [10, 120, 230]);
    let cover_path = dir.join("cover.jpg");
    fs::write(&cover_path, &cover).expect("write the picture");
    let args = [
        "-attach",
        cover_path.to_str().expect("UTF-8 path"),
        "-metadata:s:t",
        "mimetype=image/jpeg",
        "-metadata:s:t",
        "filename=cover.jpg",
    ];
    let Some(path) = crate::media::ffmpeg(&dir, "attached.mka", "flac", &[], &args) else {
        return;
    };
    let covers = read(&path);
    assert!(covers.fallback_reason.is_some(), "{covers:?}");
    assert_eq!(covers.covers.len(), 1, "{covers:?}");
    let front = covers.front().expect("a cover");
    assert!(matches!(
        tag_of(&covers, 0),
        EmbeddedTag::MatroskaAttachment | EmbeddedTag::FfmpegAttachedPicture
    ));
    assert_eq!(
        front.source().picture_type(),
        Some(PictureType::FRONT_COVER)
    );
    assert_eq!(front.bytes().expect("bytes"), &cover[..]);
}

#[test]
fn a_file_without_pictures() {
    let dir = fresh_dir("embedded-none");
    let Some(path) = plain(&dir, "none.flac", "flac") else {
        return;
    };
    let covers = read(&path);
    assert!(covers.covers.is_empty() && covers.skipped.is_empty());
    assert!(covers.front().is_none());
}

#[test]
fn a_file_nobody_can_read() {
    let dir = fresh_dir("embedded-garbage");
    let path = dir.join("garbage.mp3");
    fs::write(&path, [0x5Au8; 4096]).expect("write");
    assert!(read_embedded_covers(&path, &CoverLimits::default()).is_err());
}
