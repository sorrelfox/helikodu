// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The facet browser's panes and the counts of the Library statistics view.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, SystemTime};

pub use helikodu_service::library::{FacetChoice, FacetField, FacetKey, FacetList, FacetValue};

use super::{AlbumKey, ArtistKey, CoverKey, TrackRow};

/// An artist among the most played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayedArtistRow {
    /// The artist, to open them.
    pub key: ArtistKey,
    /// Their name.
    pub name: String,
    /// Plays of their tracks together.
    pub plays: u64,
}

/// An album among the most played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayedAlbumRow {
    /// The album, to open it.
    pub key: AlbumKey,
    /// Its title.
    pub title: String,
    /// Its album artist.
    pub artist: String,
    /// Its cover, if it has one.
    pub cover: Option<CoverKey>,
    /// The seed of its generated cover.
    pub cover_seed: u64,
    /// Plays of its tracks together.
    pub plays: u64,
}

/// From when the statistics count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatisticsSince {
    /// The start of this month, for the tracks added lately.
    pub month: SystemTime,
    /// The start of this year, for the listening figures.
    pub year: SystemTime,
}

/// What the Library statistics view shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryStats {
    /// Tracks.
    pub tracks: usize,
    /// Tracks added this month.
    pub added_this_month: usize,
    /// Albums.
    pub albums: usize,
    /// Albums without a cover.
    pub albums_without_cover: usize,
    /// Artists.
    pub artists: usize,
    /// Composers.
    pub composers: usize,
    /// The length of all tracks.
    pub duration: Duration,
    /// The size of the files in bytes.
    pub size: u64,
    /// Library folders.
    pub folders: usize,
    /// The drives the folders are on.
    pub drives: usize,
    /// The tracks of each codec (lower case; `None` when unknown), most first.
    pub formats: Vec<(Option<String>, usize)>,
    /// Lossless tracks at up to 48 kHz and 16 bits.
    pub lossless_cd: usize,
    /// Lossless tracks above 48 kHz or 16 bits.
    pub lossless_hi_res: usize,
    /// Lossy tracks.
    pub lossy: usize,
    /// Plays this year.
    pub plays: u64,
    /// Time listened this year.
    pub listened: Duration,
    /// Skips this year.
    pub skips: u64,
    /// Tracks never played.
    pub never_played: usize,
    /// The most played artists.
    pub top_artists: Vec<PlayedArtistRow>,
    /// The most played albums.
    pub top_albums: Vec<PlayedAlbumRow>,
}

/// The facet values of `field` among `rows`, for backends that hold their
/// tracks in memory; `album_artist` names the album artist of an album, and
/// `labels` gives the labels of a track.
pub(crate) fn facets_of(
    rows: &[TrackRow],
    field: FacetField,
    album_artist: impl Fn(AlbumKey) -> String,
    labels: impl Fn(&TrackRow) -> Vec<String>,
) -> FacetList {
    let mut counts: BTreeMap<(FacetKey, String), usize> = BTreeMap::new();
    for row in rows {
        if field == FacetField::Label {
            // A track counts under each of its labels.
            for label in labels(row) {
                *counts.entry((text_key(&label), label)).or_default() += 1;
            }
            continue;
        }
        let (key, name) = match field {
            FacetField::Genre if !row.genre.is_empty() => (text_key(&row.genre), row.genre.clone()),
            FacetField::AlbumArtist => {
                let name = album_artist(row.album);
                (text_key(&name), name)
            }
            FacetField::Album => (
                FacetKey::Id(i64::try_from(row.album.0).unwrap_or(-1)),
                row.album_title.clone(),
            ),
            FacetField::Artist if !row.artist.is_empty() => {
                (text_key(&row.artist), row.artist.clone())
            }
            FacetField::Year => match row.year {
                Some(year) => (FacetKey::Id(i64::from(year)), year.to_string()),
                None => continue,
            },
            FacetField::Format => match &row.format.codec {
                Some(codec) => (FacetKey::Text(codec.to_lowercase()), codec.to_lowercase()),
                None => continue,
            },
            _ => continue,
        };
        *counts.entry((key, name)).or_default() += 1;
    }
    let mut values: Vec<FacetValue> = counts
        .into_iter()
        .map(|((key, name), tracks)| FacetValue { key, name, tracks })
        .collect();
    match field {
        FacetField::Year => values.sort_by(|a, b| b.key.cmp(&a.key)),
        FacetField::Format => {
            values.sort_by(|a, b| b.tracks.cmp(&a.tracks).then(a.key.cmp(&b.key)))
        }
        _ => values.sort_by_key(|v| v.name.to_lowercase()),
    }
    FacetList {
        values,
        tracks: rows.len(),
    }
}

/// Whether `row` passes every choice of `chosen`, as [`facets_of`] keys them.
pub(crate) fn passes(
    row: &TrackRow,
    chosen: &[FacetChoice],
    album_artist: &impl Fn(AlbumKey) -> String,
    labels: &impl Fn(&TrackRow) -> Vec<String>,
) -> bool {
    chosen.iter().filter(|c| !c.keys.is_empty()).all(|choice| {
        let keys: HashSet<&FacetKey> = choice.keys.iter().collect();
        let key = match choice.field {
            FacetField::Label => {
                return labels(row).iter().any(|l| keys.contains(&text_key(l)));
            }
            FacetField::Genre => text_key(&row.genre),
            FacetField::AlbumArtist => text_key(&album_artist(row.album)),
            FacetField::Album => FacetKey::Id(i64::try_from(row.album.0).unwrap_or(-1)),
            FacetField::Artist => text_key(&row.artist),
            FacetField::Composer => return false,
            FacetField::Year => match row.year {
                Some(year) => FacetKey::Id(i64::from(year)),
                None => return false,
            },
            FacetField::Format => match &row.format.codec {
                Some(codec) => FacetKey::Text(codec.to_lowercase()),
                None => return false,
            },
        };
        keys.contains(&key)
    })
}

fn text_key(text: &str) -> FacetKey {
    FacetKey::Text(text.to_owned())
}
