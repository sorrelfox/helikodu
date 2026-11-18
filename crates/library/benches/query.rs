// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The latency of the query language over a generated library.
//!
//! ```sh
//! cargo bench -p helikodu-library --bench query [-- <number of tracks> [<query> …]]
//! ```
//!
//! Prints the time to build the library, then one line per query: the number of
//! matches and the median and slowest time to parse, compile and fetch the first page
//! of 50 with the total count. The default is 100 000 tracks; the budget is that of
//! search, 100 ms.

#![forbid(unsafe_code)]

use std::hint::black_box;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use helikodu_library::raw::rusqlite;
use helikodu_library::{
    AudioProperties, DatabaseConfig, FileStamp, Library, QueryRequest, TrackRecord, TrackTags,
    parse_query,
};

const WORDS: &[&str] = &[
    "love",
    "night",
    "blue",
    "über",
    "straße",
    "café",
    "dvořák",
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
    "home",
    "москва",
    "ёлка",
    "αγάπη",
    "月亮",
    "代表",
    "ジブリ",
];
const GENRES: &[&str] = &[
    "Rock",
    "Jazz",
    "Classical",
    "Pop",
    "Folk",
    "Jazz Fusion",
    "Ambient",
];
const CODECS: &[&str] = &["flac", "mp3", "opus", "alac", "aac"];

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

    fn phrase(&mut self) -> String {
        let words = 1 + self.next() % 3;
        (0..words)
            .map(|_| self.pick(WORDS))
            .collect::<Vec<_>>()
            .join(" ")
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
                size: 3_000_000 + (n % 50_000_000) as u64,
                mtime_ns: Some(0),
                inode: Some(n as u64),
                quick_hash: (n as u64).to_le_bytes().to_vec(),
            },
        );
        let hi_res = album % 7 == 0;
        record.audio = AudioProperties {
            duration: Some(Duration::from_secs(60 + (n % 400) as u64)),
            sample_rate: Some(if hi_res { 96_000 } else { 44_100 }),
            bit_depth: Some(if hi_res { 24 } else { 16 }),
            channels: Some(2),
            codec: Some(CODECS[album % CODECS.len()].to_owned()),
            ..AudioProperties::default()
        };
        record.tags = TrackTags {
            title: Some(random.phrase()),
            artists: vec![artists[(album * 7) % artists.len()].clone()],
            album: Some(albums[album % albums.len()].clone()),
            genres: vec![GENRES[album % GENRES.len()].to_owned()],
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
    // Statistics for two tracks in three, and times added over three years.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    library
        .database()
        .writer()
        .expect("writer")
        .write(move |c| {
            c.execute(
                "UPDATE tracks SET added_at = ?1 - (id * 7919 % 1095) * 86400",
                [now],
            )?;
            c.execute(
                "INSERT OR REPLACE INTO stats (identity_id, play_count, rating, last_played)
                 SELECT identity_id, id % 9, CASE WHEN id % 4 = 0 THEN NULL ELSE id % 11 END,
                        ?1 - (id * 104729 % 900) * 86400
                 FROM tracks WHERE id % 3 <> 0",
                [now],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .expect("statistics");
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
    println!(
        "{count} generated tracks in {:.1} s",
        start.elapsed().as_secs_f64()
    );
    println!(
        "{:<56} {:>8} {:>10} {:>10}",
        "query (first page of 50 and the total)", "matches", "median", "slowest"
    );
    let all = [
        "love",
        "genre:jazz",
        "artist:love",
        "title:night",
        "title:ni",
        "album:\"blue moon\"",
        "year:1970..1979 rating>=4",
        "genre:jazz -genre:fusion added<30d",
        "codec:flac samplerate>=88200 sort:added desc limit:50",
        "title~\"^(love|night)\" OR duration<70s",
        "plays=0",
        "-has:rating",
        "rating>=4 sort:rating desc",
        "is:hires",
        "lastplayed>1y sort:lastplayed",
        "love -year<1990",
        "added<30d sort:added desc",
        "sort:random limit:2h",
        "sort:artist,year desc",
        "(genre:rock OR genre:pop) year>=2000 bitrate<320 sort:title",
    ];
    let queries: Vec<&str> = if chosen.is_empty() {
        all.to_vec()
    } else {
        chosen
    };
    for text in queries {
        let mut times = Vec::new();
        let mut total = 0;
        for _ in 0..7 {
            let start = Instant::now();
            let query = parse_query(text).expect("query");
            let page = library
                .query_tracks(&QueryRequest::new(query))
                .expect("run");
            times.push(start.elapsed());
            total = page.total;
            black_box(page);
        }
        times.sort();
        println!(
            "{:<56} {:>8} {:>8.1} ms {:>8.1} ms",
            text,
            total,
            times[times.len() / 2].as_secs_f64() * 1e3,
            times[times.len() - 1].as_secs_f64() * 1e3
        );
    }
}
