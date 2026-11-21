// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! How alike tracks sound, from the sound features of the track analysis
//! ([`SoundFeatures`]); no network and no trained model.
//!
//! - Each feature is normalised over the library: its distance from the
//!   library's mean in standard deviations, limited to ±[`Z_LIMIT`]. While the
//!   library holds few analysed tracks, the deviation leans on a typical spread
//!   of each feature, so that two tracks of a tiny library are not made far
//!   apart only because they are the only two.
//! - The features are weighted in groups ([`WEIGHTS`]), so that the 24 timbre
//!   values do not outvote rhythm, tonality and dynamics.
//! - The similarity of two tracks is `exp(-d² / 2)`, where `d²` is the weighted
//!   mean of the squared normalised differences: 1 for the same sound, about
//!   0.37 for two tracks picked at random from the library, towards 0 for
//!   opposites.
//! - The tracks most like an album or an artist are those most like the mean of
//!   their tracks' normalised features.
//!
//! The [`SimilarityIndex`] keeps every analysed track's normalised vector in one
//! flat `f32` array (34 values, 136 bytes a track, 14 MB for 100 000 tracks), so
//! a query compares the seed with every track by brute force, in a few
//! milliseconds for a large library. The library builds it on first use and
//! keeps it until analyses are stored, or, at most every
//! [`REVALIDATE`] seconds, until the stored features look different (their count
//! or newest time changed, for example after another process analysed tracks).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helikodu_model::analysis::{SOUND_FEATURES_LEN, SoundFeatures};
use helikodu_model::library::TrackId;
use rusqlite::params;

use crate::Library;
use crate::catalog::{AlbumId, PersonId, track_id_from_sql};
use crate::db::DbError;

/// The weight of each feature (see [`SoundFeatures`] for the order): the
/// cepstral means share 3, their deviations 2, the rest about 1 each.
pub(crate) const WEIGHTS: [f32; SOUND_FEATURES_LEN] = {
    let mut weights = [0.0; SOUND_FEATURES_LEN];
    let mut i = 0;
    while i < 12 {
        weights[i] = 0.25;
        weights[12 + i] = 2.0 / 12.0;
        i += 1;
    }
    weights[24] = 1.0; // centroid
    weights[25] = 0.5; // centroid deviation
    weights[26] = 1.0; // rolloff
    weights[27] = 1.0; // flatness
    weights[28] = 0.5; // zero crossings
    weights[29] = 1.0; // onset rate
    weights[30] = 1.0; // flux
    weights[31] = 0.75; // chroma peakiness
    weights[32] = 0.75; // chroma entropy
    weights[33] = 0.75; // loudness spread
    weights
};

/// A typical spread of each feature across music, the prior of the
/// normalisation.
const PRIOR_SPREAD: [f32; SOUND_FEATURES_LEN] = {
    let mut spread = [0.0; SOUND_FEATURES_LEN];
    let mut i = 0;
    while i < 12 {
        spread[i] = 2.0;
        spread[12 + i] = 0.5;
        i += 1;
    }
    spread[24] = 0.7;
    spread[25] = 0.3;
    spread[26] = 0.8;
    spread[27] = 6.0;
    spread[28] = 0.7;
    spread[29] = 1.0;
    spread[30] = 0.1;
    spread[31] = 0.08;
    spread[32] = 0.05;
    spread[33] = 3.0;
    spread
};

/// How many tracks the prior counts for.
const PRIOR_TRACKS: f32 = 20.0;

/// The largest normalised deviation of a feature.
pub(crate) const Z_LIMIT: f32 = 4.0;

/// How often, at most, the cached index is checked against the database.
pub(crate) const REVALIDATE: Duration = Duration::from_secs(30);

/// A track and how alike it sounds, from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimilarTrack {
    /// The track.
    pub track: TrackId,
    /// Its similarity, from 0 (nothing alike) to 1 (the same sound).
    pub similarity: f32,
}

/// What to find similar tracks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimilaritySeed {
    /// One track; other copies of the same recording are left out.
    Track(TrackId),
    /// An album; its own tracks are left out.
    Album(AlbumId),
    /// An artist (as the first artist of tracks); the artist's own tracks are
    /// left out.
    Artist(PersonId),
}

/// The normalised sound features of every analysed track, for fast comparisons;
/// see the module documentation.
#[derive(Debug, Clone, Default)]
pub struct SimilarityIndex {
    tracks: Vec<TrackId>,
    /// The identity of each track, to leave out copies of a seed.
    identities: Vec<i64>,
    rows: HashMap<TrackId, usize>,
    /// The normalised and weighted vectors, one row of
    /// [`SOUND_FEATURES_LEN`] values per track.
    vectors: Vec<f32>,
    /// The sum of the weights.
    weight: f32,
}

impl SimilarityIndex {
    /// An index of `entries`: each track's identity (any number; tracks of the
    /// same identity are copies) and features.
    pub fn build(entries: impl IntoIterator<Item = (TrackId, i64, SoundFeatures)>) -> Self {
        let entries: Vec<(TrackId, i64, SoundFeatures)> = entries.into_iter().collect();
        #[allow(clippy::cast_precision_loss)]
        let n = entries.len() as f64;
        let mut mean = [0.0f64; SOUND_FEATURES_LEN];
        let mut squares = [0.0f64; SOUND_FEATURES_LEN];
        for (_, _, features) in &entries {
            for (i, &v) in features.values().iter().enumerate() {
                mean[i] += f64::from(v);
                squares[i] += f64::from(v) * f64::from(v);
            }
        }
        let mut scale = [0.0f32; SOUND_FEATURES_LEN];
        let mut centre = [0.0f32; SOUND_FEATURES_LEN];
        for i in 0..SOUND_FEATURES_LEN {
            let (m, variance) = if n > 0.0 {
                let m = mean[i] / n;
                (m, (squares[i] / n - m * m).max(0.0))
            } else {
                (0.0, 0.0)
            };
            let prior = f64::from(PRIOR_SPREAD[i]).powi(2);
            let k = f64::from(PRIOR_TRACKS);
            let blended = ((n * variance + k * prior) / (n + k)).sqrt().max(1e-6);
            #[allow(clippy::cast_possible_truncation)]
            {
                centre[i] = m as f32;
                scale[i] = WEIGHTS[i].sqrt() / blended as f32;
            }
        }
        let mut index = Self {
            tracks: Vec::with_capacity(entries.len()),
            identities: Vec::with_capacity(entries.len()),
            rows: HashMap::with_capacity(entries.len()),
            vectors: Vec::with_capacity(entries.len() * SOUND_FEATURES_LEN),
            weight: WEIGHTS.iter().sum(),
        };
        for (track, identity, features) in entries {
            if index.rows.contains_key(&track) {
                continue;
            }
            index.rows.insert(track, index.tracks.len());
            index.tracks.push(track);
            index.identities.push(identity);
            for (i, &v) in features.values().iter().enumerate() {
                let limit = Z_LIMIT * WEIGHTS[i].sqrt();
                index
                    .vectors
                    .push(((v - centre[i]) * scale[i]).clamp(-limit, limit));
            }
        }
        index
    }

    /// The number of tracks.
    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Whether it holds no track.
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Whether `track` has features.
    pub fn contains(&self, track: TrackId) -> bool {
        self.rows.contains_key(&track)
    }

    fn row(&self, at: usize) -> &[f32] {
        &self.vectors[at * SOUND_FEATURES_LEN..(at + 1) * SOUND_FEATURES_LEN]
    }

    fn score(&self, squared: f32) -> f32 {
        (-squared / (2.0 * self.weight)).exp().clamp(0.0, 1.0)
    }

    /// How alike `a` and `b` sound, from 0 to 1; `None` when either has no
    /// features.
    pub fn similarity(&self, a: TrackId, b: TrackId) -> Option<f32> {
        let (a, b) = (*self.rows.get(&a)?, *self.rows.get(&b)?);
        Some(self.score(squared_distance(self.row(a), self.row(b))))
    }

    /// The `limit` tracks most like `seeds` (the mean of their vectors), most
    /// alike first. The seeds, the tracks of the same identity as a seed and
    /// those in `exclude` are left out. Empty when no seed has features.
    pub fn nearest(
        &self,
        seeds: &[TrackId],
        limit: usize,
        exclude: &HashSet<TrackId>,
    ) -> Vec<SimilarTrack> {
        let seed_rows: Vec<usize> = seeds
            .iter()
            .filter_map(|track| self.rows.get(track).copied())
            .collect();
        if seed_rows.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut centre = [0.0f32; SOUND_FEATURES_LEN];
        for &at in &seed_rows {
            for (c, v) in centre.iter_mut().zip(self.row(at)) {
                *c += v;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let count = seed_rows.len() as f32;
        for c in &mut centre {
            *c /= count;
        }
        let skip_identities: HashSet<i64> =
            seed_rows.iter().map(|&at| self.identities[at]).collect();
        let skip_rows: HashSet<usize> = seed_rows.into_iter().collect();
        let mut found: Vec<(f32, u32)> = self
            .vectors
            .as_chunks::<SOUND_FEATURES_LEN>()
            .0
            .iter()
            .enumerate()
            .filter(|(at, _)| {
                !skip_rows.contains(at)
                    && !skip_identities.contains(&self.identities[*at])
                    && !exclude.contains(&self.tracks[*at])
            })
            .map(|(at, row)| {
                (
                    squared_distance(&centre, row),
                    u32::try_from(at).unwrap_or(u32::MAX),
                )
            })
            .collect();
        let by_distance = |a: &(f32, u32), b: &(f32, u32)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1));
        if found.len() > limit {
            found.select_nth_unstable_by(limit, by_distance);
            found.truncate(limit);
        }
        found.sort_unstable_by(by_distance);
        found
            .into_iter()
            .map(|(squared, at)| SimilarTrack {
                track: self.tracks[at as usize],
                similarity: self.score(squared),
            })
            .collect()
    }
}

fn squared_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// The index the library keeps; see the module documentation.
#[derive(Debug, Default)]
pub(crate) struct SimilarityCache {
    generation: AtomicU64,
    cached: Mutex<Option<Cached>>,
}

#[derive(Debug)]
struct Cached {
    index: Arc<SimilarityIndex>,
    generation: u64,
    stamp: (i64, i64),
    checked: Instant,
}

/// The SQL condition that the analysis `a` of track `t` in file `f` is current.
const CURRENT: &str = "a.file_size IS f.size AND a.file_mtime_ns IS f.mtime_ns
     AND a.start_frame IS t.start_frame AND a.end_frame IS t.end_frame";

impl Library {
    /// Forgets the cached similarity index after analyses changed.
    pub(crate) fn similarity_changed(&self) {
        self.similarity_cache()
            .generation
            .fetch_add(1, Ordering::Relaxed);
    }

    /// The similarity index of the library's analysed tracks whose files are
    /// present, built on first use and kept while the analyses stay as they were.
    pub fn similarity_index(&self) -> Result<Arc<SimilarityIndex>, DbError> {
        let cache = self.similarity_cache();
        let mut cached = cache.cached.lock().unwrap_or_else(PoisonError::into_inner);
        let generation = cache.generation.load(Ordering::Relaxed);
        if let Some(found) = cached.as_mut().filter(|c| c.generation == generation) {
            if found.checked.elapsed() < REVALIDATE {
                return Ok(Arc::clone(&found.index));
            }
            let stamp = self.features_stamp()?;
            found.checked = Instant::now();
            if stamp == found.stamp {
                return Ok(Arc::clone(&found.index));
            }
        }
        let stamp = self.features_stamp()?;
        let entries = self.database().read(|c| {
            let mut statement = c.prepare_cached(&format!(
                "SELECT a.track_id, t.identity_id, a.features
                 FROM track_analysis a
                 JOIN tracks t ON t.id = a.track_id
                 JOIN files f ON f.id = t.file_id
                 WHERE a.features IS NOT NULL AND a.failure IS NULL
                   AND f.missing_since IS NULL AND {CURRENT}"
            ))?;
            let rows = statement.query_map([], |row| {
                let bytes: Vec<u8> = row.get(2)?;
                let track = track_id_from_sql(row.get(0)?);
                let identity: i64 = row.get(1)?;
                Ok(SoundFeatures::from_bytes(&bytes).map(|features| (track, identity, features)))
            })?;
            let mut entries = Vec::new();
            for row in rows {
                entries.extend(row?);
            }
            Ok(entries)
        })?;
        let index = Arc::new(SimilarityIndex::build(entries));
        *cached = Some(Cached {
            index: Arc::clone(&index),
            generation,
            stamp,
            checked: Instant::now(),
        });
        Ok(index)
    }

    /// The count and the newest time of the stored features.
    fn features_stamp(&self) -> Result<(i64, i64), DbError> {
        self.database().read(|c| {
            c.prepare_cached(
                "SELECT count(*), coalesce(max(analyzed_at), 0) FROM track_analysis
                 WHERE features IS NOT NULL",
            )?
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
        })
    }

    /// How alike `a` and `b` sound, from 0 to 1; `None` when either track has no
    /// current analysis with sound features (not analysed yet, silent, or its
    /// file is missing).
    pub fn track_similarity(&self, a: TrackId, b: TrackId) -> Result<Option<f32>, DbError> {
        Ok(self.similarity_index()?.similarity(a, b))
    }

    /// The `limit` tracks that sound most like `seed`, most alike first. Empty
    /// when no track of the seed has sound features.
    pub fn similar_tracks(
        &self,
        seed: SimilaritySeed,
        limit: usize,
    ) -> Result<Vec<SimilarTrack>, DbError> {
        let seeds = self.seed_tracks(seed)?;
        let exclude: HashSet<TrackId> = match seed {
            SimilaritySeed::Track(_) => HashSet::new(),
            SimilaritySeed::Album(_) | SimilaritySeed::Artist(_) => seeds.iter().copied().collect(),
        };
        Ok(self.similarity_index()?.nearest(&seeds, limit, &exclude))
    }

    /// The tracks of `seed`.
    fn seed_tracks(&self, seed: SimilaritySeed) -> Result<Vec<TrackId>, DbError> {
        let (sql, id) = match seed {
            SimilaritySeed::Track(track) => return Ok(vec![track]),
            SimilaritySeed::Album(album) => ("SELECT id FROM tracks WHERE album_id = ?1", album.0),
            SimilaritySeed::Artist(person) => (
                "SELECT track_id FROM track_people
                 WHERE person_id = ?1 AND role = 0 AND position = 0",
                person.0,
            ),
        };
        self.database().read(|c| {
            let mut statement = c.prepare_cached(sql)?;
            let rows = statement.query_map(params![id], |row| {
                row.get::<_, i64>(0).map(track_id_from_sql)
            })?;
            rows.collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(mut f: impl FnMut(usize) -> f32) -> SoundFeatures {
        let mut values = [0.0; SOUND_FEATURES_LEN];
        for (i, v) in values.iter_mut().enumerate() {
            *v = f(i);
        }
        SoundFeatures(values)
    }

    /// A deterministic pseudo-random source.
    struct Random(u64);

    impl Random {
        fn unit(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            #[allow(clippy::cast_precision_loss)]
            let unit = (self.0 >> 40) as f32 / (1u64 << 24) as f32;
            unit
        }
    }

    fn random_library(n: u64, seed: u64) -> Vec<(TrackId, i64, SoundFeatures)> {
        let mut random = Random(seed);
        (1..=n)
            .map(|i| {
                let f = features(|d| (random.unit() - 0.5) * 2.0 * PRIOR_SPREAD[d]);
                #[allow(clippy::cast_possible_wrap)]
                (TrackId::new(i), i as i64, f)
            })
            .collect()
    }

    #[test]
    fn similarity_is_one_for_the_same_sound_and_falls_with_distance() {
        let mut entries = random_library(200, 1);
        let base = entries[0].2;
        let near = features(|i| base.0[i] + 0.05 * PRIOR_SPREAD[i]);
        let far = features(|i| base.0[i] + 3.0 * PRIOR_SPREAD[i]);
        entries.push((TrackId::new(1001), 1001, base));
        entries.push((TrackId::new(1002), 1002, near));
        entries.push((TrackId::new(1003), 1003, far));
        let index = SimilarityIndex::build(entries);
        let first = TrackId::new(1);
        let same = index.similarity(first, TrackId::new(1001)).unwrap();
        let close = index.similarity(first, TrackId::new(1002)).unwrap();
        let apart = index.similarity(first, TrackId::new(1003)).unwrap();
        assert!((same - 1.0).abs() < 1e-6, "{same}");
        assert!(close > 0.95, "{close}");
        assert!(apart < 0.1, "{apart}");
        assert_eq!(index.similarity(first, TrackId::new(9999)), None);
        assert_eq!(
            index.similarity(first, TrackId::new(2)),
            index.similarity(TrackId::new(2), first)
        );
        // Random pairs are near exp(-1).
        let mut sum = 0.0;
        for i in 2..=101 {
            sum += index.similarity(first, TrackId::new(i)).unwrap();
        }
        let mean = sum / 100.0;
        assert!((0.15..0.6).contains(&mean), "{mean}");
    }

    #[test]
    fn nearest_tracks_come_most_alike_first_without_seeds_or_copies() {
        let mut entries = random_library(100, 2);
        let base = entries[4].2;
        for (k, step) in [0.3f32, 0.1, 0.2].into_iter().enumerate() {
            let id = 500 + k as u64;
            let f = features(|i| base.0[i] + step * PRIOR_SPREAD[i]);
            entries.push((TrackId::new(id), id as i64, f));
        }
        // A copy of the seed: same identity.
        entries.push((TrackId::new(600), 5, base));
        let index = SimilarityIndex::build(entries);
        let found = index.nearest(&[TrackId::new(5)], 3, &HashSet::new());
        let ids: Vec<u64> = found.iter().map(|s| s.track.get()).collect();
        assert_eq!(ids, [501, 502, 500]);
        assert!(found.windows(2).all(|w| w[0].similarity >= w[1].similarity));
        let exclude: HashSet<TrackId> = [TrackId::new(501)].into();
        let found = index.nearest(&[TrackId::new(5)], 2, &exclude);
        let ids: Vec<u64> = found.iter().map(|s| s.track.get()).collect();
        assert_eq!(ids, [502, 500]);
        assert!(
            index
                .nearest(&[TrackId::new(9999)], 5, &HashSet::new())
                .is_empty()
        );
    }

    #[test]
    fn a_tiny_library_leans_on_the_prior() {
        let a = features(|i| PRIOR_SPREAD[i] * 0.1);
        let b = features(|_| 0.0);
        let index = SimilarityIndex::build([(TrackId::new(1), 1, a), (TrackId::new(2), 2, b)]);
        let s = index.similarity(TrackId::new(1), TrackId::new(2)).unwrap();
        assert!(s > 0.9, "{s}");
    }

    /// The brute-force query on a synthetic library of 20 000 tracks; a library
    /// of 100 000 takes five times as long.
    #[test]
    fn queries_stay_fast() {
        let entries = random_library(20_000, 3);
        let started = Instant::now();
        let index = SimilarityIndex::build(entries);
        let built = started.elapsed();
        let started = Instant::now();
        let rounds = 10;
        for i in 0..rounds {
            let found = index.nearest(&[TrackId::new(1 + i * 100)], 25, &HashSet::new());
            assert_eq!(found.len(), 25);
        }
        let per_query = started.elapsed() / u32::try_from(rounds).unwrap();
        eprintln!("20 000 tracks: built in {built:?}, {per_query:?} a query");
        // Generous, for unoptimised builds on slow machines.
        assert!(per_query < Duration::from_millis(500), "{per_query:?}");
    }
}
