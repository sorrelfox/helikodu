// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The keyboard shortcuts the user changed: the `[shortcuts]` table.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::SettingsError;

/// The most keys one action can have.
pub const MAX_KEYS_PER_ACTION: usize = 4;

/// The shortcuts changed from their defaults, and whether the shortcuts that
/// work in other applications were set up: the `[shortcuts]` table.
///
/// Actions are named as the front end names them (`app.next`, `win.search`);
/// keys are written as GTK accelerators (`<Control>Right`). An action that is
/// not listed has its default keys; an empty list removes them.
///
/// ```toml
/// [shortcuts]
/// global = false            # set up in the desktop (the GlobalShortcuts portal)
///
/// [shortcuts.keys]
/// "app.toggle-shuffle" = ["<Control>h"]
/// "app.stop" = []
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutSettings {
    /// Whether the shortcuts for other applications were handed to the
    /// desktop; they are bound again at start only then, so that no dialog
    /// appears before the user asks for one.
    pub global: bool,
    /// The keys of each changed action.
    pub keys: BTreeMap<String, Vec<String>>,
}

impl ShortcutSettings {
    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        for (action, keys) in &self.keys {
            let key = format!("shortcuts.keys.\"{action}\"");
            if action.is_empty() || action.len() > 64 {
                return Err(SettingsError {
                    key,
                    problem: "is not the name of an action".into(),
                });
            }
            if keys.len() > MAX_KEYS_PER_ACTION {
                return Err(SettingsError {
                    key,
                    problem: format!(
                        "has {} shortcuts, but may have at most {MAX_KEYS_PER_ACTION}",
                        keys.len()
                    ),
                });
            }
            if keys.iter().any(|k| k.trim().is_empty() || k.len() > 64) {
                return Err(SettingsError {
                    key,
                    problem: "has a shortcut that is empty or too long".into(),
                });
            }
        }
        Ok(())
    }

    /// The keys of `action`: the changed ones, or `default`.
    pub fn keys_of<'a>(&'a self, action: &str, default: &'a [&'a str]) -> Vec<&'a str> {
        match self.keys.get(action) {
            Some(keys) => keys.iter().map(String::as_str).collect(),
            None => default.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_keys_replace_the_defaults() {
        let mut shortcuts = ShortcutSettings::default();
        assert_eq!(
            shortcuts.keys_of("app.next", &["<Control>Right"]),
            ["<Control>Right"]
        );
        shortcuts.keys.insert("app.next".into(), vec![]);
        assert!(
            shortcuts
                .keys_of("app.next", &["<Control>Right"])
                .is_empty()
        );
        assert_eq!(shortcuts.validate(), Ok(()));
        shortcuts.keys.insert("app.stop".into(), vec![" ".into()]);
        assert!(shortcuts.validate().is_err());
        shortcuts
            .keys
            .insert("app.stop".into(), vec!["a".into(); MAX_KEYS_PER_ACTION + 1]);
        assert!(shortcuts.validate().is_err());
    }

    #[test]
    fn the_table_round_trips() {
        let text = "global = true\n\n[keys]\n\"app.stop\" = []\n\"app.toggle-shuffle\" = [\"<Control>h\"]\n";
        let shortcuts: ShortcutSettings = toml::from_str(text).unwrap();
        assert!(shortcuts.global);
        assert_eq!(shortcuts.keys["app.toggle-shuffle"], ["<Control>h"]);
        assert_eq!(toml::to_string(&shortcuts).unwrap(), text);
    }
}
