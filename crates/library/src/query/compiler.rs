// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiling a query tree into parameterised SQL over the library schema.
//!
//! The result is a condition, an order and a limit over the tracks `t`, their files
//! `f` and folders `d`, their albums `a` and their statistics `s` (see [`FROM`]). Every
//! value of the query is a bound parameter; only names this module chooses (columns,
//! roles, codecs) are written into the SQL text.
//!
//! - Plain text matches as plain search does: the trigram and word indexes, or a scan
//!   of the folded columns for one or two characters.
//! - Text fields compare folded text, so case and accents do not matter. `title:`
//!   uses the trigram index from three characters; people, genres and albums are
//!   matched in their own small tables and then looked up by index.
//! - `~` uses the `regexp()` function on the text as written.
//! - Every condition is true or false, never NULL: a track without a value fails a
//!   comparison and passes its negation.

mod calendar;

use helikodu_model::query::{
    Comparison, Expr, Field, FieldKind, Flag, Limit, LimitUnit, MediaKind, Operator, Presence,
    Query, SortBy, SortKey, TermProblem, Value,
};
use rusqlite::types::Value as SqlValue;

use crate::catalog::{BY_ALBUM_ARTIST, LOSSLESS};
use crate::search::{fold, term_condition, terms};

/// The tables a compiled query refers to.
pub(crate) const FROM: &str = "FROM tracks t
    JOIN files f ON f.id = t.file_id
    JOIN dirs d ON d.id = f.dir_id
    LEFT JOIN albums a ON a.id = t.album_id
    LEFT JOIN stats s ON s.identity_id = t.identity_id";

/// What relative dates and a shuffled order are relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueryContext {
    /// Now, in seconds since the Unix epoch.
    pub(crate) now: i64,
    /// The local time zone's offset from UTC in seconds, for calendar dates.
    pub(crate) utc_offset: i64,
    /// The seed of `sort:random`.
    pub(crate) seed: u64,
}

/// A compiled query.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Compiled {
    /// The condition on a track.
    pub(crate) condition: String,
    /// The order, ending with the track id so that it is total.
    pub(crate) order: String,
    /// The limit, if any.
    pub(crate) limit: Option<CompiledLimit>,
    /// The values of `?1`, `?2`, … in `condition` and `order`.
    pub(crate) params: Vec<SqlValue>,
}

/// A limit: a number of tracks, or a running total of `measure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompiledLimit {
    /// What is added up per track; `None` counts tracks.
    pub(crate) measure: Option<&'static str>,
    /// The most the running total may reach.
    pub(crate) amount: i64,
}

/// Compiles `query`. Fails if a comparison does not fit together.
pub(crate) fn compile(query: &Query, context: &QueryContext) -> Result<Compiled, TermProblem> {
    if let Some((_, problem)) = query.problem() {
        return Err(problem);
    }
    let mut sql = Sql {
        params: Vec::new(),
        context: *context,
    };
    let condition = match &query.filter {
        Some(filter) => sql.expr(filter),
        None => "1".to_owned(),
    };
    let mut order: Vec<String> = query.sort.iter().map(|key| sql.sort_key(key)).collect();
    // A shuffled order hardly has ties: the id settles them, without the cost of the
    // album order.
    let shuffled = query.sort.iter().any(|key| key.by == SortBy::Random);
    order.push(if shuffled { "t.id" } else { BY_ALBUM_ARTIST }.to_owned());
    Ok(Compiled {
        condition,
        order: order.join(", "),
        limit: query.limit.map(limit),
        params: sql.params,
    })
}

fn limit(limit: Limit) -> CompiledLimit {
    let amount = i64::from(limit.amount);
    let (measure, factor) = match limit.unit {
        LimitUnit::Tracks => (None, 1),
        LimitUnit::Minutes => (Some("coalesce(t.duration_ms, 0)"), 60_000),
        LimitUnit::Hours => (Some("coalesce(t.duration_ms, 0)"), 3_600_000),
        LimitUnit::Megabytes => (Some("coalesce(f.size, 0)"), 1_000_000),
        LimitUnit::Gigabytes => (Some("coalesce(f.size, 0)"), 1_000_000_000),
    };
    CompiledLimit {
        measure,
        amount: amount.saturating_mul(factor),
    }
}

/// Where a text field's values are.
enum TextSource {
    /// A column of the joined tables: as written, folded, and its column in the
    /// trigram index.
    Column {
        raw: &'static str,
        folded: &'static str,
        trigram: Option<&'static str>,
    },
    /// The people in a role (`track_people.role`).
    People(i64),
    /// The album artists, as the library groups albums.
    AlbumPeople,
    /// The genres.
    Genres,
    /// The album title.
    Album,
    /// Other tags, by key.
    Tag(String),
}

fn text_source(field: &Field) -> Option<TextSource> {
    let column = |raw, folded| TextSource::Column {
        raw,
        folded,
        trigram: None,
    };
    Some(match field {
        Field::Title => TextSource::Column {
            raw: "t.title",
            folded: "t.title_fold",
            trigram: Some("title"),
        },
        Field::Artist => TextSource::People(0),
        Field::AlbumArtist => TextSource::AlbumPeople,
        Field::Composer => TextSource::People(2),
        Field::Performer => TextSource::People(3),
        Field::Conductor => TextSource::People(4),
        Field::Lyricist => TextSource::People(5),
        Field::Genre => TextSource::Genres,
        Field::Album => TextSource::Album,
        Field::Work => column("t.work", "hk_fold(t.work)"),
        Field::Movement => column("t.movement", "hk_fold(t.movement)"),
        Field::DiscSubtitle => column("t.disc_subtitle", "hk_fold(t.disc_subtitle)"),
        Field::ReleaseType => column("t.release_type", "hk_fold(t.release_type)"),
        Field::Codec => column("t.codec", "lower(t.codec)"),
        Field::Container => column("t.container", "lower(t.container)"),
        Field::Path => column(
            "(d.path_text || '/' || f.name_text)",
            "hk_fold(d.path_text || '/' || f.name_text)",
        ),
        Field::Folder => column("d.path_text", "hk_fold(d.path_text)"),
        Field::Filename => column("f.name_text", "hk_fold(f.name_text)"),
        Field::Tag(key) => TextSource::Tag(key.clone()),
        _ => return None,
    })
}

/// The column of a field that holds numbers, lengths, sizes or times, and whether it
/// can be NULL.
fn scalar_column(field: &Field) -> Option<(&'static str, bool)> {
    Some(match field {
        Field::Year => ("t.year", true),
        Field::OriginalYear => ("t.original_year", true),
        Field::Track => ("t.track_no", true),
        Field::Disc => ("t.disc_no", true),
        Field::MovementNumber => ("t.movement_no", true),
        Field::SampleRate => ("t.sample_rate", true),
        Field::BitDepth => ("t.bit_depth", true),
        Field::Channels => ("t.channels", true),
        Field::Bitrate => ("t.bitrate", true),
        Field::TrackGain => ("t.rg_track_gain", true),
        Field::AlbumGain => ("t.rg_album_gain", true),
        Field::Plays => ("coalesce(s.play_count, 0)", false),
        Field::Skips => ("coalesce(s.skip_count, 0)", false),
        Field::Rating => ("s.rating", true),
        Field::Duration => ("t.duration_ms", true),
        Field::Resume => ("s.resume_ms", true),
        Field::Size => ("f.size", true),
        Field::Added => ("t.added_at", false),
        Field::Modified => ("t.modified_at", true),
        Field::LastPlayed => ("s.last_played", true),
        Field::FirstPlayed => ("s.first_played", true),
        Field::LastSkipped => ("s.last_skipped", true),
        _ => return None,
    })
}

/// `expr` as a truth value: NULL, where the expression can be NULL, is false.
fn truth(expr: String, nullable: bool) -> String {
    if nullable {
        format!("coalesce({expr}, 0)")
    } else {
        expr
    }
}

fn negate(expr: String) -> String {
    format!("NOT ({expr})")
}

/// The `tracks.kind` of `kind`.
fn kind_number(kind: MediaKind) -> i64 {
    match kind {
        MediaKind::Audiobook => 1,
        MediaKind::Podcast => 2,
        MediaKind::Video => 3,
        _ => 0,
    }
}

/// The comparison operator of SQL for `op`; `:` and `=` are both equality.
fn sql_operator(op: Operator) -> &'static str {
    match op {
        Operator::Less => "<",
        Operator::LessOrEqual => "<=",
        Operator::Greater => ">",
        Operator::GreaterOrEqual => ">=",
        _ => "=",
    }
}

struct Sql {
    params: Vec<SqlValue>,
    context: QueryContext,
}

impl Sql {
    /// Adds a parameter and returns its placeholder.
    fn bind(&mut self, value: impl Into<SqlValue>) -> String {
        self.params.push(value.into());
        format!("?{}", self.params.len())
    }

    fn expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::All(children) if children.is_empty() => "1".to_owned(),
            Expr::Any(children) if children.is_empty() => "0".to_owned(),
            Expr::All(children) => self.group(children, " AND "),
            Expr::Any(children) => self.group(children, " OR "),
            Expr::Not(inner) => negate(self.expr(inner)),
            Expr::Text(text) => self.text(text),
            Expr::Compare(comparison) => self.compare(comparison),
            Expr::Has(presence) => self.has(presence),
            Expr::Is(flag) => is(*flag),
            _ => "0".to_owned(),
        }
    }

    fn group(&mut self, children: &[Expr], joiner: &str) -> String {
        let parts: Vec<String> = children.iter().map(|child| self.expr(child)).collect();
        format!("({})", parts.join(joiner))
    }

    /// Plain text, matched as one term of plain search.
    fn text(&mut self, text: &str) -> String {
        let folded = fold(text);
        let term = folded.split_whitespace().collect::<Vec<_>>().join(" ");
        if term.is_empty() {
            return "1".to_owned();
        }
        truth(term_condition(&term, &mut self.params), true)
    }

    fn compare(&mut self, c: &Comparison) -> String {
        if c.op == Operator::NotEquals {
            let equal = Comparison {
                op: Operator::Equals,
                ..c.clone()
            };
            return negate(self.compare(&equal));
        }
        match c.field.kind() {
            FieldKind::Text => match (text_source(&c.field), &c.value) {
                (Some(source), Value::Text(text)) => self.text_field(&source, c.op, text),
                _ => "0".to_owned(),
            },
            FieldKind::Kind => match &c.value {
                Value::Kind(kind) => {
                    let p = self.bind(kind_number(*kind));
                    format!("t.kind = {p}")
                }
                _ => "0".to_owned(),
            },
            FieldKind::Date => self.date(&c.field, c.op, &c.value),
            _ => self.scalar(&c.field, c.op, &c.value),
        }
    }

    /// A comparison of a text field, with `op` one of `:`, `=` and `~`.
    fn text_field(&mut self, source: &TextSource, op: Operator, text: &str) -> String {
        let folded = fold(text);
        match source {
            TextSource::Column {
                raw,
                folded: column,
                trigram,
            } => match (op, trigram) {
                (Operator::Contains, Some(index)) if terms::is_long(&folded) => {
                    let p = self.bind(format!("{index} : {}", terms::fts_string(&folded)));
                    format!("t.id IN (SELECT rowid FROM tracks_tri WHERE tracks_tri MATCH {p})")
                }
                (Operator::Contains, _) => {
                    let p = self.bind(folded);
                    truth(format!("instr({column}, {p}) > 0"), true)
                }
                (Operator::Matches, _) => {
                    let p = self.bind(text.to_owned());
                    truth(format!("regexp({p}, {raw})"), true)
                }
                _ => {
                    let p = self.bind(folded);
                    truth(format!("{column} = {p}"), true)
                }
            },
            TextSource::People(role) => {
                let condition = self.name_condition(op, text, "p.fold", "p.name");
                format!(
                    "t.id IN (SELECT tp.track_id FROM track_people tp
                        JOIN people p ON p.id = tp.person_id
                        WHERE tp.role = {role} AND {condition})"
                )
            }
            TextSource::AlbumPeople => {
                let condition = self.name_condition(op, text, "p.fold", "p.name");
                truth(
                    format!(
                        "t.album_id IN (SELECT ap.album_id FROM album_people ap
                            JOIN people p ON p.id = ap.person_id WHERE {condition})"
                    ),
                    true,
                )
            }
            TextSource::Genres => {
                let condition = self.name_condition(op, text, "g.fold", "g.name");
                format!(
                    "t.id IN (SELECT tg.track_id FROM track_genres tg
                        JOIN genres g ON g.id = tg.genre_id WHERE {condition})"
                )
            }
            TextSource::Album => {
                let condition = self.name_condition(op, text, "al.title_fold", "al.title");
                truth(
                    format!("t.album_id IN (SELECT al.id FROM albums al WHERE {condition})"),
                    true,
                )
            }
            TextSource::Tag(key) => {
                let key = self.bind(key.clone());
                let condition = self.name_condition(op, text, "hk_fold(tt.value)", "tt.value");
                format!(
                    "t.id IN (SELECT tt.track_id FROM track_tags tt
                        WHERE tt.key = {key} AND {condition})"
                )
            }
        }
    }

    /// `op` (`:`, `=` or `~`) on a name in its `folded` and `raw` columns.
    fn name_condition(&mut self, op: Operator, text: &str, folded: &str, raw: &str) -> String {
        match op {
            Operator::Contains => {
                let p = self.bind(fold(text));
                format!("instr({folded}, {p}) > 0")
            }
            Operator::Matches => {
                let p = self.bind(text.to_owned());
                truth(format!("regexp({p}, {raw})"), true)
            }
            _ => {
                let p = self.bind(fold(text));
                format!("{folded} = {p}")
            }
        }
    }

    /// A number, rating, length or size in `op` (not `!=`) to `value`.
    fn scalar(&mut self, field: &Field, op: Operator, value: &Value) -> String {
        let Some((column, nullable)) = scalar_column(field) else {
            return "0".to_owned();
        };
        if let Value::Range(low, high) = value {
            let mut ends = [
                (&**low, sql_scalar(field, low)),
                (&**high, sql_scalar(field, high)),
            ];
            let [(_, Some(first)), (_, Some(second))] = &ends else {
                return "0".to_owned();
            };
            if order_key(second) < order_key(first) {
                ends.swap(0, 1);
            }
            let [(_, Some(low)), (high_value, Some(mut high))] = ends else {
                return "0".to_owned();
            };
            // A range up to a whole second takes in that whole second.
            if let (Value::Duration(ms), SqlValue::Integer(end)) = (high_value, &mut high)
                && ms % 1000 == 0
            {
                *end = end.saturating_add(999);
            }
            let (low, high) = (self.bind(low), self.bind(high));
            return truth(format!("{column} BETWEEN {low} AND {high}"), nullable);
        }
        let Some(bound) = sql_scalar(field, value) else {
            return "0".to_owned();
        };
        // A length in whole seconds equals every length within that second.
        if let (Value::Duration(ms), Operator::Contains | Operator::Equals) = (value, op)
            && ms % 1000 == 0
        {
            let p = self.bind(i64::try_from(ms / 1000).unwrap_or(i64::MAX));
            return truth(format!("{column} / 1000 = {p}"), nullable);
        }
        let p = self.bind(bound);
        truth(format!("{column} {} {p}", sql_operator(op)), nullable)
    }

    /// A time compared with a calendar date or an age.
    fn date(&mut self, field: &Field, op: Operator, value: &Value) -> String {
        let Some((column, nullable)) = scalar_column(field) else {
            return "0".to_owned();
        };
        let context = self.context;
        let condition = match value {
            Value::Range(low, high) => {
                let (Some(low), Some(high)) =
                    (span_of_end(&context, low), span_of_end(&context, high))
                else {
                    return "0".to_owned();
                };
                let start = low.0.min(high.0);
                let end = low.1.max(high.1);
                let (start, end) = (self.bind(start), self.bind(end));
                format!("{column} >= {start} AND {column} < {end}")
            }
            Value::Age(age) => {
                let bound = self.bind(calendar::age_bound(&context, *age));
                match op {
                    // Less than 30 days ago: after the bound.
                    Operator::Less => format!("{column} > {bound}"),
                    Operator::LessOrEqual | Operator::Contains | Operator::Equals => {
                        format!("{column} >= {bound}")
                    }
                    Operator::Greater => format!("{column} < {bound}"),
                    _ => format!("{column} <= {bound}"),
                }
            }
            Value::Date(date) => {
                let (start, end) = calendar::period(&context, *date);
                match op {
                    Operator::Less => format!("{column} < {}", self.bind(start)),
                    Operator::LessOrEqual => format!("{column} < {}", self.bind(end)),
                    Operator::Greater => format!("{column} >= {}", self.bind(end)),
                    Operator::GreaterOrEqual => format!("{column} >= {}", self.bind(start)),
                    _ => {
                        let (start, end) = (self.bind(start), self.bind(end));
                        format!("{column} >= {start} AND {column} < {end}")
                    }
                }
            }
            _ => return "0".to_owned(),
        };
        truth(format!("({condition})"), nullable)
    }

    fn has(&mut self, presence: &Presence) -> String {
        let field = match presence {
            Presence::Lyrics => return "t.has_lyrics = 1".to_owned(),
            Presence::Cover => {
                return "(t.cover_id IS NOT NULL OR a.cover_id IS NOT NULL
                         OR d.cover_id IS NOT NULL)"
                    .to_owned();
            }
            Presence::Field(field) => field,
            _ => return "0".to_owned(),
        };
        if let Some(source) = text_source(field) {
            return match source {
                TextSource::Column { raw, .. } => truth(format!("{raw} <> ''"), true),
                TextSource::People(role) => {
                    format!("t.id IN (SELECT track_id FROM track_people WHERE role = {role})")
                }
                TextSource::AlbumPeople => truth(
                    "t.album_id IN (SELECT album_id FROM album_people)".to_owned(),
                    true,
                ),
                TextSource::Genres => "t.id IN (SELECT track_id FROM track_genres)".to_owned(),
                TextSource::Album => "t.album_id IS NOT NULL".to_owned(),
                TextSource::Tag(key) => {
                    let key = self.bind(key);
                    format!("t.id IN (SELECT track_id FROM track_tags WHERE key = {key})")
                }
            };
        }
        match (field, scalar_column(field)) {
            (Field::Plays | Field::Skips, Some((column, _))) => format!("{column} > 0"),
            (_, Some((column, _))) => format!("{column} IS NOT NULL"),
            // Every track has a kind.
            _ => "1".to_owned(),
        }
    }

    fn sort_key(&mut self, key: &SortKey) -> String {
        let field = match &key.by {
            SortBy::Random => {
                // A hash of the id and the seed: the same order for the same seed.
                let seed = self.bind(i64::try_from(self.context.seed % (1 << 31)).unwrap_or(0));
                let x = format!("((t.id + {seed}) % 2147483647)");
                return format!("((({x} * {x}) % 2147483647) * 48271) % 2147483647");
            }
            SortBy::Field(field) => field,
        };
        let d = if key.descending { " DESC" } else { "" };
        let people = |role: i64| {
            format!(
                "(SELECT coalesce(p.sort_name, p.name) FROM track_people tp
                    JOIN people p ON p.id = tp.person_id
                    WHERE tp.track_id = t.id AND tp.role = {role}
                    ORDER BY tp.position LIMIT 1) COLLATE HK_LOCALE"
            )
        };
        let (expr, nullable) = match field {
            Field::Title => ("t.title_sort".to_owned(), true),
            Field::Artist => ("t.artist COLLATE HK_LOCALE".to_owned(), true),
            Field::AlbumArtist => ("a.artist_sort".to_owned(), true),
            Field::Album => ("a.title_sort".to_owned(), true),
            Field::Composer => (people(2), true),
            Field::Performer => (people(3), true),
            Field::Conductor => (people(4), true),
            Field::Lyricist => (people(5), true),
            Field::Genre => (
                "(SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                    WHERE tg.track_id = t.id ORDER BY tg.position LIMIT 1) COLLATE HK_LOCALE"
                    .to_owned(),
                true,
            ),
            Field::Tag(tag) => {
                let key = self.bind(tag.clone());
                (
                    format!(
                        "(SELECT tt.value FROM track_tags tt WHERE tt.track_id = t.id
                            AND tt.key = {key} ORDER BY tt.rowid LIMIT 1) COLLATE HK_LOCALE"
                    ),
                    true,
                )
            }
            Field::Work => ("t.work COLLATE HK_LOCALE".to_owned(), true),
            Field::Movement => ("t.movement COLLATE HK_LOCALE".to_owned(), true),
            Field::DiscSubtitle => ("t.disc_subtitle COLLATE HK_LOCALE".to_owned(), true),
            Field::ReleaseType => ("t.release_type COLLATE HK_LOCALE".to_owned(), true),
            Field::Codec => ("lower(t.codec)".to_owned(), true),
            Field::Container => ("lower(t.container)".to_owned(), true),
            Field::Path => return format!("d.path{d}, f.name{d}"),
            Field::Folder => ("d.path".to_owned(), false),
            Field::Filename => ("f.name_text COLLATE HK_LOCALE".to_owned(), false),
            Field::Kind => ("t.kind".to_owned(), false),
            field => match scalar_column(field) {
                Some((column, nullable)) => (column.to_owned(), nullable),
                None => return "t.id".to_owned(),
            },
        };
        if nullable {
            format!("{expr} IS NULL, {expr}{d}")
        } else {
            format!("{expr}{d}")
        }
    }
}

/// The span of a date or an age as the end of a range: a date is its whole period, an
/// age the moment it names.
fn span_of_end(context: &QueryContext, value: &Value) -> Option<(i64, i64)> {
    match value {
        Value::Date(date) => Some(calendar::period(context, *date)),
        Value::Age(age) => {
            let bound = calendar::age_bound(context, *age);
            Some((bound, bound.saturating_add(1)))
        }
        _ => None,
    }
}

/// A value to order two range ends by.
fn order_key(value: &SqlValue) -> f64 {
    match value {
        SqlValue::Integer(n) => *n as f64,
        SqlValue::Real(x) => *x,
        _ => 0.0,
    }
}

/// The SQL value of a number, length or size of `field` in the column's unit.
fn sql_scalar(field: &Field, value: &Value) -> Option<SqlValue> {
    let whole_or_real = |n: helikodu_model::query::Decimal| match n.as_int() {
        Some(n) => SqlValue::Integer(n),
        None => SqlValue::Real(n.as_f64()),
    };
    Some(match (field, value) {
        // kbit/s in thousandths is bit/s.
        (Field::Bitrate, Value::Number(n)) => SqlValue::Integer(n.thousandths()),
        // Stars in thousandths, in half stars.
        (Field::Rating, Value::Number(n)) => SqlValue::Integer(n.thousandths() / 500),
        (Field::TrackGain | Field::AlbumGain, Value::Number(n)) => SqlValue::Real(n.as_f64()),
        (_, Value::Number(n)) => whole_or_real(*n),
        (_, Value::Duration(ms)) => SqlValue::Integer(i64::try_from(*ms).unwrap_or(i64::MAX)),
        (_, Value::Size(bytes)) => SqlValue::Integer(i64::try_from(*bytes).unwrap_or(i64::MAX)),
        _ => return None,
    })
}

fn is(flag: Flag) -> String {
    match flag {
        Flag::Lossless => format!("(t.codec IS NOT NULL AND {LOSSLESS})"),
        Flag::HiRes => format!(
            "(t.codec IS NOT NULL AND {LOSSLESS}
              AND (coalesce(t.sample_rate, 0) > 48000 OR coalesce(t.bit_depth, 0) > 16))"
        ),
        Flag::Compilation => {
            "(coalesce(a.is_compilation, 0) = 1 OR coalesce(t.compilation, 0) = 1)".to_owned()
        }
        Flag::Loved => "coalesce(s.loved, 0) = 1".to_owned(),
        _ => "0".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::query::Decimal;

    use super::*;

    const MARK: &str = "Zqx'\"); DROP TABLE tracks; --";

    fn context() -> QueryContext {
        QueryContext {
            now: 1_800_000_000,
            utc_offset: 7200,
            seed: 42,
        }
    }

    /// The highest `?N` in `sql`.
    fn highest_placeholder(sql: &str) -> usize {
        sql.split('?')
            .skip(1)
            .filter_map(|rest| {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn values_are_bound_never_written() {
        let tag = Field::Tag("ZQX".into());
        let mut exprs = vec![Expr::text(MARK)];
        for field in Field::named()
            .filter(|f| f.kind() == FieldKind::Text)
            .chain([tag.clone()])
        {
            for op in [
                Operator::Contains,
                Operator::Equals,
                Operator::NotEquals,
                Operator::Matches,
            ] {
                exprs.push(Expr::compare(field.clone(), op, Value::Text(MARK.into())));
            }
            exprs.push(Expr::Has(Presence::Field(field)));
        }
        let query = Query {
            filter: Some(Expr::Any(exprs)),
            sort: vec![
                SortKey::ascending(tag),
                SortKey {
                    by: SortBy::Random,
                    descending: false,
                },
            ],
            limit: Some(Limit::tracks(5)),
        };
        let compiled = compile(&query, &context()).unwrap();
        for sql in [&compiled.condition, &compiled.order] {
            assert!(!sql.to_lowercase().contains("zqx"), "{sql}");
            assert!(!sql.contains("DROP"), "{sql}");
        }
        assert!(
            compiled
                .params
                .iter()
                .any(|p| matches!(p, SqlValue::Text(t) if t.contains("Zqx")))
        );
        let sql = format!("{} {}", compiled.condition, compiled.order);
        assert_eq!(highest_placeholder(&sql), compiled.params.len());
    }

    #[test]
    fn numbers_are_bound_in_column_units() {
        let compare = |field: Field, value: Value| {
            let query = Query::new(Expr::compare(field, Operator::GreaterOrEqual, value));
            compile(&query, &context()).unwrap().params
        };
        assert_eq!(
            compare(
                Field::Rating,
                Value::Number(Decimal::from_thousandths(3500))
            ),
            [SqlValue::Integer(7)]
        );
        assert_eq!(
            compare(Field::Bitrate, Value::number(320)),
            [SqlValue::Integer(320_000)]
        );
        assert_eq!(
            compare(Field::TrackGain, Value::number(-6)),
            [SqlValue::Real(-6.0)]
        );
        assert_eq!(
            compare(Field::Duration, Value::Duration(30_000)),
            [SqlValue::Integer(30_000)]
        );
        assert_eq!(
            compile(
                &Query::new(Expr::compare(
                    Field::Title,
                    Operator::Less,
                    Value::number(1)
                )),
                &context()
            ),
            Err(TermProblem::Operator)
        );
    }

    #[test]
    fn limits_count_tracks_time_or_bytes() {
        let limit = |amount, unit| super::limit(Limit { amount, unit });
        assert_eq!(limit(50, LimitUnit::Tracks).measure, None);
        assert_eq!(limit(90, LimitUnit::Minutes).amount, 5_400_000);
        assert_eq!(limit(2, LimitUnit::Hours).amount, 7_200_000);
        assert_eq!(limit(700, LimitUnit::Megabytes).amount, 700_000_000);
        assert_eq!(limit(4, LimitUnit::Gigabytes).amount, 4_000_000_000);
    }
}
