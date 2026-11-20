// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Play counts written into the files by the library service itself, over FLAC
//! files generated with `ffmpeg`: after a quiet spell once plays were recorded,
//! not at all while the setting is off, at once when it is switched on, after
//! the start for what was left, never into the file the player has open, and
//! within the close limit when the service ends. No sound is played: the
//! player's events are published on the bus by the test.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use helikodu_model::audio::{AudioSpec, ChannelLayout, Endianness, SampleFormat, SampleRate};
use helikodu_model::library::{MediaSource, TrackRef};
use helikodu_model::player::EntryId;
use helikodu_model::settings::{Settings, SettingsGroup};
use helikodu_service::Event;
use helikodu_service::bus::EventBus;
use helikodu_service::library::tag_editing::{
    FieldKey, ReadOptions, field_values, load_for_editing,
};
use helikodu_service::library::{
    LibraryService, LibraryServiceOptions, Listen, ListenEnd, MusicLibrary, PlayRules, ScanRequest,
    StatsWriteOptions,
};
use helikodu_service::player::{EndReason, PlayerEvent};
use helikodu_service::settings::SettingsEvent;

const HOUR: Duration = Duration::from_secs(3600);

fn have_ffmpeg() -> bool {
    let found = Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("SKIPPED: ffmpeg is not installed");
    }
    found
}

struct Fixture {
    music: MusicLibrary,
    folder: PathBuf,
    files: Vec<PathBuf>,
    worker: PathBuf,
}

/// `count` one-second FLAC files, scanned into a new library.
fn fixture(test: &str, count: usize) -> Fixture {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-stats-writes")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    let folder = dir.join("music");
    fs::create_dir_all(&folder).unwrap();
    let mut files = Vec::new();
    for n in 0..count {
        let file = folder.join(format!("{n}.flac"));
        let output = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
            .arg("-i")
            .arg(format!(
                "sine=frequency={}:duration=1:sample_rate=44100",
                440 + n * 110
            ))
            .args(["-metadata", &format!("title=Track {n}"), "-c:a", "flac"])
            .arg(&file)
            .output()
            .unwrap();
        assert!(output.status.success());
        files.push(file);
    }
    let worker = worker_program();
    let music = MusicLibrary::open(&dir.join("library.db")).unwrap();
    music
        .scan(ScanRequest {
            folders: vec![folder.clone()],
            worker_program: Some(worker.clone()),
            ..ScanRequest::default()
        })
        .unwrap()
        .join()
        .unwrap();
    Fixture {
        music,
        folder,
        files,
        worker,
    }
}

impl Fixture {
    fn service(&self, bus: &EventBus<Event>, writes: StatsWriteOptions) -> LibraryService {
        LibraryService::start(
            self.music.clone(),
            bus.clone(),
            LibraryServiceOptions {
                worker_program: Some(self.worker.clone()),
                stats_writes: writes,
                ..LibraryServiceOptions::default()
            },
        )
        .unwrap()
    }

    fn settings(&self, on: bool) -> Settings {
        let mut settings = Settings::default();
        settings.library.folders = vec![self.folder.clone()];
        settings.advanced.write_play_counts_to_tags = on;
        settings
    }

    /// Records a play of file `n`, without telling anyone.
    fn record(&self, n: usize) -> PlayerEvent {
        let track = self.music.track_at(&self.files[n]).unwrap().unwrap();
        let listen = Listen {
            started_at: SystemTime::now(),
            played: Duration::from_secs(1),
            end: ListenEnd::Finished,
            position: None,
            context: None,
        };
        let outcome = self
            .music
            .record_listen(track, listen, PlayRules::default())
            .unwrap();
        PlayerEvent::ListenRecorded { track, outcome }
    }

    /// Records a play of file `n` as the player does, announcing it.
    fn play(&self, bus: &EventBus<Event>, n: usize) {
        let event = self.record(n);
        bus.publish(&Event::Player(event));
    }

    /// The play count in file `n`.
    fn in_file(&self, n: usize) -> Option<String> {
        let opened = load_for_editing(&self.files[n], &ReadOptions::default()).unwrap();
        field_values(&opened.tags, &FieldKey::Custom("FMPS_PLAYCOUNT".into()))
            .into_iter()
            .next()
    }

    fn wait_for(&self, n: usize, count: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while self.in_file(n).as_deref() != Some(count) {
            assert!(
                Instant::now() < deadline,
                "file {n} holds {:?}, not {count}",
                self.in_file(n)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn pending(&self) -> usize {
        self.music
            .pending_stats_writes(&self.settings(true).advanced)
            .unwrap()
    }
}

fn opened(bus: &EventBus<Event>, path: &Path) {
    bus.publish(&Event::Player(PlayerEvent::TrackChanged {
        index: 0,
        count: 1,
        entry: EntryId::new(1),
        track: TrackRef::File(MediaSource::file(path)),
        path: path.to_path_buf(),
        spec: AudioSpec::new(
            SampleFormat::S16(Endianness::Little),
            SampleRate::HZ_44100,
            ChannelLayout::STEREO,
        ),
        length: None,
    }));
}

fn writes(start_delay: Duration, quiet: Duration) -> StatsWriteOptions {
    StatsWriteOptions {
        start_delay,
        quiet,
        ..StatsWriteOptions::default()
    }
}

#[test]
fn recorded_plays_reach_the_files_after_a_quiet_spell() {
    if !have_ffmpeg() {
        return;
    }
    let f = fixture("quiet", 1);
    let bus = EventBus::new();
    let service = f.service(&bus, writes(HOUR, Duration::from_millis(500)));
    service.set_stats_writes(&f.settings(true));
    f.play(&bus, 0);
    f.play(&bus, 0);
    // Not at once: only after the quiet spell.
    assert_eq!(f.in_file(0), None);
    f.wait_for(0, "2");
    // Recorded as written once the file has been read again.
    let deadline = Instant::now() + Duration::from_secs(30);
    while f.pending() != 0 {
        assert!(Instant::now() < deadline, "not recorded as written");
        std::thread::sleep(Duration::from_millis(20));
    }
    f.play(&bus, 0);
    f.wait_for(0, "3");
}

#[test]
fn nothing_is_written_while_the_setting_is_off() {
    if !have_ffmpeg() {
        return;
    }
    let f = fixture("off", 1);
    let bus = EventBus::new();
    let service = f.service(&bus, writes(Duration::ZERO, Duration::from_millis(50)));
    service.set_stats_writes(&f.settings(false));
    f.play(&bus, 0);
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(f.in_file(0), None);
    assert_eq!(f.pending(), 1);

    // Switched on: written at once, without a new play.
    bus.publish(&Event::Settings(SettingsEvent::Changed {
        groups: vec![SettingsGroup::Advanced],
        settings: Arc::new(f.settings(true)),
    }));
    f.wait_for(0, "1");
    // And the end of the service writes nothing while it is off again.
    service.set_stats_writes(&f.settings(false));
    f.record(0);
    drop(service);
    assert_eq!(f.in_file(0).as_deref(), Some("1"));
}

#[test]
fn the_open_file_waits_until_it_is_closed() {
    if !have_ffmpeg() {
        return;
    }
    let f = fixture("playing", 2);
    let bus = EventBus::new();
    let service = f.service(&bus, writes(HOUR, Duration::from_millis(200)));
    service.set_stats_writes(&f.settings(true));
    opened(&bus, &f.files[1]);
    f.play(&bus, 0);
    f.play(&bus, 1);
    f.wait_for(0, "1");
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(f.in_file(1), None, "the open file is left alone");
    assert_eq!(f.pending(), 1);

    // Another track plays: the first file is written.
    opened(&bus, &f.files[0]);
    f.wait_for(1, "1");
    // Playback ends: the second is no longer open either.
    f.play(&bus, 0);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(f.in_file(0).as_deref(), Some("1"));
    bus.publish(&Event::Player(PlayerEvent::Ended {
        reason: EndReason::Stopped,
        played: 2,
        failed: 0,
    }));
    f.wait_for(0, "2");
    drop(service);
}

#[test]
fn the_start_writes_what_was_left_and_the_end_what_is_new() {
    if !have_ffmpeg() {
        return;
    }
    let f = fixture("start-and-end", 2);
    let bus = EventBus::new();
    f.record(0);
    let service = f.service(&bus, writes(Duration::ZERO, HOUR));
    service.set_stats_writes(&f.settings(true));
    f.wait_for(0, "1");

    // Played just before the end, with the file still open: written at the end.
    opened(&bus, &f.files[1]);
    f.play(&bus, 1);
    f.play(&bus, 0);
    let started = Instant::now();
    drop(service);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(f.in_file(0).as_deref(), Some("2"));
    assert_eq!(f.in_file(1).as_deref(), Some("1"));
    assert_eq!(f.pending(), 0);
}

#[test]
fn the_end_does_not_wait_beyond_its_limit() {
    if !have_ffmpeg() {
        return;
    }
    let f = fixture("limit", 6);
    let bus = EventBus::new();
    let service = f.service(
        &bus,
        StatsWriteOptions {
            close_limit: Duration::ZERO,
            ..writes(HOUR, HOUR)
        },
    );
    service.set_stats_writes(&f.settings(true));
    for n in 0..6 {
        f.play(&bus, n);
    }
    let started = Instant::now();
    drop(service);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    // What did not fit waits for the next start.
    let left = f.pending();
    assert!(left > 0, "everything was written");
    let bus = EventBus::new();
    let service = f.service(&bus, writes(Duration::ZERO, HOUR));
    service.set_stats_writes(&f.settings(true));
    for n in 0..6 {
        f.wait_for(n, "1");
    }
}

/// The `helikodu-scan-worker` program of this build, built on first use.
fn worker_program() -> PathBuf {
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let release = !cfg!(debug_assertions);
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args([
            "build",
            "--quiet",
            "-p",
            "helikodu-scan-worker",
            "--target-dir",
        ])
        .arg(&target)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    if release {
        command.arg("--release");
    }
    assert!(
        command.status().unwrap().success(),
        "building the scan worker"
    );
    target
        .join(if release { "release" } else { "debug" })
        .join("helikodu-scan-worker")
}
