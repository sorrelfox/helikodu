// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What comes after the last row of the queue while endless play or the
//! Auto-DJ is on: what they add next, or that nothing is left to add, with
//! "Change…" and "Turn off"; and the spark that marks the rows they added.

use gtk::glib;
use gtk::prelude::*;
use helikodu_model::settings::{PlaybackSettings, SeedKind};

use crate::backend::{AlbumKey, QueueSnapshot, RadioSeed, RepeatMode, TrackRow};
use crate::i18n::{fill, ntr, tr};
use crate::related::Alike;

/// The widget name of the "When the queue ends" group on Settings › Playback.
pub(crate) const SETTINGS_ROW: &str = "when-queue-ends";

/// The icon of a row Helikodu added.
const SPARK: &str = "helikodu-spark-symbolic";

/// The words of the block: its title, the line under it (with markup), and
/// whether it warns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EndWords {
    pub(crate) title: String,
    pub(crate) detail: Option<String>,
    pub(crate) warning: bool,
}

/// `icon` when the icon theme has it, else `fallback`.
fn icon_or(icon: &str, fallback: &str) -> String {
    let has = gtk::gdk::Display::default()
        .is_some_and(|display| gtk::IconTheme::for_display(&display).has_icon(icon));
    if has { icon } else { fallback }.to_owned()
}

/// The spark of a row Helikodu added: 12 px in `t3`, described for screen
/// readers.
pub(crate) fn spark() -> gtk::Image {
    let mark = gtk::Image::from_icon_name(SPARK);
    mark.set_pixel_size(12);
    mark.add_css_class("hk-auto-mark");
    mark.set_valign(gtk::Align::Center);
    let words = tr("Added by Helikodu");
    mark.set_tooltip_text(Some(&words));
    mark.update_property(&[gtk::accessible::Property::Description(&words)]);
    mark
}

/// A query as the block shows it, in the mono font.
fn mono(text: &str) -> String {
    format!(
        "<span font_family=\"monospace\">{}</span>",
        glib::markup_escape_text(text)
    )
}

/// The track endless play continues from, as the queue shows it: the radio's
/// seed, else the last library track of the queue.
fn seed_words(queue: &QueueSnapshot, settings: &PlaybackSettings) -> Option<(String, String)> {
    let rows = &queue.items;
    match queue.radio {
        Some(RadioSeed::Track(id)) => rows
            .iter()
            .find(|t| t.key.0 == id)
            .map(|t| (t.title.clone(), t.artist.clone())),
        Some(RadioSeed::Album(id)) => {
            let album = AlbumKey(u64::try_from(id).unwrap_or(0));
            rows.iter()
                .find(|t| t.album == album && !t.album_title.is_empty())
                .map(|t| (t.album_title.clone(), t.artist.clone()))
        }
        Some(RadioSeed::Artist(_)) => queue
            .context
            .clone()
            .or_else(|| rows.first().map(|t| t.artist.clone()))
            .map(|name| (name, String::new())),
        None => {
            let last: &TrackRow = rows.iter().rev().find(|t| t.key.0 != 0)?;
            Some(match settings.endless.seed {
                SeedKind::Track => (last.title.clone(), last.artist.clone()),
                SeedKind::Album if !last.album_title.is_empty() => {
                    (last.album_title.clone(), last.artist.clone())
                }
                SeedKind::Album => (last.title.clone(), last.artist.clone()),
                SeedKind::Artist => (last.artist.clone(), String::new()),
            })
        }
    }
}

/// The words of the block for `repeat`, or `None` when the mode adds nothing.
pub(crate) fn words(
    repeat: RepeatMode,
    queue: &QueueSnapshot,
    settings: &PlaybackSettings,
) -> Option<EndWords> {
    match repeat {
        RepeatMode::Endless => {
            let filter = settings.endless.filter.trim();
            if queue.nothing_to_add {
                let detail = if filter.is_empty() {
                    glib::markup_escape_text(&tr(
                        "No more related tracks: every one was played or queued lately.",
                    ))
                    .to_string()
                } else {
                    fill(
                        &glib::markup_escape_text(&tr("No related track matches the filter, {}.")),
                        &[&mono(filter)],
                    )
                };
                return Some(EndWords {
                    title: tr("Nothing more to add"),
                    detail: Some(detail),
                    warning: true,
                });
            }
            let batch = settings.endless.batch as usize;
            let how = fill(
                &ntr("{} at a time, {}", "{} at a time, {}", batch),
                &[
                    &batch.to_string(),
                    &Alike::of(&settings.endless.weights).by(),
                ],
            );
            Some(match seed_words(queue, settings) {
                Some((name, artist)) => EndWords {
                    title: fill(&tr("Then: tracks related to “{}”"), &[&name]),
                    detail: Some(
                        glib::markup_escape_text(&if artist.is_empty() {
                            how
                        } else {
                            fill(&tr("{} · {}"), &[&artist, &how])
                        })
                        .to_string(),
                    ),
                    warning: false,
                },
                None => EndWords {
                    title: tr("Then: tracks related to the last track"),
                    detail: Some(glib::markup_escape_text(&how).to_string()),
                    warning: false,
                },
            })
        }
        RepeatMode::AutoDj => {
            let filter = settings.auto_dj.filter.trim();
            if queue.nothing_to_add {
                let detail = if filter.is_empty() {
                    glib::markup_escape_text(&tr(
                        "No track of the library can be added. The music stops after the last track.",
                    ))
                    .to_string()
                } else {
                    fill(
                        &glib::markup_escape_text(&tr(
                            "No track matches the Auto-DJ filter, {}. The music stops after the last track.",
                        )),
                        &[&mono(filter)],
                    )
                };
                return Some(EndWords {
                    title: tr("Nothing more to add"),
                    detail: Some(detail),
                    warning: true,
                });
            }
            if queue.items.is_empty() {
                return Some(EndWords {
                    title: tr("Auto-DJ is on. Press Play and it fills the queue."),
                    detail: None,
                    warning: false,
                });
            }
            let ahead = settings.auto_dj.ahead as usize;
            let detail = if filter.is_empty() {
                glib::markup_escape_text(&tr("From the whole library")).to_string()
            } else {
                fill(
                    &glib::markup_escape_text(&tr("Only tracks that match {}")),
                    &[&mono(filter)],
                )
            };
            Some(EndWords {
                title: fill(
                    &ntr(
                        "Auto-DJ keeps {} track ahead",
                        "Auto-DJ keeps {} tracks ahead",
                        ahead,
                    ),
                    &[&ahead.to_string()],
                ),
                detail: Some(detail),
                warning: false,
            })
        }
        _ => None,
    }
}

/// The text of the toast shown once when nothing more can be added while the
/// queue is not in view.
pub(crate) fn toast_text(repeat: RepeatMode, settings: &PlaybackSettings) -> String {
    match repeat {
        RepeatMode::AutoDj if !settings.auto_dj.filter.trim().is_empty() => {
            tr("Nothing more to add: no track matches the Auto-DJ filter")
        }
        RepeatMode::Endless if !settings.endless.filter.trim().is_empty() => {
            tr("Nothing more to add: no related track matches the filter")
        }
        RepeatMode::Endless => tr("Nothing more to add: no more related tracks"),
        _ => tr("Nothing more to add: no track of the library can be added"),
    }
}

/// The block as a row of the queue list, with `change` and `turn_off` behind
/// its buttons.
pub(crate) fn row(
    words: &EndWords,
    change: impl Fn() + 'static,
    turn_off: impl Fn() + 'static,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("section");
    row.add_css_class("end");
    row.set_activatable(false);
    row.set_focusable(false);
    row.set_widget_name("queue-end");
    let block = gtk::Box::new(gtk::Orientation::Vertical, 4);
    block.add_css_class("hk-queue-next");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    let icon = if words.warning {
        icon_or("helikodu-warning-symbolic", "dialog-warning-symbolic")
    } else {
        icon_or("helikodu-repeat-plus-symbolic", "helikodu-repeat-symbolic")
    };
    let image = gtk::Image::from_icon_name(&icon);
    image.add_css_class("hk-t2");
    image.set_valign(gtk::Align::Start);
    image.set_margin_top(1);
    line.append(&image);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&words.title));
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    title.add_css_class("hk-next-title");
    text.append(&title);
    let mut accessible = words.title.clone();
    if let Some(detail) = &words.detail {
        let under = gtk::Label::new(None);
        under.set_markup(detail);
        under.set_xalign(0.0);
        under.set_wrap(true);
        under.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        under.add_css_class("detail");
        accessible = format!("{accessible}. {}", under.text());
        text.append(&under);
    }
    line.append(&text);
    block.append(&line);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    buttons.set_halign(gtk::Align::End);
    let change_button = gtk::Button::with_label(&tr("Change…"));
    let off_button = gtk::Button::with_label(&tr("Turn off"));
    off_button.set_tooltip_text(Some(&tr("Sets Repeat to Off")));
    for button in [&change_button, &off_button] {
        button.add_css_class("hk-btn");
        button.add_css_class("text");
        button.add_css_class("hk-queue-next-button");
        buttons.append(button);
    }
    change_button.connect_clicked(move |_| change());
    off_button.connect_clicked(move |_| turn_off());
    block.append(&buttons);
    block.update_property(&[gtk::accessible::Property::Label(&accessible)]);
    row.set_child(Some(&block));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(key: u64, title: &str, artist: &str, album: &str) -> TrackRow {
        TrackRow {
            key: crate::backend::TrackKey(key),
            album: AlbumKey(key / 10),
            title: title.into(),
            artist: artist.into(),
            album_title: album.into(),
            entry: Some(key),
            ..TrackRow::default()
        }
    }

    fn queue() -> QueueSnapshot {
        QueueSnapshot {
            items: vec![
                track(11, "Glass Harbour", "Mari Kask", "Tidewater Rooms"),
                track(25, "Mirror Lake", "Aino Rebane", "Still Water"),
            ],
            current: Some(0),
            added: vec![25],
            ..QueueSnapshot::default()
        }
    }

    #[test]
    fn the_block_names_what_comes_next_as_drawn() {
        let settings = PlaybackSettings::default();
        let words = words(RepeatMode::Endless, &queue(), &settings).unwrap();
        assert_eq!(words.title, "Then: tracks related to “Mirror Lake”");
        assert_eq!(
            words.detail.as_deref(),
            Some("Aino Rebane · 10 at a time, by artist and genre")
        );
        let words = words_for(RepeatMode::AutoDj, &settings);
        assert_eq!(words.title, "Auto-DJ keeps 5 tracks ahead");
        assert_eq!(words.detail.as_deref(), Some("From the whole library"));
        let mut settings = settings;
        settings.auto_dj.filter = "genre:Jazz".into();
        let words = words_for(RepeatMode::AutoDj, &settings);
        assert_eq!(
            words.detail.as_deref(),
            Some("Only tracks that match <span font_family=\"monospace\">genre:Jazz</span>")
        );
        assert!(super::words(RepeatMode::All, &queue(), &settings).is_none());
    }

    fn words_for(repeat: RepeatMode, settings: &PlaybackSettings) -> EndWords {
        words(repeat, &queue(), settings).unwrap()
    }

    #[test]
    fn nothing_to_add_names_the_filter() {
        let mut settings = PlaybackSettings::default();
        settings.auto_dj.filter = "genre:Tango".into();
        let mut queue = queue();
        queue.nothing_to_add = true;
        let words = words(RepeatMode::AutoDj, &queue, &settings).unwrap();
        assert!(words.warning);
        assert_eq!(words.title, "Nothing more to add");
        assert_eq!(
            words.detail.as_deref(),
            Some(
                "No track matches the Auto-DJ filter, <span font_family=\"monospace\">genre:Tango</span>. The music stops after the last track."
            )
        );
        let related = super::words(RepeatMode::Endless, &queue, &settings).unwrap();
        assert_eq!(
            related.detail.as_deref(),
            Some("No more related tracks: every one was played or queued lately.")
        );
    }

    #[test]
    fn an_empty_queue_waits_for_play() {
        let words = words(
            RepeatMode::AutoDj,
            &QueueSnapshot::default(),
            &PlaybackSettings::default(),
        )
        .unwrap();
        assert_eq!(
            words.title,
            "Auto-DJ is on. Press Play and it fills the queue."
        );
        assert_eq!(words.detail, None);
    }

    #[test]
    fn a_radio_names_its_seed() {
        let mut queue = queue();
        queue.radio = Some(RadioSeed::Track(11));
        let words = words(RepeatMode::Endless, &queue, &PlaybackSettings::default()).unwrap();
        assert_eq!(words.title, "Then: tracks related to “Glass Harbour”");
    }
}
