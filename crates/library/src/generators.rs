// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generator playlists: playlists defined by steps and filled by running them.
//!
//! The steps start with a source and go on with filters, sorts, caps and limits,
//! each working on the tracks the one before left. Their text form has one step per
//! line; a line that starts with `#` is a comment:
//!
//! ```text
//! # The source: library, playlist "Name" or query <query>.
//! from library
//! # Keeps what the query finds (the search field's language).
//! where genre:Jazz rating>=3
//! # Drops the tracks of a playlist, smart playlist or generator.
//! exclude playlist "Heard it"
//! # rating, plays, skips, last-played, added, year, title, artist, album, length
//! # or random; then asc (the default) or desc.
//! sort last-played
//! per-artist 2
//! per-album 1
//! # Or: limit 90 minutes.
//! limit 50
//! ```
//!
//! A generator keeps its last result. It is made again each time it is opened when
//! `on_open` is set, and otherwise only when asked.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;
use std::time::SystemTime;

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension};

use crate::catalog::ids::{sql_track_id, track_id_from_sql};
use crate::catalog::roots::visibility;
use crate::db::DbError;
use crate::db::unix_time::{from_unix_seconds, to_unix_seconds};
use crate::library::Library;
use crate::playlists::PlaylistId;
use crate::query::{QueryError, QueryRequest, SyntaxError, parse_query};

/// Identifies a generator playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GeneratorId(i64);

impl GeneratorId {
    /// The number of the generator in the database.
    pub fn get(self) -> i64 {
        self.0
    }

    /// The generator numbered `id`, as [`get`](Self::get) gives it.
    pub fn from_raw(id: i64) -> Self {
        Self(id)
    }
}

impl fmt::Display for GeneratorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Where a generator's tracks come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratorSource {
    /// Every music track of the library, by album.
    Library,
    /// The tracks of a playlist, smart playlist or other generator, by name.
    Playlist(String),
    /// The tracks a query finds, in its order.
    Query(String),
}

/// What a sort step sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeneratorSort {
    /// The rating.
    Rating,
    /// The play count.
    Plays,
    /// The skip count.
    Skips,
    /// When the track last played.
    LastPlayed,
    /// When the track came into the library.
    Added,
    /// The release year.
    Year,
    /// The title.
    Title,
    /// The first track artist.
    Artist,
    /// The album title.
    Album,
    /// The length.
    Length,
    /// A random order.
    Random,
}

impl GeneratorSort {
    const NAMES: [(Self, &'static str); 11] = [
        (Self::Rating, "rating"),
        (Self::Plays, "plays"),
        (Self::Skips, "skips"),
        (Self::LastPlayed, "last-played"),
        (Self::Added, "added"),
        (Self::Year, "year"),
        (Self::Title, "title"),
        (Self::Artist, "artist"),
        (Self::Album, "album"),
        (Self::Length, "length"),
        (Self::Random, "random"),
    ];

    /// The name in the text form.
    pub fn name(self) -> &'static str {
        Self::NAMES
            .iter()
            .find(|(sort, _)| *sort == self)
            .map_or("random", |(_, name)| name)
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(name))
            .map(|(sort, _)| *sort)
    }

    /// The SQL value sorted by, over `tracks t`, `albums a` and `stats s`.
    fn sql(self) -> &'static str {
        match self {
            Self::Rating => "s.rating",
            Self::Plays => "coalesce(s.play_count, 0)",
            Self::Skips => "coalesce(s.skip_count, 0)",
            Self::LastPlayed => "s.last_played",
            Self::Added => "t.added_at",
            Self::Year => "coalesce(t.year, a.year)",
            Self::Title => "t.title_sort",
            Self::Artist => {
                "(SELECT coalesce(p.sort_name, p.name) FROM track_people tp
                  JOIN people p ON p.id = tp.person_id
                  WHERE tp.track_id = t.id AND tp.role = 0 ORDER BY tp.position LIMIT 1)"
            }
            Self::Album => "a.title_sort",
            Self::Length => "t.duration_ms",
            Self::Random => "0",
        }
    }
}

/// One step after the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratorStep {
    /// Keeps the tracks the query finds.
    Where(String),
    /// Drops the tracks of the playlist, smart playlist or generator of that name.
    Exclude(String),
    /// Sorts, keeping the order before among equals; tracks without the value go
    /// last.
    Sort {
        /// What to sort by.
        by: GeneratorSort,
        /// Largest first.
        descending: bool,
    },
    /// Keeps at most this many tracks of each first track artist.
    PerArtist(usize),
    /// Keeps at most this many tracks of each album.
    PerAlbum(usize),
    /// Keeps this many tracks from the start.
    Limit(usize),
    /// Keeps tracks from the start while they add up to at most this many minutes.
    LimitMinutes(u64),
}

/// The steps of a generator: a source and what follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorSteps {
    /// Where the tracks come from.
    pub source: GeneratorSource,
    /// What happens to them, in order.
    pub steps: Vec<GeneratorStep>,
}

impl Default for GeneratorSteps {
    fn default() -> Self {
        Self {
            source: GeneratorSource::Library,
            steps: Vec::new(),
        }
    }
}

/// A line of the text form that cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {problem}")]
pub struct StepError {
    /// The line, from 1.
    pub line: usize,
    /// What is wrong with it.
    pub problem: String,
}

/// Reads a quoted name: `"…"`, with `\"` and `\\` inside.
fn quoted(text: &str) -> Option<String> {
    let inner = text.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut name = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => name.push(chars.next()?),
            '"' => return None,
            c => name.push(c),
        }
    }
    Some(name)
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The query of a step, checked.
fn checked_query(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("the query is missing".into());
    }
    parse_query(text).map_err(|e| e.to_string())?;
    Ok(text.to_owned())
}

fn count(text: &str) -> Result<usize, String> {
    text.trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("{:?} is not a number above 0", text.trim()))
}

impl FromStr for GeneratorSteps {
    type Err = StepError;

    fn from_str(text: &str) -> Result<Self, StepError> {
        let mut steps = Self::default();
        let mut first = true;
        for (number, raw) in text.lines().enumerate() {
            let line = number + 1;
            let content = raw.trim();
            if content.is_empty() || content.starts_with('#') {
                continue;
            }
            let fail = |problem: String| StepError { line, problem };
            let (word, rest) = content
                .split_once(char::is_whitespace)
                .map_or((content, ""), |(w, r)| (w, r.trim()));
            match word.to_ascii_lowercase().as_str() {
                "from" => {
                    if !first {
                        return Err(fail("the source must be the first step".into()));
                    }
                    let (kind, what) = rest
                        .split_once(char::is_whitespace)
                        .map_or((rest, ""), |(k, w)| (k, w.trim()));
                    steps.source = match kind.to_ascii_lowercase().as_str() {
                        "library" if what.is_empty() => GeneratorSource::Library,
                        "playlist" => GeneratorSource::Playlist(quoted(what).ok_or_else(|| {
                            fail("the playlist's name must be in double quotes".into())
                        })?),
                        "query" => GeneratorSource::Query(checked_query(what).map_err(fail)?),
                        _ => {
                            return Err(fail(
                                "the source is \"library\", \"playlist\" or \"query\"".into(),
                            ));
                        }
                    };
                }
                "where" => steps
                    .steps
                    .push(GeneratorStep::Where(checked_query(rest).map_err(fail)?)),
                "exclude" => {
                    let what = rest
                        .strip_prefix("playlist")
                        .or_else(|| rest.strip_prefix("generator"))
                        .unwrap_or(rest);
                    steps
                        .steps
                        .push(GeneratorStep::Exclude(quoted(what).ok_or_else(|| {
                            fail("exclude takes a playlist's name in double quotes".into())
                        })?));
                }
                "sort" => {
                    let mut words = rest.split_whitespace();
                    let words_by = words.next().filter(|w| !w.eq_ignore_ascii_case("by"));
                    let by_word = match words_by {
                        Some(w) => Some(w),
                        None => words.next(),
                    };
                    let by = by_word
                        .and_then(GeneratorSort::from_name)
                        .ok_or_else(|| fail(format!("sort by what? ({})", sort_names())))?;
                    let descending = match words.next().map(str::to_ascii_lowercase).as_deref() {
                        None | Some("asc") => false,
                        Some("desc") => true,
                        Some(other) => {
                            return Err(fail(format!("{other:?}: \"asc\" or \"desc\"")));
                        }
                    };
                    steps.steps.push(GeneratorStep::Sort { by, descending });
                }
                "per-artist" => steps
                    .steps
                    .push(GeneratorStep::PerArtist(count(rest).map_err(fail)?)),
                "per-album" => steps
                    .steps
                    .push(GeneratorStep::PerAlbum(count(rest).map_err(fail)?)),
                "limit" => {
                    let mut words = rest.split_whitespace();
                    let amount = count(words.next().unwrap_or("")).map_err(fail)?;
                    match words.next().map(str::to_ascii_lowercase).as_deref() {
                        None | Some("tracks" | "track") => {
                            steps.steps.push(GeneratorStep::Limit(amount));
                        }
                        Some("minutes" | "minute" | "min") => {
                            steps.steps.push(GeneratorStep::LimitMinutes(amount as u64))
                        }
                        Some(other) => {
                            return Err(fail(format!("{other:?}: \"tracks\" or \"minutes\"")));
                        }
                    }
                }
                other => {
                    return Err(fail(format!(
                        "{other:?} is not a step (from, where, exclude, sort, per-artist, \
                         per-album, limit)"
                    )));
                }
            }
            first = false;
        }
        Ok(steps)
    }
}

fn sort_names() -> String {
    GeneratorSort::NAMES
        .iter()
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for GeneratorSteps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            GeneratorSource::Library => writeln!(f, "from library")?,
            GeneratorSource::Playlist(name) => writeln!(f, "from playlist {}", quote(name))?,
            GeneratorSource::Query(query) => writeln!(f, "from query {query}")?,
        }
        for step in &self.steps {
            match step {
                GeneratorStep::Where(query) => writeln!(f, "where {query}")?,
                GeneratorStep::Exclude(name) => writeln!(f, "exclude playlist {}", quote(name))?,
                GeneratorStep::Sort { by, descending } => writeln!(
                    f,
                    "sort {}{}",
                    by.name(),
                    if *descending { " desc" } else { "" }
                )?,
                GeneratorStep::PerArtist(n) => writeln!(f, "per-artist {n}")?,
                GeneratorStep::PerAlbum(n) => writeln!(f, "per-album {n}")?,
                GeneratorStep::Limit(n) => writeln!(f, "limit {n}")?,
                GeneratorStep::LimitMinutes(n) => writeln!(f, "limit {n} minutes")?,
            }
        }
        Ok(())
    }
}

/// A generator playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generator {
    /// The generator.
    pub id: GeneratorId,
    /// Its name.
    pub name: String,
    /// Its steps.
    pub steps: GeneratorSteps,
    /// Whether it is made again each time it is opened.
    pub on_open: bool,
    /// Its last result, in order; `None` before the first run.
    pub tracks: Option<Vec<TrackId>>,
    /// When the last result was made.
    pub generated: Option<SystemTime>,
    /// When it was created.
    pub created: SystemTime,
    /// When its name, steps or `on_open` last changed.
    pub modified: SystemTime,
    /// The playlist folder it is in, `None` at the top.
    pub parent: Option<PlaylistId>,
}

/// What running steps without storing them gave: see
/// [`Library::try_generator`].
#[derive(Debug)]
pub struct GeneratorTrial {
    /// The tracks the steps left, or, when a step failed, those the steps before
    /// it left.
    pub tracks: Vec<TrackId>,
    /// How many tracks were left after the source (first) and after each step
    /// that ran.
    pub counts: Vec<usize>,
    /// The step that could not run, counted with the source as 0, and why.
    pub failure: Option<(usize, GeneratorError)>,
}

/// Why a generator could not be stored or run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GeneratorError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A query of a step cannot be read.
    #[error(transparent)]
    Syntax(#[from] SyntaxError),
    /// A query of a step cannot run.
    #[error(transparent)]
    Query(#[from] QueryError),
    /// The steps cannot be read.
    #[error(transparent)]
    Steps(#[from] StepError),
    /// A step names a playlist that is not there.
    #[error("there is no playlist or generator named {0:?}")]
    UnknownPlaylist(String),
    /// There is no such generator.
    #[error("there is no generator {0}")]
    Unknown(String),
    /// Another generator has the name.
    #[error("there is already a generator named {0:?}")]
    NameTaken(String),
    /// A generator needs a name.
    #[error("a generator needs a name")]
    EmptyName,
    /// A generator is its own source, directly or through others.
    #[error("the generator {0:?} uses itself")]
    Cycle(String),
}

const COLUMNS: &str =
    "id, name, steps, on_open, tracks, generated_at, created_at, modified_at, parent_id";

fn read_generator(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Generator, String)> {
    let tracks: Option<String> = row.get(4)?;
    let tracks = tracks.map(|json| {
        serde_json::from_str::<Vec<u64>>(&json)
            .unwrap_or_default()
            .into_iter()
            .map(TrackId::new)
            .collect()
    });
    let steps: String = row.get(2)?;
    Ok((
        Generator {
            id: GeneratorId(row.get(0)?),
            name: row.get(1)?,
            steps: GeneratorSteps::default(),
            on_open: row.get::<_, i64>(3)? != 0,
            tracks,
            generated: row.get::<_, Option<i64>>(5)?.map(from_unix_seconds),
            created: from_unix_seconds(row.get(6)?),
            modified: from_unix_seconds(row.get(7)?),
            parent: row.get::<_, Option<i64>>(8)?.map(PlaylistId),
        },
        steps,
    ))
}

fn with_steps((mut generator, text): (Generator, String)) -> Generator {
    // Steps were checked when stored; a query language that later reads them
    // differently leaves an empty generator rather than none.
    generator.steps = text.parse().unwrap_or_default();
    generator
}

fn json_list(ids: &[TrackId]) -> String {
    let list: Vec<String> = ids.iter().map(|id| id.get().to_string()).collect();
    format!("[{}]", list.join(","))
}

/// SplitMix64, for the random sort.
fn shuffle(ids: &mut [TrackId], seed: u64) {
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..ids.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        ids.swap(i, j);
    }
}

/// The tracks of the static or smart playlist named `name`, if there is one.
fn playlist_named(library: &Library, name: &str) -> Result<Option<Vec<TrackId>>, GeneratorError> {
    let name = name.to_owned();
    let found = library.database().read(move |c| {
        c.prepare_cached(
            "SELECT id, kind FROM playlists WHERE name = ?1 COLLATE NOCASE AND kind IN (1, 2)
             ORDER BY id LIMIT 1",
        )?
        .query_row([&name], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .optional()
    })?;
    let Some((id, kind)) = found else {
        return Ok(None);
    };
    if kind == 2 {
        let tracks = library
            .all_smart_playlist_tracks(PlaylistId(id))
            .map_err(|e| GeneratorError::Unknown(e.to_string()))?;
        return Ok(Some(tracks.into_iter().map(|t| t.id).collect()));
    }
    let tracks = library.database().read(move |c| {
        c.prepare_cached(
            "SELECT (SELECT min(t.id) FROM tracks t WHERE t.identity_id = i.identity_id)
             FROM playlist_items i WHERE i.playlist_id = ?1 ORDER BY i.position",
        )?
        .query_map([id], |row| row.get::<_, Option<i64>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
    })?;
    Ok(Some(
        tracks
            .into_iter()
            .flatten()
            .map(track_id_from_sql)
            .collect(),
    ))
}

impl Library {
    /// Every generator playlist, by name.
    pub fn generators(&self) -> Result<Vec<Generator>, DbError> {
        let rows = self.database().read(|c| {
            c.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM generators ORDER BY name COLLATE NOCASE"
            ))?
            .query_map([], read_generator)?
            .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(rows.into_iter().map(with_steps).collect())
    }

    /// The generator `id`, if it is there.
    pub fn generator(&self, id: GeneratorId) -> Result<Option<Generator>, DbError> {
        let row = self.database().read(move |c| {
            c.prepare_cached(&format!("SELECT {COLUMNS} FROM generators WHERE id = ?1"))?
                .query_row([id.0], read_generator)
                .optional()
        })?;
        Ok(row.map(with_steps))
    }

    /// The generator named `name` (without regard to case), or numbered `name`.
    pub fn find_generator(&self, name: &str) -> Result<Option<Generator>, DbError> {
        let wanted = name.to_owned();
        let row = self.database().read(move |c| {
            c.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM generators WHERE name = ?1 COLLATE NOCASE
                 OR CAST(id AS TEXT) = ?1 ORDER BY name = ?1 COLLATE NOCASE DESC LIMIT 1"
            ))?
            .query_row([&wanted], read_generator)
            .optional()
        })?;
        Ok(row.map(with_steps))
    }

    /// Stores a new generator playlist; it has no result until it first runs.
    pub fn create_generator(
        &self,
        name: &str,
        steps: &GeneratorSteps,
        on_open: bool,
    ) -> Result<GeneratorId, GeneratorError> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(GeneratorError::EmptyName);
        }
        if self
            .find_generator(&name)?
            .is_some_and(|g| g.name.eq_ignore_ascii_case(&name))
        {
            return Err(GeneratorError::NameTaken(name));
        }
        let text = steps.to_string();
        let now = to_unix_seconds(SystemTime::now());
        let id = self.database().writer()?.write(move |c| {
            c.execute(
                "INSERT INTO generators (name, steps, on_open, created_at, modified_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                (&name, &text, i64::from(on_open), now),
            )?;
            Ok(c.last_insert_rowid())
        })?;
        Ok(GeneratorId(id))
    }

    /// Changes the name, the steps or `on_open` of the generator `id`: those given.
    /// New steps drop the last result.
    pub fn update_generator(
        &self,
        id: GeneratorId,
        name: Option<&str>,
        steps: Option<&GeneratorSteps>,
        on_open: Option<bool>,
    ) -> Result<(), GeneratorError> {
        let Some(current) = self.generator(id)? else {
            return Err(GeneratorError::Unknown(id.to_string()));
        };
        let name = match name.map(str::trim) {
            Some("") => return Err(GeneratorError::EmptyName),
            Some(name) => {
                if let Some(other) = self.find_generator(name)?
                    && other.id != id
                    && other.name.eq_ignore_ascii_case(name)
                {
                    return Err(GeneratorError::NameTaken(name.to_owned()));
                }
                name.to_owned()
            }
            None => current.name,
        };
        let new_steps = steps.map(ToString::to_string);
        let on_open = on_open.unwrap_or(current.on_open);
        let now = to_unix_seconds(SystemTime::now());
        self.database().writer()?.write(move |c| {
            c.execute(
                "UPDATE generators SET name = ?2, on_open = ?3, modified_at = ?4 WHERE id = ?1",
                (id.0, &name, i64::from(on_open), now),
            )?;
            if let Some(text) = new_steps {
                c.execute(
                    "UPDATE generators SET steps = ?2, tracks = NULL, generated_at = NULL
                     WHERE id = ?1",
                    (id.0, &text),
                )?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Removes the generator `id`; whether it was there.
    pub fn remove_generator(&self, id: GeneratorId) -> Result<bool, DbError> {
        self.database()
            .writer()?
            .write(move |c| Ok(c.execute("DELETE FROM generators WHERE id = ?1", [id.0])? > 0))
    }

    /// Puts the generator `id` in the playlist folder `parent` (at the top for
    /// `None`).
    pub fn move_generator(
        &self,
        id: GeneratorId,
        parent: Option<PlaylistId>,
    ) -> Result<(), GeneratorError> {
        if let Some(folder) = parent {
            let is_folder = self.database().read(move |c| {
                c.prepare_cached("SELECT 1 FROM playlists WHERE id = ?1 AND kind = 0")?
                    .query_row([folder.0], |_| Ok(()))
                    .optional()
            })?;
            if is_folder.is_none() {
                return Err(GeneratorError::Unknown(format!("folder {}", folder.0)));
            }
        }
        let changed = self.database().writer()?.write(move |c| {
            c.execute(
                "UPDATE generators SET parent_id = ?2 WHERE id = ?1",
                (id.0, parent.map(|p| p.0)),
            )
        })?;
        if changed == 0 {
            return Err(GeneratorError::Unknown(id.to_string()));
        }
        Ok(())
    }

    /// Runs `steps` without storing anything; `seed` orders `sort random`.
    pub fn preview_generator(
        &self,
        steps: &GeneratorSteps,
        seed: u64,
    ) -> Result<Vec<TrackId>, GeneratorError> {
        self.run_steps(steps, seed, &mut Vec::new())
    }

    /// Runs `steps` without storing anything, counting the tracks left after
    /// each step; a step that cannot run ends the run and is reported with the
    /// counts so far. `name` is the generator being edited, if any, so that
    /// steps that use it are refused.
    pub fn try_generator(
        &self,
        steps: &GeneratorSteps,
        seed: u64,
        name: Option<&str>,
    ) -> GeneratorTrial {
        let mut using: Vec<String> = name.map(str::to_owned).into_iter().collect();
        let mut counts = Vec::new();
        let mut tracks = Vec::new();
        let failure = self
            .run_counted(steps, seed, &mut using, &mut counts, &mut tracks)
            .err()
            .map(|error| (counts.len(), error));
        GeneratorTrial {
            tracks,
            counts,
            failure,
        }
    }

    /// Runs the generator `id` again and stores its result.
    pub fn regenerate(&self, id: GeneratorId, seed: u64) -> Result<Vec<TrackId>, GeneratorError> {
        let Some(generator) = self.generator(id)? else {
            return Err(GeneratorError::Unknown(id.to_string()));
        };
        let tracks = self.run_steps(&generator.steps, seed, &mut vec![generator.name])?;
        let json = json_list(&tracks);
        let now = to_unix_seconds(SystemTime::now());
        self.database().writer()?.write(move |c| {
            c.execute(
                "UPDATE generators SET tracks = ?2, generated_at = ?3 WHERE id = ?1",
                (id.0, &json, now),
            )
        })?;
        Ok(tracks)
    }

    /// The tracks of the generator `id` as opening it shows them: made again when
    /// it is made on open or has never run, its last result otherwise (less the
    /// tracks that have left the library).
    pub fn open_generator(
        &self,
        id: GeneratorId,
        seed: u64,
    ) -> Result<Vec<TrackId>, GeneratorError> {
        let Some(generator) = self.generator(id)? else {
            return Err(GeneratorError::Unknown(id.to_string()));
        };
        match generator.tracks {
            Some(tracks) if !generator.on_open => self.still_there(&tracks),
            _ => self.regenerate(id, seed),
        }
    }

    /// `tracks` less those no longer in the library, in order.
    fn still_there(&self, tracks: &[TrackId]) -> Result<Vec<TrackId>, GeneratorError> {
        let json = json_list(tracks);
        let present: HashSet<i64> = self.database().read(move |c| {
            c.prepare_cached("SELECT id FROM tracks WHERE id IN (SELECT value FROM json_each(?1))")?
                .query_map([&json], |row| row.get(0))?
                .collect()
        })?;
        Ok(tracks
            .iter()
            .copied()
            .filter(|t| present.contains(&sql_track_id(*t)))
            .collect())
    }

    /// The tracks of the playlist or generator named `name`; `using` holds the
    /// generators being run, to refuse a loop.
    fn named_tracks(
        &self,
        name: &str,
        seed: u64,
        using: &mut Vec<String>,
    ) -> Result<Vec<TrackId>, GeneratorError> {
        if let Some(tracks) = playlist_named(self, name)? {
            return Ok(tracks);
        }
        let Some(generator) = self
            .find_generator(name)?
            .filter(|g| g.name.eq_ignore_ascii_case(name))
        else {
            return Err(GeneratorError::UnknownPlaylist(name.to_owned()));
        };
        if using
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&generator.name))
        {
            return Err(GeneratorError::Cycle(generator.name));
        }
        if let Some(tracks) = &generator.tracks
            && !generator.on_open
        {
            return self.still_there(tracks);
        }
        using.push(generator.name.clone());
        let tracks = self.run_steps(&generator.steps, seed, using);
        using.pop();
        tracks
    }

    fn query_ids(&self, text: &str) -> Result<Vec<TrackId>, GeneratorError> {
        let request = QueryRequest::new(parse_query(text)?);
        Ok(self.query_track_ids(&request)?)
    }

    fn run_steps(
        &self,
        steps: &GeneratorSteps,
        seed: u64,
        using: &mut Vec<String>,
    ) -> Result<Vec<TrackId>, GeneratorError> {
        let mut tracks = Vec::new();
        self.run_counted(steps, seed, using, &mut Vec::new(), &mut tracks)?;
        Ok(tracks)
    }

    /// Runs `steps` into `tracks`, pushing the number left after the source and
    /// after each step to `counts`.
    fn run_counted(
        &self,
        steps: &GeneratorSteps,
        seed: u64,
        using: &mut Vec<String>,
        counts: &mut Vec<usize>,
        tracks: &mut Vec<TrackId>,
    ) -> Result<(), GeneratorError> {
        *tracks = match &steps.source {
            GeneratorSource::Library => self.database().read(|c| {
                let visible = visibility(c, false)?;
                c.prepare(&format!(
                    "SELECT t.id FROM tracks t JOIN files f ON f.id = t.file_id
                     LEFT JOIN albums a ON a.id = t.album_id
                     WHERE t.kind = 0 AND {visible}
                     ORDER BY a.title_sort, t.album_id, t.disc_no, t.track_no, t.id"
                ))?
                .query_map([], |row| row.get(0).map(track_id_from_sql))?
                .collect::<rusqlite::Result<Vec<_>>>()
            })?,
            GeneratorSource::Playlist(name) => self.named_tracks(name, seed, using)?,
            GeneratorSource::Query(query) => self.query_ids(query)?,
        };
        // Each track once, in its first place.
        let mut seen = HashSet::new();
        tracks.retain(|t| seen.insert(*t));
        counts.push(tracks.len());
        for (index, step) in steps.steps.iter().enumerate() {
            match step {
                GeneratorStep::Where(query) => {
                    let found: HashSet<TrackId> = self.query_ids(query)?.into_iter().collect();
                    tracks.retain(|t| found.contains(t));
                }
                GeneratorStep::Exclude(name) => {
                    let gone: HashSet<TrackId> =
                        self.named_tracks(name, seed, using)?.into_iter().collect();
                    tracks.retain(|t| !gone.contains(t));
                }
                GeneratorStep::Sort {
                    by: GeneratorSort::Random,
                    ..
                } => shuffle(tracks, seed.wrapping_add(index as u64)),
                GeneratorStep::Sort { by, descending } => {
                    let json = json_list(tracks);
                    let order = if *descending { "DESC" } else { "ASC" };
                    let value = by.sql();
                    *tracks = self
                        .database()
                        .read(move |c| sorted(c, &json, value, order))?;
                }
                GeneratorStep::PerArtist(cap) => {
                    let firsts = self.database().read({
                        let json = json_list(tracks);
                        move |c| first_artists(c, &json)
                    })?;
                    capped(tracks, *cap, |t| firsts.get(t).copied());
                }
                GeneratorStep::PerAlbum(cap) => {
                    let albums = self.database().read({
                        let json = json_list(tracks);
                        move |c| albums_of(c, &json)
                    })?;
                    capped(tracks, *cap, |t| albums.get(t).copied());
                }
                GeneratorStep::Limit(n) => tracks.truncate(*n),
                GeneratorStep::LimitMinutes(minutes) => {
                    let lengths = self.database().read({
                        let json = json_list(tracks);
                        move |c| lengths_of(c, &json)
                    })?;
                    let budget = minutes.saturating_mul(60_000);
                    let mut total: u64 = 0;
                    let keep = tracks
                        .iter()
                        .take_while(|t| {
                            total = total.saturating_add(lengths.get(t).copied().unwrap_or(0));
                            total <= budget
                        })
                        .count();
                    tracks.truncate(keep);
                }
            }
            counts.push(tracks.len());
        }
        Ok(())
    }
}

/// Keeps at most `cap` tracks of each key; tracks without one are all kept.
fn capped(tracks: &mut Vec<TrackId>, cap: usize, key: impl Fn(&TrackId) -> Option<i64>) {
    let mut counts: HashMap<i64, usize> = HashMap::new();
    tracks.retain(|t| match key(t) {
        Some(k) => {
            let n = counts.entry(k).or_default();
            *n += 1;
            *n <= cap
        }
        None => true,
    });
}

fn sorted(c: &Connection, json: &str, value: &str, order: &str) -> rusqlite::Result<Vec<TrackId>> {
    c.prepare(&format!(
        "SELECT t.id FROM json_each(?1) k
         JOIN tracks t ON t.id = k.value
         LEFT JOIN albums a ON a.id = t.album_id
         LEFT JOIN stats s ON s.identity_id = t.identity_id
         ORDER BY ({value}) IS NULL, ({value}) {order}, k.key"
    ))?
    .query_map([json], |row| row.get(0).map(track_id_from_sql))?
    .collect()
}

fn first_artists(c: &Connection, json: &str) -> rusqlite::Result<HashMap<TrackId, i64>> {
    c.prepare(
        "SELECT k.value, (SELECT tp.person_id FROM track_people tp
                          WHERE tp.track_id = k.value AND tp.role = 0
                          ORDER BY tp.position LIMIT 1)
         FROM json_each(?1) k",
    )?
    .query_map([json], |row| {
        Ok((
            track_id_from_sql(row.get(0)?),
            row.get::<_, Option<i64>>(1)?,
        ))
    })?
    .filter_map(|r| r.map(|(t, p)| p.map(|p| (t, p))).transpose())
    .collect()
}

fn albums_of(c: &Connection, json: &str) -> rusqlite::Result<HashMap<TrackId, i64>> {
    c.prepare(
        "SELECT t.id, t.album_id FROM json_each(?1) k JOIN tracks t ON t.id = k.value
         WHERE t.album_id IS NOT NULL",
    )?
    .query_map([json], |row| {
        Ok((track_id_from_sql(row.get(0)?), row.get(1)?))
    })?
    .collect()
}

fn lengths_of(c: &Connection, json: &str) -> rusqlite::Result<HashMap<TrackId, u64>> {
    c.prepare(
        "SELECT t.id, coalesce(t.duration_ms, 0) FROM json_each(?1) k
         JOIN tracks t ON t.id = k.value",
    )?
    .query_map([json], |row| {
        Ok((
            track_id_from_sql(row.get(0)?),
            u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
        ))
    })?
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_form_reads_back() {
        let text = "# Forgotten favourites\n\
                    from playlist \"Best \\\"of\\\"\"\n\
                    where rating>=4\n\
                    exclude playlist \"Last week\"\n\
                    sort by last-played\n\
                    sort plays desc\n\
                    per-artist 2\n\
                    per-album 1\n\
                    limit 90 minutes\n\
                    limit 20\n";
        let steps: GeneratorSteps = text.parse().unwrap();
        assert_eq!(
            steps.source,
            GeneratorSource::Playlist("Best \"of\"".into())
        );
        assert_eq!(steps.steps.len(), 8);
        assert_eq!(
            steps.steps[3],
            GeneratorStep::Sort {
                by: GeneratorSort::Plays,
                descending: true
            }
        );
        let printed = steps.to_string();
        assert_eq!(printed.parse::<GeneratorSteps>().unwrap(), steps);
        assert!(printed.starts_with("from playlist \"Best \\\"of\\\"\"\n"));
    }

    #[test]
    fn problems_name_their_line() {
        for (text, line) in [
            ("where rating>=4\nfrom library", 2),
            ("from somewhere", 1),
            ("\n\nsort by colour", 3),
            ("limit 0", 1),
            ("limit 5 hours", 1),
            ("where (unclosed", 1),
            ("exclude playlist Unquoted", 1),
            ("shuffle", 1),
        ] {
            let error = text.parse::<GeneratorSteps>().unwrap_err();
            assert_eq!(error.line, line, "{text}: {error}");
        }
        assert_eq!(
            "".parse::<GeneratorSteps>().unwrap(),
            GeneratorSteps::default()
        );
    }

    #[test]
    fn caps_keep_the_first_of_each() {
        let mut tracks: Vec<TrackId> = (1..=6).map(TrackId::new).collect();
        capped(&mut tracks, 1, |t| {
            (t.get() != 6).then_some((t.get() % 2) as i64)
        });
        assert_eq!(
            tracks,
            vec![TrackId::new(1), TrackId::new(2), TrackId::new(6)]
        );
    }
}
