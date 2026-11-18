// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! How writes behave: ID3v2 versions, stale files, in-place writes, links, the
//! rating frames, and batches over a selection with mixed values.

use std::cell::RefCell;
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

use helikodu_meta::tags::rating::Rating;
use helikodu_meta::tags::writing::{
    BatchItem, FieldKey, Id3Version, InPlaceReason, SelectionView, Shared, TagChanges, TagField,
    WriteError, WriteMode, WriteOptions, field_values, load_for_editing, write_batch, write_tags,
};
use helikodu_meta::tags::{ReadOptions, read_tags};
use lofty::config::ParseOptions;
use lofty::file::AudioFile;
use lofty::id3::v2::Frame;
use lofty::mpeg::MpegFile;

use crate::media::{Format, WRITABLE, encode, entries, folder, metadata};

const MP3: Format = WRITABLE[0];
const FLAC: Format = WRITABLE[2];

fn id3_version_byte(path: &Path) -> u8 {
    let bytes = fs::read(path).expect("read");
    assert_eq!(
        &bytes[..3],
        b"ID3",
        "{} starts with an ID3v2 tag",
        path.display()
    );
    bytes[3]
}

fn title(path: &Path) -> Option<String> {
    read_tags(path, &ReadOptions::default())
        .expect("reads")
        .tags
        .title
}

fn set_title(value: &str) -> TagChanges {
    let mut c = TagChanges::new();
    c.set_one(TagField::Title, value);
    c
}

#[test]
fn new_mp3_tags_are_id3v24_unless_id3v23_is_chosen() {
    let dir = folder("id3-new");
    let no_tag = ["-id3v2_version", "0", "-write_xing", "0"];
    let Some(a) = encode(&dir, "a.mp3", MP3, &no_tag) else {
        return;
    };
    let b = encode(&dir, "b.mp3", MP3, &no_tag).expect("encoded");
    assert_ne!(
        &fs::read(&a).expect("read")[..3],
        b"ID3",
        "no tag to begin with"
    );

    let outcome =
        write_tags(&a, &set_title("Four"), None, &WriteOptions::default()).expect("written");
    assert_eq!(outcome.id3_version, Some(Id3Version::V2_4));
    assert_eq!(id3_version_byte(&a), 4);

    let v23 = WriteOptions {
        id3_version: Id3Version::V2_3,
        ..WriteOptions::default()
    };
    let mut c = set_title("Three");
    c.set_one(TagField::ArtistSort, "Sorted, The")
        .set_one(TagField::Date, "2004-07-01")
        .set(TagField::Genre, ["Jazz", "Soul"])
        .set(TagField::Artist, ["One", "Two"])
        .set(TagField::Performer, ["Pat (guitar)"])
        .set_one(TagField::Producer, "Prod")
        .set_one(TagField::Mood, "Calm");
    let outcome = write_tags(&b, &c, None, &v23).expect("written");
    assert_eq!(outcome.id3_version, Some(Id3Version::V2_3));
    assert_eq!(id3_version_byte(&b), 3);
    let tags = read_tags(&b, &ReadOptions::default()).expect("reads").tags;
    assert_eq!(tags.title.as_deref(), Some("Three"));
    assert_eq!(tags.artist_sort.as_deref(), Some("Sorted, The"));
    assert_eq!(tags.year(), Some(2004));
    assert_eq!(tags.genres, ["Jazz", "Soul"]);
    assert_eq!(tags.artists, ["One", "Two"]);
    let key = |f: TagField| field_values(&tags, &FieldKey::Known(f));
    assert_eq!(key(TagField::Performer), ["Pat (guitar)"]);
    assert_eq!(key(TagField::Producer), ["Prod"]);
    assert_eq!(key(TagField::Mood), ["Calm"]);
}

#[test]
fn an_existing_id3v23_tag_keeps_its_version() {
    let dir = folder("id3-keep");
    let args: Vec<String> = ["-id3v2_version", "3"]
        .into_iter()
        .map(str::to_owned)
        .chain(metadata(&[("title", "Old"), ("artist", "Someone")]))
        .collect();
    let Some(path) = encode(&dir, "a.mp3", MP3, &args) else {
        return;
    };
    assert_eq!(id3_version_byte(&path), 3);
    let outcome =
        write_tags(&path, &set_title("New"), None, &WriteOptions::default()).expect("written");
    assert_eq!(outcome.id3_version, Some(Id3Version::V2_3));
    assert_eq!(id3_version_byte(&path), 3);
    let tags = read_tags(&path, &ReadOptions::default())
        .expect("reads")
        .tags;
    assert_eq!(tags.title.as_deref(), Some("New"));
    assert_eq!(tags.artist.as_deref(), Some("Someone"));
}

#[test]
fn a_file_changed_since_it_was_read_is_refused() {
    let dir = folder("stale");
    let Some(path) = encode(&dir, "a.flac", FLAC, &metadata(&[("title", "Old")])) else {
        return;
    };
    let loaded = load_for_editing(&path, &ReadOptions::default()).expect("loads");
    assert!(loaded.writable);
    assert_eq!(loaded.values(&TagField::Title.into()), ["Old"]);

    // Another program touches the file after the editor read it.
    let later = SystemTime::now() + Duration::from_secs(5);
    File::options()
        .write(true)
        .open(&path)
        .expect("open")
        .set_modified(later)
        .expect("set the time");
    let before = fs::read(&path).expect("read");
    let result = write_tags(
        &path,
        &set_title("New"),
        Some(loaded.stamp),
        &WriteOptions::default(),
    );
    assert!(
        matches!(result, Err(WriteError::ChangedSinceRead)),
        "{result:?}"
    );
    assert_eq!(fs::read(&path).expect("read"), before);

    // Reloaded, it can be written; the new stamp allows the next edit.
    let loaded = load_for_editing(&path, &ReadOptions::default()).expect("loads");
    let outcome = write_tags(
        &path,
        &set_title("New"),
        Some(loaded.stamp),
        &WriteOptions::default(),
    )
    .expect("written");
    write_tags(
        &path,
        &set_title("Newer"),
        Some(outcome.stamp),
        &WriteOptions::default(),
    )
    .expect("written again");
    assert_eq!(title(&path).as_deref(), Some("Newer"));
}

#[test]
fn files_above_the_threshold_are_written_in_place() {
    let dir = folder("in-place");
    let Some(path) = encode(&dir, "a.flac", FLAC, &metadata(&[("title", "Old")])) else {
        return;
    };
    let inode = |p: &Path| std::os::unix::fs::MetadataExt::ino(&fs::metadata(p).expect("stat"));
    let before = inode(&path);
    let tiny = WriteOptions {
        in_place_above: 1024,
        ..WriteOptions::default()
    };
    let outcome = write_tags(&path, &set_title("In place"), None, &tiny).expect("written");
    assert_eq!(outcome.mode, WriteMode::InPlace(InPlaceReason::Large));
    assert_eq!(inode(&path), before, "the same file, not a replacement");
    assert_eq!(title(&path).as_deref(), Some("In place"));
    assert_eq!(entries(&dir), ["a.flac"]);

    // Below the threshold the file is replaced by a new one.
    let outcome =
        write_tags(&path, &set_title("Copied"), None, &WriteOptions::default()).expect("written");
    assert_eq!(outcome.mode, WriteMode::Replaced);
    assert_ne!(inode(&path), before);
}

#[test]
fn links_are_kept() {
    let dir = folder("links");
    let Some(path) = encode(&dir, "a.flac", FLAC, &metadata(&[("title", "Old")])) else {
        return;
    };
    let hard = dir.join("hard.flac");
    fs::hard_link(&path, &hard).expect("link");
    let outcome =
        write_tags(&hard, &set_title("Linked"), None, &WriteOptions::default()).expect("written");
    assert_eq!(outcome.mode, WriteMode::InPlace(InPlaceReason::HardLinked));
    assert_eq!(
        title(&path).as_deref(),
        Some("Linked"),
        "both names see the change"
    );

    let soft = dir.join("soft.flac");
    std::os::unix::fs::symlink("a.flac", &soft).expect("symlink");
    fs::remove_file(&hard).expect("unlink");
    let outcome = write_tags(
        &soft,
        &set_title("Through the link"),
        None,
        &WriteOptions::default(),
    )
    .expect("written");
    assert_eq!(outcome.mode, WriteMode::Replaced);
    assert!(
        fs::symlink_metadata(&soft)
            .expect("stat")
            .file_type()
            .is_symlink()
    );
    assert_eq!(title(&path).as_deref(), Some("Through the link"));
}

#[test]
fn ratings_use_popm_fmps_and_rating() {
    let dir = folder("ratings");
    let Some(mp3) = encode(&dir, "a.mp3", MP3, &metadata(&[("title", "x")])) else {
        return;
    };
    let mut c = TagChanges::new();
    c.rating = Some(Rating::from_half_stars(8));
    write_tags(&mp3, &c, None, &WriteOptions::default()).expect("written");
    let parsed = MpegFile::read_from(&mut File::open(&mp3).expect("open"), ParseOptions::new())
        .expect("parses");
    let tag = parsed.id3v2().expect("an ID3v2 tag");
    let popm = tag
        .into_iter()
        .find_map(|f| match f {
            Frame::Popularimeter(p) => Some((p.email.to_string(), p.rating)),
            _ => None,
        })
        .expect("a POPM frame");
    assert_eq!(popm, ("Helikodu".to_owned(), 196));
    assert_eq!(tag.get_user_text("FMPS_Rating"), Some("0.8"));

    let flac = encode(&dir, "a.flac", FLAC, &metadata(&[("title", "x")])).expect("encoded");
    write_tags(&flac, &c, None, &WriteOptions::default()).expect("written");
    let tags = read_tags(&flac, &ReadOptions::default())
        .expect("reads")
        .tags;
    assert_eq!(tags.rating, Rating::from_half_stars(8));
    let read = lofty::flac::FlacFile::read_from(
        &mut File::open(&flac).expect("open"),
        ParseOptions::new(),
    )
    .expect("parses");
    let comments = read.vorbis_comments().expect("comments");
    assert_eq!(comments.get("FMPS_RATING"), Some("0.8"));
    assert_eq!(comments.get("RATING"), Some("80"));
}

#[test]
fn a_selection_shows_mixed_values_and_a_batch_writes_each_file() {
    let dir = folder("batch");
    let mut paths = Vec::new();
    for (i, genre) in [Some("Jazz"), Some("Jazz"), None].into_iter().enumerate() {
        let title = format!("Track {i}");
        let mut pairs = vec![("title", title.as_str()), ("album", "Shared")];
        if let Some(genre) = genre {
            pairs.push(("genre", genre));
        }
        let Some(path) = encode(&dir, &format!("{i}.flac"), FLAC, &metadata(&pairs)) else {
            return;
        };
        paths.push(path);
    }
    let files: Vec<_> = paths
        .iter()
        .map(|p| load_for_editing(p, &ReadOptions::default()).expect("loads"))
        .collect();
    let view = SelectionView::of(&files);
    assert_eq!(view.files, 3);
    assert_eq!(view.writable, 3);
    assert_eq!(
        view.field(&TagField::Album.into()),
        Shared::Same(vec!["Shared".to_owned()])
    );
    assert_eq!(view.field(&TagField::Title.into()).distinct(), 3);
    assert_eq!(
        view.field(&TagField::Genre.into()),
        Shared::Mixed(vec![(vec!["Jazz".to_owned()], 2), (Vec::new(), 1)])
    );
    assert_eq!(
        view.field(&TagField::Composer.into()),
        Shared::Same(Vec::new())
    );
    assert_eq!(view.front_cover, Shared::Same(None));

    // The album changes for all; the mixed titles stay. The third file changes
    // under the editor and is refused; the others are still written.
    let mut changes = TagChanges::new();
    changes.set_one(TagField::Album, "Renamed");
    let items: Vec<_> = files
        .iter()
        .map(|f| BatchItem::for_file(f, changes.clone()))
        .collect();
    File::options()
        .write(true)
        .open(&paths[2])
        .expect("open")
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .expect("touch");
    let progress = RefCell::new(Vec::new());
    let report = write_batch(
        &items,
        &WriteOptions::default(),
        &mut |p| progress.borrow_mut().push((p.done, p.total, p.failed)),
        &|| false,
    );
    assert!(!report.cancelled);
    assert_eq!(report.written().count(), 2);
    let failed: Vec<_> = report.failed().collect();
    assert_eq!(failed.len(), 1);
    assert!(matches!(failed[0].1, WriteError::ChangedSinceRead));
    assert_eq!(
        progress.into_inner(),
        [(1, 3, false), (2, 3, false), (3, 3, true)]
    );
    for (i, path) in paths.iter().enumerate() {
        let tags = read_tags(path, &ReadOptions::default())
            .expect("reads")
            .tags;
        let album = field_values(&tags, &FieldKey::Known(TagField::Album));
        assert_eq!(album, [if i < 2 { "Renamed" } else { "Shared" }]);
        assert_eq!(tags.title, Some(format!("Track {i}")));
    }

    // Cancelled after the first file: the rest are not touched.
    let files: Vec<_> = paths
        .iter()
        .map(|p| load_for_editing(p, &ReadOptions::default()).expect("loads"))
        .collect();
    let mut changes = TagChanges::new();
    changes.set_one(TagField::Album, "Cancelled");
    let items: Vec<_> = files
        .iter()
        .map(|f| BatchItem::for_file(f, changes.clone()))
        .collect();
    let asked = std::cell::Cell::new(0);
    let report = write_batch(&items, &WriteOptions::default(), &mut |_| {}, &|| {
        asked.set(asked.get() + 1);
        asked.get() > 1
    });
    assert!(report.cancelled);
    assert!(report.results[0].1.is_ok());
    assert!(matches!(report.results[1].1, Err(WriteError::Cancelled)));
    assert!(matches!(report.results[2].1, Err(WriteError::Cancelled)));
    let album = |p: &Path| {
        read_tags(p, &ReadOptions::default())
            .expect("reads")
            .tags
            .album
    };
    assert_eq!(album(&paths[0]).as_deref(), Some("Cancelled"));
    assert_eq!(album(&paths[1]).as_deref(), Some("Renamed"));
}

#[test]
fn invalid_values_are_refused_before_the_file_is_touched() {
    let dir = folder("invalid");
    let Some(path) = encode(&dir, "a.flac", FLAC, &metadata(&[("title", "Old")])) else {
        return;
    };
    let before = fs::read(&path).expect("read");
    let mut c = TagChanges::new();
    c.set_one(TagField::Title, "Fine")
        .set_one(TagField::TrackNumber, "three");
    let result = write_tags(&path, &c, None, &WriteOptions::default());
    assert!(
        matches!(result, Err(WriteError::InvalidValue { .. })),
        "{result:?}"
    );
    assert_eq!(fs::read(&path).expect("read"), before);
}
