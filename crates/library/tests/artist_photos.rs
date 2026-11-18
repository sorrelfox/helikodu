// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Artist photos through `Library`: a chosen photo before the folder's
//! `artist.*`, before none; removing the chosen one; the copy kept by the library;
//! and both surviving a reopened database.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use helikodu_library::{
    ArtistPhotoError, ArtistQuery, ArtistRole, CoverHash, CoverLocation, DatabaseConfig, Library,
    PersonId, RootId,
};
use helikodu_meta::covers::RgbaPicture;
use support::record;

fn png(rgb: [u8; 3]) -> Vec<u8> {
    let pixels = [rgb[0], rgb[1], rgb[2], 255].repeat(16 * 16);
    RgbaPicture::new(16, 16, pixels)
        .unwrap()
        .encode_png()
        .unwrap()
}

/// A new, empty folder for one test.
fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("artist-photos")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A library at `dir/library.db` whose root `dir/music` holds two albums of
/// "Mari Kask" in `Mari Kask/One` and `Mari Kask/Two`.
fn library(dir: &Path) -> (Library, RootId, PathBuf) {
    let music = dir.join("music");
    let artist = music.join("Mari Kask");
    for album in ["One", "Two"] {
        fs::create_dir_all(artist.join(album)).unwrap();
    }
    let library = Library::open(&DatabaseConfig::file(dir.join("library.db"))).unwrap();
    let root = library.add_root(&music).unwrap();
    for album in ["One", "Two"] {
        let path = artist.join(album).join("01.flac");
        let mut r = record(root, path.to_str().unwrap(), album);
        r.tags.title = Some(format!("{album} song"));
        r.tags.artists = vec!["Mari Kask".to_owned()];
        r.tags.album_artists = vec!["Mari Kask".to_owned()];
        r.tags.album = Some(album.to_owned());
        library.upsert_track(r).unwrap();
    }
    (library, root, artist)
}

fn person(library: &Library) -> PersonId {
    let page = library
        .artists(&ArtistQuery::new(ArtistRole::AlbumArtist))
        .unwrap();
    assert_eq!(page.items.len(), 1);
    page.items[0].id
}

#[test]
fn chosen_before_folder_before_none() {
    let dir = folder("priority");
    let (library, _, artist) = library(&dir);
    let store = dir.join("store");
    let mari = person(&library);

    // None yet.
    assert_eq!(library.find_artist_folder_photos().unwrap(), 0);
    assert_eq!(library.artist_photo(mari).unwrap(), None);
    assert!(library.artist_photos().unwrap().is_empty());

    // The folder's photo.
    let in_folder = artist.join("Artist.PNG");
    fs::write(&in_folder, png([200, 10, 10])).unwrap();
    assert_eq!(library.find_artist_folder_photos().unwrap(), 1);
    let found = library.artist_photo(mari).unwrap().unwrap();
    assert!(!found.chosen);
    assert_eq!(found.path, in_folder);
    assert_eq!(found.hash, CoverHash::of(&png([200, 10, 10])));
    assert!(library.cover_hashes().unwrap().contains(&found.hash));
    assert_eq!(
        library.cover_locations(&found.hash).unwrap(),
        vec![CoverLocation::File {
            path: in_folder.clone()
        }]
    );

    // A chosen photo comes first; the picked file is copied and left as it is.
    let picked = dir.join("singer.png");
    fs::write(&picked, png([10, 200, 10])).unwrap();
    let chosen = library.set_artist_photo(mari, &picked, &store).unwrap();
    assert!(chosen.chosen);
    assert_eq!(chosen.hash, CoverHash::of(&png([10, 200, 10])));
    assert!(chosen.path.starts_with(&store));
    assert_eq!(fs::read(&chosen.path).unwrap(), png([10, 200, 10]));
    assert_eq!(fs::read(&picked).unwrap(), png([10, 200, 10]));
    assert_eq!(library.artist_photo(mari).unwrap(), Some(chosen.clone()));
    assert_eq!(library.artist_photos().unwrap().get(&mari), Some(&chosen));
    assert_eq!(
        library.cover_locations(&chosen.hash).unwrap(),
        vec![CoverLocation::File {
            path: chosen.path.clone()
        }]
    );

    // Another choice replaces it, and the old copy goes.
    let other = dir.join("other.png");
    fs::write(&other, png([10, 10, 200])).unwrap();
    let second = library.set_artist_photo(mari, &other, &store).unwrap();
    assert!(!chosen.path.exists());
    assert!(second.path.exists());

    // Removing it falls back to the folder's photo and deletes the copy.
    let now = library.remove_artist_photo(mari).unwrap();
    assert_eq!(now, Some(found.clone()));
    assert!(!second.path.exists());
    assert_eq!(library.artist_photo(mari).unwrap(), Some(found));

    // Without the folder's photo: none.
    fs::remove_file(&in_folder).unwrap();
    assert_eq!(library.find_artist_folder_photos().unwrap(), 0);
    assert_eq!(library.artist_photo(mari).unwrap(), None);
    assert_eq!(library.remove_artist_photo(mari).unwrap(), None);
}

#[test]
fn photos_survive_a_reopened_database() {
    let dir = folder("persistence");
    let store = dir.join("store");
    let picked = dir.join("singer.jpg.png");
    fs::write(&picked, png([1, 2, 3])).unwrap();
    let chosen = {
        let (library, _, artist) = library(&dir);
        fs::write(artist.join("artist.webp.txt"), b"not a photo").unwrap();
        fs::write(artist.join("artist.png"), png([9, 9, 9])).unwrap();
        assert_eq!(library.find_artist_folder_photos().unwrap(), 1);
        library
            .set_artist_photo(person(&library), &picked, &store)
            .unwrap()
    };
    let library = Library::open(&DatabaseConfig::file(dir.join("library.db"))).unwrap();
    let mari = person(&library);
    assert_eq!(library.artist_photo(mari).unwrap(), Some(chosen));
    // A folder photo that did not change is not read again.
    assert_eq!(library.find_artist_folder_photos().unwrap(), 1);
    let fallback = library.remove_artist_photo(mari).unwrap().unwrap();
    assert!(!fallback.chosen);
    assert_eq!(fallback.hash, CoverHash::of(&png([9, 9, 9])));
}

#[test]
fn a_file_that_is_no_picture_is_refused() {
    let dir = folder("refused");
    let (library, _, _) = library(&dir);
    let mari = person(&library);
    let store = dir.join("store");
    let text = dir.join("notes.png");
    fs::write(&text, b"these are not pixels").unwrap();
    assert!(matches!(
        library.set_artist_photo(mari, &text, &store),
        Err(ArtistPhotoError::Picture(_))
    ));
    assert!(matches!(
        library.set_artist_photo(mari, &dir.join("missing.png"), &store),
        Err(ArtistPhotoError::Read { .. })
    ));
    assert!(matches!(
        library.set_artist_photo(PersonId::from_raw(9999), &text, &store),
        Err(ArtistPhotoError::UnknownArtist)
    ));
    assert_eq!(library.artist_photo(mari).unwrap(), None);
}
