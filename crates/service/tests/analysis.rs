// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track analysis job on a small library of generated WAV files: stopped
//! half way and resumed, the album's loudness measured as one programme in the
//! same pass, a broken file recorded once and not tried again; and the background
//! analysis of the library service, which follows its setting and changes of the
//! library, and yields to playback from the same disk.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::f64::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use helikodu_library::{
    AudioProperties, DatabaseConfig, FileStamp, Library, RootId, TrackRecord, TrackTags,
};
use helikodu_model::audio::{AudioSpec, ChannelLayout, Endianness, SampleFormat, SampleRate};
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::{EntryId, PlaybackState, TransportState};
use helikodu_model::settings::LibrarySettings;
use helikodu_service::Event;
use helikodu_service::bus::EventBus;
use helikodu_service::jobs::analysis::{AnalysisGate, AnalysisOptions, run};
use helikodu_service::library::{
    AnalysisCoverage, AnalysisOutcome, AnalysisState, BackgroundAnalysisOptions, CancelToken,
    LibraryEvent, LibraryService, LibraryServiceOptions, MusicLibrary,
};
use helikodu_service::player::PlayerEvent;

const RATE: u32 = 48_000;

fn dir(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-analysis")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a stereo 16-bit WAV file: `silence` seconds of silence, a 1 kHz sine at
/// `dbfs` for `tone` seconds, and `after` seconds of silence.
fn write_wav(path: &Path, silence: f64, dbfs: f64, tone: f64, after: f64) {
    let frames = |secs: f64| (secs * f64::from(RATE)) as usize;
    let amplitude = 10f64.powf(dbfs / 20.0) * 32_767.0;
    let mut samples: Vec<i16> = vec![0; frames(silence)];
    samples.extend((0..frames(tone)).map(|n| {
        (amplitude * (2.0 * PI * 1000.0 * n as f64 / f64::from(RATE)).sin()).round() as i16
    }));
    samples.extend(std::iter::repeat_n(0, frames(after)));
    let data_len = samples.len() * 4;
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
    for v in samples {
        b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, b).unwrap();
}

fn add(library: &Library, root: RootId, path: &Path, album: Option<&str>, n: u32) -> TrackId {
    let meta = fs::metadata(path).unwrap();
    let mut record = TrackRecord::new(
        root,
        path,
        FileStamp {
            size: meta.len(),
            mtime_ns: Some(i64::from(n) + 1),
            inode: None,
            quick_hash: path.as_os_str().as_encoded_bytes().to_vec(),
        },
    );
    record.audio = AudioProperties {
        duration: Some(Duration::from_secs(4)),
        sample_rate: Some(RATE),
        channels: Some(2),
        codec: Some("pcm_s16le".into()),
        ..AudioProperties::default()
    };
    record.tags = TrackTags {
        title: Some(format!("Track {n}")),
        artists: vec!["Band".into()],
        album: album.map(str::to_owned),
        track_no: Some(n + 1),
        ..TrackTags::default()
    };
    library.upsert_track(record).unwrap().track
}

struct Fixture {
    library: Arc<Library>,
    music: MusicLibrary,
    root: RootId,
    folder: PathBuf,
    album: Vec<TrackId>,
    lone: TrackId,
    broken: TrackId,
}

/// An album of three tracks (the first after a second of silence), a lone track
/// and a file that is not audio.
fn fixture(test: &str) -> Fixture {
    let folder = dir(test);
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    let root = library.add_root(&folder).unwrap();
    let mut album = Vec::new();
    for (n, (silence, level)) in [(1.0, -20.0), (0.0, -26.0), (0.0, -23.0)]
        .iter()
        .enumerate()
    {
        let path = folder.join(format!("{n}.wav"));
        write_wav(&path, *silence, *level, 3.0, 0.5);
        album.push(add(&library, root, &path, Some("Record"), n as u32));
    }
    let path = folder.join("lone.wav");
    write_wav(&path, 0.0, -18.0, 2.0, 0.0);
    let lone = add(&library, root, &path, None, 10);
    let path = folder.join("broken.wav");
    fs::write(&path, b"RIFF this is not a wave file at all").unwrap();
    let broken = add(&library, root, &path, None, 11);
    let music = MusicLibrary::from_library(Arc::clone(&library), ":memory:");
    Fixture {
        library,
        music,
        root,
        folder,
        album,
        lone,
        broken,
    }
}

/// A gate that stops the job before its track number `limit + 1`.
struct StopAfter {
    limit: usize,
    started: AtomicUsize,
}

impl AnalysisGate for StopAfter {
    fn before_track(&self, _path: &Path) -> bool {
        self.started.fetch_add(1, Ordering::SeqCst) < self.limit
    }

    fn between_blocks(&self) -> bool {
        true
    }
}

#[test]
fn the_job_resumes_after_a_stop_and_skips_a_broken_file() {
    let f = fixture("job");
    let options = AnalysisOptions::default();

    // Stopped after two tracks, in the middle of the album.
    let gate = StopAfter {
        limit: 2,
        started: AtomicUsize::new(0),
    };
    let first = run(&f.music, &options, &gate, |_| {});
    assert!(first.cancelled);
    assert_eq!(first.error, None);
    assert_eq!((first.analyzed, first.failed), (2, 0));
    // Their analyses are stored; the album's loudness waits for the whole album.
    assert_eq!(
        f.music.analysis_coverage().unwrap(),
        AnalysisCoverage {
            tracks: 5,
            analyzed: 2,
            failed: 0
        }
    );
    assert_eq!(f.music.measured_loudness(f.album[0]).unwrap(), None);

    // Resumed: the rest, the album decoded whole once more for its loudness.
    let mut events = Vec::new();
    let second = run(&f.music, &options, &CancelToken::new(), |e| {
        events.push(e);
    });
    assert!(!second.cancelled);
    assert_eq!(second.error, None);
    assert_eq!((second.analyzed, second.failed), (2, 1));
    assert_eq!(second.loudness_measured, 4);
    assert_eq!(
        f.music.analysis_coverage().unwrap(),
        AnalysisCoverage {
            tracks: 5,
            analyzed: 4,
            failed: 1
        }
    );
    for track in &f.album {
        let loudness = f.music.measured_loudness(*track).unwrap().unwrap();
        assert!(loudness.current);
        assert!(
            loudness.measured.album.is_some(),
            "album values for {track}"
        );
    }
    let lone = f.music.measured_loudness(f.lone).unwrap().unwrap();
    assert_eq!(lone.measured.album, None);
    let gain = lone.measured.track_gain_db().unwrap();
    // A stereo 1 kHz sine at −18 dBFS peak reads about −18 LUFS.
    assert!(gain.abs() < 0.5, "{gain}");

    // What was stored.
    let stored = f.music.track_analysis(f.album[0]).unwrap().unwrap();
    assert!(stored.current);
    let AnalysisOutcome::Analyzed(analysis) = stored.outcome else {
        panic!("not analysed: {:?}", stored.outcome);
    };
    let leading = analysis.leading_silence();
    assert!(leading.abs_diff(u64::from(RATE)) <= 480, "{leading}");
    let trailing = analysis.trailing_silence();
    assert!(trailing.abs_diff(u64::from(RATE) / 2) <= 480, "{trailing}");
    assert_eq!(analysis.envelope.len(), 5);
    let broken = f.music.track_analysis(f.broken).unwrap().unwrap();
    assert!(matches!(broken.outcome, AnalysisOutcome::Failed(_)));

    // Nothing is left, and the broken file is not tried again.
    let third = run(&f.music, &options, &CancelToken::new(), |_| {});
    assert_eq!(
        (third.analyzed, third.failed, third.cancelled),
        (0, 0, false)
    );
}

#[test]
fn the_service_analyses_in_the_background_as_the_setting_says() {
    let f = fixture("service");
    let bus = EventBus::new();
    let states = bus.subscribe_with(
        |event: &Event| match event {
            Event::Library(LibraryEvent::Analysis(progress)) => Some(progress.state),
            _ => None,
        },
        None,
    );
    let options = LibraryServiceOptions {
        analysis: BackgroundAnalysisOptions {
            start_delay: Duration::ZERO,
            settle: Duration::ZERO,
            pause_on_battery: false,
            ..BackgroundAnalysisOptions::default()
        },
        ..LibraryServiceOptions::default()
    };
    let service = LibraryService::start(f.music.clone(), bus.clone(), options).unwrap();
    let off = LibrarySettings {
        analyze_in_background: false,
        ..LibrarySettings::default()
    };
    service.set_background_analysis(&off);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(service.analysis_progress().analyzed, 0);
    assert_eq!(service.analysis_progress().state, AnalysisState::Off);

    // Playback reads from the same disk: the analysis yields to it.
    let playing = |playback: PlaybackState, transport: TransportState| {
        bus.publish(&Event::Player(PlayerEvent::State {
            playback,
            transport,
        }));
    };
    bus.publish(&Event::Player(PlayerEvent::TrackChanged {
        index: 0,
        count: 1,
        entry: EntryId::new(1),
        track: TrackRef::Library(f.lone),
        path: f.folder.join("lone.wav"),
        spec: AudioSpec::new(
            SampleFormat::S16(Endianness::Little),
            SampleRate::HZ_48000,
            ChannelLayout::STEREO,
        ),
        length: None,
    }));
    playing(PlaybackState::Playing, TransportState::Playing);

    service.set_background_analysis(&LibrarySettings::default());
    let wait_done = |service: &LibraryService| {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let progress = service.analysis_progress();
            if progress.remaining() == 0 && progress.state == AnalysisState::Idle {
                return progress;
            }
            assert!(Instant::now() < deadline, "not done: {progress:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let done = wait_done(&service);
    assert_eq!((done.total, done.analyzed, done.failed), (5, 4, 1));
    let seen: Vec<AnalysisState> = states.receiver().try_iter().collect();
    assert!(seen.contains(&AnalysisState::Yielding), "{seen:?}");
    assert!(!seen.contains(&AnalysisState::Running), "{seen:?}");
    playing(PlaybackState::Stopped, TransportState::Stopped);

    // A file changes, and the library says so: it is analysed again.
    let path = f.folder.join("1.wav");
    write_wav(&path, 2.0, -20.0, 1.0, 0.0);
    add(&f.library, f.root, &path, Some("Record"), 20);
    assert!(!f.music.track_analysis(f.album[1]).unwrap().unwrap().current);
    bus.publish(&Event::Library(LibraryEvent::Changed));
    let deadline = Instant::now() + Duration::from_secs(60);
    while !f.music.track_analysis(f.album[1]).unwrap().unwrap().current {
        assert!(Instant::now() < deadline, "not analysed again");
        std::thread::sleep(Duration::from_millis(20));
    }
    let done = wait_done(&service);
    assert_eq!((done.analyzed, done.failed), (4, 1));
    let seen: Vec<AnalysisState> = states.receiver().try_iter().collect();
    assert!(seen.contains(&AnalysisState::Running), "{seen:?}");

    service.set_background_analysis(&off);
    let deadline = Instant::now() + Duration::from_secs(10);
    while service.analysis_progress().state != AnalysisState::Off {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}
