// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The generated corpus: its folder and the expectations of its manifest.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One value or a list of values of an expected tag field.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum Values {
    One(String),
    Many(Vec<String>),
}

impl Values {
    pub(crate) fn list(&self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value.clone()],
            Self::Many(values) => values.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PictureExpect {
    #[serde(rename = "type")]
    pub(crate) kind: Option<u8>,
    pub(crate) size: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LyricsExpect {
    pub(crate) synced: bool,
    pub(crate) lines: usize,
    pub(crate) first_line: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CueExpect {
    pub(crate) number: u32,
    /// Relative to the sheet's folder, percent-encoded.
    pub(crate) file: String,
    pub(crate) start: String,
    pub(crate) end: Option<String>,
    pub(crate) pregap: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) artist: Option<String>,
}

/// What the manifest says a reader must find (see `tests/corpus/README.md`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Expect {
    #[serde(default)]
    pub(crate) tags: BTreeMap<String, Values>,
    pub(crate) charset: Option<String>,
    #[serde(default)]
    pub(crate) pictures: Vec<PictureExpect>,
    pub(crate) lyrics: Option<LyricsExpect>,
    #[serde(default)]
    pub(crate) cue: Vec<CueExpect>,
}

/// One file of the manifest.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Entry {
    /// Relative to the corpus folder, percent-encoded.
    pub(crate) path: String,
    pub(crate) set: String,
    #[serde(default)]
    pub(crate) expect: Option<Expect>,
}

impl Entry {
    pub(crate) fn expect(&self) -> Expect {
        self.expect.clone().unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(rename = "file", default)]
    files: Vec<Entry>,
}

/// The corpus on disk.
#[derive(Debug)]
pub(crate) struct Corpus {
    pub(crate) dir: PathBuf,
    pub(crate) files: Vec<Entry>,
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
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        Some(Self {
            dir,
            files: manifest.files,
        })
    }

    /// The files of `sets` that `wanted` selects, or `None` after printing why
    /// the test is skipped (a corpus made before these sets existed).
    pub(crate) fn select(
        &self,
        test: &str,
        sets: &[&str],
        wanted: impl Fn(&Expect) -> bool,
    ) -> Option<Vec<Entry>> {
        let files: Vec<Entry> = self
            .files
            .iter()
            .filter(|f| sets.contains(&f.set.as_str()) && wanted(&f.expect()))
            .cloned()
            .collect();
        if files.is_empty() {
            eprintln!(
                "SKIPPED: {test}: the corpus has no {} files; run `cargo xtask corpus`",
                sets.join(" or ")
            );
            return None;
        }
        Some(files)
    }

    /// The absolute path of a file, with its raw bytes.
    pub(crate) fn path(&self, entry: &Entry) -> PathBuf {
        self.dir.join(OsStr::from_bytes(&decode_name(&entry.path)))
    }
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

/// Fails the test with every problem found, one line each.
pub(crate) fn assert_no_problems(test: &str, checked: usize, problems: &[String]) {
    eprintln!(
        "{test}: {checked} files checked, {} problems",
        problems.len()
    );
    assert!(
        problems.is_empty(),
        "{test}: {} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
