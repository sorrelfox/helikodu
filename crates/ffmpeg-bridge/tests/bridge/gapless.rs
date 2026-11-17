// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Encoder delay and padding are trimmed: lossy files decode to exactly the source
//! length, aligned with the source (cross-correlation lag 0).

use std::path::Path;

use crate::media::{self, Source};

/// Decodes `path` and checks length, alignment and the reported duration.
fn check_exact(path: &Path, src: Source) {
    let d = media::decode(path);
    assert_eq!(d.spec.rate.hz(), src.rate(), "{}", path.display());
    assert_eq!(d.frames(), src.frames(), "length of {}", path.display());
    let lag = media::lag(&d.samples, &media::reference(src), 2, 3000);
    assert_eq!(lag, 0, "start of {} is not aligned", path.display());
}

#[test]
fn mp3_lame_tag() {
    if let Some(p) = media::encode(
        "lame_v2.mp3",
        Source::S44k16,
        "lame",
        &[],
        &["--quiet", "-V2", "{in}", "{out}"],
    ) {
        check_exact(&p, Source::S44k16);
    }
}

#[test]
fn mp3_ffmpeg_libmp3lame() {
    if let Some(p) = media::ffmpeg(
        "ffmpeg_lame.mp3",
        Source::S44k16,
        &["libmp3lame"],
        &["-c:a", "libmp3lame", "-b:a", "192k"],
        "mp3",
    ) {
        check_exact(&p, Source::S44k16);
    }
}

#[test]
fn opus_in_ogg_opusenc() {
    if let Some(p) = media::encode(
        "opusenc.opus",
        Source::S48k16,
        "opusenc",
        &[],
        &["--quiet", "{in}", "{out}"],
    ) {
        check_exact(&p, Source::S48k16);
    }
}

#[test]
fn opus_in_ogg_ffmpeg() {
    if let Some(p) = media::ffmpeg(
        "ffmpeg_libopus.opus",
        Source::S48k16,
        &["libopus"],
        &["-c:a", "libopus", "-b:a", "128k"],
        "ogg",
    ) {
        check_exact(&p, Source::S48k16);
        let d = media::decode(&p);
        let duration = d.decoder.duration().expect("Ogg states its length");
        assert_eq!(
            duration.frames_at(d.spec.rate),
            Source::S48k16.frames() as u64
        );
    }
}

/// FFmpeg 9.0 does not trim the end padding of Opus in MP4; the bridge does, from
/// the edit list.
#[test]
fn opus_in_mp4_end_is_trimmed() {
    if let Some(p) = media::ffmpeg(
        "ffmpeg_libopus.mp4",
        Source::S48k16,
        &["libopus"],
        &["-c:a", "libopus", "-b:a", "128k"],
        "mp4",
    ) {
        check_exact(&p, Source::S48k16);
        let d = media::decode(&p);
        let duration = d
            .decoder
            .duration()
            .expect("the edit list gives the length");
        assert_eq!(
            duration.frames_at(d.spec.rate),
            Source::S48k16.frames() as u64
        );
    }
}

/// FFmpeg 7.1 does not trim the end padding of AAC in MP4; the bridge does.
#[test]
fn aac_in_m4a() {
    if let Some(p) = media::ffmpeg(
        "ffmpeg_aac.m4a",
        Source::S44k16,
        &["aac"],
        &["-c:a", "aac", "-b:a", "192k"],
        "ipod",
    ) {
        check_exact(&p, Source::S44k16);
    }
}

#[test]
fn vorbis_in_ogg() {
    if let Some(p) = media::ffmpeg(
        "ffmpeg_vorbis.ogg",
        Source::S44k16,
        &["libvorbis"],
        &["-c:a", "libvorbis"],
        "ogg",
    ) {
        check_exact(&p, Source::S44k16);
    }
}
