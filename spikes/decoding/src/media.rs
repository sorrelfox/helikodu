// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Creates the test media: a known reference signal written as WAV by this
//! program, then encoded with the command-line tools (ffmpeg, lame, opusenc,
//! flac, wavpack). Nothing here is committed; it all lands in target/media.

use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Frame counts are deliberately not multiples of any codec frame size.
pub const FRAMES_44K: usize = 220_757; // 5.0058 s
pub const FRAMES_48K: usize = 240_263; // 5.0055 s

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    S44k16,
    S48k16,
    S48k24,
}

impl Source {
    pub fn rate(self) -> u32 {
        match self {
            Source::S44k16 => 44_100,
            _ => 48_000,
        }
    }
    pub fn frames(self) -> usize {
        match self {
            Source::S44k16 => FRAMES_44K,
            _ => FRAMES_48K,
        }
    }
    pub fn bits(self) -> u32 {
        match self {
            Source::S48k24 => 24,
            _ => 16,
        }
    }
    pub fn wav_name(self) -> &'static str {
        match self {
            Source::S44k16 => "src_44k_16.wav",
            Source::S48k16 => "src_48k_16.wav",
            Source::S48k24 => "src_48k_24.wav",
        }
    }
}

/// The reference signal as integers, interleaved stereo.
pub fn reference_ints(src: Source) -> Vec<i32> {
    let rate = f64::from(src.rate());
    let n = src.frames();
    let dur = n as f64 / rate;
    let scale = f64::from(1u32 << (src.bits() - 1));
    let mut lcg: u32 = 0x1234_5678;
    let mut noise = move || {
        lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(lcg >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0
    };
    // A rising and a falling chirp: never periodic, so a cross-correlation has
    // one clear peak and any start offset shows up as a non-zero lag.
    let chirp = |t: f64, f0: f64, f1: f64| (TAU * (f0 * t + (f1 - f0) * t * t / (2.0 * dur))).sin();
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / rate;
        let l = 0.25 * chirp(t, 100.0, 8000.0) + 0.02 * noise();
        let r = 0.20 * chirp(t, 6000.0, 150.0) + 0.02 * noise();
        for x in [l, r] {
            out.push((x * scale).round().clamp(-scale, scale - 1.0) as i32);
        }
    }
    out
}

pub fn reference(src: Source) -> Vec<f64> {
    let scale = f64::from(1u32 << (src.bits() - 1));
    reference_ints(src)
        .into_iter()
        .map(|v| f64::from(v) / scale)
        .collect()
}

fn write_wav(path: &Path, src: Source) -> std::io::Result<()> {
    let ints = reference_ints(src);
    let bytes_per = (src.bits() / 8) as usize;
    let data_len = ints.len() * bytes_per;
    let mut b = Vec::with_capacity(44 + data_len);
    let block_align = 2 * bytes_per as u16;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&src.rate().to_le_bytes());
    b.extend_from_slice(&(src.rate() * u32::from(block_align)).to_le_bytes());
    b.extend_from_slice(&block_align.to_le_bytes());
    b.extend_from_slice(&(src.bits() as u16).to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    for v in ints {
        b.extend_from_slice(&v.to_le_bytes()[..bytes_per]);
    }
    fs::write(path, b)
}

/// One encoded test file.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: &'static str,
    pub file: &'static str,
    pub source: Source,
    pub lossless: bool,
    /// False where FFmpeg is known to have no gapless information to use.
    pub expect_exact: bool,
    pub tool: &'static str,
    args: &'static [&'static str],
}

const FF: &[&str] = &["-hide_banner", "-loglevel", "error", "-y"];

/// `{in}` is the source WAV, `{out}` the output file, `{cover}` a PNG.
pub const CASES: &[Case] = &[
    Case {
        name: "MP3 lame -V2 (LAME tag)",
        file: "mp3_lame_vbr.mp3",
        source: Source::S44k16,
        lossless: false,
        expect_exact: true,
        tool: "lame",
        args: &["--quiet", "-V2", "{in}", "{out}"],
    },
    Case {
        name: "MP3 lame CBR 192 (Info tag)",
        file: "mp3_lame_cbr.mp3",
        source: Source::S44k16,
        lossless: false,
        expect_exact: true,
        tool: "lame",
        args: &["--quiet", "-b", "192", "{in}", "{out}"],
    },
    Case {
        name: "MP3 ffmpeg libmp3lame",
        file: "mp3_ffmpeg.mp3",
        source: Source::S44k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "libmp3lame", "-b:a", "192k", "{out}"],
    },
    Case {
        name: "MP3 lame -t (no info tag)",
        file: "mp3_lame_notag.mp3",
        source: Source::S44k16,
        lossless: false,
        expect_exact: false,
        tool: "lame",
        args: &["--quiet", "-t", "-b", "192", "{in}", "{out}"],
    },
    Case {
        name: "AAC-LC ffmpeg aac, M4A",
        file: "aac_ffmpeg.m4a",
        source: Source::S44k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "aac", "-b:a", "192k", "{out}"],
    },
    Case {
        name: "AAC-LC libfdk_aac, M4A",
        file: "aac_fdk.m4a",
        source: Source::S44k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "libfdk_aac", "-b:a", "192k", "{out}"],
    },
    // HE-AAC is not covered: the libfdk_aac in Fedora's FFmpeg is the patent-free
    // build and refuses the HE profile ("Unable to set the AOT 5").
    Case {
        name: "AAC-LC M4A without edit list",
        file: "aac_noedts.m4a",
        source: Source::S44k16,
        lossless: false,
        expect_exact: false,
        tool: "ffmpeg",
        args: &[
            "-i",
            "{in}",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-use_editlist",
            "0",
            "{out}",
        ],
    },
    Case {
        name: "AAC-LC raw ADTS",
        file: "aac_adts.aac",
        source: Source::S44k16,
        lossless: false,
        expect_exact: false,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "aac", "-b:a", "192k", "{out}"],
    },
    Case {
        name: "Opus opusenc, Ogg",
        file: "opus_opusenc.opus",
        source: Source::S48k16,
        lossless: false,
        expect_exact: true,
        tool: "opusenc",
        args: &["--quiet", "{in}", "{out}"],
    },
    Case {
        name: "Opus ffmpeg libopus, Ogg",
        file: "opus_ffmpeg.opus",
        source: Source::S48k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "libopus", "-b:a", "128k", "{out}"],
    },
    Case {
        name: "Opus ffmpeg libopus, Matroska",
        file: "opus_ffmpeg.mka",
        source: Source::S48k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "libopus", "-b:a", "128k", "{out}"],
    },
    Case {
        name: "Opus ffmpeg libopus, WebM",
        file: "opus_ffmpeg.webm",
        source: Source::S48k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "libopus", "-b:a", "128k", "{out}"],
    },
    // Known gap, measured here: the edit list is written correctly, but FFmpeg 9.0.2
    // does not trim the end padding of Opus in MP4 (the ffmpeg CLI shows the same).
    Case {
        name: "Opus ffmpeg libopus, MP4",
        file: "opus_ffmpeg.mp4",
        source: Source::S48k16,
        lossless: false,
        expect_exact: false,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "libopus", "-b:a", "128k", "{out}"],
    },
    Case {
        name: "Vorbis ffmpeg libvorbis, Ogg",
        file: "vorbis.ogg",
        source: Source::S44k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "libvorbis", "-q:a", "5", "{out}"],
    },
    Case {
        name: "FLAC flac -8, 16 bit",
        file: "flac_16.flac",
        source: Source::S44k16,
        lossless: true,
        expect_exact: true,
        tool: "flac",
        args: &["--silent", "-f", "-8", "-o", "{out}", "{in}"],
    },
    Case {
        name: "FLAC flac -5, 24 bit 48 kHz",
        file: "flac_24.flac",
        source: Source::S48k24,
        lossless: true,
        expect_exact: true,
        tool: "flac",
        args: &["--silent", "-f", "-o", "{out}", "{in}"],
    },
    Case {
        name: "FLAC ffmpeg, Matroska",
        file: "flac_ffmpeg.mka",
        source: Source::S44k16,
        lossless: true,
        expect_exact: true,
        tool: "ffmpeg",
        args: &["-i", "{in}", "-c:a", "flac", "{out}"],
    },
    Case {
        name: "WavPack wavpack",
        file: "wavpack.wv",
        source: Source::S44k16,
        lossless: true,
        expect_exact: true,
        tool: "wavpack",
        args: &["-q", "-y", "{in}", "-o", "{out}"],
    },
    Case {
        name: "MP3 with cover art (lame --ti)",
        file: "mp3_cover.mp3",
        source: Source::S44k16,
        lossless: false,
        expect_exact: true,
        tool: "lame",
        args: &["--quiet", "-b", "192", "--ti", "{cover}", "{in}", "{out}"],
    },
    Case {
        name: "FLAC with cover art (flac --picture)",
        file: "flac_cover.flac",
        source: Source::S44k16,
        lossless: true,
        expect_exact: true,
        tool: "flac",
        args: &["--silent", "-f", "--picture={cover}", "-o", "{out}", "{in}"],
    },
    // Video containers. The video is longer than the audio, so it is never cut.
    Case {
        name: "MP4: MPEG-4 video + AAC",
        file: "video_mpeg4_aac.mp4",
        source: Source::S44k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25:duration=6",
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
            "-b:a",
            "192k",
            "{out}",
        ],
    },
    Case {
        name: "MKV: VP8 video + AAC (not default) + Opus (default)",
        file: "video_vp8_2audio.mkv",
        source: Source::S48k16,
        lossless: false,
        expect_exact: true,
        tool: "ffmpeg",
        args: &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25:duration=6",
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
            "300k",
            "-c:a:0",
            "aac",
            "-b:a:0",
            "96k",
            "-c:a:1",
            "libopus",
            "-b:a:1",
            "128k",
            "-disposition:a:0",
            "0",
            "-disposition:a:1",
            "default",
            "{out}",
        ],
    },
    Case {
        name: "MKV: FLAC first, then VP9 video",
        file: "video_flac_vp9.mkv",
        source: Source::S44k16,
        lossless: true,
        expect_exact: true,
        tool: "ffmpeg",
        args: &[
            "-i",
            "{in}",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=10:duration=6",
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:a",
            "flac",
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "realtime",
            "-b:v",
            "200k",
            "{out}",
        ],
    },
];

pub fn path_of(dir: &Path, case: &Case) -> PathBuf {
    dir.join(case.file)
}

fn run(tool: &str, args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new(tool);
    if tool == "ffmpeg" {
        cmd.args(FF);
    }
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| format!("{tool}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{tool} {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Writes the sources and encodes every case that is not there yet.
pub fn create_all(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for src in [Source::S44k16, Source::S48k16, Source::S48k24] {
        let p = dir.join(src.wav_name());
        if !p.exists() {
            write_wav(&p, src).map_err(|e| e.to_string())?;
        }
    }
    let cover = dir.join("cover.png");
    if !cover.exists() {
        run(
            "ffmpeg",
            &strings(&[
                "-f",
                "lavfi",
                "-i",
                "color=c=0x3080c0:s=64x64",
                "-frames:v",
                "1",
                &s(&cover),
            ]),
        )?;
    }
    for case in CASES {
        let out = path_of(dir, case);
        if out.exists() {
            continue;
        }
        let input = dir.join(case.source.wav_name());
        let args: Vec<String> = case
            .args
            .iter()
            .map(|a| {
                a.replace("{in}", &s(&input))
                    .replace("{out}", &s(&out))
                    .replace("{cover}", &s(&cover))
            })
            .collect();
        // A missing tool or encoder (libfdk_aac is not in every FFmpeg build)
        // skips that case instead of stopping the whole run.
        if let Err(e) = run(case.tool, &args) {
            eprintln!("skipping {}: {e}", case.file);
        }
    }
    Ok(())
}

pub fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| (*x).to_owned()).collect()
}

pub fn ffmpeg(args: &[&str]) -> Result<(), String> {
    run("ffmpeg", &strings(args))
}
