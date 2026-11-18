// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `loudness` end to end: an album of two FLAC files made by the `ffmpeg` tool,
//! scanned by real worker processes, measured track by track, as an album and as
//! "the rest of the library", with the JSON output parsed; tags written only with
//! `--write-tags`. Without `ffmpeg` the tests print `SKIPPED:` and pass.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/media.rs"]
mod support;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use support::{cli, folder, have_ffmpeg, text, tone, worker_program};

struct Fixture {
    music: PathBuf,
    database: PathBuf,
    worker: PathBuf,
}

impl Fixture {
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

    fn json(&self, args: &[&OsStr]) -> Value {
        let (code, stdout, stderr) = self.run(args);
        assert_eq!(code, Some(0), "{args:?}: {stderr}");
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
    }

    fn file(&self, name: &str) -> PathBuf {
        self.music.join(name)
    }
}

fn os(text: &str) -> &OsStr {
    OsStr::new(text)
}

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {value}"))
}

#[test]
fn measure_tracks_albums_and_the_rest() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder("loudness", "all");
    let fx = Fixture {
        music: dir.join("Music"),
        database: dir.join("db").join("library.db"),
        worker: worker_program(),
    };
    let tags = |title: &'static str, track: &'static str| {
        vec![
            ("title", title),
            ("artist", "Band"),
            ("album", "Record"),
            ("track", track),
        ]
    };
    tone(&fx.file("1.flac"), "flac", 1000, 4, &tags("One", "1"));
    tone(&fx.file("2.flac"), "flac", 1000, 4, &tags("Two", "2"));
    let (code, _, stderr) = fx.run(&[
        os("scan"),
        fx.music.as_os_str(),
        os("--worker"),
        fx.worker.as_os_str(),
    ]);
    assert_eq!(code, Some(0), "{stderr}");
    let before = fs::read(fx.file("1.flac")).unwrap();

    // One track: track values only, the file untouched.
    let one = fx.json(&[os("loudness"), os("--json"), fx.file("1.flac").as_os_str()]);
    assert_eq!(one["total"], 1);
    assert_eq!(one["measured"], 1);
    assert_eq!(one["tags_written"], 0);
    let track = &one["tracks"][0];
    assert_eq!(track["path"], fx.file("1.flac").to_str().unwrap());
    assert_eq!(track["tags"], "not_asked");
    let loudness = &track["loudness"];
    // ffmpeg's sine peaks at 1/8 (−18.06 dBFS); made stereo, each channel gets it
    // 3 dB lower, so a 1 kHz tone measures about −21.07 LUFS.
    let lufs = number(&loudness["integrated_lufs"]);
    assert!((lufs + 21.07).abs() < 0.2, "{lufs}");
    assert!((number(&loudness["track_gain_db"]) - (-18.0 - lufs)).abs() < 1e-9);
    assert!(number(&loudness["true_peak"]) >= number(&loudness["sample_peak"]));
    assert_eq!(loudness["album"], Value::Null);
    assert_eq!(fs::read(fx.file("1.flac")).unwrap(), before);

    // The rest of the library: the second track, with the first for the album.
    let rest = fx.json(&[os("loudness"), os("--json"), os("--library")]);
    assert_eq!(rest["total"], 2);
    for track in rest["tracks"].as_array().unwrap() {
        let album = &track["loudness"]["album"];
        assert!(
            (number(&album["integrated_lufs"]) - lufs).abs() < 0.1,
            "{album}"
        );
    }
    let nothing = fx.json(&[os("loudness"), os("--json"), os("--library")]);
    assert_eq!(nothing["total"], 0);

    // The album of a track, by its number, with the tags written.
    let id = rest["tracks"][1]["track_id"].as_u64().unwrap().to_string();
    let written = fx.json(&[
        os("loudness"),
        os("--json"),
        os("--album"),
        os("--write-tags"),
        os("--worker"),
        fx.worker.as_os_str(),
        os(&id),
    ]);
    assert_eq!(written["total"], 2);
    assert_eq!(written["tags_written"], 2);
    assert!(fs::read(fx.file("1.flac")).unwrap() != before);
    let info = fx.json(&[os("info"), os("--json"), fx.file("1.flac").as_os_str()]);
    let measured = &written["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["path"] == fx.file("1.flac").to_str().unwrap())
        .unwrap()["loudness"];
    let gain = number(&info["loudness"]["track_gain_db"]);
    assert!(
        (gain - number(&measured["track_gain_db"])).abs() < 0.006,
        "{gain}"
    );
    let album_gain = number(&info["loudness"]["album_gain_db"]);
    assert!((album_gain - number(&measured["album"]["gain_db"])).abs() < 0.006);
    // Still current after the write: nothing left to measure.
    let nothing = fx.json(&[os("loudness"), os("--json"), os("--library")]);
    assert_eq!(nothing["total"], 0);
}

#[test]
fn usage_and_unknown_tracks() {
    let dir = folder("loudness", "usage");
    let database = dir.join("library.db");
    let run = |args: &[&OsStr]| {
        let mut all: Vec<OsString> = args.iter().map(|a| a.to_os_string()).collect();
        all.push("--database".into());
        all.push(database.clone().into());
        cli(&all)
    };
    // No target, or targets with --library: usage errors.
    assert_eq!(run(&[os("loudness")]).status.code(), Some(2));
    assert_eq!(
        run(&[os("loudness"), os("--library"), os("5")])
            .status
            .code(),
        Some(2)
    );
    // No library yet.
    let output = run(&[os("loudness"), os("--library")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("no library"),
        "{}",
        text(&output.stderr)
    );
}
