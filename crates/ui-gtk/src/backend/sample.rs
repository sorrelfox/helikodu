// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A backend over invented content: a generated library and a player that only
//! pretends to play. For the widget tests, the screenshots and `--demo`.
//!
//! Every name, count and length here is made up from the word lists below and
//! a fixed seed, so two runs show the same library. Its files do not exist; the
//! folders are made up from the tracks' paths.

mod services;

pub use services::MemoryServices;

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use super::{
    AlbumDetail, AlbumFilter, AlbumKey, AlbumQuery, AlbumRow, AlbumScope, AlbumSort, ArtistDetail,
    ArtistKey, ArtistRow, AudioFormat, Backend, BackendError, DiscGroup, FolderFile, FolderListing,
    FolderRow, FolderState, GenreKey, GenreRow, LibraryCounts, LibrarySource, NowPlaying,
    OutputInfo, PlaybackState, PlayerControl, PlayerRequest, PlaylistKey, PlaylistRow,
    QueueSnapshot, Repeat, SearchResults, SignalState, TrackColumn, TrackKey, TrackQuery, TrackRow,
    TrackScope, TrackSort, Update,
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

#[derive(Debug, Clone)]
enum Entry {
    Track(TrackKey),
    Missing(Box<TrackRow>),
}

#[derive(Debug, Clone)]
struct SamplePlaylist {
    key: PlaylistKey,
    name: String,
    entries: Vec<Entry>,
    modified: SystemTime,
}

#[derive(Debug, Default)]
struct Playlists {
    next: u64,
    lists: Vec<SamplePlaylist>,
}

/// The invented library.
#[derive(Debug)]
pub struct SampleLibrary {
    albums: Vec<SampleAlbum>,
    artists: Vec<String>,
    /// Each track's album and index in it, by key - 1.
    tracks: Vec<(u32, u16)>,
    ratings: Mutex<HashMap<TrackKey, Option<u8>>>,
    playlists: Mutex<Playlists>,
    roots: Vec<(PathBuf, FolderState)>,
    /// The tiles past this position stay unloaded, as while a scan runs.
    loaded: usize,
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
            playlists: Mutex::new(Playlists::default()),
            roots,
            loaded: usize::MAX,
        };
        library.add_sample_playlists(&mut r);
        library
    }

    fn add_sample_playlists(&self, r: &mut Random) {
        if self.tracks.is_empty() {
            return;
        }
        let total = self.tracks.len() as u64;
        let mut lists = self.lists();
        for (name, count) in [("Favourites", 214), ("Workouts", 6), ("Late evening", 48)] {
            lists.next += 1;
            let mut entries: Vec<Entry> = (0..count.min(self.tracks.len()))
                .map(|_| Entry::Track(TrackKey(1 + r.next() % total)))
                .collect();
            if name == "Late evening" && entries.len() > 4 {
                entries[4] = Entry::Missing(Box::new(TrackRow {
                    title: "Kopu Light".into(),
                    artist: "Mari Kask".into(),
                    album_title: "Letters from Hiiumaa".into(),
                    duration: Some(Duration::from_secs(326)),
                    missing: true,
                    ..TrackRow::default()
                }));
            }
            let key = PlaylistKey(lists.next);
            lists.lists.push(SamplePlaylist {
                key,
                name: name.into(),
                entries,
                modified: SystemTime::now(),
            });
        }
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

    fn track(&self, key: TrackKey) -> Option<TrackRow> {
        let index = usize::try_from(key.0).ok()?.checked_sub(1)?;
        let (album, track) = self.tracks.get(index)?;
        let mut row = self.albums[*album as usize].tracks[*track as usize].clone();
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
        Ok(())
    }

    fn playlists(&self) -> Result<Vec<PlaylistRow>, BackendError> {
        let lists = self.lists();
        Ok(lists
            .lists
            .iter()
            .map(|p| PlaylistRow {
                key: p.key,
                name: p.name.clone(),
                items: p.entries.len(),
                duration: p
                    .entries
                    .iter()
                    .filter_map(|e| match e {
                        Entry::Track(k) => self.track(*k).and_then(|t| t.duration),
                        Entry::Missing(t) => t.duration,
                    })
                    .sum(),
                modified: Some(p.modified),
            })
            .collect())
    }

    fn playlist_items(&self, key: PlaylistKey) -> Result<Vec<TrackRow>, BackendError> {
        let lists = self.lists();
        let list = lists
            .lists
            .iter()
            .find(|p| p.key == key)
            .ok_or_else(|| BackendError("no such playlist".into()))?;
        Ok(list
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut row = match e {
                    Entry::Track(k) => self.track(*k).unwrap_or_default(),
                    Entry::Missing(t) => (**t).clone(),
                };
                row.entry = Some(i as u64);
                row.from_context = true;
                row
            })
            .collect())
    }

    fn create_playlist(&self, name: &str) -> Result<PlaylistKey, BackendError> {
        let mut lists = self.lists();
        lists.next += 1;
        let key = PlaylistKey(lists.next);
        lists.lists.push(SamplePlaylist {
            key,
            name: name.to_owned(),
            entries: Vec::new(),
            modified: SystemTime::now(),
        });
        Ok(key)
    }

    fn rename_playlist(&self, key: PlaylistKey, name: &str) -> Result<(), BackendError> {
        self.edit(key, |p| {
            p.name = name.to_owned();
            Ok(())
        })
    }

    fn delete_playlist(&self, key: PlaylistKey) -> Result<(), BackendError> {
        self.lists().lists.retain(|p| p.key != key);
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

    fn import_playlist(&self, path: &Path) -> Result<(PlaylistKey, String), BackendError> {
        let name = path.file_stem().map_or_else(
            || "Imported".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let key = self.create_playlist(&name)?;
        Ok((key, name))
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
}

impl SampleLibrary {
    fn edit(
        &self,
        key: PlaylistKey,
        change: impl FnOnce(&mut SamplePlaylist) -> Result<(), BackendError>,
    ) -> Result<(), BackendError> {
        let mut lists = self.lists();
        let list = lists
            .lists
            .iter_mut()
            .find(|p| p.key == key)
            .ok_or_else(|| BackendError("no such playlist".into()))?;
        change(list)?;
        list.modified = SystemTime::now();
        Ok(())
    }

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
    shuffle: bool,
    repeat: Repeat,
    stop_after: bool,
    queue: Vec<TrackRow>,
    context: Option<String>,
    current: Option<usize>,
    state: PlaybackState,
    elapsed: Duration,
    next_entry: u64,
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
        self.emit(Update::Position { elapsed, length });
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
            }
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
                    state.shuffle
                };
                self.emit(Update::Modes { shuffle, repeat });
            }
            PlayerRequest::SetStopAfter(on) => {
                self.state.borrow_mut().stop_after = on;
                self.emit(Update::StopAfter(on));
            }
        }
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

fn build(options: &SampleOptions, first_run: bool) -> (Backend, async_channel::Sender<Update>) {
    build_over(SampleLibrary::new(options.albums), options, first_run)
}

fn build_over(
    library: SampleLibrary,
    options: &SampleOptions,
    first_run: bool,
) -> (Backend, async_channel::Sender<Update>) {
    let library = Arc::new(library);
    let (sender, receiver) = async_channel::unbounded();
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
