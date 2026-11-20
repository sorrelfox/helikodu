// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The completion under the search entry: while a field name is typed it lists
//! the fields that start with it (the name in mono with the typed letters in
//! the accent, what the field holds, an example); after a field and its
//! operator it lists the values of the fields that have few (kind, codec,
//! has, is). Tab completes, Up and Down choose, Escape closes. The entry keeps
//! the focus throughout.
//!
//! The user's labels and fields come from the library each time the entry takes
//! the focus: the fields are listed among the built-in ones, `label:` lists the
//! labels with their numbers of tracks, and a field that stands for a tag lists
//! its values when they are few.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use helikodu_model::query::{Field, FieldKind, Flag, MediaKind};

use crate::backend::{Backend, Vocabulary};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;

/// The most rows shown at once.
const MAX_ROWS: usize = 8;

/// The height of a row.
const ROW_HEIGHT: i32 = 30;

/// Up to this many labels are all listed after `label:`; with more, only those
/// that start with the typed letters.
const ALL_LABELS: usize = 200;

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
        Field::Label => (tr("One of your labels"), "label:favourite"),
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

/// `value` as the query language reads it as one value: in quotes when it has
/// white space or a character the language gives a meaning.
fn quoted(value: &str) -> String {
    let plain = !value.is_empty()
        && !value.starts_with('-')
        && !value
            .chars()
            .any(|c| c.is_whitespace() || "():=!~<>\"".contains(c));
    if plain {
        value.to_owned()
    } else {
        format!("\"{value}\"")
    }
}

/// The labels after `name` and its operator `op` that start with `typed`.
fn label_choices(name: &str, op: &str, typed: &str, words: &Vocabulary) -> Vec<Choice> {
    let typed = typed.trim_start_matches('"').to_lowercase();
    if words.labels.len() > ALL_LABELS && typed.is_empty() {
        return Vec::new();
    }
    words
        .labels
        .iter()
        .filter(|(label, _)| {
            let label = label.to_lowercase();
            label.starts_with(&typed) && label != typed
        })
        .map(|(label, tracks)| {
            let query = format!("{name}{op}{}", quoted(label));
            Choice {
                name: label.clone(),
                description: fill(
                    &ntr("{} track", "{} tracks", *tracks),
                    &[&format::count(*tracks)],
                ),
                insert: format!("{query} "),
                example: query,
            }
        })
        .collect()
}

/// The user's fields whose names start with `typed`.
fn your_field_choices(typed: &str, words: &Vocabulary) -> Vec<Choice> {
    words
        .fields
        .iter()
        .filter(|f| f.field.name.starts_with(typed) && f.field.name != typed)
        .map(|f| {
            let name = f.field.name.clone();
            let description = match &f.field.tag {
                Some(tag) => fill(&tr("Your field, the tag {}"), &[tag]),
                None => tr("Your field, computed"),
            };
            let example = match &f.example {
                Some(value) => format!("{name}={}", quoted(value)),
                None => format!("has:{name}"),
            };
            Choice {
                insert: format!("{name}:"),
                name,
                description,
                example,
            }
        })
        .collect()
}

/// The choices for `word`: fields by name, or values after a field and its
/// operator; `words` holds the user's labels and fields.
fn choices(word: &str, words: &Vocabulary) -> Vec<Choice> {
    if let Some((name, value)) = word.split_once([':', '=']) {
        let op = &word[name.len()..=name.len()];
        let lower = name.to_ascii_lowercase();
        if Field::built_in(&lower) == Some(Field::Label) {
            return label_choices(name, op, value, words);
        }
        let own = words.fields.iter().find(|f| f.field.name == lower);
        let values: Vec<String> = match lower.as_str() {
            // A computed field offers no values; a custom field its few ones.
            _ if own.is_some() => own.map(|f| f.values.clone()).unwrap_or_default(),
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
        let typed = value.trim_start_matches('"').to_lowercase();
        return values
            .into_iter()
            .filter(|v| {
                let v = v.to_lowercase();
                v.starts_with(&typed) && v != typed
            })
            .map(|v| Choice {
                description: String::new(),
                example: format!("{name}{op}{}", quoted(&v)),
                insert: format!("{name}{op}{} ", quoted(&v)),
                name: v,
            })
            .collect();
    }
    let named = word.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !named {
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
    found.extend(your_field_choices(&typed, words));
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.truncate(MAX_ROWS);
    found
}

/// The completion popover of one entry.
pub(crate) struct QueryCompletion {
    entry: gtk::SearchEntry,
    popover: gtk::Popover,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    backend: Backend,
    /// The user's labels and fields, as last loaded.
    words: RefCell<Rc<Vocabulary>>,
    /// Whether they were asked for since the entry took the focus.
    asked: Cell<bool>,
    /// The completion itself, for the answer of the library.
    me: RefCell<std::rc::Weak<Self>>,
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
    /// The completion of `entry`, with the labels and fields of `backend`.
    pub(crate) fn new(entry: &gtk::SearchEntry, backend: &Backend) -> Rc<Self> {
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
        // A long list of values scrolls; the popover shows eight rows at most.
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(ROW_HEIGHT * MAX_ROWS as i32);
        scroller.set_child(Some(&list));
        content.append(&scroller);
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
            scroller,
            backend: backend.clone(),
            words: RefCell::new(Rc::new(Vocabulary::default())),
            asked: Cell::new(false),
            me: RefCell::new(std::rc::Weak::new()),
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
        completion.me.replace(Rc::downgrade(&completion));
        let weak = Rc::downgrade(&completion);
        entry.connect_has_focus_notify(move |entry| {
            if !entry.has_focus()
                && let Some(completion) = weak.upgrade()
            {
                // Asked for again when the entry is used next.
                completion.asked.set(false);
                completion.close();
            }
        });
        completion
    }

    /// Asks the library for the labels and the user's fields again; the
    /// popover follows them if it is open.
    fn load_words(&self) {
        self.asked.set(true);
        let weak = self.me.borrow().clone();
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let Ok(words) = backend.query(|library| library.vocabulary()).await else {
                return;
            };
            let Some(completion) = weak.upgrade() else {
                return;
            };
            if **completion.words.borrow() == words {
                return;
            }
            completion.words.replace(Rc::new(words));
            completion.update();
        });
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
        if typing && !self.asked.get() {
            self.load_words();
        }
        let words = self.words.borrow().clone();
        let found = if typing {
            choices(word, &words)
        } else {
            Vec::new()
        };
        if found.is_empty() {
            self.close();
            return;
        }
        let typed = word
            .split_once([':', '='])
            .map_or(word.len(), |(_, value)| value.trim_start_matches('"').len());
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
            // Keep the chosen row in view.
            let adjustment = self.scroller.vadjustment();
            let top = f64::from(ROW_HEIGHT) * index as f64;
            let bottom = top + f64::from(ROW_HEIGHT);
            if top < adjustment.value() {
                adjustment.set_value(top);
            } else if bottom > adjustment.value() + adjustment.page_size() {
                adjustment.set_value(bottom - adjustment.page_size());
            }
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
        let words = Vocabulary::default();
        let names = |word: &str| -> Vec<String> {
            choices(word, &words).into_iter().map(|c| c.name).collect()
        };
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

    fn words() -> Vocabulary {
        use crate::backend::{FieldWords, YourField};
        Vocabulary {
            labels: vec![
                ("after hours".into(), 41),
                ("favourite".into(), 204),
                ("late night".into(), 63),
            ],
            fields: vec![
                FieldWords {
                    field: YourField {
                        name: "decade".into(),
                        tag: None,
                    },
                    example: Some("1990s".into()),
                    values: Vec::new(),
                },
                FieldWords {
                    field: YourField {
                        name: "mood".into(),
                        tag: Some("MOOD".into()),
                    },
                    example: Some("calm".into()),
                    values: vec!["bright".into(), "calm".into()],
                },
            ],
        }
    }

    #[test]
    fn labels_and_your_fields_complete() {
        let words = words();
        let found = |word: &str| choices(word, &words);
        let names =
            |word: &str| -> Vec<String> { found(word).into_iter().map(|c| c.name).collect() };
        // `label` among the fields, and the user's fields with the others.
        assert_eq!(
            names("l"),
            ["label", "lastplayed", "lastskipped", "limit", "lyricist"]
        );
        assert_eq!(names("d"), ["decade", "disc", "discsubtitle", "duration"]);
        let decade = &found("dec")[0];
        assert_eq!(decade.example, "decade=1990s");
        assert_eq!(decade.insert, "decade:");
        assert_eq!(found("moo")[0].description, "Your field, the tag MOOD");
        // After `label:` the labels, a name of several words in quotes.
        assert_eq!(names("label:"), ["after hours", "favourite", "late night"]);
        let late = &found("label:la")[0];
        assert_eq!(late.example, "label:\"late night\"");
        assert_eq!(late.insert, "label:\"late night\" ");
        assert_eq!(late.description, "63 tracks");
        assert_eq!(names("labels=\"fav"), ["favourite"]);
        // A custom field offers its values; a computed one none.
        assert_eq!(names("mood:"), ["bright", "calm"]);
        assert!(names("decade:").is_empty());
        // With no labels the popover stays closed.
        assert!(choices("label:", &Vocabulary::default()).is_empty());
        assert_eq!(quoted("live"), "live");
        assert_eq!(quoted("for the car"), "\"for the car\"");
    }
}
