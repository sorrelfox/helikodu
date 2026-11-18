// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The appearance settings: the `[appearance]` table.

use serde::{Deserialize, Serialize};

/// How the window looks: the `[appearance]` table.
///
/// ```toml
/// [appearance]
/// color_scheme = "system"   # system | light | dark
/// accent = "desktop"        # desktop | amber | blue | green | violet
/// window_style = "auto"     # auto | integrated | classic
/// menu_bar = true           # Classic style only
/// stripe_rows = true
/// language = ""             # empty: the desktop's language
/// notifications = true      # when the track changes
/// tray_icon = true          # where the desktop has a tray
/// close_to_tray = true      # closing the window keeps playing in the tray
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// Light, dark, or as the desktop says.
    pub color_scheme: ColorScheme,
    /// The accent colour.
    pub accent: Accent,
    /// Client-side decorations or the system title bar.
    pub window_style: WindowStyle,
    /// Whether the Classic window style shows the menu bar.
    pub menu_bar: bool,
    /// Whether every second row of a table is shaded.
    pub stripe_rows: bool,
    /// The interface language as a locale name (`sv_SE`), or empty for the
    /// desktop's. Changing it sorts the library again in the background.
    pub language: String,
    /// Whether the desktop shows a notification when the track changes.
    pub notifications: bool,
    /// Whether an icon is shown in the system tray, where the desktop has one.
    pub tray_icon: bool,
    /// Whether closing the window keeps playing in the tray instead of quitting
    /// (only with the tray icon; Ctrl+Q always quits).
    pub close_to_tray: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::System,
            accent: Accent::Desktop,
            window_style: WindowStyle::Auto,
            menu_bar: true,
            stripe_rows: true,
            language: String::new(),
            notifications: true,
            tray_icon: true,
            close_to_tray: true,
        }
    }
}

/// The colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorScheme {
    /// As the desktop says.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// The accent colour: the desktop's, or one of the presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Accent {
    /// The desktop's accent colour, or amber when it has none.
    #[default]
    Desktop,
    /// Amber.
    Amber,
    /// Blue.
    Blue,
    /// Green.
    Green,
    /// Violet.
    Violet,
}

impl Accent {
    /// The presets, in the order the settings page shows them.
    pub const PRESETS: [Self; 4] = [Self::Amber, Self::Blue, Self::Green, Self::Violet];
}

/// The window style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowStyle {
    /// Integrated on GNOME, Classic elsewhere.
    #[default]
    Auto,
    /// Client-side decorations; the window buttons sit in the right-most strip.
    Integrated,
    /// The system title bar, with an optional menu bar.
    Classic,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn values_round_trip() {
        let settings = Settings {
            appearance: AppearanceSettings {
                color_scheme: ColorScheme::Dark,
                accent: Accent::Violet,
                window_style: WindowStyle::Classic,
                menu_bar: false,
                stripe_rows: false,
                language: "sv_SE".into(),
                notifications: false,
                tray_icon: false,
                close_to_tray: false,
            },
            ..Settings::default()
        };
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("accent = \"violet\""), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
        assert!(toml::from_str::<Settings>("[appearance]\naccent = \"pink\"\n").is_err());
    }
}
