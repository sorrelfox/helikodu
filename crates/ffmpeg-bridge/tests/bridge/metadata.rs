// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tags, chapters, attached pictures and durations.

use helikodu_ffmpeg_bridge::{MediaFile, PictureLimits, PictureSource};

use crate::media::{self, FRAMES_44K, Source};

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

#[test]
fn flac_tags_and_cover() {
    let Some(path) = media::encode(
        "tagged_cover.flac",
        Source::S44k16,
        "ffmpeg",
        &["flac", "png"],
        &[
            "-i",
            "{in}",
            "-i",
            "{cover}",
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:a",
            "flac",
            "-c:v",
            "png",
            "-disposition:v",
            "attached_pic",
            "-metadata:s:v",
            "comment=Cover (front)",
            "-metadata",
            "title=Tëst title",
            "-metadata",
            "artist=Someone",
            "-metadata",
            "REPLAYGAIN_TRACK_GAIN=-6.50 dB",
            "-f",
            "flac",
            "{out}",
        ],
    ) else {
        return;
    };
    let file = MediaFile::open(&path).expect("open");
    let tags = file.tags();
    assert_eq!(tags.get("TITLE"), Some("Tëst title"));
    assert_eq!(tags.get("artist"), Some("Someone"));
    assert_eq!(tags.get("replaygain_track_gain"), Some("-6.50 dB"));

    let pictures = file.attached_pictures(&PictureLimits::default());
    assert_eq!(pictures.len(), 1);
    let picture = &pictures[0];
    assert_eq!(picture.source, PictureSource::AttachedPicture);
    assert_eq!(picture.mime_type.as_deref(), Some("image/png"));
    assert!(picture.data.starts_with(PNG_MAGIC));
    assert_eq!(picture.description.as_deref(), Some("Cover (front)"));

    let tiny = PictureLimits {
        max_bytes: 16,
        max_count: 16,
    };
    assert!(
        file.attached_pictures(&tiny).is_empty(),
        "the size limit applies"
    );
    let none = PictureLimits {
        max_bytes: usize::MAX,
        max_count: 0,
    };
    assert!(
        file.attached_pictures(&none).is_empty(),
        "the count limit applies"
    );

    let duration = file.duration().expect("duration");
    assert_eq!(
        duration.as_duration().as_millis(),
        (FRAMES_44K as u128 * 1000) / 44_100
    );
}

#[test]
fn ogg_tags_are_on_the_stream() {
    let Some(path) = media::ffmpeg(
        "tagged.opus",
        Source::S48k16,
        &["libopus"],
        &["-c:a", "libopus", "-metadata", "album=An album"],
        "ogg",
    ) else {
        return;
    };
    let file = MediaFile::open(&path).expect("open");
    assert_eq!(file.tags_with_stream(0).get("album"), Some("An album"));
}

#[test]
fn chapters_are_read() {
    let meta = media::dir().join("chapters.ffmeta");
    std::fs::write(
        &meta,
        ";FFMETADATA1\ntitle=With chapters\n\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=2000\ntitle=First\n\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=2000\nEND=5000\ntitle=Second\n",
    )
    .expect("write");
    let meta = media::s(&meta);
    let Some(path) = media::encode(
        "chapters.mka",
        Source::S44k16,
        "ffmpeg",
        &["flac"],
        &[
            "-i",
            "{in}",
            "-i",
            &meta,
            "-map",
            "0:a",
            "-map_metadata",
            "1",
            "-map_chapters",
            "1",
            "-c:a",
            "flac",
            "-f",
            "matroska",
            "{out}",
        ],
    ) else {
        return;
    };
    let file = MediaFile::open(&path).expect("open");
    let chapters = file.chapters();
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0].title.as_deref(), Some("First"));
    assert_eq!(chapters[1].title.as_deref(), Some("Second"));
    assert_eq!(chapters[1].start.as_duration().as_millis(), 2000);
    assert_eq!(
        chapters[1].end.expect("end").as_duration().as_millis(),
        5000
    );
}

#[test]
fn matroska_image_attachments_are_pictures() {
    let Some(cover) = media::cover_png() else {
        return;
    };
    let cover = media::s(&cover);
    let Some(path) = media::encode(
        "attachment.mka",
        Source::S44k16,
        "ffmpeg",
        &["flac"],
        &[
            "-i",
            "{in}",
            "-map",
            "0:a",
            "-c:a",
            "flac",
            "-attach",
            &cover,
            "-metadata:s:t",
            "mimetype=image/png",
            "-f",
            "matroska",
            "{out}",
        ],
    ) else {
        return;
    };
    let file = MediaFile::open(&path).expect("open");
    let pictures = file.attached_pictures(&PictureLimits::default());
    assert_eq!(pictures.len(), 1, "{:?}", file.streams());
    // FFmpeg turns image attachments into attached-picture streams.
    assert_eq!(pictures[0].source, PictureSource::AttachedPicture);
    assert!(pictures[0].data.starts_with(PNG_MAGIC));
    assert_eq!(pictures[0].description.as_deref(), Some("cover.png"));
}
