// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Album identity: which tracks form one album.
//!
//! Every track with an album title or a MusicBrainz release ID gets a group key
//! (`tracks.album_group`), and the tracks with one key form one album, unless that
//! would put two tracks with the same disc and track number from different folders
//! on it; then each folder is an album of its own.
//!
//! The group key is:
//!
//! 1. `mb` and the MusicBrainz release ID, when the track has one.
//! 2. Otherwise `al`, the album artist, the folded album title and the year:
//!    - The album artist is the folded `ALBUMARTIST` (as credited, or the album
//!      artists joined). Names such as `Various Artists` and `VA` count as "various
//!      artists".
//!    - Without one, a track with the compilation flag is by "various artists".
//!    - Otherwise the tracks with the same album title in the same album folder
//!      decide together: the artist on more than half of them (the first track
//!      artist, without `feat.` and what follows) is the album artist; without such
//!      an artist, the album is by "various artists".
//!    - The year is the original year, or else the year, that most tracks with the
//!      same album title and album artist in the album folder have.
//!
//! The album folder of a track is its folder, or the folder above when its folder is
//! a disc folder (`CD1`, `Disc 2 - Live`), so the discs of an album in folders of their
//! own are decided together.
//!
//! Regrouping is incremental: writing, moving or removing tracks regroups the
//! partitions (album folder and folded album title) and group keys they were and are
//! in, which is everything that their change can affect.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::sync::LazyLock;

use rusqlite::{Connection, OptionalExtension, params};

use super::people::person;
use super::track_record::ArtistRole;
use crate::search::fold;

/// The folded album artist of compilations.
const VARIOUS: &str = "various artists";

/// Album artist names that mean "various artists", compared folded.
const VARIOUS_NAMES: &[&str] = &[
    "various artists",
    "various artist",
    "various",
    "va",
    "v.a.",
    "v/a",
    "v. a.",
    "verschiedene interpreten",
    "verschiedene künstler",
    "artistes divers",
    "varios artistas",
    "varios",
    "artisti vari",
    "artisti varii",
    "diverse artiesten",
    "diverse kunstenaars",
    "blandade artister",
    "olika artister",
    "eri esittäjiä",
    "erinevad esitajad",
    "разные исполнители",
    "сборник",
    "różni wykonawcy",
    "různí interpreti",
    "vários artistas",
];

/// The folded forms of [`VARIOUS_NAMES`].
static VARIOUS_FOLDED: LazyLock<HashSet<String>> =
    LazyLock::new(|| VARIOUS_NAMES.iter().map(|name| fold(name)).collect());

/// Whether the folded album artist `folded` means "various artists".
pub(crate) fn is_various(folded: &str) -> bool {
    VARIOUS_FOLDED.contains(folded)
}

/// The main artist in a credit: the text before `feat.`, `ft.` or `featuring`.
pub(crate) fn main_artist(artist: &str) -> &str {
    let lower = artist.to_ascii_lowercase();
    let cut = [
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " (feat. ",
        " (feat ",
        " (ft. ",
        " [feat. ",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .min();
    match cut {
        // ASCII lower-casing keeps every byte offset.
        Some(at) if at > 0 => artist[..at].trim_end(),
        _ => artist,
    }
}

/// What has to be regrouped after tracks were written, moved or removed.
#[derive(Debug, Default)]
pub(crate) struct Regroup {
    /// Album folders and folded album titles whose group keys are decided again.
    partitions: HashSet<(i64, String)>,
    /// Tracks without an album title, whose key depends on themselves alone.
    singles: HashSet<i64>,
    /// Group keys whose albums are formed again.
    groups: HashSet<String>,
}

impl Regroup {
    /// Adds what `track` is in now. A track that is gone adds nothing.
    pub(crate) fn add_track(&mut self, c: &Connection, track: i64) -> rusqlite::Result<()> {
        let found: Option<(Option<i64>, Option<String>, Option<String>)> = c
            .prepare_cached(
                "SELECT album_dir_id, album_fold, album_group FROM tracks WHERE id = ?1",
            )?
            .query_row([track], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        if let Some((dir, album, group)) = found {
            self.add(Some(track), dir, album, group);
        }
        Ok(())
    }

    /// Adds what the tracks selected by `filter` (a condition on `tracks`, with the
    /// parameter `?1`) are in, before they are removed or moved.
    pub(crate) fn add_tracks_where(
        &mut self,
        c: &Connection,
        filter: &str,
        value: i64,
    ) -> rusqlite::Result<()> {
        let sql =
            format!("SELECT id, album_dir_id, album_fold, album_group FROM tracks WHERE {filter}");
        let mut statement = c.prepare_cached(&sql)?;
        let rows = statement.query_map([value], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        for row in rows {
            let (track, dir, album, group) = row?;
            self.add(Some(track), dir, album, group);
        }
        Ok(())
    }

    fn add(
        &mut self,
        track: Option<i64>,
        dir: Option<i64>,
        album: Option<String>,
        group: Option<String>,
    ) {
        match (dir, album) {
            (Some(dir), Some(album)) => {
                self.partitions.insert((dir, album));
            }
            _ => {
                if let Some(track) = track {
                    self.singles.insert(track);
                }
            }
        }
        if let Some(group) = group {
            self.groups.insert(group);
        }
    }

    /// Whether there is nothing to do.
    pub(crate) fn is_empty(&self) -> bool {
        self.partitions.is_empty() && self.singles.is_empty() && self.groups.is_empty()
    }

    /// Decides the group keys, forms the albums, and returns the tracks whose album
    /// changed (another album row, or an album whose title changed), whose search
    /// rows must be written again.
    pub(crate) fn run(mut self, c: &Connection) -> rusqlite::Result<Vec<i64>> {
        let mut changed = Vec::new();
        // The tracks whose key changes, by their new key. Their rows still hold the
        // old key until their album is formed.
        let mut moving: HashMap<i64, String> = HashMap::new();
        let mut note = |track: i64, old: Option<String>, key: Option<String>| {
            let differs = old != key;
            self.groups.extend(old);
            match key {
                Some(key) => {
                    self.groups.insert(key.clone());
                    if differs {
                        moving.insert(track, key);
                    }
                    None
                }
                None => differs.then_some(track),
            }
        };
        let mut ungrouped = Vec::new();
        for (dir, album) in std::mem::take(&mut self.partitions) {
            for (track, old, key) in partition_keys(c, dir, &album)? {
                ungrouped.extend(note(track, old, Some(key)));
            }
        }
        for track in std::mem::take(&mut self.singles) {
            let found: Option<(Option<String>, Option<String>)> = c
                .prepare_cached("SELECT album_group, release_mbid FROM tracks WHERE id = ?1")?
                .query_row([track], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            if let Some((old, mbid)) = found {
                ungrouped.extend(note(track, old, mbid.map(|mbid| mbid_key(&mbid))));
            }
        }
        // Tracks that lost their album title.
        let mut clear = c.prepare_cached(
            "UPDATE tracks SET album_group = NULL, album_id = NULL WHERE id = ?1",
        )?;
        for track in ungrouped {
            clear.execute([track])?;
            changed.push(track);
        }
        let mut groups: Vec<String> = self.groups.into_iter().collect();
        groups.sort();
        for group in groups {
            form_albums(c, &group, &moving, &mut changed)?;
        }
        changed.sort_unstable();
        changed.dedup();
        Ok(changed)
    }
}

fn mbid_key(mbid: &str) -> String {
    format!("mb\u{1f}{}", mbid.trim().to_ascii_lowercase())
}

/// A track of a partition, as its group key is decided.
struct Member {
    id: i64,
    group: Option<String>,
    album_artist: Option<String>,
    mbid: Option<String>,
    compilation: bool,
    year: Option<i64>,
    main_artist: Option<String>,
}

/// The group keys of the tracks in the album folder `dir` with the folded album
/// title `album`: each track with its old key and its new one.
fn partition_keys(
    c: &Connection,
    dir: i64,
    album: &str,
) -> rusqlite::Result<Vec<(i64, Option<String>, String)>> {
    let members: Vec<Member> = c
        .prepare_cached(
            "SELECT id, album_group, album_artist, release_mbid, coalesce(compilation, 0),
                    coalesce(original_year, year), main_artist
             FROM tracks WHERE album_dir_id = ?1 AND album_fold = ?2",
        )?
        .query_map(params![dir, album], |row| {
            Ok(Member {
                id: row.get(0)?,
                group: row.get(1)?,
                album_artist: row.get(2)?,
                mbid: row.get(3)?,
                compilation: row.get(4)?,
                year: row.get(5)?,
                main_artist: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    // The album artist of each track, or `None` for the tracks that decide together.
    let artists: Vec<Option<String>> = members
        .iter()
        .map(|m| match &m.album_artist {
            Some(artist) => {
                let folded = fold(artist);
                Some(if is_various(&folded) {
                    VARIOUS.to_owned()
                } else {
                    folded
                })
            }
            None if m.compilation => Some(VARIOUS.to_owned()),
            None => None,
        })
        .collect();
    let undecided: Vec<String> = members
        .iter()
        .zip(&artists)
        .filter(|(m, artist)| artist.is_none() && m.mbid.is_none())
        .map(|(m, _)| m.main_artist.as_deref().map(fold).unwrap_or_default())
        .collect();
    let together = majority(&undecided).unwrap_or_else(|| VARIOUS.to_owned());

    let artists: Vec<String> = artists
        .into_iter()
        .map(|artist| artist.unwrap_or_else(|| together.clone()))
        .collect();
    let mut years: HashMap<&str, Vec<i64>> = HashMap::new();
    for (m, artist) in members.iter().zip(&artists) {
        if m.mbid.is_none()
            && let Some(year) = m.year
        {
            years.entry(artist.as_str()).or_default().push(year);
        }
    }
    let years: HashMap<&str, Option<i64>> = years
        .into_iter()
        .map(|(artist, list)| (artist, mode(list)))
        .collect();
    Ok(members
        .iter()
        .zip(&artists)
        .map(|(m, artist)| {
            let old = m.group.clone();
            let key = match &m.mbid {
                Some(mbid) => mbid_key(mbid),
                None => {
                    let year = years
                        .get(artist.as_str())
                        .copied()
                        .flatten()
                        .map_or(String::new(), |y| y.to_string());
                    format!("al\u{1f}{artist}\u{1f}{album}\u{1f}{year}")
                }
            };
            (m.id, old, key)
        })
        .collect())
}

/// The value that more than half of `values` have.
fn majority(values: &[String]) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts
        .into_iter()
        .find(|(_, count)| count * 2 > values.len())
        .map(|(value, _)| value.to_owned())
}

/// The most frequent value; of equally frequent ones, the smallest.
fn mode<T: Ord + Hash + Eq>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut counts: HashMap<T, usize> = HashMap::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|(a, x), (b, y)| x.cmp(y).then_with(|| b.cmp(a)))
        .map(|(value, _)| value)
}

/// A track of a group, as its album is formed.
#[derive(Debug)]
struct Part {
    id: i64,
    folder: Option<i64>,
    disc: i64,
    track_no: Option<i64>,
    album_id: Option<i64>,
    title: Option<String>,
    title_sort: Option<String>,
    album_artist: Option<String>,
    album_artist_sort: Option<String>,
    main_artist: Option<String>,
    compilation: bool,
    release_type: Option<String>,
    mbid: Option<String>,
    group_mbid: Option<String>,
    year: Option<i64>,
    original_year: Option<i64>,
    disc_no: Option<i64>,
    disc_total: Option<i64>,
    added_at: i64,
    /// Whether the track's row still holds another group key.
    regrouped: bool,
}

/// Forms the albums of the group `key`: one, or one per album folder when tracks
/// from different folders have the same disc and track number. Adds the tracks
/// whose album changed to `changed`.
fn form_albums(
    c: &Connection,
    key: &str,
    moving: &HashMap<i64, String>,
    changed: &mut Vec<i64>,
) -> rusqlite::Result<()> {
    // The tracks with the key, less those leaving it, and those coming to it.
    let mut parts: Vec<Part> = c
        .prepare_cached(&format!("{PART_COLUMNS} WHERE album_group = ?1"))?
        .query_map([key], part)?
        .filter(|p| p.as_ref().map_or(true, |p| !moving.contains_key(&p.id)))
        .collect::<rusqlite::Result<_>>()?;
    let mut by_id = c.prepare_cached(&format!("{PART_COLUMNS} WHERE id = ?1"))?;
    for (track, _) in moving.iter().filter(|(_, k)| k.as_str() == key) {
        if let Some(mut found) = by_id.query_row([track], part).optional()? {
            found.regrouped = true;
            parts.push(found);
        }
    }
    parts.sort_by_key(|p| p.id);
    if parts.is_empty() {
        return Ok(());
    }
    form_parts(c, key, &parts, changed)
}

/// The columns that [`part`] reads, from `tracks`.
const PART_COLUMNS: &str = "SELECT id, album_dir_id, coalesce(disc_no, 1), track_no, album_id,
    album_title, album_sort, album_artist, album_artist_sort, main_artist,
    coalesce(compilation, 0), release_type, release_mbid, release_group_mbid, year,
    original_year, disc_no, disc_total, added_at FROM tracks";

fn part(row: &rusqlite::Row<'_>) -> rusqlite::Result<Part> {
    Ok(Part {
        id: row.get(0)?,
        folder: row.get(1)?,
        disc: row.get(2)?,
        track_no: row.get(3)?,
        album_id: row.get(4)?,
        title: row.get(5)?,
        title_sort: row.get(6)?,
        album_artist: row.get(7)?,
        album_artist_sort: row.get(8)?,
        main_artist: row.get(9)?,
        compilation: row.get(10)?,
        release_type: row.get(11)?,
        mbid: row.get(12)?,
        group_mbid: row.get(13)?,
        year: row.get(14)?,
        original_year: row.get(15)?,
        disc_no: row.get(16)?,
        disc_total: row.get(17)?,
        added_at: row.get(18)?,
        regrouped: false,
    })
}

/// Forms the albums of the group `key` from its tracks `parts`.
fn form_parts(
    c: &Connection,
    key: &str,
    parts: &[Part],
    changed: &mut Vec<i64>,
) -> rusqlite::Result<()> {
    // Two tracks with one disc and track number in different folders: two releases.
    let mut places: HashMap<(i64, i64), HashSet<Option<i64>>> = HashMap::new();
    for part in parts {
        if let Some(number) = part.track_no {
            places
                .entry((part.disc, number))
                .or_default()
                .insert(part.folder);
        }
    }
    let split = places.values().any(|folders| folders.len() > 1);
    let mut albums: BTreeMap<String, Vec<&Part>> = BTreeMap::new();
    for part in parts {
        let album_key = if split {
            let folder = part.folder.map_or(String::new(), |f| f.to_string());
            format!("{key}\u{1f}d{folder}")
        } else {
            key.to_owned()
        };
        albums.entry(album_key).or_default().push(part);
    }
    for (album_key, tracks) in albums {
        write_album(c, &album_key, key, &tracks, changed)?;
    }
    Ok(())
}

/// The values an album row takes from its tracks.
#[derive(Debug, Clone, PartialEq)]
struct AlbumValues {
    title: Option<String>,
    title_sort: Option<String>,
    title_fold: Option<String>,
    artist: Option<String>,
    artist_sort: Option<String>,
    year: Option<i64>,
    original_year: Option<i64>,
    is_compilation: bool,
    release_type: Option<String>,
    mbid: Option<String>,
    group_mbid: Option<String>,
    disc_total: Option<i64>,
}

/// Who an album is by.
enum AlbumArtists {
    /// The album artists of this track, as tagged.
    Tagged { track: i64 },
    /// The main artist of the tracks; this name.
    Inferred { name: String },
    /// Various artists.
    Various,
    /// Nobody: the tracks have no artists.
    Unknown,
}

/// Writes the album row `key` for `tracks` and links them to it.
fn write_album(
    c: &Connection,
    key: &str,
    group: &str,
    tracks: &[&Part],
    changed: &mut Vec<i64>,
) -> rusqlite::Result<()> {
    let tagged = mode(tracks.iter().filter_map(|t| t.album_artist.as_deref()));
    let any_flag = tracks.iter().any(|t| t.compilation);
    let artists = match tagged {
        Some(artist) if is_various(&fold(artist)) => AlbumArtists::Various,
        Some(artist) => AlbumArtists::Tagged {
            track: tracks
                .iter()
                .find(|t| t.album_artist.as_deref() == Some(artist))
                .map_or(0, |t| t.id),
        },
        None if any_flag => AlbumArtists::Various,
        None => {
            let mains: Vec<String> = tracks
                .iter()
                .map(|t| t.main_artist.clone().unwrap_or_default())
                .collect();
            let folded: Vec<String> = mains.iter().map(|m| fold(m)).collect();
            match majority(&folded) {
                Some(winner) if !winner.is_empty() => AlbumArtists::Inferred {
                    name: mains
                        .iter()
                        .zip(&folded)
                        .filter(|(_, f)| **f == winner)
                        .map(|(m, _)| m.as_str())
                        .min()
                        .unwrap_or_default()
                        .to_owned(),
                },
                Some(_) => AlbumArtists::Unknown,
                None => AlbumArtists::Various,
            }
        }
    };

    let people: Vec<i64> = match &artists {
        AlbumArtists::Tagged { track } => c
            .prepare_cached(
                "SELECT person_id FROM track_people WHERE track_id = ?1 AND role = ?2
                 ORDER BY position",
            )?
            .query_map(params![track, ArtistRole::AlbumArtist.to_sql()], |row| {
                row.get(0)
            })?
            .collect::<rusqlite::Result<_>>()?,
        AlbumArtists::Inferred { name } => vec![person(c, name, None, None)?],
        AlbumArtists::Various | AlbumArtists::Unknown => Vec::new(),
    };
    let first_sort: Option<String> = match people.first() {
        Some(person) => c
            .prepare_cached("SELECT sort_name FROM people WHERE id = ?1")?
            .query_row([person], |row| row.get(0))
            .optional()?
            .flatten(),
        None => None,
    };
    let (artist, artist_sort) = match &artists {
        AlbumArtists::Tagged { .. } => {
            let artist = tagged.map(str::to_owned);
            let sort = mode(tracks.iter().filter_map(|t| t.album_artist_sort.as_deref()))
                .map(str::to_owned)
                .or(if people.len() == 1 { first_sort } else { None })
                .or_else(|| artist.clone());
            (artist, sort)
        }
        AlbumArtists::Inferred { name } => (Some(name.clone()), first_sort.or(Some(name.clone()))),
        AlbumArtists::Various | AlbumArtists::Unknown => (None, None),
    };
    let title = mode(tracks.iter().filter_map(|t| t.title.as_deref())).map(str::to_owned);
    let values = AlbumValues {
        title_sort: mode(tracks.iter().filter_map(|t| t.title_sort.as_deref()))
            .map(str::to_owned)
            .or_else(|| title.clone()),
        title_fold: title.as_deref().map(fold),
        title,
        artist,
        artist_sort,
        year: mode(tracks.iter().filter_map(|t| t.year)),
        original_year: mode(tracks.iter().filter_map(|t| t.original_year)),
        is_compilation: any_flag || matches!(artists, AlbumArtists::Various),
        release_type: mode(tracks.iter().filter_map(|t| t.release_type.as_deref()))
            .map(str::to_owned),
        mbid: mode(tracks.iter().filter_map(|t| t.mbid.as_deref())).map(str::to_owned),
        group_mbid: mode(tracks.iter().filter_map(|t| t.group_mbid.as_deref())).map(str::to_owned),
        disc_total: tracks
            .iter()
            .filter_map(|t| t.disc_total.max(t.disc_no))
            .max(),
    };
    let added_at = tracks.iter().map(|t| t.added_at).min();

    let existing: Option<(i64, Option<i64>, AlbumValues)> = c
        .prepare_cached(
            "SELECT id, title, title_sort, title_fold, artist, artist_sort, year,
                    original_year, is_compilation, release_type, mbid, release_group_mbid,
                    disc_total, added_at
             FROM albums WHERE album_key = ?1",
        )?
        .query_row([key], |row| {
            Ok((
                row.get(0)?,
                row.get(13)?,
                AlbumValues {
                    title: row.get(1)?,
                    title_sort: row.get(2)?,
                    title_fold: row.get(3)?,
                    artist: row.get(4)?,
                    artist_sort: row.get(5)?,
                    year: row.get(6)?,
                    original_year: row.get(7)?,
                    is_compilation: row.get(8)?,
                    release_type: row.get(9)?,
                    mbid: row.get(10)?,
                    group_mbid: row.get(11)?,
                    disc_total: row.get(12)?,
                },
            ))
        })
        .optional()?;
    let v = &values;
    let album = match existing {
        Some((id, old_added, old)) => {
            if let Some(added_at) = added_at
                && old_added.is_none_or(|old| old > added_at)
            {
                c.prepare_cached("UPDATE albums SET added_at = ?2 WHERE id = ?1")?
                    .execute(params![id, added_at])?;
            }
            if old != values {
                c.prepare_cached(
                    "UPDATE albums SET title = ?2, title_sort = ?3, title_fold = ?4,
                         artist = ?5, artist_sort = ?6, year = ?7, original_year = ?8,
                         is_compilation = ?9, release_type = ?10, mbid = ?11,
                         release_group_mbid = ?12, disc_total = ?13
                     WHERE id = ?1",
                )?
                .execute(params![
                    id,
                    v.title,
                    v.title_sort,
                    v.title_fold,
                    v.artist,
                    v.artist_sort,
                    v.year,
                    v.original_year,
                    v.is_compilation,
                    v.release_type,
                    v.mbid,
                    v.group_mbid,
                    v.disc_total
                ])?;
            }
            if old.title_fold != values.title_fold {
                changed.extend(tracks.iter().map(|t| t.id));
            }
            id
        }
        None => c
            .prepare_cached(
                "INSERT INTO albums (album_key, title, title_sort, title_fold, artist,
                     artist_sort, year, original_year, is_compilation, release_type, mbid,
                     release_group_mbid, disc_total, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 RETURNING id",
            )?
            .query_row(
                params![
                    key,
                    v.title,
                    v.title_sort,
                    v.title_fold,
                    v.artist,
                    v.artist_sort,
                    v.year,
                    v.original_year,
                    v.is_compilation,
                    v.release_type,
                    v.mbid,
                    v.group_mbid,
                    v.disc_total,
                    added_at
                ],
                |row| row.get(0),
            )?,
    };
    write_album_people(c, album, &people)?;

    let mut link = c.prepare_cached("UPDATE tracks SET album_id = ?2 WHERE id = ?1")?;
    let mut join =
        c.prepare_cached("UPDATE tracks SET album_id = ?2, album_group = ?3 WHERE id = ?1")?;
    for track in tracks {
        if track.regrouped {
            join.execute(params![track.id, album, group])?;
        } else if track.album_id != Some(album) {
            link.execute(params![track.id, album])?;
        } else {
            continue;
        }
        if track.album_id != Some(album) {
            changed.push(track.id);
        }
    }
    Ok(())
}

/// Makes the album's artists `people`, in order, changing only what differs.
fn write_album_people(c: &Connection, album: i64, people: &[i64]) -> rusqlite::Result<()> {
    let existing: BTreeSet<(i64, i64)> = c
        .prepare_cached("SELECT position, person_id FROM album_people WHERE album_id = ?1")?
        .query_map([album], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let wanted: BTreeSet<(i64, i64)> = (0_i64..).zip(people.iter().copied()).collect();
    if existing == wanted {
        return Ok(());
    }
    let mut upsert = c.prepare_cached(
        "INSERT INTO album_people (album_id, position, person_id) VALUES (?1, ?2, ?3)
         ON CONFLICT (album_id, position) DO UPDATE SET person_id = excluded.person_id",
    )?;
    for (position, person) in &wanted {
        if !existing.contains(&(*position, *person)) {
            upsert.execute(params![album, position, person])?;
        }
    }
    let count = i64::try_from(people.len()).unwrap_or(i64::MAX);
    c.prepare_cached("DELETE FROM album_people WHERE album_id = ?1 AND position >= ?2")?
        .execute(params![album, count])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_artists() {
        assert_eq!(main_artist("A feat. B"), "A");
        assert_eq!(main_artist("A (feat. B)"), "A");
        assert_eq!(main_artist("Ä ft. B"), "Ä");
        assert_eq!(main_artist("A Featuring B"), "A");
        assert_eq!(main_artist("Feature"), "Feature");
        assert_eq!(main_artist("A & B"), "A & B");
    }

    #[test]
    fn modes_and_majorities() {
        assert_eq!(mode([3, 1, 3, 1, 2]), Some(1));
        assert_eq!(mode([2000, 2001, 2001]), Some(2001));
        assert_eq!(mode(Vec::<i32>::new()), None);
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(majority(&s(&["a", "a", "b"])), Some("a".to_owned()));
        assert_eq!(majority(&s(&["a", "b"])), None);
        assert_eq!(majority(&s(&["a", "b", "c", "a"])), None);
    }

    #[test]
    fn various_artists() {
        for name in ["Various Artists", "VA", "V.A.", "Verschiedene Künstler"] {
            assert!(is_various(&fold(name)), "{name}");
        }
        assert!(!is_various(&fold("Various Positions")));
    }
}
