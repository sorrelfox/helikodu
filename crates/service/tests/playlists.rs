// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playlists through the service: folders and smart playlists on the library, and the
//! playlist files of the library folders picked up when the setting is switched on.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use helikodu_library::{DatabaseConfig, Library};
use helikodu_model::settings::LibrarySettings;
use helikodu_service::Event;
use helikodu_service::bus::EventBus;
use helikodu_service::library::{
    LibraryEvent, LibraryService, LibraryServiceOptions, MusicLibrary, PinTarget, PlaylistType,
    parse_query,
};

fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-playlists")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("music")).unwrap();
    dir
}

fn library(dir: &Path) -> MusicLibrary {
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    library.add_root(dir.join("music")).unwrap();
    MusicLibrary::from_library(Arc::new(library), ":memory:")
}

#[test]
fn folders_smart_playlists_and_pins() {
    let dir = folder("tree");
    let music = library(&dir);
    let folder = music.create_playlist_folder("Moods", None).unwrap();
    let smart = music
        .create_smart_playlist("Old", &parse_query("year<1970").unwrap(), Some(folder))
        .unwrap();
    let list = music.create_playlist("Plain").unwrap();
    music.pin(PinTarget::Playlist(smart)).unwrap();
    let tree = music.playlist_tree().unwrap();
    let kinds: Vec<(PlaylistType, usize)> =
        tree.iter().map(|n| (n.summary.kind, n.depth)).collect();
    assert_eq!(
        kinds,
        [
            // Favourites, built in, first.
            (PlaylistType::Static, 0),
            (PlaylistType::Folder, 0),
            (PlaylistType::Smart, 1),
            (PlaylistType::Static, 0),
        ]
    );
    assert!(tree[0].summary.builtin);
    assert!(music.smart_playlist_tracks(smart).unwrap().is_empty());
    assert_eq!(
        music
            .query_match_count(&parse_query("year<1970").unwrap())
            .unwrap(),
        (0, 0)
    );
    assert_eq!(music.pins().unwrap()[0].name, "Old");
    music.set_playlist_locked(list, true).unwrap();
    assert!(music.playlist(list).unwrap().unwrap().locked);
}

#[test]
fn switching_the_setting_on_picks_up_playlist_files() {
    let dir = folder("found");
    let music = library(&dir);
    fs::write(dir.join("music/Road trip.m3u8"), "a.flac\nb.flac\n").unwrap();
    let bus = EventBus::new();
    let events = bus.subscribe_with(
        |event: &Event| match event {
            Event::Library(LibraryEvent::Changed) => Some(()),
            _ => None,
        },
        None,
    );
    let service =
        LibraryService::start(music.clone(), bus, LibraryServiceOptions::default()).unwrap();
    service.set_import_playlists(&LibrarySettings::default());
    // Only the built-in Favourites.
    assert_eq!(music.playlist_tree().unwrap().len(), 1);
    service.set_import_playlists(&LibrarySettings {
        import_playlists: true,
        ..LibrarySettings::default()
    });
    events.recv_timeout(Duration::from_secs(30)).unwrap();
    let tree = music.playlist_tree().unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[1].summary.name, "Road trip");
    assert_eq!(tree[1].summary.items, 2);
    assert_eq!(tree[1].summary.dead, 2);
}
