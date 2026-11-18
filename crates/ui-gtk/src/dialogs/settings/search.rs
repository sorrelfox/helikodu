// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings search: the settings whose label, explanation or choices
//! hold every word typed, in groups by page, drawn as on their pages and
//! working there. The matching words are in the accent.
//!
//! When a search starts, the pages are built once more for it (all but
//! Shortcuts, which has a filter of its own); its rows are taken from those
//! copies. When the search ends the copies go, and the pages are built again
//! if a setting changed meanwhile, so that they show the new value.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;

use super::{Context, Page, build_page};
use crate::backend::services::Settings;
use crate::dialogs::common::divider;
use crate::i18n::{fill, ntr, tr};
use crate::views::search::highlighted;
use crate::widgets::label;

/// One setting the search can find.
struct Entry {
    page: Page,
    row: gtk::Widget,
    /// Its words: labels, explanations and choices, in lower case.
    words: String,
}

/// A search in progress.
pub(super) struct Search {
    entries: Vec<Entry>,
    /// The settings when it started.
    before: Arc<Settings>,
    /// Keeps the copies of the pages alive.
    _copies: Vec<gtk::Widget>,
}

/// What a search found: the number of settings per page, in page order.
pub(super) type Found = Vec<(Page, usize)>;

/// Every label text under `widget`, and the strings of its dropdowns.
fn words_of(widget: &gtk::Widget, words: &mut String, labels: &mut Vec<gtk::Label>) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        words.push_str(&label.text().to_lowercase());
        words.push(' ');
        labels.push(label.clone());
    }
    if let Some(dropdown) = widget.downcast_ref::<gtk::DropDown>()
        && let Some(strings) = dropdown.model().and_downcast::<gtk::StringList>()
    {
        for i in 0..strings.n_items() {
            if let Some(text) = strings.string(i) {
                words.push_str(&text.to_lowercase());
                words.push(' ');
            }
        }
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        words_of(&c, words, labels);
        child = c.next_sibling();
    }
}

/// The page body (`hk-settings-page`) inside `widget`.
fn body_of(widget: &gtk::Widget) -> Option<gtk::Widget> {
    if widget.has_css_class("hk-settings-page") {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = body_of(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

impl Search {
    /// Builds the pages for a search.
    pub(super) fn start(context: &Rc<Context>) -> Self {
        let mut entries = Vec::new();
        let mut copies = Vec::new();
        for page in Page::ALL {
            if page == Page::Shortcuts {
                continue;
            }
            let copy = build_page(page, context);
            if let Some(body) = body_of(&copy) {
                let mut child = body.first_child();
                while let Some(row) = child {
                    child = row.next_sibling();
                    if !row.has_css_class("hk-setting") || !row.is_visible() {
                        continue;
                    }
                    let mut words = String::new();
                    words_of(&row, &mut words, &mut Vec::new());
                    entries.push(Entry { page, row, words });
                }
            }
            copies.push(copy);
        }
        Self {
            entries,
            before: context.settings(),
            _copies: copies,
        }
    }

    /// Whether the settings changed since the search started.
    pub(super) fn changed(&self, context: &Context) -> bool {
        *context.settings() != *self.before
    }

    /// Shows the settings matching `text` in `results`; returns what was
    /// found.
    pub(super) fn show(
        &self,
        text: &str,
        results: &gtk::Box,
        open_page: &Rc<dyn Fn(Page)>,
    ) -> Found {
        while let Some(child) = results.first_child() {
            results.remove(&child);
        }
        for entry in &self.entries {
            if entry.row.parent().is_some() {
                entry.row.unparent();
            }
        }
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        let mut found: Found = Vec::new();
        for entry in &self.entries {
            if words.is_empty() || !words.iter().all(|w| entry.words.contains(w.as_str())) {
                continue;
            }
            if found.last().map(|(p, _)| *p) != Some(entry.page) {
                if !found.is_empty() {
                    results.append(&divider());
                }
                results.append(&group_head(entry.page, open_page));
                found.push((entry.page, 0));
            }
            if let Some(last) = found.last_mut() {
                last.1 += 1;
            }
            let mut labels = Vec::new();
            words_of(&entry.row, &mut String::new(), &mut labels);
            for label in labels {
                label.set_attributes(Some(&highlighted(&label.text(), &words)));
            }
            results.append(&entry.row);
        }
        if found.is_empty() {
            let empty = crate::widgets::empty_state::empty_state(
                "system-search-symbolic",
                &fill(&tr("No settings match “{}”"), &[text.trim()]),
                &tr("Try other words, or look through the pages."),
                &[],
            );
            results.append(&empty);
        }
        found
    }
}

/// The head of a page's group: its name, and "Open the page".
fn group_head(page: Page, open_page: &Rc<dyn Fn(Page)>) -> gtk::Box {
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("hk-search-group-head");
    let name = label(&page.title(), &["hk-section-label"]);
    name.set_hexpand(true);
    head.append(&name);
    let open = gtk::Button::new();
    open.add_css_class("hk-open-page");
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    inner.append(&gtk::Label::new(Some(&tr("Open the page"))));
    inner.append(&gtk::Image::from_icon_name("go-next-symbolic"));
    open.set_child(Some(&inner));
    open.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("Open the page {}"),
        &[&page.title()],
    ))]);
    let open_page = open_page.clone();
    open.connect_clicked(move |_| open_page(page));
    head.append(&open);
    head
}

/// The line under the header: "N settings on M pages · Esc goes back".
pub(super) fn summary(found: &Found) -> String {
    let settings: usize = found.iter().map(|(_, n)| n).sum();
    let pages = found.len();
    fill(
        &tr("{} on {} · Esc goes back"),
        &[
            &fill(
                &ntr("{} setting", "{} settings", settings),
                &[&settings.to_string()],
            ),
            &fill(&ntr("{} page", "{} pages", pages), &[&pages.to_string()]),
        ],
    )
}

/// The search's state in the window.
#[derive(Default)]
pub(super) struct SearchState {
    pub(super) search: RefCell<Option<Search>>,
    /// The page shown before the search.
    pub(super) before: RefCell<Option<String>>,
}
