// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The undo and redo matrix of tag edits: each kind of change (a field set, added,
//! cleared, values that differ per file, the cover, the lyrics) is undone and done
//! again; steps go back and forward in order; a new edit drops what waited to be
//! done again; a file changed in between is left alone; and the library follows
//! every step.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "support/scanning.rs"]
mod support;

use std::path::{Path, PathBuf};

use helikodu_library::tag_editing::{FieldKey, read_lyrics};
use helikodu_library::{
    CancelToken, DatabaseConfig, Library, ScanOptions, StepProblem, TagStepReport, TagWriteSettings,
};
use helikodu_meta::covers::{CoverLimits, read_embedded_covers};
use helikodu_meta::tags::writing::{
    BatchItem, CoverChange, TagChanges, TagField, load_for_editing,
};
use helikodu_meta::tags::{ReadOptions, read_tags};
use helikodu_model::library::TrackId;
use support::{flac, folder, have_ffmpeg, worker_program};

/// A 1 × 1 red PNG.
const RED: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];
/// A 1 × 1 blue PNG.
const BLUE: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x60, 0xf8, 0x0f,
    0x00, 0x01, 0x03, 0x01, 0x00, 0x08, 0x89, 0xc2, 0xec, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

struct Setup {
    library: Library,
    options: ScanOptions,
    files: Vec<PathBuf>,
}

impl Setup {
    /// A library over `count` FLAC files titled "Old 1", "Old 2", … on the
    /// album "Album", with track numbers 9.
    fn new(name: &str, count: usize) -> Self {
        let root = folder(name).join("music");
        let files: Vec<PathBuf> = (1..=count)
            .map(|i| {
                let path = root.join(format!("{i:02} song.flac"));
                let title = format!("Old {i}");
                flac(
                    &path,
                    440 + 10 * u32::try_from(i).unwrap(),
                    1,
                    &[("title", &title), ("album", "Album"), ("tracknumber", "9")],
                );
                path
            })
            .collect();
        let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
        let mut options = ScanOptions::new([&root]);
        options.workers.program = Some(worker_program());
        options.workers.count = 1;
        library.scan(&options, None, &CancelToken::new()).unwrap();
        Self {
            library,
            options,
            files,
        }
    }

    fn edit(&self, changes: &[(usize, TagChanges)]) -> bool {
        let items: Vec<BatchItem> = changes
            .iter()
            .map(|(i, c)| {
                let file = load_for_editing(&self.files[*i], &ReadOptions::default()).unwrap();
                BatchItem::for_file(&file, c.clone())
            })
            .collect();
        let report = self
            .library
            .edit_tags(
                &items,
                &TagWriteSettings::default(),
                &self.options,
                &mut |_| {},
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(report.write.batch.failed().count(), 0);
        report.step.is_some()
    }

    fn undo(&self) -> Option<TagStepReport> {
        self.library
            .undo_tags(&TagWriteSettings::default(), &self.options)
            .unwrap()
    }

    fn redo(&self) -> Option<TagStepReport> {
        self.library
            .redo_tags(&TagWriteSettings::default(), &self.options)
            .unwrap()
    }

    fn values(&self, i: usize, field: TagField) -> Vec<String> {
        load_for_editing(&self.files[i], &ReadOptions::default())
            .unwrap()
            .values(&FieldKey::Known(field))
    }

    fn title(&self, i: usize) -> String {
        self.values(i, TagField::Title).join("; ")
    }

    fn db_title(&self, i: usize) -> Option<String> {
        let track: TrackId = self.library.track_at(&self.files[i], 0).unwrap().unwrap();
        let id = i64::try_from(track.get()).unwrap();
        self.library
            .database()
            .read(|c| c.query_row("SELECT title FROM tracks WHERE id = ?1", [id], |r| r.get(0)))
            .unwrap()
    }
}

fn title(text: &str) -> TagChanges {
    let mut c = TagChanges::new();
    c.set_one(TagField::Title, text);
    c
}

fn all_ok(report: &TagStepReport) {
    let failed: Vec<_> = report.failed().collect();
    assert!(failed.is_empty(), "{failed:?}");
}

fn front_cover(path: &Path) -> Option<Vec<u8>> {
    read_embedded_covers(path, &CoverLimits::default())
        .unwrap()
        .front()
        .map(|c| c.bytes().unwrap().to_vec())
}

#[test]
fn a_field_set_is_undone_and_done_again() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-set", 2);
    assert!(s.library.tag_history().unwrap().undo.is_none());
    assert!(s.undo().is_none(), "nothing to undo yet");
    assert!(s.edit(&[(0, title("New 1")), (1, title("New 2"))]));
    let history = s.library.tag_history().unwrap();
    assert_eq!(history.undo.as_ref().map(|s| s.files), Some(2));
    assert!(history.redo.is_none());
    assert_eq!(s.db_title(0).as_deref(), Some("New 1"));

    all_ok(&s.undo().unwrap());
    assert_eq!((s.title(0), s.title(1)), ("Old 1".into(), "Old 2".into()));
    assert_eq!(
        s.db_title(0).as_deref(),
        Some("Old 1"),
        "the library follows"
    );
    let history = s.library.tag_history().unwrap();
    assert!(history.undo.is_none());
    assert_eq!(history.redo.map(|s| s.files), Some(2));

    all_ok(&s.redo().unwrap());
    assert_eq!((s.title(0), s.title(1)), ("New 1".into(), "New 2".into()));
    assert_eq!(s.db_title(1).as_deref(), Some("New 2"));
    assert!(s.redo().is_none(), "nothing more to do again");
}

#[test]
fn fields_added_and_cleared_are_undone() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-add-clear", 1);
    let mut c = TagChanges::new();
    c.set(TagField::Genre, ["Folk", "Ambient"])
        .clear(TagField::Album)
        .set_one(TagField::Composer, "Mari Kask");
    assert!(s.edit(&[(0, c)]));
    assert_eq!(s.values(0, TagField::Genre), ["Folk", "Ambient"]);
    assert!(s.values(0, TagField::Album).is_empty());

    all_ok(&s.undo().unwrap());
    assert!(
        s.values(0, TagField::Genre).is_empty(),
        "the added field goes"
    );
    assert!(s.values(0, TagField::Composer).is_empty());
    assert_eq!(
        s.values(0, TagField::Album),
        ["Album"],
        "the cleared one is back"
    );

    all_ok(&s.redo().unwrap());
    assert_eq!(s.values(0, TagField::Genre), ["Folk", "Ambient"]);
    assert!(s.values(0, TagField::Album).is_empty());
}

#[test]
fn values_per_file_go_back_per_file() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-numbering", 3);
    let numbered: Vec<(usize, TagChanges)> = (0..3)
        .map(|i| {
            let mut c = TagChanges::new();
            c.set_one(TagField::TrackNumber, (i + 1).to_string())
                .set_one(TagField::TrackTotal, "3");
            (i, c)
        })
        .collect();
    assert!(s.edit(&numbered));
    assert_eq!(s.values(2, TagField::TrackNumber), ["3"]);
    all_ok(&s.undo().unwrap());
    for i in 0..3 {
        assert_eq!(s.values(i, TagField::TrackNumber), ["9"]);
        assert!(s.values(i, TagField::TrackTotal).is_empty());
    }
    all_ok(&s.redo().unwrap());
    for i in 0..3 {
        assert_eq!(s.values(i, TagField::TrackNumber), [(i + 1).to_string()]);
    }
}

#[test]
fn steps_go_back_and_forward_in_order() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-order", 1);
    assert!(s.edit(&[(0, title("A"))]));
    assert!(s.edit(&[(0, title("B"))]));
    assert!(s.edit(&[(0, title("C"))]));
    all_ok(&s.undo().unwrap());
    assert_eq!(s.title(0), "B");
    all_ok(&s.undo().unwrap());
    assert_eq!(s.title(0), "A");
    all_ok(&s.redo().unwrap());
    assert_eq!(s.title(0), "B");
    all_ok(&s.undo().unwrap());
    all_ok(&s.undo().unwrap());
    assert_eq!(s.title(0), "Old 1");
    assert!(s.undo().is_none());
    all_ok(&s.redo().unwrap());
    all_ok(&s.redo().unwrap());
    all_ok(&s.redo().unwrap());
    assert_eq!(s.title(0), "C");
    assert!(s.redo().is_none());
}

#[test]
fn a_new_edit_drops_what_waited_to_be_done_again() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-branch", 1);
    assert!(s.edit(&[(0, title("A"))]));
    assert!(s.edit(&[(0, title("B"))]));
    all_ok(&s.undo().unwrap());
    assert!(s.edit(&[(0, title("X"))]));
    assert!(s.library.tag_history().unwrap().redo.is_none());
    assert!(s.redo().is_none());
    all_ok(&s.undo().unwrap());
    assert_eq!(s.title(0), "A");
}

#[test]
fn an_edit_that_changes_nothing_is_not_a_step() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-nothing", 1);
    assert!(!s.edit(&[(0, title("Old 1"))]));
    assert!(s.library.tag_history().unwrap().undo.is_none());
}

#[test]
fn a_file_changed_since_is_left_alone() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-conflict", 2);
    assert!(s.edit(&[(0, title("New 1")), (1, title("New 2"))]));
    // Another program changes the first file's title.
    helikodu_meta::tags::write_tags(&s.files[0], &title("Elsewhere"), None, &Default::default())
        .unwrap();
    let report = s.undo().unwrap();
    let failed: Vec<_> = report.failed().collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, &s.files[0]);
    assert!(matches!(failed[0].1, StepProblem::Changed));
    assert_eq!(s.title(0), "Elsewhere", "left alone");
    assert_eq!(s.title(1), "Old 2", "the other file is undone");

    // Doing it again leaves the changed file alone too.
    let report = s.redo().unwrap();
    assert_eq!(report.failed().count(), 1);
    assert_eq!(s.title(0), "Elsewhere");
    assert_eq!(s.title(1), "New 2");
}

#[test]
fn a_later_step_on_the_same_file_must_be_undone_first() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-overlap", 2);
    assert!(s.edit(&[(0, title("A")), (1, title("A2"))]));
    assert!(s.edit(&[(0, title("B"))]));
    all_ok(&s.undo().unwrap());
    all_ok(&s.undo().unwrap());
    assert_eq!((s.title(0), s.title(1)), ("Old 1".into(), "Old 2".into()));
}

#[test]
fn the_cover_and_the_lyrics_are_undone() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-cover", 1);
    let path = &s.files[0];
    let mut c = TagChanges::new();
    c.cover = Some(CoverChange::SetFront(RED.to_vec()));
    c.lyrics = Some(Some("First line\nSecond line".into()));
    assert!(s.edit(&[(0, c)]));
    assert_eq!(front_cover(path).as_deref(), Some(RED));

    let mut c = TagChanges::new();
    c.cover = Some(CoverChange::SetFront(BLUE.to_vec()));
    c.lyrics = Some(None);
    assert!(s.edit(&[(0, c)]));
    assert_eq!(front_cover(path).as_deref(), Some(BLUE));
    assert_eq!(read_lyrics(path).unwrap(), None);

    all_ok(&s.undo().unwrap());
    assert_eq!(
        front_cover(path).as_deref(),
        Some(RED),
        "the old picture is back"
    );
    assert_eq!(
        read_lyrics(path).unwrap().as_deref(),
        Some("First line\nSecond line")
    );
    all_ok(&s.undo().unwrap());
    assert_eq!(front_cover(path), None);
    assert_eq!(read_lyrics(path).unwrap(), None);
    all_ok(&s.redo().unwrap());
    all_ok(&s.redo().unwrap());
    assert_eq!(front_cover(path).as_deref(), Some(BLUE));
}

#[test]
fn a_file_back_as_wanted_counts_as_done() {
    if !have_ffmpeg() {
        return;
    }
    let s = Setup::new("undo-already", 1);
    assert!(s.edit(&[(0, title("New"))]));
    // Someone else already put the old title back.
    helikodu_meta::tags::write_tags(&s.files[0], &title("Old 1"), None, &Default::default())
        .unwrap();
    all_ok(&s.undo().unwrap());
    assert_eq!(s.title(0), "Old 1");
    let tags = read_tags(&s.files[0], &ReadOptions::default()).unwrap();
    assert_eq!(tags.tags.title.as_deref(), Some("Old 1"));
}
