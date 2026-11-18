// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The loudness scanner job: an album measured with its album gain and stored in the
//! library, the events it sends, cancellation, and tags written only when asked
//! (FLAC and Opus files generated with the `ffmpeg` tool; skipped without it).

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::f64::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helikodu_library::{
    AudioProperties, DatabaseConfig, FileStamp, Library, TrackRecord, TrackTags, read_file_details,
};
use helikodu_model::library::TrackId;
use helikodu_service::Event;
use helikodu_service::bus::EventBus;
use helikodu_service::jobs::loudness::{LoudnessEvent, LoudnessJob, LoudnessRequest, TagOutcome};
use helikodu_service::library::{
    LibraryEvent, LibraryService, LibraryServiceOptions, LoudnessSelection, MusicLibrary,
    ScanRequest,
};

const RATE: u32 = 48_000;

fn dir(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-loudness")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a stereo 1 kHz sine at `dbfs` (peak level) as a 16-bit WAV file.
fn write_wav(path: &Path, dbfs: f64, secs: u32) {
    let frames = (RATE * secs) as usize;
    let amplitude = 10f64.powf(dbfs / 20.0) * 32_767.0;
    let data_len = frames * 4;
    let mut b = Vec::with_capacity(44 + data_len);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    for n in 0..frames {
        let v = (amplitude * (2.0 * PI * 1000.0 * n as f64 / f64::from(RATE)).sin()).round() as i16;
        b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, b).unwrap();
}

/// A library holding WAV files at the given levels as the tracks of one album.
fn album_library(test: &str, levels: &[f64], secs: u32) -> (MusicLibrary, Vec<TrackId>) {
    let music = dir(test);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let root = library.add_root(&music).unwrap();
    let mut tracks = Vec::new();
    for (n, level) in levels.iter().enumerate() {
        let path = music.join(format!("{n}.wav"));
        write_wav(&path, *level, secs);
        let mut record = TrackRecord::new(
            root,
            &path,
            FileStamp {
                size: fs::metadata(&path).unwrap().len(),
                mtime_ns: Some(1),
                inode: None,
                quick_hash: path.as_os_str().as_encoded_bytes().to_vec(),
            },
        );
        record.audio = AudioProperties {
            duration: Some(Duration::from_secs(u64::from(secs))),
            sample_rate: Some(RATE),
            channels: Some(2),
            codec: Some("pcm_s16le".into()),
            ..AudioProperties::default()
        };
        record.tags = TrackTags {
            title: Some(format!("Track {n}")),
            artists: vec!["Band".into()],
            album: Some("Record".into()),
            track_no: Some(n as u32 + 1),
            ..TrackTags::default()
        };
        tracks.push(library.upsert_track(record).unwrap().track);
    }
    (
        MusicLibrary::from_library(Arc::new(library), ":memory:"),
        tracks,
    )
}

fn assert_near(what: &str, measured: f64, expected: f64, tolerance: f64) {
    assert!(
        (measured - expected).abs() <= tolerance,
        "{what}: {measured:.3}, expected {expected:.3} ± {tolerance}"
    );
}

#[test]
fn an_album_is_measured_and_stored_in_the_library_only() {
    let levels = [-20.0, -26.0, -23.0];
    let (library, tracks) = album_library("album", &levels, 5);
    let files: Vec<Vec<u8>> = (0..3)
        .map(|n| fs::read(dir_path("album", n)).unwrap())
        .collect();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&events);
    let mut request = LoudnessRequest::unmeasured();
    request.threads = 2;
    let job = LoudnessJob::start(&library, request, move |event| {
        seen.lock().unwrap().push(event);
    })
    .unwrap();
    let summary = job.join();
    assert_eq!(summary.error, None);
    assert_eq!((summary.total, summary.measured, summary.failed), (3, 3, 0));
    assert!(!summary.cancelled);
    assert_eq!(summary.tags_written, 0);

    let events = events.lock().unwrap();
    assert_eq!(events.first(), Some(&LoudnessEvent::Started { tracks: 3 }));
    let progress: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            LoudnessEvent::Progress {
                done,
                total: 3,
                ok: true,
                ..
            } => Some(*done),
            _ => None,
        })
        .collect();
    assert_eq!(progress, [1, 2, 3]);
    let stored: usize = events
        .iter()
        .map(|e| match e {
            LoudnessEvent::Stored(results) => results.len(),
            _ => 0,
        })
        .sum();
    assert_eq!(stored, 3);
    assert!(matches!(events.last(), Some(LoudnessEvent::Finished(s)) if s == &summary));

    // Album gain from the album as one programme: the mean of the energies.
    let energy = levels.iter().map(|l| 10f64.powf(l / 10.0)).sum::<f64>() / 3.0;
    let album_lufs = 10.0 * energy.log10();
    for (track, level) in tracks.iter().zip(levels) {
        let stored = library.measured_loudness(*track).unwrap().unwrap();
        assert!(stored.current);
        let measured = stored.measured;
        assert_near("track", measured.integrated_lufs.unwrap(), level, 0.1);
        assert_near(
            "track gain",
            measured.track_gain_db().unwrap(),
            -18.0 - level,
            0.1,
        );
        let album = measured.album.unwrap();
        assert_near("album", album.integrated_lufs.unwrap(), album_lufs, 0.1);
        assert_near(
            "album gain",
            album.gain_db().unwrap(),
            -18.0 - album_lufs,
            0.1,
        );
        assert_near("album peak", album.sample_peak, 0.1, 1e-3);
    }
    // Nothing was written into the files.
    for (n, before) in files.iter().enumerate() {
        assert_eq!(&fs::read(dir_path("album", n)).unwrap(), before);
    }
    let coverage = library.loudness_coverage().unwrap();
    assert_eq!((coverage.tracks, coverage.measured), (3, 3));
    // Nothing left to measure.
    let again = LoudnessJob::start(&library, LoudnessRequest::unmeasured(), |_| {})
        .unwrap()
        .join();
    assert_eq!(again.total, 0);
}

fn dir_path(test: &str, n: usize) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-loudness")
        .join(test)
        .join(format!("{n}.wav"))
}

#[test]
fn a_track_picked_alone_gets_no_album_values() {
    let (library, tracks) = album_library("picked", &[-20.0, -30.0], 3);
    let request = LoudnessRequest::new(LoudnessSelection::Tracks(vec![tracks[1]]));
    let summary = LoudnessJob::start(&library, request, |_| {})
        .unwrap()
        .join();
    assert_eq!(summary.measured, 1);
    let measured = library
        .measured_loudness(tracks[1])
        .unwrap()
        .unwrap()
        .measured;
    assert_eq!(measured.album, None);
    assert_near("track", measured.integrated_lufs.unwrap(), -30.0, 0.1);
    assert_eq!(library.measured_loudness(tracks[0]).unwrap(), None);
}

#[test]
fn a_cancelled_job_stops_and_keeps_no_half_album() {
    let (library, tracks) = album_library("cancel", &[-20.0; 6], 30);
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&events);
    let mut request = LoudnessRequest::unmeasured();
    request.threads = 1;
    let job = LoudnessJob::start(&library, request, move |event| {
        seen.lock().unwrap().push(event);
    })
    .unwrap();
    job.cancel();
    let summary = job.join();
    assert!(summary.cancelled, "{summary:?}");
    assert_eq!(summary.total, 6);
    assert!(summary.measured < 6);
    // The album is stored together or not at all.
    for track in tracks {
        assert_eq!(library.measured_loudness(track).unwrap(), None);
    }
    assert!(matches!(
        events.lock().unwrap().last(),
        Some(LoudnessEvent::Finished(s)) if s.cancelled
    ));
}

#[test]
fn the_library_service_announces_the_measurement() {
    let (library, tracks) = album_library("service", &[-23.0], 3);
    let bus = EventBus::new();
    let events = bus.subscribe_with(
        |event: &Event| match event {
            Event::Library(LibraryEvent::Loudness(event)) => Some(event.clone()),
            _ => None,
        },
        None,
    );
    let service =
        LibraryService::start(library.clone(), bus, LibraryServiceOptions::default()).unwrap();
    service
        .measure_loudness(LoudnessRequest::unmeasured())
        .unwrap();
    let finished = loop {
        if let LoudnessEvent::Finished(summary) =
            events.recv_timeout(Duration::from_secs(30)).unwrap()
        {
            break summary;
        }
    };
    assert_eq!(finished.measured, 1);
    service.stop_loudness();
    assert!(library.measured_loudness(tracks[0]).unwrap().is_some());
}

fn have_ffmpeg_encoder(name: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains(name))
}

fn encode(input: &Path, output: &Path, codec: &str) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-metadata", "album=Encoded", "-metadata", "artist=Band"])
        .args(["-c:a", codec])
        .arg(output)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn tags_are_written_only_when_asked() {
    if !have_ffmpeg_encoder("flac") {
        eprintln!("SKIPPED: ffmpeg with a FLAC encoder is not installed");
        return;
    }
    let opus = have_ffmpeg_encoder("libopus");
    let base = dir("tags");
    let music = base.join("music");
    fs::create_dir_all(&music).unwrap();
    let wav = base.join("source.wav");
    write_wav(&wav, -14.0, 4);
    let flac = music.join("one.flac");
    encode(&wav, &flac, "flac");
    let ogg = music.join("two.opus");
    if opus {
        encode(&wav, &ogg, "libopus");
    } else {
        eprintln!("SKIPPED (Opus part): ffmpeg has no libopus encoder");
    }

    let worker = worker_program();
    let library = MusicLibrary::open(&base.join("library.db")).unwrap();
    library
        .scan(ScanRequest {
            folders: vec![music.clone()],
            worker_program: Some(worker.clone()),
            ..ScanRequest::default()
        })
        .unwrap()
        .join()
        .unwrap();
    let flac_track = library.track_at(&flac).unwrap().unwrap();

    // Off: the library only.
    let before = fs::read(&flac).unwrap();
    let mut request = LoudnessRequest::unmeasured();
    request.worker_program = Some(worker.clone());
    let summary = LoudnessJob::start(&library, request.clone(), |_| {})
        .unwrap()
        .join();
    assert_eq!(summary.error, None);
    assert!(
        summary
            .results
            .iter()
            .all(|r| r.tags == TagOutcome::NotAsked)
    );
    assert_eq!(fs::read(&flac).unwrap(), before);
    assert!(read_file_details(&flac).unwrap().loudness.is_unknown());

    // On: written, and the measurement stays current although the file changed.
    request.selection =
        LoudnessSelection::Tracks(summary.results.iter().map(|r| r.track).collect());
    request.write_tags = true;
    let summary = LoudnessJob::start(&library, request, |_| {})
        .unwrap()
        .join();
    assert_eq!(summary.error, None);
    let expected = if opus { 2 } else { 1 };
    assert_eq!(summary.tags_written, expected, "{summary:?}");
    let stored = library.measured_loudness(flac_track).unwrap().unwrap();
    assert!(stored.current, "restamped after the write");
    let measured = stored.measured;
    let tags = read_file_details(&flac).unwrap().loudness;
    assert_near(
        "written gain",
        tags.track_gain_db.unwrap(),
        measured.track_gain_db().unwrap(),
        0.005,
    );
    assert_near(
        "written peak",
        tags.track_peak.unwrap(),
        measured.true_peak,
        1e-6,
    );
    // Both files are the album "Encoded": album values were written too.
    if opus {
        assert_near(
            "written album gain",
            tags.album_gain_db.unwrap(),
            measured.album.unwrap().gain_db().unwrap(),
            0.005,
        );
        let ogg_track = library.track_at(&ogg).unwrap().unwrap();
        let ogg_measured = library
            .measured_loudness(ogg_track)
            .unwrap()
            .unwrap()
            .measured;
        let ogg_tags = read_file_details(&ogg).unwrap().loudness;
        assert_near(
            "Opus gain",
            ogg_tags.track_gain_db.unwrap(),
            ogg_measured.track_gain_db().unwrap(),
            0.005,
        );
        // Opus gets R128 gains (to 1/256 dB), which carry no peak.
        assert_eq!(ogg_tags.track_peak, None, "R128, not ReplayGain");
        assert_near(
            "Opus album gain",
            ogg_tags.album_gain_db.unwrap(),
            ogg_measured.album.unwrap().gain_db().unwrap(),
            0.005,
        );
    }
    let unmeasured = LoudnessRequest::unmeasured().selection;
    assert!(library.loudness_targets(&unmeasured).unwrap().is_empty());
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
