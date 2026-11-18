// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Times the library queries of the views through the backend the application
//! uses, over the library of the XDG folders of the environment: the album
//! grid, the track table, search, facets, statistics, history, smart playlists
//! and missing files. Prints one line per query with the median and slowest
//! time of a few runs.
//!
//! ```sh
//! XDG_DATA_HOME=… XDG_CONFIG_HOME=… cargo run --release -p helikodu-ui-gtk --example queries
//! ```
//!
//! Option: `--runs N` (default 5). It creates one smart playlist in the
//! database it is pointed at, so point it at a copy.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use helikodu_service::files::AppDirs;
use helikodu_service::{Service, ServiceConfig};
use helikodu_ui_gtk::backend::{
    AlbumQuery, Backend, BackendError, FacetChoice, FacetField, FacetKey, StatisticsSince,
    TrackColumn, TrackKey, TrackQuery, TrackSort, service,
};

fn connect() -> Result<Backend, BackendError> {
    let fail = |e: &dyn std::fmt::Display| BackendError(e.to_string());
    let dirs = AppDirs::from_env().map_err(|e| fail(&e))?;
    let service = Service::start(ServiceConfig::new(dirs)).map_err(|e| fail(&e))?;
    service::connect(
        service,
        service::ServiceOptions {
            mpris: false,
            inhibit_suspend: false,
        },
    )
}

/// Runs `f` `runs` times and prints the median and slowest time with `what`.
fn time<T: std::fmt::Debug>(
    what: &str,
    runs: usize,
    mut f: impl FnMut() -> Result<T, BackendError>,
) -> Option<T> {
    let mut times = Vec::with_capacity(runs);
    let mut last = None;
    for _ in 0..runs.max(1) {
        let start = Instant::now();
        match f() {
            Ok(value) => last = Some(value),
            Err(e) => {
                println!("| {what} | failed: {e} | |");
                return None;
            }
        }
        times.push(start.elapsed());
    }
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "| {what} | {:.1} ms | {:.1} ms | {} |",
        ms(times[times.len() / 2]),
        ms(times[times.len() - 1]),
        last.as_ref().map_or(0, |v| size(&format!("{v:?}")))
    );
    last
}

/// A rough size of a result: the rows of a list (the top-level elements of
/// its debug text), or 1.
fn size(debug: &str) -> usize {
    let Some(body) = debug.strip_prefix('[') else {
        return 1;
    };
    if body == "]" {
        return 0;
    }
    let mut depth = 0i32;
    let mut count = 1;
    let mut quoted = false;
    let mut escaped = false;
    for c in body.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if quoted => {}
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
            ',' if depth == 0 => count += 1,
            _ => {}
        }
    }
    count
}

fn main() -> ExitCode {
    let mut runs = 5;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next().and_then(|v| v.parse().ok())) {
            ("--runs", Some(n)) => runs = n,
            _ => {
                eprintln!("queries: usage: queries [--runs N]");
                return ExitCode::FAILURE;
            }
        }
    }
    let backend = match connect() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("queries: {e}");
            return ExitCode::FAILURE;
        }
    };
    let library = backend.library();
    let services = backend.services();
    println!("| Query | Median | Slowest | Rows |");
    println!("|---|---|---|---|");
    time("counts", runs, || library.counts());
    let albums = time("album grid: all album keys", runs, || {
        library.album_keys(&AlbumQuery::default())
    })
    .unwrap_or_default();
    time("album grid: 60 rows", runs, || {
        library.album_rows(&AlbumQuery::default(), albums.len() / 2, 60)
    });
    if let Some(album) = albums.get(albums.len() / 3) {
        time("album page", runs, || library.album(*album));
    }
    time("artists", runs, || library.artists());
    time("genres", runs, || library.genres());
    let keys = time("track table: all keys, album order", runs, || {
        library.track_keys(&TrackQuery::default())
    })
    .unwrap_or_default();
    time("track table: all keys, by title", runs, || {
        library.track_keys(&TrackQuery {
            sort: vec![TrackSort {
                column: TrackColumn::Title,
                descending: false,
            }],
            ..TrackQuery::default()
        })
    });
    let middle = keys.len() / 2;
    let page: Vec<TrackKey> = keys.iter().skip(middle).take(50).copied().collect();
    time("track table: 50 rows", runs, || library.track_rows(&page));
    for text in [
        "a",
        "cor",
        "canción",
        "forêt řeka",
        "noël",
        "genre:jazz year:>2000",
    ] {
        time(&format!("search \"{text}\""), runs, || {
            library.search(text).map(|r| r.tracks)
        });
    }
    for text in ["canción", "e"] {
        time(&format!("track table filtered: \"{text}\""), runs, || {
            library.track_keys(&TrackQuery {
                text: text.into(),
                ..TrackQuery::default()
            })
        });
    }
    let all = TrackQuery::default();
    let mut genre = None;
    for (field, name) in [
        (FacetField::Genre, "genre"),
        (FacetField::AlbumArtist, "album artist"),
        (FacetField::Album, "album"),
        (FacetField::Artist, "artist"),
        (FacetField::Year, "year"),
        (FacetField::Format, "format"),
    ] {
        let list = time(&format!("facet values: {name}"), runs, || {
            library.facet_values(&all, &[], field)
        });
        if field == FacetField::Genre {
            genre = list.and_then(|l| l.values.first().map(|v| v.key.clone()));
        }
    }
    if let Some(key) = genre {
        let chosen = [FacetChoice {
            field: FacetField::Genre,
            keys: vec![key],
        }];
        time("facet values: artists of one genre", runs, || {
            library.facet_values(&all, &chosen, FacetField::Artist)
        });
        time("facet tracks: one genre", runs, || {
            library.facet_track_keys(&all, &chosen)
        });
    } else {
        let _ = FacetKey::Id(0);
    }
    let now = SystemTime::now();
    let since = StatisticsSince {
        month: now - Duration::from_secs(30 * 86_400),
        year: now - Duration::from_secs(365 * 86_400),
    };
    time("statistics", runs, || library.statistics(since));
    time("history: all", runs, || library.history(None));
    time("history: last 30 days", runs, || {
        library.history(Some(now - Duration::from_secs(30 * 86_400)))
    });
    time("history totals", runs, || library.history_totals());
    let rule = helikodu_service::library::parse_query_lenient("plays:>10 rating:>=3").query;
    time("smart rules: match count", runs, || {
        library.query_match_count(&rule)
    });
    let smart = time("smart playlist: create", 1, || {
        library.create_smart_playlist("Often played", &rule, None)
    });
    if let Some(smart) = smart {
        time("smart playlist: tracks", runs, || {
            library.smart_tracks(smart)
        });
        let _ = library.delete_playlist(smart);
    }
    time("missing files", runs, || services.missing_files());
    ExitCode::SUCCESS
}
