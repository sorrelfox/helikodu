// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The compiled query language against a generated library: every field with every
//! operator it takes, compared with what the same comparison gives on the values the
//! library was filled with; plain text against plain search; boolean logic, missing
//! values, sorting and limits; values that try to break out of their parameters; saved
//! searches and quick filters.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_library::raw::rusqlite;
use helikodu_library::{ArtistRole, SavedSearchId};
use helikodu_library::{
    AudioProperties, CatalogTags, Credit, FileStamp, Library, Page, QueryError, QueryRequest,
    ReplayGain, RootId, SearchRequest, TrackKind, TrackRecord, TrackTags, fold, parse_query,
};
use helikodu_model::library::TrackId;
use helikodu_model::query::{
    Age, AgeUnit, CalendarDate, Comparison, Decimal, Expr, Field, FieldKind, Flag, Limit,
    LimitUnit, MediaKind, Operator, Presence, Query, QuickFilter, SortKey, Value,
};

const TRACKS: usize = 240;
const DAY: i64 = 86_400;

const WORDS: &[&str] = &[
    "Blue Hour",
    "Harbour",
    "Quiet Corner",
    "Straße",
    "Café del Mar",
    "ёлка",
    "月亮代表",
    "Intro",
    "Outro",
    "Glass Harbour",
    "Ferry Song",
    "Ελάτε",
];
const PEOPLE: &[&str] = &[
    "Anton Verde",
    "Sola",
    "Björk",
    "Juno Park",
    "Мари Каск",
    "久石譲",
];
const ALBUMS: &[&str] = &["Blue Hour Sessions", "Safe Harbour", "Small Hours", "Ёлка"];
const GENRES: &[&str] = &["Jazz", "Soul", "Ambient", "Jazz Fusion", "Électro"];
const CODECS: &[&str] = &["flac", "mp3", "opus", "alac", "pcm_s24le", "FLAC", "aac"];
const FOLDERS: &[&str] = &["Jazz/Blue", "Rock", "Ёлка", "Misc (2020)"];
const MOODS: &[&str] = &["calm", "Dark", "upbeat"];

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    fn some<T>(&mut self, percent: u64, value: impl FnOnce(&mut Self) -> T) -> Option<T> {
        if self.chance(percent) {
            Some(value(self))
        } else {
            None
        }
    }
}

/// What a track was filled with, in the units of the database.
#[derive(Debug, Default)]
struct Model {
    id: u64,
    text: HashMap<Field, Vec<String>>,
    scalar: HashMap<Field, Option<f64>>,
    kind: Option<MediaKind>,
    has_lyrics: bool,
    compilation: bool,
    loved: bool,
    /// Whether the track has a row of statistics.
    stats: bool,
}

impl Model {
    fn text(&self, field: &Field) -> &[String] {
        self.text.get(field).map_or(&[], Vec::as_slice)
    }

    fn scalar(&self, field: &Field) -> Option<f64> {
        self.scalar.get(field).copied().flatten()
    }
}

struct Fixture {
    library: Library,
    tracks: Vec<Model>,
    now: i64,
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Fills a library with generated tracks and returns it with what it holds.
fn fixture() -> Fixture {
    let (library, root) = support::library();
    let mut rng = Lcg(7);
    let now = unix_now();
    let mut records = Vec::new();
    let mut models = Vec::new();
    for i in 0..TRACKS {
        let (record, model) = generate(&mut rng, root, i, now);
        records.push(record);
        models.push(model);
    }
    let written = library.upsert_tracks(records).unwrap();
    for (model, write) in models.iter_mut().zip(&written) {
        model.id = write.track.get();
    }
    // What the scan cannot write: statistics, times, sizes and other tags.
    let rows: Vec<_> = models.iter().map(stats_row).collect();
    library
        .database()
        .writer()
        .unwrap()
        .write(move |c| {
            for row in &rows {
                write_row(c, row)?;
            }
            Ok(())
        })
        .unwrap();
    Fixture {
        library,
        tracks: models,
        now,
    }
}

fn generate(rng: &mut Lcg, root: RootId, i: usize, now: i64) -> (TrackRecord, Model) {
    let folder = rng.pick(FOLDERS);
    let name = format!("{:03} {}.flac", i, rng.pick(WORDS));
    let path = format!("/music/{folder}/{name}");
    let size = rng.next() % 90_000_000 + 1_000_000;
    let mut record = TrackRecord::new(
        root,
        &path,
        FileStamp {
            size,
            mtime_ns: Some(1),
            inode: None,
            quick_hash: format!("hash {i}").into_bytes(),
        },
    );
    let mut m = Model::default();
    let title = rng.some(92, |rng| format!("{} {}", rng.pick(WORDS), i % 7));
    let artists: Vec<String> = (0..rng.below(3))
        .map(|_| rng.pick(PEOPLE).to_owned())
        .collect();
    let mut artists_unique = artists.clone();
    artists_unique.dedup();
    let album_index = rng.below(ALBUMS.len() + 1);
    let album = ALBUMS.get(album_index).map(|a| (*a).to_owned());
    // Every track of an album has the same album artist, year and compilation flag,
    // so that the album holds together.
    let album_artist = album.as_ref().map(|_| PEOPLE[album_index].to_owned());
    let year = album
        .as_ref()
        .map(|_| 1960 + album_index as i32 * 13)
        .or_else(|| rng.some(50, |rng| 1990 + rng.below(30) as i32));
    m.compilation = album_index == 0;
    let genres: Vec<String> = match rng.below(4) {
        0 => vec![],
        1 => vec![rng.pick(GENRES).to_owned()],
        _ => {
            let a = rng.pick(GENRES);
            let b = rng.pick(GENRES);
            if a == b {
                vec![a.to_owned()]
            } else {
                vec![a.to_owned(), b.to_owned()]
            }
        }
    };
    let composers: Vec<String> = rng
        .some(30, |rng| rng.pick(PEOPLE).to_owned())
        .into_iter()
        .collect();
    let performer = rng.some(20, |rng| rng.pick(PEOPLE).to_owned());
    let conductor = rng.some(15, |rng| rng.pick(PEOPLE).to_owned());
    let lyricist = rng.some(15, |rng| rng.pick(PEOPLE).to_owned());
    record.tags = TrackTags {
        title: title.clone(),
        artists: artists_unique.clone(),
        album_artists: album_artist.iter().cloned().collect(),
        composers: composers.clone(),
        album: album.clone(),
        genres: genres.clone(),
        track_no: rng.some(80, |rng| 1 + rng.below(20) as u32),
        disc_no: rng.some(40, |rng| 1 + rng.below(3) as u32),
        year,
        ..TrackTags::default()
    };
    let credit = |role, name: &Option<String>| {
        name.clone().map(|name| Credit {
            role,
            name,
            instrument: None,
        })
    };
    let work = rng.some(25, |rng| format!("Suite No. {}", rng.below(4)));
    let movement = rng.some(25, |rng| {
        rng.pick(&["Allegro", "Adagio", "Presto"]).to_owned()
    });
    let disc_subtitle = rng.some(15, |rng| rng.pick(&["Live", "Rarities"]).to_owned());
    let release_type = rng.some(40, |rng| rng.pick(&["album", "live", "ep"]).to_owned());
    record.catalog = CatalogTags {
        credits: [
            credit(ArtistRole::Performer, &performer),
            credit(ArtistRole::Conductor, &conductor),
            credit(ArtistRole::Lyricist, &lyricist),
        ]
        .into_iter()
        .flatten()
        .collect(),
        compilation: m.compilation.then_some(true),
        release_types: release_type.iter().cloned().collect(),
        original_year: rng.some(30, |rng| 1950 + rng.below(40) as i32),
        work: work.clone(),
        movement: movement.clone(),
        movement_no: rng.some(25, |rng| 1 + rng.below(4) as u32),
        disc_subtitle: disc_subtitle.clone(),
        has_lyrics: rng.chance(30),
        ..CatalogTags::default()
    };
    let codec = rng.some(95, |rng| rng.pick(CODECS).to_owned());
    let container = rng.some(80, |rng| {
        rng.pick(&["flac", "ogg", "mp4", "wav"]).to_owned()
    });
    // Lengths with and without a part of a second.
    let duration_ms = rng.some(95, |rng| {
        (20 + rng.below(600)) as u64 * 1000 + [0, 0, 500][rng.below(3)]
    });
    let sample_rate = rng.some(90, |rng| {
        [44_100, 48_000, 88_200, 96_000, 192_000][rng.below(5)]
    });
    let bit_depth = rng.some(70, |rng| [16_u16, 24, 32][rng.below(3)]);
    let bitrate = rng.some(80, |rng| {
        [128_000_u32, 320_000, 900_000, 1_411_000][rng.below(4)]
    });
    let channels = rng.some(90, |rng| [1_u16, 2, 6][rng.below(3)]);
    record.audio = AudioProperties {
        duration: duration_ms.map(Duration::from_millis),
        sample_rate,
        channels,
        bit_depth,
        bitrate,
        codec: codec.clone(),
        container: container.clone(),
    };
    let kind = match rng.below(10) {
        0 => (TrackKind::Audiobook, MediaKind::Audiobook),
        1 => (TrackKind::Podcast, MediaKind::Podcast),
        2 => (TrackKind::AudioFromVideo, MediaKind::Video),
        _ => (TrackKind::Music, MediaKind::Music),
    };
    record.kind = kind.0;
    m.kind = Some(kind.1);
    let track_gain = rng.some(60, |rng| -(rng.below(24) as f64) / 2.0);
    let album_gain = rng.some(40, |rng| -(rng.below(24) as f64) / 4.0);
    record.replay_gain = ReplayGain {
        track_gain_db: track_gain,
        album_gain_db: album_gain,
        ..ReplayGain::default()
    };
    m.has_lyrics = record.catalog.has_lyrics;

    let mut text = |field: Field, values: Vec<String>| {
        m.text.insert(field, values);
    };
    text(Field::Title, title.into_iter().collect());
    text(Field::Artist, artists_unique);
    text(Field::AlbumArtist, album_artist.into_iter().collect());
    text(Field::Album, album.into_iter().collect());
    text(Field::Composer, composers);
    text(Field::Performer, performer.into_iter().collect());
    text(Field::Conductor, conductor.into_iter().collect());
    text(Field::Lyricist, lyricist.into_iter().collect());
    text(Field::Genre, genres);
    text(Field::Work, work.into_iter().collect());
    text(Field::Movement, movement.into_iter().collect());
    text(Field::DiscSubtitle, disc_subtitle.into_iter().collect());
    text(Field::ReleaseType, release_type.into_iter().collect());
    text(Field::Codec, codec.into_iter().collect());
    text(Field::Container, container.into_iter().collect());
    text(Field::Path, vec![path]);
    text(Field::Folder, vec![format!("/music/{folder}")]);
    text(Field::Filename, vec![name]);
    let mood = rng.some(40, |rng| rng.pick(MOODS).to_owned());
    text(Field::Tag("MOOD".into()), mood.into_iter().collect());

    let tags = &record.tags;
    let catalog = &record.catalog;
    let num = |n: Option<u32>| n.map(f64::from);
    let scalars = [
        (Field::Year, tags.year.map(f64::from)),
        (Field::OriginalYear, catalog.original_year.map(f64::from)),
        (Field::Track, num(tags.track_no)),
        (Field::Disc, num(tags.disc_no)),
        (Field::MovementNumber, num(catalog.movement_no)),
        (Field::SampleRate, num(sample_rate)),
        (Field::BitDepth, bit_depth.map(f64::from)),
        (Field::Channels, channels.map(f64::from)),
        (Field::Bitrate, num(bitrate)),
        (Field::TrackGain, track_gain),
        (Field::AlbumGain, album_gain),
        (Field::Duration, duration_ms.map(|ms| ms as f64)),
        (Field::Size, Some(size as f64)),
    ];
    m.scalar.extend(scalars);
    // Statistics: some tracks have none at all.
    let stats = rng.chance(70);
    let time = |rng: &mut Lcg, percent| {
        // Whole days and five hours ago, so that no age in whole days, weeks or the
        // hours the tests use falls on one; or a fixed day in 2023.
        rng.some(percent, |rng| match rng.below(4) {
            0 => 1_686_830_400 + rng.below(200) as i64 * DAY, // from 2023-06-15 12:00 UTC
            _ => now - rng.below(800) as i64 * DAY - 5 * 3600,
        })
    };
    let f = |n: Option<i64>| n.map(|n| n as f64);
    m.scalar.insert(Field::Added, f(time(rng, 100)));
    m.scalar.insert(Field::Modified, f(time(rng, 50)));
    m.stats = stats;
    let (plays, skips) = if stats {
        (rng.below(4) as f64, rng.below(3) as f64)
    } else {
        (0.0, 0.0)
    };
    m.scalar.insert(Field::Plays, Some(plays));
    m.scalar.insert(Field::Skips, Some(skips));
    let kept = |value: Option<i64>| if stats { f(value) } else { None };
    let rating = rng.some(60, |rng| rng.below(11) as i64);
    m.scalar.insert(Field::Rating, kept(rating));
    let last = time(rng, 60);
    m.scalar.insert(Field::LastPlayed, kept(last));
    let first = time(rng, 60);
    m.scalar.insert(Field::FirstPlayed, kept(first));
    let skipped = time(rng, 40);
    m.scalar.insert(Field::LastSkipped, kept(skipped));
    let resume = rng.some(30, |rng| rng.below(3_000) as i64 * 1000);
    m.scalar.insert(Field::Resume, kept(resume));
    m.loved = stats && rng.chance(20);
    (record, m)
}

/// The values of one track that are written with SQL.
struct Row {
    id: i64,
    stats: bool,
    values: HashMap<&'static str, Option<i64>>,
    loved: bool,
    mood: Option<String>,
}

fn stats_row(m: &Model) -> Row {
    let int = |field: Field| m.scalar(&field).map(|v| v as i64);
    let values = HashMap::from([
        ("added_at", int(Field::Added)),
        ("modified_at", int(Field::Modified)),
        ("play_count", int(Field::Plays)),
        ("skip_count", int(Field::Skips)),
        ("rating", int(Field::Rating)),
        ("last_played", int(Field::LastPlayed)),
        ("first_played", int(Field::FirstPlayed)),
        ("last_skipped", int(Field::LastSkipped)),
        ("resume_ms", int(Field::Resume)),
    ]);
    Row {
        id: m.id as i64,
        stats: m.stats,
        values,
        loved: m.loved,
        mood: m.text(&Field::Tag("MOOD".into())).first().cloned(),
    }
}

fn write_row(c: &rusqlite::Connection, row: &Row) -> rusqlite::Result<()> {
    let v = |key: &str| row.values[key];
    c.execute(
        "UPDATE tracks SET added_at = ?2, modified_at = ?3 WHERE id = ?1",
        rusqlite::params![row.id, v("added_at"), v("modified_at")],
    )?;
    c.execute(
        "DELETE FROM stats WHERE identity_id = (SELECT identity_id FROM tracks WHERE id = ?1)",
        [row.id],
    )?;
    if row.stats {
        c.execute(
            "INSERT INTO stats (identity_id, play_count, skip_count, rating, last_played,
                 first_played, last_skipped, resume_ms, loved)
             SELECT identity_id, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9 FROM tracks WHERE id = ?1",
            rusqlite::params![
                row.id,
                v("play_count"),
                v("skip_count"),
                v("rating"),
                v("last_played"),
                v("first_played"),
                v("last_skipped"),
                v("resume_ms"),
                row.loved,
            ],
        )?;
    }
    if let Some(mood) = &row.mood {
        c.execute(
            "INSERT INTO track_tags (track_id, key, value) VALUES (?1, 'MOOD', ?2)",
            rusqlite::params![row.id, mood],
        )?;
    }
    Ok(())
}

impl Fixture {
    /// The ids `query` selects, as a set.
    fn found(&self, query: &Query) -> BTreeSet<u64> {
        self.ids(query).into_iter().collect()
    }

    /// The ids `query` selects, in order.
    fn ids(&self, query: &Query) -> Vec<u64> {
        let request = QueryRequest::new(query.clone());
        let ids = self.library.query_track_ids(&request).unwrap();
        let page = self
            .library
            .query_tracks(&request.page(Page::first(1000)))
            .unwrap();
        let paged: Vec<u64> = page.items.iter().map(|t| t.id.get()).collect();
        let ids: Vec<u64> = ids.iter().map(|t| t.get()).collect();
        assert_eq!(paged, ids, "{query:?}");
        assert_eq!(page.total, ids.len());
        ids
    }

    /// The ids of the tracks for which `holds` is true.
    fn expected(&self, holds: impl Fn(&Model) -> bool) -> BTreeSet<u64> {
        self.tracks
            .iter()
            .filter(|t| holds(t))
            .map(|t| t.id)
            .collect()
    }

    /// The local time zone's offset at `at`, as SQLite sees it.
    fn local_period(&self, date: CalendarDate) -> (i64, i64) {
        let (start, modifier) = match (date.month_number(), date.day_number()) {
            (Some(m), Some(d)) => (format!("{:04}-{m:02}-{d:02}", date.year_number()), "+1 day"),
            (Some(m), None) => (format!("{:04}-{m:02}-01", date.year_number()), "+1 month"),
            _ => (format!("{:04}-01-01", date.year_number()), "+1 year"),
        };
        self.library
            .database()
            .read(|c| {
                c.query_row(
                    "SELECT unixepoch(?1, 'utc'), unixepoch(?1, ?2, 'utc')",
                    rusqlite::params![start, modifier],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap()
    }

    /// Whether comparison `c` holds for track `t`.
    fn holds(&self, t: &Model, c: &Comparison) -> bool {
        if c.op == Operator::NotEquals {
            let equal = Comparison {
                op: Operator::Equals,
                ..c.clone()
            };
            return !self.holds(t, &equal);
        }
        match (c.field.kind(), &c.value) {
            (FieldKind::Text, Value::Text(q)) => t.text(&c.field).iter().any(|v| match c.op {
                Operator::Contains => fold(v).contains(&fold(q)),
                Operator::Matches => regex::Regex::new(q).unwrap().is_match(v),
                _ => fold(v) == fold(q),
            }),
            (FieldKind::Kind, Value::Kind(kind)) => t.kind == Some(*kind),
            (FieldKind::Date, value) => {
                let Some(at) = t.scalar(&c.field) else {
                    return false;
                };
                let at = at as i64;
                let (start, end) = match value {
                    Value::Date(date) => self.local_period(*date),
                    Value::Age(age) => {
                        let bound = self.now - age_seconds(*age);
                        return match c.op {
                            Operator::Less => at > bound,
                            Operator::Greater => at < bound,
                            Operator::GreaterOrEqual => at <= bound,
                            _ => at >= bound,
                        };
                    }
                    Value::Range(low, high) => {
                        let span = |v: &Value| match v {
                            Value::Date(d) => self.local_period(*d),
                            Value::Age(a) => {
                                let b = self.now - age_seconds(*a);
                                (b, b + 1)
                            }
                            _ => unreachable!(),
                        };
                        let (a, b) = (span(low), span(high));
                        return at >= a.0.min(b.0) && at < a.1.max(b.1);
                    }
                    _ => unreachable!(),
                };
                match c.op {
                    Operator::Less => at < start,
                    Operator::LessOrEqual => at < end,
                    Operator::Greater => at >= end,
                    Operator::GreaterOrEqual => at >= start,
                    _ => at >= start && at < end,
                }
            }
            (_, value) => {
                let Some(have) = t.scalar(&c.field) else {
                    return false;
                };
                if let Value::Range(low, high) = value {
                    let (mut a, mut b) =
                        (column_value(&c.field, low), column_value(&c.field, high));
                    if b < a {
                        std::mem::swap(&mut a, &mut b);
                    }
                    let whole_second =
                        |v: &Value| matches!(v, Value::Duration(ms) if ms % 1000 == 0);
                    let upper_value = if column_value(&c.field, high) >= column_value(&c.field, low)
                    {
                        &**high
                    } else {
                        &**low
                    };
                    if whole_second(upper_value) {
                        b += 999.0;
                    }
                    return have >= a && have <= b;
                }
                let want = column_value(&c.field, value);
                if let (Value::Duration(ms), Operator::Contains | Operator::Equals) = (value, c.op)
                    && ms % 1000 == 0
                {
                    return (have / 1000.0).floor() == want / 1000.0;
                }
                match c.op {
                    Operator::Less => have < want,
                    Operator::LessOrEqual => have <= want,
                    Operator::Greater => have > want,
                    Operator::GreaterOrEqual => have >= want,
                    _ => have == want,
                }
            }
        }
    }

    fn has(&self, t: &Model, presence: &Presence) -> bool {
        match presence {
            Presence::Lyrics => t.has_lyrics,
            Presence::Cover => false,
            Presence::Field(Field::Kind | Field::Path | Field::Folder | Field::Filename) => true,
            Presence::Field(field @ (Field::Plays | Field::Skips)) => {
                t.scalar(field).unwrap_or(0.0) > 0.0
            }
            Presence::Field(field) if field.kind() == FieldKind::Text => {
                t.text(field).iter().any(|v| !v.is_empty())
            }
            Presence::Field(field) => t.scalar(field).is_some(),
            _ => unreachable!(),
        }
    }

    fn is(&self, t: &Model, flag: Flag) -> bool {
        let codec = t.text(&Field::Codec).first().map(|c| c.to_lowercase());
        let lossless = codec.is_some_and(|c| {
            ["flac", "alac", "ape", "wavpack", "tta", "tak"].contains(&c.as_str())
                || c.starts_with("pcm")
                || c.starts_with("dsd")
        });
        match flag {
            Flag::Lossless => lossless,
            Flag::HiRes => {
                lossless
                    && (t.scalar(&Field::SampleRate).unwrap_or(0.0) > 48_000.0
                        || t.scalar(&Field::BitDepth).unwrap_or(0.0) > 16.0)
            }
            Flag::Compilation => t.compilation,
            Flag::Loved => t.loved,
            _ => unreachable!(),
        }
    }

    /// Whether `expr` holds for `t`; plain text is left to [`Fixture::text_ids`].
    fn eval(&self, t: &Model, expr: &Expr) -> bool {
        match expr {
            Expr::All(children) => children.iter().all(|c| self.eval(t, c)),
            Expr::Any(children) => children.iter().any(|c| self.eval(t, c)),
            Expr::Not(inner) => !self.eval(t, inner),
            Expr::Compare(c) => self.holds(t, c),
            Expr::Has(presence) => self.has(t, presence),
            Expr::Is(flag) => self.is(t, *flag),
            _ => unreachable!("{expr:?}"),
        }
    }

    /// Checks `expr` against the oracle; whether it selects some tracks but not all.
    fn check(&self, expr: Expr) -> bool {
        let expected = self.expected(|t| self.eval(t, &expr));
        let found = self.found(&Query::new(expr.clone()));
        assert_eq!(
            found,
            expected,
            "{}",
            helikodu_library::print_query(&Query::new(expr))
        );
        !expected.is_empty() && expected.len() < TRACKS
    }
}

fn age_seconds(age: Age) -> i64 {
    let n = i64::from(age.amount);
    match age.unit {
        AgeUnit::Hours => n * 3600,
        AgeUnit::Days => n * DAY,
        AgeUnit::Weeks => n * 7 * DAY,
        _ => unreachable!("the oracle knows hours, days and weeks"),
    }
}

/// A query value in the unit of the field's column.
fn column_value(field: &Field, value: &Value) -> f64 {
    match (field, value) {
        (Field::Bitrate, Value::Number(n)) => n.as_f64() * 1000.0,
        (Field::Rating, Value::Number(n)) => n.as_f64() * 2.0,
        (_, Value::Number(n)) => n.as_f64(),
        (_, Value::Duration(ms)) => *ms as f64,
        (_, Value::Size(bytes)) => *bytes as f64,
        _ => unreachable!("{value:?}"),
    }
}

/// Values to compare `field` with: some the library holds, and some it does not.
fn samples(fixture: &Fixture, field: &Field) -> Vec<Value> {
    let tracks = &fixture.tracks;
    match field.kind() {
        FieldKind::Text => {
            let mut values: Vec<String> = tracks
                .iter()
                .flat_map(|t| t.text(field).iter().cloned())
                .step_by(37)
                .take(3)
                .collect();
            // Parts, other cases and accents, short and missing ones.
            let mut more = Vec::new();
            for value in &values {
                let chars: Vec<char> = value.chars().collect();
                let mid = chars.len() / 3;
                more.push(
                    chars[mid..(mid + 4).min(chars.len())]
                        .iter()
                        .collect::<String>(),
                );
                more.push(value.to_uppercase());
                more.push(chars.iter().take(2).collect());
            }
            values.extend(more);
            values.extend(["bjork".to_owned(), "ELKA".to_owned(), "zzz".to_owned()]);
            values.extend(["e".to_owned(), String::new()]);
            values.into_iter().map(Value::Text).collect()
        }
        FieldKind::Kind => MediaKind::ALL.into_iter().map(Value::Kind).collect(),
        FieldKind::Date => vec![
            Value::Age(Age {
                amount: 30,
                unit: AgeUnit::Days,
            }),
            Value::Age(Age {
                amount: 12,
                unit: AgeUnit::Hours,
            }),
            Value::Age(Age {
                amount: 50,
                unit: AgeUnit::Hours,
            }),
            Value::Age(Age {
                amount: 52,
                unit: AgeUnit::Weeks,
            }),
            Value::Date(CalendarDate::year(2023).unwrap()),
            Value::Date(CalendarDate::month(2023, 9).unwrap()),
            Value::Date(CalendarDate::day(2023, 7, 3).unwrap()),
            Value::range(
                Value::Date(CalendarDate::month(2023, 8).unwrap()),
                Value::Date(CalendarDate::year(2023).unwrap()),
            ),
            Value::range(
                Value::Age(Age {
                    amount: 400,
                    unit: AgeUnit::Days,
                }),
                Value::Age(Age {
                    amount: 10,
                    unit: AgeUnit::Days,
                }),
            ),
        ],
        kind => {
            let mut held: Vec<f64> = tracks.iter().filter_map(|t| t.scalar(field)).collect();
            held.sort_by(f64::total_cmp);
            held.dedup();
            let pick = [held.first(), held.get(held.len() / 2), held.last()];
            let mut values: Vec<Value> = pick
                .into_iter()
                .flatten()
                .map(|v| query_value(field, kind, *v))
                .collect();
            if values.len() >= 2 {
                values.push(Value::range(values[1].clone(), values[0].clone()));
            }
            if kind == FieldKind::Duration {
                // A length in whole seconds of a track with half a second more.
                if let Some(half) = held.iter().find(|v| **v as u64 % 1000 == 500) {
                    values.push(Value::Duration(*half as u64 - 500));
                    values.push(Value::Duration(*half as u64));
                }
            }
            values
        }
    }
}

/// The query value of a column value.
fn query_value(field: &Field, kind: FieldKind, v: f64) -> Value {
    match (kind, field) {
        (FieldKind::Duration, _) => Value::Duration(v as u64),
        (FieldKind::Size, _) => Value::Size(v as u64),
        (_, Field::Bitrate) => Value::Number(Decimal::from_thousandths(v as i64)),
        (_, Field::Rating) => Value::Number(Decimal::from_thousandths(v as i64 * 500)),
        _ => Value::Number(Decimal::from_thousandths((v * 1000.0).round() as i64)),
    }
}

fn operators(kind: FieldKind) -> Vec<Operator> {
    match kind {
        FieldKind::Text => vec![
            Operator::Contains,
            Operator::Equals,
            Operator::NotEquals,
            Operator::Matches,
        ],
        FieldKind::Kind => vec![Operator::Contains, Operator::Equals, Operator::NotEquals],
        _ => Operator::ALL
            .into_iter()
            .filter(|op| *op != Operator::Matches)
            .collect(),
    }
}

#[test]
fn every_field_with_every_operator() {
    let fixture = fixture();
    let mut checked = 0;
    let mut selective = 0;
    let fields: Vec<Field> = Field::named().chain([Field::Tag("MOOD".into())]).collect();
    for field in &fields {
        for value in samples(&fixture, field) {
            for op in operators(field.kind()) {
                let value = match (&value, op) {
                    (Value::Range(..), op) if op != Operator::Contains => continue,
                    (Value::Text(t), Operator::Matches) => {
                        Value::Text(format!("(?i){}", regex::escape(t)))
                    }
                    _ => value.clone(),
                };
                let expr = Expr::compare(field.clone(), op, value);
                if let Expr::Compare(c) = &expr {
                    assert_eq!(c.check(), Ok(()), "{c:?}");
                }
                selective += usize::from(fixture.check(expr));
                checked += 1;
            }
        }
        fixture.check(Expr::Has(Presence::Field(field.clone())));
        fixture.check(Expr::Has(Presence::Field(field.clone())).negate());
    }
    assert!(checked > 600, "{checked}");
    // Most comparisons tell tracks apart, so that a wrong condition shows.
    assert!(selective * 2 > checked, "{selective} of {checked}");
    for presence in [Presence::Lyrics, Presence::Cover] {
        fixture.check(Expr::Has(presence));
    }
    for flag in Flag::ALL {
        fixture.check(Expr::Is(flag));
        fixture.check(Expr::Is(flag).negate());
    }
}

#[test]
fn regular_expressions_match_the_text_as_written() {
    let fixture = fixture();
    for (field, pattern) in [
        (Field::Title, "^(Intro|Outro) "),
        (Field::Title, "Harbour [0-3]$"),
        (Field::Artist, "^[A-Z]"),
        (Field::Genre, "Jazz$"),
        (Field::Path, r"/\d{3} "),
        (Field::Codec, "^FLAC$"),
        (Field::Tag("MOOD".into()), "^[a-z]+$"),
    ] {
        fixture.check(Expr::compare(
            field,
            Operator::Matches,
            Value::Text(pattern.into()),
        ));
    }
}

#[test]
fn boolean_logic_and_missing_values() {
    let fixture = fixture();
    let year = |op, y| Expr::compare(Field::Year, op, Value::number(y));
    let genre = |g: &str| Expr::compare(Field::Genre, Operator::Contains, Value::Text(g.into()));
    let rated = Expr::Has(Presence::Field(Field::Rating));
    let exprs = [
        year(Operator::Less, 1990).negate(),
        year(Operator::Equals, 1960).or(genre("soul")),
        genre("jazz").and(genre("fusion").negate()),
        rated
            .clone()
            .negate()
            .and(year(Operator::GreaterOrEqual, 2000).negate()),
        Expr::Any(vec![
            Expr::Is(Flag::Loved),
            Expr::compare(Field::Rating, Operator::GreaterOrEqual, Value::number(4)),
            Expr::All(vec![genre("ambient"), Expr::Has(Presence::Lyrics).negate()]),
        ]),
        Expr::compare(Field::Rating, Operator::Less, Value::number(3)).negate(),
        Expr::compare(
            Field::Title,
            Operator::Contains,
            Value::Text("harbour".into()),
        )
        .negate(),
        Expr::compare(
            Field::Album,
            Operator::Equals,
            Value::Text("small hours".into()),
        )
        .negate(),
        Expr::compare(
            Field::Artist,
            Operator::NotEquals,
            Value::Text("sola".into()),
        ),
        Expr::Any(vec![]),
        Expr::Any(vec![]).negate(),
    ];
    for expr in exprs {
        fixture.check(expr);
    }
    // Without a value, a comparison fails and its negation holds.
    let unrated = fixture.found(&parse_query("-has:rating").unwrap());
    let not_high = fixture.found(&parse_query("-rating>=0").unwrap());
    assert_eq!(unrated, not_high);
    assert!(!unrated.is_empty());
    // Never played includes the tracks without statistics.
    let never = fixture.found(&parse_query("plays=0").unwrap());
    assert_eq!(
        never,
        fixture.expected(|t| t.scalar(&Field::Plays) == Some(0.0))
    );
}

#[test]
fn plain_text_finds_what_plain_search_finds() {
    let fixture = fixture();
    for term in [
        "harbour",
        "HARBOUR",
        "har",
        "ha",
        "h",
        "jazz",
        "björk",
        "bjork",
        "ёлка",
        "елка",
        "月亮",
        "月",
        "cafe",
        "strasse",
        "blue",
        "music",
        "Ελάτε",
        "zzz",
    ] {
        let search = fixture
            .library
            .search(&SearchRequest::new(term).page(0, 1000))
            .unwrap();
        let expected: BTreeSet<u64> = search.hits.iter().map(|h| h.track.get()).collect();
        let found = fixture.found(&Query::new(Expr::text(term)));
        assert_eq!(found, expected, "{term}");
        // Combined with a field, plain text narrows it.
        let flac = fixture.found(&parse_query(&format!("\"{term}\" codec:flac")).unwrap());
        assert!(flac.is_subset(&found), "{term}");
    }
    // Several words must all match.
    let both = fixture.found(&parse_query("blue hour").unwrap());
    let blue = fixture.found(&parse_query("blue").unwrap());
    let hour = fixture.found(&parse_query("hour").unwrap());
    assert_eq!(both, &blue & &hour);
    assert!(!both.is_empty());
}

#[test]
fn sorting_puts_missing_values_last_and_limits_cut_the_sorted_list() {
    let fixture = fixture();
    let by_id: HashMap<u64, &Model> = fixture.tracks.iter().map(|t| (t.id, t)).collect();
    for (field, descending) in [
        (Field::Year, false),
        (Field::Year, true),
        (Field::Rating, true),
        (Field::Duration, false),
        (Field::Added, true),
        (Field::Plays, true),
        (Field::Size, false),
    ] {
        let query = Query::default().sorted_by(SortKey {
            by: helikodu_model::query::SortBy::Field(field.clone()),
            descending,
        });
        let ids = fixture.ids(&query);
        assert_eq!(ids.len(), TRACKS);
        let keys: Vec<Option<f64>> = ids.iter().map(|id| by_id[id].scalar(&field)).collect();
        let first_missing = keys.iter().position(Option::is_none).unwrap_or(keys.len());
        assert!(
            keys[first_missing..].iter().all(Option::is_none),
            "{field:?}"
        );
        let present: Vec<f64> = keys[..first_missing].iter().flatten().copied().collect();
        let ordered = present.windows(2).all(|w| {
            if descending {
                w[0] >= w[1]
            } else {
                w[0] <= w[1]
            }
        });
        assert!(ordered, "{field:?} {descending}");
    }
    // Text sorts in the locale order, missing titles last.
    let titles = fixture.ids(&parse_query("sort:title").unwrap());
    let untitled = titles
        .iter()
        .position(|id| by_id[id].text(&Field::Title).is_empty())
        .unwrap();
    assert!(
        titles[untitled..]
            .iter()
            .all(|id| by_id[id].text(&Field::Title).is_empty())
    );
    // A shuffled order is a permutation, the same for the same seed.
    let random = parse_query("sort:random").unwrap();
    let shuffled = |seed| {
        let mut request = QueryRequest::new(random.clone());
        request.seed = seed;
        fixture.library.query_track_ids(&request).unwrap()
    };
    assert_eq!(shuffled(1), shuffled(1));
    assert_ne!(shuffled(1), shuffled(2));
    let mut sorted: Vec<TrackId> = shuffled(3);
    sorted.sort();
    assert_eq!(sorted.len(), TRACKS);

    // Limits keep the start of the sorted list.
    let all = fixture.ids(&parse_query("sort:duration desc").unwrap());
    let ten = fixture.ids(&parse_query("sort:duration desc limit:10").unwrap());
    assert_eq!(ten, all[..10]);
    let hour = fixture.ids(&parse_query("sort:duration desc limit:60min").unwrap());
    let lengths: Vec<f64> = all
        .iter()
        .map(|id| by_id[id].scalar(&Field::Duration).unwrap_or(0.0))
        .collect();
    let mut total = 0.0;
    let fit = lengths
        .iter()
        .take_while(|ms| {
            total += **ms;
            total <= 3_600_000.0
        })
        .count();
    assert_eq!(hour, all[..fit]);
    let sizes = fixture.ids(&parse_query("is:lossless sort:size limit:300MB").unwrap());
    let size_sum: f64 = sizes
        .iter()
        .map(|id| by_id[id].scalar(&Field::Size).unwrap())
        .sum();
    assert!(size_sum <= 300_000_000.0 && !sizes.is_empty());
    // A page of a limited query, and its total.
    let request = QueryRequest::new(parse_query("sort:duration desc limit:10").unwrap())
        .page(Page::new(8, 5));
    let page = fixture.library.query_tracks(&request).unwrap();
    assert_eq!(page.total, 10);
    let paged: Vec<u64> = page.items.iter().map(|t| t.id.get()).collect();
    assert_eq!(paged, all[8..10]);
    assert_eq!(
        fixture
            .library
            .query_count(&QueryRequest::new(parse_query("limit:0").unwrap()))
            .unwrap(),
        0
    );
}

#[test]
fn values_never_reach_the_sql_text() {
    let fixture = fixture();
    let hostile = [
        "'; DROP TABLE tracks; --",
        "\" OR 1=1 --",
        "x') OR ('1'='1",
        "%' OR title LIKE '%",
        "?1",
        "title : \"a\" OR artists : b",
        "NEAR(a b)",
        "\\",
    ];
    for value in hostile {
        for field in [
            Field::Title,
            Field::Artist,
            Field::Genre,
            Field::Album,
            Field::Path,
            Field::Tag("MOOD".into()),
        ] {
            for op in [Operator::Contains, Operator::Equals] {
                fixture.check(Expr::compare(field.clone(), op, Value::Text(value.into())));
            }
        }
        // Plain text and regular expressions too.
        fixture.found(&Query::new(Expr::text(value)));
        let pattern = Value::Text(regex::escape(value));
        fixture.check(Expr::compare(Field::Title, Operator::Matches, pattern));
    }
    let count: i64 = fixture
        .library
        .database()
        .read(|c| c.query_row("SELECT count(*) FROM tracks", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(count, TRACKS as i64);
}

#[test]
fn ill_fitting_trees_are_refused() {
    let fixture = fixture();
    let bad = Query::new(Expr::compare(
        Field::Title,
        Operator::Less,
        Value::number(3),
    ));
    assert!(matches!(
        fixture.library.query_tracks(&QueryRequest::new(bad)),
        Err(QueryError::Invalid(_))
    ));
}

#[test]
fn quick_filters_select_what_they_say() {
    let fixture = fixture();
    for filter in QuickFilter::ALL {
        let query = filter.query();
        let found = fixture.found(&Query {
            sort: Vec::new(),
            ..query.clone()
        });
        let holds = |t: &Model| match filter {
            QuickFilter::RecentlyAdded => t
                .scalar(&Field::Added)
                .is_some_and(|a| a as i64 > fixture.now - 30 * DAY),
            QuickFilter::RecentlyPlayed => t
                .scalar(&Field::LastPlayed)
                .is_some_and(|a| a as i64 > fixture.now - 14 * DAY),
            QuickFilter::Forgotten => t
                .scalar(&Field::LastPlayed)
                .is_some_and(|a| (a as i64) < fixture.now - 365 * DAY),
            other => fixture.eval(t, query_filter(&other.query())),
        };
        if filter == QuickFilter::Forgotten {
            // A calendar year back: compare away from the edges only.
            let near_edge = |t: &Model| {
                t.scalar(&Field::LastPlayed).is_some_and(|a| {
                    let age = fixture.now - a as i64;
                    (364 * DAY..367 * DAY).contains(&age)
                })
            };
            let expected = fixture.expected(|t| !near_edge(t) && holds(t));
            let found: BTreeSet<u64> = found
                .into_iter()
                .filter(|id| !near_edge(fixture.tracks.iter().find(|t| t.id == *id).unwrap()))
                .collect();
            assert_eq!(found, expected, "{filter:?}");
            continue;
        }
        assert_eq!(found, fixture.expected(holds), "{filter:?}");
        // And the sorted query runs.
        fixture.ids(&query);
    }
}

fn query_filter(query: &Query) -> &Expr {
    query.filter.as_ref().unwrap()
}

#[test]
fn saved_searches_keep_name_query_and_order() {
    let fixture = fixture();
    let library = &fixture.library;
    assert!(library.saved_searches().unwrap().is_empty());
    let month = parse_query("added<30d").unwrap();
    let never = parse_query("plays=0").unwrap();
    let jazz = parse_query("genre:jazz -genre:fusion").unwrap();
    let a = library.save_search("Added this month", &month).unwrap();
    let b = library.save_search("  Never played ", &never).unwrap();
    let c = library.save_search("Jazz without fusion", &jazz).unwrap();
    let list = library.saved_searches().unwrap();
    let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Added this month", "Never played", "Jazz without fusion"]
    );
    assert_eq!(list[2].text, "genre:jazz -genre:fusion");
    assert_eq!(list[2].query, jazz);
    assert_eq!(list[1].position, 1);
    assert_eq!(list[0].error, None);

    library.move_saved_search(c, 0).unwrap();
    let order = |l: &Library| -> Vec<SavedSearchId> {
        l.saved_searches().unwrap().iter().map(|s| s.id).collect()
    };
    assert_eq!(order(library), [c, a, b]);
    library.move_saved_search(c, 99).unwrap();
    assert_eq!(order(library), [a, b, c]);

    library.rename_saved_search(b, "Unplayed").unwrap();
    library
        .set_saved_search_query(b, &QuickFilter::Unplayed.query())
        .unwrap();
    let saved = library.saved_search(b).unwrap().unwrap();
    assert_eq!(saved.name, "Unplayed");
    assert_eq!(saved.query, QuickFilter::Unplayed.query());
    assert!(saved.modified >= saved.created);

    library.delete_saved_search(a).unwrap();
    assert_eq!(order(library), [b, c]);
    let positions: Vec<usize> = library
        .saved_searches()
        .unwrap()
        .iter()
        .map(|s| s.position)
        .collect();
    assert_eq!(positions, [0, 1]);

    assert!(matches!(
        library.save_search(" ", &month),
        Err(QueryError::EmptyName)
    ));
    assert!(matches!(
        library.delete_saved_search(a),
        Err(QueryError::UnknownSavedSearch(id)) if id == a
    ));
    assert!(library.saved_search(a).unwrap().is_none());

    // Text a later version wrote is searched for as plain text.
    library
        .database()
        .writer()
        .unwrap()
        .write(move |c| {
            c.execute(
                "UPDATE saved_searches SET query = 'colour:blue' WHERE id = ?1",
                [c_id(b)],
            )
        })
        .unwrap();
    let saved = library.saved_search(b).unwrap().unwrap();
    assert!(saved.error.is_some());
    assert_eq!(saved.query.filter, Some(Expr::text("colour:blue")));
    // The count the side column shows.
    let count = library.query_count(&QueryRequest::new(jazz)).unwrap();
    assert_eq!(
        count,
        fixture
            .found(&parse_query("genre:jazz -genre:fusion").unwrap())
            .len()
    );
}

fn c_id(id: SavedSearchId) -> i64 {
    id.get()
}

#[test]
fn limit_units_parse_as_documented() {
    for (text, limit) in [
        ("limit:50", Limit::tracks(50)),
        (
            "limit:2h",
            Limit {
                amount: 2,
                unit: LimitUnit::Hours,
            },
        ),
        (
            "limit:700MB",
            Limit {
                amount: 700,
                unit: LimitUnit::Megabytes,
            },
        ),
    ] {
        assert_eq!(parse_query(text).unwrap().limit, Some(limit));
    }
}
