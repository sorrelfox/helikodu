// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stream listing and choice, and no video decoder is ever opened.

use helikodu_ffmpeg_bridge::{Error, MediaFile, StreamChoice, StreamKind};

use crate::codec_probe;
use crate::media::{self, FRAMES_44K, FRAMES_48K, Source};

/// Two audio tracks (AAC not default, Opus default) behind a VP8 video stream.
fn two_audio_mkv() -> Option<std::path::PathBuf> {
    media::encode(
        "video_vp8_2audio.mkv",
        Source::S48k16,
        "ffmpeg",
        &["libvpx", "aac", "libopus"],
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=10:duration=6",
            "-i",
            "{in}",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "1:a",
            "-c:v",
            "libvpx",
            "-b:v",
            "100k",
            "-c:a:0",
            "aac",
            "-c:a:1",
            "libopus",
            "-disposition:a:0",
            "0",
            "-disposition:a:1",
            "default",
            "-f",
            "matroska",
            "{out}",
        ],
    )
}

fn video_mp4() -> Option<std::path::PathBuf> {
    media::encode(
        "video_mpeg4_aac.mp4",
        Source::S44k16,
        "ffmpeg",
        &["mpeg4", "aac"],
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=10:duration=6",
            "-i",
            "{in}",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            "-f",
            "mp4",
            "{out}",
        ],
    )
}

#[test]
fn streams_are_listed_with_dispositions() {
    let Some(path) = two_audio_mkv() else { return };
    let file = MediaFile::open(&path).expect("open");
    let kinds: Vec<StreamKind> = file.streams().iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [StreamKind::Video, StreamKind::Audio, StreamKind::Audio]
    );
    let audio: Vec<_> = file.audio_streams().collect();
    assert_eq!(audio.len(), 2);
    assert_eq!(audio[0].codec, "aac");
    assert_eq!(audio[1].codec, "opus");
    assert!(!audio[0].disposition.is_default());
    assert!(audio[1].disposition.is_default());
    assert_eq!(audio[1].sample_rate, Some(48_000));
    assert_eq!(audio[1].channels, Some(2));
}

#[test]
fn the_default_stream_is_chosen_and_an_index_overrides_it() {
    let Some(path) = two_audio_mkv() else { return };
    let file = MediaFile::open(&path).expect("open");
    assert_eq!(file.choose_stream(StreamChoice::Auto).expect("choose"), 2);
    assert_eq!(
        file.choose_stream(StreamChoice::Index(1)).expect("choose"),
        1
    );
    assert!(matches!(
        file.choose_stream(StreamChoice::Index(0)),
        Err(Error::NotAnAudioStream { index: 0 })
    ));
    assert!(matches!(
        file.choose_stream(StreamChoice::Index(7)),
        Err(Error::NotAnAudioStream { index: 7 })
    ));

    let d = media::decode_rest(
        MediaFile::open(&path)
            .expect("open")
            .into_decoder(StreamChoice::Auto)
            .expect("decoder"),
    );
    assert_eq!(d.decoder.stream().codec, "opus");
    assert_eq!(d.frames(), FRAMES_48K);

    let decoder = MediaFile::open(&path)
        .expect("open")
        .into_decoder(StreamChoice::Index(1))
        .expect("decoder");
    assert_eq!(decoder.stream().codec, "aac");
    // AAC in Matroska carries no padding information, so only the start is exact.
    let d = media::decode_rest(decoder);
    assert!(d.frames() >= FRAMES_48K);
}

/// Every `avcodec_open2` call on this thread is recorded, FFmpeg's own included.
/// Calls for video streams are made, but refused by the codec whitelist before the
/// decoder is initialised.
#[test]
fn no_video_decoder_is_opened() {
    assert!(
        codec_probe::is_active(),
        "the avcodec_open2 probe is not in use"
    );
    let mut files = Vec::new();
    files.extend(video_mp4());
    files.extend(two_audio_mkv());
    files.extend(media::encode(
        "flac_cover.flac",
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
            "-f",
            "flac",
            "{out}",
        ],
    ));
    for path in files {
        codec_probe::take();
        let d = media::decode(&path);
        assert!(
            d.frames() == FRAMES_44K || d.frames() == FRAMES_48K,
            "{}",
            path.display()
        );
        let records = codec_probe::take();
        assert!(
            records.iter().any(|r| !r.is_video() && r.opened()),
            "the audio decoder was recorded: {records:?}"
        );
        let opened_video: Vec<&str> = records
            .iter()
            .filter(|r| r.is_video() && r.opened())
            .map(|r| r.codec.as_str())
            .collect();
        assert!(
            opened_video.is_empty(),
            "{}: video decoders opened: {opened_video:?}",
            path.display()
        );
        // FFmpeg did try: the codec whitelist is what refused it.
        assert!(
            records.iter().any(|r| r.is_video() && !r.opened()),
            "{}: no refused video decoder recorded: {records:?}",
            path.display()
        );
    }
}

#[test]
fn audio_of_a_video_file_plays() {
    let Some(path) = video_mp4() else { return };
    let file = MediaFile::open(&path).expect("open");
    assert_eq!(file.streams()[0].kind, StreamKind::Video);
    let d = media::decode_rest(file.into_decoder(StreamChoice::Auto).expect("decoder"));
    assert_eq!(d.decoder.stream_index(), 1);
    assert_eq!(d.frames(), FRAMES_44K);
}

#[test]
fn a_file_without_audio_is_refused() {
    let Some(path) = media::encode(
        "video_only.mkv",
        Source::S44k16,
        "ffmpeg",
        &["mpeg4"],
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=5:duration=1",
            "-c:v",
            "mpeg4",
            "-f",
            "matroska",
            "{out}",
        ],
    ) else {
        return;
    };
    let file = MediaFile::open(&path).expect("open");
    assert!(matches!(
        file.into_decoder(StreamChoice::Auto),
        Err(Error::NoAudioStream)
    ));
}

/// The generated corpus's video set (`cargo xtask corpus`), when it exists: no
/// file opens a video decoder, and every file with audio decodes it to the end.
#[test]
fn no_video_decoder_is_opened_for_the_corpus_video_set() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let corpus = std::env::var_os("HELIKODU_CORPUS")
        .map_or_else(|| root.join("target/corpus"), std::path::PathBuf::from);
    let Ok(entries) = std::fs::read_dir(corpus.join("video")) else {
        eprintln!(
            "SKIPPED: no corpus video set under {} (make it with `cargo xtask corpus`)",
            corpus.display()
        );
        return;
    };
    assert!(
        codec_probe::is_active(),
        "the avcodec_open2 probe is not in use"
    );
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    assert!(!paths.is_empty(), "the corpus video set is empty");
    for path in paths {
        codec_probe::take();
        let media =
            MediaFile::open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
        let has_audio = media.audio_streams().next().is_some();
        if has_audio {
            let mut decoder = media
                .into_decoder(StreamChoice::Auto)
                .unwrap_or_else(|e| panic!("decoder for {}: {e}", path.display()));
            let mut frames = 0;
            while let Some(block) = decoder
                .read()
                .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()))
            {
                frames += block.frames();
            }
            assert!(frames > 0, "{}: no audio decoded", path.display());
        } else {
            assert!(
                matches!(
                    media.into_decoder(StreamChoice::Auto),
                    Err(Error::NoAudioStream)
                ),
                "{}: a decoder without an audio stream",
                path.display()
            );
        }
        let records = codec_probe::take();
        let opened_video: Vec<&str> = records
            .iter()
            .filter(|r| r.is_video() && r.opened())
            .map(|r| r.codec.as_str())
            .collect();
        assert!(
            opened_video.is_empty(),
            "{}: video decoders opened: {opened_video:?}",
            path.display()
        );
        if has_audio {
            assert!(
                records.iter().any(|r| !r.is_video() && r.opened()),
                "{}: the audio decoder was not recorded: {records:?}",
                path.display()
            );
        }
    }
}
