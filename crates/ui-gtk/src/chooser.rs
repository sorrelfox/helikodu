// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's folder chooser, asked for in one place.
//!
//! A window has at most one folder chooser open: asking again while one is
//! open does nothing, so a second click, a key held down or an event that
//! arrives as the chooser closes cannot stack choosers or open them in a row.
//! Cancel answers `None` and nothing else happens.
//!
//! Tests stand in for the chooser with [`replace_folder_chooser`], since the
//! real one needs a desktop portal or a visible dialog.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

/// The answer to a folder request: the folder, or `None` for Cancel.
pub type FolderReply = Box<dyn FnOnce(Option<PathBuf>)>;

/// Something that answers folder requests in place of the desktop's chooser:
/// it gets the chooser's title and the reply to call once.
pub type FolderChooser = Rc<dyn Fn(&str, FolderReply)>;

thread_local! {
    static STAND_IN: RefCell<Option<FolderChooser>> = const { RefCell::new(None) };
    static OPEN_OVER: RefCell<Vec<glib::WeakRef<gtk::Window>>> = const { RefCell::new(Vec::new()) };
}

/// Answers every folder request on this thread with `chooser` instead of the
/// desktop's chooser; `None` goes back to the desktop's. For tests.
#[doc(hidden)]
pub fn replace_folder_chooser(chooser: Option<FolderChooser>) {
    STAND_IN.with(|s| s.replace(chooser));
}

/// Whether a folder chooser is open over `parent`.
pub(crate) fn is_open_over(parent: &gtk::Window) -> bool {
    OPEN_OVER.with(|open| {
        open.borrow()
            .iter()
            .any(|w| w.upgrade().as_ref() == Some(parent))
    })
}

fn set_open(parent: &gtk::Window, open: bool) {
    OPEN_OVER.with(|list| {
        let mut list = list.borrow_mut();
        list.retain(|w| w.upgrade().is_some_and(|w| &w != parent));
        if open {
            list.push(parent.downgrade());
        }
    });
}

/// Asks for a folder over `parent`, with `title` on the chooser, `accept` on
/// its button (the desktop's word when `None`) and `start` as the folder it
/// opens in. `reply` gets the folder, or `None` when the chooser was cancelled
/// or closed. Does nothing while a folder chooser is already open over
/// `parent`.
pub(crate) fn select_folder(
    parent: &gtk::Window,
    title: &str,
    accept: Option<&str>,
    start: Option<&Path>,
    reply: impl FnOnce(Option<PathBuf>) + 'static,
) {
    if is_open_over(parent) {
        return;
    }
    set_open(parent, true);
    let weak = parent.downgrade();
    let reply = move |path: Option<PathBuf>| {
        if let Some(parent) = weak.upgrade() {
            set_open(&parent, false);
        }
        reply(path);
    };
    if let Some(chooser) = STAND_IN.with(|s| s.borrow().clone()) {
        chooser(title, Box::new(reply));
        return;
    }
    let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
    if let Some(accept) = accept {
        dialog.set_accept_label(Some(accept));
    }
    if let Some(start) = start {
        dialog.set_initial_folder(Some(&gio::File::for_path(start)));
    }
    dialog.select_folder(Some(parent), gio::Cancellable::NONE, move |result| {
        reply(result.ok().and_then(|f| f.path()));
    });
}
