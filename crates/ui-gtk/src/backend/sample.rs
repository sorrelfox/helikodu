// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A backend over invented content: a generated library and a player that only
//! pretends to play. For the widget tests, the screenshots and `--demo`.
//!
//! Every name, count and length here is made up from the word lists below and
//! a fixed seed, so two runs show the same library.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use super::{
    AlbumDetail, AlbumFilter, AlbumKey, AlbumQuery, AlbumRow, AlbumSort, AudioFormat, Backend,
    BackendError, DiscGroup, LibraryCounts, LibrarySource, NowPlaying, OutputInfo, PlaybackState,
    PlayerControl, PlayerRequest, QueueSnapshot, Repeat, SignalState, TrackKey, TrackRow, Update,
};

/// What the sample backend contains and does at the start.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleOptions {
    /// The number of albums.
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
const GENRES: &[&str] = &["Ambient", "Folk", "Jazz", "Electronic", "Classical", "Rock"];

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
    added: u64,
    genre: &'static str,
    tracks: Vec<TrackRow>,
}

/// The invented library.
#[derive(Debug)]
pub struct SampleLibrary {
    albums: Vec<SampleAlbum>,
    artists: usize,
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

impl SampleLibrary {
    /// A library of `albums` invented albums.
    pub fn new(albums: usize) -> Self {
        let mut r = Random(0x4845_4C49);
        let mut artists = Vec::new();
        for _ in 0..(albums / 3).max(1) {
            let name = if r.below(4) == 0 {
                format!("The {} {}", r.pick(NOUNS), r.pick(ENSEMBLES))
            } else {
                format!("{} {}", r.pick(FIRST), r.pick(LAST))
            };
            artists.push(name);
        }
        let mut list = Vec::with_capacity(albums);
        let mut track_key = 1;
        for index in 0..albums {
            let key = AlbumKey(index as u64 + 1);
            let title = match r.below(3) {
                0 => format!("{} {}", r.pick(ADJECTIVES), r.pick(NOUNS)),
                1 => format!("{} of {}", r.pick(NOUNS), r.pick(NOUNS)),
                _ => format!("{} {}", r.pick(ADJECTIVES), r.pick(WORDS)),
            };
            let artist = artists[r.below(artists.len())].clone();
            let year = 1972 + r.below(55) as i32;
            let format = format(&mut r);
            let discs = if r.below(12) == 0 { 2 } else { 1 };
            let mut tracks = Vec::new();
            for disc in 1..=discs {
                for number in 1..=(5 + r.below(8)) as u32 {
                    tracks.push(TrackRow {
                        key: TrackKey(track_key),
                        album: key,
                        number: Some(number),
                        disc: (discs > 1).then_some(disc),
                        title: format!("{} {}", r.pick(ADJECTIVES), r.pick(WORDS)),
                        artist: artist.clone(),
                        duration: Some(Duration::from_secs(120 + r.below(330) as u64)),
                        plays: r.below(60) as u32,
                        rating: (r.below(5) != 0).then(|| 1 + r.below(5) as u8),
                        entry: None,
                        from_context: true,
                        cover: None,
                    });
                    track_key += 1;
                }
            }
            list.push(SampleAlbum {
                row: AlbumRow {
                    key,
                    title,
                    artist,
                    year: Some(year),
                    format,
                    track_count: tracks.len(),
                    cover_seed: r.next(),
                    cover: None,
                },
                added: r.next() % 1_000_000,
                genre: r.pick(GENRES),
                tracks,
            });
        }
        Self {
            albums: list,
            artists: artists.len(),
        }
    }

    fn get(&self, key: AlbumKey) -> Option<&SampleAlbum> {
        let index = usize::try_from(key.0).ok()?.checked_sub(1)?;
        self.albums.get(index)
    }

    fn sorted(&self, query: &AlbumQuery) -> Vec<&SampleAlbum> {
        let mut albums: Vec<&SampleAlbum> = self
            .albums
            .iter()
            .filter(|a| {
                let f = &a.row.format;
                let lossless = f.bits.is_some();
                match query.filter {
                    AlbumFilter::All => true,
                    AlbumFilter::Lossless => lossless,
                    AlbumFilter::HiRes => {
                        lossless
                            && (f.sample_rate.unwrap_or(0) > 48_000 || f.bits.unwrap_or(0) > 16)
                    }
                }
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
            match discs.last_mut() {
                Some(group) if group.number == track.disc => group.tracks.push(track.clone()),
                _ => discs.push(DiscGroup {
                    number: track.disc,
                    subtitle: None,
                    tracks: vec![track.clone()],
                }),
            }
        }
        let artist_folder = album.row.artist.replace(' ', "_");
        Some(AlbumDetail {
            row: album.row.clone(),
            discs,
            genre: Some(album.genre.to_string()),
            added: None,
            size: Some(album.tracks.len() as u64 * 31_000_000),
            folder: Some(format!(
                "~/Music/{artist_folder}/{} {}",
                album.row.year.unwrap_or(0),
                album.row.title
            )),
            album_gain_db: Some(-6.3),
        })
    }
}

impl LibrarySource for SampleLibrary {
    fn counts(&self) -> Result<LibraryCounts, BackendError> {
        Ok(LibraryCounts {
            albums: self.albums.len(),
            artists: self.artists,
            tracks: self.albums.iter().map(|a| a.tracks.len()).sum(),
            genres: GENRES.len(),
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

impl PlayerControl for FakePlayer {
    fn send(&self, request: PlayerRequest) {
        match request {
            PlayerRequest::PlayAlbum { album, start, .. } => {
                let Some(detail) = self.library.detail(album) else {
                    return;
                };
                {
                    let mut state = self.state.borrow_mut();
                    state.queue = detail
                        .tracks()
                        .cloned()
                        .enumerate()
                        .map(|(i, mut t)| {
                            t.entry = Some(i as u64 + 1);
                            t
                        })
                        .collect();
                    state.context = Some(detail.row.title.clone());
                    state.current = Some(start.min(state.queue.len().saturating_sub(1)));
                    state.state = PlaybackState::Playing;
                    state.elapsed = Duration::ZERO;
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
            PlayerRequest::PlayFiles { .. } => self.emit(Update::Notice {
                message: "The sample player plays no files.".into(),
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
    let library = Arc::new(SampleLibrary::new(options.albums));
    let (sender, receiver) = async_channel::unbounded();
    let player = Rc::new(FakePlayer {
        library: library.clone(),
        state: RefCell::new(FakeState::default()),
        updates: sender,
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
    Backend::new(library, player, receiver)
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
