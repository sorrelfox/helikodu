// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drag and drop inside the window: what a dragged row or tile carries, and
//! the drop targets that take tracks (the queue panel, playlists in the
//! sidebar) and files from a file manager (the whole window).

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::backend::{AlbumKey, PlaylistKey, TrackKey};

/// What a drag inside the window carries: tracks, or an album to look up.
#[derive(Debug, Clone, Default, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "HkDraggedTracks")]
pub(crate) struct DraggedTracks {
    /// The tracks, in order.
    pub(crate) keys: Vec<TrackKey>,
    /// An album whose tracks are meant, for a tile.
    pub(crate) album: Option<AlbumKey>,
    /// The playlist and positions the rows come from, for reordering.
    pub(crate) from_playlist: Option<(PlaylistKey, Vec<usize>)>,
}

impl DraggedTracks {
    /// A drag source on `widget` that carries what `content` gives when the
    /// drag starts (at the press position), or nothing to refuse it.
    pub(crate) fn source(
        widget: &impl IsA<gtk::Widget>,
        content: impl Fn(f64, f64) -> Option<(Self, String)> + 'static,
    ) -> gtk::DragSource {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let label = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let text = label.clone();
        source.connect_prepare(move |_, x, y| {
            let (dragged, label) = content(x, y)?;
            text.replace(label);
            Some(gdk::ContentProvider::for_value(&dragged.to_value()))
        });
        source.connect_drag_begin(move |_, drag| {
            // A raised copy of the row's title follows the pointer.
            let ghost = gtk::Label::new(Some(&label.borrow()));
            ghost.add_css_class("hk-drag-ghost");
            gtk::DragIcon::for_drag(drag).set_child(Some(&ghost));
        });
        widget.add_controller(source.clone());
        source
    }

    /// A drop target for dragged tracks.
    pub(crate) fn target(action: gdk::DragAction) -> gtk::DropTarget {
        gtk::DropTarget::new(Self::static_type(), action)
    }
}

/// A drop target for files from a file manager; `dropped` gets the files.
pub(crate) fn file_target(dropped: impl Fn(Vec<std::path::PathBuf>) + 'static) -> gtk::DropTarget {
    let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    target.connect_drop(move |_, value, _, _| {
        let Ok(files) = value.get::<gdk::FileList>() else {
            return false;
        };
        let paths: Vec<_> = files.files().iter().filter_map(|f| f.path()).collect();
        if paths.is_empty() {
            return false;
        }
        dropped(paths);
        true
    });
    target
}
