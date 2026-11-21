// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Invented generator playlists and tracks that sound alike.
//!
//! The steps run over the sample tracks: a "where" step keeps a stable share
//! of them that follows its text (the sample does not read queries beyond
//! checking that they can be read), sorts use the rows' values and invented
//! play dates and skips, caps and limits work as in the library. The sample
//! has one generator, "Forgotten favourites", with the steps of the design.
//!
//! Tracks that sound alike get invented scores that follow the two tracks'
//! keys. As in the invented analysis, one track in seven could not be read
//! and one in seven is not analysed yet; a library of fewer than 247 tracks
//! has too few analysed (the sample counts 81 in 100 as analysed).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use super::SampleLibrary;
use super::playlists::Entry;
use crate::backend::generators::{SIMILAR_FLOOR, too_few_analysed};
use crate::backend::{
    BackendError, GeneratorInfo, GeneratorRun, GeneratorSort, GeneratorSource, GeneratorStep,
    GeneratorSteps, GeneratorTrack, NotYet, PlaylistKey, PlaylistType, SimilarFound, SimilarRow,
    SimilarSeed, StepProblem, TrackKey, TrackRow,
};

/// The steps and the last result of a sample generator.
#[derive(Debug, Clone)]
pub(in crate::backend::sample) struct SampleGenerator {
    pub(in crate::backend::sample) steps: GeneratorSteps,
    pub(in crate::backend::sample) on_open: bool,
    pub(in crate::backend::sample) last: Option<Vec<TrackKey>>,
    pub(in crate::backend::sample) made: Option<SystemTime>,
}

impl SampleGenerator {
    /// The generator of the design.
    pub(in crate::backend::sample) fn forgotten_favourites() -> Self {
        let steps = "from library\n\
                     where rating>=4\n\
                     exclude playlist \"Heard this week\"\n\
                     sort last-played\n\
                     per-artist 2\n\
                     per-album 1\n\
                     limit 50"
            .parse()
            .unwrap_or_default();
        Self {
            steps,
            on_open: true,
            last: None,
            made: None,
        }
    }

    fn new() -> Self {
        Self {
            steps: GeneratorSteps::default(),
            on_open: true,
            last: None,
            made: None,
        }
    }
}

/// A stable spread of `key` with `salt`.
fn spread(key: u64, salt: u64) -> u64 {
    let mut z = key
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(salt.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn text_salt(text: &str) -> u64 {
    text.bytes()
        .fold(7_u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)))
}

/// When the sample says `key` last played: up to two and a half years ago.
pub(in crate::backend::sample) fn last_played(key: TrackKey) -> Option<SystemTime> {
    if key.0.is_multiple_of(11) {
        return None;
    }
    let days = spread(key.0, 3) % 900;
    super::sample_now().checked_sub(Duration::from_secs(86_400 * (days + 3)))
}

fn skips(key: TrackKey) -> u32 {
    (spread(key.0, 5) % 7) as u32
}

/// Whether the sample says `key` was analysed, could not be read, or neither.
fn analysis_of(key: TrackKey) -> Option<NotYet> {
    match key.0 % 7 {
        0 => Some(NotYet::Unreadable),
        1 => Some(NotYet::NotAnalysed),
        _ => None,
    }
}

impl SampleLibrary {
    fn generator_track(&self, key: TrackKey) -> Option<GeneratorTrack> {
        Some(GeneratorTrack {
            row: self.track(key)?,
            last_played: last_played(key),
            skips: skips(key),
        })
    }

    fn all_track_keys(&self) -> Vec<TrackKey> {
        (1..=self.tracks.len() as u64).map(TrackKey).collect()
    }

    /// The tracks of the playlist, smart playlist or generator named `name`.
    fn named_tracks(
        &self,
        name: &str,
        using: &mut Vec<String>,
    ) -> Result<Vec<TrackKey>, StepProblem> {
        let found = self
            .lists()
            .lists
            .iter()
            .find(|p| p.kind != PlaylistType::Folder && p.name.eq_ignore_ascii_case(name))
            .map(|p| (p.kind, p.entries.clone(), p.key, p.generator.clone()));
        let Some((kind, entries, key, generator)) = found else {
            return Err(StepProblem::MissingPlaylist(name.to_owned()));
        };
        match kind {
            PlaylistType::Smart => {
                let query = self.sample_query(key).unwrap_or_default();
                Ok(self.sample_matches(&query))
            }
            PlaylistType::Generator => {
                if using.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                    return Err(StepProblem::Other(format!(
                        "the generator {name:?} uses itself"
                    )));
                }
                let generator = generator.unwrap_or_else(SampleGenerator::new);
                if let (Some(last), false) = (&generator.last, generator.on_open) {
                    return Ok(last.clone());
                }
                using.push(name.to_owned());
                let (tracks, _, problem) = self.run_sample_steps(&generator.steps, using);
                using.pop();
                match problem {
                    Some((_, problem)) => Err(problem),
                    None => Ok(tracks),
                }
            }
            _ => Ok(entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Track(k) => Some(*k),
                    Entry::Missing(_) => None,
                })
                .collect()),
        }
    }

    /// Runs `steps`: the tracks, the counts after the source and each step, and
    /// the step that could not run.
    fn run_sample_steps(
        &self,
        steps: &GeneratorSteps,
        using: &mut Vec<String>,
    ) -> (Vec<TrackKey>, Vec<usize>, Option<(usize, StepProblem)>) {
        let mut counts = Vec::new();
        let mut tracks = match &steps.source {
            GeneratorSource::Library => self.all_track_keys(),
            GeneratorSource::Playlist(name) => match self.named_tracks(name, using) {
                Ok(tracks) => tracks,
                Err(problem) => return (Vec::new(), counts, Some((0, problem))),
            },
            GeneratorSource::Query(text) => match helikodu_service::library::parse_query(text) {
                Ok(query) => self.sample_matches(&query),
                Err(e) => {
                    return (
                        Vec::new(),
                        counts,
                        Some((0, StepProblem::Query(e.to_string()))),
                    );
                }
            },
        };
        let mut seen = HashSet::new();
        tracks.retain(|t| seen.insert(*t));
        counts.push(tracks.len());
        for (index, step) in steps.steps.iter().enumerate() {
            let number = index + 1;
            match step {
                GeneratorStep::Where(text) => {
                    if let Err(e) = helikodu_service::library::parse_query(text) {
                        return (
                            tracks,
                            counts,
                            Some((number, StepProblem::Query(e.to_string()))),
                        );
                    }
                    let salt = text_salt(text);
                    tracks.retain(|t| spread(t.0, salt) % 10 < 4);
                }
                GeneratorStep::Exclude(name) => match self.named_tracks(name, using) {
                    Ok(gone) => {
                        let gone: HashSet<TrackKey> = gone.into_iter().collect();
                        tracks.retain(|t| !gone.contains(t));
                    }
                    Err(problem) => return (tracks, counts, Some((number, problem))),
                },
                GeneratorStep::Sort {
                    by: GeneratorSort::Random,
                    ..
                } => {
                    let salt = number as u64 + 17;
                    tracks.sort_by_key(|t| spread(t.0, salt));
                }
                GeneratorStep::Sort { by, descending } => {
                    let rows: HashMap<TrackKey, TrackRow> = tracks
                        .iter()
                        .filter_map(|t| Some((*t, self.track(*t)?)))
                        .collect();
                    tracks.sort_by(|a, b| {
                        let order = compare(*by, *a, *b, &rows);
                        if *descending { order.reverse() } else { order }
                    });
                }
                GeneratorStep::PerArtist(cap) | GeneratorStep::PerAlbum(cap) => {
                    let artist = matches!(step, GeneratorStep::PerArtist(_));
                    let mut counted: HashMap<String, usize> = HashMap::new();
                    tracks.retain(|t| {
                        let Some(row) = self.track(*t) else {
                            return true;
                        };
                        let key = if artist {
                            row.artist
                        } else {
                            row.album.0.to_string()
                        };
                        let n = counted.entry(key).or_default();
                        *n += 1;
                        *n <= *cap
                    });
                }
                GeneratorStep::Limit(n) => tracks.truncate(*n),
                GeneratorStep::LimitMinutes(minutes) => {
                    let budget = Duration::from_secs(minutes * 60);
                    let mut total = Duration::ZERO;
                    let keep = tracks
                        .iter()
                        .take_while(|t| {
                            total += self.track(**t).and_then(|r| r.duration).unwrap_or_default();
                            total <= budget
                        })
                        .count();
                    tracks.truncate(keep);
                }
            }
            counts.push(tracks.len());
        }
        (tracks, counts, None)
    }

    fn generator_rows(&self, tracks: &[TrackKey]) -> Vec<GeneratorTrack> {
        tracks
            .iter()
            .filter_map(|k| self.generator_track(*k))
            .collect()
    }

    fn sample_generator(
        &self,
        key: PlaylistKey,
    ) -> Result<(String, SampleGenerator), BackendError> {
        let mut lists = self.lists();
        let list = lists.get(key)?;
        let generator = list
            .generator
            .clone()
            .ok_or_else(|| BackendError("not a generator playlist".into()))?;
        Ok((list.name.clone(), generator))
    }

    pub(super) fn sample_generator_info(
        &self,
        key: PlaylistKey,
    ) -> Result<Option<GeneratorInfo>, BackendError> {
        Ok(self
            .sample_generator(key)
            .ok()
            .map(|(name, g)| GeneratorInfo {
                key,
                name,
                steps: g.steps,
                on_open: g.on_open,
            }))
    }

    pub(super) fn sample_new_generator(
        &self,
        name: &str,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        let mut lists = self.lists();
        let key = lists.add(name, PlaylistType::Generator, parent);
        lists.get(key)?.generator = Some(SampleGenerator::new());
        Ok(key)
    }

    pub(super) fn sample_generator_tracks(
        &self,
        key: PlaylistKey,
        again: bool,
    ) -> Result<GeneratorRun, BackendError> {
        let (name, generator) = self.sample_generator(key)?;
        let make = again || generator.on_open || generator.last.is_none();
        if !make {
            return Ok(GeneratorRun {
                tracks: self.generator_rows(generator.last.as_deref().unwrap_or_default()),
                made: generator.made,
                ..GeneratorRun::default()
            });
        }
        let (tracks, counts, problem) = self.run_sample_steps(&generator.steps, &mut vec![name]);
        if let Some(problem) = problem {
            return Ok(GeneratorRun {
                tracks: self.generator_rows(generator.last.as_deref().unwrap_or_default()),
                counts,
                problem: Some(problem),
                made: generator.made,
                fresh: false,
            });
        }
        let now = SystemTime::now();
        if let Some(stored) = self.lists().get(key)?.generator.as_mut() {
            stored.last = Some(tracks.clone());
            stored.made = Some(now);
        }
        Ok(GeneratorRun {
            tracks: self.generator_rows(&tracks),
            counts,
            problem: None,
            made: Some(now),
            fresh: true,
        })
    }

    pub(super) fn sample_try_generator(
        &self,
        key: PlaylistKey,
        steps: &GeneratorSteps,
    ) -> Result<GeneratorRun, BackendError> {
        let name = self.sample_generator(key).map(|(n, _)| n).ok();
        let (tracks, counts, problem) =
            self.run_sample_steps(steps, &mut name.into_iter().collect());
        Ok(GeneratorRun {
            tracks: self.generator_rows(&tracks),
            counts,
            problem,
            made: None,
            fresh: true,
        })
    }

    pub(super) fn sample_set_generator(
        &self,
        key: PlaylistKey,
        steps: Option<&GeneratorSteps>,
        on_open: Option<bool>,
    ) -> Result<(), BackendError> {
        let changed_steps = {
            let mut lists = self.lists();
            let list = lists.get(key)?;
            let generator = list
                .generator
                .as_mut()
                .ok_or_else(|| BackendError("not a generator playlist".into()))?;
            if let Some(on) = on_open {
                generator.on_open = on;
            }
            match steps {
                Some(steps) if *steps != generator.steps => {
                    generator.steps = steps.clone();
                    generator.last = None;
                    generator.made = None;
                    true
                }
                _ => false,
            }
        };
        if changed_steps {
            self.sample_generator_tracks(key, true)?;
        }
        Ok(())
    }

    /// The last result of a generator, as playlist entries.
    pub(super) fn sample_generator_items(&self, key: PlaylistKey) -> Option<Vec<TrackRow>> {
        let generator = self.lists().get(key).ok()?.generator.clone()?;
        Some(
            generator
                .last
                .unwrap_or_default()
                .iter()
                .enumerate()
                .filter_map(|(i, k)| {
                    let mut row = self.track(*k)?;
                    row.entry = Some(i as u64);
                    row.from_context = true;
                    Some(row)
                })
                .collect(),
        )
    }

    /// The tracks of the seed.
    fn seed_tracks(&self, seed: SimilarSeed) -> Vec<TrackKey> {
        match seed {
            SimilarSeed::Track(key) => vec![key],
            SimilarSeed::Album(album) => self
                .get(album)
                .map(|a| a.tracks.iter().map(|t| t.key).collect())
                .unwrap_or_default(),
            SimilarSeed::Artist(artist) => {
                let index = usize::try_from(artist.0).unwrap_or(0).wrapping_sub(1);
                self.albums
                    .iter()
                    .filter(|a| a.artist == index)
                    .flat_map(|a| a.tracks.iter().map(|t| t.key))
                    .collect()
            }
        }
    }

    pub(super) fn sample_similar(
        &self,
        seed: SimilarSeed,
        limit: usize,
    ) -> Result<SimilarFound, BackendError> {
        let own = self.seed_tracks(seed);
        let analysed: Vec<TrackKey> = own
            .iter()
            .copied()
            .filter(|k| analysis_of(*k).is_none())
            .collect();
        if analysed.is_empty() {
            let unreadable = !own.is_empty()
                && own
                    .iter()
                    .all(|k| analysis_of(*k) == Some(NotYet::Unreadable));
            return Ok(SimilarFound::NotYet(if unreadable {
                NotYet::Unreadable
            } else {
                NotYet::NotAnalysed
            }));
        }
        let total = self.tracks.len();
        let counted = total * 81 / 100;
        if too_few_analysed(counted, total) {
            return Ok(SimilarFound::NotYet(NotYet::TooFew(counted)));
        }
        let own: HashSet<TrackKey> = own.into_iter().collect();
        let salt = analysed.iter().fold(0_u64, |h, k| h ^ k.0);
        // Most tracks are far; a few hundred come close. The closest ones
        // lean towards the seed's own album and artist.
        let mut scored: Vec<(TrackKey, f32)> = self
            .all_track_keys()
            .into_iter()
            .filter(|k| !own.contains(k) && analysis_of(*k).is_none())
            .map(|k| {
                let base = (spread(k.0, salt) % 1000) as f32 / 1000.0;
                let score = 0.97 - base.powf(0.25) * 0.6;
                (k, score)
            })
            .filter(|(_, s)| *s >= SIMILAR_FLOOR)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let more = scored.len() > limit;
        Ok(SimilarFound::Tracks {
            rows: scored
                .into_iter()
                .take(limit)
                .filter_map(|(k, score)| {
                    Some(SimilarRow {
                        row: self.track(k)?,
                        score,
                    })
                })
                .collect(),
            more,
        })
    }
}

/// The order of two sample tracks by `by`, missing values last.
fn compare(
    by: GeneratorSort,
    a: TrackKey,
    b: TrackKey,
    rows: &HashMap<TrackKey, TrackRow>,
) -> std::cmp::Ordering {
    let (Some(x), Some(y)) = (rows.get(&a), rows.get(&b)) else {
        return std::cmp::Ordering::Equal;
    };
    match by {
        GeneratorSort::Rating => x.rating.cmp(&y.rating),
        GeneratorSort::Plays => x.plays.cmp(&y.plays),
        GeneratorSort::Skips => skips(a).cmp(&skips(b)),
        GeneratorSort::LastPlayed => match (last_played(a), last_played(b)) {
            (Some(p), Some(q)) => p.cmp(&q),
            (p, q) => q.is_none().cmp(&p.is_none()).reverse(),
        },
        GeneratorSort::Added => x.added.cmp(&y.added),
        GeneratorSort::Year => x.year.cmp(&y.year),
        GeneratorSort::Title => x.title.to_lowercase().cmp(&y.title.to_lowercase()),
        GeneratorSort::Artist => x.artist.to_lowercase().cmp(&y.artist.to_lowercase()),
        GeneratorSort::Album => x
            .album_title
            .to_lowercase()
            .cmp(&y.album_title.to_lowercase()),
        GeneratorSort::Length => x.duration.cmp(&y.duration),
        GeneratorSort::Random => std::cmp::Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_generator_runs_its_steps() {
        let library = SampleLibrary::new(300);
        let key = library
            .sample_playlists()
            .into_iter()
            .find(|p| p.kind == PlaylistType::Generator)
            .map(|p| p.key)
            .unwrap();
        let run = library.sample_generator_tracks(key, false).unwrap();
        assert!(run.problem.is_none());
        assert_eq!(run.tracks.len(), 50);
        assert_eq!(run.counts.len(), 7);
        assert_eq!(run.counts.last(), Some(&50));
        // Without the excluded playlist the step cannot run, and the last
        // result stays.
        let heard = library
            .sample_playlists()
            .into_iter()
            .find(|p| p.name == "Heard this week")
            .unwrap();
        library.sample_delete(heard.key);
        let again = library.sample_generator_tracks(key, true).unwrap();
        assert_eq!(
            again.problem,
            Some((2, StepProblem::MissingPlaylist("Heard this week".into())))
        );
        assert_eq!(again.tracks.len(), 50);
    }

    #[test]
    fn similar_tracks_have_their_states() {
        let library = SampleLibrary::new(300);
        let found = library
            .sample_similar(SimilarSeed::Track(TrackKey(2)), 50)
            .unwrap();
        let SimilarFound::Tracks { rows, more } = found else {
            panic!("a list");
        };
        assert_eq!(rows.len(), 50);
        assert!(more);
        assert!(rows.windows(2).all(|w| w[0].score >= w[1].score));
        assert!(rows.iter().all(|r| r.score >= SIMILAR_FLOOR));
        assert_eq!(
            library.sample_similar(SimilarSeed::Track(TrackKey(7)), 50),
            Ok(SimilarFound::NotYet(NotYet::Unreadable))
        );
        assert_eq!(
            library.sample_similar(SimilarSeed::Track(TrackKey(8)), 50),
            Ok(SimilarFound::NotYet(NotYet::NotAnalysed))
        );
        let small = SampleLibrary::new(10);
        assert!(matches!(
            small.sample_similar(SimilarSeed::Track(TrackKey(2)), 50),
            Ok(SimilarFound::NotYet(NotYet::TooFew(_)))
        ));
    }

    #[test]
    fn a_new_generator_starts_from_the_library() {
        let library = SampleLibrary::new(30);
        let key = library.sample_new_generator("New", None).unwrap();
        let info = library.sample_generator_info(key).unwrap().unwrap();
        assert_eq!(info.steps, GeneratorSteps::default());
    }
}
