// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The lines of the lyrics, scrolled so the line sung now stays in the middle.
//!
//! Synced lyrics are one button per line: activating it seeks there. Earlier
//! lines, the current one and later ones carry the classes `past`, `current`
//! and `future`; the current one is also bold, so the state is not in colour
//! alone. Plain lyrics are text: no current line, no seeking, scrolled by hand.
//!
//! The list scrolls only when the current line changes, and only while it is
//! mapped: nothing is drawn while it is hidden or playback is paused. Under
//! reduced motion it jumps instead of gliding. Scrolling by hand stops the
//! following until the next seek, a click on a line or other lyrics.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{glib, graphene};

use crate::backend::LyricsView;
use crate::format;

/// How long the glide to the next line takes.
const GLIDE: Duration = Duration::from_millis(280);

/// Where the lines are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinesStyle {
    /// The Lyrics tab of the panel: the panel's colours, no times.
    Panel,
    /// Now Playing: large lines in the glass colours, with their times.
    Glass,
}

type SeekHandler = Box<dyn Fn(Duration)>;

/// The lines and their scroller.
pub(crate) struct LyricLines {
    scroller: gtk::ScrolledWindow,
    column: gtk::Box,
    rows: RefCell<Vec<gtk::Widget>>,
    times: RefCell<Vec<Option<Duration>>>,
    current: Cell<Option<usize>>,
    show_times: Cell<bool>,
    /// Whether the list follows the current line.
    follow: Cell<bool>,
    /// A scroll that waits for the rows' sizes.
    pending: Cell<bool>,
    /// The glide in progress, and whether it has ended by itself.
    glide: RefCell<Option<(gtk::TickCallbackId, Rc<Cell<bool>>)>>,
    seek: RefCell<Option<SeekHandler>>,
}

impl std::fmt::Debug for LyricLines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LyricLines")
            .field("rows", &self.rows.borrow().len())
            .field("current", &self.current.get())
            .finish()
    }
}

impl LyricLines {
    /// Empty lines in `style`.
    pub(crate) fn new(style: LinesStyle) -> Rc<Self> {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.add_css_class("hk-lyrics");
        column.set_accessible_role(gtk::AccessibleRole::List);
        column.update_property(&[gtk::accessible::Property::Label(&crate::i18n::tr("Lyrics"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&column));
        match style {
            LinesStyle::Panel => scroller.add_css_class("hk-lyrics-panel"),
            LinesStyle::Glass => scroller.add_css_class("hk-lyrics-glass"),
        }
        let lines = Rc::new(Self {
            scroller,
            column,
            rows: RefCell::new(Vec::new()),
            times: RefCell::new(Vec::new()),
            current: Cell::new(None),
            show_times: Cell::new(style == LinesStyle::Glass),
            follow: Cell::new(true),
            pending: Cell::new(false),
            glide: RefCell::new(None),
            seek: RefCell::new(None),
        });
        // Scrolling by hand stops the following.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&lines);
        scroll.connect_scroll(move |_, _, _| {
            if let Some(lines) = weak.upgrade() {
                lines.follow.set(false);
                lines.stop_glide();
            }
            glib::Propagation::Proceed
        });
        lines.scroller.add_controller(scroll);
        // The rows have their sizes after layout; a jump asked for before
        // happens then, and when the list is shown again.
        let weak = Rc::downgrade(&lines);
        lines.scroller.vadjustment().connect_changed(move |_| {
            if let Some(lines) = weak.upgrade()
                && lines.pending.get()
            {
                lines.scroll_to_current(false);
            }
        });
        let weak = Rc::downgrade(&lines);
        lines.scroller.connect_map(move |_| {
            if let Some(lines) = weak.upgrade() {
                lines.pending.set(true);
                lines.scroll_to_current(false);
            }
        });
        lines
    }

    /// The scroller.
    pub(crate) fn widget(&self) -> &gtk::ScrolledWindow {
        &self.scroller
    }

    /// Calls `seek` with the time of a line the user activates.
    pub(crate) fn connect_seek(&self, seek: impl Fn(Duration) + 'static) {
        self.seek.replace(Some(Box::new(seek)));
    }

    /// Shows the time of each synced line, or not.
    pub(crate) fn set_show_times(self: &Rc<Self>, show: bool) {
        if self.show_times.replace(show) != show {
            for row in self.rows.borrow().iter() {
                if let Some(time) = row
                    .first_child()
                    .and_then(|b| b.first_child())
                    .filter(|w| w.has_css_class("hk-lyric-time"))
                {
                    time.set_visible(show);
                }
            }
        }
    }

    /// Shows `lyrics` (none: no lines) with `line` sung now.
    pub(crate) fn set_lyrics(self: &Rc<Self>, lyrics: Option<&LyricsView>, line: Option<usize>) {
        self.stop_glide();
        while let Some(child) = self.column.first_child() {
            self.column.remove(&child);
        }
        let mut rows = Vec::new();
        let mut times = Vec::new();
        if let Some(lyrics) = lyrics {
            for (index, line) in lyrics.lines.iter().enumerate() {
                let row = if lyrics.synced {
                    self.synced_row(index, line.time, &line.text)
                } else {
                    plain_row(&line.text)
                };
                if line.verse_start {
                    row.add_css_class("verse");
                }
                self.column.append(&row);
                rows.push(row);
                times.push(line.time.filter(|_| lyrics.synced));
            }
        }
        self.rows.replace(rows);
        self.times.replace(times);
        self.current.set(None);
        self.follow.set(true);
        self.scroller.vadjustment().set_value(0.0);
        self.mark(line);
        self.pending.set(true);
        self.scroll_to_current(false);
    }

    /// Makes `line` the current one and scrolls to it.
    pub(crate) fn set_line(self: &Rc<Self>, line: Option<usize>) {
        let before = self.current.get();
        if before == line {
            return;
        }
        // A jump other than to the next line is a seek: follow again.
        let jump = match (before, line) {
            (Some(a), Some(b)) => b != a + 1,
            _ => true,
        };
        if jump {
            self.follow.set(true);
        }
        self.mark(line);
        self.scroll_to_current(!jump);
    }

    /// Sets the classes of the rows between the old and the new current line.
    fn mark(&self, line: Option<usize>) {
        let rows = self.rows.borrow();
        let times = self.times.borrow();
        if times.iter().all(Option::is_none) {
            self.current.set(None);
            return;
        }
        let line = line.filter(|&l| l < rows.len());
        let state = |index: usize| match line {
            Some(l) if index < l => "past",
            Some(l) if index == l => "current",
            _ => "future",
        };
        let old = self.current.get().filter(|&l| l < rows.len());
        // Only the rows whose state can change: between the two lines.
        let range = match (old, line) {
            (Some(a), Some(b)) => a.min(b)..=a.max(b),
            (Some(a), None) | (None, Some(a)) => 0..=a,
            (None, None) => 0..=rows.len().saturating_sub(1),
        };
        for index in range {
            let Some(row) = rows.get(index) else { continue };
            let wanted = state(index);
            for class in ["past", "current", "future"] {
                if class == wanted {
                    if !row.has_css_class(class) {
                        row.add_css_class(class);
                    }
                } else if row.has_css_class(class) {
                    row.remove_css_class(class);
                }
            }
        }
        self.current.set(line);
    }

    fn synced_row(
        self: &Rc<Self>,
        index: usize,
        time: Option<Duration>,
        text: &str,
    ) -> gtk::Widget {
        let button = gtk::Button::new();
        button.add_css_class("hk-lyric");
        button.add_css_class("future");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        let stamp = gtk::Label::new(Some(&time.map(format::duration).unwrap_or_default()));
        stamp.add_css_class("hk-lyric-time");
        stamp.set_xalign(1.0);
        stamp.set_valign(gtk::Align::Baseline);
        stamp.set_visible(self.show_times.get());
        line.append(&stamp);
        let words = gtk::Label::new(Some(text));
        words.add_css_class("hk-lyric-text");
        words.set_wrap(true);
        words.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        words.set_xalign(0.0);
        words.set_hexpand(true);
        words.set_valign(gtk::Align::Baseline);
        line.append(&words);
        button.set_child(Some(&line));
        if let Some(time) = time {
            button.update_property(&[
                gtk::accessible::Property::Label(text),
                gtk::accessible::Property::Description(&crate::i18n::fill(
                    &crate::i18n::tr("Play from {}"),
                    &[&format::duration(time)],
                )),
            ]);
        }
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            let Some(lines) = weak.upgrade() else { return };
            let time = lines.times.borrow().get(index).copied().flatten();
            if let (Some(time), Some(seek)) = (time, lines.seek.borrow().as_ref()) {
                lines.follow.set(true);
                seek(time);
            }
        });
        button.upcast()
    }

    /// Scrolls so the current line is in the middle, as far as the list goes.
    fn scroll_to_current(&self, glide: bool) {
        if !self.follow.get() || !self.scroller.is_mapped() {
            return;
        }
        let rows = self.rows.borrow();
        let Some(row) = self.current.get().and_then(|l| rows.get(l)) else {
            self.pending.set(false);
            return;
        };
        let Some(top) = row.compute_point(&self.column, &graphene::Point::new(0.0, 0.0)) else {
            return;
        };
        let height = f64::from(row.height());
        if height <= 0.0 {
            // Not laid out yet: the adjustment's next change brings the sizes.
            self.pending.set(true);
            return;
        }
        self.pending.set(false);
        let adjustment = self.scroller.vadjustment();
        let page = adjustment.page_size();
        let target = (f64::from(top.y()) + height / 2.0 - page / 2.0).clamp(
            adjustment.lower(),
            (adjustment.upper() - page).max(adjustment.lower()),
        );
        self.stop_glide();
        if !glide || crate::style::current().reduced_motion {
            adjustment.set_value(target);
            return;
        }
        let from = adjustment.value();
        if (target - from).abs() < 0.5 {
            return;
        }
        let start = Cell::new(None::<i64>);
        let ended = Rc::new(Cell::new(false));
        let done = Rc::clone(&ended);
        let id = self.scroller.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            let begun = *start.get().get_or_insert(now);
            let t = ((now - begun) as f64 / GLIDE.as_micros() as f64).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            adjustment.set_value(from + (target - from) * eased);
            if t >= 1.0 {
                done.set(true);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        self.glide.replace(Some((id, ended)));
    }

    fn stop_glide(&self) {
        if let Some((id, ended)) = self.glide.take()
            && !ended.get()
        {
            id.remove();
        }
    }

    /// Activates the row of line `index`, as a click does; whether it is one.
    pub(crate) fn activate(&self, index: usize) -> bool {
        self.rows
            .borrow()
            .get(index)
            .and_then(|r| r.downcast_ref::<gtk::Button>())
            .map(|b| b.emit_clicked())
            .is_some()
    }

    /// The texts of the rows and which one is current, for tests.
    pub(crate) fn texts(&self) -> (Vec<String>, Option<usize>) {
        let texts = self
            .rows
            .borrow()
            .iter()
            .map(|row| {
                let mut label = row.clone();
                while !label.is::<gtk::Label>() || label.has_css_class("hk-lyric-time") {
                    let next = if label.is::<gtk::Label>() {
                        label.next_sibling()
                    } else {
                        label.first_child()
                    };
                    match next {
                        Some(next) => label = next,
                        None => return String::new(),
                    }
                }
                label
                    .downcast::<gtk::Label>()
                    .map(|l| l.text().to_string())
                    .unwrap_or_default()
            })
            .collect();
        (texts, self.current.get())
    }
}

/// A line of plain lyrics: text, not a button.
fn plain_row(text: &str) -> gtk::Widget {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("hk-lyric");
    label.add_css_class("plain");
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_accessible_role(gtk::AccessibleRole::ListItem);
    label.upcast()
}
