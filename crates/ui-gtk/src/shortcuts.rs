// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The keyboard shortcuts: every action that can have one, its default keys,
//! and the keys the user chose instead (`[shortcuts]` in the settings).
//! [`apply`] gives the application the keys in effect; Settings › Shortcuts
//! changes them, and the Keyboard shortcuts window (Ctrl+?) lists them.
//!
//! Space plays and pauses outside text fields whatever else is set: it is a
//! shortcut of the main window, not an accelerator, so it is shown as the
//! default of "Play or pause" but never given to the application.

use gtk::gio;
use gtk::prelude::*;
use helikodu_model::settings::ShortcutSettings;

use crate::backend::{Backend, PlayerRequest, Repeat};
use crate::i18n::tr;

/// A group of shortcuts, as the settings page and the Keyboard shortcuts
/// window show them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    /// Playback, the left column.
    Playback,
    /// Library and lists.
    Library,
    /// The window.
    Window,
}

impl Group {
    /// The groups in order, with their titles.
    pub(crate) fn all() -> [(Self, String); 3] {
        [
            (Self::Playback, tr("Playback")),
            (Self::Library, tr("Library and lists")),
            (Self::Window, tr("Window")),
        ]
    }
}

/// The key that plays and pauses outside text fields.
pub(crate) const SPACE: &str = "space";

/// One action that can have a shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    /// Its group.
    pub(crate) group: Group,
    /// The action, such as `app.next`.
    pub(crate) action: &'static str,
    /// What it does.
    pub(crate) name: String,
    /// Its keys until the user changes them.
    pub(crate) defaults: &'static [&'static str],
}

/// Every action that can have a shortcut, group by group.
pub(crate) fn bindings() -> Vec<Binding> {
    use Group::{Library, Playback, Window};
    let b = |group, action, name: String, defaults| Binding {
        group,
        action,
        name,
        defaults,
    };
    vec![
        b(
            Playback,
            "app.play-pause",
            tr("Play or pause"),
            &[SPACE][..],
        ),
        b(Playback, "app.stop", tr("Stop"), &[]),
        b(Playback, "app.next", tr("Next track"), &["<Control>Right"]),
        b(
            Playback,
            "app.previous",
            tr("Previous track"),
            &["<Control>Left"],
        ),
        b(
            Playback,
            "app.seek-forward",
            tr("Seek forward"),
            &["<Control><Shift>Right"],
        ),
        b(
            Playback,
            "app.seek-back",
            tr("Seek back"),
            &["<Control><Shift>Left"],
        ),
        b(Playback, "win.volume-up", tr("Volume up"), &["<Control>Up"]),
        b(
            Playback,
            "win.volume-down",
            tr("Volume down"),
            &["<Control>Down"],
        ),
        b(Playback, "app.toggle-shuffle", tr("Shuffle"), &[]),
        b(Playback, "app.toggle-repeat", tr("Repeat"), &[]),
        b(Playback, "app.stop-after", tr("Stop after this track"), &[]),
        b(
            Playback,
            "app.loop-start",
            tr("Set the start of a loop"),
            &["<Control>bracketleft"],
        ),
        b(
            Playback,
            "app.loop-end",
            tr("Set the end of a loop"),
            &["<Control>bracketright"],
        ),
        b(
            Playback,
            "app.loop-clear",
            tr("Clear the loop"),
            &["<Control>backslash"],
        ),
        b(Playback, "app.previous-album", tr("Previous album"), &[]),
        b(Playback, "app.next-album", tr("Next album"), &[]),
        b(Playback, "app.random-album", tr("Random album"), &[]),
        b(Playback, "win.sleep-timer", tr("Sleep timer"), &[]),
        b(Library, "win.search", tr("Search"), &["<Control>f"]),
        b(
            Library,
            "win.jump-to-playing",
            tr("Jump to the playing track"),
            &["<Control>j"],
        ),
        b(
            Library,
            "win.track-properties",
            tr("Track properties"),
            &["<Alt>Return"],
        ),
        b(
            Library,
            "win.track-copies",
            tr("Copies of the track's file"),
            &["<Alt>c"],
        ),
        b(Library, "win.select-all", tr("Select all"), &["<Control>a"]),
        b(
            Library,
            "app.new-playlist",
            tr("New playlist"),
            &["<Control>n"],
        ),
        b(Library, "win.undo", tr("Undo"), &["<Control>z"]),
        b(
            Library,
            "win.redo",
            tr("Redo"),
            &["<Control><Shift>z", "<Control>y"],
        ),
        b(Window, "win.toggle-sidebar", tr("Sidebar"), &["F9"]),
        b(Window, "win.show-menubar", tr("Menu bar"), &["<Control>m"]),
        b(Window, "win.now-playing", tr("Now Playing"), &["F11"]),
        b(
            Window,
            "app.mini-player",
            tr("Mini player"),
            &["<Control><Shift>m"],
        ),
        b(Window, "win.cycle-pane", tr("Next pane"), &["F6"]),
        b(Window, "win.back", tr("Back"), &["<Alt>Left"]),
        b(Window, "win.forward", tr("Forward"), &["<Alt>Right"]),
        b(
            Window,
            "app.preferences",
            tr("Settings"),
            &["<Control>comma"],
        ),
        b(
            Window,
            "app.shortcuts",
            tr("Keyboard shortcuts"),
            &["<Control>question"],
        ),
        b(Window, "app.quit", tr("Quit"), &["<Control>q"]),
    ]
}

/// The keys of `binding` in effect under `settings`.
pub(crate) fn keys(binding: &Binding, settings: &ShortcutSettings) -> Vec<String> {
    settings
        .keys_of(binding.action, binding.defaults)
        .into_iter()
        .map(normalize)
        .collect()
}

/// Whether `binding` has other keys than its defaults.
pub(crate) fn changed(binding: &Binding, settings: &ShortcutSettings) -> bool {
    let defaults: Vec<String> = binding.defaults.iter().copied().map(normalize).collect();
    keys(binding, settings) != defaults
}

/// An accelerator in GTK's own spelling, so that equal keys compare equal
/// (`<Ctrl>J` and `<Control>j`).
pub(crate) fn normalize(accelerator: &str) -> String {
    match gtk::accelerator_parse(accelerator) {
        Some((key, mods)) => gtk::accelerator_name(key.to_lower(), mods).to_string(),
        None => accelerator.to_owned(),
    }
}

/// The accelerator for a key press, or `None` for a modifier on its own.
pub(crate) fn from_press(key: gtk::gdk::Key, state: gtk::gdk::ModifierType) -> Option<String> {
    use gtk::gdk::ModifierType;
    let mods = state
        & (ModifierType::CONTROL_MASK
            | ModifierType::ALT_MASK
            | ModifierType::SHIFT_MASK
            | ModifierType::SUPER_MASK);
    let key = key.to_lower();
    let name = key.name()?;
    if name.starts_with("Shift_")
        || name.starts_with("Control_")
        || name.starts_with("Alt_")
        || name.starts_with("Super_")
        || name.starts_with("Meta_")
        || name.starts_with("ISO_")
        || name == "Caps_Lock"
    {
        return None;
    }
    if !gtk::accelerator_valid(key, mods) && mods.is_empty() && name != "space" {
        return None;
    }
    Some(gtk::accelerator_name(key, mods).to_string())
}

/// The binding that has `accelerator` under `settings`, other than `except`.
pub(crate) fn owner(
    accelerator: &str,
    settings: &ShortcutSettings,
    except: Option<&str>,
) -> Option<Binding> {
    let accelerator = normalize(accelerator);
    bindings()
        .into_iter()
        .find(|b| Some(b.action) != except && keys(b, settings).contains(&accelerator))
}

/// Gives `application` the keys in effect under `settings`.
pub(crate) fn apply(application: &gtk::Application, settings: &ShortcutSettings) {
    for binding in bindings() {
        apply_one(application, &binding, settings);
    }
}

/// The keys a text field keeps for itself while it has the focus: select
/// all, undo and redo.
const TEXT_KEYS: [&str; 4] = [
    "<Control>a",
    "<Control>z",
    "<Control><Shift>z",
    "<Control>y",
];

/// Lifts the keys of [`TEXT_KEYS`] from the actions that have them while a
/// text field has the focus (`typing`), and gives them back after.
pub(crate) fn guard_text_keys(
    application: &gtk::Application,
    settings: &ShortcutSettings,
    typing: bool,
) {
    let text: Vec<String> = TEXT_KEYS.into_iter().map(normalize).collect();
    for binding in bindings() {
        let keys = keys(&binding, settings);
        if !keys.iter().any(|k| text.contains(k)) {
            continue;
        }
        if typing {
            let rest: Vec<&str> = keys
                .iter()
                .map(String::as_str)
                .filter(|k| *k != SPACE && !text.iter().any(|t| t == k))
                .collect();
            application.set_accels_for_action(binding.action, &rest);
        } else {
            apply_one(application, &binding, settings);
        }
    }
}

fn apply_one(application: &gtk::Application, binding: &Binding, settings: &ShortcutSettings) {
    let keys: Vec<String> = keys(binding, settings)
        .into_iter()
        .filter(|k| k != SPACE)
        .collect();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    application.set_accels_for_action(binding.action, &keys);
}

/// Adds the actions that exist only to be given shortcuts: shuffle and repeat
/// on and off.
pub(crate) fn install_actions(application: &gtk::Application, backend: &Backend) {
    let state_is_off = |application: &gtk::Application, name: &str| {
        application
            .lookup_action(name)
            .and_then(|a| a.state())
            .and_then(|s| s.get::<String>())
            .is_none_or(|s| s == "off")
    };
    let shuffle = gio::SimpleAction::new("toggle-shuffle", None);
    let b = backend.clone();
    let app = application.downgrade();
    shuffle.connect_activate(move |_, _| {
        if let Some(application) = app.upgrade() {
            b.send(PlayerRequest::SetShuffle(state_is_off(
                &application,
                "shuffle",
            )));
        }
    });
    application.add_action(&shuffle);
    let repeat = gio::SimpleAction::new("toggle-repeat", None);
    let b = backend.clone();
    let app = application.downgrade();
    repeat.connect_activate(move |_, _| {
        if let Some(application) = app.upgrade() {
            let off = state_is_off(&application, "repeat");
            b.send(PlayerRequest::SetRepeat(if off {
                Repeat::All
            } else {
                Repeat::Off
            }));
        }
    });
    application.add_action(&repeat);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_listed_once() {
        let all = bindings();
        for (i, binding) in all.iter().enumerate() {
            assert!(
                all.iter()
                    .skip(i + 1)
                    .all(|other| other.action != binding.action),
                "{} twice",
                binding.action
            );
        }
    }
}
