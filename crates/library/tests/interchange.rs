// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Imports from other players, the export and import of the user data, and
//! statistics carried to a replacement: fixtures made in the tests, matching,
//! the merge rules, a second import of the same source, undo.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_library::interchange::{
    ImportRunKind, ImportSource, MergeOptions, PathMapping, PlannedPlaylistKind, Player,
    default_sources_in, read_source,
};
use helikodu_library::raw::rusqlite::Connection;
use helikodu_library::{
    FieldDefinition, ItemContent, LabelTarget, Library, Listen, ListenEnd, Page, PlayRules,
    PlaylistType, Rating, RootId, UserField,
};
use helikodu_model::library::TrackId;
use support::{library, tagged};

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// A new, empty folder for one test.
fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("interchange")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The library of the tests: two tracks of one album, a track of another, and
/// two tracks of the same title and artist on different albums.
struct Fixture {
    library: Library,
    one: TrackId,
    two: TrackId,
    other: TrackId,
}

fn track(
    library: &Library,
    root: RootId,
    path: &str,
    tags: (&str, &str, &str),
    no: u32,
) -> TrackId {
    let mut record = tagged(root, path, path, tags.0, tags.1, tags.2);
    record.tags.track_no = Some(no);
    library.upsert_track(record).unwrap().track
}

fn fixture() -> Fixture {
    let (library, root) = library();
    let one = track(
        &library,
        root,
        "/music/A/01.flac",
        ("Song One", "Alpha", "First"),
        1,
    );
    let two = track(
        &library,
        root,
        "/music/A/02.flac",
        ("Song Two", "Alpha", "First"),
        2,
    );
    let other = track(
        &library,
        root,
        "/music/B/01.flac",
        ("Other", "Beta", "Second"),
        1,
    );
    track(
        &library,
        root,
        "/music/X/01.flac",
        ("Same", "Gamma", "X"),
        1,
    );
    track(
        &library,
        root,
        "/music/Y/01.flac",
        ("Same", "Gamma", "Y"),
        1,
    );
    Fixture {
        library,
        one,
        two,
        other,
    }
}

fn playlists(library: &Library) -> Vec<(String, PlaylistType, Option<String>)> {
    library
        .playlist_tree()
        .unwrap()
        .into_iter()
        .filter(|n| n.summary.name != "Favourites")
        .map(|n| {
            let query = (n.summary.kind == PlaylistType::Smart).then(|| {
                helikodu_library::print_query(&library.smart_playlist_query(n.summary.id).unwrap())
            });
            (n.summary.name, n.summary.kind, query)
        })
        .collect()
}

fn playlist_named(library: &Library, name: &str) -> Vec<ItemContent> {
    let node = library
        .playlist_tree()
        .unwrap()
        .into_iter()
        .find(|n| n.summary.name == name)
        .unwrap();
    library
        .playlist_items(node.summary.id, Page::default())
        .unwrap()
        .items
        .into_iter()
        .map(|i| i.content)
        .collect()
}

fn write_rhythmbox(dir: &Path, plays: u32) -> PathBuf {
    let db = dir.join("rhythmdb.xml");
    fs::write(
        &db,
        format!(
            r#"<?xml version="1.0" standalone="yes"?>
<rhythmdb version="2.0">
  <entry type="song">
    <title>Song One</title><artist>Alpha</artist><album>First</album>
    <track-number>1</track-number><duration>200</duration>
    <location>file:///music/A/01.flac</location>
    <first-seen>500</first-seen><last-played>1000</last-played>
    <play-count>{plays}</play-count><rating>4</rating>
  </entry>
  <entry type="song">
    <title>Song Two</title><artist>Alpha</artist><album>First</album>
    <track-number>2</track-number><duration>201</duration>
    <location>file:///old/place/02.flac</location><play-count>2</play-count>
  </entry>
  <entry type="song">
    <title>Same</title><artist>Gamma</artist><duration>200</duration>
    <location>file:///old/same.flac</location><play-count>9</play-count>
  </entry>
  <entry type="song">
    <title>Gone</title><artist>Nobody</artist>
    <location>file:///old/gone.flac</location><play-count>1</play-count>
  </entry>
  <entry type="iradio"><title>Radio</title><location>http://radio/</location></entry>
</rhythmdb>"#
        ),
    )
    .unwrap();
    fs::write(
        dir.join("playlists.xml"),
        r#"<?xml version="1.0"?>
<rhythmdb-playlists>
  <playlist name="Mix" show-browser="true" browser-position="180" search-type="search-match" type="static">
    <location>file:///music/A/01.flac</location>
    <location>file:///old/place/02.flac</location>
    <location>file:///old/unknown.flac</location>
  </playlist>
  <playlist name="Good" type="automatic" sort-key="Rating" sort-direction="1" limit-count="10">
    <conjunction><equals prop="type">song</equals><greater prop="rating">3</greater></conjunction>
  </playlist>
  <playlist name="Odd" type="automatic">
    <conjunction><greater prop="mtime">3</greater></conjunction>
  </playlist>
  <playlist name="Play Queue" type="queue"><location>file:///music/A/01.flac</location></playlist>
</rhythmdb-playlists>"#,
    )
    .unwrap();
    db
}

#[test]
fn rhythmbox_import_matches_merges_and_undoes() {
    let f = fixture();
    let dir = folder("rhythmbox");
    let source = ImportSource {
        player: Player::Rhythmbox,
        path: write_rhythmbox(&dir, 5),
    };
    // Our own plays and rating before the import.
    f.library
        .record_listen(
            f.two,
            Listen {
                started_at: at(3000),
                played: Duration::from_secs(150),
                end: ListenEnd::Finished,
                position: None,
                context: None,
            },
            PlayRules::default(),
        )
        .unwrap();
    f.library.set_rating(f.two, Rating::from_stars(1)).unwrap();

    let plan = f
        .library
        .plan_import(&source, &MergeOptions::default())
        .unwrap();
    assert_eq!(plan.kind, ImportRunKind::Player);
    assert_eq!(plan.matching.entries, 4);
    assert_eq!(plan.matching.by_path, 1);
    assert_eq!(plan.matching.by_tags, 1);
    assert_eq!(plan.matching.ambiguous, 1);
    assert_eq!(plan.matching.unmatched, 1);
    assert_eq!(plan.unmatched, ["Nobody – Gone"]);
    assert_eq!(plan.totals.tracks, 2);
    assert_eq!(plan.totals.plays_added, 7);
    assert_eq!(plan.totals.ratings, 1);
    assert_eq!(plan.totals.added, 1);
    let kinds: Vec<&PlannedPlaylistKind> = plan.playlists.iter().map(|p| &p.kind).collect();
    assert_eq!(
        kinds[0],
        &PlannedPlaylistKind::Static {
            items: 3,
            matched: 2,
            from_rules: false
        }
    );
    assert_eq!(
        kinds[1],
        &PlannedPlaylistKind::Smart("rating>3 sort:rating desc limit:10".to_owned())
    );
    assert!(matches!(kinds[2], PlannedPlaylistKind::Unmapped(_)));
    assert_eq!(kinds.len(), 3);
    // Nothing changed yet.
    assert_eq!(f.library.stats(f.one).unwrap().play_count, 0);

    let run = f.library.apply_import(&plan).unwrap();
    let one = f.library.stats(f.one).unwrap();
    assert_eq!(one.play_count, 5);
    assert_eq!(one.rating, Rating::from_stars(4));
    assert_eq!(one.last_played, Some(at(1000)));
    assert_eq!(one.first_played, Some(at(1000)));
    let two = f.library.stats(f.two).unwrap();
    assert_eq!(two.play_count, 3);
    // Our rating stays.
    assert_eq!(two.rating, Rating::from_stars(1));
    assert_eq!(two.last_played, Some(at(3000)));
    assert_eq!(f.library.stats(f.other).unwrap().play_count, 0);
    assert_eq!(f.library.added_times(&[f.one]).unwrap()[&f.one], at(500));
    assert_eq!(
        playlists(&f.library),
        [
            ("Mix".to_owned(), PlaylistType::Static, None),
            (
                "Good".to_owned(),
                PlaylistType::Smart,
                Some("rating>3 sort:rating desc limit:10".to_owned())
            ),
        ]
    );
    let mix = playlist_named(&f.library, "Mix");
    assert!(matches!(&mix[0], ItemContent::Track(t) if t.id == f.one));
    assert!(matches!(&mix[1], ItemContent::Track(t) if t.id == f.two));
    assert!(matches!(&mix[2], ItemContent::Dead(_)));

    // The same source again: nothing new.
    let again = f
        .library
        .plan_import(&source, &MergeOptions::default())
        .unwrap();
    assert_eq!(again.totals.tracks, 0);
    assert_eq!(again.totals.plays_added, 0);
    assert!(
        again
            .playlists
            .iter()
            .filter(|p| !matches!(p.kind, PlannedPlaylistKind::Unmapped(_)))
            .all(|p| p.kind == PlannedPlaylistKind::Exists)
    );
    assert!(again.is_empty());

    // Two more plays there: only those are added.
    write_rhythmbox(&dir, 7);
    let more = f
        .library
        .plan_import(&source, &MergeOptions::default())
        .unwrap();
    assert_eq!(more.totals.plays_added, 2);
    let second = f.library.apply_import(&more).unwrap();
    assert_eq!(f.library.stats(f.one).unwrap().play_count, 7);
    assert_eq!(playlists(&f.library).len(), 2);

    // Undo, newest first.
    assert!(f.library.undo_import(run).is_err());
    f.library.undo_import(second).unwrap();
    assert_eq!(f.library.stats(f.one).unwrap().play_count, 5);
    f.library.undo_import(run).unwrap();
    let one = f.library.stats(f.one).unwrap();
    assert_eq!(one.play_count, 0);
    assert_eq!(one.rating, None);
    assert_eq!(one.last_played, None);
    let two = f.library.stats(f.two).unwrap();
    assert_eq!(two.play_count, 1);
    assert_eq!(two.rating, Rating::from_stars(1));
    assert!(playlists(&f.library).is_empty());
    assert_ne!(f.library.added_times(&[f.one]).unwrap()[&f.one], at(500));
    let runs = f.library.import_runs().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|r| r.undone));
    // After the undo, the first import counts in full again.
    let fresh = f
        .library
        .plan_import(&source, &MergeOptions::default())
        .unwrap();
    assert_eq!(fresh.totals.plays_added, 9);
}

#[test]
fn replacing_ratings_is_asked_for() {
    let f = fixture();
    let dir = folder("replace-ratings");
    let source = ImportSource {
        player: Player::Rhythmbox,
        path: write_rhythmbox(&dir, 1),
    };
    f.library.set_rating(f.one, Rating::from_stars(2)).unwrap();
    let options = MergeOptions {
        replace_ratings: true,
        skip_playlists: true,
        ..MergeOptions::default()
    };
    let plan = f.library.plan_import(&source, &options).unwrap();
    assert!(plan.playlists.is_empty());
    f.library.apply_import(&plan).unwrap();
    assert_eq!(
        f.library.stats(f.one).unwrap().rating,
        Rating::from_stars(4)
    );
}

fn strawberry_schema(c: &Connection, location: &str, id: &str) {
    c.execute_batch(&format!(
        "CREATE TABLE songs (title TEXT, album TEXT, artist TEXT, albumartist TEXT,
             track INTEGER, disc INTEGER, length INTEGER, {location} TEXT,
             playcount INTEGER NOT NULL DEFAULT 0, skipcount INTEGER NOT NULL DEFAULT 0,
             lastplayed INTEGER NOT NULL DEFAULT -1, rating REAL NOT NULL DEFAULT -1);
         CREATE TABLE playlists (name TEXT NOT NULL, last_played INTEGER,
             dynamic_playlist_type INTEGER, special_type TEXT);
         CREATE TABLE playlist_items (playlist INTEGER NOT NULL, type TEXT,
             {id} INTEGER, {location} TEXT, title TEXT, artist TEXT, album TEXT,
             length INTEGER);"
    ))
    .unwrap();
}

#[test]
fn strawberry_is_read_from_a_copy_while_it_is_locked() {
    let f = fixture();
    let dir = folder("strawberry");
    let path = dir.join("strawberry.db");
    let c = Connection::open(&path).unwrap();
    c.pragma_update(None, "journal_mode", "wal").unwrap();
    strawberry_schema(&c, "url", "collection_id");
    c.execute_batch(
        "INSERT INTO songs (title, album, artist, track, length, url, playcount, skipcount,
                            lastplayed, rating)
         VALUES ('Song One', 'First', 'Alpha', 1, 200000000000, 'file:///music/A/01.flac',
                 4, 2, 1200, 0.7),
                ('Other', 'Second', 'Beta', 1, 199000000000, 'file:///moved/b.flac',
                 1, 0, -1, -1);
         INSERT INTO playlists (name) VALUES ('Saved');
         INSERT INTO playlists (name, dynamic_playlist_type) VALUES ('Dynamic', 1);
         INSERT INTO playlist_items (playlist, type, collection_id) VALUES (1, 'Collection', 2);
         INSERT INTO playlist_items (playlist, type, url, title)
             VALUES (1, 'File', 'file:///music/A/01.flac', 'Song One');",
    )
    .unwrap();
    // The player holds the database.
    c.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let before = fs::read(&path).unwrap();
    let source = ImportSource {
        player: Player::Strawberry,
        path: path.clone(),
    };
    let read = read_source(&source).unwrap();
    assert_eq!(read.tracks.len(), 2);
    assert_eq!(read.tracks[0].rating, Rating::from_half_stars(7));
    assert_eq!(read.tracks[0].duration, Some(Duration::from_secs(200)));
    assert_eq!(read.tracks[1].last_played, None);
    let plan = f
        .library
        .plan_import(&source, &MergeOptions::default())
        .unwrap();
    assert_eq!(plan.matching.matched(), 2);
    assert!(matches!(
        plan.playlists[1].kind,
        PlannedPlaylistKind::Unmapped(_)
    ));
    f.library.apply_import(&plan).unwrap();
    c.execute_batch("COMMIT").unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    let one = f.library.stats(f.one).unwrap();
    assert_eq!((one.play_count, one.skip_count), (4, 2));
    assert_eq!(one.rating, Rating::from_half_stars(7));
    assert_eq!(f.library.stats(f.other).unwrap().play_count, 1);
    let saved = playlist_named(&f.library, "Saved");
    assert!(matches!(&saved[0], ItemContent::Track(t) if t.id == f.other));
    assert!(matches!(&saved[1], ItemContent::Track(t) if t.id == f.one));
}

#[test]
fn clementine_names_its_columns_differently() {
    let f = fixture();
    let dir = folder("clementine");
    let path = dir.join("clementine.db");
    let c = Connection::open(&path).unwrap();
    strawberry_schema(&c, "filename", "library_id");
    c.execute_batch(
        "INSERT INTO songs (title, album, artist, track, length, filename, playcount, rating)
         VALUES ('Song Two', 'First', 'Alpha', 2, 200000000000, 'file:///music/A/02.flac', 3, 0.5);
         INSERT INTO playlists (name) VALUES ('List');
         INSERT INTO playlist_items (playlist, type, library_id) VALUES (1, 'Library', 1);",
    )
    .unwrap();
    drop(c);
    let source = ImportSource {
        player: Player::Clementine,
        path: dir.clone(),
    };
    let plan = f
        .library
        .plan_import(&source, &MergeOptions::default())
        .unwrap();
    assert_eq!(plan.matching.by_path, 1);
    f.library.apply_import(&plan).unwrap();
    let two = f.library.stats(f.two).unwrap();
    assert_eq!(two.play_count, 3);
    assert_eq!(two.rating, Rating::from_half_stars(5));
    assert_eq!(playlist_named(&f.library, "List").len(), 1);
}

#[test]
fn lollypop_counts_popularity_as_plays() {
    let f = fixture();
    let dir = folder("lollypop");
    let c = Connection::open(dir.join("lollypop.db")).unwrap();
    c.execute_batch(
        "CREATE TABLE albums (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE track_artists (track_id INT NOT NULL, artist_id INT NOT NULL);
         CREATE TABLE tracks (id INTEGER PRIMARY KEY, name TEXT NOT NULL, uri TEXT NOT NULL,
             duration INT, tracknumber INT, album_id INT NOT NULL, popularity INT NOT NULL,
             rate INT NOT NULL DEFAULT 0, loved INT NOT NULL DEFAULT 0,
             ltime INT NOT NULL DEFAULT 0, mtime INT NOT NULL);
         INSERT INTO albums VALUES (1, 'Second');
         INSERT INTO artists VALUES (1, 'Beta');
         INSERT INTO track_artists VALUES (1, 1);
         INSERT INTO tracks VALUES (1, 'Other', 'file:///elsewhere/other.flac', 200000, 1, 1,
                                    6, 5, 1, 4000, 0);",
    )
    .unwrap();
    drop(c);
    let p = Connection::open(dir.join("playlists.db")).unwrap();
    p.execute_batch(
        "CREATE TABLE playlists (id INTEGER PRIMARY KEY, name TEXT NOT NULL,
             smart_enabled INT NOT NULL DEFAULT 0, smart_sql TEXT, mtime BIGINT NOT NULL);
         CREATE TABLE tracks (playlist_id INT NOT NULL, uri TEXT NOT NULL);
         INSERT INTO playlists VALUES (1, 'Lolly', 0, NULL, 0), (2, 'Smart', 1, 'SELECT 1', 0);
         INSERT INTO tracks VALUES (1, 'file:///elsewhere/other.flac');",
    )
    .unwrap();
    drop(p);
    let source = ImportSource {
        player: Player::Lollypop,
        path: dir.join("lollypop.db"),
    };
    let plan = f
        .library
        .plan_import(&source, &MergeOptions::default())
        .unwrap();
    assert_eq!(plan.matching.by_tags, 1);
    assert_eq!(plan.totals.loved, 1);
    f.library.apply_import(&plan).unwrap();
    let other = f.library.stats(f.other).unwrap();
    assert_eq!(other.play_count, 6);
    assert_eq!(other.rating, Rating::from_stars(5));
    assert_eq!(other.last_played, Some(at(4000)));
    assert_eq!(playlist_named(&f.library, "Lolly").len(), 1);
    assert!(matches!(
        plan.playlists[1].kind,
        PlannedPlaylistKind::Unmapped(_)
    ));
}

#[test]
fn itunes_paths_are_mapped_and_smart_playlists_kept_as_lists() {
    let f = fixture();
    let dir = folder("itunes");
    let path = dir.join("Library.xml");
    fs::write(
        &path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Major Version</key><integer>1</integer>
  <key>Tracks</key>
  <dict>
    <key>101</key>
    <dict>
      <key>Track ID</key><integer>101</integer>
      <key>Name</key><string>Song One</string>
      <key>Artist</key><string>Alpha</string>
      <key>Total Time</key><integer>200000</integer>
      <key>Date Added</key><string>not a date</string>
      <key>Play Count</key><integer>11</integer>
      <key>Play Date UTC</key><date>2009-06-01T19:10:24Z</date>
      <key>Skip Count</key><integer>2</integer>
      <key>Skip Date</key><date>2010-01-01T00:00:00Z</date>
      <key>Rating</key><integer>60</integer>
      <key>Loved</key><true/>
      <key>Track Type</key><string>File</string>
      <key>Location</key><string>file://localhost/C:/Users/me/Music/A/01.flac</string>
    </dict>
    <key>102</key>
    <dict>
      <key>Track ID</key><integer>102</integer>
      <key>Name</key><string>Other</string>
      <key>Artist</key><string>Beta</string>
      <key>Rating</key><integer>100</integer>
      <key>Rating Computed</key><true/>
      <key>Track Type</key><string>File</string>
      <key>Location</key><string>file://localhost/C:/Users/me/Music/B/01.flac</string>
    </dict>
  </dict>
  <key>Playlists</key>
  <array>
    <dict><key>Name</key><string>Library</string><key>Master</key><true/>
      <key>Playlist Items</key><array><dict><key>Track ID</key><integer>101</integer></dict></array></dict>
    <dict><key>Name</key><string>Music</string><key>Distinguished Kind</key><integer>4</integer></dict>
    <dict><key>Name</key><string>Old</string><key>Folder</key><true/>
      <key>Playlist Persistent ID</key><string>F1</string></dict>
    <dict><key>Name</key><string>Road</string><key>Parent Persistent ID</key><string>F1</string>
      <key>Playlist Items</key><array>
        <dict><key>Track ID</key><integer>102</integer></dict>
        <dict><key>Track ID</key><integer>101</integer></dict>
      </array></dict>
    <dict><key>Name</key><string>Top</string>
      <key>Smart Info</key><data>AQEAAwAAAAIAAAAZ</data>
      <key>Smart Criteria</key><data>U0xzdAABAAEAAAAD</data>
      <key>Playlist Items</key><array><dict><key>Track ID</key><integer>101</integer></dict></array></dict>
  </array>
</dict>
</plist>"#,
    )
    .unwrap();
    let source = ImportSource {
        player: Player::ITunes,
        path,
    };
    // Without a mapping, the Windows paths match nothing but the tags still do.
    let plain = f
        .library
        .plan_import(&source, &MergeOptions::default())
        .unwrap();
    assert_eq!(plain.matching.by_path, 0);
    assert_eq!(plain.matching.by_tags, 2);
    let options = MergeOptions {
        mappings: vec![PathMapping::new(r"C:\Users\me\Music", "/music")],
        ..MergeOptions::default()
    };
    let plan = f.library.plan_import(&source, &options).unwrap();
    assert_eq!(plan.matching.by_path, 2);
    assert_eq!(plan.notes.len(), 1);
    assert_eq!(plan.playlists.len(), 2);
    assert_eq!(plan.playlists[0].folder, ["Old"]);
    assert_eq!(
        plan.playlists[1].kind,
        PlannedPlaylistKind::Static {
            items: 1,
            matched: 1,
            from_rules: true
        }
    );
    f.library.apply_import(&plan).unwrap();
    let one = f.library.stats(f.one).unwrap();
    assert_eq!((one.play_count, one.skip_count), (11, 2));
    assert_eq!(one.rating, Rating::from_stars(3));
    assert_eq!(one.last_played, Some(at(1_243_883_424)));
    assert_eq!(one.last_skipped, Some(at(1_262_304_000)));
    // A computed rating is not taken.
    assert_eq!(f.library.stats(f.other).unwrap().rating, None);
    let tree = f.library.playlist_tree().unwrap();
    let road = tree.iter().find(|n| n.summary.name == "Road").unwrap();
    assert_eq!(road.depth, 1);
    assert_eq!(playlist_named(&f.library, "Road").len(), 2);
}

#[test]
fn default_sources_are_offered_only_when_they_exist() {
    let home = folder("home");
    assert!(default_sources_in(&home, None, None).is_empty());
    let flatpak = home.join(".var/app/org.gnome.Lollypop/data/lollypop/lollypop.db");
    fs::create_dir_all(flatpak.parent().unwrap()).unwrap();
    fs::write(&flatpak, b"").unwrap();
    let rhythmbox = home.join(".local/share/rhythmbox/rhythmdb.xml");
    fs::create_dir_all(rhythmbox.parent().unwrap()).unwrap();
    fs::write(&rhythmbox, b"").unwrap();
    let found = default_sources_in(&home, None, None);
    assert_eq!(
        found,
        [
            ImportSource {
                player: Player::Rhythmbox,
                path: rhythmbox
            },
            ImportSource {
                player: Player::Lollypop,
                path: flatpak
            },
        ]
    );
    assert_eq!(Player::from_id("Music"), Some(Player::ITunes));
}

/// A library with statistics, history, playlists, Favourites and a saved search.
fn with_user_data() -> Fixture {
    let f = fixture();
    let rules = PlayRules::default();
    for started in [1000, 2000] {
        f.library
            .record_listen(
                f.one,
                Listen {
                    started_at: at(started),
                    played: Duration::from_secs(190),
                    end: ListenEnd::Finished,
                    position: None,
                    context: Some("album".to_owned()),
                },
                rules,
            )
            .unwrap();
    }
    f.library
        .set_rating(f.two, Rating::from_half_stars(7))
        .unwrap();
    let mix = f.library.create_playlist("Mix").unwrap();
    f.library.add_to_playlist(mix, vec![f.two, f.one]).unwrap();
    let folder = f.library.create_playlist_folder("Box", None).unwrap();
    let query = helikodu_library::parse_query("rating>=3").unwrap();
    f.library
        .create_smart_playlist("Rated", &query, Some(folder))
        .unwrap();
    f.library.set_favourite(f.other, true).unwrap();
    f.library
        .save_search(
            "Alpha",
            &helikodu_library::parse_query("artist:alpha").unwrap(),
        )
        .unwrap();
    f
}

#[test]
fn user_data_goes_round_trip() {
    let source = with_user_data();
    let dir = folder("user-data");
    let file = dir.join("export.json");
    let export = source.library.export_user_data_to(&file).unwrap();
    assert_eq!(export.tracks(), 2);
    assert_eq!(export.history(), 2);
    assert_eq!(export.playlists(), 2);
    assert_eq!(export.favourites(), 1);
    assert_eq!(export.saved_searches(), 1);

    // A library rebuilt from the same files.
    let target = fixture();
    let plan = target
        .library
        .plan_user_data_import(&file, &MergeOptions::default())
        .unwrap();
    assert_eq!(plan.kind, ImportRunKind::UserData);
    assert_eq!(plan.matching.by_path, 2);
    assert_eq!(plan.totals.history, 2);
    assert_eq!(plan.totals.favourites, 1);
    assert_eq!(plan.totals.saved_searches, 1);
    let run = target.library.apply_import(&plan).unwrap();
    let one = target.library.stats(target.one).unwrap();
    assert_eq!(one, source.library.stats(source.one).unwrap());
    assert_eq!(
        target.library.stats(target.two).unwrap().rating,
        Rating::from_half_stars(7)
    );
    let history = target
        .library
        .history(&helikodu_library::HistoryQuery::default())
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].context.as_deref(), Some("album"));
    assert_eq!(playlists(&target.library), playlists(&source.library));
    let mix = playlist_named(&target.library, "Mix");
    assert!(matches!(&mix[0], ItemContent::Track(t) if t.id == target.two));
    assert!(target.library.is_favourite(target.other).unwrap());
    assert_eq!(
        target.library.saved_searches().unwrap()[0].text,
        "artist:alpha"
    );
    let tree = target.library.playlist_tree().unwrap();
    let rated = tree.iter().find(|n| n.summary.name == "Rated").unwrap();
    assert_eq!(rated.depth, 1);

    // Reading the export into the library it came from changes nothing.
    let same = source
        .library
        .plan_user_data_import(&file, &MergeOptions::default())
        .unwrap();
    assert!(same.is_empty(), "{same:?}");

    // Undo takes all of it back.
    target.library.undo_import(run).unwrap();
    assert_eq!(target.library.stats(target.one).unwrap().play_count, 0);
    assert!(
        target
            .library
            .history(&helikodu_library::HistoryQuery::default())
            .unwrap()
            .is_empty()
    );
    assert!(playlists(&target.library).is_empty());
    assert!(!target.library.is_favourite(target.other).unwrap());
    assert!(target.library.saved_searches().unwrap().is_empty());
}

/// The labels given to `target` itself.
fn own_labels(library: &Library, target: LabelTarget) -> Vec<String> {
    library.labels_of(&target).unwrap().own
}

#[test]
fn labels_and_fields_go_round_trip_and_are_undone() {
    let source = fixture();
    let lib = &source.library;
    lib.add_label(&LabelTarget::Tracks(vec![source.one]), "Late night")
        .unwrap();
    lib.add_label(&LabelTarget::AlbumOf(source.two), "Road")
        .unwrap();
    lib.add_label(&LabelTarget::Artist("Beta".to_owned()), "Road")
        .unwrap();
    lib.define_field(UserField::computed("decade", "{left(year, 3)}0s"))
        .unwrap();
    lib.define_field(UserField::tag("mood", "MOOD")).unwrap();
    let dir = folder("labels-fields");
    let file = dir.join("export.json");
    let export = lib.export_user_data_to(&file).unwrap();
    assert_eq!(export.labels(), 2);
    assert_eq!(export.label_items(), 3);
    assert_eq!(export.fields(), 2);
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["labels"][0]["name"], "Late night");
    assert_eq!(json["labels"][0]["tracks"][0]["path"], "/music/A/01.flac");
    assert_eq!(json["labels"][1]["albums"][0]["album_artist"], "Alpha");
    assert_eq!(json["labels"][1]["albums"][0]["album"], "First");
    assert_eq!(json["labels"][1]["artists"][0], "Beta");
    assert_eq!(json["fields"][0]["kind"], "computed");
    assert_eq!(json["fields"][1]["definition"], "MOOD");

    // A library rebuilt from the same files, with a label of its own.
    let target = fixture();
    let t = &target.library;
    t.add_label(&LabelTarget::Tracks(vec![target.two]), "road")
        .unwrap();
    let plan = t
        .plan_user_data_import(&file, &MergeOptions::default())
        .unwrap();
    assert_eq!(plan.totals.labels, 1, "{plan:?}");
    assert_eq!(plan.totals.label_links, 3);
    assert_eq!(plan.totals.label_unmatched, 0);
    assert_eq!(plan.totals.fields, 2);
    assert_eq!(plan.totals.field_conflicts, 0);
    assert!(!plan.is_empty());
    let run = t.apply_import(&plan).unwrap();
    assert_eq!(
        own_labels(t, LabelTarget::Tracks(vec![target.one])),
        ["Late night"]
    );
    // The label the library had keeps its spelling.
    assert_eq!(own_labels(t, LabelTarget::AlbumOf(target.two)), ["road"]);
    assert_eq!(
        own_labels(t, LabelTarget::Artist("beta".to_owned())),
        ["road"]
    );
    assert_eq!(t.user_fields().unwrap(), lib.user_fields().unwrap());

    // Reading the export into the library it came from changes nothing.
    let same = lib
        .plan_user_data_import(&file, &MergeOptions::default())
        .unwrap();
    assert!(same.is_empty(), "{same:?}");

    // What the user adds after the import stays when it is undone.
    t.add_label(&LabelTarget::Tracks(vec![target.other]), "Road")
        .unwrap();
    t.define_field(UserField::computed("era", "{decade}!"))
        .unwrap();
    t.undo_import(run).unwrap();
    assert!(own_labels(t, LabelTarget::Tracks(vec![target.one])).is_empty());
    assert!(own_labels(t, LabelTarget::AlbumOf(target.one)).is_empty());
    assert!(own_labels(t, LabelTarget::Artist("Beta".to_owned())).is_empty());
    assert_eq!(
        own_labels(t, LabelTarget::Tracks(vec![target.other])),
        ["Road"]
    );
    assert_eq!(
        own_labels(t, LabelTarget::Tracks(vec![target.two])),
        ["Road"]
    );
    let names: Vec<String> = t.labels().unwrap().into_iter().map(|l| l.name).collect();
    assert_eq!(names, ["Road"]);
    // `mood` goes; `decade` stays because `era` uses it.
    let fields: Vec<String> = t
        .user_fields()
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(fields, ["decade", "era"]);
}

#[test]
fn an_export_without_labels_or_fields_still_imports() {
    let target = fixture();
    let dir = folder("old-export");
    let file = dir.join("old.json");
    fs::write(
        &file,
        r#"{"format": "helikodu-user-data", "version": 1, "exported_at": 1763550000,
            "tracks": [{"path": "/music/A/01.flac", "title": "Song One", "artist": "Alpha",
                        "album": "First", "play_count": 3, "skip_count": 0}],
            "playlists": [], "favourites": [], "saved_searches": []}"#,
    )
    .unwrap();
    let plan = target
        .library
        .plan_user_data_import(&file, &MergeOptions::default())
        .unwrap();
    assert_eq!(plan.matching.by_path, 1);
    assert_eq!(plan.totals.plays_added, 3);
    assert_eq!(plan.totals.label_links, 0);
    assert_eq!(plan.totals.fields, 0);
    let run = target.library.apply_import(&plan).unwrap();
    assert_eq!(target.library.stats(target.one).unwrap().play_count, 3);
    assert!(target.library.labels().unwrap().is_empty());
    assert!(target.library.user_fields().unwrap().is_empty());
    target.library.undo_import(run).unwrap();
    assert_eq!(target.library.stats(target.one).unwrap().play_count, 0);
}

#[test]
fn a_field_defined_differently_here_is_kept() {
    let source = fixture();
    source
        .library
        .define_field(UserField::tag("mood", "MOOD"))
        .unwrap();
    source
        .library
        .define_field(UserField::computed("shout", "{upper(title)}"))
        .unwrap();
    let dir = folder("field-conflict");
    let file = dir.join("export.json");
    source.library.export_user_data_to(&file).unwrap();

    let target = fixture();
    let t = &target.library;
    t.define_field(UserField::tag("Mood", "FEELING")).unwrap();
    let plan = t
        .plan_user_data_import(&file, &MergeOptions::default())
        .unwrap();
    assert_eq!(plan.totals.fields, 1);
    assert_eq!(plan.totals.field_conflicts, 1);
    let conflict = &plan.field_conflicts[0];
    assert_eq!(conflict.name, "mood");
    assert_eq!(
        conflict.ours,
        FieldDefinition::Tag {
            key: "FEELING".to_owned()
        }
    );
    assert_eq!(
        conflict.theirs,
        FieldDefinition::Tag {
            key: "MOOD".to_owned()
        }
    );
    let run = t.apply_import(&plan).unwrap();
    let fields = t.user_fields().unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0], UserField::tag("mood", "FEELING"));
    assert_eq!(fields[1].name, "shout");

    // Undo removes the field the import defined and keeps the library's own.
    t.undo_import(run).unwrap();
    assert_eq!(
        t.user_fields().unwrap(),
        [UserField::tag("mood", "FEELING")]
    );
}

#[test]
fn statistics_follow_a_replaced_file() {
    let (library, root) = library();
    let old = library
        .upsert_track(tagged(root, "/music/song.mp3", "mp3", "Song", "Alpha", "A"))
        .unwrap()
        .track;
    library
        .record_listen(
            old,
            Listen {
                started_at: at(1000),
                played: Duration::from_secs(190),
                end: ListenEnd::Finished,
                position: None,
                context: None,
            },
            PlayRules::default(),
        )
        .unwrap();
    library.set_rating(old, Rating::from_stars(4)).unwrap();
    let list = library.create_playlist("List").unwrap();
    library.add_to_playlist(list, vec![old]).unwrap();
    let mut flac = tagged(root, "/music/song.flac", "flac", "Song", "Alpha", "A");
    flac.audio.duration = Some(Duration::from_millis(201_500));
    let new = library.upsert_track(flac).unwrap().track;
    // Nothing to carry while the old file is there.
    assert!(library.carry_candidates().unwrap().is_empty());
    library
        .mark_file_missing("/music/song.mp3", at(5000))
        .unwrap();
    let candidates = library.carry_candidates().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].old, old);
    assert_eq!(candidates[0].new, new);
    assert_eq!(candidates[0].stats.play_count, 1);
    assert_eq!(candidates[0].history, 1);
    assert_eq!(candidates[0].playlist_items, 1);

    let run = library.carry_stats(old, new).unwrap();
    let stats = library.stats(new).unwrap();
    assert_eq!(stats.play_count, 1);
    assert_eq!(stats.rating, Rating::from_stars(4));
    assert_eq!(library.stats(old).unwrap().play_count, 0);
    let items = library.playlist_items(list, Page::default()).unwrap().items;
    assert!(matches!(&items[0].content, ItemContent::Track(t) if t.id == new));
    assert!(library.carry_candidates().unwrap().is_empty());

    library.undo_import(run).unwrap();
    assert_eq!(library.stats(new).unwrap().play_count, 0);
    assert_eq!(library.stats(new).unwrap().rating, None);
    let back = library.stats(old).unwrap();
    assert_eq!(back.play_count, 1);
    assert_eq!(back.rating, Rating::from_stars(4));
    let history = library
        .history(&helikodu_library::HistoryQuery {
            track: Some(old),
            ..helikodu_library::HistoryQuery::default()
        })
        .unwrap();
    assert_eq!(history.len(), 1);
}
