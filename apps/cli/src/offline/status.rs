// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What `play` prints: a line per file and problem on standard error, and in a
//! terminal a status line that is redrawn in place.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;

use crossterm::terminal;
use helikodu_model::audio::{BackendSelection, MediaTime, OutputSelection};
use helikodu_model::player::{TransportState, Volume};
use helikodu_service::player::{PlayerEvent, resolve_backend};

/// The state of playback as far as the printing goes.
#[derive(Debug)]
pub(super) struct Status {
    /// The terminal is in raw mode: lines end with a carriage return too.
    raw: bool,
    /// Standard error is a terminal: draw the status line.
    live: bool,
    state: TransportState,
    track: Option<(usize, usize, PathBuf)>,
    position: MediaTime,
    length: Option<MediaTime>,
    volume: Volume,
    output: OutputSelection,
}

impl Status {
    pub(super) fn new(raw: bool, live: bool, output: OutputSelection, volume: Volume) -> Self {
        Self {
            raw,
            live,
            state: TransportState::Stopped,
            track: None,
            position: MediaTime::ZERO,
            length: None,
            volume,
            output,
        }
    }

    /// Prints a line of its own, above the status line.
    pub(super) fn message(&self, text: &str) {
        let end = if self.raw { "\r\n" } else { "\n" };
        let clear = if self.live { "\r\x1b[K" } else { "" };
        let mut err = io::stderr().lock();
        let _ = write!(err, "{clear}{text}{end}");
        let _ = err.flush();
        drop(err);
        self.redraw();
    }

    /// Follows `event` and prints what it says.
    pub(super) fn show(&mut self, event: &PlayerEvent) {
        match event {
            PlayerEvent::State(state) => {
                self.state = *state;
                self.redraw();
            }
            PlayerEvent::TrackChanged {
                index,
                count,
                path,
                spec,
                length,
            } => {
                self.track = Some((*index, *count, path.clone()));
                self.position = MediaTime::ZERO;
                self.length = *length;
                self.message(&format!(
                    "Playing {}/{count}: {} ({spec})",
                    index + 1,
                    path.display()
                ));
            }
            PlayerEvent::Position {
                position, length, ..
            } => {
                self.position = *position;
                self.length = *length;
                self.redraw();
            }
            PlayerEvent::VolumeChanged(volume) => {
                self.volume = *volume;
                self.redraw();
            }
            PlayerEvent::OutputChanged(output) => {
                self.output = output.clone();
                self.message(&format!("Output: {}", describe_output(output)));
            }
            PlayerEvent::Problem { file, error } => match file {
                Some((_, path)) => self.message(&format!("{}: {error}", path.display())),
                None => self.message(&format!("error: {error}")),
            },
            PlayerEvent::Ended { played, failed, .. } => {
                self.clear();
                if *failed > 0 {
                    let text = format!(
                        "{played} of {} files played; {failed} could not be played",
                        played + failed
                    );
                    let end = if self.raw { "\r\n" } else { "\n" };
                    eprint!("{text}{end}");
                }
            }
        }
    }

    /// Removes the status line.
    pub(super) fn clear(&self) {
        if self.live {
            let mut err = io::stderr().lock();
            let _ = write!(err, "\r\x1b[K");
            let _ = err.flush();
        }
    }

    /// Draws the status line again, once a file has started.
    fn redraw(&self) {
        let Some((index, count, path)) = self.track.as_ref().filter(|_| self.live) else {
            return;
        };
        let mut line = format!(
            "[{}/{count}] {:<7} {}",
            index + 1,
            self.state.as_str(),
            clock(self.position)
        );
        if let Some(length) = self.length {
            let _ = write!(line, " / {}", clock(length));
        }
        let percent = (self.volume.position() * 100.0).round();
        let _ = write!(
            line,
            "  vol {percent}%  {}  {}",
            describe_output(&self.output),
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        // A terminal that does not know its size says it has no columns.
        let width = match terminal::size() {
            Ok((columns, _)) if columns > 0 => usize::from(columns),
            _ => 80,
        };
        let line: String = line.chars().take(width.saturating_sub(1)).collect();
        let mut err = io::stderr().lock();
        let _ = write!(err, "\r\x1b[K{line}");
        let _ = err.flush();
    }
}

/// `backend device`, with the backend the automatic choice stands for.
pub(super) fn describe_output(output: &OutputSelection) -> String {
    let backend = match output.backend {
        BackendSelection::Automatic => {
            format!("auto ({})", resolve_backend(BackendSelection::Automatic))
        }
        BackendSelection::Backend(kind) => kind.to_string(),
    };
    format!("{backend} {} ({})", output.device, output.mode)
}

/// `m:ss`, or `h:mm:ss` from one hour on.
fn clock(time: MediaTime) -> String {
    let seconds = time.as_duration().as_secs();
    let (minutes, seconds) = (seconds / 60, seconds % 60);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_format() {
        assert_eq!(clock(MediaTime::from_millis(0)), "0:00");
        assert_eq!(clock(MediaTime::from_millis(83_999)), "1:23");
        assert_eq!(clock(MediaTime::from_millis(3_723_000)), "1:02:03");
    }
}
