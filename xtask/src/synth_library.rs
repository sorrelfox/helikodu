// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask synth-library`: a synthetic music library for benchmarks and
//! UI performance tests.
//!
//! The library is a folder tree (artist / album / disc) of one-second silent
//! FLAC or Opus files with made-up but realistic tags; see [`catalogue`] for
//! what it contains. One silent template file is encoded per run; every track
//! is a copy of it with its own Vorbis comment block, written here directly,
//! so no encoder or tagger runs per file.
//!
//! Next to the files are `synth-library.json` (the parameters) and
//! `tracks.jsonl` (one JSON record per track: its path, percent-encoded as in
//! the test corpus, and its tags). `--names-only` writes only those two, for
//! tests that fill the database without scanning files.
//!
//! A run with the same parameters, template and generator version keeps every
//! file that is already there with the expected size; a run with different
//! parameters into the same folder removes the old library first.

mod catalogue;
mod flac;
mod ogg;
mod paths;
mod random;
mod template;
mod vorbis;
mod words;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufWriter, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::corpus::names;
use catalogue::{Album, Catalogue, Kind, Track};
use flac::FlacTemplate;
use ogg::OpusTemplate;

const USAGE: &str = "\
usage: cargo xtask synth-library --tracks N [options]

  --tracks N        number of tracks; `k` and `m` suffixes allowed (100k, 0.5m)
  --format F        flac (default) or opus
  --out DIR         output folder
                    (default: target/synth-library/<format>-<tracks>-seed<seed>)
  --seed S          random seed (default: 1)
  --names-only      write only the track manifest, no audio files
  --verify          check up to 200 evenly spread files with ffprobe afterwards
";

/// Changes whenever the generated library changes for the same parameters,
/// so that a library made by an older `xtask` is made again.
const GENERATOR: &str = "xtask synth-library 2";
/// The parameter file in the library folder.
const HEADER_FILE: &str = "synth-library.json";
/// The track manifest in the library folder.
const MANIFEST_FILE: &str = "tracks.jsonl";
/// How many files `--verify` checks.
const VERIFY_SAMPLE: usize = 200;
/// Every track is one second long.
const DURATION_MS: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Format {
    Flac,
    Opus,
}

impl Format {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Format::Flac => "flac",
            Format::Opus => "opus",
        }
    }
}

#[derive(Debug, Clone)]
struct Options {
    tracks: usize,
    format: Format,
    out: Option<PathBuf>,
    seed: u64,
    names_only: bool,
    verify: bool,
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    let options = parse_args(args)?;
    let Some(options) = options else {
        print!("{USAGE}");
        return Ok(());
    };
    let out = match &options.out {
        Some(out) => out.clone(),
        None => crate::repository_root()?
            .join("target/synth-library")
            .join(default_folder(&options)),
    };
    let summary = generate(&options, &out)?;
    summary.print(&out);
    if options.verify && !options.names_only {
        let checked = verify(&out, &summary.sample)?;
        println!("synth-library: {checked} files checked with ffprobe");
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<Option<Options>> {
    let mut tracks = None;
    let mut format = Format::Flac;
    let mut out = None;
    let mut seed = 1;
    let mut names_only = false;
    let mut verify = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .with_context(|| format!("`{name}` needs a value\n\n{USAGE}"))
        };
        match arg.as_str() {
            "--tracks" => tracks = Some(parse_count(value("--tracks")?)?),
            "--format" => {
                format = match value("--format")?.as_str() {
                    "flac" => Format::Flac,
                    "opus" => Format::Opus,
                    other => bail!("unknown format `{other}`: use flac or opus"),
                }
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--seed" => {
                let text = value("--seed")?;
                seed = text
                    .parse()
                    .with_context(|| format!("`{text}` is not a seed (a whole number)"))?;
            }
            "--names-only" => names_only = true,
            "--verify" => verify = true,
            "--help" | "-h" => return Ok(None),
            other => bail!("unknown option `{other}`\n\n{USAGE}"),
        }
    }
    let Some(tracks) = tracks else {
        bail!("`--tracks` is required\n\n{USAGE}");
    };
    if tracks == 0 {
        bail!("`--tracks` must be at least 1");
    }
    Ok(Some(Options {
        tracks,
        format,
        out,
        seed,
        names_only,
        verify,
    }))
}

/// "5000", "100k", "0.5m".
fn parse_count(text: &str) -> Result<usize> {
    let lower = text.to_ascii_lowercase();
    let (number, factor) = if let Some(number) = lower.strip_suffix('k') {
        (number, 1_000.0)
    } else if let Some(number) = lower.strip_suffix('m') {
        (number, 1_000_000.0)
    } else {
        (lower.as_str(), 1.0)
    };
    let value: f64 = number
        .parse()
        .with_context(|| format!("`{text}` is not a track count"))?;
    if !value.is_finite() || value < 0.0 || value * factor > 1e8 {
        bail!("`{text}` is not a usable track count");
    }
    Ok((value * factor).round() as usize)
}

fn default_folder(options: &Options) -> String {
    format!(
        "{}-{}-seed{}{}",
        options.format.extension(),
        options.tracks,
        options.seed,
        if options.names_only { "-names" } else { "" }
    )
}

/// The parameters a library was made with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    generator: String,
    seed: u64,
    tracks: usize,
    format: Format,
    names_only: bool,
    /// SHA-256 of the template file, which depends on the encoder version.
    template_sha256: Option<String>,
    template_encoder: Option<String>,
    duration_ms: u32,
    /// Whether the run that wrote this finished.
    complete: bool,
}

/// One line of `tracks.jsonl`.
#[derive(Debug, Serialize)]
struct Record<'a> {
    /// Relative to the library folder; percent-encoded where not UTF-8.
    path: String,
    duration_ms: u32,
    /// Key to values; several values are a multi-value tag.
    tags: BTreeMap<&'a str, Vec<&'a str>>,
}

#[derive(Debug, Default)]
struct Counters {
    written: AtomicU64,
    kept: AtomicU64,
    bytes: AtomicU64,
    disk_bytes: AtomicU64,
}

#[derive(Debug, Default)]
struct Summary {
    tracks: usize,
    albums: usize,
    compilations: usize,
    classical: usize,
    multi_disc: usize,
    legacy_names: usize,
    untagged: usize,
    written: u64,
    kept: u64,
    bytes: u64,
    disk_bytes: u64,
    names_only: bool,
    elapsed: Duration,
    /// Tracks kept for `--verify`.
    sample: Vec<Track>,
}

impl Summary {
    fn print(&self, out: &Path) {
        println!(
            "synth-library: {} tracks in {} albums in {}",
            self.tracks,
            self.albums,
            out.display()
        );
        println!(
            "synth-library: {} compilations, {} classical, {} multi-disc; {} tracks with non-UTF-8 names, {} untagged",
            self.compilations, self.classical, self.multi_disc, self.legacy_names, self.untagged
        );
        if self.names_only {
            println!("synth-library: manifest only (--names-only), no audio files");
        } else {
            println!(
                "synth-library: {} files written, {} kept; {} on disk ({} of data)",
                self.written,
                self.kept,
                human(self.disk_bytes),
                human(self.bytes)
            );
        }
        println!("synth-library: done in {:.1} s", self.elapsed.as_secs_f64());
    }
}

fn human(bytes: u64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.2} GiB", mib / 1024.0)
    } else {
        format!("{mib:.1} MiB")
    }
}

/// Makes a template file into a writer of complete files.
#[derive(Debug)]
enum Writer {
    Flac(FlacTemplate),
    Opus(OpusTemplate),
}

impl Writer {
    fn file(&self, tags: &[(&str, String)]) -> Result<Vec<u8>> {
        let comment = vorbis::comment(vorbis::VENDOR, tags)?;
        match self {
            Writer::Flac(template) => template.file(&comment),
            Writer::Opus(template) => template.file(&comment),
        }
    }
}

fn generate(options: &Options, out: &Path) -> Result<Summary> {
    let started = Instant::now();
    let mut header = Header {
        generator: String::from(GENERATOR),
        seed: options.seed,
        tracks: options.tracks,
        format: options.format,
        names_only: options.names_only,
        template_sha256: None,
        template_encoder: None,
        duration_ms: DURATION_MS,
        complete: false,
    };
    let writer = if options.names_only {
        None
    } else {
        let scratch = out.with_file_name(format!(
            ".{}.template-{}",
            out.file_name().map_or_else(
                || String::from("synth"),
                |n| n.to_string_lossy().into_owned()
            ),
            std::process::id()
        ));
        let (bytes, encoder) = template::make(options.format, &scratch)?;
        header.template_sha256 = Some(crate::corpus::manifest::sha256(&bytes));
        header.template_encoder = Some(encoder);
        Some(match options.format {
            Format::Flac => Writer::Flac(FlacTemplate::parse(&bytes)?),
            Format::Opus => Writer::Opus(OpusTemplate::parse(&bytes)?),
        })
    };

    prepare_folder(out, &header)?;
    write_header(out, &header)?;

    let counters = Arc::new(Counters::default());
    let failed = Arc::new(AtomicBool::new(false));
    let errors: Arc<Mutex<Vec<anyhow::Error>>> = Arc::default();
    let (sender, receiver) = mpsc::sync_channel::<Album>(256);
    let receiver = Arc::new(Mutex::new(receiver));
    let writer = writer.map(Arc::new);
    let workers: Vec<_> = match &writer {
        None => Vec::new(),
        Some(writer) => {
            let count = thread::available_parallelism().map_or(4, |n| n.get());
            (0..count)
                .map(|_| {
                    let receiver = Arc::clone(&receiver);
                    let writer = Arc::clone(writer);
                    let counters = Arc::clone(&counters);
                    let failed = Arc::clone(&failed);
                    let errors = Arc::clone(&errors);
                    let out = out.to_path_buf();
                    thread::spawn(move || {
                        loop {
                            let album = {
                                let Ok(receiver) = receiver.lock() else {
                                    return;
                                };
                                match receiver.recv() {
                                    Ok(album) => album,
                                    Err(_) => return,
                                }
                            };
                            if let Err(error) = write_album(&out, &writer, &album, &counters) {
                                failed.store(true, Ordering::Relaxed);
                                if let Ok(mut errors) = errors.lock() {
                                    errors.push(error);
                                }
                                return;
                            }
                        }
                    })
                })
                .collect()
        }
    };

    let mut summary = Summary {
        names_only: options.names_only,
        ..Summary::default()
    };
    let manifest_path = out.join(MANIFEST_FILE);
    let manifest_file = fs::File::create(&manifest_path)
        .with_context(|| format!("could not create {}", manifest_path.display()))?;
    let mut manifest = BufWriter::with_capacity(1 << 20, manifest_file);
    let step = (options.tracks / VERIFY_SAMPLE).max(1);
    for album in Catalogue::new(options.seed, options.tracks, options.format) {
        summary.albums += 1;
        match album.kind {
            Kind::Compilation => summary.compilations += 1,
            Kind::Classical => summary.classical += 1,
            Kind::Regular | Kind::Single => {}
        }
        if album.discs > 1 {
            summary.multi_disc += 1;
        }
        for track in &album.tracks {
            if std::str::from_utf8(&track.path).is_err() {
                summary.legacy_names += 1;
            }
            if track.tags.is_empty() {
                summary.untagged += 1;
            }
            if summary.tracks.is_multiple_of(step) && summary.sample.len() < VERIFY_SAMPLE {
                summary.sample.push(track.clone());
            }
            summary.tracks += 1;
            write_record(&mut manifest, track)?;
        }
        if writer.is_some() {
            if failed.load(Ordering::Relaxed) {
                break;
            }
            if sender.send(album).is_err() {
                break;
            }
        }
    }
    drop(sender);
    manifest
        .flush()
        .with_context(|| format!("could not write {}", manifest_path.display()))?;
    for worker in workers {
        if worker.join().is_err() {
            bail!("a writer thread panicked");
        }
    }
    if let Ok(mut errors) = errors.lock()
        && let Some(error) = errors.pop()
    {
        return Err(error);
    }

    header.complete = true;
    write_header(out, &header)?;
    summary.written = counters.written.load(Ordering::Relaxed);
    summary.kept = counters.kept.load(Ordering::Relaxed);
    summary.bytes = counters.bytes.load(Ordering::Relaxed);
    summary.disk_bytes = counters.disk_bytes.load(Ordering::Relaxed);
    summary.elapsed = started.elapsed();
    Ok(summary)
}

/// Makes sure `out` is empty, holds a library made with the same parameters
/// (kept), or holds another library (removed).
fn prepare_folder(out: &Path, header: &Header) -> Result<()> {
    if out.exists() {
        let header_path = out.join(HEADER_FILE);
        let old: Option<Header> = fs::read(&header_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        match old {
            Some(old) => {
                let same = Header {
                    complete: header.complete,
                    ..old.clone()
                } == *header;
                if !same {
                    println!(
                        "synth-library: {} holds a library with other parameters; making it again",
                        out.display()
                    );
                    fs::remove_dir_all(out)
                        .with_context(|| format!("could not remove {}", out.display()))?;
                }
            }
            None => {
                let empty = fs::read_dir(out)
                    .with_context(|| format!("could not read {}", out.display()))?
                    .next()
                    .is_none();
                if !empty {
                    bail!(
                        "{} is not empty and has no {HEADER_FILE}; not writing into it",
                        out.display()
                    );
                }
            }
        }
    }
    fs::create_dir_all(out).with_context(|| format!("could not create {}", out.display()))
}

fn write_header(out: &Path, header: &Header) -> Result<()> {
    let path = out.join(HEADER_FILE);
    let mut text =
        serde_json::to_string_pretty(header).context("could not serialise the header")?;
    text.push('\n');
    fs::write(&path, text).with_context(|| format!("could not write {}", path.display()))
}

fn write_record(out: &mut impl Write, track: &Track) -> Result<()> {
    let mut tags: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (key, value) in &track.tags {
        tags.entry(key).or_default().push(value);
    }
    let record = Record {
        path: names::encode(&track.path),
        duration_ms: DURATION_MS,
        tags,
    };
    serde_json::to_writer(&mut *out, &record).context("could not write the manifest")?;
    out.write_all(b"\n").context("could not write the manifest")
}

fn write_album(out: &Path, writer: &Writer, album: &Album, counters: &Counters) -> Result<()> {
    let mut made_folder: Option<&[u8]> = None;
    for track in &album.tracks {
        let path = out.join(OsStr::from_bytes(&track.path));
        let folder_bytes = track
            .path
            .iter()
            .rposition(|&byte| byte == b'/')
            .map_or(&track.path[..0], |slash| &track.path[..slash]);
        if made_folder != Some(folder_bytes) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("could not create {}", parent.display()))?;
            }
            made_folder = Some(folder_bytes);
        }
        let bytes = writer.file(&track.tags)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.len() == bytes.len() as u64 => {
                counters.kept.fetch_add(1, Ordering::Relaxed);
                metadata
            }
            _ => {
                fs::write(&path, &bytes)
                    .with_context(|| format!("could not write {}", path.display()))?;
                counters.written.fetch_add(1, Ordering::Relaxed);
                fs::symlink_metadata(&path)
                    .with_context(|| format!("could not read {}", path.display()))?
            }
        };
        counters.bytes.fetch_add(metadata.len(), Ordering::Relaxed);
        counters
            .disk_bytes
            .fetch_add(metadata.blocks() * 512, Ordering::Relaxed);
    }
    Ok(())
}

/// Checks the sampled files with `ffprobe`: readable, about one second long,
/// and with the title, album and first artist of the manifest.
fn verify(out: &Path, sample: &[Track]) -> Result<usize> {
    if crate::corpus::tools::find_program("ffprobe").is_none() {
        bail!("`--verify` needs `ffprobe`");
    }
    // ffprobe takes tens of milliseconds to start, so several run at once.
    let threads = thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = sample.len().div_ceil(threads).max(1);
    let problems: Vec<String> = thread::scope(|scope| {
        let handles: Vec<_> = sample
            .chunks(chunk)
            .map(|tracks| {
                scope.spawn(move || {
                    tracks
                        .iter()
                        .filter_map(|track| {
                            let path = out.join(OsStr::from_bytes(&track.path));
                            probe(&path, track)
                                .err()
                                .map(|error| format!("{}: {error:#}", names::encode(&track.path)))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| vec![String::from("a probe thread panicked")])
            })
            .collect()
    });
    if !problems.is_empty() {
        bail!(
            "{} of {} files failed the check:\n  {}",
            problems.len(),
            sample.len(),
            problems.join("\n  ")
        );
    }
    Ok(sample.len())
}

fn probe(path: &Path, track: &Track) -> Result<()> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:format_tags:stream_tags",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .context("could not start `ffprobe`")?;
    if !output.status.success() {
        bail!(
            "ffprobe cannot read it: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("ffprobe printed invalid JSON")?;
    let duration: f64 = report["format"]["duration"]
        .as_str()
        .and_then(|text| text.parse().ok())
        .context("ffprobe reports no duration")?;
    if !(0.95..=1.05).contains(&duration) {
        bail!("lasts {duration} s, expected about 1 s");
    }
    // FFmpeg renames some keys (ALBUMARTIST → album_artist) and joins repeated
    // keys with ";"; only plain keys are compared, case-insensitively.
    let mut probed: BTreeMap<String, String> = BTreeMap::new();
    let mut collect = |tags: &serde_json::Value| {
        if let Some(tags) = tags.as_object() {
            for (key, value) in tags {
                if let Some(value) = value.as_str() {
                    probed.insert(key.to_ascii_lowercase(), value.to_owned());
                }
            }
        }
    };
    collect(&report["format"]["tags"]);
    if let Some(streams) = report["streams"].as_array() {
        for stream in streams {
            collect(&stream["tags"]);
        }
    }
    for key in ["TITLE", "ALBUM", "ARTIST"] {
        let expected: Vec<&str> = track
            .tags
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
            .collect();
        let actual = probed.get(&key.to_ascii_lowercase());
        match (expected.first(), actual) {
            (None, None) => {}
            (Some(first), Some(actual)) if actual.split(';').any(|v| v == *first) => {}
            (expected, actual) => {
                bail!("{key} is {actual:?}, expected {expected:?}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(parse_count("5000").unwrap(), 5000);
        assert_eq!(parse_count("100k").unwrap(), 100_000);
        assert_eq!(parse_count("0.5M").unwrap(), 500_000);
        assert!(parse_count("lots").is_err());
        assert!(parse_count("-3").is_err());
    }

    #[test]
    fn options() {
        let args: Vec<String> = ["--tracks", "2k", "--format", "opus", "--seed", "9"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let options = parse_args(&args).unwrap().unwrap();
        assert_eq!(options.tracks, 2000);
        assert_eq!(options.format, Format::Opus);
        assert_eq!(default_folder(&options), "opus-2000-seed9");
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&[String::from("--bogus")]).is_err());
    }

    fn scratch(name: &str) -> PathBuf {
        let root = crate::repository_root().unwrap();
        root.join("target/tmp")
            .join(format!("synth-library-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn names_only_writes_just_the_manifest() {
        let out = scratch("names");
        let options = Options {
            tracks: 300,
            format: Format::Flac,
            out: None,
            seed: 5,
            names_only: true,
            verify: false,
        };
        let summary = generate(&options, &out).unwrap();
        assert_eq!(summary.tracks, 300);
        let mut entries: Vec<String> = fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert_eq!(entries, [HEADER_FILE, MANIFEST_FILE]);
        let manifest = fs::read_to_string(out.join(MANIFEST_FILE)).unwrap();
        let lines: Vec<serde_json::Value> = manifest
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 300);
        assert!(
            lines
                .iter()
                .all(|l| l["path"].as_str().unwrap().ends_with(".flac"))
        );
        assert!(lines.iter().any(|l| l["tags"]["TITLE"].is_array()));
        let header: Header =
            serde_json::from_slice(&fs::read(out.join(HEADER_FILE)).unwrap()).unwrap();
        assert!(header.complete);
        fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn refuses_a_foreign_folder() {
        let out = scratch("foreign");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("keep.txt"), "mine").unwrap();
        let options = Options {
            tracks: 5,
            format: Format::Flac,
            out: None,
            seed: 1,
            names_only: true,
            verify: false,
        };
        assert!(generate(&options, &out).is_err());
        assert!(out.join("keep.txt").exists());
        fs::remove_dir_all(&out).unwrap();
    }

    /// Makes 200 files of each format, checks every one with ffprobe, and
    /// checks that a second run keeps them all. Skipped without FFmpeg.
    #[test]
    fn small_library_passes_ffprobe() {
        let tools = crate::corpus::tools::find_program("ffmpeg").is_some()
            && crate::corpus::tools::find_program("ffprobe").is_some();
        if !tools {
            eprintln!("notice: ffmpeg or ffprobe is not installed; skipped");
            return;
        }
        for format in [Format::Flac, Format::Opus] {
            let out = scratch(format.extension());
            let options = Options {
                tracks: 200,
                format,
                out: None,
                seed: 11,
                names_only: false,
                verify: true,
            };
            let first = generate(&options, &out).unwrap();
            assert_eq!(first.written, 200);
            assert_eq!(first.sample.len(), 200);
            assert_eq!(verify(&out, &first.sample).unwrap(), 200);
            let second = generate(&options, &out).unwrap();
            assert_eq!((second.written, second.kept), (0, 200));
            fs::remove_dir_all(&out).unwrap();
        }
    }
}
