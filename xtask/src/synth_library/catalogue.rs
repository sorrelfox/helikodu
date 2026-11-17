// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The made-up music collection: albums, their tracks, tags and paths.
//!
//! Everything follows from the seed and the track count. Artists are drawn
//! with a Zipf-like law, so a few have dozens of albums and most have one or
//! two. Albums are regular releases, compilations, classical recordings or
//! singles and EPs; some span several discs. The shares are constants below,
//! and the unit tests check that a generated collection hits them.

use std::collections::HashSet;

use super::Format;
use super::paths::{self, Legacy};
use super::random::{Rng, Zipf};
use super::words::{self, Name, Script};

/// Album kinds and how often each occurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Regular,
    Compilation,
    Classical,
    Single,
}

const KINDS: [(Kind, u32); 4] = [
    (Kind::Regular, 74),
    (Kind::Compilation, 9),
    (Kind::Classical, 9),
    (Kind::Single, 8),
];

/// Share of albums (other than singles) on more than one disc.
pub(crate) const MULTI_DISC_SHARE: f64 = 0.08;
/// Share of tracks on regular albums with a second, featured artist.
const FEATURED_SHARE: f64 = 0.08;
/// Share of albums whose genre field has several values.
const MULTI_GENRE_SHARE: f64 = 0.2;
/// Share of albums with ReplayGain (FLAC) or R128 (Opus) gain tags.
pub(crate) const GAIN_SHARE: f64 = 0.35;
/// Share of albums by artists with Latin or Cyrillic names whose folder and
/// file names are written in Latin-1 or cp1251 rather than UTF-8.
const LEGACY_NAME_SHARE: f64 = 0.12;
/// Share of tracks with no tags at all (only the file name tells).
pub(crate) const UNTAGGED_SHARE: f64 = 0.005;
const NO_TITLE_SHARE: f64 = 0.02;
const NO_ARTIST_SHARE: f64 = 0.015;
const NO_ALBUM_SHARE: f64 = 0.02;
const NO_TRACK_NUMBER_SHARE: f64 = 0.03;
/// Share of albums tagged with MusicBrainz IDs.
pub(crate) const MUSICBRAINZ_SHARE: f64 = 0.25;
/// Share of person artists whose sort name ("Family, Given") is tagged.
const PERSON_SORT_SHARE: f64 = 0.4;

/// The album artist of compilations.
pub(crate) const VARIOUS_ARTISTS: &str = "Various Artists";

/// One file.
#[derive(Debug, Clone)]
pub(crate) struct Track {
    /// The path relative to the library folder, as bytes: not always UTF-8.
    pub(crate) path: Vec<u8>,
    /// Vorbis comment fields in the order they are written; a key that
    /// appears more than once is a multi-value tag.
    pub(crate) tags: Vec<(&'static str, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct Album {
    pub(crate) kind: Kind,
    pub(crate) discs: u32,
    pub(crate) tracks: Vec<Track>,
}

/// The people and bands albums are drawn from.
#[derive(Debug)]
struct Pools {
    artists: Vec<Name>,
    artist_zipf: Zipf,
    composers: Vec<Name>,
    composer_zipf: Zipf,
    conductors: Vec<Name>,
    soloists: Vec<(Name, &'static str)>,
    ensembles: Vec<String>,
}

impl Pools {
    fn new(rng: &mut Rng, tracks: usize) -> Self {
        // About 30 tracks per artist, 2000 per composer: the proportions of
        // a large personal collection.
        let artist_count = (tracks / 30).max(20);
        let artists = (0..artist_count)
            .map(|_| {
                let script = rng.weighted(&Script::WEIGHTS);
                if rng.chance(0.55) {
                    words::band(rng, script)
                } else {
                    words::person(rng, script, PERSON_SORT_SHARE)
                }
            })
            .collect();
        let classical_script = |rng: &mut Rng| {
            rng.weighted(&[
                (Script::German, 6),
                (Script::OtherLatin, 4),
                (Script::Cyrillic, 3),
                (Script::English, 3),
                (Script::Estonian, 2),
                (Script::Swedish, 2),
                (Script::Japanese, 1),
            ])
        };
        let composer_count = (tracks / 2000).max(10);
        let composers = (0..composer_count)
            .map(|_| {
                let script = classical_script(rng);
                words::person(rng, script, 0.8)
            })
            .collect();
        let conductors = (0..(tracks / 4000).max(5))
            .map(|_| {
                let script = classical_script(rng);
                words::person(rng, script, 0.3)
            })
            .collect();
        let soloists = (0..(tracks / 2000).max(10))
            .map(|_| {
                let script = classical_script(rng);
                (
                    words::person(rng, script, 0.3),
                    *rng.pick(words::INSTRUMENTS),
                )
            })
            .collect();
        let ensembles = (0..20)
            .map(|_| {
                format!(
                    "{} {}",
                    rng.pick(words::CITIES),
                    rng.pick(words::ENSEMBLE_KINDS)
                )
            })
            .collect();
        Self {
            artists,
            artist_zipf: Zipf::new(artist_count, 8.0),
            composers,
            composer_zipf: Zipf::new(composer_count, 2.0),
            conductors,
            soloists,
            ensembles,
        }
    }

    fn artist(&self, rng: &mut Rng) -> &Name {
        &self.artists[self.artist_zipf.sample(rng)]
    }
}

/// Yields albums until the track count is reached.
#[derive(Debug)]
pub(crate) struct Catalogue {
    rng: Rng,
    format: Format,
    remaining: usize,
    pools: Pools,
    used_folders: HashSet<Vec<u8>>,
    compilation_volume: u32,
}

impl Catalogue {
    pub(crate) fn new(seed: u64, tracks: usize, format: Format) -> Self {
        let mut rng = Rng::new(seed);
        let pools = Pools::new(&mut rng, tracks);
        Self {
            rng,
            format,
            remaining: tracks,
            pools,
            used_folders: HashSet::new(),
            compilation_volume: 0,
        }
    }
}

impl Iterator for Catalogue {
    type Item = Album;

    fn next(&mut self) -> Option<Album> {
        if self.remaining == 0 {
            return None;
        }
        let album = self.album();
        self.remaining -= album.tracks.len();
        Some(album)
    }
}

/// What every track of an album shares.
#[derive(Debug, Default)]
struct AlbumTags {
    album: String,
    album_artists: Vec<Name>,
    date: Option<String>,
    original_date: Option<String>,
    genres: Vec<String>,
    label: Option<String>,
    release_type: Option<&'static str>,
    /// The MusicBrainz release and release group IDs, on a share of the albums
    /// ([`MUSICBRAINZ_SHARE`]); their tracks also get recording IDs.
    musicbrainz: Option<(String, String)>,
    compilation: bool,
    /// ReplayGain album gain in dB, and peak.
    gain: Option<(f64, f64)>,
    write_album_artist: bool,
    write_album: bool,
    write_track_numbers: bool,
    /// "3/12" in TRACKNUMBER rather than a separate TRACKTOTAL.
    slashed_numbers: bool,
    write_disc_numbers: bool,
}

/// What differs per track.
#[derive(Debug, Default)]
struct TrackTags {
    title: String,
    artists: Vec<Name>,
    composer: Option<Name>,
    conductor: Option<Name>,
    performers: Vec<String>,
    work: Option<(String, u32, u32, String)>,
}

impl Catalogue {
    fn album(&mut self) -> Album {
        let kind = self.rng.weighted(&KINDS);
        let discs = if kind != Kind::Single && self.rng.chance(MULTI_DISC_SHARE) {
            self.rng.weighted(&[(2, 70), (3, 20), (4, 10)])
        } else {
            1
        };
        let (low, high) = match kind {
            Kind::Regular => (7, 15),
            Kind::Compilation => (12, 20),
            Kind::Classical => (3, 10),
            Kind::Single => (1, 4),
        };
        let mut per_disc: Vec<u32> = (0..discs).map(|_| self.rng.range(low, high)).collect();
        // The last album takes only the tracks that are left.
        let mut left = self.remaining;
        for count in &mut per_disc {
            let take = (*count as usize).min(left);
            *count = take as u32;
            left -= take;
        }
        per_disc.retain(|&count| count > 0);
        let discs = per_disc.len() as u32;

        let mut shared = AlbumTags {
            write_album_artist: true,
            write_album: !self.rng.chance(NO_ALBUM_SHARE),
            write_track_numbers: !self.rng.chance(NO_TRACK_NUMBER_SHARE),
            slashed_numbers: self.rng.chance(0.1),
            write_disc_numbers: discs > 1 || self.rng.chance(0.3),
            musicbrainz: if self.rng.chance(MUSICBRAINZ_SHARE) {
                Some((self.rng.uuid(), self.rng.uuid()))
            } else {
                None
            },
            ..AlbumTags::default()
        };
        let year = 2025 - self.rng.below(66).min(self.rng.below(66)) as u32;
        shared.date = match self.rng.weighted(&[(0, 65), (1, 25), (2, 10)]) {
            0 => Some(year.to_string()),
            1 => Some(format!(
                "{year}-{:02}-{:02}",
                self.rng.range(1, 12),
                self.rng.range(1, 28)
            )),
            _ => None,
        };
        if year > 1980 && self.rng.chance(0.12) {
            shared.original_date = Some((year - self.rng.range(5, 25)).to_string());
        }
        if self.rng.chance(0.3) {
            shared.label = Some((*self.rng.pick(words::LABELS)).to_owned());
        }
        if self.rng.chance(GAIN_SHARE) {
            shared.gain = Some((
                -f64::from(self.rng.range(100, 1200)) / 100.0,
                f64::from(self.rng.range(800_000, 1_000_000)) / 1_000_000.0,
            ));
        }

        // Per kind: the album's names and a function for each track's.
        let total_tracks: u32 = per_disc.iter().sum();
        let mut per_track: Vec<TrackTags> = Vec::with_capacity(total_tracks as usize);
        let folder_artist: String;
        let mut legacy = None;
        match kind {
            Kind::Regular | Kind::Single => {
                let artist = self.pools.artist(&mut self.rng).clone();
                legacy = self.legacy_for(artist.script);
                shared.album_artists = vec![artist.clone()];
                shared.write_album_artist = self.rng.chance(0.7);
                shared.genres = self.genres();
                for _ in 0..total_tracks {
                    let mut artists = vec![artist.clone()];
                    if kind == Kind::Regular && self.rng.chance(FEATURED_SHARE) {
                        artists.push(self.pools.artist(&mut self.rng).clone());
                    }
                    let title = self.title(artist.script);
                    per_track.push(TrackTags {
                        title,
                        artists,
                        ..TrackTags::default()
                    });
                }
                shared.album = if kind == Kind::Single {
                    if total_tracks > 2 {
                        shared.release_type = Some("ep");
                        format!("{} EP", per_track[0].title)
                    } else {
                        shared.release_type = Some("single");
                        per_track[0].title.clone()
                    }
                } else if self.rng.chance(0.08) {
                    (*self.rng.pick(words::COMMON_ALBUM_TITLES)).to_owned()
                } else {
                    self.title(artist.script)
                };
                folder_artist = artist.text.clone();
            }
            Kind::Compilation => {
                self.compilation_volume += 1;
                shared.compilation = true;
                shared.album_artists = vec![Name {
                    text: String::from(VARIOUS_ARTISTS),
                    sort: None,
                    script: Script::English,
                }];
                shared.album = format!(
                    "{} {} Vol. {}",
                    self.rng.pick(words::COMPILATION_WORDS),
                    self.rng.pick(words::COMPILATION_WORDS),
                    self.compilation_volume
                );
                shared.genres = self.genres();
                for _ in 0..total_tracks {
                    let artist = self.pools.artist(&mut self.rng).clone();
                    let title = self.title(artist.script);
                    per_track.push(TrackTags {
                        title,
                        artists: vec![artist],
                        ..TrackTags::default()
                    });
                }
                folder_artist = String::from(VARIOUS_ARTISTS);
            }
            Kind::Classical => {
                let main_composer = self.composer();
                let conductor = self.rng.pick(&self.pools.conductors).clone();
                let ensemble = Name {
                    text: self.rng.pick(&self.pools.ensembles).clone(),
                    sort: None,
                    script: Script::English,
                };
                let soloist = self
                    .rng
                    .chance(0.5)
                    .then(|| self.rng.pick(&self.pools.soloists).clone());
                shared.genres = vec![String::from("Classical")];
                if self.rng.chance(0.5) {
                    shared
                        .genres
                        .push((*self.rng.pick(words::CLASSICAL_GENRES)).to_owned());
                }
                let mut works = Vec::new();
                while (per_track.len() as u32) < total_tracks {
                    let composer = if self.rng.chance(0.7) {
                        main_composer.clone()
                    } else {
                        self.composer()
                    };
                    let form = *self.rng.pick(words::FORMS);
                    let work = format!(
                        "{form} No. {} in {}, Op. {}",
                        self.rng.range(1, 9),
                        self.rng.pick(words::KEYS),
                        self.rng.range(1, 130)
                    );
                    let movements = self.rng.range(1, 4);
                    for movement in 1..=movements {
                        if per_track.len() as u32 == total_tracks {
                            break;
                        }
                        let tempo = (*self.rng.pick(words::TEMPI)).to_owned();
                        let mut artists = vec![ensemble.clone(), conductor.clone()];
                        let mut performers = vec![format!("{} (orchestra)", ensemble.text)];
                        if let Some((soloist, instrument)) = &soloist {
                            artists.insert(0, soloist.clone());
                            performers.push(format!("{} ({instrument})", soloist.text));
                        }
                        per_track.push(TrackTags {
                            title: format!("{work}: {}. {tempo}", roman(movement)),
                            artists,
                            composer: Some(composer.clone()),
                            conductor: Some(conductor.clone()),
                            performers,
                            work: Some((work.clone(), movement, movements, tempo)),
                        });
                    }
                    works.push(form);
                }
                // Composer-first tagging (album artist = composer) or
                // performer-first (conductor and orchestra as two values).
                if self.rng.chance(0.4) {
                    shared.album_artists = vec![main_composer.clone()];
                    folder_artist = main_composer.text.clone();
                } else {
                    shared.album_artists = vec![conductor.clone(), ensemble.clone()];
                    folder_artist = conductor.text.clone();
                }
                shared.album = format!("{}: {}", main_composer.text, works.join(", "));
                legacy = self.legacy_for(main_composer.script);
            }
        }

        let folder = self.album_folder(&folder_artist, &shared, year, legacy);
        let disc_folders = discs > 1 && self.rng.chance(0.6);
        let extension = self.format.extension();

        let mut tracks = Vec::with_capacity(per_track.len());
        let mut index = 0;
        for (disc_index, &count) in per_disc.iter().enumerate() {
            let disc = disc_index as u32 + 1;
            for number in 1..=count {
                let track = &per_track[index];
                index += 1;
                let tags = self.tags(&shared, track, number, count, disc, discs);
                let stem = if discs > 1 && !disc_folders {
                    format!("{disc}-{number:02} - {}", track.title)
                } else {
                    format!("{number:02} - {}", track.title)
                };
                let mut path = folder.clone();
                if disc_folders {
                    path.extend_from_slice(format!("/Disc {disc}").as_bytes());
                }
                path.push(b'/');
                path.extend_from_slice(&legacy_bytes(&paths::component(&stem), legacy));
                path.push(b'.');
                path.extend_from_slice(extension.as_bytes());
                tracks.push(Track { path, tags });
            }
        }
        Album {
            kind,
            discs,
            tracks,
        }
    }

    fn composer(&mut self) -> Name {
        self.pools.composers[self.pools.composer_zipf.sample(&mut self.rng)].clone()
    }

    /// A title in the artist's script, or sometimes in English.
    fn title(&mut self, script: Script) -> String {
        let script = if script != Script::English && self.rng.chance(0.25) {
            Script::English
        } else {
            script
        };
        words::phrase(&mut self.rng, script, 1, 4)
    }

    fn genres(&mut self) -> Vec<String> {
        let count = if self.rng.chance(MULTI_GENRE_SHARE) {
            self.rng.range(2, 3)
        } else {
            1
        };
        let mut genres: Vec<String> = Vec::new();
        while genres.len() < count as usize {
            let genre = (*self.rng.pick(words::GENRES)).to_owned();
            if !genres.contains(&genre) {
                genres.push(genre);
            }
        }
        genres
    }

    fn legacy_for(&mut self, script: Script) -> Option<Legacy> {
        let legacy = match script {
            Script::Estonian | Script::Swedish | Script::German | Script::OtherLatin => {
                Legacy::Latin1
            }
            Script::Cyrillic => Legacy::Cp1251,
            _ => return None,
        };
        self.rng.chance(LEGACY_NAME_SHARE).then_some(legacy)
    }

    /// "Artist/Year - Album", unique in the collection.
    fn album_folder(
        &mut self,
        artist: &str,
        shared: &AlbumTags,
        year: u32,
        legacy: Option<Legacy>,
    ) -> Vec<u8> {
        let album = match &shared.date {
            Some(_) => format!("{year} - {}", shared.album),
            None => shared.album.clone(),
        };
        let mut attempt = 1;
        loop {
            let name = if attempt == 1 {
                album.clone()
            } else {
                format!("{album} ({attempt})")
            };
            let mut folder = paths::component(artist).into_bytes();
            folder.push(b'/');
            folder.extend_from_slice(&legacy_bytes(&paths::component(&name), legacy));
            if self.used_folders.insert(folder.clone()) {
                return folder;
            }
            attempt += 1;
        }
    }

    fn tags(
        &mut self,
        shared: &AlbumTags,
        track: &TrackTags,
        number: u32,
        count: u32,
        disc: u32,
        discs: u32,
    ) -> Vec<(&'static str, String)> {
        let mut tags: Vec<(&'static str, String)> = Vec::with_capacity(24);
        if self.rng.chance(UNTAGGED_SHARE) {
            return tags;
        }
        if !self.rng.chance(NO_TITLE_SHARE) {
            tags.push(("TITLE", track.title.clone()));
        }
        if !self.rng.chance(NO_ARTIST_SHARE) {
            for artist in &track.artists {
                tags.push(("ARTIST", artist.text.clone()));
            }
            for artist in &track.artists {
                if let Some(sort) = &artist.sort {
                    tags.push(("ARTISTSORT", sort.clone()));
                }
            }
        }
        if shared.write_album {
            tags.push(("ALBUM", shared.album.clone()));
        }
        if shared.write_album_artist {
            for artist in &shared.album_artists {
                tags.push(("ALBUMARTIST", artist.text.clone()));
            }
            for artist in &shared.album_artists {
                if let Some(sort) = &artist.sort {
                    tags.push(("ALBUMARTISTSORT", sort.clone()));
                }
            }
        }
        if shared.compilation {
            tags.push(("COMPILATION", String::from("1")));
        }
        if shared.write_track_numbers {
            if shared.slashed_numbers {
                tags.push(("TRACKNUMBER", format!("{number}/{count}")));
            } else {
                tags.push(("TRACKNUMBER", number.to_string()));
                tags.push(("TRACKTOTAL", count.to_string()));
            }
        }
        if shared.write_disc_numbers {
            tags.push(("DISCNUMBER", disc.to_string()));
            tags.push(("DISCTOTAL", discs.to_string()));
        }
        if let Some(date) = &shared.date {
            tags.push(("DATE", date.clone()));
        }
        if let Some(date) = &shared.original_date {
            tags.push(("ORIGINALDATE", date.clone()));
        }
        for genre in &shared.genres {
            tags.push(("GENRE", genre.clone()));
        }
        if let Some(composer) = &track.composer {
            tags.push(("COMPOSER", composer.text.clone()));
            if let Some(sort) = &composer.sort {
                tags.push(("COMPOSERSORT", sort.clone()));
            }
        }
        if let Some(conductor) = &track.conductor {
            tags.push(("CONDUCTOR", conductor.text.clone()));
        }
        for performer in &track.performers {
            tags.push(("PERFORMER", performer.clone()));
        }
        if let Some((work, movement, movements, name)) = &track.work {
            tags.push(("WORK", work.clone()));
            tags.push(("MOVEMENTNAME", name.clone()));
            tags.push(("MOVEMENT", movement.to_string()));
            tags.push(("MOVEMENTTOTAL", movements.to_string()));
        }
        if let Some(label) = &shared.label {
            tags.push(("LABEL", label.clone()));
        }
        if let Some(release_type) = shared.release_type {
            tags.push(("RELEASETYPE", release_type.to_owned()));
        }
        if let Some((release, release_group)) = &shared.musicbrainz {
            tags.push(("MUSICBRAINZ_TRACKID", self.rng.uuid()));
            tags.push(("MUSICBRAINZ_ALBUMID", release.clone()));
            tags.push(("MUSICBRAINZ_RELEASEGROUPID", release_group.clone()));
        }
        if let Some((album_gain, album_peak)) = shared.gain {
            let track_gain = album_gain + f64::from(self.rng.range(0, 400)) / 100.0 - 2.0;
            let track_peak =
                (album_peak - f64::from(self.rng.range(0, 100_000)) / 1_000_000.0).max(0.1);
            match self.format {
                Format::Flac => {
                    tags.push(("REPLAYGAIN_TRACK_GAIN", format!("{track_gain:.2} dB")));
                    tags.push(("REPLAYGAIN_TRACK_PEAK", format!("{track_peak:.6}")));
                    tags.push(("REPLAYGAIN_ALBUM_GAIN", format!("{album_gain:.2} dB")));
                    tags.push(("REPLAYGAIN_ALBUM_PEAK", format!("{album_peak:.6}")));
                }
                Format::Opus => {
                    // RFC 7845: Q7.8 fixed point, relative to −23 LUFS, which
                    // is 5 dB below the ReplayGain reference.
                    tags.push(("R128_TRACK_GAIN", r128(track_gain).to_string()));
                    tags.push(("R128_ALBUM_GAIN", r128(album_gain).to_string()));
                }
            }
        }
        tags
    }
}

fn r128(replaygain: f64) -> i32 {
    ((replaygain - 5.0) * 256.0).round() as i32
}

fn roman(number: u32) -> &'static str {
    ["I", "II", "III", "IV", "V", "VI", "VII", "VIII"]
        .get(number as usize - 1)
        .copied()
        .unwrap_or("IX")
}

/// `name` in the legacy encoding where every character has a byte there,
/// otherwise as UTF-8.
fn legacy_bytes(name: &str, legacy: Option<Legacy>) -> Vec<u8> {
    legacy
        .and_then(|legacy| paths::encode(name, legacy))
        .unwrap_or_else(|| name.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use super::*;

    fn collect(seed: u64, tracks: usize) -> Vec<Album> {
        Catalogue::new(seed, tracks, Format::Flac).collect()
    }

    fn values<'a>(track: &'a Track, key: &str) -> Vec<&'a str> {
        track
            .tags
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    #[test]
    fn exact_track_count_and_unique_paths() {
        for tracks in [1, 7, 200, 5000] {
            let albums = collect(1, tracks);
            let all: Vec<&Track> = albums.iter().flat_map(|a| &a.tracks).collect();
            assert_eq!(all.len(), tracks);
            let unique: HashSet<&[u8]> = all.iter().map(|t| t.path.as_slice()).collect();
            assert_eq!(unique.len(), tracks, "duplicate paths");
            assert!(albums.iter().all(|a| !a.tracks.is_empty()));
        }
    }

    #[test]
    fn deterministic_per_seed() {
        let digest = |albums: &[Album]| {
            albums
                .iter()
                .flat_map(|a| &a.tracks)
                .map(|t| format!("{:?}{:?}", t.path, t.tags))
                .collect::<Vec<_>>()
        };
        assert_eq!(digest(&collect(42, 3000)), digest(&collect(42, 3000)));
        assert_ne!(digest(&collect(42, 3000)), digest(&collect(43, 3000)));
    }

    #[test]
    fn distribution_hits_its_targets() {
        let albums = collect(7, 20_000);
        let tracks: Vec<&Track> = albums.iter().flat_map(|a| &a.tracks).collect();
        let share = |count: usize, total: usize| count as f64 / total as f64;
        let kind_share = |kind: Kind| {
            share(
                albums.iter().filter(|a| a.kind == kind).count(),
                albums.len(),
            )
        };

        assert!((0.05..0.14).contains(&kind_share(Kind::Compilation)));
        assert!((0.05..0.14).contains(&kind_share(Kind::Classical)));
        assert!((0.04..0.13).contains(&kind_share(Kind::Single)));
        let multi_disc = share(albums.iter().filter(|a| a.discs > 1).count(), albums.len());
        assert!((0.03..0.12).contains(&multi_disc), "{multi_disc}");

        // Compilations: "Various Artists" and per-track artists.
        for album in albums.iter().filter(|a| a.kind == Kind::Compilation) {
            let artists: BTreeSet<&str> = album
                .tracks
                .iter()
                .flat_map(|t| values(t, "ARTIST"))
                .collect();
            assert!(artists.len() > 1 || album.tracks.len() < 3);
            for track in &album.tracks {
                if !track.tags.is_empty() {
                    assert_eq!(values(track, "ALBUMARTIST"), [VARIOUS_ARTISTS]);
                    assert_eq!(values(track, "COMPILATION"), ["1"]);
                }
            }
        }
        // Multi-disc albums count their discs.
        let multi = albums.iter().find(|a| a.discs > 1).unwrap();
        let discs: BTreeSet<&str> = multi
            .tracks
            .iter()
            .flat_map(|t| values(t, "DISCNUMBER"))
            .collect();
        assert_eq!(discs.len(), multi.discs as usize);

        let with = |key: &str| tracks.iter().filter(|t| !values(t, key).is_empty()).count();
        let multi_value = |key: &str| tracks.iter().filter(|t| values(t, key).len() > 1).count();
        for key in [
            "COMPOSER",
            "CONDUCTOR",
            "PERFORMER",
            "WORK",
            "MOVEMENTNAME",
            "ORIGINALDATE",
        ] {
            assert!(with(key) > 0, "no {key}");
        }
        assert!(multi_value("ARTIST") > tracks.len() / 50);
        assert!(multi_value("GENRE") > tracks.len() / 20);
        assert!(multi_value("ALBUMARTIST") > 0);
        assert!(with("ARTISTSORT") > tracks.len() / 20);
        assert!(
            tracks
                .iter()
                .flat_map(|t| values(t, "ARTISTSORT"))
                .any(|sort| sort.ends_with(", The"))
        );
        let gain = share(with("REPLAYGAIN_TRACK_GAIN"), tracks.len());
        assert!((0.25..0.45).contains(&gain), "{gain}");
        let untagged = tracks.iter().filter(|t| t.tags.is_empty()).count();
        assert!(untagged > 0 && untagged < tracks.len() / 50);
        assert!(with("TITLE") < tracks.len());

        // Non-UTF-8 names: present but rare.
        let invalid = tracks
            .iter()
            .filter(|t| std::str::from_utf8(&t.path).is_err())
            .count();
        assert!(invalid > 0 && invalid < tracks.len() / 20, "{invalid}");

        // Every script appears in artist names.
        let mut scripts: HashMap<&str, bool> = HashMap::new();
        type InScript = fn(char) -> bool;
        let ranges: [(&str, InScript); 9] = [
            ("Estonian", |c| "õÕšž".contains(c)),
            ("Swedish/German", |c| "åÅäöüß".contains(c)),
            ("Cyrillic", |c| ('\u{400}'..='\u{4ff}').contains(&c)),
            ("Greek", |c| ('\u{370}'..='\u{3ff}').contains(&c)),
            ("CJK", |c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            ("Kana/Hangul", |c| {
                ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{ac00}'..='\u{d7af}').contains(&c)
            }),
            ("Thai", |c| ('\u{e00}'..='\u{e7f}').contains(&c)),
            ("Arabic", |c| ('\u{600}'..='\u{6ff}').contains(&c)),
            ("Hebrew", |c| ('\u{590}'..='\u{5ff}').contains(&c)),
        ];
        for track in &tracks {
            for value in values(track, "ARTIST") {
                for (name, test) in &ranges {
                    if value.chars().any(test) {
                        scripts.insert(name, true);
                    }
                }
            }
        }
        assert_eq!(scripts.len(), ranges.len(), "{scripts:?}");
    }

    #[test]
    fn musicbrainz_ids_mark_a_share_of_the_albums() {
        let albums = collect(11, 20_000);
        let mut releases = HashSet::new();
        let mut tagged = 0;
        for album in &albums {
            let ids: BTreeSet<(Vec<&str>, Vec<&str>)> = album
                .tracks
                .iter()
                .filter(|t| !t.tags.is_empty())
                .map(|t| {
                    (
                        values(t, "MUSICBRAINZ_ALBUMID"),
                        values(t, "MUSICBRAINZ_RELEASEGROUPID"),
                    )
                })
                .collect();
            // Every tagged track of an album has the same IDs, or none.
            assert!(ids.len() <= 1, "{ids:?}");
            if let Some((release, group)) = ids.into_iter().next()
                && let ([release], [group]) = (release.as_slice(), group.as_slice())
            {
                tagged += 1;
                assert_eq!(release.len(), 36);
                assert_ne!(release, group);
                assert!(releases.insert(release.to_string()), "{release} twice");
                for track in album.tracks.iter().filter(|t| !t.tags.is_empty()) {
                    assert_eq!(values(track, "MUSICBRAINZ_TRACKID").len(), 1);
                }
            }
        }
        let share = tagged as f64 / albums.len() as f64;
        assert!((0.18..0.32).contains(&share), "{share}");
    }

    #[test]
    fn popularity_is_skewed() {
        let albums = collect(3, 30_000);
        let mut per_artist: HashMap<String, usize> = HashMap::new();
        for album in albums.iter().filter(|a| a.kind == Kind::Regular) {
            let first = &album.tracks[0];
            let artist = values(first, "ARTIST").first().map(|s| s.to_string());
            if let Some(artist) = artist {
                *per_artist.entry(artist).or_default() += 1;
            }
        }
        let mut counts: Vec<usize> = per_artist.into_values().collect();
        counts.sort_unstable_by(|a, b| b.cmp(a));
        let median = counts[counts.len() / 2];
        assert!(
            counts[0] >= 5 * median.max(1),
            "top {} median {median}",
            counts[0]
        );
    }

    #[test]
    fn opus_uses_r128_gain() {
        let albums: Vec<Album> = Catalogue::new(7, 2000, Format::Opus).collect();
        let tracks: Vec<&Track> = albums.iter().flat_map(|a| &a.tracks).collect();
        assert!(tracks.iter().all(|t| t.path.ends_with(b".opus")));
        assert!(
            tracks
                .iter()
                .any(|t| !values(t, "R128_TRACK_GAIN").is_empty())
        );
        assert!(
            tracks
                .iter()
                .all(|t| values(t, "REPLAYGAIN_TRACK_GAIN").is_empty())
        );
        assert_eq!(r128(-7.0), -3072);
        assert_eq!(r128(5.0), 0);
    }
}
