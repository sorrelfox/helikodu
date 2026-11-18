// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Folder sync: mirroring chosen playlists, searches, albums and artists to a
//! folder (a phone, a memory stick, another player), converting files on the way.
//!
//! Copies go one way, from the library to the destination. Helikodu removes only
//! files it put there itself: it keeps a list of them, with the source each came
//! from, in `.helikodu-sync.json` at the top of the destination. Files someone else
//! put there are left alone, also when one stands where a copy would go. Each
//! chosen playlist can be written as an `.m3u8` file in `Playlists/`, with paths
//! relative to it, so the other player finds the tracks.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use helikodu_library::{
    AlbumId, Page, PersonId, PlaylistId, PlaylistType, SavedSearchId, TrackFilter,
    parse_query_lenient,
};
use helikodu_library::{ItemContent, MAX_PAGE};
use helikodu_model::library::TrackId;
use serde::{Deserialize, Serialize};

use super::job::{ConvertEvent, ConvertSummary, ItemOutcome, JobAction, JobItem, JobOptions};
use super::naming::{NameStyle, NamingPattern};
use super::plan::{SourceTrack, source_tracks};
use super::presets::Preset;
use crate::files::write_atomic;
use crate::library::{LibraryError, MusicLibrary};

/// The list of files Helikodu put on a destination.
pub const MANIFEST_FILE: &str = ".helikodu-sync.json";

/// The folder chosen playlists are written to.
pub const PLAYLIST_FOLDER: &str = "Playlists";

/// Something chosen for a destination.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SyncSource {
    /// A playlist, a smart playlist or a folder of playlists.
    Playlist {
        /// The playlist's number in the library.
        id: i64,
    },
    /// A saved search.
    SavedSearch {
        /// Its number in the library.
        id: i64,
    },
    /// An album.
    Album {
        /// Its number in the library.
        id: i64,
    },
    /// An artist's tracks.
    Artist {
        /// Their number in the library.
        id: i64,
    },
    /// A search in the query language.
    Query {
        /// The query.
        text: String,
    },
}

/// What happens to the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncFiles {
    /// Lossless files are converted with the preset; the others are copied.
    #[default]
    ConvertLossless,
    /// Every file is copied as it is.
    CopyAsIs,
    /// Every file is converted with the preset (unless it is in its format).
    ConvertAll,
}

/// A destination and what goes there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncDestination {
    /// A stable identifier.
    pub id: String,
    /// The user's name for it.
    pub name: String,
    /// Its folder.
    pub path: PathBuf,
    /// What is copied there, in order.
    #[serde(default)]
    pub sources: Vec<SyncSource>,
    /// What happens to the files.
    #[serde(default)]
    pub files: SyncFiles,
    /// The preset files are converted with.
    #[serde(default = "default_preset")]
    pub preset: String,
    /// The naming pattern.
    #[serde(default = "default_pattern")]
    pub pattern: String,
    /// The characters names may hold.
    #[serde(default = "default_names")]
    pub names: String,
    /// Whether each chosen playlist is written as an `.m3u8` file.
    #[serde(default = "yes")]
    pub write_playlists: bool,
    /// Whether files that are no longer chosen are removed.
    #[serde(default = "yes")]
    pub remove_unchosen: bool,
    /// When it was last synced, in seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced: Option<i64>,
}

fn default_preset() -> String {
    "opus-128".to_owned()
}

fn default_pattern() -> String {
    NamingPattern::DEFAULT.to_owned()
}

fn default_names() -> String {
    NameStyle::Portable.id().to_owned()
}

const fn yes() -> bool {
    true
}

impl SyncDestination {
    /// A destination with the defaults.
    pub fn new(id: String, name: String, path: PathBuf) -> Self {
        Self {
            id,
            name,
            path,
            sources: Vec::new(),
            files: SyncFiles::default(),
            preset: default_preset(),
            pattern: default_pattern(),
            names: default_names(),
            write_playlists: true,
            remove_unchosen: true,
            last_synced: None,
        }
    }

    /// Whether its folder is there (the drive is connected).
    pub fn is_connected(&self) -> bool {
        self.path.is_dir()
    }
}

/// A chosen source with its tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSource {
    /// The source.
    pub source: SyncSource,
    /// Its name: the playlist's, the search's, the album's or the artist's.
    pub name: String,
    /// Whether it is a playlist (written as `.m3u8`) and not a folder of them.
    pub is_playlist: bool,
    /// For a folder of playlists, how many playlists it holds.
    pub playlists: usize,
    /// Its tracks, in order.
    pub tracks: Vec<TrackId>,
}

/// The tracks of `source`.
pub fn resolve(
    library: &MusicLibrary,
    source: &SyncSource,
) -> Result<ResolvedSource, LibraryError> {
    let inner = library.inner();
    let resolved = |name: String, is_playlist: bool, tracks: Vec<TrackId>| ResolvedSource {
        source: source.clone(),
        name,
        is_playlist,
        playlists: 0,
        tracks,
    };
    Ok(match source {
        SyncSource::Playlist { id } => {
            let id = PlaylistId::from_raw(*id);
            let tree = inner.playlist_tree()?;
            let Some(at) = tree.iter().position(|n| n.summary.id == id) else {
                return Ok(resolved(String::new(), false, Vec::new()));
            };
            let node = &tree[at];
            if node.summary.kind == PlaylistType::Folder {
                let mut tracks = Vec::new();
                let mut playlists = 0;
                for child in tree[at + 1..]
                    .iter()
                    .take_while(|n| n.depth > node.depth)
                    .filter(|n| n.summary.kind != PlaylistType::Folder)
                {
                    playlists += 1;
                    tracks.extend(playlist_tracks(
                        library,
                        child.summary.id,
                        child.summary.kind,
                    )?);
                }
                ResolvedSource {
                    playlists,
                    ..resolved(node.summary.name.clone(), false, tracks)
                }
            } else {
                let tracks = playlist_tracks(library, id, node.summary.kind)?;
                resolved(node.summary.name.clone(), true, tracks)
            }
        }
        SyncSource::SavedSearch { id } => {
            let id = SavedSearchId::from_raw(*id);
            match library
                .saved_searches()?
                .into_iter()
                .find(|s| s.search.id == id)
            {
                Some(entry) => {
                    let tracks = library.query_track_ids(&entry.search.query, 0)?;
                    resolved(entry.search.name, true, tracks)
                }
                None => resolved(String::new(), false, Vec::new()),
            }
        }
        SyncSource::Album { id } => {
            let album = AlbumId::from_raw(*id);
            let name = library
                .album(album)?
                .and_then(|a| a.summary.title)
                .unwrap_or_default();
            let tracks = library
                .all_tracks(TrackFilter::Album(album))?
                .into_iter()
                .map(|t| t.id)
                .collect();
            resolved(name, false, tracks)
        }
        SyncSource::Artist { id } => {
            let person = PersonId::from_raw(*id);
            let found = library.all_tracks(TrackFilter::Artist(person, None))?;
            let name = found
                .iter()
                .find_map(|t| t.artist.clone())
                .unwrap_or_default();
            resolved(name, false, found.into_iter().map(|t| t.id).collect())
        }
        SyncSource::Query { text } => {
            let query = parse_query_lenient(text).query;
            let tracks = library.query_track_ids(&query, 0)?;
            resolved(text.clone(), true, tracks)
        }
    })
}

fn playlist_tracks(
    library: &MusicLibrary,
    id: PlaylistId,
    kind: PlaylistType,
) -> Result<Vec<TrackId>, LibraryError> {
    let inner = library.inner();
    let mut out = Vec::new();
    // Items read so far, dead ones included, which have no track.
    let mut offset = 0;
    loop {
        let page = Page::new(offset, MAX_PAGE);
        let (items, read, total): (Vec<TrackId>, usize, usize) = if kind == PlaylistType::Smart {
            let page = inner.smart_playlist_tracks(id, page)?;
            let read = page.items.len();
            (
                page.items.into_iter().map(|t| t.id).collect(),
                read,
                page.total,
            )
        } else {
            let page = inner.playlist_items(id, page)?;
            let read = page.items.len();
            let items = page
                .items
                .into_iter()
                .filter_map(|item| match item.content {
                    ItemContent::Track(t) => Some(t.id),
                    ItemContent::Dead(_) => None,
                })
                .collect();
            (items, read, page.total)
        };
        out.extend(items);
        offset += read;
        if read == 0 || offset >= total {
            return Ok(out);
        }
    }
}

/// One file Helikodu put on a destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Its path on the destination, relative to the destination folder.
    pub path: PathBuf,
    /// The library file it came from.
    pub source: PathBuf,
    /// The source's size when it was copied.
    pub source_size: u64,
    /// The source's modification time, in nanoseconds since 1970.
    pub source_mtime: i64,
    /// The preset it was converted with; empty when copied as it is.
    #[serde(default)]
    pub preset: String,
}

/// What Helikodu put on a destination.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The files, by their path on the destination.
    pub files: Vec<ManifestEntry>,
}

impl Manifest {
    /// The manifest of the destination at `folder`; empty when it has none.
    pub fn load(folder: &Path) -> Self {
        fs::read(folder.join(MANIFEST_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the manifest into `folder`.
    pub fn save(&self, folder: &Path) -> io::Result<()> {
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(&folder.join(MANIFEST_FILE), &text)
    }
}

/// What a sync does with one path on the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    /// Convert a track there.
    Convert,
    /// Copy a track there as it is.
    Copy,
    /// It is there and current.
    Keep,
    /// Remove a file Helikodu put there that is no longer chosen.
    Remove,
    /// Write a playlist file.
    Playlist,
}

/// One row of a sync plan.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncItem {
    /// What happens.
    pub action: SyncAction,
    /// The path on the destination, relative to it.
    pub relative: PathBuf,
    /// The track, for copies and conversions.
    pub track: Option<SourceTrack>,
    /// The codec of the source (`flac`), or of the removed file's extension.
    pub from_codec: String,
    /// The format of the copy (`Opus`); empty when the file is copied as it is.
    pub to_format: String,
    /// The expected size of the file, in bytes; for a removal, its size.
    pub bytes: u64,
}

/// What a sync of a destination will do.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncPlan {
    /// Every row: conversions and copies, then playlists, then removals. Files
    /// that are current are left out of the rows and only counted.
    pub items: Vec<SyncItem>,
    /// Tracks to copy as they are.
    pub to_copy: usize,
    /// Tracks to convert.
    pub to_convert: usize,
    /// Tracks there already.
    pub already_there: usize,
    /// Files to remove.
    pub to_remove: usize,
    /// The space the copies and conversions need, in bytes, less what removals
    /// free.
    pub bytes_needed: u64,
    /// The space free on the destination, when it is connected.
    pub bytes_free: Option<u64>,
    /// The playlists to write: the name and the tracks' paths on the destination.
    pub playlists: Vec<(String, Vec<PathBuf>)>,
    /// The manifest the destination has now.
    pub manifest: Manifest,
    /// Files on the destination that Helikodu did not put there and that stand
    /// where a copy would go; they are left alone.
    pub foreign: usize,
}

/// Plans the sync of `destination`. `presets` are every preset there is.
pub fn plan_sync(
    library: &MusicLibrary,
    destination: &SyncDestination,
    presets: &[Preset],
) -> Result<SyncPlan, LibraryError> {
    let preset = presets
        .iter()
        .find(|p| p.id == destination.preset)
        .cloned()
        .unwrap_or_else(|| Preset::builtins().remove(0));
    let pattern = NamingPattern::parse(&destination.pattern).unwrap_or_default();
    let style = NameStyle::from_id(&destination.names).unwrap_or_default();
    let manifest = Manifest::load(&destination.path);
    let owned: HashMap<&Path, &ManifestEntry> = manifest
        .files
        .iter()
        .map(|e| (e.path.as_path(), e))
        .collect();

    let mut plan = SyncPlan::default();
    let mut placed: HashMap<TrackId, PathBuf> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut wanted: HashSet<PathBuf> = HashSet::new();
    let mut playlists: Vec<(String, Vec<TrackId>)> = Vec::new();
    for source in &destination.sources {
        let resolved = resolve(library, source)?;
        let new: Vec<TrackId> = resolved
            .tracks
            .iter()
            .copied()
            .filter(|t| !placed.contains_key(t))
            .collect();
        for track in source_tracks(library, &new)? {
            let convert = match destination.files {
                SyncFiles::CopyAsIs => false,
                SyncFiles::ConvertAll => true,
                SyncFiles::ConvertLossless => is_lossless(&track),
            } && !(track.is_whole_file()
                && preset.takes_as_is(track.codec(), track.kbps(), track.audio.sample_rate));
            // A section of a file (a CUE track) can only be converted.
            let convert = convert || !track.is_whole_file();
            let extension = if convert {
                preset.encode_format().extension().to_owned()
            } else {
                track
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            };
            let stem = pattern.render(&track, style);
            let relative = unique(&stem, &extension, &mut taken);
            placed.insert(track.track, relative.clone());
            wanted.insert(relative.clone());
            let target = destination.path.join(&relative);
            let stamp = source_stamp(&track.path);
            let preset_id = if convert { preset.id.as_str() } else { "" };
            let current = owned.get(relative.as_path()).is_some_and(|e| {
                e.source == track.path
                    && Some((e.source_size, e.source_mtime)) == stamp
                    && e.preset == preset_id
            }) && target.exists();
            if current {
                plan.already_there += 1;
                continue;
            }
            if target.exists() && !owned.contains_key(relative.as_path()) {
                plan.foreign += 1;
                plan.already_there += 1;
                continue;
            }
            let bytes = if convert {
                preset.expected_bytes(
                    track.duration().map_or(0.0, |d| d.as_secs_f64()),
                    track.audio.sample_rate.unwrap_or(44_100),
                    track.audio.channels.unwrap_or(2),
                    u32::from(track.audio.bit_depth.unwrap_or(16)),
                )
            } else {
                track.size.unwrap_or(0)
            };
            plan.bytes_needed += bytes;
            if convert {
                plan.to_convert += 1;
            } else {
                plan.to_copy += 1;
            }
            plan.items.push(SyncItem {
                action: if convert {
                    SyncAction::Convert
                } else {
                    SyncAction::Copy
                },
                relative,
                from_codec: track.codec().to_owned(),
                to_format: if convert {
                    preset.encode_format().name().to_owned()
                } else {
                    String::new()
                },
                track: Some(track),
                bytes,
            });
        }
        if resolved.is_playlist && destination.write_playlists {
            playlists.push((resolved.name.clone(), resolved.tracks.clone()));
        }
    }
    let mut names: HashSet<String> = HashSet::new();
    for (name, tracks) in playlists {
        let file = clean_playlist_name(&name, style, &mut names);
        let relative = Path::new(PLAYLIST_FOLDER).join(format!("{file}.m3u8"));
        let paths: Vec<PathBuf> = tracks
            .iter()
            .filter_map(|t| placed.get(t).cloned())
            .collect();
        wanted.insert(relative.clone());
        plan.items.push(SyncItem {
            action: SyncAction::Playlist,
            relative,
            track: None,
            from_codec: String::new(),
            to_format: String::new(),
            bytes: paths.iter().map(|p| p.as_os_str().len() as u64 + 4).sum(),
        });
        plan.playlists.push((name, paths));
    }
    if destination.remove_unchosen {
        for entry in &manifest.files {
            if wanted.contains(&entry.path) {
                continue;
            }
            let Ok(meta) = fs::metadata(destination.path.join(&entry.path)) else {
                continue;
            };
            plan.to_remove += 1;
            plan.bytes_needed = plan.bytes_needed.saturating_sub(meta.len());
            plan.items.push(SyncItem {
                action: SyncAction::Remove,
                relative: entry.path.clone(),
                track: None,
                from_codec: entry
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default(),
                to_format: String::new(),
                bytes: meta.len(),
            });
        }
    }
    plan.bytes_free = free_space(&destination.path);
    plan.manifest = manifest;
    Ok(plan)
}

fn is_lossless(track: &SourceTrack) -> bool {
    const LOSSLESS: &[&str] = &[
        "flac",
        "alac",
        "ape",
        "wavpack",
        "tta",
        "tak",
        "mlp",
        "truehd",
        "shorten",
        "dsd_lsbf",
        "dsd_msbf",
        "dsd_lsbf_planar",
        "dsd_msbf_planar",
        "dst",
        "dsf",
        "dff",
    ];
    let codec = track.codec();
    codec.starts_with("pcm_") || LOSSLESS.contains(&codec) || codec.starts_with("dsd")
}

fn unique(stem: &Path, extension: &str, taken: &mut HashSet<String>) -> PathBuf {
    let mut n = 1;
    loop {
        let mut name = stem.as_os_str().to_owned();
        if n > 1 {
            name.push(format!(" ({n})"));
        }
        name.push(".");
        name.push(extension);
        let path = PathBuf::from(name);
        if taken.insert(path.to_string_lossy().to_lowercase()) {
            return path;
        }
        n += 1;
    }
}

fn clean_playlist_name(name: &str, style: NameStyle, taken: &mut HashSet<String>) -> String {
    let base = super::naming::clean_name(name, style);
    let base = if base.is_empty() {
        "Playlist".to_owned()
    } else {
        base
    };
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            base.clone()
        } else {
            format!("{base} ({n})")
        };
        if taken.insert(candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

/// The size and modification time of a source file.
fn source_stamp(path: &Path) -> Option<(u64, i64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.mtime() * 1_000_000_000 + meta.mtime_nsec()))
}

/// The space free for an ordinary user at `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

/// An `.m3u8` playlist of `tracks` (paths relative to the destination) as a file
/// in [`PLAYLIST_FOLDER`] reads them.
pub fn m3u8_text(tracks: &[PathBuf]) -> String {
    let mut text = String::from("#EXTM3U\n");
    for track in tracks {
        text.push_str("../");
        text.push_str(&track.to_string_lossy());
        text.push('\n');
    }
    text
}

/// Carries out `plan` on `destination`: converts and copies with `options`
/// (existing files that Helikodu put there are replaced), writes the playlists,
/// removes the files no longer chosen and writes the new manifest. Events carry
/// the index of the plan's row.
pub fn run_sync(
    library: Option<&MusicLibrary>,
    destination: &SyncDestination,
    plan: &SyncPlan,
    presets: &[Preset],
    options: JobOptions,
    cancel: &helikodu_library::CancelToken,
    mut events: impl FnMut(ConvertEvent) + Send,
) -> ConvertSummary {
    let preset = presets
        .iter()
        .find(|p| p.id == destination.preset)
        .cloned()
        .unwrap_or_else(|| Preset::builtins().remove(0));
    let mut rows: Vec<usize> = Vec::new();
    let mut items: Vec<JobItem> = Vec::new();
    for (row, item) in plan.items.iter().enumerate() {
        let Some(track) = &item.track else { continue };
        let action = match item.action {
            SyncAction::Convert => JobAction::Convert(preset.settings()),
            SyncAction::Copy => JobAction::Copy,
            _ => continue,
        };
        rows.push(row);
        items.push(JobItem {
            track: Some(track.track),
            source: track.source.clone(),
            copy_tags: item.action == SyncAction::Convert,
            tags: Some(track.tags.clone()),
            target: destination.path.join(&item.relative),
            action,
        });
    }
    let total = plan.items.len();
    let mut finished = ConvertSummary::default();
    let mut manifest: BTreeMap<PathBuf, ManifestEntry> = plan
        .manifest
        .files
        .iter()
        .map(|e| (e.path.clone(), e.clone()))
        .collect();
    let mut written: Vec<(usize, ItemOutcome)> = Vec::new();
    {
        let job_options = JobOptions {
            replace: true,
            ..options
        };
        let events = std::sync::Mutex::new(&mut events);
        let written = std::sync::Mutex::new(&mut written);
        super::job::run(library, &items, job_options, cancel, |event| {
            let mapped = match event {
                ConvertEvent::Started { .. } => ConvertEvent::Started { items: total },
                ConvertEvent::Progress { index, fraction } => ConvertEvent::Progress {
                    index: rows[index],
                    fraction,
                },
                ConvertEvent::Done { index, outcome } => {
                    if let Ok(mut w) = written.lock() {
                        w.push((rows[index], outcome.clone()));
                    }
                    ConvertEvent::Done {
                        index: rows[index],
                        outcome,
                    }
                }
                ConvertEvent::Finished(summary) => {
                    finished = summary;
                    return;
                }
            };
            if let Ok(mut e) = events.lock() {
                (*e)(mapped);
            }
        });
    }
    for (row, outcome) in &written {
        let item = &plan.items[*row];
        let Some(track) = &item.track else { continue };
        if let ItemOutcome::Written(_) = outcome {
            let (source_size, source_mtime) = source_stamp(&track.path).unwrap_or((0, 0));
            manifest.insert(
                item.relative.clone(),
                ManifestEntry {
                    path: item.relative.clone(),
                    source: track.path.clone(),
                    source_size,
                    source_mtime,
                    preset: if item.action == SyncAction::Convert {
                        preset.id.clone()
                    } else {
                        String::new()
                    },
                },
            );
        }
    }
    if !cancel.is_cancelled() {
        let mut playlists = plan.playlists.iter();
        for (row, item) in plan.items.iter().enumerate() {
            let outcome = match item.action {
                SyncAction::Playlist => {
                    let Some((_, tracks)) = playlists.next() else {
                        continue;
                    };
                    let target = destination.path.join(&item.relative);
                    let result = target
                        .parent()
                        .map_or(Ok(()), fs::create_dir_all)
                        .and_then(|()| write_atomic(&target, m3u8_text(tracks).as_bytes()));
                    match result {
                        Ok(()) => {
                            manifest.insert(
                                item.relative.clone(),
                                ManifestEntry {
                                    path: item.relative.clone(),
                                    source: PathBuf::new(),
                                    source_size: 0,
                                    source_mtime: 0,
                                    preset: String::new(),
                                },
                            );
                            ItemOutcome::Written(item.bytes)
                        }
                        Err(e) => ItemOutcome::Failed(e.to_string()),
                    }
                }
                SyncAction::Remove => {
                    let target = destination.path.join(&item.relative);
                    match fs::remove_file(&target) {
                        Ok(()) => {
                            manifest.remove(&item.relative);
                            remove_empty_folders(&destination.path, &target);
                            ItemOutcome::Written(0)
                        }
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {
                            manifest.remove(&item.relative);
                            ItemOutcome::Skipped
                        }
                        Err(e) => ItemOutcome::Failed(e.to_string()),
                    }
                }
                _ => continue,
            };
            match &outcome {
                ItemOutcome::Written(_) => finished.written += 1,
                ItemOutcome::Skipped => finished.skipped += 1,
                ItemOutcome::Failed(_) => finished.failed += 1,
            }
            events(ConvertEvent::Done {
                index: row,
                outcome,
            });
        }
    }
    finished.total = total;
    let files: Vec<ManifestEntry> = manifest.into_values().collect();
    if let Err(e) = (Manifest { files }).save(&destination.path) {
        finished.failed += 1;
        events(ConvertEvent::Done {
            index: total,
            outcome: ItemOutcome::Failed(e.to_string()),
        });
    }
    events(ConvertEvent::Finished(finished.clone()));
    finished
}

/// Removes the folders between `file` and `root` that are left empty.
fn remove_empty_folders(root: &Path, file: &Path) {
    let mut folder = file.parent();
    while let Some(f) = folder {
        if f == root || !f.starts_with(root) || fs::remove_dir(f).is_err() {
            break;
        }
        folder = f.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlists_point_up_from_their_folder() {
        let text = m3u8_text(&[PathBuf::from("A/B/01 x.opus")]);
        assert_eq!(text, "#EXTM3U\n../A/B/01 x.opus\n");
    }

    #[test]
    fn destinations_keep_their_choices() {
        let mut d = SyncDestination::new("dest-1".into(), "Phone".into(), "/run/media/p".into());
        d.sources = vec![
            SyncSource::Playlist { id: 3 },
            SyncSource::Query {
                text: "rating >= 4".into(),
            },
        ];
        d.files = SyncFiles::ConvertAll;
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<SyncDestination>(&json).unwrap(), d);
    }

    #[test]
    fn empty_folders_go_with_the_last_file() {
        let root = std::env::temp_dir().join(format!("helikodu-sync-{}", std::process::id()));
        let file = root.join("A/B/x.opus");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();
        fs::remove_file(&file).unwrap();
        remove_empty_folders(&root, &file);
        assert!(root.is_dir());
        assert!(!root.join("A").exists());
        let _ = fs::remove_dir_all(root);
    }
}
