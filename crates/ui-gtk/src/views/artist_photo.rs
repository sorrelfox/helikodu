// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Choosing an artist's photo: the menu items, the file chooser and dropping a
//! picture onto the avatar. The photo is a local file; nothing is fetched.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gdk, gio};

use crate::backend::ArtistKey;
use crate::i18n::{fill, tr};

/// The action that asks for a photo of the artist in its target.
pub(crate) const SET_ACTION: &str = "artists.set-photo";
/// The action that forgets the photo chosen for the artist in its target.
pub(crate) const REMOVE_ACTION: &str = "artists.remove-photo";

/// The picture files a photo can be.
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// The menu section with "Set artist photo…" and, when the artist has a photo
/// chosen in the app, "Remove photo".
pub(crate) fn menu_section(artist: ArtistKey, own_photo: bool) -> gio::Menu {
    let section = gio::Menu::new();
    let set = gio::MenuItem::new(Some(&tr("Set artist photo…")), None);
    set.set_action_and_target_value(Some(SET_ACTION), Some(&artist.0.to_variant()));
    section.append_item(&set);
    if own_photo {
        let remove = gio::MenuItem::new(Some(&tr("Remove photo")), None);
        remove.set_action_and_target_value(Some(REMOVE_ACTION), Some(&artist.0.to_variant()));
        section.append_item(&remove);
    }
    section
}

/// Whether `path` names a picture a photo can be.
pub(crate) fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Asks for a picture for the artist `name` and calls `chosen` with it.
pub(crate) fn choose(parent: &gtk::Widget, name: &str, chosen: impl Fn(PathBuf) + 'static) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&tr("Images")));
    for mime in ["image/jpeg", "image/png", "image/webp"] {
        filter.add_mime_type(mime);
    }
    for extension in EXTENSIONS {
        filter.add_suffix(extension);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title(fill(&tr("Choose a photo of {}"), &[name]))
        .filters(&filters)
        .default_filter(&filter)
        .modal(true)
        .build();
    let window = parent.root().and_downcast::<gtk::Window>();
    dialog.open(window.as_ref(), gio::Cancellable::NONE, move |result| {
        if let Ok(Some(path)) = result.map(|f| f.path()) {
            chosen(path);
        }
    });
}

/// A drop target on the avatar that takes one picture file and calls `dropped`
/// with it.
pub(crate) fn drop_target(dropped: impl Fn(PathBuf) + 'static) -> gtk::DropTarget {
    let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    target.connect_drop(move |_, value, _, _| {
        let Ok(files) = value.get::<gdk::FileList>() else {
            return false;
        };
        let Some(path) = files
            .files()
            .into_iter()
            .filter_map(|f| f.path())
            .find(|p| is_picture(p))
        else {
            return false;
        };
        dropped(path);
        true
    });
    target
}

#[cfg(test)]
mod tests {
    use super::is_picture;
    use std::path::Path;

    #[test]
    fn pictures_by_extension() {
        assert!(is_picture(Path::new("/a/singer.JPG")));
        assert!(is_picture(Path::new("artist.webp")));
        assert!(!is_picture(Path::new("notes.txt")));
        assert!(!is_picture(Path::new("photo")));
    }
}
