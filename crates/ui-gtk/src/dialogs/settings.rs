// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Settings window: a navigation column, a header with the page title,
//! and the pages. Every control is bound to the service's settings store and
//! applies its change at once; a value the store refuses is named in a notice
//! under the header and the control returns to the stored value.
//!
//! Pages for later versions (DSP and EQ, Shortcuts) are listed but not
//! sensitive; controls for later versions on the other pages are shown
//! insensitive or left out.

mod advanced;
mod appearance;
pub(crate) mod library;
mod loudness;
mod output;
mod playback;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk::glib;
use gtk::prelude::*;

use super::common::{dialog_window, find_open, header_strip};
use crate::backend::Backend;
use crate::backend::services::{Services, Settings};
use crate::i18n::{fill, tr};
use crate::playback::Playback;
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "settings";

/// A page of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    /// Library folders, scanning and storage.
    Library,
    /// The player's behaviour.
    Playback,
    /// The audio interface.
    Output,
    /// Loudness normalization.
    Loudness,
    /// DSP and EQ (a later version).
    Dsp,
    /// Colours, window style, language, notifications.
    Appearance,
    /// Keyboard shortcuts (a later version).
    Shortcuts,
    /// Tags, the database, logs, developer mode, reset.
    Advanced,
}

impl Page {
    const ALL: [Self; 8] = [
        Self::Library,
        Self::Playback,
        Self::Output,
        Self::Loudness,
        Self::Dsp,
        Self::Appearance,
        Self::Shortcuts,
        Self::Advanced,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Playback => "playback",
            Self::Output => "output",
            Self::Loudness => "loudness",
            Self::Dsp => "dsp",
            Self::Appearance => "appearance",
            Self::Shortcuts => "shortcuts",
            Self::Advanced => "advanced",
        }
    }

    fn title(self) -> String {
        match self {
            Self::Library => tr("Library"),
            Self::Playback => tr("Playback"),
            Self::Output => tr("Audio output"),
            Self::Loudness => tr("Loudness"),
            Self::Dsp => tr("DSP and EQ"),
            Self::Appearance => tr("Appearance"),
            Self::Shortcuts => tr("Shortcuts"),
            Self::Advanced => tr("Advanced"),
        }
    }

    fn available(self) -> bool {
        !matches!(self, Self::Dsp | Self::Shortcuts)
    }
}

/// What every page works with.
pub(crate) struct Context {
    pub(crate) window: gtk::Window,
    pub(crate) main: glib::WeakRef<MainWindow>,
    pub(crate) backend: Backend,
    pub(crate) services: Arc<dyn Services>,
    pub(crate) playback: Option<Rc<Playback>>,
    notice: gtk::Revealer,
    notice_text: gtk::Label,
    stack: gtk::Stack,
    rebuild: RefCell<Option<Box<dyn Fn()>>>,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("settings::Context").finish_non_exhaustive()
    }
}

impl Context {
    /// The settings as they are now.
    pub(crate) fn settings(&self) -> Arc<Settings> {
        self.services.settings()
    }

    /// Changes the settings. A refused change is named in the notice; the
    /// result says whether it was made.
    pub(crate) fn update(&self, mut change: impl FnMut(&mut Settings)) -> bool {
        match self.services.update_settings(&mut change) {
            Ok(()) => {
                self.notice.set_reveal_child(false);
                true
            }
            Err(error) => {
                self.notice_text
                    .set_text(&fill(&tr("This change was not saved: {}"), &[&error.0]));
                self.notice.set_reveal_child(true);
                false
            }
        }
    }

    /// Shows a toast in the main window.
    pub(crate) fn toast(&self, text: &str) {
        if let Some(main) = self.main.upgrade() {
            main.toast(text, false);
        }
    }

    /// Builds every page again from the stored settings, keeping the page shown.
    pub(crate) fn rebuild(&self) {
        if let Some(rebuild) = self.rebuild.borrow().as_ref() {
            rebuild();
        }
    }
}

/// A scrolled page body with the design's padding and row spacing.
pub(crate) fn page_body() -> (gtk::ScrolledWindow, gtk::Box) {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    body.add_css_class("hk-settings-page");
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&body));
    (scroller, body)
}

fn build_page(page: Page, context: &Rc<Context>) -> gtk::Widget {
    match page {
        Page::Library => library::page(context),
        Page::Playback => playback::page(context),
        Page::Output => output::page(context),
        Page::Loudness => loudness::page(context),
        Page::Appearance => appearance::page(context),
        Page::Advanced => advanced::page(context),
        Page::Dsp | Page::Shortcuts => gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
    }
}

/// Opens the Settings window over `main` at `page` (or the page shown last),
/// or brings it to the front.
pub(crate) fn open(main: &MainWindow, page: Option<Page>) -> gtk::Window {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        if let Some(page) = page
            && let Some(stack) = find_stack(&window)
        {
            stack.set_visible_child_name(page.id());
        }
        window.present();
        return window;
    }
    let Some(backend) = main.backend() else {
        return dialog_window(Some(parent), NAME, &tr("Settings"), (1080, 780), false);
    };
    let window = dialog_window(Some(parent), NAME, &tr("Settings"), (1080, 780), false);
    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);

    // Navigation.
    let nav = gtk::Box::new(gtk::Orientation::Vertical, 0);
    nav.add_css_class("hk-settings-nav");
    nav.set_size_request(224, -1);
    let nav_head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    nav_head.add_css_class("hk-strip");
    nav_head.add_css_class("plain");
    nav_head.append(&label(&tr("Settings"), &["hk-title"]));
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&nav_head));
    nav.append(&handle);
    let search = gtk::SearchEntry::new();
    search.add_css_class("hk-input");
    search.set_placeholder_text(Some(&tr("Search settings")));
    search.set_margin_start(8);
    search.set_margin_end(8);
    search.set_margin_bottom(8);
    // Searching the settings comes in a later version.
    search.set_sensitive(false);
    nav.append(&search);
    let list = gtk::ListBox::new();
    list.add_css_class("hk-list");
    list.set_margin_start(8);
    list.set_margin_end(8);
    list.update_property(&[gtk::accessible::Property::Label(&tr("Settings pages"))]);
    for page in Page::ALL {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(page.id());
        row.set_child(Some(&label(&page.title(), &[])));
        row.set_sensitive(page.available());
        list.append(&row);
    }
    nav.append(&list);
    layout.append(&nav);

    // Header, notice and pages.
    let main_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_column.set_hexpand(true);
    main_column.add_css_class("hk-settings-main");
    let (header, strip) = header_strip(&window, "", Some(&tr("Changes apply immediately")));
    main_column.append(&header);
    let notice_text = label("", &[]);
    notice_text.set_wrap(true);
    notice_text.set_hexpand(true);
    let notice_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    notice_box.add_css_class("hk-notice");
    notice_box.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
    notice_box.append(&notice_text);
    notice_box.update_property(&[gtk::accessible::Property::Label(&tr("Problem"))]);
    let notice = gtk::Revealer::new();
    notice.set_child(Some(&notice_box));
    main_column.append(&notice);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_widget_name("settings-stack");
    main_column.append(&stack);
    layout.append(&main_column);
    window.set_child(Some(&layout));

    let context = Rc::new(Context {
        window: window.clone(),
        main: main.downgrade(),
        services: backend.services(),
        backend,
        playback: main.playback(),
        notice,
        notice_text,
        stack: stack.clone(),
        rebuild: RefCell::new(None),
    });
    let weak = Rc::downgrade(&context);
    context.rebuild.replace(Some(Box::new(move || {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let shown = context.stack.visible_child_name();
        while let Some(child) = context.stack.first_child() {
            context.stack.remove(&child);
        }
        for page in Page::ALL {
            context
                .stack
                .add_named(&build_page(page, &context), Some(page.id()));
        }
        if let Some(shown) = shown {
            context.stack.set_visible_child_name(&shown);
        }
    })));
    context.rebuild();

    let title = strip
        .first_child()
        .and_downcast::<gtk::Label>()
        .unwrap_or_default();
    let weak_list = list.downgrade();
    stack.connect_visible_child_name_notify(move |stack| {
        let Some(name) = stack.visible_child_name() else {
            return;
        };
        if let Some(page) = Page::ALL.iter().find(|p| p.id() == name.as_str()) {
            title.set_text(&page.title());
        }
        if let Some(list) = weak_list.upgrade() {
            let mut child = list.first_child();
            while let Some(row) = child.and_downcast::<gtk::ListBoxRow>() {
                if row.widget_name() == name.as_str() {
                    list.select_row(Some(&row));
                    break;
                }
                child = row.next_sibling();
            }
        }
    });
    let weak_stack = stack.downgrade();
    list.connect_row_selected(move |_, row| {
        if let (Some(stack), Some(row)) = (weak_stack.upgrade(), row) {
            stack.set_visible_child_name(row.widget_name().as_str());
        }
    });
    // The context lives as long as the window.
    let keep = RefCell::new(Some(context));
    window.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    let first = page.unwrap_or(Page::Library);
    stack.set_visible_child_name(first.id());
    if first == Page::Library {
        // The notify does not fire for the page already shown.
        stack.notify("visible-child-name");
    }
    window.present();
    window
}

fn find_stack(window: &gtk::Window) -> Option<gtk::Stack> {
    let mut pending = vec![window.child()?];
    while let Some(widget) = pending.pop() {
        if widget.widget_name() == "settings-stack" {
            return widget.downcast().ok();
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c);
        }
    }
    None
}

/// The page shown in the Settings window over `main`, if it is open.
pub(crate) fn shown_page(main: &MainWindow) -> Option<String> {
    let window = find_open(main.upcast_ref(), NAME)?;
    find_stack(&window)?
        .visible_child_name()
        .map(|s| s.to_string())
}
