// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The completion under the search entry: while a field name is typed it lists
//! the fields that start with it (the name in mono with the typed letters in
//! the accent, what the field holds, an example); after a field and its
//! operator it lists the values of the fields that have few (kind, codec,
//! has, is). Tab completes, Up and Down choose, Escape closes. The entry keeps
//! the focus throughout.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, pango};
use helikodu_model::query::{Field, FieldKind, Flag, MediaKind};

use crate::i18n::tr;
use crate::widgets::label;

/// The most rows shown at once.
const MAX_ROWS: usize = 8;

/// One choice.
#[derive(Debug, Clone)]
struct Choice {
    /// What the row shows in mono.
    name: String,
    /// What it holds.
    description: String,
    /// An example, at the right.
    example: String,
    /// What replaces the typed word.
    insert: String,
}

/// What a field holds and an example of it.
fn describe(field: &Field) -> (String, &'static str) {
    match field {
        Field::Title => (tr("Track title"), "title:harbour"),
        Field::Artist => (tr("Track artist"), "artist:kask"),
        Field::AlbumArtist => (tr("Album artist"), "albumartist:okto"),
        Field::Album => (tr("Album title"), "album:\"winter light\""),
        Field::Composer => (tr("Composer"), "composer:pärt"),
        Field::Performer => (tr("Performer"), "performer:hahn"),
        Field::Conductor => (tr("Conductor"), "conductor:järvi"),
        Field::Lyricist => (tr("Lyricist"), "lyricist:kaplinski"),
        Field::Genre => (tr("Genre"), "genre:jazz"),
        Field::Work => (tr("Classical work"), "work:\"suite no. 1\""),
        Field::Movement => (tr("Movement name"), "movement:allegro"),
        Field::DiscSubtitle => (tr("Disc title"), "discsubtitle:live"),
        Field::ReleaseType => (tr("Release type"), "releasetype:live"),
        Field::Codec => (tr("Codec"), "codec:flac"),
        Field::Container => (tr("Container"), "container:ogg"),
        Field::Path => (tr("Whole path of the file"), "path:/music/jazz"),
        Field::Folder => (tr("Folder of the file"), "folder:incoming"),
        Field::Filename => (tr("Name of the file"), "filename:live"),
        Field::Kind => (tr("Music, audiobook, podcast or video"), "kind:audiobook"),
        Field::Year => (tr("Release year"), "year:1970..1979"),
        Field::OriginalYear => (tr("Year of the original release"), "originalyear<1980"),
        Field::Track => (tr("Track number"), "track:1"),
        Field::Disc => (tr("Disc number"), "disc:2"),
        Field::MovementNumber => (tr("Movement number"), "movementno:3"),
        Field::SampleRate => (tr("Sample rate in Hz"), "samplerate>=88200"),
        Field::BitDepth => (tr("Bits per sample"), "bitdepth:24"),
        Field::Channels => (tr("Number of channels"), "channels>2"),
        Field::Bitrate => (tr("Bit rate in kbit/s"), "bitrate<192"),
        Field::TrackGain => (tr("ReplayGain track gain in dB"), "trackgain<-10"),
        Field::AlbumGain => (tr("ReplayGain album gain in dB"), "albumgain<-10"),
        Field::Plays => (tr("How often it was played"), "plays=0"),
        Field::Skips => (tr("How often it was skipped"), "skips>3"),
        Field::Rating => (tr("Rating in stars"), "rating>=4"),
        Field::Duration => (tr("Length"), "duration<30s"),
        Field::Resume => (tr("Where listening stopped"), "resume>0"),
        Field::Size => (tr("Size of the file"), "size>100MB"),
        Field::Added => (tr("When the track came into the library"), "added<30d"),
        Field::Modified => (tr("When its tags last changed"), "modified<7d"),
        Field::LastPlayed => (tr("When it was last played"), "lastplayed>1y"),
        Field::FirstPlayed => (tr("When it was first played"), "firstplayed<30d"),
        Field::LastSkipped => (tr("When it was last skipped"), "lastskipped<7d"),
        _ => (tr("A tag"), "tag:mood=calm"),
    }
}

/// The word being typed at `cursor` (a byte offset) and where it starts.
fn word_at(text: &str, cursor: usize) -> (usize, &str) {
    let before = &text[..cursor.min(text.len())];
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '(' | ')' | '!'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    // A leading minus negates; it is not part of the name.
    let start = if before[start..].starts_with('-') {
        start + 1
    } else {
        start
    };
    (start, &before[start..])
}

/// The choices for `word`: fields by name, or values after a field and its
/// operator.
fn choices(word: &str) -> Vec<Choice> {
    if let Some((name, value)) = word.split_once([':', '=']) {
        let values: Vec<String> = match name.to_ascii_lowercase().as_str() {
            "kind" => MediaKind::ALL.iter().map(|k| k.name().to_owned()).collect(),
            "codec" | "format" => [
                "flac", "alac", "wav", "aiff", "wavpack", "ape", "mp3", "aac", "opus", "vorbis",
                "dsd",
            ]
            .map(str::to_owned)
            .to_vec(),
            "has" => ["lyrics", "cover", "rating", "genre", "composer", "year"]
                .map(str::to_owned)
                .to_vec(),
            "is" => Flag::ALL.iter().map(|f| f.name().to_owned()).collect(),
            _ => return Vec::new(),
        };
        let typed = value.to_lowercase();
        let op = &word[name.len()..=name.len()];
        return values
            .into_iter()
            .filter(|v| v.starts_with(&typed) && *v != typed)
            .map(|v| Choice {
                name: v.clone(),
                description: String::new(),
                example: format!("{name}{op}{v}"),
                insert: format!("{name}{op}{v} "),
            })
            .collect();
    }
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic()) {
        return Vec::new();
    }
    let typed = word.to_ascii_lowercase();
    let mut found: Vec<Choice> = Field::named()
        .filter_map(|field| {
            let name = field.name().into_owned();
            if !name.starts_with(&typed) || name == typed {
                return None;
            }
            let (description, example) = describe(&field);
            // A text field takes `:` at once; the others take a comparison.
            let insert = match field.kind() {
                FieldKind::Text | FieldKind::Kind => format!("{name}:"),
                _ => name.clone(),
            };
            Some(Choice {
                name,
                description,
                example: example.to_owned(),
                insert,
            })
        })
        .collect();
    for (name, description, example) in [
        ("has", tr("Has a value, lyrics or a cover"), "has:lyrics"),
        (
            "is",
            tr("Lossless, hi-res, compilation or loved"),
            "is:lossless",
        ),
        ("sort", tr("Order of the results"), "sort:added desc"),
        ("limit", tr("How many tracks to keep"), "limit:50"),
    ] {
        if name.starts_with(&typed) && name != typed {
            found.push(Choice {
                name: name.to_owned(),
                description,
                example: example.to_owned(),
                insert: format!("{name}:"),
            });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.truncate(MAX_ROWS);
    found
}

/// The completion popover of one entry.
pub(crate) struct QueryCompletion {
    entry: gtk::SearchEntry,
    popover: gtk::Popover,
    list: gtk::ListBox,
    choices: RefCell<Vec<Choice>>,
    /// The word and where it starts, while the popover is open.
    word: RefCell<(usize, String)>,
    /// The word Escape closed the popover on; it stays closed until it changes.
    dismissed: RefCell<Option<String>>,
    selected: Cell<usize>,
}

impl std::fmt::Debug for QueryCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryCompletion").finish_non_exhaustive()
    }
}

impl QueryCompletion {
    /// The completion of `entry`.
    pub(crate) fn new(entry: &gtk::SearchEntry) -> Rc<Self> {
        let popover = gtk::Popover::new();
        popover.add_css_class("hk-completion");
        popover.set_has_arrow(false);
        popover.set_autohide(false);
        popover.set_can_focus(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_halign(gtk::Align::Start);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let list = gtk::ListBox::new();
        list.add_css_class("hk-completion-list");
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_can_focus(false);
        list.update_property(&[gtk::accessible::Property::Label(&tr("Completions"))]);
        content.append(&list);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let hint = label(
            &tr("Tab completes · Up and Down choose · Esc closes"),
            &["hk-t3", "hk-completion-hint"],
        );
        hint.set_xalign(0.0);
        content.append(&hint);
        popover.set_child(Some(&content));
        popover.set_parent(entry);
        let completion = Rc::new(Self {
            entry: entry.clone(),
            popover,
            list,
            choices: RefCell::new(Vec::new()),
            word: RefCell::new((0, String::new())),
            dismissed: RefCell::new(None),
            selected: Cell::new(0),
        });
        let weak = Rc::downgrade(&completion);
        completion.list.connect_row_activated(move |_, row| {
            if let Some(completion) = weak.upgrade() {
                completion.selected.set(row.index().max(0) as usize);
                completion.complete();
            }
        });
        // The popover goes with the entry.
        let popover = completion.popover.downgrade();
        entry.connect_destroy(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.unparent();
            }
        });
        let weak = Rc::downgrade(&completion);
        entry.connect_has_focus_notify(move |entry| {
            if !entry.has_focus()
                && let Some(completion) = weak.upgrade()
            {
                completion.close();
            }
        });
        completion
    }

    /// Whether the popover is open.
    pub(crate) fn is_open(&self) -> bool {
        self.popover.is_visible()
    }

    /// Opens, fills or closes the popover for the word at the cursor.
    pub(crate) fn update(&self) {
        let text = self.entry.text();
        let cursor = usize::try_from(self.entry.position()).unwrap_or(0);
        let byte = text
            .char_indices()
            .nth(cursor)
            .map_or(text.len(), |(i, _)| i);
        let (start, word) = word_at(&text, byte);
        if self.dismissed.borrow().as_deref() == Some(word) {
            return;
        }
        self.dismissed.replace(None);
        // The focus widget of the window; the window need not be active.
        let typing = self.entry.is_focus()
            || self
                .entry
                .root()
                .and_then(|r| r.focus())
                .is_some_and(|f| f.is_ancestor(&self.entry));
        let found = if typing { choices(word) } else { Vec::new() };
        if found.is_empty() {
            self.close();
            return;
        }
        let typed = word
            .split_once([':', '='])
            .map_or(word.len(), |(_, value)| value.len());
        crate::widgets::remove_rows(&self.list);
        for choice in &found {
            self.list.append(&row(choice, typed));
        }
        self.word.replace((start, word.to_owned()));
        self.choices.replace(found);
        self.select(0);
        // As wide as the entry: its width less the popover's padding and border.
        if let Some(content) = self.popover.child() {
            content.set_size_request((self.entry.width() - 14).max(0), -1);
        }
        if !self.popover.is_visible() {
            self.popover.popup();
        }
    }

    fn select(&self, index: usize) {
        self.selected.set(index);
        if let Some(row) = self.list.row_at_index(index as i32) {
            self.list.select_row(Some(&row));
        }
    }

    /// Closes the popover.
    pub(crate) fn close(&self) {
        if self.popover.is_visible() {
            self.popover.popdown();
        }
    }

    /// Replaces the word with the chosen completion.
    fn complete(&self) {
        let Some(choice) = self.choices.borrow().get(self.selected.get()).cloned() else {
            return;
        };
        let (start, word) = self.word.borrow().clone();
        let text = self.entry.text().to_string();
        let end = (start + word.len()).min(text.len());
        let insert = choice.insert;
        let new = format!("{}{}{}", &text[..start], insert, &text[end..]);
        let cursor = text[..start].chars().count() + insert.chars().count();
        self.close();
        self.entry.set_text(&new);
        self.entry.set_position(cursor as i32);
        self.update();
    }

    /// Handles a key of the entry; whether it was used.
    pub(crate) fn key(&self, key: gdk::Key) -> bool {
        if !self.is_open() {
            return false;
        }
        let count = self.choices.borrow().len();
        match key {
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => self.complete(),
            gdk::Key::Down => self.select((self.selected.get() + 1).min(count.saturating_sub(1))),
            gdk::Key::Up => self.select(self.selected.get().saturating_sub(1)),
            gdk::Key::Escape => {
                self.dismissed.replace(Some(self.word.borrow().1.clone()));
                self.close();
            }
            _ => return false,
        }
        true
    }

    /// The popover (for screenshots).
    pub(crate) fn popover(&self) -> gtk::Popover {
        self.popover.clone()
    }

    /// The names listed (for tests).
    pub(crate) fn names(&self) -> Vec<String> {
        if !self.is_open() {
            return Vec::new();
        }
        self.choices
            .borrow()
            .iter()
            .map(|c| c.name.clone())
            .collect()
    }
}

/// A row: the name with the typed letters in the accent, the description, the
/// example.
fn row(choice: &Choice, typed: usize) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_can_focus(false);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let name = label(&choice.name, &["hk-mono", "hk-completion-name"]);
    let attributes = pango::AttrList::new();
    let accent = crate::style::current().accent.text;
    let mut color = pango::AttrColor::new_foreground(
        u16::from(accent.r) * 257,
        u16::from(accent.g) * 257,
        u16::from(accent.b) * 257,
    );
    color.set_start_index(0);
    color.set_end_index(u32::try_from(typed.min(choice.name.len())).unwrap_or(0));
    attributes.insert(color);
    name.set_attributes(Some(&attributes));
    name.set_width_chars(13);
    name.set_xalign(0.0);
    line.append(&name);
    let description = label(&choice.description, &["hk-t2"]);
    description.set_hexpand(true);
    description.set_xalign(0.0);
    description.set_ellipsize(pango::EllipsizeMode::End);
    line.append(&description);
    let example = label(
        &choice.example,
        &["hk-t3", "hk-mono", "hk-completion-example"],
    );
    example.set_xalign(1.0);
    line.append(&example);
    row.set_child(Some(&line));
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "{}, {}",
        choice.name, choice.description
    ))]);
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_complete_by_prefix_and_values_after_the_operator() {
        let names =
            |word: &str| -> Vec<String> { choices(word).into_iter().map(|c| c.name).collect() };
        assert_eq!(
            names("a"),
            ["added", "album", "albumartist", "albumgain", "artist"]
        );
        assert!(names("harbour").is_empty());
        assert!(names("artist").is_empty());
        assert_eq!(names("kind:au"), ["audiobook"]);
        assert_eq!(names("is:"), ["lossless", "hires", "compilation", "loved"]);
        assert!(names("artist:k").is_empty());
        assert_eq!(word_at("genre:jazz -gen", 15), (12, "gen"));
        assert_eq!(word_at("(art", 4), (1, "art"));
    }
}
