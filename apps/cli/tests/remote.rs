// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The commands for the running application, against a stand-in for it that serves
//! the application's D-Bus interface on a private session bus.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::json;
use zbus::{Connection, interface};

const BUS_NAME: &str = "io.github.sorrelfox.Helikodu";
const OBJECT_PATH: &str = "/io/github/sorrelfox/Helikodu";

/// The interface's definition, which the application serves.
const INTERFACE_XML: &str = include_str!("../../../crates/desktop/src/app_api/interface.xml");

/// A `dbus-daemon` of its own, ended when dropped.
struct PrivateBus {
    daemon: Child,
    address: String,
}

impl PrivateBus {
    fn start() -> Option<Self> {
        let mut daemon = match Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(daemon) => daemon,
            Err(error) => {
                eprintln!("SKIPPED: dbus-daemon cannot be started: {error}");
                return None;
            }
        };
        let stdout = daemon.stdout.take()?;
        let mut address = String::new();
        if BufReader::new(stdout).read_line(&mut address).is_err() || address.trim().is_empty() {
            let _ = daemon.kill();
            let _ = daemon.wait();
            return None;
        }
        Some(Self {
            daemon,
            address: address.trim().to_owned(),
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// The stand-in: records each call and answers with fixed values.
struct FakeApp {
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeApp {
    fn note(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

#[interface(name = "io.github.sorrelfox.Helikodu.Control1")]
impl FakeApp {
    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }

    fn get_status(&self) -> String {
        json!({
            "api_version": 1,
            "state": "playing",
            "track": {"id": 7, "title": "Shoreline", "artist": "Mari Kask", "album": "Tidewater Rooms"},
            "position_ms": 102_000,
            "length_ms": 295_000,
            "volume": 0.5,
            "shuffle": "off",
            "repeat": "all",
            "stop_after": false,
            "queue_length": 2,
            "queue_position": 0,
            "output": {"device": "Topping D10s", "signal": "bit-perfect", "sample_rate": 96000, "bits": 24},
        })
        .to_string()
    }

    fn get_queue(&self) -> String {
        json!({
            "context": "Tidewater Rooms",
            "current": 0,
            "tracks": [
                {"title": "Shoreline", "artist": "Mari Kask", "album": "Tidewater Rooms", "length_ms": 295_000},
                {"title": "Glass Harbour", "artist": "Mari Kask", "album": "Tidewater Rooms", "length_ms": 312_000},
            ],
        })
        .to_string()
    }

    fn list_playlists(&self) -> String {
        json!([{"id": 3, "name": "Late evening", "kind": "static", "tracks": 48, "length_ms": 1}])
            .to_string()
    }

    fn next(&self) {
        self.note("Next".into());
    }

    fn pause(&self) {
        self.note("Pause".into());
    }

    fn seek(&self, offset_ms: i64) {
        self.note(format!("Seek {offset_ms}"));
    }

    fn set_position(&self, position_ms: u64) {
        self.note(format!("SetPosition {position_ms}"));
    }

    fn set_volume(&self, volume: f64) {
        self.note(format!("SetVolume {volume:.2}"));
    }

    fn set_repeat(&self, mode: String) {
        self.note(format!("SetRepeat {mode}"));
    }

    fn enqueue(&self, files: Vec<String>, next: bool) -> u32 {
        self.note(format!("Enqueue {} {next}", files.join(",")));
        u32::try_from(files.len()).unwrap()
    }

    fn play_query(&self, query: String, shuffle: bool) -> u32 {
        self.note(format!("PlayQuery {query} {shuffle}"));
        if query == "nothing" { 0 } else { 19 }
    }

    fn rate(&self, track: u64, stars: i32) -> zbus::fdo::Result<()> {
        self.note(format!("Rate {track} {stars}"));
        if track == 99 {
            return Err(zbus::fdo::Error::Failed("there is no track 99".into()));
        }
        Ok(())
    }

    fn export_playlist(&self, playlist: String, path: String) -> u32 {
        self.note(format!("ExportPlaylist {playlist} {path}"));
        48
    }
}

/// Serves the stand-in on `bus`; it serves while the connection lives.
fn serve(bus: &PrivateBus) -> (Connection, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let app = FakeApp {
        calls: calls.clone(),
    };
    let connection = zbus::block_on(async {
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(OBJECT_PATH, app)
            .name(BUS_NAME)
            .build()
            .await
            .unwrap()
    });
    (connection, calls)
}

fn cli(bus: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_helikodu-cli"))
        .args(args)
        .env("DBUS_SESSION_BUS_ADDRESS", bus)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn every_method_the_commands_call_is_in_the_interface() {
    for method in [
        "GetStatus",
        "GetQueue",
        "ListPlaylists",
        "PlayPause",
        "Play",
        "Pause",
        "Stop",
        "Next",
        "Previous",
        "Seek",
        "SetPosition",
        "SetVolume",
        "SetShuffle",
        "SetRepeat",
        "Enqueue",
        "EnqueueQuery",
        "PlayQuery",
        "Rate",
        "ExportPlaylist",
    ] {
        assert!(
            INTERFACE_XML.contains(&format!("<method name=\"{method}\"")),
            "{method} is in the interface"
        );
    }
    assert!(INTERFACE_XML.contains("<interface name=\"io.github.sorrelfox.Helikodu.Control1\">"));
}

#[test]
fn without_the_application_the_commands_end_with_code_4() {
    let output = cli("unix:path=/nonexistent/helikodu-test-bus", &["status"]);
    assert_eq!(output.status.code(), Some(4), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("Helikodu is not running"));
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let output = cli(&bus.address, &["next"]);
    assert_eq!(output.status.code(), Some(4), "{}", text(&output.stderr));
}

#[test]
fn the_commands_reach_the_running_application() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let (_connection, calls) = serve(&bus);
    let address = bus.address.as_str();

    let output = cli(address, &["status"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(stdout.starts_with("Playing: Shoreline - Mari Kask · Tidewater Rooms\n"));
    assert!(stdout.contains("Position: 1:42 / 4:55\n"));
    assert!(stdout.contains("Queue: 1 of 2\n"));

    let output = cli(address, &["status", "--json"]);
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["track"]["id"], 7);

    let output = cli(address, &["queue"]);
    assert!(text(&output.stdout).contains(">   1     4:55  Shoreline"));
    let output = cli(address, &["playlists"]);
    assert!(text(&output.stdout).contains("Late evening"));

    for args in [
        &["next"][..],
        &["pause"],
        &["seek", "+10"],
        &["seek", "-5"],
        &["seek", "1:23"],
        &["volume", "+10"],
        &["volume", "30"],
        &["repeat", "one"],
        &["play-query", "--shuffle", "genre:Ambient", "year:2019"],
        &["rate", "4"],
        &["rate", "none", "--track", "12"],
        &["export-playlist", "Late evening", "/tmp/late.m3u8"],
    ] {
        let output = cli(address, args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            text(&output.stderr)
        );
    }
    let file = std::env::current_exe().unwrap();
    let output = cli(address, &["enqueue", "--next", file.to_str().unwrap()]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "1 track added\n");

    let output = cli(address, &["volume"]);
    assert_eq!(text(&output.stdout), "50\n");

    // A query without tracks, and an error of the application.
    let output = cli(address, &["play-query", "nothing"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("no tracks found"));
    let output = cli(address, &["rate", "3", "--track", "99"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("there is no track 99"),
        "{}",
        text(&output.stderr)
    );
    // Arguments the command refuses never reach it.
    let output = cli(address, &["rate", "7"]);
    assert_eq!(output.status.code(), Some(1));
    let output = cli(address, &["seek", "soon"]);
    assert_eq!(output.status.code(), Some(1));

    let calls = calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        [
            "Next".to_owned(),
            "Pause".to_owned(),
            "Seek 10000".to_owned(),
            "Seek -5000".to_owned(),
            "SetPosition 83000".to_owned(),
            "SetVolume 0.60".to_owned(),
            "SetVolume 0.30".to_owned(),
            "SetRepeat one".to_owned(),
            "PlayQuery genre:Ambient year:2019 true".to_owned(),
            "Rate 0 4".to_owned(),
            "Rate 12 -1".to_owned(),
            "ExportPlaylist Late evening /tmp/late.m3u8".to_owned(),
            format!("Enqueue {} true", file.display()),
            "PlayQuery nothing false".to_owned(),
            "Rate 99 3".to_owned(),
        ]
    );
}
