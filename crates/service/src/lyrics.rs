// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The lyrics of the playing track, and the line sung now.
//!
//! [`LyricsService::start`] runs a thread that follows the player's events:
//! when an entry starts it reads that track's lyrics (the `.lrc` file beside it
//! or its tags, through a [`LyricsProvider`]) and announces them with
//! [`LyricsEvent::Changed`]; while it plays it announces the line sung with
//! [`LyricsEvent::Line`], only when that line changes. Between the player's
//! positions the thread waits exactly until the next line is due, so it does
//! not wake while paused, stopped or between widely spaced lines.
//!
//! The offset of each track (positive shows the lines sooner, as LRC's
//! `[offset:]`) is kept in a small file of the state folder.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helikodu_model::player::{EntryId, PlaybackState};
use serde::{Deserialize, Serialize};

use crate::Event;
use crate::bus::{EventBus, Subscription};
use crate::clock::SharedClock;
use crate::player::{PlayerEvent, PlayerStatus};

pub use helikodu_library::lyrics::{
    EmbeddedKind, LocalLyrics, LrcMetadata, LyricLine, Lyrics, LyricsProvider, LyricsQuery,
    LyricsSource, SidecarKind, WordTiming,
};

/// The file in the state folder that keeps the offset of each track.
pub const OFFSETS_FILE: &str = "lyrics-offsets.json";
/// How many tracks keep an offset; the oldest are forgotten.
const MAX_OFFSETS: usize = 2_000;
/// The largest offset, either way, in milliseconds.
pub const MAX_OFFSET_MS: i64 = 60_000;
/// The longest wait while playing: positions keep the clock honest.
const LONGEST_WAIT: Duration = Duration::from_secs(1);
/// A line change this close is taken as due.
const EARLY: Duration = Duration::from_millis(5);

/// What the lyrics service announces.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum LyricsEvent {
    /// The lyrics of the current entry were read again: another entry, a
    /// reload or a new offset. `lyrics` is `None` when the track has none,
    /// `entry` when nothing plays.
    Changed {
        /// The entry.
        entry: Option<EntryId>,
        /// The track's file.
        path: Option<PathBuf>,
        /// The lyrics.
        lyrics: Option<Arc<Lyrics>>,
        /// The offset of this track in milliseconds; positive shows the lines
        /// sooner.
        offset_ms: i64,
    },
    /// The line sung now changed; `None` before the first line.
    Line {
        /// The entry.
        entry: EntryId,
        /// The index into the lines.
        index: Option<usize>,
    },
}

/// The lyrics service's view of the current entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LyricsState {
    /// The entry.
    pub entry: Option<EntryId>,
    /// Its file.
    pub path: Option<PathBuf>,
    /// Its lyrics.
    pub lyrics: Option<Arc<Lyrics>>,
    /// Its offset in milliseconds.
    pub offset_ms: i64,
    /// The line sung now.
    pub line: Option<usize>,
}

enum Input {
    Player(PlayerEvent),
    SetOffset(i64),
    AdjustOffset(i64),
    Reload,
    Shutdown,
}

/// The running lyrics service. Dropping it ends its thread.
#[derive(Debug)]
pub struct LyricsService {
    inputs: Sender<Input>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Mutex<LyricsState>>,
    // Kept for as long as the service lives; the bus forgets it after. In a
    // mutex only so the service can be shared between threads.
    _subscription: Mutex<Subscription<()>>,
}

impl LyricsService {
    /// Starts following the player's events on `bus`. `status` is the player's
    /// state now, so a restored entry has its lyrics at once; `state_dir` keeps
    /// the offsets (none are kept without it).
    pub fn start(
        bus: &EventBus<Event>,
        clock: SharedClock,
        provider: Arc<dyn LyricsProvider>,
        status: &PlayerStatus,
        state_dir: Option<&Path>,
    ) -> std::io::Result<Self> {
        let (inputs, mailbox) = mpsc::channel();
        let forward = inputs.clone();
        // The filter hands the player's events to the thread; once the thread
        // is gone, it lets the bus send to the dropped subscription, which makes
        // the bus forget it.
        let subscription = bus.subscribe_with(
            move |event: &Event| match event {
                Event::Player(
                    e @ (PlayerEvent::TrackChanged { .. }
                    | PlayerEvent::Position { .. }
                    | PlayerEvent::State { .. }
                    | PlayerEvent::Ended { .. }),
                ) => forward.send(Input::Player(e.clone())).err().map(|_| ()),
                _ => None,
            },
            None,
        );
        let shared = Arc::new(Mutex::new(LyricsState::default()));
        let mut follower = Follower {
            bus: bus.clone(),
            clock,
            provider,
            offsets: Offsets::load(state_dir.map(|d| d.join(OFFSETS_FILE))),
            shared: Arc::clone(&shared),
            state: LyricsState::default(),
            playing: false,
            anchor: (Duration::ZERO, Duration::ZERO),
        };
        if let Some(now) = &status.now_playing {
            follower.playing = status.playback == PlaybackState::Playing;
            follower.anchor = (status.position.as_duration(), follower.clock.elapsed());
            follower.load(Some(now.entry), Some(now.path.clone()));
        }
        let thread = thread::Builder::new()
            .name("helikodu-lyrics".into())
            .spawn(move || follower.run(&mailbox))?;
        Ok(Self {
            inputs,
            thread: Some(thread),
            shared,
            _subscription: Mutex::new(subscription),
        })
    }

    /// The current entry's lyrics and line, as last announced.
    pub fn current(&self) -> LyricsState {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Sets the offset of the current track, in milliseconds (positive shows
    /// the lines sooner); kept for the track.
    pub fn set_offset(&self, offset_ms: i64) {
        let _ = self.inputs.send(Input::SetOffset(offset_ms));
    }

    /// Moves the offset of the current track by `delta_ms`.
    pub fn adjust_offset(&self, delta_ms: i64) {
        let _ = self.inputs.send(Input::AdjustOffset(delta_ms));
    }

    /// Reads the current track's lyrics again, after its lyrics file changed.
    pub fn reload(&self) {
        let _ = self.inputs.send(Input::Reload);
    }
}

impl Drop for LyricsService {
    fn drop(&mut self) {
        let _ = self.inputs.send(Input::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The thread's state.
struct Follower {
    bus: EventBus<Event>,
    clock: SharedClock,
    provider: Arc<dyn LyricsProvider>,
    offsets: Offsets,
    shared: Arc<Mutex<LyricsState>>,
    state: LyricsState,
    playing: bool,
    /// The last known position and the clock's time then.
    anchor: (Duration, Duration),
}

impl Follower {
    fn run(mut self, mailbox: &Receiver<Input>) {
        loop {
            let input = match self.wait() {
                Some(wait) => match mailbox.recv_timeout(wait) {
                    Ok(input) => Some(input),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match mailbox.recv() {
                    Ok(input) => Some(input),
                    Err(_) => return,
                },
            };
            match input {
                Some(Input::Shutdown) => return,
                Some(input) => self.handle(input),
                None => {}
            }
            self.follow_line();
        }
    }

    fn handle(&mut self, input: Input) {
        match input {
            Input::Player(PlayerEvent::TrackChanged { entry, path, .. }) => {
                self.anchor = (Duration::ZERO, self.clock.elapsed());
                self.load(Some(entry), Some(path));
            }
            Input::Player(PlayerEvent::Position { position, .. }) => {
                self.anchor = (position.as_duration(), self.clock.elapsed());
            }
            Input::Player(PlayerEvent::State { playback, .. }) => {
                self.anchor = (self.position(), self.clock.elapsed());
                self.playing = playback == PlaybackState::Playing;
            }
            Input::Player(PlayerEvent::Ended { .. }) => {
                self.playing = false;
                self.load(None, None);
            }
            Input::Player(_) | Input::Shutdown => {}
            Input::SetOffset(ms) => self.set_offset(ms),
            Input::AdjustOffset(delta) => {
                self.set_offset(self.state.offset_ms.saturating_add(delta))
            }
            Input::Reload => {
                let (entry, path) = (self.state.entry, self.state.path.clone());
                self.load(entry, path);
            }
        }
    }

    /// The position now, between the player's reports.
    fn position(&self) -> Duration {
        let (position, at) = self.anchor;
        if self.playing {
            position + self.clock.elapsed().saturating_sub(at)
        } else {
            position
        }
    }

    /// How long to wait for the next input: until the next line is due while
    /// playing synced lyrics, otherwise for ever.
    fn wait(&self) -> Option<Duration> {
        if !self.playing {
            return None;
        }
        let lyrics = self.state.lyrics.as_ref().filter(|l| l.synced)?;
        let next = lyrics.next_change(self.position(), self.state.offset_ms)?;
        Some(next.clamp(EARLY, LONGEST_WAIT))
    }

    /// Reads the lyrics of `path` and announces them.
    fn load(&mut self, entry: Option<EntryId>, path: Option<PathBuf>) {
        let lyrics =
            path.as_deref()
                .and_then(|path| match self.provider.find(&LyricsQuery { path }) {
                    Ok(found) => found.map(Arc::new),
                    Err(e) => {
                        log::warn!("cannot read the lyrics of {}: {e}", path.display());
                        None
                    }
                });
        let offset_ms = path.as_deref().map_or(0, |p| self.offsets.get(p));
        self.state = LyricsState {
            entry,
            path,
            lyrics,
            offset_ms,
            line: None,
        };
        self.announce_changed();
        self.follow_line();
    }

    fn set_offset(&mut self, offset_ms: i64) {
        let Some(path) = self.state.path.clone() else {
            return;
        };
        let offset_ms = offset_ms.clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS);
        if offset_ms == self.state.offset_ms {
            return;
        }
        self.state.offset_ms = offset_ms;
        self.offsets.set(&path, offset_ms);
        self.announce_changed();
        self.follow_line();
    }

    fn announce_changed(&self) {
        *self.shared.lock().unwrap_or_else(PoisonError::into_inner) = self.state.clone();
        self.bus.publish(&Event::Lyrics(LyricsEvent::Changed {
            entry: self.state.entry,
            path: self.state.path.clone(),
            lyrics: self.state.lyrics.clone(),
            offset_ms: self.state.offset_ms,
        }));
    }

    /// Announces the line sung now when it changed.
    fn follow_line(&mut self) {
        let Some(entry) = self.state.entry else {
            return;
        };
        let line = self
            .state
            .lyrics
            .as_ref()
            .and_then(|l| l.line_at(self.position() + EARLY, self.state.offset_ms));
        if line == self.state.line {
            return;
        }
        self.state.line = line;
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .line = line;
        self.bus
            .publish(&Event::Lyrics(LyricsEvent::Line { entry, index: line }));
    }
}

/// The offsets of the tracks, most recently set last.
#[derive(Debug, Default, Serialize, Deserialize)]
struct OffsetFile {
    tracks: Vec<(PathBuf, i64)>,
}

struct Offsets {
    file: Option<PathBuf>,
    data: OffsetFile,
}

impl Offsets {
    fn load(file: Option<PathBuf>) -> Self {
        let data = file
            .as_deref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { file, data }
    }

    fn get(&self, path: &Path) -> i64 {
        self.data
            .tracks
            .iter()
            .rev()
            .find(|(p, _)| p == path)
            .map_or(0, |(_, ms)| *ms)
    }

    fn set(&mut self, path: &Path, offset_ms: i64) {
        self.data.tracks.retain(|(p, _)| p != path);
        if offset_ms != 0 {
            self.data.tracks.push((path.to_owned(), offset_ms));
        }
        let excess = self.data.tracks.len().saturating_sub(MAX_OFFSETS);
        self.data.tracks.drain(..excess);
        let Some(file) = &self.file else { return };
        let saved = serde_json::to_vec(&self.data)
            .map_err(std::io::Error::other)
            .and_then(|bytes| crate::files::write_atomic(file, &bytes));
        if let Err(e) = saved {
            log::warn!("cannot save the lyrics offsets: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::player::EndReason;
    use helikodu_library::lyrics::LyricsError;
    use helikodu_model::audio::{AudioSpec, ChannelLayout, MediaTime, SampleFormat, SampleRate};
    use helikodu_model::library::{MediaSource, TrackRef};
    use helikodu_model::player::TransportState;

    /// Lyrics for any path whose name holds "sung": lines at 1, 3 and 5 s.
    #[derive(Debug)]
    struct Fake;

    impl LyricsProvider for Fake {
        fn find_all(&self, query: &LyricsQuery<'_>) -> Result<Vec<Lyrics>, LyricsError> {
            if !query.path.to_string_lossy().contains("sung") {
                return Ok(Vec::new());
            }
            let text = "[00:01.00]one\n[00:03.00]two\n[00:05.00]three\n";
            Ok(
                Lyrics::from_text(text, LyricsSource::Embedded(EmbeddedKind::Other))
                    .into_iter()
                    .collect(),
            )
        }
    }

    fn track_changed(entry: u64, path: &str) -> Event {
        Event::Player(PlayerEvent::TrackChanged {
            index: 0,
            count: 1,
            entry: EntryId::new(entry),
            track: TrackRef::File(MediaSource::file(path)),
            path: PathBuf::from(path),
            spec: AudioSpec::new(
                SampleFormat::U8,
                SampleRate::HZ_44100,
                ChannelLayout::STEREO,
            ),
            length: None,
        })
    }

    fn position(millis: u64) -> Event {
        Event::Player(PlayerEvent::Position {
            index: 0,
            position: MediaTime::from_millis(millis),
            length: None,
        })
    }

    fn state(playback: PlaybackState) -> Event {
        Event::Player(PlayerEvent::State {
            playback,
            transport: TransportState::Stopped,
        })
    }

    /// The next lyrics event, waiting at most two seconds.
    fn next(events: &Subscription<LyricsEvent>) -> LyricsEvent {
        events
            .recv_timeout(Duration::from_secs(2))
            .expect("a lyrics event")
    }

    fn line(events: &Subscription<LyricsEvent>) -> Option<usize> {
        match next(events) {
            LyricsEvent::Line { index, .. } => index,
            other => panic!("not a line: {other:?}"),
        }
    }

    fn quiet(events: &Subscription<LyricsEvent>) {
        if let Ok(event) = events.recv_timeout(Duration::from_millis(150)) {
            panic!("an event: {event:?}");
        }
    }

    fn start(
        dir: Option<&Path>,
    ) -> (
        LyricsService,
        EventBus<Event>,
        FakeClock,
        Subscription<LyricsEvent>,
    ) {
        let bus = EventBus::new();
        let clock = FakeClock::new(std::time::SystemTime::UNIX_EPOCH);
        let events = bus.subscribe_with(
            |e: &Event| match e {
                Event::Lyrics(l) => Some(l.clone()),
                _ => None,
            },
            None,
        );
        let service = LyricsService::start(
            &bus,
            clock.shared(),
            Arc::new(Fake),
            &PlayerStatus::default(),
            dir,
        )
        .expect("start");
        (service, bus, clock, events)
    }

    #[test]
    fn lines_follow_the_position_and_the_clock() {
        let (service, bus, clock, events) = start(None);
        bus.publish(&state(PlaybackState::Playing));
        bus.publish(&track_changed(1, "/m/sung.flac"));
        match next(&events) {
            LyricsEvent::Changed { entry, lyrics, .. } => {
                assert_eq!(entry, Some(EntryId::new(1)));
                assert_eq!(lyrics.expect("lyrics").lines.len(), 3);
            }
            other => panic!("{other:?}"),
        }
        // Before the first line nothing is announced.
        bus.publish(&position(500));
        quiet(&events);
        // The positions the player reports.
        bus.publish(&position(1_000));
        assert_eq!(line(&events), Some(0));
        bus.publish(&position(1_400));
        quiet(&events);
        // Between positions the clock moves the line on.
        clock.advance(Duration::from_millis(1_700));
        assert_eq!(line(&events), Some(1));
        // Paused: the clock moves, the line stays.
        bus.publish(&state(PlaybackState::Paused));
        quiet(&events);
        clock.advance(Duration::from_secs(10));
        quiet(&events);
        // A seek back.
        bus.publish(&position(200));
        assert_eq!(line(&events), None);
        assert_eq!(service.current().line, None);
        // A track without lyrics, then the end.
        bus.publish(&track_changed(2, "/m/instrumental.flac"));
        assert!(matches!(
            next(&events),
            LyricsEvent::Changed { lyrics: None, .. }
        ));
        bus.publish(&Event::Player(PlayerEvent::Ended {
            reason: EndReason::Stopped,
            played: 2,
            failed: 0,
        }));
        assert!(matches!(
            next(&events),
            LyricsEvent::Changed { entry: None, .. }
        ));
    }

    #[test]
    fn offsets_move_the_line_and_are_kept_per_track() {
        let dir =
            std::env::temp_dir().join(format!("helikodu-lyrics-offsets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let (service, bus, _clock, events) = start(Some(&dir));
            bus.publish(&track_changed(1, "/m/sung.flac"));
            assert!(matches!(
                next(&events),
                LyricsEvent::Changed { offset_ms: 0, .. }
            ));
            bus.publish(&position(2_800));
            assert_eq!(line(&events), Some(0));
            // Lines 300 ms sooner: the second one is due.
            service.adjust_offset(300);
            assert!(matches!(
                next(&events),
                LyricsEvent::Changed { offset_ms: 300, .. }
            ));
            assert_eq!(line(&events), Some(1));
        }
        // Kept for the track in the next run.
        let (_service, bus, _clock, events) = start(Some(&dir));
        bus.publish(&track_changed(5, "/m/sung.flac"));
        assert!(matches!(
            next(&events),
            LyricsEvent::Changed { offset_ms: 300, .. }
        ));
        bus.publish(&track_changed(6, "/m/other-sung.flac"));
        assert!(matches!(
            next(&events),
            LyricsEvent::Changed { offset_ms: 0, .. }
        ));
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
