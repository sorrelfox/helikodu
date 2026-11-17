// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The keys that control playback while `play` runs in a terminal.

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use helikodu_service::player::PlayerCommand;

/// How far the arrow keys seek.
const SEEK_STEP: Duration = Duration::from_secs(5);
/// How much `+` and `-` change the volume, as a share of the whole range.
const VOLUME_STEP: f32 = 0.05;

/// The keys, as shown when playback starts.
pub(super) const HELP: &str = "keys: space pause, n/p next/previous, \u{2190}/\u{2192} seek 5 s, +/- volume, o output, q quit";

/// What a key asks for.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Action {
    Command(PlayerCommand),
    Quit,
}

/// The terminal in raw mode, so that keys arrive one by one without echo. Dropping
/// it restores the terminal.
#[derive(Debug)]
pub(super) struct RawMode(());

impl RawMode {
    pub(super) fn enable() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self(()))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// The actions of the keys pressed since the last call, waiting at most `wait` for
/// the first one.
pub(super) fn pressed(wait: Duration) -> io::Result<Vec<Action>> {
    let mut actions = Vec::new();
    let mut wait = wait;
    while event::poll(wait)? {
        wait = Duration::ZERO;
        if let Event::Key(key) = event::read()?
            && let Some(action) = action(key)
        {
            actions.push(action);
        }
    }
    Ok(actions)
}

/// The action of one key event.
fn action(key: KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let command = match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Some(Action::Quit);
        }
        KeyCode::Char('q' | 'Q') | KeyCode::Esc => return Some(Action::Quit),
        KeyCode::Char(' ') => PlayerCommand::PlayPause,
        KeyCode::Char('n') => PlayerCommand::Next,
        KeyCode::Char('p') => PlayerCommand::Previous,
        KeyCode::Right => PlayerCommand::SeekForward(SEEK_STEP),
        KeyCode::Left => PlayerCommand::SeekBackward(SEEK_STEP),
        KeyCode::Char('o') => PlayerCommand::NextOutput,
        KeyCode::Char('+' | '=') => PlayerCommand::AdjustVolume(VOLUME_STEP),
        KeyCode::Char('-') => PlayerCommand::AdjustVolume(-VOLUME_STEP),
        _ => return None,
    };
    Some(Action::Command(command))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Option<Action> {
        action(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn keys_map_to_commands() {
        assert_eq!(
            press(KeyCode::Char(' ')),
            Some(Action::Command(PlayerCommand::PlayPause))
        );
        assert_eq!(
            press(KeyCode::Left),
            Some(Action::Command(PlayerCommand::SeekBackward(SEEK_STEP)))
        );
        assert_eq!(
            press(KeyCode::Char('o')),
            Some(Action::Command(PlayerCommand::NextOutput))
        );
        assert_eq!(press(KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(
            action(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(press(KeyCode::Char('x')), None);
    }

    #[test]
    fn releases_are_ignored() {
        let mut key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(action(key), None);
    }
}
