// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The manifest, `target/corpus/manifest.toml`: every file with its SHA-256
//! checksum, the tool versions and the recipes it was made from.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::recipe::Expect;

pub(crate) const FILE_NAME: &str = "manifest.toml";

const HEADER: &str = "\
# The test corpus: written by `cargo xtask corpus`, checked by
# `cargo xtask corpus --check`. Paths are percent-encoded (see
# tests/corpus/README.md).
";

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    /// SHA-256 of the recipe file the corpus was made from.
    pub(crate) recipes_sha256: String,
    /// The number of distinct formats in the formats set.
    pub(crate) formats: usize,
    /// Program name to version line, for the programs used.
    pub(crate) tools: BTreeMap<String, String>,
    #[serde(default, rename = "file")]
    pub(crate) files: Vec<Entry>,
    #[serde(default, rename = "skipped")]
    pub(crate) skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    /// Relative to the corpus folder, percent-encoded.
    pub(crate) path: String,
    pub(crate) set: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<String>,
    pub(crate) sha256: String,
    pub(crate) size: u64,
    /// The program that made the file, or the built-in step.
    pub(crate) tool: String,
    /// SHA-256 over the recipe, the tool version and the input's checksum.
    /// The file is up to date while this does not change.
    pub(crate) fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expect: Option<Expect>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Skipped {
    pub(crate) path: String,
    pub(crate) reason: String,
}

impl Manifest {
    /// Reads the manifest; a missing or unreadable one is empty.
    pub(crate) fn load_or_default(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        let body = toml::to_string(self).context("could not write the manifest")?;
        fs::write(path, format!("{HEADER}\n{body}"))
            .with_context(|| format!("could not write {}", path.display()))
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    Ok(sha256(&bytes))
}
