// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Listens: how much of a track was actually played, and recording them in the
//! library on a thread of their own so that the player never waits for the disk.

use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use helikodu_library::{Listen, ListenEnd, ListenOutcome, PlayRules};
use helikodu_model::audio::MediaTime;
use helikodu_model::library::TrackId;
use helikodu_model::player::ItemId;

use crate::library::MusicLibrary;

/// The longest step between two positions that counts as played time. Positions
/// come ten times a second; a larger step is a seek, which does not count.
const MAX_STEP: Duration = Duration::from_secs(2);

/// One listen in progress.
#[derive(Debug, Clone)]
pub(crate) struct ListenTracker {
    pub(crate) item: ItemId,
    track: TrackId,
    started_at: SystemTime,
    played: Duration,
    last: Option<Duration>,
    /// Whether a seek was asked for during the listen.
    seeked: bool,
    length: Option<Duration>,
    context: Option<String>,
}

impl ListenTracker {
    pub(crate) fn new(
        item: ItemId,
        track: TrackId,
        started_at: SystemTime,
        length: Option<MediaTime>,
        context: Option<String>,
    ) -> Self {
        Self {
            item,
            track,
            started_at,
            played: Duration::ZERO,
            last: None,
            seeked: false,
            length: length.map(MediaTime::as_duration),
            context,
        }
    }

    /// The engine reports `position`.
    pub(crate) fn position(&mut self, position: MediaTime) {
        let position = position.as_duration();
        if let Some(last) = self.last
            && position > last
            && position - last <= MAX_STEP
        {
            self.played += position - last;
        }
        self.last = Some(position);
    }

    /// A seek was asked for: the next position starts afresh.
    pub(crate) fn seeked(&mut self) {
        self.last = None;
        self.seeked = true;
    }

    /// The time played so far.
    #[cfg(test)]
    pub(crate) fn played(&self) -> Duration {
        self.played
    }

    /// The listen, ended by `end`.
    pub(crate) fn finish(mut self, end: ListenEnd) -> (TrackId, Listen) {
        if end == ListenEnd::Finished
            && let Some(length) = self.length
        {
            match self.last {
                // The last position comes a little before the end.
                Some(last) if length > last && length - last <= MAX_STEP => {
                    self.played += length - last;
                }
                // A short track can end before the engine reports a position (when
                // rendering faster than real time): it played from start to end.
                None if !self.seeked => self.played = length,
                _ => {}
            }
        }
        let listen = Listen {
            started_at: self.started_at,
            played: self.played,
            end,
            position: self.last,
            context: self.context,
        };
        (self.track, listen)
    }
}

/// The result of recording a listen.
pub(crate) type Recorded = (TrackId, Result<ListenOutcome, String>);

/// Records listens on its own thread and hands each result to `done`.
#[derive(Debug)]
pub(crate) struct Recorder {
    jobs: Option<Sender<(TrackId, Listen, PlayRules)>>,
    thread: Option<JoinHandle<()>>,
}

impl Recorder {
    pub(crate) fn start(
        library: MusicLibrary,
        done: impl Fn(Recorded) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (jobs, receiver) = mpsc::channel::<(TrackId, Listen, PlayRules)>();
        let thread = thread::Builder::new()
            .name("helikodu-stats".into())
            .spawn(move || {
                for (track, listen, rules) in receiver {
                    let result = library
                        .record_listen(track, listen, rules)
                        .map_err(|error| error.to_string());
                    done((track, result));
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    pub(crate) fn record(&self, track: TrackId, listen: Listen, rules: PlayRules) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send((track, listen, rules));
        }
    }

    /// Records what is queued and stops.
    pub(crate) fn finish(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> MediaTime {
        MediaTime::from_millis(millis)
    }

    #[test]
    fn seeks_do_not_count() {
        let mut listen = ListenTracker::new(
            ItemId::new(1),
            TrackId::new(1),
            SystemTime::UNIX_EPOCH,
            Some(ms(60_000)),
            None,
        );
        for t in (0..=10_000).step_by(100) {
            listen.position(ms(t));
        }
        assert_eq!(listen.played(), Duration::from_secs(10));
        // A jump forward without notice, then a seek back.
        listen.position(ms(40_000));
        listen.seeked();
        listen.position(ms(5_000));
        listen.position(ms(6_000));
        assert_eq!(listen.played(), Duration::from_secs(11));
        listen.position(ms(59_000));
        let (_, finished) = listen.finish(ListenEnd::Finished);
        assert_eq!(finished.played, Duration::from_secs(12));
        assert_eq!(finished.end, ListenEnd::Finished);
    }
}
