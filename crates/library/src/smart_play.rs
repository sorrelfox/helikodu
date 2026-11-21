// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Picking tracks from the library for smarter playback, all offline:
//!
//! - [`Library::play_facts`] gives the statistics a weighted shuffle weighs.
//! - [`Library::related_tracks`] continues from a seed track, album or artist (endless
//!   play): every candidate is scored against the seed on several likenesses (artist,
//!   album artist, genre, year, labels, play statistics and, once the sound analysis
//!   provides it, sound), and picks are drawn at random with the score as weight, so
//!   the closest tracks come most often without the same batch every time.
//! - [`Library::auto_dj_tracks`] picks from the tracks a query finds, weighted by
//!   rating and play statistics.
//!
//! Both avoid the tracks they are given (those played or queued last), and both are
//! deterministic for a given random seed.

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use helikodu_model::library::TrackId;
use helikodu_model::settings::{
    AutoDjSettings, EndlessSettings, PlayFacts, PlayWeight, RelatedWeights, below_rating,
    play_weight,
};
use rusqlite::Connection;

use crate::catalog::ids::{sql_track_id, track_id_from_sql};
use crate::catalog::roots::visibility;
use crate::catalog::{AlbumId, GenreId, PersonId};
use crate::db::DbError;
use crate::db::unix_time::to_unix_seconds;
use crate::library::Library;
use crate::query::{QueryError, QueryRequest, SyntaxError, parse_query};

impl TrackFacts {
    /// Nothing known of `id` but itself.
    pub fn new(id: TrackId) -> Self {
        Self {
            id,
            album: None,
            artists: Vec::new(),
            album_artists: Vec::new(),
            genres: Vec::new(),
            labels: Vec::new(),
            year: None,
            play: PlayFacts::default(),
        }
    }
}

/// What the related tracks continue from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaySeed {
    /// A track.
    Track(TrackId),
    /// An album.
    Album(AlbumId),
    /// An artist: the tracks on which the person is a track or album artist.
    Artist(PersonId),
}

/// What the picks know of a library track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackFacts {
    /// The track.
    pub id: TrackId,
    /// Its album.
    pub album: Option<AlbumId>,
    /// Its track artists.
    pub artists: Vec<PersonId>,
    /// Its album artists.
    pub album_artists: Vec<PersonId>,
    /// Its genres.
    pub genres: Vec<GenreId>,
    /// The labels given to it (the numbers of the labels).
    pub labels: Vec<i64>,
    /// Its release year, or its album's.
    pub year: Option<i32>,
    /// Its statistics.
    pub play: PlayFacts,
}

/// How alike two tracks sound, from 0 (not at all) to 1 (the same), or `None` when
/// either has not been analysed. The sound analysis provides one; endless play
/// takes it as one more weighted likeness (`playback.endless.weights.sound`).
pub trait SoundSimilarity {
    /// The likeness of `a` and `b`.
    fn similarity(&self, a: TrackId, b: TrackId) -> Option<f64>;
}

/// No sound analysis: every likeness is unknown.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSoundSimilarity;

impl SoundSimilarity for NoSoundSimilarity {
    fn similarity(&self, _a: TrackId, _b: TrackId) -> Option<f64> {
        None
    }
}

/// Why tracks could not be picked.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PickError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The filter query cannot be read.
    #[error("the filter cannot be read: {0}")]
    Syntax(#[from] SyntaxError),
    /// The filter query cannot run.
    #[error(transparent)]
    Query(#[from] QueryError),
}

/// The seed as the scores see it: what its tracks have together.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeedProfile {
    /// The seed's tracks; the first few stand for it in sound likeness.
    pub tracks: Vec<TrackId>,
    /// Their track artists.
    pub artists: HashSet<PersonId>,
    /// Their album artists.
    pub album_artists: HashSet<PersonId>,
    /// Their genres.
    pub genres: HashSet<GenreId>,
    /// Their labels.
    pub labels: HashSet<i64>,
    /// The first and last of their years.
    pub years: Option<(i32, i32)>,
    /// Their mean rating in half stars, with tracks without one counted as three
    /// stars.
    pub rating: f64,
    /// Their mean play count.
    pub plays: f64,
}

/// The rating an unrated track counts as in the statistics likeness, in half stars.
const UNRATED: f64 = 6.0;

/// How many of the seed's tracks stand for it in sound likeness.
const SOUND_SEEDS: usize = 5;

impl SeedProfile {
    /// The profile of `tracks`.
    pub fn of(tracks: &[TrackFacts]) -> Self {
        let mut profile = Self::default();
        let mut ratings = 0.0;
        let mut plays = 0.0;
        for track in tracks {
            profile.tracks.push(track.id);
            profile.artists.extend(&track.artists);
            profile.album_artists.extend(&track.album_artists);
            profile.genres.extend(&track.genres);
            profile.labels.extend(&track.labels);
            if let Some(year) = track.year {
                profile.years = Some(match profile.years {
                    Some((first, last)) => (first.min(year), last.max(year)),
                    None => (year, year),
                });
            }
            ratings += track.play.rating.map_or(UNRATED, f64::from);
            plays += f64::from(track.play.plays);
        }
        if !tracks.is_empty() {
            let n = tracks.len() as f64;
            profile.rating = ratings / n;
            profile.plays = plays / n;
        }
        profile
    }
}

/// The share of `items` found in `set`, from 0 to 1; `None` when either is empty.
fn overlap<T: Eq + std::hash::Hash>(items: &[T], set: &HashSet<T>) -> Option<f64> {
    if items.is_empty() || set.is_empty() {
        return None;
    }
    let shared = items.iter().filter(|i| set.contains(i)).count();
    Some(shared as f64 / items.len().min(set.len()) as f64)
}

/// How close `year` is to the span `years`: 1 inside it widened by `range`, falling
/// to 0 one more `range` (at least a year) further out.
fn year_closeness(year: i32, (first, last): (i32, i32), range: u32) -> f64 {
    let range = i64::from(range);
    let year = i64::from(year);
    let distance = (i64::from(first) - range - year).max(year - i64::from(last) - range);
    if distance <= 0 {
        return 1.0;
    }
    let fade = range.max(1) as f64;
    (1.0 - distance as f64 / fade).max(0.0)
}

/// How much `track` is like the seed `profile`, from 0 to 1: the mean of the
/// likenesses known for it, each counted as much as its weight says. A likeness that
/// cannot be told (no genres, no year, no sound analysis) is left out rather than
/// counted as unlike, except artists and album artists, which a track either shares
/// or not.
pub fn related_score(
    profile: &SeedProfile,
    track: &TrackFacts,
    weights: &RelatedWeights,
    year_range: u32,
    sound: &dyn SoundSimilarity,
) -> f64 {
    let shares = |items: &[PersonId], set: &HashSet<PersonId>| {
        if set.is_empty() {
            None
        } else {
            Some(if items.iter().any(|p| set.contains(p)) {
                1.0
            } else {
                0.0
            })
        }
    };
    let statistics = {
        let rating = track.play.rating.map_or(UNRATED, f64::from);
        let rating_likeness = 1.0 - (rating - profile.rating).abs() / 10.0;
        let ratio = (f64::from(track.play.plays) + 1.0) / (profile.plays + 1.0);
        let plays_likeness = 1.0 / (1.0 + ratio.ln().abs());
        Some((rating_likeness + plays_likeness) / 2.0)
    };
    let sound_likeness = profile
        .tracks
        .iter()
        .take(SOUND_SEEDS)
        .filter(|seed| **seed != track.id)
        .filter_map(|seed| sound.similarity(*seed, track.id))
        .map(|s| s.clamp(0.0, 1.0))
        .reduce(f64::max);
    let inputs = [
        (weights.artist, shares(&track.artists, &profile.artists)),
        (
            weights.album_artist,
            shares(&track.album_artists, &profile.album_artists),
        ),
        (weights.genre, overlap(&track.genres, &profile.genres)),
        (
            weights.year,
            track
                .year
                .zip(profile.years)
                .map(|(year, years)| year_closeness(year, years, year_range)),
        ),
        (weights.labels, overlap(&track.labels, &profile.labels)),
        (weights.statistics, statistics),
        (weights.sound, sound_likeness),
    ];
    let mut total = 0.0;
    let mut weight = 0.0;
    for (w, likeness) in inputs {
        if let Some(likeness) = likeness
            && w > 0
        {
            total += f64::from(w) * likeness;
            weight += f64::from(w);
        }
    }
    if weight > 0.0 { total / weight } else { 0.0 }
}

/// The random numbers of the picks: SplitMix64.
#[derive(Debug, Clone)]
struct Draws(u64);

impl Draws {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number above 0 and below 1.
    fn unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
}

/// Up to `count` of `weighted` (items with weights above 0), drawn without putting
/// back, each draw as likely as its weight (Efraimidis and Spirakis); in the order
/// drawn. No more than `per_artist` come from one artist when that is set.
pub fn weighted_pick(
    weighted: Vec<(TrackFacts, f64)>,
    count: usize,
    per_artist: Option<usize>,
    seed: u64,
) -> Vec<TrackId> {
    let mut draws = Draws(seed);
    let mut keyed: Vec<(f64, TrackFacts)> = weighted
        .into_iter()
        .filter(|(_, w)| *w > 0.0 && w.is_finite())
        .map(|(facts, w)| (draws.unit().ln() / w, facts))
        .collect();
    // The largest key first: ln(u) / w is closest to 0 for heavy items.
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    let mut by_artist: HashMap<PersonId, usize> = HashMap::new();
    let mut picked = Vec::with_capacity(count.min(keyed.len()));
    for (_, facts) in keyed {
        if picked.len() >= count {
            break;
        }
        if let Some(cap) = per_artist
            && facts
                .artists
                .iter()
                .any(|a| by_artist.get(a).copied().unwrap_or(0) >= cap)
        {
            continue;
        }
        for artist in &facts.artists {
            *by_artist.entry(*artist).or_default() += 1;
        }
        picked.push(facts.id);
    }
    picked
}

/// The tracks related to `profile` from `candidates`, as [`Library::related_tracks`]
/// picks them.
pub fn pick_related(
    profile: &SeedProfile,
    candidates: Vec<TrackFacts>,
    settings: &EndlessSettings,
    count: usize,
    seed: u64,
    sound: &dyn SoundSimilarity,
) -> Vec<TrackId> {
    let weighted: Vec<(TrackFacts, f64)> = candidates
        .into_iter()
        .map(|facts| {
            let score = related_score(
                profile,
                &facts,
                &settings.weights,
                settings.year_range,
                sound,
            );
            // Cubed, so that close tracks come far more often than loose ones; a
            // floor keeps a library without any likeness playing.
            let weight = score.powi(3).max(1e-6);
            (facts, weight)
        })
        .collect();
    // A batch is not all one artist, unless nothing else is there.
    let cap = (count / 3).max(2);
    let picked = weighted_pick(weighted.clone(), count, Some(cap), seed);
    if picked.len() >= count.min(weighted.len()) {
        return picked;
    }
    weighted_pick(weighted, count, None, seed)
}

/// The tracks the Auto-DJ adds from `candidates`, as [`Library::auto_dj_tracks`]
/// picks them.
pub fn pick_auto_dj(
    candidates: Vec<TrackFacts>,
    weight_by: &[PlayWeight],
    min_rating: u8,
    count: usize,
    seed: u64,
) -> Vec<TrackId> {
    let weighted = candidates
        .into_iter()
        .filter(|facts| !below_rating(min_rating, facts.play.rating))
        .map(|facts| {
            let weight = play_weight(weight_by, &facts.play);
            (facts, weight)
        })
        .collect();
    weighted_pick(weighted, count, None, seed)
}

/// The tracks of the facts loaded: all music tracks that are not hidden, or those of
/// a list.
enum Scope<'a> {
    All,
    Tracks(&'a [i64]),
}

const DAY: i64 = 86_400;

fn json_list(ids: &[i64]) -> String {
    let list: Vec<String> = ids.iter().map(i64::to_string).collect();
    format!("[{}]", list.join(","))
}

/// The facts of the tracks of `scope`, in track order; `now` is the Unix time the
/// days since the last play count from.
fn load_facts(c: &Connection, scope: &Scope<'_>, now: i64) -> rusqlite::Result<Vec<TrackFacts>> {
    let (condition, list) = match scope {
        Scope::All => (
            format!("t.kind = 0 AND {}", visibility(c, false)?),
            "[]".to_owned(),
        ),
        Scope::Tracks(ids) => (
            "t.id IN (SELECT value FROM json_each(?1))".to_owned(),
            json_list(ids),
        ),
    };
    let only = |column: &str| match scope {
        Scope::All => "1".to_owned(),
        Scope::Tracks(_) => format!("{column} IN (SELECT value FROM json_each(?1))"),
    };
    let mut facts: Vec<TrackFacts> = Vec::new();
    let mut index: HashMap<i64, usize> = HashMap::new();
    {
        let mut statement = c.prepare(&format!(
            "SELECT t.id, t.album_id, coalesce(t.year, a.year), s.rating,
                    coalesce(s.play_count, 0), s.last_played
             FROM tracks t
             JOIN files f ON f.id = t.file_id
             LEFT JOIN albums a ON a.id = t.album_id
             LEFT JOIN stats s ON s.identity_id = t.identity_id
             WHERE {condition} AND (?1 IS NOT NULL)
             ORDER BY t.id"
        ))?;
        let mut rows = statement.query([&list])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let last_played: Option<i64> = row.get(5)?;
            index.insert(id, facts.len());
            facts.push(TrackFacts {
                album: row.get::<_, Option<i64>>(1)?.map(AlbumId::from_raw),
                year: row
                    .get::<_, Option<i64>>(2)?
                    .and_then(|y| i32::try_from(y).ok()),
                play: PlayFacts {
                    rating: row
                        .get::<_, Option<i64>>(3)?
                        .and_then(|r| u8::try_from(r).ok()),
                    plays: u32::try_from(row.get::<_, i64>(4)?.max(0)).unwrap_or(u32::MAX),
                    days_since_played: last_played
                        .map(|at| u32::try_from((now - at).max(0) / DAY).unwrap_or(u32::MAX)),
                },
                ..TrackFacts::new(track_id_from_sql(id))
            });
        }
    }
    let mut people = c.prepare(&format!(
        "SELECT track_id, person_id, role FROM track_people
         WHERE role IN (0, 1) AND {} AND (?1 IS NOT NULL) ORDER BY track_id, role, position",
        only("track_id")
    ))?;
    let mut rows = people.query([&list])?;
    while let Some(row) = rows.next()? {
        if let Some(&at) = index.get(&row.get::<_, i64>(0)?) {
            let person = PersonId::from_raw(row.get(1)?);
            if row.get::<_, i64>(2)? == 0 {
                facts[at].artists.push(person);
            } else {
                facts[at].album_artists.push(person);
            }
        }
    }
    let mut genres = c.prepare(&format!(
        "SELECT track_id, genre_id FROM track_genres WHERE {} AND (?1 IS NOT NULL)",
        only("track_id")
    ))?;
    let mut rows = genres.query([&list])?;
    while let Some(row) = rows.next()? {
        if let Some(&at) = index.get(&row.get::<_, i64>(0)?) {
            facts[at].genres.push(GenreId::from_raw(row.get(1)?));
        }
    }
    let mut labels = c.prepare(&format!(
        "SELECT t.id, l.label_id FROM label_tracks l
         JOIN tracks t ON t.identity_id = l.identity_id
         WHERE {} AND (?1 IS NOT NULL)",
        only("t.id")
    ))?;
    let mut rows = labels.query([&list])?;
    while let Some(row) = rows.next()? {
        if let Some(&at) = index.get(&row.get::<_, i64>(0)?) {
            facts[at].labels.push(row.get(1)?);
        }
    }
    Ok(facts)
}

/// The tracks of `seed`.
fn seed_tracks(c: &Connection, seed: PlaySeed) -> rusqlite::Result<Vec<i64>> {
    let (sql, id) = match seed {
        PlaySeed::Track(track) => return Ok(vec![sql_track_id(track)]),
        PlaySeed::Album(album) => (
            "SELECT id FROM tracks WHERE album_id = ?1 ORDER BY disc_no, track_no",
            album.get(),
        ),
        PlaySeed::Artist(person) => (
            "SELECT DISTINCT track_id FROM track_people
             WHERE person_id = ?1 AND role IN (0, 1) ORDER BY track_id",
            person.get(),
        ),
    };
    c.prepare_cached(sql)?
        .query_map([id], |row| row.get(0))?
        .collect()
}

/// A seed for the random picks from the time.
pub fn time_seed() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

impl Library {
    /// The statistics of `tracks` that weighted picks weigh, by track; tracks not in
    /// the library are left out.
    pub fn play_facts(&self, tracks: &[TrackId]) -> Result<HashMap<TrackId, PlayFacts>, DbError> {
        let ids: Vec<i64> = tracks.iter().copied().map(sql_track_id).collect();
        let now = to_unix_seconds(SystemTime::now());
        let facts = self
            .database()
            .read(|c| load_facts(c, &Scope::Tracks(&ids), now))?;
        Ok(facts.into_iter().map(|f| (f.id, f.play)).collect())
    }

    /// The tracks `tracks`, in that order, as lists show them; tracks not in the
    /// library are left out.
    pub fn summaries_of(&self, tracks: &[TrackId]) -> Result<Vec<crate::TrackSummary>, DbError> {
        let ids: Vec<i64> = tracks.iter().copied().map(sql_track_id).collect();
        self.database()
            .read(|c| crate::query::track_summaries(c, &ids))
    }

    /// What the picks know of `tracks`, in track order.
    pub fn track_facts(&self, tracks: &[TrackId]) -> Result<Vec<TrackFacts>, DbError> {
        let ids: Vec<i64> = tracks.iter().copied().map(sql_track_id).collect();
        let now = to_unix_seconds(SystemTime::now());
        self.database()
            .read(|c| load_facts(c, &Scope::Tracks(&ids), now))
    }

    /// The tracks the query `text` finds; every track for empty text.
    pub fn tracks_matching(&self, text: &str) -> Result<Option<HashSet<TrackId>>, PickError> {
        if text.trim().is_empty() {
            return Ok(None);
        }
        let query = parse_query(text)?;
        let request = QueryRequest {
            include_masked: false,
            ..QueryRequest::new(query)
        };
        Ok(Some(self.query_track_ids(&request)?.into_iter().collect()))
    }

    /// The music tracks the query `filter` finds (all for empty text), less `avoid`.
    fn pick_candidates(
        &self,
        filter: &str,
        avoid: &HashSet<TrackId>,
    ) -> Result<Vec<TrackFacts>, PickError> {
        let matching = self.tracks_matching(filter)?;
        let now = to_unix_seconds(SystemTime::now());
        let mut facts = self.database().read(|c| load_facts(c, &Scope::All, now))?;
        facts.retain(|f| {
            !avoid.contains(&f.id) && matching.as_ref().is_none_or(|m| m.contains(&f.id))
        });
        Ok(facts)
    }

    /// The profile of `seed`; empty when it names nothing.
    pub fn seed_profile(&self, seed: PlaySeed) -> Result<SeedProfile, DbError> {
        let now = to_unix_seconds(SystemTime::now());
        let facts = self.database().read(|c| {
            let ids = seed_tracks(c, seed)?;
            load_facts(c, &Scope::Tracks(&ids), now)
        })?;
        Ok(SeedProfile::of(&facts))
    }

    /// Up to `count` music tracks related to `seed`, for endless play: scored on
    /// the likenesses `settings` weighs (with `sound` for how alike they sound), drawn
    /// at random with the closest most likely, from the tracks the settings' filter
    /// finds, never one of `avoid` or of the seed itself. The same `seed_value` gives
    /// the same picks from the same library.
    pub fn related_tracks(
        &self,
        seed: PlaySeed,
        settings: &EndlessSettings,
        avoid: &HashSet<TrackId>,
        count: usize,
        seed_value: u64,
        sound: &dyn SoundSimilarity,
    ) -> Result<Vec<TrackId>, PickError> {
        let profile = self.seed_profile(seed)?;
        let mut avoid = avoid.clone();
        if matches!(seed, PlaySeed::Track(_)) {
            avoid.extend(&profile.tracks);
        }
        let candidates = self.pick_candidates(&settings.filter, &avoid)?;
        Ok(pick_related(
            &profile, candidates, settings, count, seed_value, sound,
        ))
    }

    /// Up to `count` music tracks for the Auto-DJ: from those its filter finds, never
    /// one of `avoid` or rated below its floor, drawn at random as its weights say.
    pub fn auto_dj_tracks(
        &self,
        settings: &AutoDjSettings,
        avoid: &HashSet<TrackId>,
        count: usize,
        seed_value: u64,
    ) -> Result<Vec<TrackId>, PickError> {
        let candidates = self.pick_candidates(&settings.filter, avoid)?;
        Ok(pick_auto_dj(
            candidates,
            &settings.weight_by,
            settings.min_rating,
            count,
            seed_value,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(id: u64, artist: i64, genre: i64, year: i32, rating: Option<u8>) -> TrackFacts {
        TrackFacts {
            artists: vec![PersonId::from_raw(artist)],
            album_artists: vec![PersonId::from_raw(artist)],
            genres: vec![GenreId::from_raw(genre)],
            year: Some(year),
            play: PlayFacts {
                rating,
                ..PlayFacts::default()
            },
            ..TrackFacts::new(TrackId::new(id))
        }
    }

    struct Fixed(f64);

    impl SoundSimilarity for Fixed {
        fn similarity(&self, _a: TrackId, b: TrackId) -> Option<f64> {
            b.get().is_multiple_of(2).then_some(self.0)
        }
    }

    #[test]
    fn closer_tracks_score_higher() {
        let seed = SeedProfile::of(&[facts(1, 10, 100, 1990, None)]);
        let weights = RelatedWeights::default();
        let same_artist = facts(2, 10, 100, 1991, None);
        let same_genre = facts(3, 11, 100, 1991, None);
        let nothing = facts(4, 12, 101, 2020, None);
        let score = |t: &TrackFacts| related_score(&seed, t, &weights, 5, &NoSoundSimilarity);
        assert!(score(&same_artist) > score(&same_genre));
        assert!(score(&same_genre) > score(&nothing));
        assert!((0.0..=1.0).contains(&score(&nothing)));
    }

    #[test]
    fn sound_is_one_more_input_and_left_out_when_unknown() {
        let seed = SeedProfile::of(&[facts(1, 10, 100, 1990, None)]);
        let weights = RelatedWeights {
            sound: 10,
            ..RelatedWeights::default()
        };
        let analysed = facts(4, 12, 101, 2020, None);
        let unanalysed = facts(5, 12, 101, 2020, None);
        let plain = related_score(&seed, &analysed, &weights, 5, &NoSoundSimilarity);
        let alike = related_score(&seed, &analysed, &weights, 5, &Fixed(1.0));
        let unlike = related_score(&seed, &analysed, &weights, 5, &Fixed(0.0));
        assert!(alike > plain && plain > unlike);
        let left_out = related_score(&seed, &unanalysed, &weights, 5, &Fixed(1.0));
        assert!((left_out - plain).abs() < 1e-9);
    }

    #[test]
    fn years_fade_outside_the_range() {
        assert!((year_closeness(1995, (1990, 1990), 5) - 1.0).abs() < 1e-9);
        assert!((year_closeness(1998, (1990, 1990), 5) - 0.4).abs() < 1e-9);
        assert!(year_closeness(2010, (1990, 1990), 5).abs() < 1e-9);
        assert!((year_closeness(1990, (1990, 1990), 0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn picks_are_deterministic_and_favour_the_related() {
        let seed = SeedProfile::of(&[facts(1, 10, 100, 1990, None)]);
        let settings = EndlessSettings::default();
        let candidates: Vec<TrackFacts> = (2..200)
            .map(|id| {
                let close = id % 4 == 0;
                facts(
                    id,
                    if close {
                        10 + (id as i64 % 3)
                    } else {
                        50 + id as i64
                    },
                    if close { 100 } else { 200 },
                    if close { 1990 } else { 2015 },
                    None,
                )
            })
            .collect();
        let a = pick_related(
            &seed,
            candidates.clone(),
            &settings,
            20,
            7,
            &NoSoundSimilarity,
        );
        let b = pick_related(
            &seed,
            candidates.clone(),
            &settings,
            20,
            7,
            &NoSoundSimilarity,
        );
        assert_eq!(a, b);
        assert_eq!(a.len(), 20);
        let unique: HashSet<_> = a.iter().collect();
        assert_eq!(unique.len(), 20);
        let close = a.iter().filter(|id| id.get() % 4 == 0).count();
        assert!(close >= 15, "{close} of 20 close");
        let c = pick_related(&seed, candidates, &settings, 20, 8, &NoSoundSimilarity);
        assert_ne!(a, c);
    }

    #[test]
    fn a_batch_is_not_one_artist_unless_nothing_else_is_there() {
        let seed = SeedProfile::of(&[facts(1, 10, 100, 1990, None)]);
        let settings = EndlessSettings::default();
        let mut candidates: Vec<TrackFacts> =
            (2..40).map(|id| facts(id, 10, 100, 1990, None)).collect();
        candidates.extend((40..60).map(|id| facts(id, 60 + id as i64, 300, 2020, None)));
        let picked = pick_related(&seed, candidates, &settings, 9, 1, &NoSoundSimilarity);
        let same = picked.iter().filter(|id| id.get() < 40).count();
        assert_eq!(picked.len(), 9);
        assert!(same <= 3, "{same}");
        let only_one: Vec<TrackFacts> = (2..12).map(|id| facts(id, 10, 100, 1990, None)).collect();
        let picked = pick_related(&seed, only_one, &settings, 6, 1, &NoSoundSimilarity);
        assert_eq!(picked.len(), 6);
    }

    #[test]
    fn auto_dj_weights_and_floors() {
        let candidates: Vec<TrackFacts> = (1..=200)
            .map(|id| facts(id, id as i64, 1, 2000, Some(if id <= 100 { 10 } else { 2 })))
            .collect();
        let picked = pick_auto_dj(candidates.clone(), &[PlayWeight::Rating], 0, 40, 3);
        let high = picked.iter().filter(|id| id.get() <= 100).count();
        assert!(high >= 30, "{high} of 40");
        let floored = pick_auto_dj(candidates, &[], 6, 300, 3);
        assert_eq!(floored.len(), 100);
        assert!(floored.iter().all(|id| id.get() <= 100));
    }
}
