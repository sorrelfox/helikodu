// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The first run: before a library exists, the main window asks where the
//! music is, without sidebar, panel or deck. The desktop's music folder is
//! listed and ticked; more folders come from the folder chooser (the portal
//! inside a sandbox). "Scan and start" stores the folders, starts the scan
//! and opens the library, which fills while the scan footer at the bottom of
//! the sidebar shows the progress. "Skip for now" opens the empty library.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::common::{compact_button, switch_row, tilde};
use super::settings::library::choose_folder;
use crate::backend::services::{DeviceSelection, ScanStatus};
use crate::i18n::{fill, tr};
use crate::popovers::output_switcher::backend_name;
use crate::widgets::{BrandMark, icon_button, label};
use crate::window::MainWindow;

const STACK: &str = "first-run-stack";

/// File name extensions counted as audio when estimating a folder's size.
const AUDIO: &[&str] = &[
    "flac", "mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "wv", "ape", "mpc",
    "wma", "dsf", "dff", "alac", "mka", "tta", "tak",
];

/// About how many audio files are under `folder`; stops counting at 100 000.
fn count_audio(folder: &Path) -> usize {
    let mut count = 0;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| AUDIO.contains(&e.to_ascii_lowercase().as_str()))
            {
                count += 1;
                if count >= 100_000 {
                    return count;
                }
            }
        }
    }
    count
}

/// "about 17,300": rounded so that it does not pretend to be exact.
fn about(count: usize) -> String {
    let rounded = match count {
        0..=99 => count,
        100..=9_999 => (count + 50) / 100 * 100,
        _ => (count + 500) / 1000 * 1000,
    };
    fill(&tr("about {} audio files"), &[&rounded.to_string()])
}

struct FolderChoice {
    path: PathBuf,
    check: gtk::CheckButton,
}

struct FirstRun {
    main: glib::WeakRef<MainWindow>,
    list: gtk::Box,
    add_row: gtk::Button,
    folders: RefCell<Vec<FolderChoice>>,
    watch: gtk::Switch,
    video: gtk::Switch,
}

impl FirstRun {
    fn add_folder(self: &Rc<Self>, path: PathBuf, music: bool) {
        if self.folders.borrow().iter().any(|f| f.path == path) {
            return;
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.add_css_class("hk-folder-row");
        let check = gtk::CheckButton::new();
        check.set_active(true);
        check.update_property(&[gtk::accessible::Property::Label(&tilde(&path))]);
        line.append(&check);
        let icon = gtk::Image::from_icon_name("folder-symbolic");
        icon.add_css_class("hk-t2");
        line.append(&icon);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&label(&tilde(&path), &["hk-mono", "hk-path"]));
        let kind = if music {
            tr("Your music folder")
        } else {
            tr("Added by you")
        };
        let line_note = label(&kind, &["hk-t3", "hk-secondary"]);
        text.append(&line_note);
        line.append(&text);
        let folder = path.clone();
        glib::spawn_future_local(async move {
            let count = gio::spawn_blocking(move || count_audio(&folder))
                .await
                .unwrap_or(0);
            line_note.set_text(&format!("{kind} · {}", about(count)));
        });
        if !music {
            let remove = icon_button(
                "window-close-symbolic",
                &fill(&tr("Remove {}"), &[&tilde(&path)]),
            );
            let weak = Rc::downgrade(self);
            let target = path.clone();
            let row = line.downgrade();
            remove.connect_clicked(move |_| {
                let (Some(page), Some(row)) = (weak.upgrade(), row.upgrade()) else {
                    return;
                };
                page.folders.borrow_mut().retain(|f| f.path != target);
                page.list.remove(&row);
            });
            line.append(&remove);
        }
        let click = gtk::GestureClick::new();
        let weak = check.downgrade();
        click.connect_released(move |gesture, _, x, y| {
            let (Some(check), Some(row)) = (weak.upgrade(), gesture.widget()) else {
                return;
            };
            if let Some(picked) = row.pick(x, y, gtk::PickFlags::DEFAULT)
                && (picked.is_ancestor(&check)
                    || picked == check.clone().upcast::<gtk::Widget>()
                    || picked.ancestor(gtk::Button::static_type()).is_some())
            {
                return;
            }
            check.set_active(!check.is_active());
        });
        line.add_controller(click);
        self.list
            .insert_child_after(&line, self.add_row.prev_sibling().as_ref());
        self.folders.borrow_mut().push(FolderChoice { path, check });
    }

    fn chosen(&self) -> Vec<PathBuf> {
        self.folders
            .borrow()
            .iter()
            .filter(|f| f.check.is_active())
            .map(|f| f.path.clone())
            .collect()
    }

    fn start(&self) {
        let Some(main) = self.main.upgrade() else {
            return;
        };
        let Some(backend) = main.backend() else {
            return;
        };
        let folders = self.chosen();
        let services = backend.services();
        let (watch, video) = (self.watch.is_active(), self.video.is_active());
        let stored = services.update_settings(&mut |s| {
            for folder in &folders {
                if !s.library.folders.contains(folder) {
                    s.library.folders.push(folder.clone());
                }
            }
            s.library.watch = watch;
            s.library.include_video = video;
        });
        if let Err(error) = stored {
            main.toast(&error.0, true);
            return;
        }
        if !folders.is_empty()
            && let Err(error) = services.scan(&folders)
        {
            main.toast(&error.0, true);
        }
        hide(&main);
    }
}

fn output_row(main: &MainWindow) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    row.add_css_class("hk-output-box");
    let icon = gtk::Image::from_icon_name("audio-speakers-symbolic");
    icon.add_css_class("hk-t2");
    row.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let sentence = gtk::Label::new(None);
    sentence.set_xalign(0.0);
    sentence.set_wrap(true);
    sentence.add_css_class("hk-t2");
    text.append(&sentence);
    text.append(&label(
        &tr("You can pick another device from the player bar at any time."),
        &["hk-t3", "hk-secondary"],
    ));
    row.append(&text);
    let choose = compact_button(&tr("Choose device"));
    row.append(&choose);
    if let (Some(backend), Some(playback)) = (main.backend(), main.playback()) {
        let show = {
            let backend = backend.clone();
            let sentence = sentence.downgrade();
            move || {
                let Some(sentence) = sentence.upgrade() else {
                    return;
                };
                let services = backend.services();
                let output = services.output();
                let device = match &output.device {
                    DeviceSelection::SystemDefault => tr("System default"),
                    DeviceSelection::Device(id) => id.as_str().to_owned(),
                };
                let through = backend_name(services.resolve_backend(output.backend));
                let t1 = crate::style::current().palette.t1.hex();
                let escape = |s: &str| glib::markup_escape_text(s).to_string();
                let bold = format!(
                    "<span weight=\"600\" foreground=\"{t1}\">{}</span>",
                    escape(&device)
                );
                sentence.set_markup(&fill(
                    &escape(&tr("Sound goes to {} through {}")),
                    &[&bold, &escape(&through)],
                ));
            }
        };
        show();
        crate::popovers::output_switcher::attach(main.upcast_ref(), &choose, &backend, &playback);
        playback.connect(move |_, change| {
            if change == crate::playback::Change::Output {
                show();
            }
        });
    }
    row
}

fn page(main: &MainWindow, integrated: bool) -> (gtk::Box, Rc<FirstRun>) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("hk-first-run");
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    strip.add_css_class("hk-strip");
    strip.add_css_class("hk-first-run-strip");
    strip.append(&BrandMark::default());
    let name = label("Helikodu", &["hk-brand"]);
    name.set_hexpand(true);
    strip.append(&name);
    if integrated {
        strip.append(&gtk::WindowControls::new(gtk::PackType::End));
    }
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&strip));
    root.append(&handle);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.add_css_class("hk-first-run-column");
    column.set_halign(gtk::Align::Center);
    column.set_valign(gtk::Align::Center);
    column.set_size_request(640, -1);
    column.append(&label(&tr("Welcome"), &["hk-section-label"]));
    let question = gtk::Label::new(Some(&tr("Where is your music?")));
    question.set_xalign(0.0);
    question.set_wrap(true);
    question.add_css_class("hk-hero-title");
    question.add_css_class("small");
    question.set_margin_top(6);
    column.append(&question);
    let lead = gtk::Label::new(Some(&tr(
        "Helikodu plays your files where they are. Nothing is copied, and nothing is changed until you edit tags or organize files.",
    )));
    lead.set_xalign(0.0);
    lead.set_wrap(true);
    lead.set_max_width_chars(70);
    lead.add_css_class("hk-lead");
    lead.set_margin_top(10);
    column.append(&lead);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("hk-bordered-list");
    list.set_margin_top(24);
    let add_row = gtk::Button::new();
    add_row.add_css_class("hk-add-row");
    let add_line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    add_line.append(&gtk::Image::from_icon_name("list-add-symbolic"));
    add_line.append(&gtk::Label::new(Some(&tr("Add another folder"))));
    add_row.set_child(Some(&add_line));
    list.append(&add_row);
    column.append(&list);

    let switches = gtk::Box::new(gtk::Orientation::Vertical, 14);
    switches.set_margin_top(24);
    let (watch_row, watch) = switch_row(
        &tr("Watch these folders"),
        Some(&tr("New and changed files appear on their own.")),
    );
    watch.set_active(true);
    switches.append(&watch_row);
    let (video_row, video) = switch_row(
        &tr("Include video files"),
        Some(&tr("Plays their sound only. No picture is ever shown.")),
    );
    switches.append(&video_row);
    column.append(&switches);

    let output = output_row(main);
    output.set_margin_top(22);
    column.append(&output);

    let offline = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    offline.set_margin_top(22);
    let lock = gtk::Image::from_icon_name("helikodu-lock-symbolic");
    lock.add_css_class("hk-t2");
    offline.append(&lock);
    offline.append(&label(
        &tr("Helikodu works offline. It never connects to the internet."),
        &["hk-t2"],
    ));
    column.append(&offline);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    buttons.set_margin_top(28);
    let start = gtk::Button::with_label(&tr("Scan and start"));
    start.add_css_class("hk-btn");
    start.add_css_class("primary");
    start.add_css_class("tall");
    buttons.append(&start);
    let skip = gtk::Button::with_label(&tr("Skip for now"));
    skip.add_css_class("hk-btn");
    skip.add_css_class("text");
    buttons.append(&skip);
    column.append(&buttons);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&column));
    root.append(&scroller);

    let first_run = Rc::new(FirstRun {
        main: main.downgrade(),
        list,
        add_row: add_row.clone(),
        folders: RefCell::new(Vec::new()),
        watch,
        video,
    });
    if let Some(music) = glib::user_special_dir(glib::UserDirectory::Music)
        .filter(|p| p.is_dir())
        .or_else(|| Some(glib::home_dir().join("Music")).filter(|p| p.is_dir()))
    {
        first_run.add_folder(music, true);
    }
    let weak = Rc::downgrade(&first_run);
    add_row.connect_clicked(move |button| {
        let Some(page) = weak.upgrade() else { return };
        let Some(window) = button.root().and_downcast::<gtk::Window>() else {
            return;
        };
        let weak = Rc::downgrade(&page);
        choose_folder(&window, move |path| {
            if let Some(page) = weak.upgrade() {
                page.add_folder(path, false);
            }
        });
    });
    let weak = Rc::downgrade(&first_run);
    start.connect_clicked(move |_| {
        if let Some(page) = weak.upgrade() {
            page.start();
        }
    });
    let weak = main.downgrade();
    skip.connect_clicked(move |_| {
        if let Some(main) = weak.upgrade() {
            hide(&main);
        }
    });
    (root, first_run)
}

/// Shows the first run in `main`.
pub(crate) fn show(main: &MainWindow) {
    let Some(content) = main.child() else { return };
    if let Ok(stack) = content.clone().downcast::<gtk::Stack>()
        && stack.widget_name() == STACK
    {
        stack.set_visible_child_name("first-run");
        return;
    }
    let integrated = main.titlebar().is_some();
    let (first_run, state) = page(main, integrated);
    main.set_child(None::<&gtk::Widget>);
    let stack = gtk::Stack::new();
    stack.set_widget_name(STACK);
    stack.add_named(&content, Some("main"));
    stack.add_named(&first_run, Some("first-run"));
    stack.set_visible_child_name("first-run");
    main.set_child(Some(&stack));
    // The page lives as long as the stack.
    let keep = RefCell::new(Some(state));
    stack.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
}

/// Leaves the first run for the library.
pub(crate) fn hide(main: &MainWindow) {
    if let Some(stack) = main.child().and_downcast::<gtk::Stack>()
        && stack.widget_name() == STACK
    {
        stack.set_visible_child_name("main");
        if let Some(page) = stack.child_by_name("first-run") {
            stack.remove(&page);
        }
    }
}

/// Whether the first run is shown in `main`.
pub(crate) fn is_shown(main: &MainWindow) -> bool {
    main.child().and_downcast::<gtk::Stack>().is_some_and(|s| {
        s.widget_name() == STACK && s.visible_child_name().as_deref() == Some("first-run")
    })
}

/// Shows the first run when the library has no folder yet.
pub(crate) fn show_if_needed(main: &MainWindow) {
    let Some(backend) = main.backend() else {
        return;
    };
    let services = backend.services();
    if !services.settings().library.folders.is_empty() {
        return;
    }
    let weak = main.downgrade();
    glib::spawn_future_local(async move {
        let folders = gio::spawn_blocking(move || services.library_folders())
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        if folders.is_empty()
            && let Some(main) = weak.upgrade()
        {
            show(&main);
        }
    });
}

/// The scan footer at the bottom of the sidebar: "Scanning library", the
/// share done, a bar and the folder being read. Hidden when no scan runs.
#[derive(Debug)]
pub(crate) struct ScanFooter {
    pub(crate) root: gtk::Box,
    share: gtk::Label,
    bar: gtk::ProgressBar,
    current: gtk::Label,
}

impl ScanFooter {
    pub(crate) fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("hk-scan-footer");
        root.set_visible(false);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = label(&tr("Scanning library"), &[]);
        title.set_hexpand(true);
        head.append(&title);
        let share = label("", &["hk-numeric", "hk-t2"]);
        head.append(&share);
        root.append(&head);
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("hk-scan-bar");
        bar.update_property(&[gtk::accessible::Property::Label(&tr("Scanning library"))]);
        root.append(&bar);
        let current = label("", &["hk-mono-meta", "hk-t3"]);
        current.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        root.append(&current);
        Self {
            root,
            share,
            bar,
            current,
        }
    }

    /// The footer whose root is `root`.
    pub(crate) fn from_root(root: gtk::Box) -> Option<Self> {
        let head = root.first_child()?;
        let share = head.last_child().and_downcast::<gtk::Label>()?;
        let bar = head.next_sibling().and_downcast::<gtk::ProgressBar>()?;
        let current = bar.next_sibling().and_downcast::<gtk::Label>()?;
        Some(Self {
            root,
            share,
            bar,
            current,
        })
    }

    /// Follows `status`; hides when the scan is over.
    pub(crate) fn update(&self, status: &ScanStatus) {
        if status.finished {
            self.root.set_visible(false);
            self.current.set_text("");
            return;
        }
        self.root.set_visible(true);
        if let Some(path) = &status.current {
            self.current.set_text(&tilde(path));
        }
        match status.fraction() {
            Some(fraction) => {
                self.bar.set_fraction(fraction);
                let percent = (fraction * 100.0).floor() as u32;
                self.share
                    .set_text(&fill(&tr("{} %"), &[&percent.to_string()]));
            }
            None => {
                self.bar.pulse();
                self.share.set_text("");
            }
        }
    }
}

/// The message after a scan.
pub(crate) fn finished_message(status: &ScanStatus) -> (String, bool) {
    match &status.error {
        Some(error) => (fill(&tr("The scan stopped: {}"), &[error]), true),
        None => (tr("The library scan is finished."), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_rounded() {
        assert_eq!(about(42), "about 42 audio files");
        assert_eq!(about(17_281), "about 17000 audio files");
        assert_eq!(about(1_249), "about 1200 audio files");
    }

    #[test]
    fn the_footer_shows_the_share() {
        let status = ScanStatus {
            done: 12,
            total: Some(100),
            ..ScanStatus::default()
        };
        assert_eq!(status.fraction(), Some(0.12));
    }
}
