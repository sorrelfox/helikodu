// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The album and artist model through `Library`: album identity (compilations,
//! multi-disc albums, classical credits, equal titles, MusicBrainz IDs, the split by
//! folder), regrouping after tag changes, orphans, masked library folders, and the
//! browsing queries with their order and paging.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use helikodu_library::{
    AlbumDetail, AlbumFilter, AlbumId, AlbumQuality, AlbumQuery, AlbumSort, AlbumSummary,
    ArtistQuery, ArtistRole, Credit, DatabaseConfig, GenreQuery, Library, LocaleCollation, Page,
    PersonId, RootId, TrackFilter, TrackOrder, TrackQuery, TrackRecord, TrackSortKey,
};
use support::{library, record};

/// A track with the usual album tags.
struct T<'a> {
    path: &'a str,
    title: &'a str,
    artist: &'a str,
    album: &'a str,
    album_artist: Option<&'a str>,
    year: Option<i32>,
    track: Option<u32>,
    disc: Option<u32>,
}

impl<'a> T<'a> {
    fn new(path: &'a str, title: &'a str, artist: &'a str, album: &'a str) -> Self {
        Self {
            path,
            title,
            artist,
            album,
            album_artist: None,
            year: None,
            track: None,
            disc: None,
        }
    }

    fn by(mut self, album_artist: &'a str) -> Self {
        self.album_artist = Some(album_artist);
        self
    }

    fn year(mut self, year: i32) -> Self {
        self.year = Some(year);
        self
    }

    fn no(mut self, track: u32) -> Self {
        self.track = Some(track);
        self
    }

    fn disc(mut self, disc: u32) -> Self {
        self.disc = Some(disc);
        self
    }

    fn record(&self, root: RootId) -> TrackRecord {
        let mut r = record(root, self.path, self.path);
        r.tags.title = Some(self.title.to_owned());
        r.tags.artists = vec![self.artist.to_owned()];
        r.tags.album = Some(self.album.to_owned());
        r.tags.album_artists = self.album_artist.iter().map(|a| (*a).to_owned()).collect();
        r.tags.year = self.year;
        r.tags.track_no = self.track;
        r.tags.disc_no = self.disc;
        r
    }
}

fn add(library: &Library, root: RootId, tracks: &[T<'_>]) {
    for track in tracks {
        library.upsert_track(track.record(root)).unwrap();
    }
}

fn all_albums(library: &Library) -> Vec<AlbumSummary> {
    albums_where(library, AlbumFilter::All)
}

fn albums_where(library: &Library, filter: AlbumFilter) -> Vec<AlbumSummary> {
    let page = library
        .albums(&AlbumQuery {
            filter,
            page: Page::first(1000),
            ..AlbumQuery::default()
        })
        .unwrap();
    assert_eq!(page.items.len(), page.total);
    page.items
}

fn titles(albums: &[AlbumSummary]) -> Vec<String> {
    albums
        .iter()
        .map(|a| a.title.clone().unwrap_or_default())
        .collect()
}

fn detail(library: &Library, album: AlbumId) -> AlbumDetail {
    library.album(album).unwrap().unwrap()
}

fn album_of(library: &Library, path: &str) -> AlbumId {
    let track = library
        .track_at(std::path::Path::new(path), 0)
        .unwrap()
        .unwrap();
    let id = i64::try_from(track.get()).unwrap();
    let album: i64 = library
        .database()
        .read(|c| {
            c.query_row("SELECT album_id FROM tracks WHERE id = ?1", [id], |r| {
                r.get(0)
            })
        })
        .unwrap();
    all_albums(library)
        .into_iter()
        .find(|a| a.id.get() == album)
        .unwrap()
        .id
}

fn count(library: &Library, table: &str) -> i64 {
    let sql = format!("SELECT count(*) FROM {table}");
    library
        .database()
        .read(move |c| c.query_row(&sql, [], |r| r.get(0)))
        .unwrap()
}

fn person(library: &Library, role: ArtistRole, name: &str) -> PersonId {
    let page = library
        .artists(&ArtistQuery {
            page: Page::first(1000),
            ..ArtistQuery::new(role)
        })
        .unwrap();
    page.items
        .iter()
        .find(|a| a.name == name)
        .unwrap_or_else(|| panic!("no {name} in {role:?}: {page:?}"))
        .id
}

#[test]
fn a_compilation_without_album_artist_is_by_various_artists() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/Hits/01.flac", "One", "Ann", "Summer Hits").no(1),
            T::new("/music/Hits/02.flac", "Two", "Bo", "Summer Hits").no(2),
            T::new("/music/Hits/03.flac", "Three", "Cy", "Summer Hits").no(3),
        ],
    );
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1, "{albums:?}");
    let album = &albums[0];
    assert_eq!(album.artist, None);
    assert!(album.is_compilation);
    assert_eq!(album.track_count, 3);
    assert_eq!(
        titles(&albums_where(&library, AlbumFilter::Compilations)),
        ["Summer Hits"]
    );
    let detail = detail(&library, album.id);
    assert!(detail.artists.is_empty());
    // Each artist appears on it.
    let ann = person(&library, ArtistRole::Artist, "Ann");
    assert_eq!(albums_where(&library, AlbumFilter::AppearsOn(ann)).len(), 1);
    assert!(albums_where(&library, AlbumFilter::ByArtist(ann)).is_empty());
}

#[test]
fn compilations_by_flag_and_by_album_artist_name() {
    let (library, root) = library();
    let mut flagged = T::new("/music/A/01.flac", "One", "Ann", "Mix")
        .no(1)
        .record(root);
    flagged.catalog.compilation = Some(true);
    library.upsert_track(flagged).unwrap();
    add(
        &library,
        root,
        &[
            T::new("/music/B/01.flac", "One", "Bo", "Tagged VA")
                .by("Various Artists")
                .no(1),
            T::new("/music/B/02.flac", "Two", "Bo", "Tagged VA")
                .by("Various Artists")
                .no(2),
        ],
    );
    let albums = albums_where(&library, AlbumFilter::Compilations);
    assert_eq!(titles(&albums), ["Mix", "Tagged VA"]);
    assert!(albums.iter().all(|a| a.artist.is_none()));
    // A compilation flag with a real album artist (a DJ mix) keeps the artist.
    let mut mix = T::new("/music/C/01.flac", "One", "Ann", "DJ Set")
        .by("DJ Cy")
        .record(root);
    mix.catalog.compilation = Some(true);
    library.upsert_track(mix).unwrap();
    let cy = person(&library, ArtistRole::AlbumArtist, "DJ Cy");
    let by_cy = albums_where(&library, AlbumFilter::ByArtist(cy));
    assert_eq!(titles(&by_cy), ["DJ Set"]);
    assert!(by_cy[0].is_compilation);
    assert_eq!(by_cy[0].artist.as_deref(), Some("DJ Cy"));
}

#[test]
fn an_album_without_album_artist_takes_the_main_track_artist() {
    let (library, root) = library();
    let mut featured = T::new("/music/X/02.flac", "Two", "Ann feat. Bo", "Solo")
        .no(2)
        .record(root);
    featured.catalog.artist_credit = Some("Ann feat. Bo".to_owned());
    featured.tags.artists = vec!["Ann".to_owned(), "Bo".to_owned()];
    library.upsert_track(featured).unwrap();
    add(
        &library,
        root,
        &[
            T::new("/music/X/01.flac", "One", "Ann", "Solo").no(1),
            T::new("/music/X/03.flac", "Three", "Ann", "Solo").no(3),
        ],
    );
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1, "{albums:?}");
    assert_eq!(albums[0].artist.as_deref(), Some("Ann"));
    assert!(!albums[0].is_compilation);
    let ann = person(&library, ArtistRole::AlbumArtist, "Ann");
    assert_eq!(albums_where(&library, AlbumFilter::ByArtist(ann)).len(), 1);
    let bo = person(&library, ArtistRole::Artist, "Bo");
    assert_eq!(
        titles(&albums_where(&library, AlbumFilter::AppearsOn(bo))),
        ["Solo"]
    );
    let tracks = &detail(&library, albums[0].id).discs[0].tracks;
    assert_eq!(tracks[1].artist.as_deref(), Some("Ann feat. Bo"));
}

#[test]
fn discs_in_folders_of_their_own_form_one_album() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/Band/Double/CD1/01.flac", "A1", "Band", "Double")
                .by("Band")
                .no(1),
            T::new("/music/Band/Double/CD1/02.flac", "A2", "Band", "Double")
                .by("Band")
                .no(2),
            T::new("/music/Band/Double/CD2/01.flac", "B1", "Band", "Double")
                .by("Band")
                .no(1),
            T::new(
                "/music/Band/Double/Disc 3 - Live/01.flac",
                "C1",
                "Band",
                "Double",
            )
            .by("Band")
            .no(1),
        ],
    );
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1, "{albums:?}");
    assert_eq!(albums[0].disc_count, 3);
    assert_eq!(albums[0].disc_total, Some(3));
    let detail = detail(&library, albums[0].id);
    let discs: Vec<(u32, Option<&str>, usize)> = detail
        .discs
        .iter()
        .map(|d| (d.number, d.subtitle.as_deref(), d.tracks.len()))
        .collect();
    assert_eq!(discs, [(1, None, 2), (2, None, 1), (3, Some("Live"), 1)]);
    let order: Vec<_> = detail
        .discs
        .iter()
        .flat_map(|d| &d.tracks)
        .map(|t| t.title.clone().unwrap())
        .collect();
    assert_eq!(order, ["A1", "A2", "B1", "C1"]);
}

#[test]
fn tagged_discs_with_subtitles_and_totals() {
    let (library, root) = library();
    let mut tracks = Vec::new();
    for (disc, no, title) in [(2, 1, "B1"), (1, 2, "A2"), (1, 1, "A1"), (2, 2, "B2")] {
        let path = format!("/music/Set/{disc}-{no:02}.flac");
        let mut r = T::new(&path, title, "Ann", "Set")
            .by("Ann")
            .year(2001)
            .no(no)
            .disc(disc)
            .record(root);
        r.catalog.disc_total = Some(3);
        r.catalog.disc_subtitle = (disc == 2).then(|| "Encore".to_owned());
        tracks.push(r);
    }
    library.upsert_tracks(tracks).unwrap();
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1);
    assert_eq!((albums[0].disc_count, albums[0].disc_total), (2, Some(3)));
    let detail = detail(&library, albums[0].id);
    assert_eq!(detail.discs[1].subtitle.as_deref(), Some("Encore"));
    let order: Vec<_> = detail
        .discs
        .iter()
        .flat_map(|d| &d.tracks)
        .map(|t| t.title.clone().unwrap())
        .collect();
    assert_eq!(order, ["A1", "A2", "B1", "B2"]);
}

#[test]
fn classical_credits_by_role() {
    let (library, root) = library();
    for (no, title) in [(1, "I. Allegro"), (2, "II. Adagio")] {
        let path = format!("/music/Classical/{no}.flac");
        let mut r = T::new(&path, title, "Orchestra", "Symphony No. 5")
            .by("Beethoven")
            .year(1990)
            .no(no)
            .record(root);
        r.tags.artists = vec!["Soloist".into(), "Orchestra".into(), "Conductor".into()];
        r.tags.composers = vec!["Beethoven".into()];
        r.catalog.composer_sort = Some("Beethoven, Ludwig van".into());
        r.catalog.credits = vec![
            Credit {
                role: ArtistRole::Conductor,
                name: "Conductor".into(),
                instrument: None,
            },
            Credit {
                role: ArtistRole::Performer,
                name: "Soloist".into(),
                instrument: Some("violin".into()),
            },
        ];
        r.catalog.work = Some("Symphony No. 5".into());
        r.catalog.movement_no = Some(no);
        library.upsert_track(r).unwrap();
    }
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].artist.as_deref(), Some("Beethoven"));

    let names = |role| {
        library
            .artists(&ArtistQuery::new(role))
            .unwrap()
            .items
            .into_iter()
            .map(|a| (a.name, a.album_count, a.track_count))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(ArtistRole::Composer),
        [("Beethoven".to_owned(), 1, 2)]
    );
    assert_eq!(
        names(ArtistRole::Conductor),
        [("Conductor".to_owned(), 1, 2)]
    );
    assert_eq!(names(ArtistRole::Performer), [("Soloist".to_owned(), 1, 2)]);
    assert_eq!(
        names(ArtistRole::AlbumArtist),
        [("Beethoven".to_owned(), 1, 2)]
    );
    assert_eq!(names(ArtistRole::Artist).len(), 3);
    // The composer's sort name comes from the tags.
    let composers = library
        .artists(&ArtistQuery::new(ArtistRole::Composer))
        .unwrap();
    assert_eq!(
        composers.items[0].sort_name.as_deref(),
        Some("Beethoven, Ludwig van")
    );

    let conductor = person(&library, ArtistRole::Conductor, "Conductor");
    let credited = albums_where(
        &library,
        AlbumFilter::Credited(conductor, ArtistRole::Conductor),
    );
    assert_eq!(credited.len(), 1);
    let tracks = library
        .tracks(&TrackQuery {
            filter: TrackFilter::Artist(conductor, Some(ArtistRole::Conductor)),
            ..TrackQuery::default()
        })
        .unwrap();
    assert_eq!(tracks.total, 2);
    let instrument: String = library
        .database()
        .read(|c| {
            c.query_row(
                "SELECT instrument FROM track_people WHERE role = 3 LIMIT 1",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(instrument, "violin");
}

#[test]
fn equal_titles_by_other_artists_or_years_are_other_albums() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/A/Hits/01.flac", "a", "Ann", "Greatest Hits")
                .by("Ann")
                .no(1),
            T::new("/music/B/Hits/01.flac", "b", "Bo", "Greatest Hits")
                .by("Bo")
                .no(1),
            T::new("/music/A/Live 1990/01.flac", "c", "Ann", "Live")
                .by("Ann")
                .year(1990)
                .no(1),
            T::new("/music/A/Live 2000/01.flac", "d", "Ann", "Live")
                .by("Ann")
                .year(2000)
                .no(1),
        ],
    );
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 4, "{albums:?}");
}

#[test]
fn two_copies_of_one_album_in_two_folders_are_split() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/flac/Album/01.flac", "One", "Ann", "Album")
                .by("Ann")
                .year(2000)
                .no(1),
            T::new("/music/flac/Album/02.flac", "Two", "Ann", "Album")
                .by("Ann")
                .year(2000)
                .no(2),
            T::new("/music/mp3/Album/01.mp3", "One", "Ann", "Album")
                .by("Ann")
                .year(2000)
                .no(1),
            T::new("/music/mp3/Album/02.mp3", "Two", "Ann", "Album")
                .by("Ann")
                .year(2000)
                .no(2),
        ],
    );
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 2, "{albums:?}");
    assert!(albums.iter().all(|a| a.track_count == 2));

    // Discs of one album in folders named after the album: one album.
    let (library, root) = support::library();
    add(
        &library,
        root,
        &[
            T::new("/music/Album (CD1)/01.flac", "One", "Ann", "Album")
                .by("Ann")
                .no(1),
            T::new("/music/Album (CD2)/01.flac", "Two", "Ann", "Album")
                .by("Ann")
                .no(1),
        ],
    );
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1, "{albums:?}");
    assert_eq!(albums[0].disc_count, 2);
}

#[test]
fn musicbrainz_ids_decide() {
    let (library, root) = library();
    let mut tracks = Vec::new();
    for (path, album, mbid) in [
        (
            "/music/x/01.flac",
            "Album",
            "aaaaaaaa-0000-0000-0000-000000000001",
        ),
        (
            "/music/y/02.flac",
            "Album (Deluxe)",
            "AAAAAAAA-0000-0000-0000-000000000001",
        ),
        (
            "/music/z/01.flac",
            "Album",
            "bbbbbbbb-0000-0000-0000-000000000002",
        ),
    ] {
        let no = if path.ends_with("01.flac") { 1 } else { 2 };
        let mut r = T::new(path, "t", "Ann", album)
            .by("Ann")
            .year(2000)
            .no(no)
            .record(root);
        r.catalog.release_mbid = Some(mbid.to_owned());
        r.catalog.release_group_mbid = Some("group".to_owned());
        tracks.push(r);
    }
    library.upsert_tracks(tracks).unwrap();
    let albums = all_albums(&library);
    // Different spellings with one release ID are one album; equal tags with two
    // release IDs are two.
    assert_eq!(albums.len(), 2, "{albums:?}");
    let first = album_of(&library, "/music/x/01.flac");
    assert_eq!(first, album_of(&library, "/music/y/02.flac"));
    assert_ne!(first, album_of(&library, "/music/z/01.flac"));
    let detail = detail(&library, first);
    assert_eq!(detail.summary.track_count, 2);
    // Of two spellings of the ID, the album keeps one.
    assert!(
        detail
            .mbid
            .as_deref()
            .is_some_and(|m| m.eq_ignore_ascii_case("aaaaaaaa-0000-0000-0000-000000000001"))
    );
    assert_eq!(detail.release_group_mbid.as_deref(), Some("group"));
}

#[test]
fn tag_changes_split_and_merge_albums() {
    let (library, root) = library();
    let tracks = [
        T::new("/music/A/01.flac", "One", "Ann", "First")
            .by("Ann")
            .no(1),
        T::new("/music/A/02.flac", "Two", "Ann", "First")
            .by("Ann")
            .no(2),
    ];
    add(&library, root, &tracks);
    assert_eq!(all_albums(&library).len(), 1);
    let before = album_of(&library, "/music/A/01.flac");

    // Retitling one track splits it off.
    add(
        &library,
        root,
        &[T::new("/music/A/02.flac", "Two", "Ann", "Second")
            .by("Ann")
            .no(2)],
    );
    let albums = all_albums(&library);
    assert_eq!(titles(&albums), ["First", "Second"]);
    assert_eq!(album_of(&library, "/music/A/01.flac"), before);

    // And back: the albums merge, and the empty one is gone.
    add(&library, root, &tracks[1..]);
    let albums = all_albums(&library);
    assert_eq!(titles(&albums), ["First"]);
    assert_eq!(albums[0].track_count, 2);
    assert_eq!(count(&library, "albums"), 1);

    // Another album artist moves the whole album.
    add(
        &library,
        root,
        &[
            T::new("/music/A/01.flac", "One", "Ann", "First")
                .by("Bo")
                .no(1),
            T::new("/music/A/02.flac", "Two", "Ann", "First")
                .by("Bo")
                .no(2),
        ],
    );
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].artist.as_deref(), Some("Bo"));
    assert_eq!(count(&library, "albums"), 1);
    // Ann is still a track artist; she is no album artist any more.
    let album_artists = library
        .artists(&ArtistQuery::new(ArtistRole::AlbumArtist))
        .unwrap();
    assert_eq!(album_artists.total, 1);

    // A third artist joining a folder without album artists makes it a compilation,
    // and leaving it makes it Ann's again.
    let (library, root) = support::library();
    add(
        &library,
        root,
        &[
            T::new("/music/S/01.flac", "1", "Ann", "S").no(1),
            T::new("/music/S/02.flac", "2", "Ann", "S").no(2),
        ],
    );
    assert_eq!(all_albums(&library)[0].artist.as_deref(), Some("Ann"));
    add(
        &library,
        root,
        &[
            T::new("/music/S/03.flac", "3", "Bo", "S").no(3),
            T::new("/music/S/04.flac", "4", "Cy", "S").no(4),
        ],
    );
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1, "{albums:?}");
    assert!(albums[0].is_compilation && albums[0].artist.is_none());
    library.remove_file("/music/S/03.flac").unwrap();
    library.remove_file("/music/S/04.flac").unwrap();
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1, "{albums:?}");
    assert_eq!(albums[0].artist.as_deref(), Some("Ann"));
    assert_eq!(count(&library, "albums"), 1);
}

#[test]
fn a_removed_copy_merges_the_split_albums_again() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/a/01.flac", "One", "Ann", "Album")
                .by("Ann")
                .no(1),
            T::new("/music/b/01.flac", "One", "Ann", "Album")
                .by("Ann")
                .no(1),
            T::new("/music/b/02.flac", "Two", "Ann", "Album")
                .by("Ann")
                .no(2),
        ],
    );
    assert_eq!(all_albums(&library).len(), 2);
    library.remove_file("/music/a/01.flac").unwrap();
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].track_count, 2);
    let key: String = library
        .database()
        .read(|c| c.query_row("SELECT album_key FROM albums", [], |r| r.get(0)))
        .unwrap();
    assert!(!key.contains("\u{1f}d"), "{key:?}");
}

#[test]
fn orphans_are_removed() {
    let (library, root) = library();
    let mut r = T::new("/music/o/01.flac", "One", "Ann", "Gone")
        .by("Ann")
        .record(root);
    r.tags.genres = vec!["Polka".into(), "Jazz".into()];
    r.tags.composers = vec!["Cy".into()];
    library.upsert_track(r.clone()).unwrap();
    let mut keep = T::new("/music/k/01.flac", "Kept", "Bo", "Kept").record(root);
    keep.tags.genres = vec!["Jazz".into()];
    library.upsert_track(keep).unwrap();
    assert_eq!(count(&library, "people"), 3);
    assert_eq!(count(&library, "genres"), 2);

    // A tag change drops a genre and a composer.
    r.tags.genres = vec!["Jazz".into()];
    r.tags.composers.clear();
    library.upsert_track(r).unwrap();
    assert_eq!(count(&library, "genres"), 1);
    assert_eq!(count(&library, "people"), 2);
    let people_before: i64 = library
        .database()
        .read(|c| c.query_row("SELECT id FROM people WHERE name = 'Ann'", [], |r| r.get(0)))
        .unwrap();

    // A rewrite with the same tags keeps the people's rows.
    add(
        &library,
        root,
        &[T::new("/music/o/01.flac", "One", "Ann", "Gone").by("Ann")],
    );
    let people_after: i64 = library
        .database()
        .read(|c| c.query_row("SELECT id FROM people WHERE name = 'Ann'", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(people_before, people_after);

    library.remove_file("/music/o/01.flac").unwrap();
    assert_eq!(count(&library, "albums"), 1);
    assert_eq!(count(&library, "people"), 1);
    assert_eq!(count(&library, "genres"), 1);
    assert_eq!(count(&library, "album_people"), 1);
    let track = library
        .track_at(std::path::Path::new("/music/k/01.flac"), 0)
        .unwrap()
        .unwrap();
    library.remove_track(track).unwrap();
    for table in ["albums", "people", "genres", "album_people", "track_people"] {
        assert_eq!(count(&library, table), 0, "{table}");
    }
}

#[test]
fn moving_a_file_regroups_it() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/a/01.flac", "One", "Ann", "Album")
                .by("Ann")
                .no(1),
            T::new("/music/b/01.flac", "One", "Ann", "Album")
                .by("Ann")
                .no(1),
        ],
    );
    assert_eq!(all_albums(&library).len(), 2);
    // Into a disc folder of the other copy: still two releases, now one folder.
    library
        .move_file(root, "/music/a/01.flac", "/music/b/CD2/01.flac")
        .unwrap();
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1, "{albums:?}");
    assert_eq!(albums[0].track_count, 2);
}

fn named_albums(library: &Library, titles: &[&str]) {
    let root = library.add_root("/music").unwrap();
    for (i, title) in titles.iter().enumerate() {
        let path = format!("/music/{i}/01.flac");
        library
            .upsert_track(T::new(&path, "t", "Ann", title).by("Ann").record(root))
            .unwrap();
    }
}

fn sorted_titles(locale: &str, input: &[&str]) -> Vec<String> {
    let config = DatabaseConfig::in_memory().with_collation(LocaleCollation::new(locale).unwrap());
    let library = Library::open(&config).unwrap();
    named_albums(&library, input);
    titles(&all_albums(&library))
}

#[test]
fn album_lists_sort_in_the_locale_order() {
    let input = ["Öde", "Zebra", "Ärlig", "Ångest", "apa", "Ödla", "Bära"];
    assert_eq!(
        sorted_titles("sv-SE", &input),
        ["apa", "Bära", "Zebra", "Ångest", "Ärlig", "Öde", "Ödla"]
    );
    assert_eq!(
        sorted_titles("en", &input),
        ["Ångest", "apa", "Ärlig", "Bära", "Öde", "Ödla", "Zebra"]
    );
    let estonian = ["Zoo", "Šaak", "Tee", "Õun", "Saar", "Ülo", "Wõru", "Xenon"];
    assert_eq!(
        sorted_titles("et", &estonian),
        ["Saar", "Šaak", "Zoo", "Tee", "Wõru", "Õun", "Ülo", "Xenon"]
    );
}

#[test]
fn albums_sum_up_their_format_and_filter_by_it() {
    let (library, root) = library();
    // (album, codec, rate, depth) of each track.
    let tracks = [
        ("Lossy", "mp3", 44_100, None),
        ("Lossy", "mp3", 44_100, None),
        ("Cd", "flac", 44_100, Some(16)),
        ("Cd", "flac", 44_100, Some(16)),
        ("Studio", "flac", 96_000, Some(24)),
        ("Studio", "flac", 44_100, Some(16)),
        ("Mixed", "flac", 96_000, Some(24)),
        ("Mixed", "aac", 44_100, None),
    ];
    for (i, (album, codec, rate, depth)) in tracks.iter().enumerate() {
        let path = format!("/music/{album}/{i}.x");
        let mut r = T::new(&path, "t", "A", album)
            .by("A")
            .no(i as u32 + 1)
            .record(root);
        r.audio.codec = Some((*codec).to_owned());
        r.audio.sample_rate = Some(*rate);
        r.audio.bit_depth = *depth;
        r.audio.bitrate = Some(if depth.is_some() { 900_000 } else { 320_000 });
        library.upsert_track(r).unwrap();
    }
    let format = |title: &str| {
        all_albums(&library)
            .into_iter()
            .find(|a| a.title.as_deref() == Some(title))
            .and_then(|a| a.format)
            .unwrap()
    };
    let lossy = format("Lossy");
    assert_eq!(lossy.codec.as_deref(), Some("mp3"));
    assert!(!lossy.lossless && !lossy.hi_res);
    assert_eq!(lossy.bit_depth, None);
    assert_eq!(lossy.bitrate, Some(320_000));
    let cd = format("Cd");
    assert!(cd.lossless && !cd.hi_res);
    assert_eq!((cd.sample_rate, cd.bit_depth), (Some(44_100), Some(16)));
    let studio = format("Studio");
    assert!(studio.lossless && studio.hi_res);
    assert_eq!(
        (studio.sample_rate, studio.bit_depth),
        (Some(96_000), Some(24))
    );
    assert!(!format("Mixed").lossless);

    let with = |quality| {
        let page = library
            .albums(&AlbumQuery {
                quality,
                page: Page::first(100),
                ..AlbumQuery::default()
            })
            .unwrap();
        assert_eq!(page.items.len(), page.total);
        titles(&page.items)
    };
    assert_eq!(with(AlbumQuality::Any).len(), 4);
    assert_eq!(with(AlbumQuality::Lossless), ["Cd", "Studio"]);
    assert_eq!(with(AlbumQuality::HiRes), ["Studio"]);
    let detail = detail(&library, album_of(&library, "/music/Studio/4.x"));
    assert!(detail.summary.format.is_some_and(|f| f.hi_res));
}

#[test]
fn album_sorts_and_paging() {
    let (library, root) = library();
    let mut tracks = Vec::new();
    for i in 0..120 {
        let path = format!("/music/{i:03}/01.flac");
        let title = format!("Album {i:03}");
        let artist = format!("Artist {:02}", 119 - i);
        let mut r = T::new(&path, "t", &artist, &title)
            .by(&artist)
            .year(1900 + (i % 7))
            .no(1)
            .record(root);
        if i == 5 {
            r.tags.album_artists.clear();
            r.catalog.compilation = Some(true);
        }
        tracks.push(r);
    }
    library.upsert_tracks(tracks).unwrap();

    let page = |sort, offset, limit| {
        library
            .albums(&AlbumQuery {
                sort,
                page: Page::new(offset, limit),
                ..AlbumQuery::default()
            })
            .unwrap()
    };
    let first = page(AlbumSort::Title, 0, 50);
    let second = page(AlbumSort::Title, 50, 50);
    let third = page(AlbumSort::Title, 100, 50);
    assert_eq!(first.total, 120);
    assert_eq!(
        (first.items.len(), second.items.len(), third.items.len()),
        (50, 50, 20)
    );
    let all: Vec<String> = [first, second, third]
        .iter()
        .flat_map(|p| titles(&p.items))
        .collect();
    let expected: Vec<String> = (0..120).map(|i| format!("Album {i:03}")).collect();
    assert_eq!(all, expected);
    assert!(page(AlbumSort::Title, 200, 50).items.is_empty());

    // By artist: Artist 00 is album 119; the compilation without artist comes last.
    let by_artist = page(AlbumSort::Artist, 0, 1000);
    assert_eq!(by_artist.items[0].title.as_deref(), Some("Album 119"));
    assert_eq!(by_artist.items[119].title.as_deref(), Some("Album 005"));
    // By year, then title.
    let by_year = page(AlbumSort::Year, 0, 3);
    assert_eq!(
        titles(&by_year.items),
        ["Album 000", "Album 007", "Album 014"]
    );
    // Newest first.
    let added = page(AlbumSort::Added, 0, 1);
    assert_eq!(added.items.len(), 1);
}

#[test]
fn genres_and_their_tracks() {
    let (library, root) = library();
    for (i, genre) in ["Rock", "rock", "Jazz", "Ägyptisch"].iter().enumerate() {
        let path = format!("/music/g/{i}.flac");
        let mut r = T::new(&path, "t", "Ann", "G")
            .by("Ann")
            .no(u32::try_from(i).unwrap() + 1)
            .record(root);
        r.tags.genres = vec![(*genre).to_owned()];
        library.upsert_track(r).unwrap();
    }
    let genres = library.genres(&GenreQuery::default()).unwrap();
    let names: Vec<_> = genres
        .items
        .iter()
        .map(|g| (g.name.as_str(), g.track_count))
        .collect();
    // `Rock` and `rock` are one genre.
    assert_eq!(names, [("Ägyptisch", 1), ("Jazz", 1), ("Rock", 2)]);
    let rock = genres.items[2].id;
    let tracks = library
        .tracks(&TrackQuery {
            filter: TrackFilter::Genre(rock),
            ..TrackQuery::default()
        })
        .unwrap();
    assert_eq!(tracks.total, 2);
    let albums = albums_where(&library, AlbumFilter::Genre(rock));
    assert_eq!(albums.len(), 1);
    let detail = detail(&library, albums[0].id);
    assert_eq!(detail.genres[0].name, "Rock");
}

#[test]
fn tracks_sort_by_format_rating_and_plays() {
    use std::time::{Duration, SystemTime};

    use helikodu_library::{Listen, ListenEnd, PlayRules, Rating};

    let (library, root) = library();
    /// Path, title, codec, rate, depth, half stars, plays.
    type Row<'a> = (&'a str, &'a str, &'a str, u32, Option<u16>, Option<u8>, u32);
    let rows: [Row<'_>; 4] = [
        ("/music/s/1.flac", "A", "flac", 96_000, Some(24), Some(6), 1),
        ("/music/s/2.mp3", "B", "mp3", 44_100, None, None, 3),
        (
            "/music/s/3.flac",
            "C",
            "flac",
            44_100,
            Some(16),
            Some(10),
            0,
        ),
        ("/music/s/4.opus", "D", "opus", 48_000, None, Some(2), 2),
    ];
    for (path, title, codec, rate, depth, stars, plays) in rows {
        let mut record = T::new(path, title, "Ann", "Mixed").by("Ann").record(root);
        record.audio.codec = Some(codec.to_owned());
        record.audio.sample_rate = Some(rate);
        record.audio.bit_depth = depth;
        let track = library.upsert_track(record).unwrap().track;
        library
            .set_rating(track, stars.and_then(Rating::from_half_stars))
            .unwrap();
        for _ in 0..plays {
            let listen = Listen {
                started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1000),
                played: Duration::from_secs(200),
                end: ListenEnd::Finished,
                position: None,
                context: None,
            };
            library
                .record_listen(track, listen, PlayRules::default())
                .unwrap();
        }
    }
    let titles = |sort: Vec<TrackOrder>| -> Vec<String> {
        library
            .tracks(&TrackQuery {
                sort,
                ..TrackQuery::default()
            })
            .unwrap()
            .items
            .into_iter()
            .map(|t| t.title.unwrap())
            .collect()
    };
    assert_eq!(
        titles(vec![TrackOrder::ascending(TrackSortKey::Format)]),
        ["C", "A", "B", "D"],
        "by codec, then sample rate"
    );
    assert_eq!(
        titles(vec![TrackOrder::descending(TrackSortKey::Rating)]),
        ["C", "A", "D", "B"],
        "unrated last"
    );
    assert_eq!(
        titles(vec![TrackOrder::ascending(TrackSortKey::Rating)]),
        ["D", "A", "C", "B"],
        "unrated last in either direction"
    );
    assert_eq!(
        titles(vec![TrackOrder::descending(TrackSortKey::Plays)]),
        ["B", "D", "A", "C"]
    );
    assert_eq!(
        titles(vec![
            TrackOrder::ascending(TrackSortKey::Format),
            TrackOrder::descending(TrackSortKey::Title),
        ]),
        ["C", "A", "B", "D"]
    );
    assert_eq!(
        titles(vec![TrackOrder::descending(TrackSortKey::Title)]),
        ["D", "C", "B", "A"]
    );
}

#[test]
fn tracks_of_an_album_and_an_artist() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/a/02.flac", "Two", "Ann", "Old")
                .by("Ann")
                .year(1990)
                .no(2),
            T::new("/music/a/01.flac", "One", "Ann", "Old")
                .by("Ann")
                .year(1990)
                .no(1),
            T::new("/music/b/01.flac", "New", "Ann", "New")
                .by("Ann")
                .year(2000)
                .no(1),
            T::new("/music/c/01.flac", "Guest", "Ann", "Theirs")
                .by("Bo")
                .year(1980)
                .no(1),
        ],
    );
    let ann = person(&library, ArtistRole::Artist, "Ann");
    let tracks = library
        .tracks(&TrackQuery {
            filter: TrackFilter::Artist(ann, None),
            ..TrackQuery::default()
        })
        .unwrap();
    let order: Vec<_> = tracks
        .items
        .iter()
        .map(|t| t.title.clone().unwrap())
        .collect();
    assert_eq!(order, ["Guest", "One", "Two", "New"]);
    let own = library
        .tracks(&TrackQuery {
            filter: TrackFilter::Artist(ann, Some(ArtistRole::AlbumArtist)),
            ..TrackQuery::default()
        })
        .unwrap();
    assert_eq!(own.total, 3);
    let old = album_of(&library, "/music/a/01.flac");
    let paged = library
        .tracks(&TrackQuery {
            filter: TrackFilter::Album(old),
            page: Page::new(1, 1),
            ..TrackQuery::default()
        })
        .unwrap();
    assert_eq!(paged.total, 2);
    assert_eq!(paged.items[0].title.as_deref(), Some("Two"));
    assert_eq!(paged.items[0].album_title.as_deref(), Some("Old"));
}

#[test]
fn the_order_of_whole_lists_matches_their_pages() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/z/02.flac", "Two", "Ann", "Old")
                .by("Ann")
                .year(1990)
                .no(2),
            T::new("/music/z/01.flac", "One", "Ann", "Old")
                .by("Ann")
                .year(1990)
                .no(1),
            T::new("/music/b/x.flac", "Untracked", "Ann", "Old")
                .by("Ann")
                .year(1990),
            T::new("/music/a/01.flac", "Disc two", "Ann", "Set")
                .by("Ann")
                .year(1975)
                .disc(2)
                .no(1),
            T::new("/music/a/02.flac", "Disc one", "Ann", "Set")
                .by("Ann")
                .year(1975)
                .disc(1)
                .no(1),
            T::new("/music/c/01.flac", "Guest", "Ann", "Theirs")
                .by("Bo")
                .year(1980)
                .no(1),
            T::new("/music/d/01.flac", "Undated", "Ann", "Someday")
                .by("Bo")
                .no(1),
            T::new("/music/e/01.flac", "Nobody's", "Ann", "Anonymous").no(1),
        ],
    );
    let ann = person(&library, ArtistRole::Artist, "Ann");
    for filter in [TrackFilter::All, TrackFilter::Artist(ann, None)] {
        for sort in [vec![], vec![TrackOrder::descending(TrackSortKey::Title)]] {
            let query = TrackQuery {
                filter,
                sort,
                ..TrackQuery::default()
            };
            let pages: Vec<_> = library
                .tracks(&query)
                .unwrap()
                .items
                .iter()
                .map(|t| (t.id, t.album))
                .collect();
            assert_eq!(pages.len(), 8);
            assert_eq!(library.track_order(&query).unwrap(), pages, "{filter:?}");
        }
    }
}

#[test]
fn masked_folders_are_hidden() {
    let (library, root) = library();
    let usb = library.add_root("/media/usb").unwrap();
    add(
        &library,
        root,
        &[T::new("/music/a/01.flac", "Home", "Ann", "Home")
            .by("Ann")
            .no(1)],
    );
    let mut away = T::new("/media/usb/b/01.flac", "Away", "Bo", "Away")
        .by("Bo")
        .no(1)
        .record(usb);
    away.tags.genres = vec!["Ska".into()];
    library.upsert_track(away).unwrap();
    add(
        &library,
        usb,
        &[T::new("/media/usb/b/02.flac", "Away 2", "Ann", "Away")
            .by("Bo")
            .no(2)],
    );
    assert_eq!(all_albums(&library).len(), 2);

    library.set_root_masked(usb, true).unwrap();
    let states = library.root_states().unwrap();
    let state = states.iter().find(|s| s.id == usb).unwrap();
    assert!(state.masked && state.mask_when_missing && state.masked_since.is_some());
    assert_eq!(titles(&all_albums(&library)), ["Home"]);
    let away_id = albums_where(&library, AlbumFilter::All);
    assert_eq!(away_id.len(), 1);
    let with_masked = library
        .albums(&AlbumQuery {
            include_masked: true,
            ..AlbumQuery::default()
        })
        .unwrap();
    assert_eq!(with_masked.total, 2);
    let away_album = with_masked
        .items
        .iter()
        .find(|a| a.title.as_deref() == Some("Away"))
        .unwrap()
        .id;
    assert!(library.album(away_album).unwrap().is_none());
    assert_eq!(
        library
            .album_including_masked(away_album)
            .unwrap()
            .unwrap()
            .summary
            .track_count,
        2
    );
    // Bo is only on the masked folder; Ann is on both, with one track left.
    let artists = library
        .artists(&ArtistQuery::new(ArtistRole::Artist))
        .unwrap();
    let names: Vec<_> = artists
        .items
        .iter()
        .map(|a| (a.name.as_str(), a.track_count))
        .collect();
    assert_eq!(names, [("Ann", 1)]);
    assert_eq!(library.genres(&GenreQuery::default()).unwrap().total, 0);
    let ann = person(&library, ArtistRole::Artist, "Ann");
    let tracks = library
        .tracks(&TrackQuery {
            filter: TrackFilter::Artist(ann, None),
            ..TrackQuery::default()
        })
        .unwrap();
    assert_eq!(tracks.total, 1);

    // Shown while masked when the folder says so.
    library.set_mask_when_missing(usb, false).unwrap();
    assert_eq!(all_albums(&library).len(), 2);
    library.set_mask_when_missing(usb, true).unwrap();
    assert_eq!(all_albums(&library).len(), 1);
    library.set_root_masked(usb, false).unwrap();
    assert_eq!(all_albums(&library).len(), 2);
    assert!(
        library
            .root_states()
            .unwrap()
            .iter()
            .all(|s| !s.masked && s.masked_since.is_none())
    );
}

#[test]
fn a_track_that_loses_its_album_title_leaves_the_album() {
    let (library, root) = library();
    add(
        &library,
        root,
        &[
            T::new("/music/a/01.flac", "One", "Ann", "Album")
                .by("Ann")
                .no(1),
            T::new("/music/a/02.flac", "Two", "Ann", "Album")
                .by("Ann")
                .no(2),
        ],
    );
    let mut untitled = T::new("/music/a/02.flac", "Two", "Ann", "x")
        .by("Ann")
        .no(2)
        .record(root);
    untitled.tags.album = None;
    library.upsert_track(untitled).unwrap();
    let albums = all_albums(&library);
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].track_count, 1);
    let loose: i64 = library
        .database()
        .read(|c| {
            c.query_row(
                "SELECT count(*) FROM tracks WHERE album_id IS NULL AND album_group IS NULL",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(loose, 1);
    library.remove_file("/music/a/01.flac").unwrap();
    assert_eq!(count(&library, "albums"), 0);
}

/// A track's path with its album's key, title, artist, compilation flag and year.
type AlbumRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    bool,
    Option<i64>,
);

/// Every track's [`AlbumRow`], in path order.
fn album_rows(library: &Library) -> Vec<AlbumRow> {
    library
        .database()
        .read(|c| {
            let mut statement = c.prepare(
                "SELECT d.path_text || '/' || f.name_text, a.album_key, a.title, a.artist,
                        a.is_compilation, a.year
                 FROM tracks t JOIN files f ON f.id = t.file_id JOIN dirs d ON d.id = f.dir_id
                 LEFT JOIN albums a ON a.id = t.album_id
                 ORDER BY 1, t.sub_index",
            )?;
            let rows = statement.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get(2)?,
                    r.get(3)?,
                    r.get::<_, Option<bool>>(4)?.unwrap_or(false),
                    r.get(5)?,
                ))
            })?;
            rows.collect()
        })
        .unwrap()
}

#[test]
fn a_batch_groups_albums_as_one_write_at_a_time_does() {
    let tracks = |root: RootId| -> Vec<TrackRecord> {
        let mut out = vec![
            // Discs in folders of their own.
            T::new("/music/Set/CD1/01.flac", "S1", "Ann", "Set")
                .by("Ann")
                .no(1)
                .record(root),
            T::new("/music/Set/CD2/01.flac", "S2", "Ann", "Set")
                .by("Ann")
                .no(1)
                .record(root),
            // A folder without album artists: three artists make a compilation.
            T::new("/music/Mix/01.flac", "M1", "Bo", "Mix")
                .no(1)
                .record(root),
            T::new("/music/Mix/02.flac", "M2", "Cy feat. Bo", "Mix")
                .no(2)
                .record(root),
            T::new("/music/Mix/03.flac", "M3", "Di", "Mix")
                .no(3)
                .record(root),
            // Two copies of one album: split by folder.
            T::new("/music/Copy1/01.flac", "C1", "Ed", "Copy")
                .by("Ed")
                .year(1999)
                .no(1)
                .record(root),
            T::new("/music/Copy2/01.flac", "C1", "Ed", "Copy")
                .by("Ed")
                .year(1999)
                .no(1)
                .record(root),
            // The year most tracks have.
            T::new("/music/Y/01.flac", "Y1", "Fay", "Y")
                .by("Fay")
                .year(2001)
                .no(1)
                .record(root),
            T::new("/music/Y/02.flac", "Y2", "Fay", "Y")
                .by("Fay")
                .year(2002)
                .no(2)
                .record(root),
            T::new("/music/Y/03.flac", "Y3", "Fay", "Y")
                .by("Fay")
                .year(2002)
                .no(3)
                .record(root),
            // No album title.
            T::new("/music/Loose/01.flac", "L", "Gus", "").record(root),
        ];
        // A MusicBrainz release across two folders.
        for (path, no) in [("/music/MB/a.flac", 1), ("/music/Elsewhere/b.flac", 2)] {
            let mut r = T::new(path, "MB", "Hal", "Tagged differently")
                .no(no)
                .record(root);
            r.catalog.release_mbid = Some("0b1c2d3e-0000-4000-8000-000000000001".into());
            out.push(r);
        }
        out[10].tags.album = None;
        out
    };

    let (one_by_one, root_a) = library();
    for record in tracks(root_a) {
        one_by_one.upsert_track(record).unwrap();
    }
    let (batch, root_b) = library();
    batch.upsert_tracks(tracks(root_b)).unwrap();
    let expected = album_rows(&one_by_one);
    assert_eq!(album_rows(&batch), expected);
    assert_eq!(count(&batch, "albums"), count(&one_by_one, "albums"));

    // A second batch that retitles, re-artists and moves tracks between albums.
    let changes = |root: RootId| -> Vec<TrackRecord> {
        vec![
            T::new("/music/Mix/02.flac", "M2", "Bo", "Mix")
                .no(2)
                .record(root),
            T::new("/music/Copy2/01.flac", "C1", "Ed", "Copy (Live)")
                .by("Ed")
                .year(1999)
                .no(1)
                .record(root),
            T::new("/music/Y/01.flac", "Y1", "Fay", "Y")
                .by("Fay")
                .year(2002)
                .no(1)
                .record(root),
            T::new("/music/Loose/01.flac", "L", "Gus", "Set")
                .by("Ann")
                .no(2)
                .record(root),
        ]
    };
    for record in changes(root_a) {
        one_by_one.upsert_track(record).unwrap();
    }
    batch.upsert_tracks(changes(root_b)).unwrap();
    let expected = album_rows(&one_by_one);
    assert_eq!(album_rows(&batch), expected);
    assert_eq!(count(&batch, "albums"), count(&one_by_one, "albums"));
    // The retitled copy no longer splits the other.
    assert!(
        expected
            .iter()
            .any(|row| row.0 == "/music/Copy1/01.flac" && row.2.as_deref() == Some("Copy"))
    );
}
