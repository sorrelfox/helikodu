// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The cost of the `HK_LOCALE` collation and of folding, over generated titles.
//!
//! ```sh
//! cargo bench -p helikodu-library --bench collation [-- <number of titles>]
//! ```
//!
//! Prints one line per measurement. The default is 100 000 titles.

#![forbid(unsafe_code)]

use std::cell::Cell;
use std::hint::black_box;
use std::time::{Duration, Instant};

use helikodu_library::{Database, DatabaseConfig, LocaleCollation, fold};

/// Words in several scripts, so that the titles exercise contractions, expansions,
/// diacritics and the implicit weights of Han characters.
const WORDS: &[&str] = &[
    "the",
    "love",
    "song",
    "Night",
    "Blue",
    "über",
    "Straße",
    "Mädchen",
    "Öl",
    "Åsa",
    "ängel",
    "själ",
    "öö",
    "šokk",
    "žanr",
    "õhtu",
    "Café",
    "naïve",
    "Dvořák",
    "Łódź",
    "Ελλάδα",
    "ουρανός",
    "Москва",
    "ёлка",
    "Чайковский",
    "月亮",
    "代表",
    "我的心",
    "夜に",
    "駆ける",
    "ジブリ",
    "ลมหายใจ",
    "คราม",
    "Live",
    "Remastered",
    "2011",
    "No. 9",
    "(Demo)",
    "Part II",
    "feat.",
    "Œuvre",
    "ﬁne",
    "Ǆungla",
    "İstanbul",
    "Ærø",
    "Ҷамшед",
];

/// A deterministic pseudo-random number generator (xorshift64*).
struct Random(u64);

impl Random {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        // The high 32 bits are the best ones.
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as usize
    }
}

fn titles(count: usize) -> Vec<String> {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    (0..count)
        .map(|_| {
            let words = 1 + random.next() % 5;
            (0..words)
                .map(|_| WORDS[random.next() % WORDS.len()])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn report(what: &str, elapsed: Duration, per: Option<(u64, &str)>) {
    let ms = elapsed.as_secs_f64() * 1e3;
    match per {
        Some((count, unit)) if count > 0 => {
            let ns = elapsed.as_secs_f64() * 1e9 / count as f64;
            println!("{what:<52} {ms:>9.1} ms  ({ns:.0} ns per {unit}, {count} {unit}s)");
        }
        _ => println!("{what:<52} {ms:>9.1} ms"),
    }
}

fn sort_with(
    titles: &[String],
    compare: impl Fn(&str, &str) -> std::cmp::Ordering,
) -> (Duration, u64) {
    let mut sorted: Vec<&str> = titles.iter().map(String::as_str).collect();
    let comparisons = Cell::new(0_u64);
    let start = Instant::now();
    sorted.sort_by(|a, b| {
        comparisons.set(comparisons.get() + 1);
        compare(a, b)
    });
    let elapsed = start.elapsed();
    black_box(&sorted);
    (elapsed, comparisons.get())
}

fn main() {
    let count: usize = std::env::args()
        .skip(1)
        .find_map(|arg| arg.parse().ok())
        .unwrap_or(100_000);
    let titles = titles(count);
    println!("{count} generated titles");

    let start = Instant::now();
    let root = LocaleCollation::root();
    report("create the root collator", start.elapsed(), None);
    let start = Instant::now();
    let swedish = LocaleCollation::new("sv").expect("Swedish collation data");
    report("create the Swedish collator", start.elapsed(), None);

    let (elapsed, n) = sort_with(&titles, |a, b| a.cmp(b));
    report("sort, code-point order", elapsed, Some((n, "comparison")));
    let (elapsed, n) = sort_with(&titles, |a, b| root.compare(a, b));
    report("sort, root collation", elapsed, Some((n, "comparison")));
    let (elapsed, n) = sort_with(&titles, |a, b| swedish.compare(a, b));
    report("sort, Swedish collation", elapsed, Some((n, "comparison")));

    let start = Instant::now();
    let folded: Vec<String> = titles.iter().map(|t| fold(t)).collect();
    report(
        "fold every title",
        start.elapsed(),
        Some((folded.len() as u64, "title")),
    );

    let config = DatabaseConfig::in_memory().with_collation(swedish);
    let db = Database::open(&config).expect("in-memory database");
    let rows: Vec<String> = titles.clone();
    let start = Instant::now();
    db.writer()
        .expect("writer")
        .write(move |c| {
            c.execute_batch(
                "INSERT INTO roots (id, path, path_text) VALUES (1, x'2f', '/');
                 INSERT INTO dirs (id, root_id, path, path_text) VALUES (1, 1, x'2f', '/');",
            )?;
            let mut file = c.prepare(
                "INSERT INTO files (id, dir_id, name, name_text) VALUES (?1, 1, ?2, ?3)",
            )?;
            let mut identity =
                c.prepare("INSERT INTO identities (id, created_at) VALUES (?1, 0)")?;
            let mut track = c.prepare(
                "INSERT INTO tracks (file_id, identity_id, title, title_sort, title_fold, added_at)
                 VALUES (?1, ?1, ?2, ?2, hk_fold(?2), 0)",
            )?;
            for (id, title) in (1_i64..).zip(&rows) {
                file.execute((id, id.to_string().into_bytes(), id.to_string()))?;
                identity.execute([id])?;
                track.execute((id, title))?;
            }
            Ok(())
        })
        .expect("insert");
    report(
        "insert tracks, keeping the collation index",
        start.elapsed(),
        Some((count as u64, "track")),
    );

    let start = Instant::now();
    db.set_collation(LocaleCollation::new("de").expect("German collation data"))
        .expect("reindex");
    report(
        "switch locale: REINDEX HK_LOCALE (all indexes)",
        start.elapsed(),
        None,
    );

    let page = |sql: &str| {
        let start = Instant::now();
        let rows = db
            .read(|c| {
                let mut statement = c.prepare(sql)?;
                let rows = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .count();
                Ok(rows)
            })
            .expect("query");
        black_box(rows);
        start.elapsed()
    };
    report(
        "page of 50 at offset 50%, through the index",
        page(&format!(
            "SELECT title FROM tracks ORDER BY title_sort LIMIT 50 OFFSET {}",
            count / 2
        )),
        None,
    );
    report(
        "all rows in order, through the index",
        page("SELECT title FROM tracks ORDER BY title_sort"),
        None,
    );
    report(
        "all rows in order, sorted without an index",
        page("SELECT title FROM tracks ORDER BY title COLLATE HK_LOCALE"),
        None,
    );
    report(
        "all rows in code-point order, without an index",
        page("SELECT title FROM tracks ORDER BY title"),
        None,
    );
}
