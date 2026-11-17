// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The latency of plain search over a generated library.
//!
//! ```sh
//! cargo bench -p helikodu-library --bench search [-- <number of tracks>]
//! ```
//!
//! Prints the time to build the library, then one line per query: the number of
//! matches and the median and slowest time to fetch the first page of 50 with the
//! total count. The default is 100 000 tracks.

#![forbid(unsafe_code)]

use std::hint::black_box;
use std::time::{Duration, Instant};

use helikodu_library::{
    AudioProperties, DatabaseConfig, FileStamp, Library, SearchRequest, TrackRecord, TrackTags,
};

/// Words in several scripts. Chinese, Japanese and Thai words are joined without
/// spaces, the others with spaces.
const LATIN: &[&str] = &[
    "love",
    "night",
    "blue",
    "über",
    "straße",
    "mädchen",
    "café",
    "naïve",
    "dvořák",
    "łódź",
    "song",
    "heart",
    "river",
    "dream",
    "fire",
    "rain",
    "summer",
    "moon",
    "city",
    "light",
    "live",
    "remastered",
    "part",
    "demo",
    "mix",
    "time",
    "road",
    "home",
    "star",
    "wind",
];
const CYRILLIC_GREEK: &[&str] = &[
    "москва",
    "ёлка",
    "ночь",
    "песня",
    "любовь",
    "ελλάδα",
    "ουρανός",
    "θάλασσα",
    "αγάπη",
];
const UNSPACED: &[&str] = &[
    "月亮",
    "代表",
    "我的心",
    "夜に",
    "駆ける",
    "ジブリ",
    "千尋",
    "神隠し",
    "ひこうき",
    "雲",
    "ลมหายใจ",
    "คราม",
    "사랑",
    "노래",
    "바다",
];

/// A deterministic pseudo-random number generator (xorshift64*).
struct Random(u64);

impl Random {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as usize
    }

    fn pick<'a>(&mut self, words: &[&'a str]) -> &'a str {
        words[self.next() % words.len()]
    }

    /// One to four words of one script family.
    fn phrase(&mut self) -> String {
        let words = 1 + self.next() % 4;
        match self.next() % 10 {
            0..=5 => (0..words)
                .map(|_| self.pick(LATIN))
                .collect::<Vec<_>>()
                .join(" "),
            6 | 7 => (0..words)
                .map(|_| self.pick(CYRILLIC_GREEK))
                .collect::<Vec<_>>()
                .join(" "),
            _ => (0..words).map(|_| self.pick(UNSPACED)).collect(),
        }
    }
}

fn library(count: usize) -> Library {
    let library = Library::open(&DatabaseConfig::in_memory()).expect("in-memory library");
    let root = library.add_root("/music").expect("root");
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let artists: Vec<String> = (0..count / 20 + 1).map(|_| random.phrase()).collect();
    let albums: Vec<String> = (0..count / 10 + 1).map(|_| random.phrase()).collect();
    let mut batch = Vec::new();
    for n in 0..count {
        let album = n / 10;
        let mut record = TrackRecord::new(
            root,
            format!("/music/{}/{:02} track.flac", album, n % 10),
            FileStamp {
                size: n as u64,
                mtime_ns: Some(0),
                inode: Some(n as u64),
                quick_hash: (n as u64).to_le_bytes().to_vec(),
            },
        );
        record.audio = AudioProperties {
            duration: Some(Duration::from_secs(120 + (n % 300) as u64)),
            sample_rate: Some(44_100),
            channels: Some(2),
            ..AudioProperties::default()
        };
        record.tags = TrackTags {
            title: Some(random.phrase()),
            artists: vec![artists[(album * 7) % artists.len()].clone()],
            album: Some(albums[album % albums.len()].clone()),
            genres: vec![["Rock", "Jazz", "Classical", "Pop", "Folk"][album % 5].to_owned()],
            track_no: Some((n % 10) as u32 + 1),
            year: Some(1960 + (album % 60) as i32),
            ..TrackTags::default()
        };
        batch.push(record);
        if batch.len() == 5000 {
            library
                .upsert_tracks(std::mem::take(&mut batch))
                .expect("insert");
        }
    }
    library.upsert_tracks(batch).expect("insert");
    library
}

fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| arg != "--bench")
        .collect();
    let count: usize = args
        .first()
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(100_000);
    let chosen: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let start = Instant::now();
    let library = library(count);
    let elapsed = start.elapsed();
    println!(
        "{count} generated tracks, written with both search indexes in {:.1} s ({:.1} µs per track)",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1e6 / count as f64
    );

    let start = Instant::now();
    library.rebuild_search_index().expect("rebuild");
    println!(
        "rebuild both search indexes: {:.0} ms",
        start.elapsed().as_secs_f64() * 1e3
    );

    println!(
        "{:<28} {:>8} {:>10} {:>10}",
        "query (first page of 50)", "matches", "median", "slowest"
    );
    let all = [
        "a",
        "o",
        "e",
        "ö",
        "9",
        "lo",
        "ni",
        "月",
        "心",
        "雲",
        "사",
        "ล",
        "мо",
        "αγ",
        "lov",
        "love",
        "LOVE song",
        "straße",
        "dvorak",
        "москва",
        "ёлка",
        "αγάπη",
        "我的心",
        "代表",
        "神隠し",
        "ジブリ",
        "ลมหายใจ",
        "사랑",
        "night rain",
        "zzz",
        "a b c",
        "remastered live",
    ];
    let queries: Vec<&str> = if chosen.is_empty() {
        all.to_vec()
    } else {
        chosen
    };
    for query in queries {
        let request = SearchRequest::new(query);
        let mut times = Vec::new();
        let mut total = 0;
        for _ in 0..7 {
            let start = Instant::now();
            let results = library.search(&request).expect("search");
            times.push(start.elapsed());
            total = results.total;
            black_box(results);
        }
        times.sort();
        println!(
            "{:<28} {:>8} {:>8.1} ms {:>8.1} ms",
            format!("{query:?}"),
            total,
            times[times.len() / 2].as_secs_f64() * 1e3,
            times[times.len() - 1].as_secs_f64() * 1e3
        );
    }
}
