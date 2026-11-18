// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The converter and folder sync over generated audio in temporary folders:
//! a conversion job with tags copied, files already there left alone, and a sync
//! that copies, stays current and removes only its own files.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::f64::consts::PI;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helikodu_engine::convert::{EncodeFormat, EncodeQuality, TranscodeSettings, transcode};
use helikodu_engine::decode::DecoderRegistry;
use helikodu_library::tag_editing::{
    ReadOptions, TagChanges, TagField, WriteOptions, load_for_editing, write_tags,
};
use helikodu_library::{
    AudioProperties, CancelToken, DatabaseConfig, FileStamp, Library, TrackRecord, TrackTags,
};
use helikodu_model::library::{MediaSource, TrackId};
use helikodu_service::convert::{
    ConvertEvent, ConvertJob, ItemOutcome, JobAction, JobItem, JobOptions, Manifest, NameStyle,
    NamingPattern, PlannedAction, Preset, SyncDestination, SyncFiles, SyncSource, plan_conversion,
    plan_sync, run_sync, source_tracks,
};
use helikodu_service::library::{MusicLibrary, TrackFilter};

const RATE: u32 = 44_100;

fn dir(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-convert")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_wav(path: &Path, hz: f64) {
    let frames = RATE as usize;
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
        let v = (8000.0 * (2.0 * PI * hz * n as f64 / f64::from(RATE)).sin()).round() as i16;
        b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, b).unwrap();
}

/// A FLAC file with tags, made from a WAV file.
fn write_flac(path: &Path, title: &str) {
    let wav = path.with_extension("wav");
    write_wav(&wav, 440.0);
    let settings = TranscodeSettings {
        format: EncodeFormat::Flac,
        quality: EncodeQuality::Lossless(None),
        sample_rate: None,
        channels: None,
        bits: None,
    };
    transcode(
        &DecoderRegistry::with_default_backends(),
        &MediaSource::file(&wav),
        File::create(path).unwrap(),
        &settings,
        &mut |_, _| {},
        &|| false,
    )
    .unwrap();
    fs::remove_file(&wav).unwrap();
    let mut changes = TagChanges::new();
    changes.set_one(TagField::Title, title);
    changes.set_one(TagField::Artist, "Mari Kask");
    changes.set_one(TagField::Album, "Tidewater Rooms");
    write_tags(path, &changes, None, &WriteOptions::default()).unwrap();
}

/// A library of three FLAC tracks of one album in `music`.
fn library(music: &Path) -> (MusicLibrary, Vec<TrackId>) {
    fs::create_dir_all(music).unwrap();
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let root = library.add_root(music).unwrap();
    let mut tracks = Vec::new();
    for (n, title) in ["Tidewater", "Glass Harbour", "Kelp"].iter().enumerate() {
        let path = music.join(format!("{n}.flac"));
        write_flac(&path, title);
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
            duration: Some(Duration::from_secs(1)),
            sample_rate: Some(RATE),
            channels: Some(2),
            bit_depth: Some(16),
            codec: Some("flac".into()),
            ..AudioProperties::default()
        };
        record.tags = TrackTags {
            title: Some((*title).into()),
            artists: vec!["Mari Kask".into()],
            album_artists: vec!["Mari Kask".into()],
            album: Some("Tidewater Rooms".into()),
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

#[test]
fn a_conversion_copies_tags_and_leaves_existing_files_alone() {
    let base = dir("job");
    let (library, tracks) = library(&base.join("music"));
    let out = base.join("out");
    let sources = source_tracks(&library, &tracks).unwrap();
    let wav = Preset::builtins()
        .into_iter()
        .find(|p| p.id == "wav-cd")
        .unwrap();
    let pattern = NamingPattern::parse("{albumartist}/{album}/{track} {title}").unwrap();
    let plan = plan_conversion(&sources, &wav, &out, &pattern, NameStyle::Portable, false);
    assert!(plan.iter().all(|p| p.action == PlannedAction::Convert));
    assert_eq!(
        plan[1].relative,
        PathBuf::from("Mari Kask/Tidewater Rooms/02 Glass Harbour.wav")
    );
    // One target exists already and is left alone.
    fs::create_dir_all(plan[2].target.parent().unwrap()).unwrap();
    fs::write(&plan[2].target, b"mine").unwrap();
    let items: Vec<JobItem> = plan
        .iter()
        .map(|p| JobItem {
            track: Some(p.track.track),
            source: p.track.source.clone(),
            copy_tags: true,
            tags: Some(p.track.tags.clone()),
            target: p.target.clone(),
            action: JobAction::Convert(wav.settings()),
        })
        .collect();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&events);
    let summary = ConvertJob::start(
        Some(library.clone()),
        items,
        JobOptions {
            workers: 2,
            ..JobOptions::default()
        },
        move |e| seen.lock().unwrap().push(e),
    )
    .unwrap()
    .join();
    assert_eq!(
        (summary.written, summary.skipped, summary.failed),
        (2, 1, 0)
    );
    assert_eq!(fs::read(&plan[2].target).unwrap(), b"mine");
    let events = events.lock().unwrap();
    assert!(matches!(
        events.first(),
        Some(ConvertEvent::Started { items: 3 })
    ));
    assert!(matches!(events.last(), Some(ConvertEvent::Finished(_))));
    assert!(events.iter().any(|e| matches!(
        e,
        ConvertEvent::Done {
            index: 2,
            outcome: ItemOutcome::Skipped
        }
    )));
    let written = load_for_editing(&plan[1].target, &ReadOptions::default()).unwrap();
    assert_eq!(written.tags.title.as_deref(), Some("Glass Harbour"));
    assert_eq!(written.audio.sample_rate, Some(RATE));
    // Nothing half-written is left behind.
    let leftovers: Vec<_> = fs::read_dir(plan[1].target.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn cancelling_leaves_nothing_behind() {
    let base = dir("cancel");
    let (library, tracks) = library(&base.join("music"));
    let sources = source_tracks(&library, &tracks).unwrap();
    let out = base.join("out");
    let items: Vec<JobItem> = sources
        .iter()
        .enumerate()
        .map(|(n, t)| JobItem {
            track: Some(t.track),
            source: t.source.clone(),
            copy_tags: false,
            tags: None,
            target: out.join(format!("{n}.flac")),
            action: JobAction::Copy,
        })
        .collect();
    let cancel = CancelToken::new();
    cancel.cancel();
    let summary = helikodu_service::convert::job::run(
        Some(&library),
        &items,
        JobOptions::default(),
        &cancel,
        |_| {},
    );
    assert!(summary.cancelled);
    assert_eq!(summary.written, 0);
    assert!(fs::read_dir(&out).map_or(true, |d| d.count() == 0));
}

#[test]
fn a_sync_copies_stays_current_and_removes_only_its_own_files() {
    let base = dir("sync");
    let (library, _) = library(&base.join("music"));
    let album = library.all_tracks(TrackFilter::All).unwrap()[0]
        .album
        .unwrap();
    let phone = base.join("phone");
    fs::create_dir_all(&phone).unwrap();
    fs::write(phone.join("not mine.txt"), b"x").unwrap();
    let mut destination = SyncDestination::new("dest-1".into(), "Phone".into(), phone.clone());
    destination.sources = vec![
        SyncSource::Album { id: album.get() },
        SyncSource::Query {
            text: "title:Kelp".into(),
        },
    ];
    destination.preset = "wav-cd".into();
    destination.files = SyncFiles::ConvertLossless;
    let presets = Preset::builtins();

    let plan = plan_sync(&library, &destination, &presets).unwrap();
    assert_eq!((plan.to_convert, plan.to_copy, plan.to_remove), (3, 0, 0));
    assert_eq!(plan.playlists.len(), 1, "the search becomes a playlist");
    assert!(plan.bytes_free.is_some());
    let summary = run_sync(
        Some(&library),
        &destination,
        &plan,
        &presets,
        JobOptions::default(),
        &CancelToken::new(),
        |_| {},
    );
    assert_eq!(summary.failed, 0, "{summary:?}");
    let manifest = Manifest::load(&phone);
    assert_eq!(manifest.files.len(), 4, "three tracks and a playlist");
    let playlist = fs::read_to_string(phone.join("Playlists/title_Kelp.m3u8")).unwrap();
    assert!(
        playlist.contains("../Mari Kask/Tidewater Rooms/03 Kelp.wav"),
        "{playlist}"
    );

    let again = plan_sync(&library, &destination, &presets).unwrap();
    assert_eq!((again.to_convert, again.already_there), (0, 3));

    destination.sources.clear();
    let removal = plan_sync(&library, &destination, &presets).unwrap();
    assert_eq!(removal.to_remove, 4);
    run_sync(
        Some(&library),
        &destination,
        &removal,
        &presets,
        JobOptions::default(),
        &CancelToken::new(),
        |_| {},
    );
    assert!(phone.join("not mine.txt").exists(), "left alone");
    assert!(!phone.join("Mari Kask").exists());
    assert!(Manifest::load(&phone).files.is_empty());
}

/// A 1×1 PNG picture.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

#[test]
fn the_cover_manager_finds_and_saves_pictures_in_the_folder() {
    use helikodu_service::cover_manager::{
        CoverSave, CoverShow, WriteContext, album_covers, find_pictures, group_counts, use_picture,
    };
    let base = dir("covers");
    let music = base.join("music");
    let (library, _) = library(&music);
    let albums = album_covers(&library).unwrap();
    assert_eq!(albums.len(), 1);
    let album = &albums[0];
    assert!(album.is_in(CoverShow::NoCover));
    assert_eq!(album.facts.files.len(), 3);
    let counts = group_counts(&albums);
    assert!(counts.contains(&(CoverShow::NoCover, 1)));
    assert!(counts.contains(&(CoverShow::OnlyInFolder, 0)));

    fs::create_dir_all(music.join("booklet")).unwrap();
    fs::write(music.join("booklet/page-01.png"), PNG).unwrap();
    let found = find_pictures(album.folder().unwrap(), None);
    assert_eq!(found.len(), 1);
    let results = use_picture(
        &library,
        album,
        &fs::read(&found[0].path).unwrap(),
        CoverSave::FolderOnly,
        &WriteContext::default(),
    )
    .unwrap();
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(fs::read(music.join("cover.png")).unwrap(), PNG);
}
