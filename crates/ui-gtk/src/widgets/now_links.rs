// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The playing track's artist and album as links, for the deck, the queue
//! panel and Now Playing: each artist of the credit opens their page, the
//! album opens its page.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::{AlbumKey, Backend, TrackKey, TrackRow};
use crate::i18n::{fill, tr};
use crate::views::actions::{Origin, Target};
use crate::window::MainWindow;

/// What separates the artists of a credit, in the order they are tried.
const SEPARATORS: [&str; 11] = [
    " (feat. ",
    " [feat. ",
    " feat. ",
    " ft. ",
    " featuring ",
    " Feat. ",
    " & ",
    ", ",
    " / ",
    "; ",
    " vs. ",
];

/// Splits an artist credit into names and the text between them:
/// `(text, is_name)`.
pub(crate) fn split_credit(credit: &str) -> Vec<(String, bool)> {
    let mut parts = Vec::new();
    let mut rest = credit;
    let mut close = String::new();
    loop {
        let found = SEPARATORS
            .iter()
            .filter_map(|s| rest.find(s).map(|i| (i, *s)))
            .min_by_key(|(i, _)| *i);
        let Some((i, sep)) = found else { break };
        if i > 0 {
            parts.push((rest[..i].to_string(), true));
        }
        parts.push((sep.to_string(), false));
        if sep.starts_with(" (") {
            close.push(')');
        } else if sep.starts_with(" [") {
            close.push(']');
        }
        rest = &rest[i + sep.len()..];
    }
    let mut last = rest.to_string();
    let mut tail = String::new();
    for c in close.chars().rev() {
        if last.ends_with(c) {
            last.pop();
            tail.insert(0, c);
        }
    }
    if !last.trim().is_empty() {
        parts.push((last, true));
    }
    if !tail.is_empty() {
        parts.push((tail, false));
    }
    parts
}

/// A link: a flat button with an ellipsized label, a pointer cursor and an
/// accessible name.
pub(crate) fn link(text: &str, name: &str, max_chars: i32) -> gtk::Button {
    let label = gtk::Label::new(Some(text));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(max_chars);
    label.set_xalign(0.0);
    let button = gtk::Button::new();
    button.set_child(Some(&label));
    button.set_has_frame(false);
    button.add_css_class("hk-now-link");
    button.set_cursor_from_name(Some("pointer"));
    button.set_tooltip_text(Some(name));
    button.update_property(&[gtk::accessible::Property::Label(name)]);
    button
}

fn text(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label
}

/// Goes to the page of the artist `name` from `widget`'s window.
pub(crate) fn go_to_artist(widget: &impl IsA<gtk::Widget>, name: &str) {
    let Some(window) = widget.as_ref().root().and_downcast::<MainWindow>() else {
        return;
    };
    let Some(backend) = window.backend() else {
        return;
    };
    window.set_now_playing(false);
    crate::views::actions::go_to_artist_named(&window, backend, name.to_string());
}

/// Goes to the page of `album` from `widget`'s window, selecting `track`
/// there when given.
pub(crate) fn go_to_album(
    widget: &impl IsA<gtk::Widget>,
    album: AlbumKey,
    track: Option<TrackKey>,
) {
    let Some(window) = widget.as_ref().root().and_downcast::<MainWindow>() else {
        return;
    };
    window.set_now_playing(false);
    window.open_album_at(album, track);
}

/// Opens the track context menu of the playing `track` on `widget`.
pub(crate) fn popup_track_menu(
    widget: &impl IsA<gtk::Widget>,
    track: TrackRow,
    at: Option<(f64, f64)>,
) -> Option<gtk::PopoverMenu> {
    let target = Target::Tracks {
        rows: vec![Rc::new(track)],
        origin: Origin::Playing,
    };
    crate::views::actions::popup(widget, target, at)
}

/// Adds the context menu of the track `current` gives to `widget`: right
/// click, the Menu key and Shift+F10.
pub(crate) fn add_track_menu(
    widget: &impl IsA<gtk::Widget>,
    current: impl Fn() -> Option<TrackRow> + 'static,
) {
    let widget = widget.as_ref();
    let current = Rc::new(current);
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    let c = current.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        let Some(track) = c() else { return };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(widget) = gesture.widget() {
            popup_track_menu(&widget, track, Some((x, y)));
        }
    });
    widget.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |keys, key, _, state| {
        if crate::views::actions::is_menu_key(key, state)
            && let (Some(track), Some(widget)) = (current(), keys.widget())
        {
            popup_track_menu(&widget, track, None);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    widget.add_controller(keys);
}

type Shown = (String, Option<(String, AlbumKey)>, Vec<String>);

/// A line of links: the artists of a credit, the album, and plain text after
/// them, separated by " · ".
#[derive(Debug, Clone)]
pub(crate) struct NowLinks {
    root: gtk::Box,
    /// The credit shown, so a late library answer for another one is dropped.
    credit: Rc<RefCell<String>>,
    /// What the line was last asked to show.
    shown: Rc<RefCell<Option<Shown>>>,
    backend: Rc<RefCell<Option<Backend>>>,
}

impl NowLinks {
    /// An empty line with the CSS `classes`.
    pub(crate) fn new(classes: &[&str]) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for class in classes {
            root.add_css_class(class);
        }
        root.add_css_class("hk-now-links");
        root.set_halign(gtk::Align::Start);
        Self {
            root,
            credit: Rc::default(),
            shown: Rc::default(),
            backend: Rc::default(),
        }
    }

    /// The line's widget.
    pub(crate) fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Looks up whole credits in `backend`'s library: a credit such as
    /// "Simon & Garfunkel" that names one artist is one link.
    pub(crate) fn set_backend(&self, backend: &Backend) {
        self.backend.replace(Some(backend.clone()));
    }

    /// Shows the artists of `credit` (if any), then the album `album` (title
    /// and key, if any), then `extra`.
    pub(crate) fn set(&self, credit: &str, album: Option<(&str, AlbumKey)>, extra: &[String]) {
        let shown = (
            credit.to_string(),
            album.map(|(t, k)| (t.to_string(), k)),
            extra.to_vec(),
        );
        if self.shown.borrow().as_ref() == Some(&shown) {
            return;
        }
        self.shown.replace(Some(shown));
        self.credit.replace(credit.to_string());
        let parts = split_credit(credit);
        let names = parts.iter().filter(|(_, n)| *n).count();
        self.build(parts, album, extra);
        if names < 2 {
            return;
        }
        let Some(backend) = self.backend.borrow().clone() else {
            return;
        };
        let whole = credit.to_string();
        let this = self.clone();
        let album = album.map(|(t, k)| (t.to_string(), k));
        let extra = extra.to_vec();
        glib::spawn_future_local(async move {
            let name = whole.clone();
            let known = backend.query(move |l| l.find_artist(&name)).await;
            if matches!(known, Ok(Some(_))) && *this.credit.borrow() == whole {
                let album = album.as_ref().map(|(t, k)| (t.as_str(), *k));
                this.build(vec![(whole.clone(), true)], album, &extra);
            }
        });
    }

    fn build(&self, parts: Vec<(String, bool)>, album: Option<(&str, AlbumKey)>, extra: &[String]) {
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }
        let mut any = false;
        for (part, is_name) in parts {
            if is_name {
                let name = fill(&tr("Go to artist {}"), &[part.trim()]);
                let button = link(&part, &name, 24);
                let artist = part.trim().to_string();
                button.connect_clicked(move |b| go_to_artist(b, &artist));
                self.root.append(&button);
            } else {
                self.root.append(&text(&part));
            }
            any = true;
        }
        if let Some((title, key)) = album.filter(|(t, _)| !t.is_empty()) {
            if any {
                self.root.append(&text(&tr(" · ")));
            }
            let name = fill(&tr("Go to album {}"), &[title]);
            let button = link(title, &name, 28);
            button.connect_clicked(move |b| go_to_album(b, key, None));
            self.root.append(&button);
            any = true;
        }
        for part in extra {
            if any {
                self.root.append(&text(&tr(" · ")));
            }
            self.root.append(&text(part));
            any = true;
        }
    }

    /// The links' accessible names, in order.
    pub(crate) fn link_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut child = self.root.first_child();
        while let Some(widget) = child {
            if widget.is::<gtk::Button>() {
                names.push(widget.tooltip_text().unwrap_or_default().to_string());
            }
            child = widget.next_sibling();
        }
        names
    }

    /// Clicks the link named `name`; whether there is one.
    pub(crate) fn click(&self, name: &str) -> bool {
        let mut child = self.root.first_child();
        while let Some(widget) = child {
            if let Some(button) = widget.downcast_ref::<gtk::Button>()
                && button.tooltip_text().as_deref() == Some(name)
            {
                button.emit_clicked();
                return true;
            }
            child = widget.next_sibling();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::split_credit;

    fn names(credit: &str) -> Vec<String> {
        split_credit(credit)
            .into_iter()
            .filter(|(_, n)| *n)
            .map(|(t, _)| t)
            .collect()
    }

    #[test]
    fn credits_split_into_artists() {
        assert_eq!(names("Anamanaguchi"), ["Anamanaguchi"]);
        assert_eq!(names("A & B"), ["A", "B"]);
        assert_eq!(names("A feat. B, C"), ["A", "B", "C"]);
        assert_eq!(names("A (feat. B)"), ["A", "B"]);
        let joined: String = split_credit("A (feat. B)")
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(joined, "A (feat. B)");
    }
}
