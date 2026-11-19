// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The recipe file, `tests/corpus/recipes.toml`.
//!
//! See `tests/corpus/README.md` for the format.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::names;

/// The tools `xtask` implements itself.
pub(crate) const BUILTIN_TOOLS: &[&str] = &[
    "copy", "truncate", "garbage", "empty", "write", "tag", "text",
];

/// The sets a recipe may belong to; each is a folder in the corpus.
pub(crate) const SETS: &[&str] = &[
    "formats", "gapless", "video", "broken", "names", "longtail", "tags", "cue", "scripts",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeFile {
    #[serde(default)]
    source: Vec<Source>,
    #[serde(default)]
    sample: Vec<Sample>,
}

/// A signal written by `xtask` itself as a WAV file in `sources/`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub(crate) name: String,
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) encoding: Encoding,
    /// Length in sample frames (samples per channel).
    pub(crate) frames: u64,
    pub(crate) signal: Signal,
    #[serde(default = "default_seed")]
    pub(crate) seed: u64,
    /// Frame positions at which the source is also written as separate parts.
    #[serde(default)]
    pub(crate) split: Vec<u64>,
}

fn default_seed() -> u64 {
    1
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Encoding {
    S16,
    S24,
    S32,
    F32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Signal {
    SweepNoise,
    Silence,
}

/// One corpus file as written in the recipe file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Sample {
    pub(crate) set: String,
    /// The path inside the set's folder, percent-encoded. With `parts = true`
    /// it contains `{part}`.
    pub(crate) name: String,
    /// For the formats set: the codec and container this sample stands for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<String>,
    /// The source signal the tool reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
    /// An earlier sample (`set/name`) the tool reads instead of a source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) from: Option<String>,
    /// Make one sample per part of a split source.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) parts: bool,
    pub(crate) tool: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) args: Vec<String>,
    /// FFmpeg features the recipe needs: `encoder:<name>` or `muxer:<name>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) needs: Vec<String>,
    /// The tool may fail on a build that lacks a feature `needs` cannot
    /// detect; the recipe is then skipped, not failed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) optional: bool,
    /// Extensions of further files the tool writes next to the output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) extra_outputs: Vec<String>,
    /// `truncate`: the fraction of the input to keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) keep: Option<f64>,
    /// `garbage`: the number of leading bytes to keep from the input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) keep_bytes: Option<u64>,
    /// `garbage`: the seed of the replacement bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seed: Option<u64>,
    /// `write`: the file the project writes itself (`tracker-module`, `nsf`,
    /// `psid`, `midi`, `soundfont`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<String>,
    /// `tag`: the tags written into the input, in order of the list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tag: Vec<TagBlock>,
    /// `text`: the text written, such as a CUE sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    /// `text`: the charset of the written text (see [`super::charset`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) charset: Option<String>,
    /// Pad the file name to exactly this many bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pad_name_to: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expect: Option<Expect>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One tag that the built-in step `tag` writes into a file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TagBlock {
    /// `vorbis`, `id3v1`, `id3v23`, `id3v24`, `ape`, `mp4` or `riff`.
    pub(crate) format: String,
    /// The charset of the text in ID3v1, ID3v2 Latin-1 frames, APEv2 and RIFF INFO;
    /// a WHATWG label or `latin1`. Unset: each format's standard encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) charset: Option<String>,
    /// Each field is the key followed by its values. Several values become
    /// one field per value (Vorbis comments) or one NUL-separated field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) fields: Vec<Vec<String>>,
    /// Embedded pictures: square PNG images in one colour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) pictures: Vec<PictureSpec>,
}

/// An embedded picture to write.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PictureSpec {
    /// The ID3v2 picture type: 3 front cover, 4 back cover, 8 artist, …
    #[serde(rename = "type")]
    pub(crate) kind: u8,
    /// Width and height in pixels.
    pub(crate) size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
}

/// One value or a list of values of an expected tag field.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum Values {
    One(String),
    Many(Vec<String>),
}

/// An embedded picture a reader must find.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PictureExpect {
    /// The picture type; unset for formats without types (MP4).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<u8>,
    pub(crate) size: u32,
}

/// The embedded lyrics a reader must find.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LyricsExpect {
    /// Whether every line has a time (LRC).
    pub(crate) synced: bool,
    pub(crate) lines: u32,
    pub(crate) first_line: String,
}

/// One track a CUE sheet must give.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CueExpect {
    pub(crate) number: u32,
    /// The audio file, relative to the sheet's folder, percent-encoded.
    pub(crate) file: String,
    /// Start in the file, as CUE time `MM:SS:FF` (75 frames a second).
    pub(crate) start: String,
    /// End in the file; unset for the end of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) end: Option<String>,
    /// Where the pregap (`INDEX 00`) starts in the same file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pregap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artist: Option<String>,
}

/// What `ffprobe` must report for a sample. Unset fields are not checked,
/// except that `sample_rate` and `channels` default to the source's values.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Expect {
    /// The demuxer name, as in `ffprobe`'s `format_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) demuxer: Option<String>,
    /// The codec of the first audio stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) channels: Option<u16>,
    /// The number of decoded sample frames of the first audio stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) samples: Option<u64>,
    /// The decoded length equals the source's (or the part's) length.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) exact_length: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) audio_streams: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) video_streams: Option<u32>,
    /// Tag fields a reader must find, by the name of the field in the tag
    /// record (`title`, `artists`, `rating`, `track_gain`, …); see
    /// `tests/corpus/README.md`. Not checked with `ffprobe`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) tags: BTreeMap<String, Values>,
    /// The legacy charset a reader must be told for the tags or the sheet to
    /// read right; unset when it is to be recognised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) charset: Option<String>,
    /// The embedded pictures, in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) pictures: Vec<PictureExpect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lyrics: Option<LyricsExpect>,
    /// The tracks of a CUE sheet (a `.cue` file or the sheet embedded in the file).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) cue: Vec<CueExpect>,
    /// The file's position (from 1) among the files of its folder, sorted by
    /// artist in each locale.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) sort: BTreeMap<String, u32>,
}

impl Expect {
    /// Whether anything is to be checked with `ffprobe`.
    pub(crate) fn needs_probe(&self) -> bool {
        self.demuxer.is_some()
            || self.codec.is_some()
            || self.sample_rate.is_some()
            || self.channels.is_some()
            || self.samples.is_some()
            || self.audio_streams.is_some()
            || self.video_streams.is_some()
    }
}

/// A sample ready to make: parts expanded, names decoded, expectations
/// completed.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Job {
    /// The output path relative to the corpus folder, percent-encoded.
    pub(crate) path: String,
    #[serde(skip)]
    pub(crate) path_bytes: Vec<u8>,
    /// The input file relative to the corpus folder, percent-encoded.
    pub(crate) input: Option<String>,
    pub(crate) recipe: Sample,
    pub(crate) expect: Option<Expect>,
}

/// The recipes, checked and expanded.
#[derive(Debug)]
pub(crate) struct Recipes {
    pub(crate) sources: Vec<Source>,
    pub(crate) jobs: Vec<Job>,
}

pub(crate) fn load(path: &Path) -> Result<Recipes> {
    let text =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    let file: RecipeFile =
        toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))?;

    let mut sources = BTreeMap::new();
    for source in &file.source {
        check_source(source)?;
        if sources.insert(source.name.clone(), source).is_some() {
            bail!("source `{}` is defined twice", source.name);
        }
    }

    let mut jobs: Vec<Job> = Vec::new();
    let mut seen = BTreeSet::new();
    for sample in &file.sample {
        let context = format!("sample `{}/{}`", sample.set, sample.name);
        let expanded = expand(sample, &sources).with_context(|| context.clone())?;
        for job in expanded {
            if let Some(from) = &sample.from
                && !job.input.as_ref().is_some_and(|input| seen.contains(input))
            {
                bail!("{context}: `from = \"{from}\"` must name an earlier sample");
            }
            if !seen.insert(job.path.clone()) {
                bail!("{context}: `{}` is made twice", job.path);
            }
            for extension in &sample.extra_outputs {
                let extra = with_extension(&job.path, extension);
                if !seen.insert(extra.clone()) {
                    bail!("{context}: `{extra}` is made twice");
                }
            }
            jobs.push(job);
        }
    }

    Ok(Recipes {
        sources: file.source,
        jobs,
    })
}

fn check_source(source: &Source) -> Result<()> {
    if source.name.is_empty()
        || !source
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        bail!(
            "source name `{}` must be ASCII letters, digits and dashes",
            source.name
        );
    }
    if source.channels == 0 || source.sample_rate == 0 || source.frames == 0 {
        bail!("source `{}` is empty", source.name);
    }
    let mut previous = 0;
    for &point in &source.split {
        if point <= previous || point >= source.frames {
            bail!(
                "source `{}`: split points must be increasing and inside the source",
                source.name
            );
        }
        previous = point;
    }
    Ok(())
}

fn expand(sample: &Sample, sources: &BTreeMap<String, &Source>) -> Result<Vec<Job>> {
    if !SETS.contains(&sample.set.as_str()) {
        bail!("unknown set `{}` (known: {})", sample.set, SETS.join(", "));
    }
    if sample.name.is_empty() || sample.name.starts_with('/') || sample.name.contains("..") {
        bail!("the name must be a relative path inside the set's folder");
    }
    if (sample.set == "formats") != sample.format.is_some() {
        bail!("`format` is required in the formats set and only there");
    }
    if sample.source.is_some() && sample.from.is_some() {
        bail!("`source` and `from` exclude each other");
    }
    let builtin = BUILTIN_TOOLS.contains(&sample.tool.as_str());
    if !builtin && sample.source.is_none() && sample.tool != "ffmpeg" {
        bail!("an external tool other than ffmpeg needs a `source`");
    }
    if (sample.tool == "tag") != !sample.tag.is_empty() {
        bail!("`tag` tables belong to the built-in step `tag`, which needs at least one");
    }
    if sample.tool == "tag" && sample.from.is_none() {
        bail!("`tag` needs `from`: the file it writes the tags into");
    }
    if (sample.tool == "text") != sample.text.is_some() {
        bail!("`text` belongs to the built-in step `text`, which needs it");
    }
    if sample.charset.is_some() && sample.tool != "text" {
        bail!("`charset` belongs to the built-in step `text`");
    }
    for need in &sample.needs {
        if !(need.starts_with("encoder:") || need.starts_with("muxer:")) {
            bail!("`{need}`: a need is `encoder:<name>` or `muxer:<name>`");
        }
    }

    let source = match &sample.source {
        Some(name) => Some(
            *sources
                .get(name)
                .with_context(|| format!("unknown source `{name}`"))?,
        ),
        None => None,
    };

    let mut expect = sample.expect.clone();
    if let (Some(expect), Some(source)) = (expect.as_mut(), source)
        && expect.audio_streams != Some(0)
    {
        expect.sample_rate.get_or_insert(source.sample_rate);
        expect.channels.get_or_insert(source.channels);
    }

    // (name, input file, length in frames)
    let mut variants: Vec<(String, Option<String>, Option<u64>)> = Vec::new();
    match (source, sample.parts) {
        (Some(source), true) => {
            if source.split.is_empty() {
                bail!("`parts = true` needs a source with `split` points");
            }
            if !sample.name.contains("{part}") {
                bail!("with `parts = true` the name must contain `{{part}}`");
            }
            for (index, length) in part_lengths(source).into_iter().enumerate() {
                let part = index + 1;
                variants.push((
                    sample.name.replace("{part}", &format!("part{part}")),
                    Some(format!("sources/{}.part{part}.wav", source.name)),
                    Some(length),
                ));
            }
        }
        (None, true) => bail!("`parts = true` needs a `source`"),
        (Some(source), false) => variants.push((
            sample.name.clone(),
            Some(format!("sources/{}.wav", source.name)),
            Some(source.frames),
        )),
        (None, false) => {
            let from = match &sample.from {
                Some(from) => Some(names::encode(&names::decode(from)?)),
                None => None,
            };
            variants.push((sample.name.clone(), from, None));
        }
    }

    let mut jobs = Vec::new();
    for (name, input, length) in variants {
        let mut path_bytes = format!("{}/", sample.set).into_bytes();
        path_bytes.extend(names::decode(&name)?);
        if let Some(length) = sample.pad_name_to {
            path_bytes = names::pad(&path_bytes, length)?;
        }
        if path_bytes.contains(&0) {
            bail!("a name cannot contain a NUL byte");
        }
        let mut expect = expect.clone();
        if let Some(expect) = expect.as_mut()
            && expect.exact_length
        {
            let Some(length) = length else {
                bail!("`exact_length` needs a `source`");
            };
            expect.samples = Some(length);
        }
        jobs.push(Job {
            path: names::encode(&path_bytes),
            path_bytes,
            input,
            recipe: sample.clone(),
            expect,
        });
    }
    Ok(jobs)
}

/// The lengths of the parts of a split source.
pub(crate) fn part_lengths(source: &Source) -> Vec<u64> {
    let mut lengths = Vec::new();
    let mut start = 0;
    for &point in source.split.iter().chain(std::iter::once(&source.frames)) {
        lengths.push(point - start);
        start = point;
    }
    lengths
}

/// `path` with its extension replaced by `extension`.
pub(crate) fn with_extension(path: &str, extension: &str) -> String {
    let file_start = path.rfind('/').map_or(0, |slash| slash + 1);
    match path[file_start..].rfind('.') {
        Some(dot) => format!("{}.{extension}", &path[..file_start + dot]),
        None => format!("{path}.{extension}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recipe_file_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/recipes.toml");
        let recipes = load(&path).unwrap();
        assert!(!recipes.jobs.is_empty());
    }

    #[test]
    fn parts_have_the_split_lengths() {
        let source = Source {
            name: String::from("test"),
            sample_rate: 44100,
            channels: 2,
            encoding: Encoding::S16,
            frames: 100,
            signal: Signal::Silence,
            seed: 1,
            split: vec![10, 55],
        };
        assert_eq!(part_lengths(&source), vec![10, 45, 45]);
    }

    #[test]
    fn replaces_the_extension() {
        assert_eq!(
            with_extension("formats/a.b/c.wv", "wvc"),
            "formats/a.b/c.wvc"
        );
        assert_eq!(with_extension("formats/c", "wvc"), "formats/c.wvc");
    }
}
