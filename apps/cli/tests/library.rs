// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `scan`, `search`, `info` and `stats` end to end: a small library made by the
//! `ffmpeg` tool (with a file name that is not UTF-8 and an excluded folder),
//! scanned by real worker processes into a temporary database, and the JSON output
//! parsed. Without `ffmpeg` the tests print `SKIPPED:` and pass.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/media.rs"]
mod support;

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use support::{cli, folder, have_ffmpeg, text, tone, worker_program};

/// The name of the file whose name is Latin-1, not UTF-8: "Café.flac".
const LATIN1_NAME: &[u8] = b"Caf\xe9.flac";

struct Fixture {
    dir: PathBuf,
    music: PathBuf,
    database: PathBuf,
}

impl Fixture {
    /// A library folder with two tracks of one album, a track in a file whose name
    /// is not UTF-8, a folder to exclude and a text file.
    fn new(test: &str) -> Self {
        let dir = folder("library", test);
        let music = dir.join("Music");
        let album = music.join("Aurora Test").join("Dawn");
        let tags = |title: &'static str, track: &'static str| {
            vec![
                ("title", title),
                ("artist", "Aurora Test"),
                ("album_artist", "Aurora Test"),
                ("album", "Dawn"),
                ("track", track),
                ("genre", "Ambient"),
                ("date", "2024"),
                ("REPLAYGAIN_TRACK_GAIN", "-4.50 dB"),
                ("REPLAYGAIN_TRACK_PEAK", "0.250000"),
            ]
        };
        tone(
            &album.join("01 Morning.flac"),
            "flac",
            440,
            1,
            &tags("Morning Light", "1"),
        );
        tone(
            &album.join("02 Noon.flac"),
            "flac",
            550,
            1,
            &tags("Noon Shadow", "2"),
        );
        tone(
            &music.join(OsStr::from_bytes(LATIN1_NAME)),
            "flac",
            660,
            1,
            &[("title", "Café Song"), ("artist", "Zoë")],
        );
        tone(
            &music.join("Skip").join("hidden.flac"),
            "flac",
            330,
            1,
            &[("title", "Skipped Track")],
        );
        std::fs::write(music.join("notes.txt"), "not audio").unwrap();
        Self {
            database: dir.join("db").join("library.db"),
            dir,
            music,
        }
    }

    /// Runs the program with `args` and `--database`, and returns the exit code,
    /// standard output and standard error.
    fn run(&self, args: &[&OsStr]) -> (Option<i32>, String, String) {
        let mut all: Vec<OsString> = args.iter().map(|a| a.to_os_string()).collect();
        all.push("--database".into());
        all.push(self.database.clone().into());
        let output = cli(&all);
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr),
        )
    }

    /// Runs a command that must succeed and print JSON, and returns the JSON.
    fn json(&self, args: &[&OsStr]) -> Value {
        let (code, stdout, stderr) = self.run(args);
        assert_eq!(code, Some(0), "{args:?}: {stderr}");
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
    }
}

fn os(text: &str) -> &OsStr {
    OsStr::new(text)
}

/// The bytes of a `path_bytes` array.
fn bytes_of(value: &Value) -> Vec<u8> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
        .collect()
}

#[test]
fn scan_search_info_and_stats() {
    if !have_ffmpeg() {
        return;
    }
    let fx = Fixture::new("all");
    let worker = worker_program();
    let beside = Path::new(env!("CARGO_BIN_EXE_helikodu-cli")).parent() == worker.parent();

    // The first scan, with the worker found beside the program when it is there.
    let mut args = vec![
        os("scan"),
        fx.music.as_os_str(),
        os("--exclude"),
        os("Skip"),
        os("--workers"),
        os("2"),
        os("--json"),
    ];
    if !beside {
        args.extend([os("--worker"), worker.as_os_str()]);
    }
    let (code, stdout, stderr) = fx.run(&args);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("Scanning"), "{stderr}");
    let summary: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(summary["roots"], 1);
    assert_eq!(summary["found"], 3, "{summary}");
    assert_eq!(summary["read"], 3);
    assert_eq!(summary["added"], 3);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["failures"], Value::Array(Vec::new()));
    assert!(summary["elapsed_ms"].is_u64());

    // A second scan of the library's own folders, with an explicit worker: nothing
    // changed. The text summary goes to standard output.
    let (code, stdout, stderr) = fx.run(&[
        os("scan"),
        os("--exclude"),
        os("Skip"),
        os("--worker"),
        worker.as_os_str(),
    ]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains("3 unchanged"), "{stdout}");
    assert!(stdout.contains("0 added"), "{stdout}");

    // Search.
    let page = fx.json(&[os("search"), os("morning"), os("--json")]);
    assert_eq!(page["total"], 1);
    assert_eq!(page["query"], "morning");
    let hit = &page["hits"][0];
    assert_eq!(hit["title"], "Morning Light");
    assert_eq!(hit["artist"], "Aurora Test");
    assert_eq!(hit["album"], "Dawn");
    assert_eq!(hit["rank"], "prefix");
    assert_eq!(hit["duration_ms"], 1000);
    assert!(hit["path"].as_str().unwrap().ends_with("01 Morning.flac"));
    assert!(hit.get("path_bytes").is_none());
    let morning = hit["track_id"].as_u64().unwrap();

    let page = fx.json(&[os("search"), os("cafe"), os("--json")]);
    assert_eq!(page["total"], 1, "{page}");
    let cafe = &page["hits"][0];
    assert_eq!(cafe["title"], "Café Song");
    let cafe_path = fx.music.join(OsStr::from_bytes(LATIN1_NAME));
    assert_eq!(
        bytes_of(&cafe["path_bytes"]),
        cafe_path.as_os_str().as_bytes()
    );
    assert!(cafe["path"].as_str().unwrap().ends_with("Caf\u{FFFD}.flac"));

    let page = fx.json(&[os("search"), os("skipped"), os("--json")]);
    assert_eq!(page["total"], 0);
    assert_eq!(page["hits"], Value::Array(Vec::new()));

    // Paging: the album has two tracks.
    let page = fx.json(&[
        os("search"),
        os("aurora"),
        os("--limit"),
        os("1"),
        os("--offset"),
        os("1"),
        os("--json"),
    ]);
    assert_eq!(page["total"], 2);
    assert_eq!(page["hits"].as_array().unwrap().len(), 1);
    assert_eq!(page["offset"], 1);
    assert_eq!(page["limit"], 1);

    let (code, stdout, _) = fx.run(&[os("search"), os("Noon"), os("shadow")]);
    assert_eq!(code, Some(0));
    assert!(
        stdout.contains("Aurora Test - Noon Shadow [Dawn]  0:01"),
        "{stdout}"
    );
    assert!(stdout.contains("Tracks 1 to 1 of 1."), "{stdout}");

    // Info by track number: from the library, with the file's loudness tags.
    let info = fx.json(&[os("info"), os(&morning.to_string()), os("--json")]);
    assert_eq!(info["origin"], "library");
    assert_eq!(info["sub_index"], 0);
    assert_eq!(info["tags"]["title"], "Morning Light");
    assert_eq!(info["tags"]["artists"][0], "Aurora Test");
    assert_eq!(info["tags"]["album_artists"][0], "Aurora Test");
    assert_eq!(info["tags"]["genres"][0], "Ambient");
    assert_eq!(info["tags"]["track_number"], 1);
    assert_eq!(info["tags"]["year"], 2024);
    assert_eq!(info["audio"]["codec"], "flac");
    assert_eq!(info["audio"]["sample_rate"], 48_000);
    assert_eq!(info["audio"]["channels"], 2);
    assert_eq!(info["loudness"]["track_gain_db"], -4.5);
    assert_eq!(info["loudness"]["track_peak"], 0.25);
    assert_eq!(info["loudness"]["album_gain_db"], Value::Null);
    assert_eq!(info["file_error"], Value::Null);
    let entry = &info["library"];
    assert_eq!(entry["track_id"], morning);
    assert!(entry["identity_id"].is_i64());
    assert_eq!(entry["kind"], "music");
    assert!(entry["added_at"].as_i64().unwrap() > 1_700_000_000);
    assert_eq!(entry["missing_since"], Value::Null);
    assert_eq!(entry["statistics"]["play_count"], 0);
    assert_eq!(entry["statistics"]["rating"], Value::Null);

    // Info by file: the file whose name is not UTF-8, found in the library.
    let info = fx.json(&[os("info"), cafe_path.as_os_str(), os("--json")]);
    assert_eq!(info["origin"], "file");
    assert_eq!(
        bytes_of(&info["path_bytes"]),
        cafe_path.as_os_str().as_bytes()
    );
    assert_eq!(info["tags"]["title"], "Café Song");
    assert_eq!(info["tags"]["artists"][0], "Zoë");
    assert_eq!(info["library"]["track_id"], cafe["track_id"]);

    // A file outside the library, given by a relative path.
    let outside = fx.dir.join("outside.flac");
    tone(&outside, "flac", 440, 1, &[("title", "Elsewhere")]);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_helikodu-cli"))
        .args([
            os("info"),
            os("outside.flac"),
            os("--json"),
            os("--database"),
        ])
        .arg(&fx.database)
        .current_dir(&fx.dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let info: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["tags"]["title"], "Elsewhere");
    assert_eq!(info["library"], Value::Null);
    assert_eq!(info["path"], outside.to_str().unwrap());

    let (code, stdout, _) = fx.run(&[os("info"), os(&morning.to_string())]);
    assert_eq!(code, Some(0));
    assert!(stdout.contains("Morning Light"), "{stdout}");
    assert!(stdout.contains("-4.50 dB, peak 0.250000"), "{stdout}");

    // Statistics. The program records no plays; the `sqlite3` tool gives one track
    // two plays when it is installed.
    let played = Command::new("sqlite3")
        .arg(&fx.database)
        .arg(format!(
            "INSERT INTO stats (identity_id, play_count, last_played)
             SELECT identity_id, 2, 1800000000 FROM tracks WHERE id = {morning}"
        ))
        .status()
        .is_ok_and(|status| status.success());
    if !played {
        eprintln!("SKIPPED: the play counts need the sqlite3 tool");
    }
    let stats = fx.json(&[os("stats"), os("--json")]);
    let roots = stats["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0]["path"], fx.music.to_str().unwrap());
    let totals = &stats["totals"];
    assert_eq!(totals["roots"], 1);
    assert_eq!(totals["tracks"], 3);
    assert_eq!(totals["files"], 3);
    assert_eq!(totals["albums"], 1);
    assert_eq!(totals["artists"], 2);
    assert_eq!(totals["genres"], 1);
    assert_eq!(totals["duration_ms"], 3000);
    assert!(totals["size_bytes"].as_u64().unwrap() > 0);
    assert_eq!(totals["missing_files"], 0);
    assert_eq!(totals["unreadable_files"], 0);
    assert_eq!(totals["rated_tracks"], 0);
    let top = stats["most_played"].as_array().unwrap();
    if played {
        assert_eq!(totals["plays"], 2);
        assert_eq!(totals["played_tracks"], 1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0]["track_id"], morning);
        assert_eq!(top[0]["title"], "Morning Light");
        assert_eq!(top[0]["artist"], "Aurora Test");
        assert_eq!(top[0]["album"], "Dawn");
        assert_eq!(top[0]["play_count"], 2);
        assert_eq!(top[0]["last_played"], 1_800_000_000);
        let info = fx.json(&[os("info"), os(&morning.to_string()), os("--json")]);
        assert_eq!(info["library"]["statistics"]["play_count"], 2);
        assert_eq!(info["library"]["statistics"]["last_played"], 1_800_000_000);
    } else {
        assert_eq!(totals["plays"], 0);
        assert!(top.is_empty());
    }

    let (code, stdout, _) = fx.run(&[os("stats")]);
    assert_eq!(code, Some(0));
    assert!(stdout.contains("3 tracks in 3 files"), "{stdout}");
    assert!(!played || stdout.contains("Morning Light"), "{stdout}");
}

#[test]
fn library_errors() {
    if !have_ffmpeg() {
        return;
    }
    let fx = Fixture::new("errors");
    // No library yet: search and stats fail, info on a track number too.
    for args in [
        &[os("search"), os("x")][..],
        &[os("stats")],
        &[os("info"), os("12")],
    ] {
        let (code, _, stderr) = fx.run(args);
        assert_eq!(code, Some(1), "{args:?}");
        assert!(stderr.contains("scan a folder first"), "{stderr}");
    }
    // A scan without folders of a library that has none.
    let (code, _, stderr) = fx.run(&[os("scan")]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("no folders"), "{stderr}");
    // A worker program that does not exist.
    let (code, _, stderr) = fx.run(&[
        os("scan"),
        fx.music.as_os_str(),
        os("--worker"),
        fx.dir.join("no-such-worker").as_os_str(),
    ]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("worker"), "{stderr}");
    // A folder that does not exist is reported, and the exit code says so.
    let (code, stdout, stderr) = fx.run(&[
        os("scan"),
        fx.dir.join("gone").as_os_str(),
        os("--worker"),
        worker_program().as_os_str(),
        os("--json"),
    ]);
    assert_eq!(code, Some(1), "{stderr}");
    let summary: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(summary["unavailable_roots"], 1);
    // An unknown track and a file that is not audio.
    let (code, _, stderr) = fx.run(&[os("info"), os("999999")]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("no track 999999"), "{stderr}");
    let (code, _, stderr) = fx.run(&[os("info"), fx.music.join("notes.txt").as_os_str()]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("cannot read"), "{stderr}");
    // Usage errors.
    for args in [
        &[os("search")][..],
        &[os("search"), os("x"), os("--limit"), os("0")],
        &[os("search"), os("x"), os("--limit"), os("1001")],
        &[os("scan"), os("--workers"), os("0")],
        &[os("info")],
        &[os("stats"), os("--top"), os("-1")],
    ] {
        let (code, _, _) = fx.run(args);
        assert_eq!(code, Some(2), "{args:?}");
    }
}
