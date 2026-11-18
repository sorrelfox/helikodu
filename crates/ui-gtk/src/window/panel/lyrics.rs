// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Lyrics tab of the panel: the playing track in one row, its lyrics, and
//! a footer with their source, the offset of the track and a menu.
//!
//! Synced lyrics are buttons that seek, with the line sung now in the middle;
//! plain lyrics are text; without lyrics the tab says where Helikodu looks.
//! The actions (`win.lyrics-*`) are the window's, shared with Now Playing.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

use crate::backend::{LyricsOrigin, LyricsView};
use crate::i18n::{fill, tr};
use crate::models::CoverCache;
use crate::models::lyrics::{LyricsChange, LyricsModel};
use crate::playback::{Change, Playback, PlayerView};
use crate::widgets::lyric_lines::{LinesStyle, LyricLines};
use crate::widgets::{CoverArt, empty_state, label};

/// The source of `lyrics` in words, as the footers show it.
pub(crate) fn source_text(lyrics: Option<&LyricsView>) -> String {
    match lyrics {
        None => tr("No lyrics"),
        Some(l) => {
            let from = match l.origin {
                LyricsOrigin::LrcFile => tr(".lrc file"),
                LyricsOrigin::TextFile => tr(".txt file"),
                LyricsOrigin::Tags => tr("from the tags of the file"),
            };
            if l.synced {
                fill(&tr("Synced · {}"), &[&from])
            } else {
                fill(&tr("Not synced · {}"), &[&from])
            }
        }
    }
}

/// An offset in milliseconds as `0.0 s`, `+0.3 s` or `−1.2 s`.
pub(crate) fn offset_text(offset_ms: i64) -> String {
    let tenths = (offset_ms as f64 / 100.0).round() as i64;
    let sign = match tenths.signum() {
        1 => "+",
        -1 => "\u{2212}",
        _ => "",
    };
    let abs = tenths.unsigned_abs();
    fill(&tr("{} s"), &[&format!("{sign}{}.{}", abs / 10, abs % 10)])
}

/// The menu of the footer's more button.
pub(crate) fn lyrics_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let view = gio::Menu::new();
    view.append(Some(&tr("Show Times")), Some("win.lyrics-show-times"));
    menu.append_section(None, &view);
    let file = gio::Menu::new();
    file.append(Some(&tr("Reload")), Some("win.lyrics-reload"));
    file.append(
        Some(&tr("Open the Folder of the Lyrics File")),
        Some("win.lyrics-open-folder"),
    );
    file.append(Some(&tr("Edit Lyrics…")), Some("win.lyrics-edit"));
    menu.append_section(None, &file);
    menu
}

/// The tab's widgets.
#[derive(Debug)]
pub(crate) struct LyricsPage {
    root: gtk::Box,
    cover: CoverArt,
    title: gtk::Label,
    subtitle: gtk::Label,
    stack: gtk::Stack,
    lines: Rc<LyricLines>,
    source: gtk::Label,
    offset_box: gtk::Box,
    offset: gtk::Label,
    covers: Rc<CoverCache>,
}

impl LyricsPage {
    /// The tab, following `playback` and `lyrics`; activating a line seeks with
    /// `seek`.
    pub(crate) fn new(
        playback: &Rc<Playback>,
        lyrics: &Rc<LyricsModel>,
        covers: &Rc<CoverCache>,
        seek: impl Fn(std::time::Duration) + 'static,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-lyrics-page");

        let track = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        track.add_css_class("hk-lyrics-track");
        let cover = CoverArt::new(40, 4.0);
        cover.add_css_class("small");
        cover.set_valign(gtk::Align::Center);
        cover.set_tooltip_text(Some(&tr("Show Now Playing")));
        cover.set_cursor_from_name(Some("pointer"));
        let click = gtk::GestureClick::new();
        click.connect_released(|gesture, _, _, _| {
            if let Some(widget) = gesture.widget() {
                let _ = widget.activate_action("win.now-playing", None);
            }
        });
        cover.add_controller(click);
        track.append(&cover);
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
        texts.set_valign(gtk::Align::Center);
        texts.set_hexpand(true);
        let title = label("", &["hk-lyrics-track-title"]);
        title.set_max_width_chars(1);
        let subtitle = label("", &["hk-t2", "hk-secondary"]);
        subtitle.set_max_width_chars(1);
        texts.append(&title);
        texts.append(&subtitle);
        track.append(&texts);
        root.append(&track);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        let lines = LyricLines::new(LinesStyle::Panel);
        lines.set_show_times(false);
        lines.connect_seek(seek);
        stack.add_named(lines.widget(), Some("lines"));
        let add = gtk::Button::with_label(&tr("Add lyrics…"));
        add.add_css_class("hk-btn");
        add.add_css_class("compact");
        add.set_action_name(Some("win.lyrics-edit"));
        let empty = empty_state::empty_state(
            "format-justify-left-symbolic",
            &tr("No lyrics for this track"),
            &tr(
                "Helikodu looks for a .lrc or .txt file with the name of the track in its folder, and for lyrics in its tags.",
            ),
            &[add],
        );
        empty.add_css_class("hk-lyrics-empty");
        stack.add_named(&empty, Some("empty"));
        root.append(&stack);

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        footer.add_css_class("hk-panel-footer");
        footer.add_css_class("hk-lyrics-footer");
        let source = label("", &["hk-t3", "hk-secondary"]);
        source.set_hexpand(true);
        source.set_max_width_chars(1);
        footer.append(&source);
        let offset_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        offset_box.set_valign(gtk::Align::Center);
        let offset_name = label(&tr("Offset"), &["hk-t3", "hk-secondary"]);
        offset_name.set_margin_end(4);
        offset_box.append(&offset_name);
        let minus =
            crate::widgets::icon_button("value-decrease-symbolic", &tr("Show the lines later"));
        minus.set_action_name(Some("win.lyrics-later"));
        minus.add_css_class("small");
        offset_box.append(&minus);
        let offset = label("", &["hk-mono", "hk-lyrics-offset"]);
        offset.set_xalign(0.5);
        offset.set_width_chars(6);
        offset.set_accessible_role(gtk::AccessibleRole::Status);
        offset_box.append(&offset);
        let plus =
            crate::widgets::icon_button("value-increase-symbolic", &tr("Show the lines sooner"));
        plus.set_action_name(Some("win.lyrics-sooner"));
        plus.add_css_class("small");
        offset_box.append(&plus);
        footer.append(&offset_box);
        let more = gtk::MenuButton::new();
        more.set_icon_name("view-more-symbolic");
        more.set_valign(gtk::Align::Center);
        more.set_tooltip_text(Some(&tr("More")));
        more.update_property(&[gtk::accessible::Property::Label(&tr("More"))]);
        more.add_css_class("hk-icon");
        more.add_css_class("small");
        more.set_menu_model(Some(&lyrics_menu()));
        crate::widgets::menu_names::name_menus(&more);
        footer.append(&more);
        root.append(&footer);

        let page = Rc::new(Self {
            root,
            cover,
            title,
            subtitle,
            stack,
            lines,
            source,
            offset_box,
            offset,
            covers: covers.clone(),
        });
        let weak = Rc::downgrade(&page);
        playback.connect(move |view, change| {
            if let Some(page) = weak.upgrade()
                && change == Change::Track
            {
                page.show_track(view);
            }
        });
        let weak = Rc::downgrade(&page);
        lyrics.connect(move |model, change| {
            let Some(page) = weak.upgrade() else { return };
            match change {
                LyricsChange::Lyrics => page.show_lyrics(model),
                LyricsChange::Line => page.lines.set_line(model.line()),
            }
        });
        page.show_lyrics(lyrics);
        page
    }

    /// The tab's root.
    pub(crate) fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// The lines widget, which keeps the "show times" choice.
    pub(crate) fn lines(&self) -> &Rc<LyricLines> {
        &self.lines
    }

    fn show_track(&self, view: &PlayerView) {
        match &view.now {
            Some(now) => {
                self.covers.show(&self.cover, now.cover, now.cover_seed, 40);
                self.title.set_text(&now.track.title);
                let parts: Vec<&str> = [now.track.artist.as_str(), now.album_title.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect();
                self.subtitle.set_text(&parts.join(" · "));
            }
            None => {
                self.cover.show(0, None);
                self.title.set_text(&tr("Nothing playing"));
                self.subtitle.set_text("");
            }
        }
    }

    fn show_lyrics(&self, model: &LyricsModel) {
        let lyrics = model.lyrics();
        self.lines.set_lyrics(lyrics.as_deref(), model.line());
        self.stack
            .set_visible_child_name(if lyrics.is_some() { "lines" } else { "empty" });
        self.source.set_text(&source_text(lyrics.as_deref()));
        let synced = lyrics.as_ref().is_some_and(|l| l.synced);
        self.offset_box.set_visible(synced);
        if let Some(l) = &lyrics {
            self.offset.set_text(&offset_text(l.offset_ms));
        }
    }

    /// The lines as text and the current one, for tests.
    pub(crate) fn texts(&self) -> (Vec<String>, Option<usize>, String) {
        let (lines, current) = self.lines.texts();
        (lines, current, self.source.text().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_in_tenths() {
        assert_eq!(offset_text(0), "0.0 s");
        assert_eq!(offset_text(300), "+0.3 s");
        assert_eq!(offset_text(-1_250), "\u{2212}1.3 s");
        assert_eq!(offset_text(40), "0.0 s");
    }
}
