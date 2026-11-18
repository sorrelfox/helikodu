// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The malformed-media corpus: damaged copies of small, valid files, made at test
//! time so that no media file is committed.
//!
//! The valid files are written by the FFmpeg bridge's own encoder (a short tone in
//! FLAC, WAV, MP3, Ogg Vorbis, Opus and AAC in MP4, as far as this FFmpeg build has
//! the encoders), tagged by lofty with a title, an artist, lyrics and a cover; DSF
//! and DSDIFF files, CUE sheets, LRC lyrics and playlists are written byte by byte.
//! Each is then damaged in fixed ways: cut short at several places, bytes flipped
//! in the header and in the body, a stretch zeroed, noise after the header, and
//! length fields set to 0, to the largest values and to one more than the file
//! holds. The damage comes from a fixed seed, so every run makes the same files.
//!
//! The files go to `<target>/tmp/meta-malformed/`, where they also make good seed
//! inputs for the fuzz targets.

use std::f64::consts::TAU;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use helikodu_ffmpeg_bridge::{
    AudioEncoder, EncodeFormat, EncodeQuality, EncoderConfig, encoder_availability,
};
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::Accessor;
use lofty::tag::{ItemKey, Tag, TagExt};

/// The length of the valid files, in seconds: long enough for several packets of
/// every codec, short enough to keep the corpus small.
const SECONDS: f64 = 0.15;

/// What kind of reader a corpus file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A media file: tags, covers, chapters and decoding.
    Media,
    /// A CUE sheet.
    Cue,
    /// An LRC lyrics file.
    Lyrics,
    /// A playlist.
    Playlist,
}

/// One file of the corpus.
#[derive(Debug, Clone)]
pub(crate) struct Sample {
    /// The file.
    pub(crate) path: PathBuf,
    /// Its reader.
    pub(crate) kind: Kind,
}

/// A small deterministic generator (xorshift64*), so the damage is the same in
/// every run.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// The folder of the corpus, emptied.
fn fresh_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("meta-malformed");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the corpus folder");
    dir
}

/// A tiny valid PNG (one red pixel), the cover of the tagged files.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// Encodes a stereo tone into `path`; false when this FFmpeg build cannot.
fn encode(path: &Path, format: EncodeFormat, rate: u32) -> bool {
    if !encoder_availability(format).is_available() {
        eprintln!(
            "no encoder for {}: its damaged copies are left out",
            format.name()
        );
        return false;
    }
    let config = EncoderConfig {
        format,
        sample_rate: rate,
        channels: 2,
        bits: 16,
        quality: if format.is_lossless() {
            EncodeQuality::Lossless(None)
        } else {
            EncodeQuality::Bitrate(96)
        },
    };
    let Ok(file) = File::create(path) else {
        return false;
    };
    let Ok(mut encoder) = AudioEncoder::create(file, &config) else {
        eprintln!("{} does not take {rate} Hz: left out", format.name());
        return false;
    };
    let frames = (f64::from(rate) * SECONDS) as usize;
    let samples: Vec<f32> = (0..frames)
        .flat_map(|i| {
            let s = (i as f64 / f64::from(rate) * 440.0 * TAU).sin() as f32 * 0.25;
            [s, -s]
        })
        .collect();
    encoder.write_f32(&samples).is_ok() && encoder.finish().is_ok()
}

/// Adds a title, an artist, lyrics and a cover to the file at `path`, in the
/// file's own kind of tag. A file lofty cannot tag stays as it is.
fn tag(path: &Path) {
    let Ok(tagged) = lofty::read_from_path(path) else {
        return;
    };
    let mut tag = Tag::new(tagged.primary_tag_type());
    tag.set_title("Шум и ярость".to_owned());
    tag.set_artist("Mari Kask".to_owned());
    tag.insert_text(
        ItemKey::Lyrics,
        "[00:00.00]first line\n[00:00.10]second line".to_owned(),
    );
    tag.push_picture(
        Picture::unchecked(PNG.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .build(),
    );
    let _ = tag.save_to_path(path, WriteOptions::default());
}

/// A DSF file of `bytes` bytes of DSD silence per channel.
fn dsf(channels: u32, bytes: usize) -> Vec<u8> {
    const BLOCK: usize = 4096;
    let groups = bytes.div_ceil(BLOCK);
    let data = groups * BLOCK * channels as usize;
    let mut b = Vec::new();
    b.extend_from_slice(b"DSD ");
    b.extend_from_slice(&28u64.to_le_bytes());
    b.extend_from_slice(&((28 + 52 + 12 + data) as u64).to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&52u64.to_le_bytes());
    for value in [1, 0, channels, channels, 2_822_400, 1] {
        b.extend_from_slice(&u32::to_le_bytes(value));
    }
    b.extend_from_slice(&((bytes * 8) as u64).to_le_bytes());
    b.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&((12 + data) as u64).to_le_bytes());
    b.resize(b.len() + data, 0x69);
    b
}

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = id.to_vec();
    b.extend_from_slice(&(body.len() as u64).to_be_bytes());
    b.extend_from_slice(body);
    if body.len() % 2 == 1 {
        b.push(0);
    }
    b
}

/// A stereo DSDIFF file of `bytes` bytes of DSD silence per channel.
fn dff(bytes: usize) -> Vec<u8> {
    let mut chnl = 2u16.to_be_bytes().to_vec();
    chnl.extend_from_slice(b"SLFTSRGT");
    let mut cmpr = b"DSD ".to_vec();
    cmpr.extend_from_slice(b"\x0eNot compressed\x00");
    let mut prop = b"SND ".to_vec();
    prop.extend(chunk(b"FS  ", &2_822_400u32.to_be_bytes()));
    prop.extend(chunk(b"CHNL", &chnl));
    prop.extend(chunk(b"CMPR", &cmpr));
    let mut form = b"DSD ".to_vec();
    form.extend(chunk(b"FVER", &0x0105_0000u32.to_be_bytes()));
    form.extend(chunk(b"PROP", &prop));
    form.extend(chunk(b"DSD ", &vec![0x69; bytes * 2]));
    chunk(b"FRM8", &form)
}

/// An MP3 file of silent MPEG-1 Layer III frames (128 kbit/s, 44.1 kHz, no
/// padding), after an ID3v2.3 tag that holds `tag_body`.
fn mp3_frames(frames: usize, tag_body: &[u8]) -> Vec<u8> {
    let mut b = b"ID3\x03\x00\x00".to_vec();
    let n = tag_body.len();
    b.extend_from_slice(&[
        (n >> 21) as u8 & 0x7f,
        (n >> 14) as u8 & 0x7f,
        (n >> 7) as u8 & 0x7f,
        n as u8 & 0x7f,
    ]);
    b.extend_from_slice(tag_body);
    for _ in 0..frames {
        b.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
        b.resize(b.len() + 417 - 4, 0);
    }
    b
}

/// An ID3v2.3 text frame.
fn id3_text(id: &[u8; 4], text: &str, size: Option<u32>) -> Vec<u8> {
    let mut b = id.to_vec();
    let body_len = u32::try_from(text.len() + 1).unwrap_or(u32::MAX);
    b.extend_from_slice(&size.unwrap_or(body_len).to_be_bytes());
    b.extend_from_slice(&[0, 0, 3]);
    b.extend_from_slice(text.as_bytes());
    b
}

/// MP4 boxes whose sizes lie.
fn mp4_lies() -> Vec<(&'static str, Vec<u8>)> {
    let ftyp = |size: u32| {
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(b"ftypM4A \0\0\0\0M4A isom");
        b
    };
    let mut zero = ftyp(24);
    zero.extend_from_slice(&0u32.to_be_bytes());
    zero.extend_from_slice(b"moov");
    let mut large = ftyp(24);
    large.extend_from_slice(&1u32.to_be_bytes());
    large.extend_from_slice(b"moov");
    large.extend_from_slice(&u64::MAX.to_be_bytes());
    let mut tiny = ftyp(24);
    tiny.extend_from_slice(&7u32.to_be_bytes());
    tiny.extend_from_slice(b"moov\0\0\0");
    let mut nested = ftyp(24);
    for _ in 0..200 {
        nested.extend_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        nested.extend_from_slice(b"moov");
    }
    vec![
        ("mp4-zero-size.m4a", zero),
        ("mp4-64bit-size.m4a", large),
        ("mp4-tiny-size.m4a", tiny),
        ("mp4-nested.m4a", nested),
        ("mp4-ftyp-huge.m4a", ftyp(u32::MAX)),
    ]
}

/// An Ogg page header that claims more segments than the file holds.
fn ogg_lies() -> Vec<(&'static str, Vec<u8>)> {
    let mut page = b"OggS\0\x02".to_vec();
    page.extend_from_slice(&[0; 8]); // granule
    page.extend_from_slice(&1u32.to_le_bytes()); // serial
    page.extend_from_slice(&0u32.to_le_bytes()); // sequence
    page.extend_from_slice(&0u32.to_le_bytes()); // checksum
    page.push(255);
    page.extend_from_slice(&[255; 255]);
    page.extend_from_slice(b"\x01vorbis");
    vec![("ogg-segments.ogg", page)]
}

/// Inputs the fuzz targets found, minimized; each crashed a reader once.
fn fuzz_finds() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        // An Opus header whose pre-skip and rate overflowed the tag library's
        // length computation (a panic).
        ("fuzz-opus-overflow.opus", OPUS_OVERFLOW.to_vec()),
        // An ADX header FFmpeg's decoder refuses, after which its demuxer divided
        // by zero (SIGFPE).
        ("fuzz-adx-header.adx", ADX_HEADER.to_vec()),
    ]
}

const OPUS_OVERFLOW: &[u8] = &[
    0x4f, 0x67, 0x67, 0x53, 0x00, 0x01, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x58,
    0xab, 0x2d, 0x00, 0x00, 0x00, 0x00, 0x0e, 0xc6, 0x0e, 0xee, 0x01, 0x13, 0x4f, 0x70, 0x75, 0x73,
    0x48, 0x65, 0x61, 0x64, 0x01, 0x01, 0x38, 0x01, 0x80, 0xbb, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4f,
    0x67, 0x67, 0x53, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x40, 0x58, 0xab,
    0x2d, 0x01, 0x00, 0x00, 0x00, 0x0e, 0xb4, 0xa8, 0x33, 0x01, 0x47, 0x4f, 0x70, 0x75, 0x73, 0x54,
    0x61, 0x67, 0x73, 0x0c, 0x00, 0x00, 0x00, 0x4c, 0x61, 0x76, 0x66, 0x36, 0x33, 0x2e, 0x31, 0x2e,
    0x31, 0x30, 0x32, 0x02, 0x00, 0x00, 0x00, 0x1c, 0x00, 0x00, 0x00, 0x65, 0x6e, 0x63, 0x6f, 0x64,
    0x65, 0x72, 0x3d, 0x4c, 0x74, 0x6c, 0x65, 0x54, 0x4f, 0x3d, 0x67, 0xb0, 0xb1, 0xae, 0xaf, 0xaf,
    0xb0, 0xb2, 0xb1, 0xb1, 0xb0, 0xb1, 0x73, 0x07, 0x00, 0x00, 0x00, 0x74, 0x74, 0x6c, 0x65, 0x54,
    0xb0, 0xb1,
];

const ADX_HEADER: &[u8] = &[
    0x80, 0x00, 0x00, 0x20, 0x03, 0x12, 0xac, 0x02, 0x00, 0x04, 0x00, 0x44, 0x00, 0x01, 0x58, 0xa0,
    0x01, 0xf4, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x63,
    0x29, 0x43, 0x52, 0x49, 0x00, 0x6f, 0xd7, 0xbf, 0x4d, 0x03, 0xfd, 0x42, 0xd1, 0xd4, 0xa7, 0xb5,
    0xa4, 0x3b, 0x03, 0x02, 0xde, 0x21, 0x07, 0x37, 0x49, 0xf5, 0x4b, 0xb3,
];

/// The damaged copies of `bytes`, by the name of the damage.
fn damage(bytes: &[u8], noise: &mut Noise) -> Vec<(String, Vec<u8>)> {
    let len = bytes.len();
    let mut out = Vec::new();
    for cut in [
        0,
        1,
        4,
        12,
        44,
        128,
        512,
        len / 3,
        len / 2,
        len.saturating_sub(1),
    ] {
        if cut < len {
            out.push((format!("cut-{cut}"), bytes[..cut].to_vec()));
        }
    }
    let head = len.min(512);
    for round in 0..3 {
        let mut b = bytes.to_vec();
        for _ in 0..4 {
            let at = noise.below(head);
            b[at] ^= 1 << noise.below(8);
        }
        out.push((format!("flip-head-{round}"), b));
    }
    for round in 0..2 {
        let mut b = bytes.to_vec();
        for _ in 0..16 {
            let at = noise.below(len);
            b[at] = noise.next() as u8;
        }
        out.push((format!("flip-body-{round}"), b));
    }
    let mut zeroed = bytes.to_vec();
    let start = len / 4;
    zeroed[start..start + (len / 4).min(4096)].fill(0);
    out.push(("zeroed".to_owned(), zeroed));
    let mut noisy = bytes[..head.min(128)].to_vec();
    noisy.extend(noise.bytes(len.min(16_384) - noisy.len().min(len.min(16_384))));
    out.push(("noise-after-header".to_owned(), noisy));
    for (name, value) in [
        ("len-zero", 0u32),
        ("len-max", u32::MAX),
        ("len-half", 0x7FFF_FFFF),
        ("len-over", u32::try_from(len + 1).unwrap_or(u32::MAX)),
    ] {
        // A word in the header that a parser likely reads as a length.
        let mut b = bytes.to_vec();
        let at = noise
            .below(head.saturating_sub(4).max(1))
            .min(len.saturating_sub(4));
        if len >= 4 {
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
            out.push((format!("{name}-le"), b.clone()));
            b[at..at + 4].copy_from_slice(&value.to_be_bytes());
            out.push((format!("{name}-be"), b));
        }
    }
    let mut twice = bytes[..len / 2].to_vec();
    twice.extend_from_slice(bytes);
    out.push(("header-twice".to_owned(), twice));
    out
}

const CUE: &str = "REM GENRE Jazz\nPERFORMER \"Mari Kask\"\nTITLE \"Tidewater\"\n\
FILE \"album.flac\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"One\"\n    INDEX 01 00:00:00\n\
  TRACK 02 AUDIO\n    TITLE \"Two\"\n    INDEX 00 00:01:00\n    INDEX 01 00:02:00\n";

/// Sheets that are wrong in their own ways.
fn cue_lies() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "cue-huge-index.cue",
            b"FILE \"a.flac\" WAVE\n TRACK 01 AUDIO\n INDEX 01 99999999999999999999:99:99\n \
              TRACK 4294967295 AUDIO\n INDEX 01 4294967295:59:74\n"
                .to_vec(),
        ),
        (
            "cue-backwards.cue",
            b"FILE \"a.flac\" WAVE\n TRACK 02 AUDIO\n INDEX 01 05:00:00\n TRACK 01 AUDIO\n INDEX 01 01:00:00\n"
                .to_vec(),
        ),
        (
            "cue-escape.cue",
            b"FILE \"../../../../etc/passwd\" WAVE\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n\
              FILE \"/etc/shadow\" WAVE\n TRACK 02 AUDIO\n INDEX 01 00:00:00\n"
                .to_vec(),
        ),
        ("cue-utf16-odd.cue", b"\xff\xfeF\0I\0L\0E\0 ".to_vec()),
        ("cue-unclosed.cue", b"FILE \"a.flac WAVE\nTRACK".to_vec()),
        ("cue-many-tracks.cue", {
            let mut s = String::from("FILE \"a.flac\" WAVE\n");
            for i in 0..3000 {
                s.push_str(&format!(" TRACK {i:02} AUDIO\n INDEX 01 00:00:{:02}\n", i % 75));
            }
            s.into_bytes()
        }),
    ]
}

const LRC: &str = "[ti:Tidewater]\n[ar:Mari Kask]\n[offset:+250]\n[00:00.00]first <00:00.05>word\n\
[00:01.50][00:03.00]twice\n";

/// Lyrics that are wrong in their own ways.
fn lrc_lies() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "lrc-huge-times.lrc",
            b"[99999999999:59.999]far\n[-5:00.00]before\n[00:00.00]<99999999:00.00>word\n[offset:-9223372036854775808]\n"
                .to_vec(),
        ),
        ("lrc-brackets.lrc", {
            let mut b = "[".repeat(10_000).into_bytes();
            b.extend_from_slice(b"00:01.00]x\n");
            b
        }),
        ("lrc-many-times.lrc", {
            let mut s = String::new();
            for i in 0..5000 {
                s.push_str(&format!("[{:02}:{:02}.00]", i / 60 % 100, i % 60));
            }
            s.push_str("line\n");
            s.into_bytes()
        }),
        ("lrc-binary.lrc", b"\0\0\xff\xfe[\0\x000\0".to_vec()),
    ]
}

const M3U: &str = "#EXTM3U\n#EXTINF:12,Mari Kask - One\n/music/one.flac\n#EXTINF:-1,Two\n\
two.flac\nfile:///music/three%20x.flac\n";
const PLS: &str = "[playlist]\nFile1=/music/one.flac\nTitle1=One\nLength1=12\n\
File2=two.flac\nNumberOfEntries=2\nVersion=2\n";
const XSPF: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<playlist version=\"1\" \
xmlns=\"http://xspf.org/ns/0/\"><trackList><track><location>file:///music/one.flac</location>\
<title>One</title><duration>12000</duration></track><track><location>two.flac</location>\
</track></trackList></playlist>\n";

/// Playlists that are wrong in their own ways.
fn playlist_lies() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "xspf-entities.xspf",
            b"<?xml version=\"1.0\"?>\n<!DOCTYPE lolz [<!ENTITY lol \"lol\">\
              <!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">\
              <!ENTITY lol3 \"&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;\">\
              <!ENTITY ext SYSTEM \"file:///etc/passwd\">]>\
              <playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\"><trackList><track>\
              <location>&lol3;&ext;</location></track></trackList></playlist>"
                .to_vec(),
        ),
        ("xspf-deep.xspf", {
            let mut b = b"<playlist><trackList>".to_vec();
            b.extend("<track>".repeat(20_000).into_bytes());
            b
        }),
        (
            "pls-counts.pls",
            b"[playlist]\nNumberOfEntries=4294967295\nFile4294967295=x.flac\nFile0=y.flac\n\
              File99999999999999999999=z.flac\nLength1=-99999999999999999999\n"
                .to_vec(),
        ),
        ("m3u-long-line.m3u", {
            let mut b = b"#EXTM3U\n#EXTINF:99999999999999999999,".to_vec();
            b.extend(vec![b'x'; 200_000]);
            b.extend_from_slice(b"\n../../../../../../etc/passwd\n");
            b
        }),
        ("m3u8-utf16-odd.m3u8", b"\xff\xfe#\0E\0X\0T\0M\0".to_vec()),
        ("m3u-nul.m3u", b"/music/a\0b.flac\n\0\0\0\n".to_vec()),
    ]
}

/// Writes the corpus and returns its files.
pub(crate) fn generate() -> Vec<Sample> {
    let dir = fresh_dir();
    let mut noise = Noise(0x5EED_CAFE_F00D_0001);
    let mut samples = Vec::new();
    let mut add = |name: &str, bytes: &[u8], kind: Kind| {
        let path = dir.join(name);
        fs::write(&path, bytes).expect("write a corpus file");
        samples.push(Sample { path, kind });
    };

    let valid = dir.join("valid");
    fs::create_dir_all(&valid).expect("create the folder of valid files");
    let mut bases: Vec<(String, Vec<u8>)> = Vec::new();
    for (format, rate, name) in [
        (EncodeFormat::Flac, 44_100, "flac.flac"),
        (EncodeFormat::Wav, 44_100, "wav.wav"),
        (EncodeFormat::Mp3, 44_100, "mp3.mp3"),
        (EncodeFormat::Vorbis, 44_100, "vorbis.ogg"),
        (EncodeFormat::Opus, 48_000, "opus.opus"),
        (EncodeFormat::Aac, 44_100, "aac.m4a"),
        (EncodeFormat::Alac, 44_100, "alac.m4a"),
    ] {
        let path = valid.join(name);
        if encode(&path, format, rate) {
            tag(&path);
            bases.push((name.to_owned(), fs::read(&path).expect("read a valid file")));
        }
    }
    bases.push(("dsf.dsf".to_owned(), dsf(2, 4096)));
    bases.push(("dff.dff".to_owned(), dff(2048)));
    bases.push((
        "id3.mp3".to_owned(),
        mp3_frames(8, &id3_text(b"TIT2", "Title", None)),
    ));
    for (name, bytes) in &bases {
        add(name, bytes, Kind::Media);
        for (damage, copy) in damage(bytes, &mut noise) {
            add(&format!("{damage}.{name}"), &copy, Kind::Media);
        }
    }
    // Tags whose sizes lie.
    add(
        "id3-frame-size.mp3",
        &mp3_frames(4, &id3_text(b"TIT2", "Title", Some(u32::MAX))),
        Kind::Media,
    );
    add(
        "id3-frame-zero.mp3",
        &mp3_frames(4, &id3_text(b"APIC", "", Some(0))),
        Kind::Media,
    );
    let mut id3_huge = mp3_frames(4, &[]);
    id3_huge[6..10].copy_from_slice(&[0x7f, 0x7f, 0x7f, 0x7f]);
    add("id3-tag-size.mp3", &id3_huge, Kind::Media);
    for (name, bytes) in mp4_lies().into_iter().chain(ogg_lies()).chain(fuzz_finds()) {
        add(name, &bytes, Kind::Media);
    }
    let mut dsf_lie = dsf(2, 4096);
    dsf_lie[56..60].copy_from_slice(&0u32.to_le_bytes()); // a zero sample rate
    add("dsf-zero-rate.dsf", &dsf_lie, Kind::Media);
    let mut dsf_lie = dsf(2, 4096);
    dsf_lie[52..56].copy_from_slice(&u32::MAX.to_le_bytes()); // channels
    add("dsf-channels.dsf", &dsf_lie, Kind::Media);

    for (base, text, kind, lies) in [
        ("sheet.cue", CUE, Kind::Cue, cue_lies()),
        ("lyrics.lrc", LRC, Kind::Lyrics, lrc_lies()),
        ("list.m3u", M3U, Kind::Playlist, playlist_lies()),
        ("list.pls", PLS, Kind::Playlist, Vec::new()),
        ("list.xspf", XSPF, Kind::Playlist, Vec::new()),
    ] {
        add(base, text.as_bytes(), kind);
        for (damage, copy) in damage(text.as_bytes(), &mut noise) {
            add(&format!("{damage}.{base}"), &copy, kind);
        }
        for (name, bytes) in lies {
            add(name, &bytes, kind);
        }
    }
    samples
}
