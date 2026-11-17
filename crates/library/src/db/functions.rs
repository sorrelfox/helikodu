// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SQL functions registered on every connection: `regexp()`, `hk_fold()` and
//! `hk_match_rank()`.

use std::borrow::Cow;

use regex::{Regex, RegexBuilder};
use rusqlite::Connection;
use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::ValueRef;

use crate::search::{fold, match_rank};

/// The most memory a compiled pattern may use. Patterns come from user queries.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Registers the functions on `connection`.
pub(crate) fn register(connection: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    // `value REGEXP pattern` calls `regexp(pattern, value)`.
    connection.create_scalar_function("regexp", 2, flags, regexp)?;
    connection.create_scalar_function("hk_fold", 1, flags, hk_fold)?;
    connection.create_scalar_function("hk_match_rank", 2, flags, hk_match_rank)?;
    Ok(())
}

/// `regexp(pattern, value)`: whether `value` contains a match of `pattern`, in the
/// syntax of the `regex` crate. NULL if either is NULL. A BLOB is matched as text, with
/// invalid UTF-8 replaced. The compiled pattern is cached for the statement.
fn regexp(ctx: &mut Context<'_>) -> rusqlite::Result<Option<bool>> {
    if ctx.get_raw(0) == ValueRef::Null {
        return Ok(None);
    }
    let Some(value) = text_of(ctx.get_raw(1)) else {
        return Ok(None);
    };
    let pattern = ctx.get_or_create_aux(0, |pattern| -> Result<Regex, BoxError> {
        let pattern = pattern.as_str()?;
        Ok(RegexBuilder::new(pattern)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_SIZE_LIMIT)
            .build()?)
    })?;
    Ok(Some(pattern.is_match(&value)))
}

/// `hk_fold(text)`: the folded form of `text` (see [`fold`]). NULL for NULL.
fn hk_fold(ctx: &mut Context<'_>) -> rusqlite::Result<Option<String>> {
    Ok(text_of(ctx.get_raw(0)).map(|text| fold(&text)))
}

/// `hk_match_rank(field, query)`: how the folded `field` matches the folded `query`,
/// from 0 (equal) to 4 (not as a whole); see [`MatchRank`](crate::search::MatchRank).
/// NULL if either is NULL.
fn hk_match_rank(ctx: &mut Context<'_>) -> rusqlite::Result<Option<i64>> {
    let (Some(field), Some(query)) = (text_of(ctx.get_raw(0)), text_of(ctx.get_raw(1))) else {
        return Ok(None);
    };
    Ok(Some(match_rank(&field, &query).to_sql()))
}

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The text form of an SQL value; `None` for NULL.
fn text_of(value: ValueRef<'_>) -> Option<Cow<'_, str>> {
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(number) => Some(Cow::Owned(number.to_string())),
        ValueRef::Real(number) => Some(Cow::Owned(number.to_string())),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(String::from_utf8_lossy(bytes)),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        super::register(&connection).unwrap();
        connection
    }

    fn query<T: rusqlite::types::FromSql>(connection: &Connection, sql: &str) -> T {
        connection.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn regexp_operator() {
        let c = connection();
        assert!(query::<bool>(&c, "SELECT 'Intro' REGEXP '^(Intro|Outro)$'"));
        assert!(!query::<bool>(
            &c,
            "SELECT 'Introduction' REGEXP '^(Intro|Outro)$'"
        ));
        assert!(query::<bool>(&c, "SELECT 'Hello' REGEXP '(?i)^hel'"));
        assert!(query::<bool>(
            &c,
            "SELECT 'Пётр Чайковский' REGEXP 'Чайков'"
        ));
        assert!(query::<bool>(&c, "SELECT 1999 REGEXP '^19'"));
        assert!(query::<bool>(&c, "SELECT x'4361666580' REGEXP '^Caf'"));
        assert_eq!(query::<Option<bool>>(&c, "SELECT NULL REGEXP 'a'"), None);
        assert_eq!(query::<Option<bool>>(&c, "SELECT 'a' REGEXP NULL"), None);
    }

    #[test]
    fn regexp_over_many_rows_uses_one_pattern() {
        let c = connection();
        let count: i64 = query(
            &c,
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000)
             SELECT count(*) FROM n WHERE ('track ' || i) REGEXP '^track \\d*7$'",
        );
        assert_eq!(count, 100);
    }

    #[test]
    fn invalid_or_huge_pattern_is_an_error() {
        let c = connection();
        let invalid = c.query_row("SELECT 'a' REGEXP '('", [], |row| row.get::<_, bool>(0));
        assert!(invalid.is_err());
        let huge = c.query_row("SELECT 'a' REGEXP '\\w{1000}{1000}'", [], |row| {
            row.get::<_, bool>(0)
        });
        assert!(huge.is_err());
    }

    #[test]
    fn hk_fold_function() {
        let c = connection();
        assert_eq!(
            query::<String>(&c, "SELECT hk_fold('Ärzte Straße')"),
            "arzte strasse"
        );
        assert_eq!(query::<Option<String>>(&c, "SELECT hk_fold(NULL)"), None);
    }

    #[test]
    fn hk_match_rank_function() {
        let c = connection();
        assert_eq!(query::<i64>(&c, "SELECT hk_match_rank('love', 'love')"), 0);
        assert_eq!(
            query::<i64>(&c, "SELECT hk_match_rank('my love', 'love')"),
            2
        );
        assert_eq!(query::<i64>(&c, "SELECT hk_match_rank('x', 'love')"), 4);
        assert_eq!(
            query::<Option<i64>>(&c, "SELECT hk_match_rank(NULL, 'love')"),
            None
        );
    }
}
