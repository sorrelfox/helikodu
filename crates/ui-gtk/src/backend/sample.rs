// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A backend over invented content: a generated library and a player that only
//! pretends to play. For the widget tests, the screenshots and `--demo`.
//!
//! Every name, count and length here is made up from the word lists below and
//! a fixed seed, so two runs show the same library. Its files do not exist; the
//! folders are made up from the tracks' paths.

mod browse;
mod copies;
mod export;
mod files;
mod history;
mod insights;
mod lyrics;
mod playlists;
mod query;
mod services;
mod signal;

pub use services::MemoryServices;
pub use signal::{playback_stats, signal_path};

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use super::{
    AlbumDetail, AlbumFilter, AlbumKey, AlbumQuery, AlbumRow, AlbumScope, AlbumSort, ArtistDetail,
    ArtistKey, ArtistRow, AudioFormat, Backend, BackendError, CoverKey, DiscGroup, FolderFile,
    FolderListing, FolderRow, FolderState, GenreKey, GenreRow, LibraryCounts, LibrarySource,
    NowPlaying, OutputInfo, PinKey, PinRow, PlaybackState, PlayerControl, PlayerRequest,
    PlaylistCleanup, PlaylistKey, PlaylistRow, PlaylistType, Query, QueueSnapshot, Repeat,
    SavedSearchKey, SavedSearchRow, SearchResults, SignalState, SmartTrack, ThumbnailState,
    TrackColumn, TrackKey, TrackQuery, TrackRow, TrackScope, TrackSort, Update,
};

/// What the sample backend contains and does at the start.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleOptions {
    /// The number of albums (about 8.5 tracks each).
    pub albums: usize,
    /// Start with this album playing: its position in the album-artist order,
    /// the track index, and the elapsed seconds.
    pub playing: Option<(usize, usize, u64)>,
}

impl Default for SampleOptions {
    fn default() -> Self {
        Self {
            albums: 1_284,
            playing: None,
        }
    }
}

const ADJECTIVES: &[&str] = &[
    "Amber", "Quiet", "Northern", "Paper", "Velvet", "Hollow", "Silver", "Distant", "Second",
    "Bright", "Patient", "Narrow", "Open", "Winter", "Copper", "Gentle", "Restless", "Folded",
    "Early", "Blue", "Late", "Small", "Salt", "Green",
];
const NOUNS: &[&str] = &[
    "Harbour", "Lanterns", "Orchard", "Signals", "Rooms", "Tides", "Echoes", "Meadow", "Rivers",
    "Lights", "Weather", "Stations", "Gardens", "Bridges", "Letters", "Islands", "Machines",
    "Horizons", "Maps", "Windows", "Hours", "Shores", "Fields", "Engines",
];
const FIRST: &[&str] = &[
    "Ada", "Brann", "Cecile", "Doro", "Elin", "Faro", "Greta", "Henrik", "Ines", "Joona", "Kaia",
    "Lumi", "Mirek", "Nell", "Oskar", "Pia", "Rasmus", "Saga", "Tarvo", "Ulla",
];
const LAST: &[&str] = &[
    "Aaltonen", "Brook", "Castell", "Dunmore", "Esker", "Fenwick", "Grove", "Halden", "Ilves",
    "Juniper", "Kestrel", "Lind", "Marrow", "Norra", "Orme", "Pike",
];
const ENSEMBLES: &[&str] = &["Quartet", "Ensemble", "Trio", "Collective", "Duo", "Choir"];
const WORDS: &[&str] = &[
    "Morning", "Glass", "Tide", "Room", "Shore", "Lantern", "Pier", "Waltz", "Field", "Line",
    "Kite", "Stone", "Ferry", "Dune", "Moth", "Cloud", "Frost", "Bell", "Thread", "Smoke",
];
const GENRES: &[&str] = &[
    "Ambient",
    "Blues",
    "Chamber music",
    "Choral",
    "Classical",
    "Electronic",
    "Folk",
    "Jazz",
    "Modern classical",
    "Pop",
    "Post-rock",
    "Rock",
    "Soundtrack",
    "World",
];

/// The time the albums were added before, so the sample shows the same dates.
fn sample_now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_000_000)
}

/// SplitMix64: a small, stable generator for the invented content.
#[derive(Debug, Clone)]
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

#[derive(Debug, Clone)]
struct SampleAlbum {
    row: AlbumRow,
    artist: usize,
    added: u64,
    genre: usize,
    single: bool,
    folder: PathBuf,
    tracks: Vec<TrackRow>,
}

use playlists::{Entry, Playlists};

/// The invented library.
#[derive(Debug)]
pub struct SampleLibrary {
    albums: Vec<SampleAlbum>,
    artists: Vec<String>,
    /// Each track's album and index in it, by key - 1.
    tracks: Vec<(u32, u16)>,
    ratings: Mutex<HashMap<TrackKey, Option<u8>>>,
    /// Where a stored rating is announced, as the service announces it.
    rated: Mutex<Option<async_channel::Sender<Update>>>,
    playlists: Mutex<Playlists>,
    roots: Vec<(PathBuf, FolderState)>,
    /// The tiles past this position stay unloaded, as while a scan runs.
    loaded: usize,
    /// The thumbnail file every cover shows, when the albums have covers.
    cover_file: Option<PathBuf>,
    /// The playback flags set through the player.
    flags: Mutex<HashMap<TrackKey, super::TrackFlags>>,
    /// The copies moved to the trash and the main copies chosen.
    copies: Mutex<copies::CopyState>,
    /// The saved searches.
    searches: Mutex<Vec<query::SampleSearch>>,
    /// The media kinds chosen for albums.
    kinds: Mutex<HashMap<AlbumKey, super::MediaKind>>,
}

fn format(r: &mut Random) -> AudioFormat {
    let (codec, rate, bits, kbps) = match r.below(10) {
        0..=2 => ("FLAC", 44_100, Some(16), None),
        3 => ("FLAC", 96_000, Some(24), None),
        4 => ("FLAC", 48_000, Some(24), None),
        5 => ("ALAC", 44_100, Some(16), None),
        6 => ("MP3", 44_100, None, Some(320)),
        7 => ("Opus", 48_000, None, Some(160)),
        8 => ("AAC", 44_100, None, Some(256)),
        _ => ("FLAC", 192_000, Some(24), None),
    };
    AudioFormat {
        codec: Some(codec.into()),
        sample_rate: Some(rate),
        bits,
        bitrate_kbps: kbps,
        channels: Some(2),
    }
}

/// The name an artist sorts by: without a leading "The".
fn sort_name(name: &str) -> String {
    match name.strip_prefix("The ") {
        Some(rest) => format!("{rest}, The"),
        None => name.to_owned(),
    }
}

fn lower(text: &str) -> String {
    text.to_lowercase()
}

/// Whether every word of `words` is in one of `fields`.
fn matches(words: &[String], fields: &[&str]) -> bool {
    words
        .iter()
        .all(|w| fields.iter().any(|f| lower(f).contains(w.as_str())))
}

fn the_roots() -> Vec<(PathBuf, FolderState)> {
    let home = gtk::glib::home_dir();
    vec![
        (home.join("Music"), FolderState::Watched),
        (PathBuf::from("/mnt/archive/Lossless"), FolderState::Polled),
        (PathBuf::from("/run/media/FIELD-REC"), FolderState::Absent),
    ]
}

/// A folder name as shown: the home folder as `~`.
fn display(path: &Path) -> String {
    let home = gtk::glib::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

impl SampleLibrary {
    /// A library of `albums` invented albums.
    pub fn new(albums: usize) -> Self {
        let mut r = Random(0x4845_4C49);
        let mut artists: Vec<String> = Vec::new();
        let mut taken = std::collections::HashSet::new();
        let wanted = (albums / 3).max(1);
        // Every name once while there are names left; then repeats are allowed.
        for attempt in 0..wanted * 8 {
            if artists.len() == wanted {
                break;
            }
            let name = if r.below(4) == 0 {
                format!("The {} {}", r.pick(NOUNS), r.pick(ENSEMBLES))
            } else {
                format!("{} {}", r.pick(FIRST), r.pick(LAST))
            };
            if taken.insert(name.clone()) || attempt >= wanted * 4 {
                artists.push(name);
            }
        }
        let roots = the_roots();
        let mut list = Vec::with_capacity(albums);
        let mut index = Vec::new();
        let mut track_key = 1;
        for number in 0..albums {
            let key = AlbumKey(number as u64 + 1);
            let title = match r.below(3) {
                0 => format!("{} {}", r.pick(ADJECTIVES), r.pick(NOUNS)),
                1 => format!("{} of {}", r.pick(NOUNS), r.pick(NOUNS)),
                _ => format!("{} {}", r.pick(ADJECTIVES), r.pick(WORDS)),
            };
            let artist_index = r.below(artists.len());
            let artist = artists[artist_index].clone();
            let year = 1972 + r.below(55) as i32;
            let format = format(&mut r);
            let single = r.below(7) == 0;
            let discs = if !single && r.below(12) == 0 { 2 } else { 1 };
            let cover_seed = r.next();
            let root = if number % 10 == 9 {
                &roots[1].0
            } else {
                &roots[0].0
            };
            let folder = root.join(&artist).join(format!("{year} {title}"));
            let extension = format.codec.as_deref().map_or("flac", |c| match c {
                "MP3" => "mp3",
                "Opus" => "opus",
                "AAC" | "ALAC" => "m4a",
                _ => "flac",
            });
            let mut tracks = Vec::new();
            for disc in 1..=discs {
                let count = if single {
                    2 + r.below(3)
                } else {
                    5 + r.below(8)
                };
                for number in 1..=count as u32 {
                    let title = format!("{} {}", r.pick(ADJECTIVES), r.pick(WORDS));
                    let track_artist = if r.below(10) == 0 {
                        let guest = &artists[r.below(artists.len())];
                        format!("{artist} & {guest}")
                    } else {
                        artist.clone()
                    };
                    let name = if discs > 1 {
                        format!("{disc}-{number:02} {title}.{extension}")
                    } else {
                        format!("{number:02} {title}.{extension}")
                    };
                    index.push((list.len() as u32, tracks.len() as u16));
                    tracks.push(TrackRow {
                        key: TrackKey(track_key),
                        album: key,
                        number: Some(number),
                        disc: (discs > 1).then_some(disc),
                        title,
                        artist: track_artist,
                        duration: Some(Duration::from_secs(120 + r.below(330) as u64)),
                        plays: r.below(60) as u32,
                        rating: (r.below(5) != 0).then(|| 1 + r.below(5) as u8),
                        entry: None,
                        from_context: true,
                        cover: None,
                        cover_seed,
                        album_title: String::new(),
                        year: Some(year),
                        format: format.clone(),
                        path: Some(folder.join(name)),
                        missing: false,
                        copies: copies::initial_copies(TrackKey(track_key)),
                        genre: String::new(),
                        added: None,
                    });
                    track_key += 1;
                }
            }
            for track in &mut tracks {
                track.album_title.clone_from(&title);
            }
            list.push(SampleAlbum {
                row: AlbumRow {
                    key,
                    title,
                    artist,
                    year: Some(year),
                    format,
                    track_count: tracks.len(),
                    cover_seed,
                    cover: None,
                },
                artist: artist_index,
                added: r.next() % 1_000_000,
                genre: r.below(GENRES.len()),
                single,
                folder,
                tracks,
            });
        }
        let library = Self {
            albums: list,
            artists,
            tracks: index,
            ratings: Mutex::new(HashMap::new()),
            rated: Mutex::new(None),
            playlists: Mutex::new(Playlists::default()),
            roots,
            loaded: usize::MAX,
            cover_file: None,
            flags: Mutex::new(HashMap::new()),
            copies: Mutex::new(copies::CopyState::default()),
            searches: Mutex::new(query::sample_searches()),
            kinds: Mutex::new(HashMap::new()),
        };
        library.add_sample_playlists(&mut r);
        library
    }

    fn searches(&self) -> std::sync::MutexGuard<'_, Vec<query::SampleSearch>> {
        self.searches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lists(&self) -> std::sync::MutexGuard<'_, Playlists> {
        self.playlists
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, key: AlbumKey) -> Option<&SampleAlbum> {
        let index = usize::try_from(key.0).ok()?.checked_sub(1)?;
        self.albums.get(index)
    }

    /// The track of `key` as it was generated.
    fn track_row_unpatched(&self, key: TrackKey) -> Option<TrackRow> {
        let index = usize::try_from(key.0).ok()?.checked_sub(1)?;
        let (album, track) = self.tracks.get(index)?;
        Some(self.albums[*album as usize].tracks[*track as usize].clone())
    }

    fn track(&self, key: TrackKey) -> Option<TrackRow> {
        let mut row = self.track_row_unpatched(key)?;
        if let Some(album) = self.get(row.album) {
            row.genre = GENRES[album.genre].to_owned();
            row.added = Some(sample_now() - Duration::from_secs(album.added));
        }
        if row.copies > 0 {
            row.copies = self.copies_now(key);
        }
        if let Some(rating) = self
            .ratings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            row.rating = *rating;
        }
        Some(row)
    }

    fn artist_row(&self, index: usize) -> ArtistRow {
        let albums: Vec<&SampleAlbum> = self.albums.iter().filter(|a| a.artist == index).collect();
        ArtistRow {
            key: ArtistKey(index as u64 + 1),
            name: self.artists[index].clone(),
            sort_name: sort_name(&self.artists[index]),
            album_count: albums.len(),
            track_count: albums.iter().map(|a| a.tracks.len()).sum(),
        }
    }

    fn artist_index(key: ArtistKey) -> Option<usize> {
        usize::try_from(key.0).ok()?.checked_sub(1)
    }

    fn in_scope(&self, album: &SampleAlbum, scope: AlbumScope) -> bool {
        match scope {
            AlbumScope::All => true,
            AlbumScope::Artist(key) => Self::artist_index(key) == Some(album.artist),
            AlbumScope::AppearsOn(key) => Self::artist_index(key).is_some_and(|i| {
                album.artist != i
                    && album
                        .tracks
                        .iter()
                        .any(|t| t.artist.ends_with(&format!("& {}", self.artists[i])))
            }),
            AlbumScope::Genre(key) => key.0 == album.genre as u64 + 1,
            AlbumScope::Composer(key) => {
                browse::composer_index(key).is_some_and(|i| album.composer() == Some(i))
            }
            AlbumScope::Years(range) => range.contains(album.row.year),
        }
    }

    fn sorted(&self, query: &AlbumQuery) -> Vec<&SampleAlbum> {
        let mut albums: Vec<&SampleAlbum> = self
            .albums
            .iter()
            .filter(|a| {
                let f = &a.row.format;
                let lossless = f.bits.is_some();
                let quality = match query.filter {
                    AlbumFilter::All => true,
                    AlbumFilter::Lossless => lossless,
                    AlbumFilter::HiRes => {
                        lossless
                            && (f.sample_rate.unwrap_or(0) > 48_000 || f.bits.unwrap_or(0) > 16)
                    }
                };
                quality && self.in_scope(a, query.scope)
            })
            .collect();
        match query.sort {
            AlbumSort::AlbumArtist => albums.sort_by(|a, b| {
                (&a.row.artist, a.row.year, &a.row.title).cmp(&(
                    &b.row.artist,
                    b.row.year,
                    &b.row.title,
                ))
            }),
            AlbumSort::Title => albums.sort_by(|a, b| a.row.title.cmp(&b.row.title)),
            AlbumSort::Year => albums.sort_by_key(|a| std::cmp::Reverse(a.row.year)),
            AlbumSort::DateAdded => albums.sort_by_key(|a| std::cmp::Reverse(a.added)),
        }
        albums
    }

    fn detail(&self, key: AlbumKey) -> Option<AlbumDetail> {
        let album = self.get(key)?;
        let mut discs: Vec<DiscGroup> = Vec::new();
        for track in &album.tracks {
            let track = self.track(track.key).unwrap_or_else(|| track.clone());
            match discs.last_mut() {
                Some(group) if group.number == track.disc => group.tracks.push(track),
                _ => discs.push(DiscGroup {
                    number: track.disc,
                    subtitle: None,
                    tracks: vec![track],
                }),
            }
        }
        Some(AlbumDetail {
            row: album.row.clone(),
            discs,
            genre: Some(GENRES[album.genre].to_string()),
            added: None,
            size: Some(album.tracks.len() as u64 * 31_000_000),
            folder: Some(display(&album.folder)),
            album_gain_db: Some(-6.3),
            kind: self.kind_of(key),
            resume: None,
            reader: None,
        })
    }

    /// Compares two tracks by one column, smallest first.
    fn compare(a: &TrackRow, b: &TrackRow, column: TrackColumn) -> Ordering {
        match column {
            TrackColumn::Number => (a.disc, a.number).cmp(&(b.disc, b.number)),
            TrackColumn::Title => lower(&a.title).cmp(&lower(&b.title)),
            TrackColumn::Artist => lower(&a.artist).cmp(&lower(&b.artist)),
            TrackColumn::Album => lower(&a.album_title).cmp(&lower(&b.album_title)),
            TrackColumn::Year => a.year.cmp(&b.year),
            TrackColumn::Format => (&a.format.codec, a.format.bits, a.format.sample_rate).cmp(&(
                &b.format.codec,
                b.format.bits,
                b.format.sample_rate,
            )),
            TrackColumn::Rating => a.rating.cmp(&b.rating),
            TrackColumn::Plays => a.plays.cmp(&b.plays),
            TrackColumn::Length => a.duration.cmp(&b.duration),
            TrackColumn::Genre => lower(&a.genre).cmp(&lower(&b.genre)),
            TrackColumn::Added => a.added.cmp(&b.added),
        }
    }

    fn folders(&self) -> impl Iterator<Item = &Path> {
        self.albums.iter().map(|a| a.folder.as_path())
    }

    fn folder_row(&self, path: &Path, root: Option<FolderState>) -> FolderRow {
        let has_children = self.folders().any(|f| f != path && f.starts_with(path));
        FolderRow {
            path: path.to_owned(),
            name: match root {
                Some(_) => display(path),
                None => path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
            root: root.is_some(),
            state: root.unwrap_or_default(),
            has_children,
        }
    }
}

impl LibrarySource for SampleLibrary {
    fn thumbnail(&self, cover: &CoverKey, size: u32) -> ThumbnailState {
        let _ = (cover, size);
        match &self.cover_file {
            Some(path) => ThumbnailState::Ready(path.clone()),
            None => ThumbnailState::Unavailable,
        }
    }
    fn counts(&self) -> Result<LibraryCounts, BackendError> {
        let artists = (0..self.artists.len())
            .filter(|i| self.albums.iter().any(|a| a.artist == *i))
            .count();
        let genres = (0..GENRES.len())
            .filter(|i| self.albums.iter().any(|a| a.genre == *i))
            .count();
        Ok(LibraryCounts {
            albums: self.albums.len(),
            artists,
            tracks: self.tracks.len(),
            genres,
            composers: self.composer_rows().len(),
            years: self.year_rows().len(),
        })
    }

    fn album_keys(&self, query: &AlbumQuery) -> Result<Vec<AlbumKey>, BackendError> {
        Ok(self.sorted(query).iter().map(|a| a.row.key).collect())
    }

    fn album_rows(
        &self,
        query: &AlbumQuery,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<AlbumRow>, BackendError> {
        let limit = limit.min(self.loaded.saturating_sub(offset));
        Ok(self
            .sorted(query)
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|a| a.row.clone())
            .collect())
    }

    fn album(&self, key: AlbumKey) -> Result<Option<AlbumDetail>, BackendError> {
        Ok(self.detail(key))
    }

    fn artists(&self) -> Result<Vec<ArtistRow>, BackendError> {
        let mut rows: Vec<ArtistRow> = (0..self.artists.len())
            .map(|i| self.artist_row(i))
            .filter(|a| a.album_count > 0)
            .collect();
        rows.sort_by_key(|a| lower(&a.sort_name));
        Ok(rows)
    }

    fn artist(&self, key: ArtistKey) -> Result<Option<ArtistDetail>, BackendError> {
        let Some(index) = Self::artist_index(key).filter(|i| *i < self.artists.len()) else {
            return Ok(None);
        };
        let row = self.artist_row(index);
        let query = |scope| AlbumQuery {
            scope,
            sort: AlbumSort::Year,
            ..AlbumQuery::default()
        };
        let own = self.sorted(&query(AlbumScope::Artist(key)));
        let appears = self.sorted(&query(AlbumScope::AppearsOn(key)));
        let mut tracks: Vec<TrackRow> = own
            .iter()
            .flat_map(|a| a.tracks.iter())
            .filter_map(|t| self.track(t.key))
            .collect();
        let duration = tracks.iter().filter_map(|t| t.duration).sum();
        tracks.sort_by_key(|t| std::cmp::Reverse(t.plays));
        tracks.truncate(5);
        Ok(Some(ArtistDetail {
            row,
            albums: own
                .iter()
                .filter(|a| !a.single)
                .map(|a| a.row.clone())
                .collect(),
            singles: own
                .iter()
                .filter(|a| a.single)
                .map(|a| a.row.clone())
                .collect(),
            appears_on: appears.iter().map(|a| a.row.clone()).collect(),
            most_played: tracks,
            duration,
        }))
    }

    fn find_artist(&self, name: &str) -> Result<Option<ArtistKey>, BackendError> {
        let name = lower(name);
        Ok(self
            .artists
            .iter()
            .position(|a| lower(a) == name)
            .map(|i| ArtistKey(i as u64 + 1)))
    }

    fn genres(&self) -> Result<Vec<GenreRow>, BackendError> {
        Ok(GENRES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let albums: Vec<&SampleAlbum> =
                    self.albums.iter().filter(|a| a.genre == i).collect();
                GenreRow {
                    key: GenreKey(i as u64 + 1),
                    name: (*name).to_owned(),
                    album_count: albums.len(),
                    track_count: albums.iter().map(|a| a.tracks.len()).sum(),
                }
            })
            .filter(|g| g.album_count > 0)
            .collect())
    }

    fn track_keys(&self, query: &TrackQuery) -> Result<Vec<TrackKey>, BackendError> {
        let parsed = helikodu_service::library::parse_query_lenient(query.text.trim());
        if parsed.error.is_none() && !parsed.query.is_empty() && !parsed.query.is_plain_text() {
            let all = self.track_keys(&TrackQuery {
                scope: query.scope,
                ..TrackQuery::default()
            })?;
            let mut selector = parsed.query;
            if !query.sort.is_empty() {
                selector.sort.clear();
            }
            let rows = query::select(&selector, self.track_rows(&all)?, SystemTime::now());
            let mut keys: Vec<TrackKey> = rows.iter().map(|r| r.key).collect();
            if !query.sort.is_empty() {
                let order = self.track_keys(&TrackQuery {
                    scope: query.scope,
                    sort: query.sort.clone(),
                    ..TrackQuery::default()
                })?;
                let found: std::collections::HashSet<TrackKey> = keys.into_iter().collect();
                keys = order.into_iter().filter(|k| found.contains(k)).collect();
            }
            return Ok(keys);
        }
        let words: Vec<String> = query.text.split_whitespace().map(lower).collect();
        let order = self.sorted(&AlbumQuery::default());
        let mut rows: Vec<TrackRow> = order
            .iter()
            .filter(|a| match query.scope {
                TrackScope::All => true,
                TrackScope::Artist(key) => {
                    self.in_scope(a, AlbumScope::Artist(key))
                        || self.in_scope(a, AlbumScope::AppearsOn(key))
                }
                TrackScope::Genre(key) => self.in_scope(a, AlbumScope::Genre(key)),
            })
            .flat_map(|a| a.tracks.iter())
            .filter(|t| match query.scope {
                TrackScope::Artist(key) => Self::artist_index(key)
                    .is_some_and(|i| t.artist.contains(self.artists[i].as_str())),
                _ => true,
            })
            .filter(|t| words.is_empty() || matches(&words, &[&t.title, &t.artist, &t.album_title]))
            .map(|t| self.track(t.key).unwrap_or_else(|| t.clone()))
            .collect();
        if !query.sort.is_empty() {
            let sort: &[TrackSort] = &query.sort;
            rows.sort_by(|a, b| {
                sort.iter()
                    .map(|key| {
                        let order = Self::compare(a, b, key.column);
                        if key.descending {
                            order.reverse()
                        } else {
                            order
                        }
                    })
                    .find(|o| *o != Ordering::Equal)
                    .unwrap_or(Ordering::Equal)
            });
        }
        Ok(rows.into_iter().map(|t| t.key).collect())
    }

    fn track_rows(&self, keys: &[TrackKey]) -> Result<Vec<TrackRow>, BackendError> {
        Ok(keys.iter().filter_map(|k| self.track(*k)).collect())
    }

    fn search(&self, text: &str) -> Result<SearchResults, BackendError> {
        let words: Vec<String> = text.split_whitespace().map(lower).collect();
        if words.is_empty() {
            return Ok(SearchResults::default());
        }
        let artists = self
            .artists()?
            .into_iter()
            .filter(|a| matches(&words, &[&a.name]))
            .take(5)
            .collect();
        let albums = self
            .sorted(&AlbumQuery::default())
            .into_iter()
            .filter(|a| matches(&words, &[&a.row.title]))
            .map(|a| a.row.clone())
            .take(8)
            .collect();
        let keys = self.track_keys(&TrackQuery {
            text: text.to_owned(),
            ..TrackQuery::default()
        })?;
        let mut tracks = self.track_rows(&keys)?;
        let first = words.first().cloned().unwrap_or_default();
        tracks.sort_by_key(|t| !lower(&t.title).contains(first.as_str()));
        tracks.truncate(50);
        Ok(SearchResults {
            artists,
            albums,
            tracks,
            track_total: keys.len(),
        })
    }

    fn set_rating(&self, track: TrackKey, stars: Option<u8>) -> Result<(), BackendError> {
        self.ratings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(track, stars.filter(|s| *s > 0));
        if let Some(updates) = self
            .rated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            let _ = updates.try_send(Update::RatingChanged {
                track,
                stars: stars.filter(|s| *s > 0),
            });
        }
        Ok(())
    }

    fn playlists(&self) -> Result<Vec<PlaylistRow>, BackendError> {
        Ok(self.sample_playlists())
    }

    fn playlist_items(&self, key: PlaylistKey) -> Result<Vec<TrackRow>, BackendError> {
        self.sample_playlist_items(key)
    }

    fn create_playlist(&self, name: &str) -> Result<PlaylistKey, BackendError> {
        Ok(self.sample_create(name, PlaylistType::Static, None, None))
    }

    fn rename_playlist(&self, key: PlaylistKey, name: &str) -> Result<(), BackendError> {
        self.sample_rename(key, name)
    }

    fn delete_playlist(&self, key: PlaylistKey) -> Result<(), BackendError> {
        self.sample_delete(key);
        Ok(())
    }

    fn add_to_playlist(
        &self,
        key: PlaylistKey,
        tracks: &[TrackKey],
        position: Option<usize>,
    ) -> Result<(), BackendError> {
        if tracks.iter().any(|t| self.track(*t).is_none()) {
            return Err(BackendError("a track is not in the library".into()));
        }
        self.edit(key, |p| {
            let at = position.unwrap_or(p.entries.len()).min(p.entries.len());
            p.entries
                .splice(at..at, tracks.iter().map(|t| Entry::Track(*t)));
            Ok(())
        })
    }

    fn move_in_playlist(
        &self,
        key: PlaylistKey,
        positions: &[usize],
        to: usize,
    ) -> Result<(), BackendError> {
        self.edit(key, |p| {
            let chosen: BTreeSet<usize> = positions.iter().copied().collect();
            if chosen.iter().any(|i| *i >= p.entries.len()) || to > p.entries.len() {
                return Err(BackendError("no such position".into()));
            }
            let moving: Vec<Entry> = chosen.iter().map(|i| p.entries[*i].clone()).collect();
            let before = chosen.iter().filter(|i| **i < to).count();
            let mut rest: Vec<Entry> = p
                .entries
                .iter()
                .enumerate()
                .filter(|(i, _)| !chosen.contains(i))
                .map(|(_, e)| e.clone())
                .collect();
            let at = (to - before).min(rest.len());
            rest.splice(at..at, moving);
            p.entries = rest;
            Ok(())
        })
    }

    fn remove_from_playlist(
        &self,
        key: PlaylistKey,
        positions: &[usize],
    ) -> Result<(), BackendError> {
        self.edit(key, |p| {
            let chosen: BTreeSet<usize> = positions.iter().copied().collect();
            let mut i = 0;
            p.entries.retain(|_| {
                let keep = !chosen.contains(&i);
                i += 1;
                keep
            });
            Ok(())
        })
    }

    fn create_playlist_folder(
        &self,
        name: &str,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        Ok(self.sample_create(name, PlaylistType::Folder, parent, None))
    }

    fn move_playlist(
        &self,
        key: PlaylistKey,
        parent: Option<PlaylistKey>,
        position: Option<usize>,
    ) -> Result<(), BackendError> {
        self.sample_move(key, parent, position)
    }

    fn duplicate_playlist(
        &self,
        key: PlaylistKey,
        name: &str,
    ) -> Result<PlaylistKey, BackendError> {
        self.sample_duplicate(key, name)
    }

    fn set_playlist_locked(&self, key: PlaylistKey, locked: bool) -> Result<(), BackendError> {
        self.sample_flag(key, |p| p.set_locked(locked))
    }

    fn set_playlist_grouped(&self, key: PlaylistKey, grouped: bool) -> Result<(), BackendError> {
        self.sample_flag(key, |p| p.set_grouped(grouped))
    }

    fn undo_playlist(&self, key: PlaylistKey) -> Result<bool, BackendError> {
        self.sample_playlist_history(key, false)
    }

    fn redo_playlist(&self, key: PlaylistKey) -> Result<bool, BackendError> {
        self.sample_playlist_history(key, true)
    }

    fn playlist_cleanup(&self, key: PlaylistKey) -> Result<PlaylistCleanup, BackendError> {
        self.sample_cleanup(key)
    }

    fn remove_playlist_duplicates(&self, key: PlaylistKey) -> Result<usize, BackendError> {
        self.sample_tidy(key, true)
    }

    fn remove_missing_from_playlist(&self, key: PlaylistKey) -> Result<usize, BackendError> {
        self.sample_tidy(key, false)
    }

    fn shuffle_playlist(&self, key: PlaylistKey) -> Result<(), BackendError> {
        self.sample_shuffle(key)
    }

    fn create_smart_playlist(
        &self,
        name: &str,
        query: &Query,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        Ok(self.sample_create(name, PlaylistType::Smart, parent, Some(query.clone())))
    }

    fn set_smart_query(&self, key: PlaylistKey, query: &Query) -> Result<(), BackendError> {
        self.sample_set_query(key, query)
    }

    fn smart_query(&self, key: PlaylistKey) -> Result<Query, BackendError> {
        self.sample_query(key)
    }

    fn smart_tracks(&self, key: PlaylistKey) -> Result<Vec<SmartTrack>, BackendError> {
        self.sample_smart_tracks(key)
    }

    fn query_match_count(&self, query: &Query) -> Result<(usize, usize), BackendError> {
        Ok(self.sample_match_count(query))
    }

    fn pins(&self) -> Result<Vec<PinRow>, BackendError> {
        Ok(self.sample_pins())
    }

    fn pin(&self, key: PinKey) -> Result<(), BackendError> {
        self.sample_pin(key, true);
        Ok(())
    }

    fn unpin(&self, key: PinKey) -> Result<(), BackendError> {
        self.sample_pin(key, false);
        Ok(())
    }

    fn import_playlist(&self, path: &Path) -> Result<(PlaylistKey, String), BackendError> {
        let name = path.file_stem().map_or_else(
            || "Imported".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let key = self.create_playlist(&name)?;
        Ok((key, name))
    }

    fn history(&self, since: Option<SystemTime>) -> Result<Vec<super::HistoryRow>, BackendError> {
        Ok(self.sample_history(since))
    }

    fn history_totals(&self) -> Result<super::HistoryTotals, BackendError> {
        Ok(self.sample_history_totals())
    }

    fn export_history(
        &self,
        path: &Path,
        since: Option<SystemTime>,
    ) -> Result<usize, BackendError> {
        self.sample_export_history(path, since)
    }

    fn export_playlist(&self, key: PlaylistKey, path: &Path) -> Result<usize, BackendError> {
        let items = self.playlist_items(key)?;
        let mut text = String::from("#EXTM3U\n");
        for item in &items {
            if let Some(file) = &item.path {
                text.push_str(&format!("{}\n", file.display()));
            }
        }
        std::fs::write(path, text).map_err(|e| BackendError(e.to_string()))?;
        Ok(items.len())
    }

    fn composers(&self) -> Result<Vec<ArtistRow>, BackendError> {
        Ok(self.composer_rows())
    }

    fn years(&self) -> Result<Vec<super::YearRow>, BackendError> {
        Ok(self.year_rows())
    }

    fn facet_values(
        &self,
        query: &TrackQuery,
        chosen: &[super::FacetChoice],
        field: super::FacetField,
    ) -> Result<super::FacetList, BackendError> {
        self.sample_facets(query, chosen, field)
    }

    fn facet_track_keys(
        &self,
        query: &TrackQuery,
        chosen: &[super::FacetChoice],
    ) -> Result<Vec<TrackKey>, BackendError> {
        self.sample_facet_keys(query, chosen)
    }

    fn statistics(
        &self,
        since: super::StatisticsSince,
    ) -> Result<super::LibraryStats, BackendError> {
        self.sample_statistics(since)
    }

    fn composer_duration(&self, composer: ArtistKey) -> Result<Duration, BackendError> {
        Ok(self
            .sorted(&AlbumQuery {
                scope: AlbumScope::Composer(composer),
                ..AlbumQuery::default()
            })
            .iter()
            .flat_map(|a| a.tracks.iter().filter_map(|t| t.duration))
            .sum())
    }

    fn track_facts(&self, track: TrackKey) -> Result<Option<super::TrackFacts>, BackendError> {
        let Some(row) = self.track(track) else {
            return Ok(None);
        };
        let date = |time: std::time::SystemTime, format: &str| {
            let seconds = time.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            let time = gtk::glib::DateTime::from_unix_local(i64::try_from(seconds).ok()?).ok()?;
            time.format(format).ok().map(|s| s.to_string())
        };
        let seconds = row.duration.map_or(0, |d| d.as_secs());
        let kbps = u64::from(row.format.bitrate_kbps.unwrap_or(900));
        Ok(Some(super::TrackFacts {
            composers: String::new(),
            last_played: (row.plays > 0)
                .then(|| date(sample_now() - Duration::from_secs(track.0 * 7_919), "%x %X"))
                .flatten(),
            added: row.added.and_then(|t| date(t, "%x")),
            file_size: Some(seconds * kbps * 125),
            track_gain_db: Some(-2.0 - (track.0 % 7) as f64 * 0.9),
            has_lyrics: track.0.is_multiple_of(3),
        }))
    }

    fn set_album_kind(&self, album: AlbumKey, kind: super::MediaKind) -> Result<(), BackendError> {
        self.set_kind(album, kind);
        Ok(())
    }

    fn folder_roots(&self) -> Result<Vec<FolderRow>, BackendError> {
        Ok(self
            .roots
            .iter()
            .map(|(path, state)| self.folder_row(path, Some(*state)))
            .collect())
    }

    fn subfolders(&self, path: &Path) -> Result<Vec<FolderRow>, BackendError> {
        let mut names = BTreeSet::new();
        for folder in self.folders() {
            if let Ok(rest) = folder.strip_prefix(path)
                && let Some(first) = rest.components().next()
            {
                names.insert(path.join(first.as_os_str()));
            }
        }
        Ok(names.iter().map(|p| self.folder_row(p, None)).collect())
    }

    fn folder(&self, path: &Path) -> Result<FolderListing, BackendError> {
        let state = self
            .roots
            .iter()
            .find(|(root, _)| path.starts_with(root))
            .map(|(_, state)| *state);
        let mut files = Vec::new();
        let mut other_files = 0;
        for album in self.albums.iter().filter(|a| a.folder == path) {
            other_files += 2;
            for track in &album.tracks {
                let Some(file) = track.path.clone() else {
                    continue;
                };
                let seconds = track.duration.map_or(0, |d| d.as_secs());
                files.push(FolderFile {
                    name: file
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    size: seconds * 4_100_000 / 10 + 1_000_000,
                    track: self.track(track.key),
                    title: None,
                    duration: None,
                    path: file,
                });
            }
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(FolderListing {
            files,
            other_files,
            state,
        })
    }

    fn saved_searches(&self) -> Result<Vec<SavedSearchRow>, BackendError> {
        let searches = self.searches().clone();
        searches
            .into_iter()
            .map(|search| {
                let count = self
                    .track_keys(&TrackQuery {
                        text: search.text.clone(),
                        ..TrackQuery::default()
                    })?
                    .len();
                Ok(SavedSearchRow {
                    key: search.key,
                    name: search.name,
                    text: search.text,
                    count,
                })
            })
            .collect()
    }

    fn save_search(
        &self,
        name: &str,
        query: &helikodu_model::query::Query,
    ) -> Result<SavedSearchKey, BackendError> {
        let mut searches = self.searches();
        let key = SavedSearchKey(searches.iter().map(|s| s.key.0).max().unwrap_or(0) + 1);
        searches.push(query::SampleSearch {
            key,
            name: name.to_owned(),
            text: helikodu_service::library::print_query(query),
        });
        Ok(key)
    }

    fn rename_saved_search(&self, key: SavedSearchKey, name: &str) -> Result<(), BackendError> {
        let mut searches = self.searches();
        let search = searches
            .iter_mut()
            .find(|s| s.key == key)
            .ok_or_else(|| BackendError("no such saved search".into()))?;
        search.name = name.to_owned();
        Ok(())
    }

    fn delete_saved_search(&self, key: SavedSearchKey) -> Result<(), BackendError> {
        self.searches().retain(|s| s.key != key);
        Ok(())
    }
}

impl SampleLibrary {
    /// The rows of `keys` as queue entries numbered from `first`.
    fn queue_rows(&self, keys: &[TrackKey], first: u64) -> Vec<TrackRow> {
        keys.iter()
            .filter_map(|k| self.track(*k))
            .enumerate()
            .map(|(i, mut t)| {
                t.entry = Some(first + i as u64);
                t
            })
            .collect()
    }
}

#[derive(Debug, Default)]
struct FakeState {
    /// The offset of the sample lyrics, in milliseconds.
    lyrics_offset: i64,
    shuffle: bool,
    repeat: Repeat,
    stop_after: bool,
    queue: Vec<TrackRow>,
    context: Option<String>,
    current: Option<usize>,
    state: PlaybackState,
    elapsed: Duration,
    next_entry: u64,
    /// Every play mode, and stop-after.
    modes: super::PlayModes,
    stop_after_mode: super::StopAfter,
    /// The A-B loop and the entry it belongs to.
    ab_loop: Option<(super::LoopPoints, Option<u64>)>,
    sleep: Option<super::SleepTimer>,
    cursor: Option<u64>,
}

/// A player that only pretends: it follows requests and reports updates at once.
#[derive(Debug)]
pub struct FakePlayer {
    library: Arc<SampleLibrary>,
    state: RefCell<FakeState>,
    updates: async_channel::Sender<Update>,
}

impl FakePlayer {
    fn emit(&self, update: Update) {
        // The receiver lives as long as the window; after that nobody listens.
        let _ = self.updates.try_send(update);
    }

    fn output(&self, signal: SignalState) -> OutputInfo {
        OutputInfo {
            device: "Studio Interface".into(),
            signal,
            sample_rate: (signal != SignalState::Idle).then_some(96_000),
            bits: (signal != SignalState::Idle).then_some(24),
        }
    }

    fn announce(&self) {
        let state = self.state.borrow();
        let now = state.current.and_then(|i| state.queue.get(i)).map(|track| {
            let album = self.library.get(track.album);
            NowPlaying {
                track: track.clone(),
                album_title: album.map(|a| a.row.title.clone()).unwrap_or_default(),
                year: album.and_then(|a| a.row.year),
                cover_seed: album.map_or(0, |a| a.row.cover_seed),
                cover: None,
                format: album.map(|a| a.row.format.clone()).unwrap_or_default(),
            }
        });
        let length = now.as_ref().and_then(|n| n.track.duration);
        let playing = state.state;
        let snapshot = QueueSnapshot {
            context: state.context.clone(),
            items: state.queue.clone(),
            current: state.current,
        };
        let elapsed = state.elapsed;
        drop(state);
        let signal = if now.is_some() {
            SignalState::BitPerfect
        } else {
            SignalState::Idle
        };
        self.emit(Update::Queue(snapshot));
        self.emit(Update::NowPlaying(now));
        self.emit(Update::State(playing));
        self.emit(Update::Output(self.output(signal)));
        if signal == SignalState::Idle {
            self.emit(Update::SignalPath(None));
        } else {
            let path = signal_path(super::SignalLamp::BitPerfect);
            self.emit(Update::SignalPath(Some(Box::new(path))));
            self.emit(Update::Stats(playback_stats()));
        }
        self.emit(Update::Position { elapsed, length });
        self.announce_lyrics();
        self.end_loop_if_moved();
    }

    /// The lyrics of the current track and the line sung now.
    fn announce_lyrics(&self) {
        let state = self.state.borrow();
        let lyrics = state
            .current
            .and_then(|i| state.queue.get(i))
            .and_then(|t| lyrics::sample_lyrics(t.number, state.lyrics_offset));
        let line = lyrics
            .as_ref()
            .and_then(|l| lyrics::line_at(l, state.elapsed));
        drop(state);
        self.emit(Update::Lyrics(lyrics.map(Arc::new)));
        self.emit(Update::LyricsLine(line));
    }

    fn step(&self, by: isize) {
        {
            let mut state = self.state.borrow_mut();
            let Some(current) = state.current else { return };
            let next = current as isize + by;
            if next < 0 || next as usize >= state.queue.len() {
                state.current = None;
                state.state = PlaybackState::Idle;
            } else {
                state.current = Some(next as usize);
            }
            state.elapsed = Duration::ZERO;
        }
        self.announce();
    }
}

impl FakePlayer {
    /// Replaces the queue with `keys` and plays the one at `start`.
    fn play(&self, keys: &[TrackKey], start: usize, title: String) {
        {
            let mut state = self.state.borrow_mut();
            let first = state.next_entry + 1;
            state.queue = self.library.queue_rows(keys, first);
            state.next_entry += state.queue.len() as u64;
            if state.queue.is_empty() {
                return;
            }
            state.context = Some(title);
            state.current = Some(start.min(state.queue.len() - 1));
            state.state = PlaybackState::Playing;
            state.elapsed = Duration::ZERO;
        }
        self.announce();
    }
}

impl PlayerControl for FakePlayer {
    /// While playing, one measurement shaped like music; nothing moves after it
    /// until another comes (tests send them through the updates).
    fn follow_analyzer(&self, per_second: Option<u32>) {
        if per_second.is_none() || self.state.borrow().state != PlaybackState::Playing {
            return;
        }
        let bands = (0..helikodu_model::player::SPECTRUM_BANDS)
            .map(|i| -14.0 - i as f32 * 0.7 - (i as f32 * 1.3).sin().abs() * 9.0)
            .collect();
        use helikodu_model::audio::ChannelPosition;
        let channels = [
            (ChannelPosition::FrontLeft, -14.2, -3.1),
            (ChannelPosition::FrontRight, -15.3, -3.9),
        ]
        .into_iter()
        .map(|(position, rms_db, peak_db)| super::ChannelLevel {
            position,
            rms_db,
            peak_db,
        })
        .collect();
        self.emit(Update::Analyzer(Arc::new(super::AnalyzerFrame {
            bands,
            channels,
            live: true,
        })));
    }

    fn send(&self, request: PlayerRequest) {
        match request {
            PlayerRequest::PlayAlbum { album, start, .. } => {
                let Some(detail) = self.library.detail(album) else {
                    return;
                };
                let keys: Vec<TrackKey> = detail.tracks().map(|t| t.key).collect();
                self.play(&keys, start, detail.row.title.clone());
            }
            PlayerRequest::PlayTracks {
                tracks,
                start,
                title,
                ..
            }
            | PlayerRequest::PlayPlaylist {
                tracks,
                start,
                title,
                ..
            } => self.play(&tracks, start, title),
            PlayerRequest::PlayArtist { artist, .. } => {
                let keys = self
                    .library
                    .track_keys(&TrackQuery {
                        scope: TrackScope::Artist(artist),
                        ..TrackQuery::default()
                    })
                    .unwrap_or_default();
                let name = self
                    .library
                    .artist(artist)
                    .ok()
                    .flatten()
                    .map(|a| a.row.name)
                    .unwrap_or_default();
                self.play(&keys, 0, name);
            }
            PlayerRequest::PlayComposer { composer, .. } => {
                let scope = AlbumScope::Composer(composer);
                let keys = self.library.scope_tracks(scope);
                self.play(&keys, 0, SampleLibrary::scope_name(scope));
            }
            PlayerRequest::PlayYears { years, .. } => {
                let scope = AlbumScope::Years(years);
                let keys = self.library.scope_tracks(scope);
                self.play(&keys, 0, SampleLibrary::scope_name(scope));
            }
            PlayerRequest::PlayGenre { genre, .. } => {
                let keys = self
                    .library
                    .track_keys(&TrackQuery {
                        scope: TrackScope::Genre(genre),
                        ..TrackQuery::default()
                    })
                    .unwrap_or_default();
                let index = usize::try_from(genre.0).unwrap_or(1).saturating_sub(1);
                let name = GENRES.get(index).copied().unwrap_or_default();
                self.play(&keys, 0, name.to_owned());
            }
            PlayerRequest::Enqueue { tracks, next } => {
                {
                    let mut state = self.state.borrow_mut();
                    let first = state.next_entry + 1;
                    let mut rows = self.library.queue_rows(&tracks, first);
                    state.next_entry += rows.len() as u64;
                    for row in &mut rows {
                        row.from_context = false;
                    }
                    let at = match (next, state.current) {
                        (true, Some(current)) => current + 1,
                        _ => state.queue.len(),
                    };
                    state.queue.splice(at..at, rows);
                }
                self.announce();
            }
            PlayerRequest::RemoveEntries(entries) => {
                {
                    let mut state = self.state.borrow_mut();
                    let playing = state.current.and_then(|i| state.queue.get(i)?.entry);
                    state
                        .queue
                        .retain(|t| !t.entry.is_some_and(|e| entries.contains(&e)));
                    state.current =
                        playing.and_then(|e| state.queue.iter().position(|t| t.entry == Some(e)));
                }
                self.announce();
            }
            PlayerRequest::MoveEntry { entry, to } => {
                {
                    let mut state = self.state.borrow_mut();
                    let playing = state.current.and_then(|i| state.queue.get(i)?.entry);
                    if let Some(from) = state.queue.iter().position(|t| t.entry == Some(entry)) {
                        let row = state.queue.remove(from);
                        let to = to.min(state.queue.len());
                        state.queue.insert(to, row);
                    }
                    state.current =
                        playing.and_then(|e| state.queue.iter().position(|t| t.entry == Some(e)));
                }
                self.announce();
            }
            PlayerRequest::PlayPause => {
                let state = {
                    let mut s = self.state.borrow_mut();
                    s.state = match s.state {
                        PlaybackState::Playing => PlaybackState::Paused,
                        PlaybackState::Paused => PlaybackState::Playing,
                        other => other,
                    };
                    s.state
                };
                self.emit(Update::State(state));
            }
            PlayerRequest::Play => {
                let state = {
                    let mut s = self.state.borrow_mut();
                    if s.state == PlaybackState::Paused {
                        s.state = PlaybackState::Playing;
                    }
                    s.state
                };
                self.emit(Update::State(state));
            }
            PlayerRequest::Stop => {
                {
                    let mut s = self.state.borrow_mut();
                    s.state = PlaybackState::Idle;
                    s.current = None;
                    s.elapsed = Duration::ZERO;
                }
                self.announce();
            }
            PlayerRequest::Next => self.step(1),
            PlayerRequest::Previous => self.step(-1),
            PlayerRequest::Seek(to) => {
                let length = {
                    let mut s = self.state.borrow_mut();
                    s.elapsed = to;
                    s.current
                        .and_then(|i| s.queue.get(i))
                        .and_then(|t| t.duration)
                };
                self.emit(Update::Position {
                    elapsed: to,
                    length,
                });
                self.announce_lyrics();
            }
            PlayerRequest::AdjustLyricsOffset(delta) => {
                {
                    let mut s = self.state.borrow_mut();
                    s.lyrics_offset = (s.lyrics_offset + delta).clamp(-60_000, 60_000);
                }
                self.announce_lyrics();
            }
            PlayerRequest::ReloadLyrics => self.announce_lyrics(),
            PlayerRequest::SeekBy(seconds) => {
                let to = {
                    let s = self.state.borrow();
                    Duration::from_secs_f64((s.elapsed.as_secs_f64() + seconds).max(0.0))
                };
                self.send(PlayerRequest::Seek(to));
            }
            PlayerRequest::SetVolume(volume) => self.emit(Update::Volume(volume)),
            PlayerRequest::PlayFiles { files, .. } => self.emit(Update::Notice {
                message: format!(
                    "The sample player plays no files ({} dropped).",
                    files.len()
                ),
            }),
            PlayerRequest::PlayEntry(entry) => {
                {
                    let mut state = self.state.borrow_mut();
                    let Some(index) = state.queue.iter().position(|t| t.entry == Some(entry))
                    else {
                        return;
                    };
                    state.current = Some(index);
                    state.state = PlaybackState::Playing;
                    state.elapsed = Duration::ZERO;
                }
                self.announce();
            }
            PlayerRequest::ClearQueue => {
                {
                    let mut state = self.state.borrow_mut();
                    let current = state.current.and_then(|i| state.queue.get(i).cloned());
                    state.queue = current.into_iter().collect();
                    state.current = state.current.map(|_| 0);
                }
                self.announce();
            }
            PlayerRequest::SetShuffle(on) => {
                let repeat = {
                    let mut state = self.state.borrow_mut();
                    state.shuffle = on;
                    state.modes.shuffle = if on {
                        super::ShuffleMode::Tracks
                    } else {
                        super::ShuffleMode::Off
                    };
                    state.repeat
                };
                self.emit(Update::Modes {
                    shuffle: on,
                    repeat,
                });
            }
            PlayerRequest::SetRepeat(repeat) => {
                let shuffle = {
                    let mut state = self.state.borrow_mut();
                    state.repeat = repeat;
                    state.modes.repeat = match repeat {
                        Repeat::Off => super::RepeatMode::Off,
                        Repeat::All => super::RepeatMode::All,
                        Repeat::One => super::RepeatMode::One,
                    };
                    state.shuffle
                };
                self.emit(Update::Modes { shuffle, repeat });
            }
            PlayerRequest::SetStopAfter(on) => {
                self.state.borrow_mut().stop_after = on;
                self.emit(Update::StopAfter(on));
            }
            other => self.send_play_mode(other),
        }
    }
}

impl FakePlayer {
    /// The play modes, the album commands, the A-B loop, the sleep timer and
    /// the track flags.
    fn send_play_mode(&self, request: PlayerRequest) {
        use super::{LoopPoints, SleepTimer, StopAfter};
        match request {
            PlayerRequest::SetShuffleMode(mode) => {
                self.change_modes(|m| m.shuffle = mode);
            }
            PlayerRequest::SetRepeatMode(mode) => self.change_modes(|m| m.repeat = mode),
            PlayerRequest::SetConsume(on) => self.change_modes(|m| m.consume = on),
            PlayerRequest::SetPlayScope(scope) => self.change_modes(|m| m.scope = scope),
            PlayerRequest::SetFollowCursor(on) => self.change_modes(|m| m.follow_cursor = on),
            PlayerRequest::SetStopAfterMode(mode) => {
                {
                    let mut state = self.state.borrow_mut();
                    state.stop_after_mode = mode;
                    state.stop_after = mode != StopAfter::Off;
                }
                self.emit(Update::StopAfter(mode != StopAfter::Off));
                self.emit(Update::StopAfterMode(mode));
            }
            PlayerRequest::SetCursor(entry) => {
                self.state.borrow_mut().cursor = entry;
                self.emit(Update::Cursor(entry));
            }
            PlayerRequest::NextAlbum
            | PlayerRequest::PreviousAlbum
            | PlayerRequest::RandomAlbum => {
                let to = {
                    let state = self.state.borrow();
                    let Some(current) = state.current else { return };
                    album_start(&state.queue, current, &request)
                };
                if let Some(to) = to {
                    {
                        let mut state = self.state.borrow_mut();
                        state.current = Some(to);
                        state.elapsed = Duration::ZERO;
                    }
                    self.announce();
                }
            }
            PlayerRequest::LoopStartHere | PlayerRequest::LoopEndHere => {
                let ab = {
                    let mut state = self.state.borrow_mut();
                    let at = state.elapsed;
                    let entry = state.current.and_then(|i| state.queue.get(i)?.entry);
                    let old = state.ab_loop.map(|(p, _)| p).unwrap_or_default();
                    let ab = if matches!(request, PlayerRequest::LoopStartHere) {
                        Some(LoopPoints {
                            start: Some(at),
                            end: old.end.filter(|end| *end > at),
                        })
                    } else if at.is_zero() {
                        None
                    } else {
                        Some(LoopPoints {
                            start: Some(old.start.filter(|s| *s < at).unwrap_or_default()),
                            end: Some(at),
                        })
                    };
                    state.ab_loop = ab.map(|p| (p, entry));
                    ab
                };
                self.emit(Update::Loop(ab));
            }
            PlayerRequest::SetLoop(range) => {
                let ab = {
                    let mut state = self.state.borrow_mut();
                    let entry = state.current.and_then(|i| state.queue.get(i)?.entry);
                    let ab = range.filter(|(s, e)| s < e).map(|(start, end)| LoopPoints {
                        start: Some(start),
                        end: Some(end),
                    });
                    state.ab_loop = ab.map(|p| (p, entry));
                    ab
                };
                self.emit(Update::Loop(ab));
            }
            PlayerRequest::StartSleepTimer { length, when } => {
                let timer = SleepTimer {
                    remaining: length,
                    length,
                    when,
                };
                self.state.borrow_mut().sleep = Some(timer);
                self.emit(Update::SleepTimer(Some(timer)));
            }
            PlayerRequest::ExtendSleepTimer(by) => {
                let timer = {
                    let mut state = self.state.borrow_mut();
                    if let Some(timer) = state.sleep.as_mut() {
                        timer.remaining += by;
                        timer.length += by;
                    }
                    state.sleep
                };
                self.emit(Update::SleepTimer(timer));
            }
            PlayerRequest::CancelSleepTimer => {
                self.state.borrow_mut().sleep = None;
                self.emit(Update::SleepTimer(None));
            }
            PlayerRequest::SetTrackFlags { track, flags } => {
                self.library
                    .flags
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(track, flags);
                self.emit(Update::TrackFlags { track, flags });
            }
            _ => {}
        }
    }

    fn change_modes(&self, change: impl FnOnce(&mut super::PlayModes)) {
        let modes = {
            let mut state = self.state.borrow_mut();
            change(&mut state.modes);
            state.shuffle = state.modes.shuffle != super::ShuffleMode::Off;
            state.repeat = match state.modes.repeat {
                super::RepeatMode::Off => Repeat::Off,
                super::RepeatMode::One => Repeat::One,
                _ => Repeat::All,
            };
            state.modes
        };
        let (shuffle, repeat) = {
            let state = self.state.borrow();
            (state.shuffle, state.repeat)
        };
        self.emit(Update::Modes { shuffle, repeat });
        self.emit(Update::PlayModes(modes));
    }

    /// Ends the A-B loop when another entry plays.
    fn end_loop_if_moved(&self) {
        let ended = {
            let mut state = self.state.borrow_mut();
            let entry = state.current.and_then(|i| state.queue.get(i)?.entry);
            let moved = state.ab_loop.is_some_and(|(_, e)| e != entry);
            if moved {
                state.ab_loop = None;
            }
            moved
        };
        if ended {
            self.emit(Update::Loop(None));
        }
    }
}

/// Where an album command goes in `queue` from `current`: the first track of
/// the next album, of the album before (or this one), or of another album.
fn album_start(queue: &[TrackRow], current: usize, request: &PlayerRequest) -> Option<usize> {
    let album = queue.get(current)?.album;
    let start_of = |i: usize| {
        let a = queue[i].album;
        let mut s = i;
        while s > 0 && queue[s - 1].album == a {
            s -= 1;
        }
        s
    };
    match request {
        PlayerRequest::NextAlbum => (current..queue.len()).find(|&i| queue[i].album != album),
        PlayerRequest::PreviousAlbum => {
            let first = start_of(current);
            Some(if first == 0 { 0 } else { start_of(first - 1) })
        }
        // The sample player picks the next album, or the first when there is none.
        _ => (current..queue.len())
            .find(|&i| queue[i].album != album)
            .or_else(|| {
                (0..current)
                    .find(|&i| queue[i].album != album)
                    .map(start_of)
            }),
    }
}

/// A backend over a [`SampleLibrary`] and a [`FakePlayer`].
pub fn backend(options: &SampleOptions) -> Backend {
    build(options, false).0
}

/// The same backend before the first run: no library folder is chosen yet.
pub fn first_run_backend(options: &SampleOptions) -> Backend {
    build(options, true).0
}

/// The backend of [`backend`] and the sender of its updates, through which a
/// test can play the part of another program (MPRIS, for example).
pub fn backend_with_updates(options: &SampleOptions) -> (Backend, async_channel::Sender<Update>) {
    build(options, false)
}

/// The backend of [`backend_with_updates`] in the middle of a scan: only the
/// first `loaded` tiles of the album list have their rows, the others show
/// placeholders.
pub fn scanning_backend(
    options: &SampleOptions,
    loaded: usize,
) -> (Backend, async_channel::Sender<Update>) {
    let mut library = SampleLibrary::new(options.albums);
    library.loaded = loaded;
    build_over(library, options, false)
}

/// The backend of [`backend`] with a cover on every album: each shows the
/// same thumbnail, a PNG file written into `folder`.
pub fn covered_backend(options: &SampleOptions, folder: &Path) -> Result<Backend, BackendError> {
    let path = folder.join("sample-cover.png");
    write_cover(&path)
        .map_err(|e| BackendError(format!("cannot write {}: {e}", path.display())))?;
    let mut library = SampleLibrary::new(options.albums);
    for album in &mut library.albums {
        let mut key = [0u8; 32];
        key[..8].copy_from_slice(&album.row.key.0.to_le_bytes());
        album.row.cover = Some(CoverKey(key));
        for track in &mut album.tracks {
            track.cover = album.row.cover;
        }
    }
    library.cover_file = Some(path);
    Ok(build_over(library, options, false).0)
}

/// A 192 px square in two colours, as a thumbnail file.
fn write_cover(path: &Path) -> Result<(), gtk::glib::BoolError> {
    const SIDE: usize = 192;
    let mut pixels = Vec::with_capacity(SIDE * SIDE * 4);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let on = (x / 24 + y / 24) % 2 == 0;
            pixels.extend_from_slice(if on {
                &[0x2E, 0x8B, 0x57, 0xFF]
            } else {
                &[0xF4, 0xF1, 0xDE, 0xFF]
            });
        }
    }
    let side = i32::try_from(SIDE).unwrap_or(192);
    let texture = gtk::gdk::MemoryTexture::new(
        side,
        side,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &gtk::glib::Bytes::from_owned(pixels),
        SIDE * 4,
    );
    gtk::gdk::prelude::TextureExt::save_to_png(&texture, path)
}

fn build(options: &SampleOptions, first_run: bool) -> (Backend, async_channel::Sender<Update>) {
    build_over(SampleLibrary::new(options.albums), options, first_run)
}

fn build_over(
    library: SampleLibrary,
    options: &SampleOptions,
    first_run: bool,
) -> (Backend, async_channel::Sender<Update>) {
    let (sender, receiver) = async_channel::unbounded();
    *library.rated.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender.clone());
    let library = Arc::new(library);
    let mut services = MemoryServices::for_library(library.clone(), sender.clone());
    if first_run {
        services = services.empty();
    }
    let player = Rc::new(FakePlayer {
        library: library.clone(),
        state: RefCell::new(FakeState::default()),
        updates: sender.clone(),
    });
    player.emit(Update::Volume(0.8));
    player.emit(Update::Output(player.output(SignalState::Idle)));
    if let Some((position, track, elapsed)) = options.playing {
        let order = library.sorted(&AlbumQuery::default());
        if let Some(album) = order.get(position) {
            player.send(PlayerRequest::PlayAlbum {
                album: album.row.key,
                start: track,
                shuffle: false,
            });
            player.send(PlayerRequest::Seek(Duration::from_secs(elapsed)));
        }
    }
    (
        Backend::new(library, player, receiver).with_services(Arc::new(services)),
        sender,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_is_stable_and_sorted() {
        let a = SampleLibrary::new(50);
        let b = SampleLibrary::new(50);
        let query = AlbumQuery::default();
        assert_eq!(a.album_keys(&query), b.album_keys(&query));
        let keys = a.album_keys(&query).unwrap();
        let rows = a.album_rows(&query, 0, keys.len()).unwrap();
        assert_eq!(rows.iter().map(|r| r.key).collect::<Vec<_>>(), keys);
        assert!(rows.windows(2).all(|w| w[0].artist <= w[1].artist));
        let hires = AlbumQuery {
            filter: AlbumFilter::HiRes,
            ..query
        };
        for row in a.album_rows(&hires, 0, 1000).unwrap() {
            assert!(row.format.bits.is_some());
        }
    }

    #[test]
    fn the_fake_player_follows_requests() {
        let backend = backend(&SampleOptions {
            albums: 10,
            playing: Some((0, 1, 30)),
        });
        let updates = backend.take_updates().unwrap();
        let mut now = None;
        while let Ok(update) = updates.try_recv() {
            if let Update::NowPlaying(n) = update {
                now = n;
            }
        }
        assert_eq!(now.unwrap().track.number, Some(2));
        backend.send(PlayerRequest::Next);
        let mut seen = false;
        while let Ok(update) = updates.try_recv() {
            if let Update::NowPlaying(Some(n)) = update {
                seen = n.track.number == Some(3);
            }
        }
        assert!(seen);
    }
}
