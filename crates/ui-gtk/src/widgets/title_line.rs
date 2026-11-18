// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A track title in a list row, with the copies badge right after the text
//! when the track has copies. The title ellipsizes before the badge does, and
//! the line asks for the same small width with or without a badge, so it
//! shares a row with other expanding columns as a label with
//! `max-width-chars` 1 does and the columns stay aligned.
//!
//! In a list without a number column the playing row's title carries the
//! playing indicator before its text ([`TitleLine::set_playing`]).

use std::cell::RefCell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::{PlayingIndicator, copies_badge};
use crate::backend::TrackKey;

/// The gap between the title and the badge.
const GAP: i32 = 6;
/// The gap between the playing indicator and the title.
const LEAD_GAP: i32 = 8;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct TitleLine {
        pub(super) label: RefCell<Option<gtk::Label>>,
        pub(super) badge: RefCell<Option<gtk::Button>>,
        pub(super) indicator: RefCell<Option<PlayingIndicator>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TitleLine {
        const NAME: &'static str = "HkTitleLine";
        type Type = super::TitleLine;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for TitleLine {
        fn constructed(&self) {
            self.parent_constructed();
            let label = super::super::label("", &[]);
            label.set_parent(&*self.obj());
            self.label.replace(Some(label));
        }

        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for TitleLine {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (mut min, mut nat) = (0, 0);
            if let Some(label) = self.label.borrow().as_ref() {
                let (m, n, _, _) = label.measure(orientation, for_size);
                min = m;
                nat = n;
            }
            let badge = self.shown_badge();
            let indicator = self.shown_indicator();
            match orientation {
                gtk::Orientation::Horizontal => {
                    // As an ellipsized label, with or without the badge, so a
                    // row with a badge shares its width with the other
                    // columns as the rows without one (and the header) do;
                    // the badge takes its room from the title. The playing
                    // indicator keeps its own room.
                    let lead = indicator.map_or(0, |i| {
                        i.measure(gtk::Orientation::Horizontal, -1).1 + LEAD_GAP
                    });
                    (min + lead, min + lead, -1, -1)
                }
                _ => {
                    for child in [
                        badge.map(Cast::upcast::<gtk::Widget>),
                        indicator.map(Cast::upcast::<gtk::Widget>),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        let (m, n, _, _) = child.measure(orientation, -1);
                        min = min.max(m);
                        nat = nat.max(n);
                    }
                    (min, nat, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(label) = self.label.borrow().clone() else {
                return;
            };
            let rtl = self.obj().direction() == gtk::TextDirection::Rtl;
            // The playing indicator leads; the rest is laid out after it.
            let lead = match self.shown_indicator() {
                Some(indicator) => {
                    let w = indicator.measure(gtk::Orientation::Horizontal, -1).1;
                    let x = if rtl { (width - w).max(0) } else { 0 };
                    indicator.size_allocate(&gtk::Allocation::new(x, 0, w, height), -1);
                    (w + LEAD_GAP).min(width)
                }
                None => 0,
            };
            let width = width - lead;
            let offset = if rtl { 0 } else { lead };
            let badge = self.shown_badge();
            let badge_size = badge
                .as_ref()
                .map(|b| {
                    (
                        b.measure(gtk::Orientation::Horizontal, -1).1,
                        b.measure(gtk::Orientation::Vertical, -1).1,
                    )
                })
                .unwrap_or_default();
            let room = if badge.is_some() {
                (width - badge_size.0 - GAP).max(0)
            } else {
                width
            };
            let natural = label.measure(gtk::Orientation::Horizontal, -1).1;
            let text = if badge.is_some() {
                natural.min(room)
            } else {
                width
            };
            let x = if rtl { width - text } else { 0 };
            label.size_allocate(&gtk::Allocation::new(x + offset, 0, text, height), baseline);
            if let Some(badge) = badge {
                let left = (text + GAP).min(width - badge_size.0).max(0);
                let x = if rtl {
                    width - left - badge_size.0
                } else {
                    left
                };
                let y = ((height - badge_size.1) / 2).max(0);
                badge.size_allocate(
                    &gtk::Allocation::new(x + offset, y, badge_size.0, badge_size.1),
                    -1,
                );
            }
        }
    }

    impl TitleLine {
        pub(super) fn shown_badge(&self) -> Option<gtk::Button> {
            self.badge.borrow().clone().filter(WidgetExt::is_visible)
        }

        pub(super) fn shown_indicator(&self) -> Option<PlayingIndicator> {
            self.indicator
                .borrow()
                .clone()
                .filter(WidgetExt::is_visible)
        }
    }
}

glib::wrapper! {
    /// A title with the copies badge after it.
    pub(crate) struct TitleLine(ObjectSubclass<imp::TitleLine>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for TitleLine {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl TitleLine {
    /// A title line whose text has the style `classes`; it expands.
    pub(crate) fn new(text: &str, classes: &[&str]) -> Self {
        let line = Self::default();
        let label = line.label();
        label.set_text(crate::format::trimmed(text));
        for class in classes {
            label.add_css_class(class);
        }
        line.set_hexpand(true);
        line
    }

    /// The title.
    pub(crate) fn label(&self) -> gtk::Label {
        self.imp()
            .label
            .borrow()
            .clone()
            .unwrap_or_else(|| gtk::Label::new(None))
    }

    /// Shows `text`, without padding before and after it.
    pub(crate) fn set_text(&self, text: &str) {
        self.label().set_text(crate::format::trimmed(text));
    }

    /// Shows the badge of `copies` copies of `track`; none for 0.
    pub(crate) fn set_copies(&self, track: TrackKey, copies: u32) {
        let imp = self.imp();
        if copies == 0 && imp.badge.borrow().is_none() {
            return;
        }
        let badge = imp
            .badge
            .borrow_mut()
            .get_or_insert_with(|| {
                let badge = copies_badge::new();
                badge.set_parent(self);
                badge
            })
            .clone();
        let was = badge.is_visible();
        copies_badge::show(&badge, track, copies);
        if was != badge.is_visible() {
            self.queue_resize();
        }
    }

    /// Marks the title as the playing track's: accent text, weight 600, and
    /// with `indicator` the playing indicator before the text, for a list
    /// without a number column to put it in.
    pub(crate) fn set_playing(&self, playing: bool, indicator: bool) {
        let label = self.label();
        if playing {
            label.add_css_class("hk-playing");
        } else {
            label.remove_css_class("hk-playing");
        }
        let imp = self.imp();
        let show = playing && indicator;
        if !show && imp.indicator.borrow().is_none() {
            return;
        }
        let lead = imp
            .indicator
            .borrow_mut()
            .get_or_insert_with(|| {
                let lead = PlayingIndicator::default();
                lead.insert_after(self, None::<&gtk::Widget>);
                lead
            })
            .clone();
        if lead.is_visible() != show {
            lead.set_visible(show);
            self.queue_resize();
        }
    }
}
