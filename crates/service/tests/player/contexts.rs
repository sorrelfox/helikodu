// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Contexts the service looks up in the library: a composer's tracks and a span of
//! years, with audiobooks left out.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use helikodu_library::{
    ArtistQuery, ArtistRole, AudioProperties, DatabaseConfig, FileStamp, Library, TrackKind,
    TrackRecord, TrackTags, YearSpan,
};
use helikodu_model::library::TrackId;
use helikodu_service::library::MusicLibrary;
use helikodu_service::player::{ContextRequest, PlayerCommand, PlayerEvent, SinkPacing};

use crate::media::{folder, next_path, player_with, until, wav_files, wav_output};

fn ended(event: &PlayerEvent) -> bool {
    matches!(event, PlayerEvent::Ended { .. })
}

fn add(
    library: &Library,
    root: &Path,
    path: &Path,
    year: i32,
    composer: Option<&str>,
    kind: TrackKind,
) -> TrackId {
    let root = library.add_root(root).unwrap();
    let mut record = TrackRecord::new(
        root,
        path,
        FileStamp {
            size: 1000,
            mtime_ns: Some(1),
            inode: None,
            quick_hash: path.as_os_str().as_encoded_bytes().to_vec(),
        },
    );
    record.kind = kind;
    record.audio = AudioProperties {
        duration: Some(Duration::from_millis(200)),
        sample_rate: Some(44_100),
        channels: Some(2),
        ..AudioProperties::default()
    };
    record.tags = TrackTags {
        title: Some(path.file_name().unwrap().to_string_lossy().into_owned()),
        album: Some(path.file_name().unwrap().to_string_lossy().into_owned()),
        artists: vec!["Okto".to_owned()],
        composers: composer.iter().map(|c| (*c).to_owned()).collect(),
        year: Some(year),
        ..TrackTags::default()
    };
    library.upsert_track(record).unwrap().track
}

#[test]
fn composers_and_years_play_without_audiobooks() {
    let dir = folder("contexts");
    let files = wav_files(&dir, 3, 200);
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    add(&library, &dir, &files[0], 2021, None, TrackKind::Music);
    add(
        &library,
        &dir,
        &files[1],
        1905,
        Some("Jean Sibelius"),
        TrackKind::Music,
    );
    add(
        &library,
        &dir,
        &files[2],
        2021,
        Some("Jean Sibelius"),
        TrackKind::Audiobook,
    );
    let music = MusicLibrary::from_library(Arc::clone(&library), ":memory:");
    let (player, events) = player_with(
        wav_output(&dir.join("out.wav")),
        SinkPacing::Freewheel,
        |config| config.library = Some(music.clone()),
    );

    player
        .send(PlayerCommand::PlayContext {
            context: ContextRequest::Years(YearSpan::year(2021)),
            title: None,
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    until(&events, ended);
    assert_eq!(player.queue().entries.len(), 1, "the audiobook is left out");

    let composer = library
        .artists(&ArtistQuery::new(ArtistRole::Composer))
        .unwrap()
        .items[0]
        .id;
    player
        .send(PlayerCommand::PlayContext {
            context: ContextRequest::Composer(composer),
            title: None,
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[1]);
    until(&events, ended);
    assert_eq!(player.queue().entries.len(), 1, "the audiobook is left out");
}
