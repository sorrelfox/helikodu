// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sidebar: the brand strip, then the library views and the playlists.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use crate::backend::LibraryCounts;
use crate::i18n::tr;
use crate::widgets::{BrandMark, icon_button, label};

/// One library row: its page name, icon, and whether it shows a count.
const LIBRARY: &[(&str, &str)] = &[
    ("home", "go-home-symbolic"),
    ("albums", "media-optical-symbolic"),
    ("artists", "avatar-default-symbolic"),
    ("tracks", "audio-x-generic-symbolic"),
    ("genres", "helikodu-tag-symbolic"),
    ("folders", "folder-symbolic"),
];

/// The visible name of a library page.
pub(crate) fn page_title(page: &str) -> String {
    match page {
        "home" => tr("Home"),
        "albums" => tr("Albums"),
        "artists" => tr("Artists"),
        "tracks" => tr("Tracks"),
        "genres" => tr("Genres"),
        "folders" => tr("Folders"),
        _ => String::new(),
    }
}

/// The icon of a library page.
pub(crate) fn page_icon(page: &str) -> &'static str {
    LIBRARY
        .iter()
        .find(|(name, _)| *name == page)
        .map_or("folder-symbolic", |(_, icon)| icon)
}

type Navigate = Box<dyn Fn(&str)>;

/// The sidebar and its list.
pub(crate) struct Sidebar {
    pub(crate) root: gtk::Box,
    pub(crate) controls: gtk::WindowControls,
    list: gtk::ListBox,
    counts: RefCell<Vec<(String, gtk::Label)>>,
    quiet: Cell<bool>,
    navigate: RefCell<Option<Navigate>>,
}

impl std::fmt::Debug for Sidebar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sidebar").finish_non_exhaustive()
    }
}

impl Sidebar {
    pub(crate) fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-sidebar");
        root.set_accessible_role(gtk::AccessibleRole::Navigation);
        root.update_property(&[gtk::accessible::Property::Label(&tr("Library"))]);

        let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        brand.add_css_class("hk-header");
        let controls = gtk::WindowControls::new(gtk::PackType::Start);
        controls.set_valign(gtk::Align::Center);
        brand.append(&controls);
        brand.append(&BrandMark::default());
        let name = label("Helikodu", &["hk-brand"]);
        name.set_hexpand(true);
        brand.append(&name);
        let hide = icon_button("helikodu-sidebar-symbolic", &tr("Hide the sidebar"));
        hide.set_action_name(Some("win.toggle-sidebar"));
        brand.append(&hide);
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&brand));
        root.append(&handle);

        let list = gtk::ListBox::new();
        list.add_css_class("hk-list");
        list.set_margin_start(8);
        list.set_margin_end(8);
        list.set_margin_bottom(8);
        list.update_property(&[gtk::accessible::Property::Label(&tr("Library"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        root.append(&scroller);

        let sidebar = Rc::new(Self {
            root,
            controls,
            list,
            counts: RefCell::new(Vec::new()),
            quiet: Cell::new(false),
            navigate: RefCell::new(None),
        });
        for (page, icon) in LIBRARY {
            sidebar.add_row(page, icon, &page_title(page), &[]);
        }
        sidebar.add_row(
            "new-playlist",
            "list-add-symbolic",
            &tr("New playlist"),
            &["hk-dim"],
        );
        sidebar.list.set_header_func(|row, before| {
            let text = match (row.widget_name().as_str(), before) {
                (_, None) => Some((tr("Library"), true)),
                ("new-playlist", _) => Some((tr("Playlists"), false)),
                _ => None,
            };
            row.set_header(
                text.map(|(text, first)| {
                    let header = label(&text, &["hk-section-label", "hk-list-section"]);
                    if first {
                        header.add_css_class("first");
                    }
                    header
                })
                .as_ref(),
            );
        });
        let weak = Rc::downgrade(&sidebar);
        sidebar.list.connect_row_selected(move |_, row| {
            let (Some(sidebar), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            if sidebar.quiet.get() {
                return;
            }
            if let Some(navigate) = sidebar.navigate.borrow().as_ref() {
                navigate(row.widget_name().as_str());
            }
        });
        sidebar
    }

    fn add_row(&self, page: &str, icon: &str, title: &str, classes: &[&str]) {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(page);
        for class in classes {
            row.add_css_class(class);
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(&gtk::Image::from_icon_name(icon));
        let name = label(title, &[]);
        name.set_hexpand(true);
        line.append(&name);
        let count = label("", &["hk-count"]);
        count.set_xalign(1.0);
        line.append(&count);
        self.counts.borrow_mut().push((page.to_owned(), count));
        row.set_child(Some(&line));
        if page == "new-playlist" {
            // Playlists come in a later version.
            row.set_sensitive(false);
        }
        self.list.append(&row);
    }

    /// Calls `navigate` with the page name when the user picks a row.
    pub(crate) fn connect_navigate(&self, navigate: impl Fn(&str) + 'static) {
        self.navigate.replace(Some(Box::new(navigate)));
    }

    /// Selects the row of `page` without navigating; no row for other pages.
    pub(crate) fn select(&self, page: Option<&str>) {
        self.quiet.set(true);
        let mut child = self.list.first_child();
        let mut found = None;
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && Some(row.widget_name().as_str()) == page
            {
                found = Some(row.clone());
            }
            child = widget.next_sibling();
        }
        self.list.select_row(found.as_ref());
        self.quiet.set(false);
    }

    /// Shows the library totals beside the rows.
    pub(crate) fn set_counts(&self, counts: &LibraryCounts) {
        for (page, label) in self.counts.borrow().iter() {
            let value = match page.as_str() {
                "albums" => Some(counts.albums),
                "artists" => Some(counts.artists),
                "tracks" => Some(counts.tracks),
                "genres" => Some(counts.genres),
                _ => None,
            };
            label.set_text(
                &value
                    .filter(|v| *v > 0)
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            );
        }
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.list.clone().upcast()
    }
}
