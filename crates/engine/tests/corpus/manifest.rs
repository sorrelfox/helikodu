// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The generated corpus: its folder, its manifest and the recipes it was made from.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// What `ffprobe` reported when the file was made (see `tests/corpus/README.md`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Expect {
    pub(crate) codec: Option<String>,
    pub(crate) sample_rate: Option<u32>,
    pub(crate) channels: Option<u16>,
    /// Decoded frames of the first audio stream.
    pub(crate) samples: Option<u64>,
    /// The decoded length equals the source's (or the part's).
    #[serde(default)]
    pub(crate) exact_length: bool,
    pub(crate) audio_streams: Option<u32>,
    pub(crate) video_streams: Option<u32>,
}

/// One file of the manifest.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Entry {
    /// Relative to the corpus folder, percent-encoded.
    pub(crate) path: String,
    pub(crate) set: String,
    pub(crate) format: Option<String>,
    #[serde(default)]
    pub(crate) expect: Option<Expect>,
}

impl Entry {
    pub(crate) fn expect(&self) -> Expect {
        self.expect.clone().unwrap_or_default()
    }

    /// The raw bytes of the relative path.
    pub(crate) fn path_bytes(&self) -> Vec<u8> {
        decode_name(&self.path)
    }

    /// The path for display: valid UTF-8 as is, other bytes escaped.
    pub(crate) fn display(&self) -> String {
        self.path.replace('\n', "\\n")
    }
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(rename = "file", default)]
    files: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Recipes {
    #[serde(default)]
    source: Vec<SourceRecipe>,
    #[serde(default)]
    sample: Vec<SampleRecipe>,
}

#[derive(Debug, Deserialize)]
struct SourceRecipe {
    name: String,
    sample_rate: u32,
    frames: u64,
}

#[derive(Debug, Deserialize)]
struct SampleRecipe {
    set: String,
    name: String,
    source: Option<String>,
    from: Option<String>,
    #[serde(default)]
    args: Vec<String>,
}

/// The corpus on disk.
#[derive(Debug)]
pub(crate) struct Corpus {
    pub(crate) dir: PathBuf,
    pub(crate) files: Vec<Entry>,
    /// The length in seconds of the signal each file was made from, by the raw
    /// bytes of its path.
    durations: HashMap<Vec<u8>, f64>,
}

impl Corpus {
    /// The corpus, or `None` after printing why the calling test is skipped.
    pub(crate) fn load(test: &str) -> Option<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = std::env::var_os("HELIKODU_CORPUS")
            .map_or_else(|| root.join("target/corpus"), PathBuf::from);
        let Ok(text) = fs::read_to_string(dir.join("manifest.toml")) else {
            eprintln!(
                "SKIPPED: {test}: no corpus at {} (make it with `cargo xtask corpus`)",
                dir.display()
            );
            return None;
        };
        let manifest: ManifestFile = toml::from_str(&text).expect("the corpus manifest parses");
        let recipes = fs::read_to_string(root.join("tests/corpus/recipes.toml"))
            .ok()
            .and_then(|text| toml::from_str::<Recipes>(&text).ok());
        let durations = recipes.map(|r| durations(&r)).unwrap_or_default();
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        Some(Self {
            dir,
            files: manifest.files,
            durations,
        })
    }

    /// The files of one set, in manifest order.
    pub(crate) fn set(&self, set: &str) -> Vec<Entry> {
        self.files
            .iter()
            .filter(|f| f.set == set)
            .cloned()
            .collect()
    }

    /// The absolute path of a file, with its raw bytes.
    pub(crate) fn path(&self, entry: &Entry) -> PathBuf {
        self.dir.join(OsStr::from_bytes(&entry.path_bytes()))
    }

    /// The length in seconds of the signal `entry` was made from, if the recipes
    /// say.
    pub(crate) fn source_seconds(&self, entry: &Entry) -> Option<f64> {
        self.durations.get(&entry.path_bytes()).copied()
    }
}

/// The source length of every sample that is not made in parts.
fn durations(recipes: &Recipes) -> HashMap<Vec<u8>, f64> {
    let sources: HashMap<&str, f64> = recipes
        .source
        .iter()
        .map(|s| (s.name.as_str(), s.frames as f64 / f64::from(s.sample_rate)))
        .collect();
    let mut out: HashMap<Vec<u8>, f64> = HashMap::new();
    // `from` refers to an earlier sample, so one pass in file order resolves all.
    for sample in &recipes.sample {
        let path = decode_name(&format!("{}/{}", sample.set, sample.name));
        let seconds = match (&sample.source, &sample.from) {
            (Some(source), _) => sources.get(source.as_str()).copied(),
            (None, Some(from)) => out.get(&decode_name(from)).copied(),
            (None, None) => None,
        };
        // `-t <seconds>` in the tool's arguments shortens the output.
        let limit = sample
            .args
            .windows(2)
            .find(|w| w[0] == "-t")
            .and_then(|w| w[1].parse::<f64>().ok());
        if let Some(seconds) = seconds {
            out.insert(path, limit.map_or(seconds, |t| seconds.min(t)));
        }
    }
    out
}

/// Decodes a percent-encoded name: `%XX` is the byte `0xXX`, any other character
/// its UTF-8 bytes.
pub(crate) fn decode_name(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = name
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Whether a codec (FFmpeg's name) is lossless: its decoded samples are exactly
/// the encoded ones.
pub(crate) fn is_lossless(codec: &str) -> bool {
    let pcm = codec.starts_with("pcm_") && !matches!(codec, "pcm_alaw" | "pcm_mulaw" | "pcm_vidc");
    pcm || matches!(
        codec,
        "flac" | "alac" | "wavpack" | "tta" | "mlp" | "truehd" | "ape" | "tak"
    )
}

#[test]
fn names_decode_to_their_bytes() {
    assert_eq!(decode_name("caf%E9.wav"), b"caf\xe9.wav");
    assert_eq!(decode_name("a%25b"), b"a%b");
    assert_eq!(decode_name("Bj\u{f6}rk"), "Bj\u{f6}rk".as_bytes());
}
