// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The state of the window that is remembered between runs: `ui-state.toml`.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

/// What the window remembers: its size, the widths and visibility of its panes and
/// the last view. Stored in `$XDG_STATE_HOME/helikodu/ui-state.toml`, not with the
/// settings: it changes as the window is used.
///
/// Values out of range are moved into range when read ([`UiState::clamped`]) rather
/// than refused, so a hand-edited file never keeps the window from opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// The version of the file's layout.
    pub version: u32,
    /// The main window.
    pub window: WindowState,
    /// The side panes.
    pub panes: PaneState,
    /// The view shown last, by the name the interface gives it (`albums`); empty for
    /// the default view.
    pub last_view: String,
}

impl UiState {
    /// The version of the layout this build writes.
    pub const VERSION: u32 = 1;

    /// The same state with every value moved into its range.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            window: WindowState {
                width: clamp(self.window.width, WindowState::WIDTH),
                height: clamp(self.window.height, WindowState::HEIGHT),
                ..self.window
            },
            panes: PaneState {
                sidebar_width: clamp(self.panes.sidebar_width, PaneState::SIDEBAR_WIDTH),
                queue_width: clamp(self.panes.queue_width, PaneState::QUEUE_WIDTH),
                details_width: clamp(self.panes.details_width, PaneState::DETAILS_WIDTH),
                ..self.panes
            },
            ..self
        }
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            window: WindowState::default(),
            panes: PaneState::default(),
            last_view: String::new(),
        }
    }
}

/// The main window's size, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    /// The width when not maximized.
    pub width: u32,
    /// The height when not maximized.
    pub height: u32,
    /// Whether it is maximized.
    pub maximized: bool,
}

impl WindowState {
    /// The allowed widths.
    pub const WIDTH: RangeInclusive<u32> = 360..=16384;
    /// The allowed heights.
    pub const HEIGHT: RangeInclusive<u32> = 300..=16384;
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            maximized: false,
        }
    }
}

/// The resizable side panes: the sidebar, the queue panel and the album page's
/// details column. The defaults are the
/// widths of the design; each has a minimum and a maximum so that the layout never
/// breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneState {
    /// The sidebar's width.
    pub sidebar_width: u32,
    /// Whether the sidebar is shown.
    pub sidebar_visible: bool,
    /// The queue panel's width.
    pub queue_width: u32,
    /// Whether the queue panel is shown.
    pub queue_visible: bool,
    /// The width of the details column on album and audiobook pages.
    pub details_width: u32,
}

impl PaneState {
    /// The allowed sidebar widths.
    pub const SIDEBAR_WIDTH: RangeInclusive<u32> = 180..=360;
    /// The allowed queue panel widths.
    pub const QUEUE_WIDTH: RangeInclusive<u32> = 260..=520;
    /// The allowed details column widths.
    pub const DETAILS_WIDTH: RangeInclusive<u32> = 220..=520;
}

impl Default for PaneState {
    fn default() -> Self {
        Self {
            sidebar_width: 228,
            sidebar_visible: true,
            queue_width: 320,
            queue_visible: true,
            details_width: 260,
        }
    }
}

fn clamp(value: u32, range: RangeInclusive<u32>) -> u32 {
    value.clamp(*range.start(), *range.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_design_widths() {
        let state = UiState::default();
        assert_eq!(state.panes.sidebar_width, 228);
        assert_eq!(state.panes.queue_width, 320);
        assert_eq!(state.panes.details_width, 260);
        assert_eq!(state.clone().clamped(), state);
        let text = toml::to_string(&state).unwrap();
        assert_eq!(toml::from_str::<UiState>(&text).unwrap(), state);
    }

    #[test]
    fn widths_are_clamped() {
        let state: UiState =
            toml::from_str(
            "[panes]\nsidebar_width = 5\nqueue_width = 9000\ndetails_width = 1\n[window]\nwidth = 1\n",
        )
                .unwrap();
        let state = state.clamped();
        assert_eq!(state.panes.sidebar_width, 180);
        assert_eq!(state.panes.queue_width, 520);
        assert_eq!(state.panes.details_width, 220);
        assert_eq!(state.window.width, 360);
        assert!(state.panes.queue_visible);
    }
}
