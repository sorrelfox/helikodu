// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The service actor: the one thread that owns the queue and the engine.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use helikodu_engine::transport::Engine;
use helikodu_library::ListenEnd;
use helikodu_model::audio::{AudioSpec, MediaTime, OutputSelection};
use helikodu_model::library::{MediaSource, TrackId, TrackRef};
use helikodu_model::player::{
    ContextSource, EngineCommand, EngineError, EngineEvent, EntryId, ItemId, PlaybackItem,
    PlaybackState, QueueContext, StopAfter, TrackEndReason, TransportState, Volume,
};

use super::command::{ContextRequest, PlayerCommand};
use super::config::{PlayerPrefs, ReplayGainMode, SessionConfig};
use super::listen::{ListenTracker, Recorded, Recorder};
use super::loudness::LoudnessTags;
use super::outputs::{next_output, output_choices};
use super::queue::{Advance, Queue};
use super::session::{SESSION_VERSION, SavedSession, SessionFiles, Strike};
use super::shared::{NowPlaying, QueueSnapshot, Shared};
use super::{EndReason, Notice, PlayerEvent};
use crate::Event;
use crate::bus::EventBus;
use crate::clock::SharedClock;
use crate::library::{MAX_PAGE, MusicLibrary, TrackFilter};

/// The most search results a search context plays.
const SEARCH_LIMIT: usize = 5000;

/// The longest the actor sleeps while a save is due, in real time.
const MAX_WAIT: Duration = Duration::from_millis(500);

/// What arrives in the actor's mailbox.
#[derive(Debug)]
pub(super) enum Input {
    Command(PlayerCommand),
    Engine(EngineEvent),
    Recorded(Recorded),
    /// Reply once everything before it is handled and the session is saved.
    Sync(Sender<()>),
    Shutdown,
}

/// Which way playback moves through the queue, and so which way an unplayable entry
/// is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Backward,
}

/// An entry handed to the engine.
#[derive(Debug, Clone)]
struct Slot {
    id: ItemId,
    entry: EntryId,
    track: TrackRef,
    path: PathBuf,
    library_track: Option<TrackId>,
    length: Option<MediaTime>,
}

/// The session as the actor keeps it.
#[derive(Debug)]
struct Session {
    files: SessionFiles,
    config: SessionConfig,
    /// When the session changed and has not been saved since.
    dirty_since: Option<Duration>,
    last_save: Duration,
}

/// What the player starts with besides its configuration.
#[derive(Debug)]
pub(super) struct Startup {
    pub(super) queue: Queue,
    pub(super) position: Option<MediaTime>,
    pub(super) volume: Volume,
    pub(super) selection: OutputSelection,
    pub(super) suspects: HashSet<TrackRef>,
    pub(super) strikes: HashMap<TrackRef, u32>,
    pub(super) crash: Option<Notice>,
    pub(super) session: Option<(SessionFiles, SessionConfig)>,
    pub(super) replay_gain: ReplayGainMode,
    pub(super) prefs: PlayerPrefs,
    pub(super) library: Option<MusicLibrary>,
}

pub(super) struct Actor {
    engine: Engine,
    bus: EventBus<Event>,
    clock: SharedClock,
    shared: Arc<Shared>,
    queue: Queue,
    loudness: LoudnessTags,
    library: Option<MusicLibrary>,
    prefs: PlayerPrefs,
    last_id: u64,
    /// The entry playing, or being loaded.
    current: Option<Slot>,
    /// The entry sent with `SetNext`, until it starts.
    pending: Option<Slot>,
    /// The item of a `Load` that has neither started nor failed yet.
    in_flight: Option<ItemId>,
    /// `NearEnd` came for the current item: the next one is wanted.
    near_end: bool,
    /// Entries that could not be played in this queue; automatic playback passes
    /// over them.
    unplayable: HashSet<EntryId>,
    suspects: HashSet<TrackRef>,
    strikes: HashMap<TrackRef, u32>,
    direction: Direction,
    engine_state: TransportState,
    position: MediaTime,
    /// A restored position, held until "Play" loads the entry.
    restored: Option<MediaTime>,
    volume: Volume,
    selection: OutputSelection,
    stop_after: StopAfter,
    /// No entry is to follow the current one.
    exhausted: bool,
    stop_requested: bool,
    stopped_after: bool,
    /// `Ended` has been sent, or nothing has played yet.
    ended: bool,
    played: usize,
    failed: usize,
    listen: Option<ListenTracker>,
    recorder: Option<Recorder>,
    session: Option<Session>,
    published_version: u64,
    published_state: Option<PlaybackState>,
    crash: Option<Notice>,
}

impl Actor {
    pub(super) fn new(
        engine: Engine,
        bus: EventBus<Event>,
        clock: SharedClock,
        shared: Arc<Shared>,
        startup: Startup,
        recorder: Option<Recorder>,
    ) -> Self {
        let session = startup.session.map(|(files, config)| Session {
            files,
            config,
            dirty_since: None,
            last_save: clock.elapsed(),
        });
        let restored = startup
            .queue
            .current()
            .map(|_| startup.position.unwrap_or(MediaTime::ZERO));
        Self {
            engine,
            bus,
            clock,
            shared,
            queue: startup.queue,
            loudness: LoudnessTags::new(startup.replay_gain),
            library: startup.library,
            prefs: startup.prefs,
            last_id: 0,
            current: None,
            pending: None,
            in_flight: None,
            near_end: false,
            unplayable: HashSet::new(),
            suspects: startup.suspects,
            strikes: startup.strikes,
            direction: Direction::Forward,
            engine_state: TransportState::Stopped,
            position: restored.unwrap_or(MediaTime::ZERO),
            restored,
            volume: startup.volume,
            selection: startup.selection,
            stop_after: StopAfter::Off,
            exhausted: false,
            stop_requested: false,
            stopped_after: false,
            ended: true,
            played: 0,
            failed: 0,
            listen: None,
            recorder,
            session,
            published_version: u64::MAX,
            published_state: None,
            crash: startup.crash,
        }
    }

    pub(super) fn run(mut self, mailbox: &Receiver<Input>) {
        if let Some(notice) = self.crash.take() {
            self.emit(PlayerEvent::Notice(notice));
        }
        self.emit(PlayerEvent::VolumeChanged(self.volume));
        self.after_input();
        loop {
            let input = match self.save_due_in() {
                Some(wait) => match mailbox.recv_timeout(wait.min(MAX_WAIT)) {
                    Ok(input) => Some(input),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match mailbox.recv() {
                    Ok(input) => Some(input),
                    Err(_) => break,
                },
            };
            match input {
                Some(Input::Command(command)) => self.command(command),
                Some(Input::Engine(event)) => self.engine_event(event),
                Some(Input::Recorded((track, result))) => self.emit(match result {
                    Ok(outcome) => PlayerEvent::ListenRecorded { track, outcome },
                    Err(reason) => PlayerEvent::Notice(Notice::ListenNotRecorded { track, reason }),
                }),
                Some(Input::Sync(reply)) => {
                    self.after_input();
                    self.save_session();
                    let _ = reply.send(());
                    continue;
                }
                Some(Input::Shutdown) => break,
                None => {}
            }
            self.after_input();
        }
        self.shut_down();
    }

    fn shut_down(&mut self) {
        self.finish_listen(ListenEnd::Stopped);
        if let Some(recorder) = &mut self.recorder {
            recorder.finish();
        }
        self.save_session();
        if let Some(session) = &mut self.session {
            let _ = session.files.clear();
        }
    }

    // --- Events and shared state --------------------------------------------------

    fn emit(&self, event: PlayerEvent) {
        self.bus.publish(&Event::Player(event));
    }

    fn problem(&self, entry: Option<EntryId>, path: Option<PathBuf>, error: EngineError) {
        let file = entry.and_then(|e| self.queue.index_of(e)).zip(path);
        self.emit(PlayerEvent::Problem { file, entry, error });
    }

    fn send(&self, command: EngineCommand) {
        if let Err(gone) = self.engine.send(command) {
            self.problem(
                None,
                None,
                EngineError::Output {
                    reason: gone.to_string(),
                },
            );
        }
    }

    fn playback_state(&self) -> PlaybackState {
        match self.engine_state {
            TransportState::Playing | TransportState::Draining => PlaybackState::Playing,
            TransportState::Paused => PlaybackState::Paused,
            TransportState::Loading | TransportState::Seeking | TransportState::Reconfiguring => {
                PlaybackState::Buffering
            }
            TransportState::Stopped if self.in_flight.is_some() => PlaybackState::Buffering,
            TransportState::Stopped if self.restored.is_some() => PlaybackState::Paused,
            TransportState::Stopped => PlaybackState::Stopped,
        }
    }

    fn emit_state(&mut self) {
        let playback = self.playback_state();
        self.published_state = Some(playback);
        self.emit(PlayerEvent::State {
            playback,
            transport: self.engine_state,
        });
    }

    /// Publishes what changed while handling an input, names what plays in the crash
    /// sentinel, and saves the session when it is due.
    fn after_input(&mut self) {
        if self.queue.version() != self.published_version {
            self.published_version = self.queue.version();
            self.shared.set_queue(QueueSnapshot {
                version: self.queue.version(),
                context: self.queue.context().clone(),
                entries: self.queue.items().to_vec(),
                current: self.queue.current_index(),
                modes: self.queue.modes(),
            });
            self.emit(PlayerEvent::QueueChanged {
                version: self.queue.version(),
            });
            self.mark_dirty();
        }
        if self.published_state != Some(self.playback_state()) {
            self.emit_state();
        }
        let status = (
            self.playback_state(),
            self.engine_state,
            self.position,
            self.volume,
            self.selection.clone(),
            self.queue.modes(),
            self.stop_after,
        );
        self.shared.update_status(|s| {
            (
                s.playback,
                s.transport,
                s.position,
                s.volume,
                s.output,
                s.modes,
                s.stop_after,
            ) = status;
        });
        self.name_in_sentinel();
        if self.save_due_in() == Some(Duration::ZERO) {
            self.save_session();
        }
        if self.playback_state() == PlaybackState::Playing
            && let Some(session) = &self.session
            && self.clock.elapsed().saturating_sub(session.last_save)
                >= session.config.position_interval
        {
            self.save_session();
        }
    }

    fn mark_dirty(&mut self) {
        let now = self.clock.elapsed();
        if let Some(session) = &mut self.session
            && session.dirty_since.is_none()
        {
            session.dirty_since = Some(now);
        }
    }

    /// How long until the session is to be saved, if it changed.
    fn save_due_in(&self) -> Option<Duration> {
        let session = self.session.as_ref()?;
        let since = session.dirty_since?;
        let due = since + session.config.debounce;
        Some(due.saturating_sub(self.clock.elapsed()))
    }

    fn save_session(&mut self) {
        let position = self.restored.unwrap_or(self.position);
        let saved = SavedSession {
            version: SESSION_VERSION,
            context: self.queue.context().clone(),
            entries: self.queue.items().to_vec(),
            linear: self.queue.linear().to_vec(),
            current: self.queue.current_index(),
            position_ms: u64::try_from(position.as_duration().as_millis()).unwrap_or(u64::MAX),
            modes: self.queue.modes(),
            volume: self.volume,
            output: self.selection.clone(),
            suspects: self.suspects.iter().cloned().collect(),
        };
        let now = self.clock.elapsed();
        let Some(session) = &mut self.session else {
            return;
        };
        session.dirty_since = None;
        session.last_save = now;
        if let Err(error) = session.files.save(&saved) {
            let reason = error.to_string();
            self.emit(PlayerEvent::Notice(Notice::SessionNotSaved { reason }));
        }
    }

    fn name_in_sentinel(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        let playing = !self.ended || self.in_flight.is_some();
        let named: Vec<Strike> = [&self.current, &self.pending]
            .into_iter()
            .flatten()
            .filter(|_| playing)
            .map(|slot| Strike {
                track: slot.track.clone(),
                strikes: self.strikes.get(&slot.track).copied().unwrap_or(0),
            })
            .collect();
        if let Err(error) = session.files.name(named) {
            let reason = format!("cannot write the crash sentinel: {error}");
            self.emit(PlayerEvent::Notice(Notice::SessionNotSaved { reason }));
        }
    }

    // --- Commands -----------------------------------------------------------------

    fn command(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::PlayFiles { files, start } => {
                let tracks = files
                    .into_iter()
                    .map(|path| TrackRef::File(MediaSource::file(path)))
                    .collect();
                self.play_tracks(QueueContext::files(), tracks, start);
            }
            PlayerCommand::PlayContext {
                context,
                title,
                start,
            } => match self.resolve_context(&context, title) {
                Ok((context, tracks)) => self.play_tracks(context, tracks, start),
                Err(reason) => {
                    self.emit(PlayerEvent::Notice(Notice::ContextUnavailable { reason }))
                }
            },
            PlayerCommand::PlayTracks {
                context,
                tracks,
                start,
            } => self.play_tracks(context, tracks, start),
            PlayerCommand::Enqueue { tracks, at } => {
                self.queue.enqueue(tracks, at);
                self.refresh_next();
            }
            PlayerCommand::PlayEntry(entry) => {
                if self.queue.jump_to(entry) {
                    self.unplayable.remove(&entry);
                    self.load(entry, Direction::Forward, None);
                }
            }
            PlayerCommand::Remove(entries) => self.remove(&entries),
            PlayerCommand::Move { entry, to } => {
                self.queue.move_entry(entry, to);
                self.refresh_next();
            }
            PlayerCommand::Clear => {
                let keep = !self.is_stopped() || self.restored.is_some();
                self.queue.clear(keep);
                if !keep {
                    self.current = None;
                }
                self.refresh_next();
            }
            PlayerCommand::PlayPause => match self.engine_state {
                TransportState::Paused | TransportState::Stopped => self.play(),
                _ => self.send(EngineCommand::Pause),
            },
            PlayerCommand::Play => self.play(),
            PlayerCommand::Pause => {
                if !self.is_stopped() {
                    self.send(EngineCommand::Pause);
                }
            }
            PlayerCommand::Stop => {
                if !self.is_stopped() {
                    self.finish_listen(ListenEnd::Stopped);
                    self.stop_requested = true;
                    self.send(EngineCommand::Stop);
                }
            }
            PlayerCommand::Next => self.next(),
            PlayerCommand::Previous => self.previous(),
            PlayerCommand::Seek(to) => self.seek(to),
            PlayerCommand::SeekForward(by) => {
                let to = self.position.as_duration().saturating_add(by);
                self.seek(MediaTime::from_duration(to));
            }
            PlayerCommand::SeekBackward(by) => {
                let to = self.position.as_duration().saturating_sub(by);
                self.seek(MediaTime::from_duration(to));
            }
            PlayerCommand::SetVolume(volume) => self.set_volume(volume),
            PlayerCommand::AdjustVolume(by) => {
                self.set_volume(Volume::clamped(self.volume.position() + by));
            }
            PlayerCommand::SetShuffle(shuffle) => {
                self.queue.set_shuffle(shuffle);
                self.emit(PlayerEvent::ModesChanged(self.queue.modes()));
                self.refresh_next();
            }
            PlayerCommand::SetRepeat(repeat) => {
                self.queue.set_repeat(repeat);
                self.emit(PlayerEvent::ModesChanged(self.queue.modes()));
                self.refresh_next();
            }
            PlayerCommand::SetStopAfter(stop_after) => {
                if stop_after != self.stop_after {
                    self.stop_after = stop_after;
                    self.emit(PlayerEvent::StopAfterChanged(stop_after));
                    self.refresh_next();
                }
            }
            PlayerCommand::SetOutput(selection) => self.set_output(selection),
            PlayerCommand::NextOutput => {
                let choices = output_choices(&self.selection);
                match next_output(&self.selection, &choices) {
                    Some(selection) => self.set_output(selection),
                    None => self.problem(
                        None,
                        None,
                        EngineError::Output {
                            reason: "there is no other output device".into(),
                        },
                    ),
                }
            }
            PlayerCommand::Configure(prefs) => self.prefs = prefs,
        }
    }

    /// Whether the engine is stopped with nothing on the way.
    fn is_stopped(&self) -> bool {
        self.engine_state == TransportState::Stopped && self.in_flight.is_none()
    }

    /// The next entry for automatic playback, telling the user about a suspect
    /// passed over.
    fn peek_next(&mut self, advance: Advance) -> Option<EntryId> {
        let suspect = RefCell::new(None);
        let unplayable = &self.unplayable;
        let suspects = &self.suspects;
        let next = self.queue.peek_next(advance, |e| {
            let skip = unplayable.contains(&e.id) || suspects.contains(&e.track);
            if skip && !unplayable.contains(&e.id) && suspect.borrow().is_none() {
                *suspect.borrow_mut() = Some((e.id, e.track.clone()));
            }
            skip
        });
        if let Some((entry, track)) = suspect.into_inner() {
            self.emit(PlayerEvent::Notice(Notice::SuspectSkipped { entry, track }));
        }
        next
    }

    fn play_tracks(&mut self, context: QueueContext, tracks: Vec<TrackRef>, start: usize) {
        let append = self.prefs.append_contexts && self.queue.len() > 0;
        let first = if append {
            let first = self.queue.append_context(context, tracks, start);
            if let Some(first) = first {
                self.queue.jump_to(first);
            }
            first
        } else {
            self.loudness.clear();
            self.unplayable.clear();
            self.queue.replace(context, tracks, start)
        };
        self.played = 0;
        self.failed = 0;
        self.restored = None;
        match first {
            Some(first) => self.load(first, Direction::Forward, None),
            None => {
                self.finish_listen(ListenEnd::Skipped);
                self.current = None;
                self.pending = None;
                if !self.is_stopped() {
                    self.exhausted = true;
                    self.send(EngineCommand::Stop);
                } else {
                    self.ended = true;
                    self.emit(PlayerEvent::Ended {
                        reason: EndReason::QueueFinished,
                        played: 0,
                        failed: 0,
                    });
                }
            }
        }
    }

    fn resolve_context(
        &self,
        request: &ContextRequest,
        title: Option<String>,
    ) -> Result<(QueueContext, Vec<TrackRef>), String> {
        let library = self
            .library
            .as_ref()
            .ok_or_else(|| "there is no library".to_owned())?;
        let number = |id: i64| u64::try_from(id).unwrap_or(0);
        let tracks_of = |filter| -> Result<Vec<TrackRef>, String> {
            Ok(library
                .all_tracks(filter)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|t| TrackRef::Library(t.id))
                .collect())
        };
        let (source, title, tracks) = match request {
            ContextRequest::Album(album) => {
                let title = match title {
                    Some(title) => Some(title),
                    None => library
                        .album(*album)
                        .map_err(|e| e.to_string())?
                        .and_then(|detail| detail.summary.title),
                };
                (
                    ContextSource::Album(number(album.get())),
                    title,
                    tracks_of(TrackFilter::Album(*album))?,
                )
            }
            ContextRequest::Artist(person) => (
                ContextSource::Artist(number(person.get())),
                title,
                tracks_of(TrackFilter::Artist(*person, None))?,
            ),
            ContextRequest::Genre(genre) => (
                ContextSource::Genre(number(genre.get())),
                title,
                tracks_of(TrackFilter::Genre(*genre))?,
            ),
            ContextRequest::Search(text) => {
                let mut tracks = Vec::new();
                loop {
                    let page = library
                        .search(text, tracks.len(), MAX_PAGE)
                        .map_err(|e| e.to_string())?;
                    let done = page.hits.is_empty()
                        || tracks.len() + page.hits.len() >= page.total.min(SEARCH_LIMIT);
                    tracks.extend(
                        page.hits
                            .into_iter()
                            .map(|h| TrackRef::Library(h.hit.track)),
                    );
                    if done {
                        break;
                    }
                }
                tracks.truncate(SEARCH_LIMIT);
                (ContextSource::Query(text.clone()), title, tracks)
            }
        };
        if tracks.is_empty() {
            return Err("it has no tracks".into());
        }
        Ok((QueueContext { source, title }, tracks))
    }

    fn remove(&mut self, entries: &[EntryId]) {
        let pending_removed = self
            .pending
            .as_ref()
            .is_some_and(|p| entries.contains(&p.entry));
        let removed_current = self.queue.remove(entries);
        if pending_removed {
            self.pending = None;
        }
        if removed_current {
            if self.is_stopped() {
                self.current = None;
                self.restored = self.queue.current().map(|_| MediaTime::ZERO);
            } else {
                match self.queue.current().map(|e| e.id) {
                    Some(next) => self.load(next, Direction::Forward, None),
                    None => {
                        self.finish_listen(ListenEnd::Skipped);
                        self.exhausted = true;
                        self.send(EngineCommand::Stop);
                    }
                }
            }
            return;
        }
        self.refresh_next();
    }

    fn play(&mut self) {
        match self.engine_state {
            TransportState::Paused => self.send(EngineCommand::Play),
            TransportState::Stopped if self.in_flight.is_none() => {
                let entry = match self.queue.current() {
                    Some(entry) => Some(entry.id),
                    None => self.peek_next(Advance::User),
                };
                if let Some(entry) = entry {
                    self.queue.set_current(entry);
                    let start = self.restored.take();
                    self.load(entry, Direction::Forward, start);
                }
            }
            _ => {}
        }
    }

    fn next(&mut self) {
        if self.queue.current().is_none() {
            return;
        }
        match self.peek_next(Advance::User) {
            Some(next) => {
                self.queue.advance_to(next);
                self.load(next, Direction::Forward, None);
            }
            None if !self.is_stopped() => {
                self.finish_listen(ListenEnd::Skipped);
                self.exhausted = true;
                self.send(EngineCommand::Stop);
            }
            None => {}
        }
    }

    fn previous(&mut self) {
        let Some(current) = self.queue.current().map(|e| e.id) else {
            return;
        };
        let restart = !self.is_stopped()
            && self
                .prefs
                .previous_restart
                .is_some_and(|after| self.position.as_duration() > after);
        let unplayable = &self.unplayable;
        let suspects = &self.suspects;
        let previous = self
            .queue
            .peek_previous(|e| unplayable.contains(&e.id) || suspects.contains(&e.track));
        match previous {
            Some(previous) if !restart => {
                self.queue.set_current(previous);
                self.load(previous, Direction::Backward, None);
            }
            _ if self.is_stopped() => {
                self.restored = None;
                self.load(current, Direction::Forward, None);
            }
            _ => self.seek(MediaTime::ZERO),
        }
    }

    fn seek(&mut self, to: MediaTime) {
        if self.restored.is_some() && self.is_stopped() {
            self.restored = Some(to);
            self.position = to;
            self.mark_dirty();
        } else if !self.is_stopped() {
            if let Some(listen) = &mut self.listen {
                listen.seeked();
            }
            self.send(EngineCommand::Seek(to));
        }
    }

    fn set_volume(&mut self, volume: Volume) {
        self.volume = volume;
        self.send(EngineCommand::SetVolume(volume));
        self.emit(PlayerEvent::VolumeChanged(volume));
        self.mark_dirty();
    }

    fn set_output(&mut self, selection: OutputSelection) {
        self.selection = selection.clone();
        self.send(EngineCommand::SetOutput(selection.clone()));
        self.emit(PlayerEvent::OutputChanged(selection));
        self.mark_dirty();
    }

    // --- Loading ------------------------------------------------------------------

    /// What the engine plays for `track`, and the library track it is.
    fn resolve(&self, track: &TrackRef) -> Result<(MediaSource, Option<TrackId>), String> {
        match track {
            TrackRef::Library(id) => {
                let library = self
                    .library
                    .as_ref()
                    .ok_or_else(|| "library tracks need the library".to_owned())?;
                match library.track_source(*id) {
                    Ok(Some(source)) => Ok((source.source, Some(*id))),
                    Ok(None) => Err("the track is no longer in the library".into()),
                    Err(error) => Err(error.to_string()),
                }
            }
            TrackRef::File(source) => {
                let in_library = match (&self.library, &source.section) {
                    (Some(library), None) => library.track_at(source.path()).ok().flatten(),
                    _ => None,
                };
                Ok((source.clone(), in_library))
            }
        }
    }

    /// A playback item for `entry`, with its slot.
    fn item_for(
        &mut self,
        entry: EntryId,
    ) -> Result<(PlaybackItem, Slot), (Option<PathBuf>, String)> {
        let track = self
            .queue
            .entry(entry)
            .map(|e| e.track.clone())
            .ok_or((None, "the entry is no longer in the queue".to_owned()))?;
        let (source, library_track) = self.resolve(&track).map_err(|reason| (None, reason))?;
        let neighbours = self
            .queue
            .neighbours(entry)
            .map(|n| n.and_then(|e| self.path_of(&e.track)));
        self.last_id += 1;
        let id = ItemId::new(self.last_id);
        let path = source.path.clone();
        let item = PlaybackItem::new(id, source);
        let item = self.loudness.complete(&path, &neighbours, item);
        Ok((
            item,
            Slot {
                id,
                entry,
                track,
                path,
                library_track,
                length: None,
            },
        ))
    }

    /// The file of `track` without asking the library for more than its path.
    fn path_of(&self, track: &TrackRef) -> Option<PathBuf> {
        match track {
            TrackRef::File(source) => Some(source.path.clone()),
            TrackRef::Library(id) => self
                .library
                .as_ref()?
                .track_source(*id)
                .ok()
                .flatten()
                .map(|s| s.source.path),
        }
    }

    /// Loads `entry`, the queue's current entry, replacing whatever plays; at
    /// `start`, or where a long track last stopped. An entry that cannot be resolved
    /// is skipped in `direction`.
    fn load(&mut self, entry: EntryId, mut direction: Direction, start: Option<MediaTime>) {
        self.finish_listen(ListenEnd::Skipped);
        let mut target = Some(entry);
        while let Some(entry) = target {
            self.queue.set_current(entry);
            match self.item_for(entry) {
                Ok((item, slot)) => {
                    let start = start
                        .or_else(|| self.resume_position(slot.library_track))
                        .unwrap_or(MediaTime::ZERO);
                    self.current = Some(slot);
                    self.pending = None;
                    self.in_flight = Some(item.id);
                    self.near_end = false;
                    self.direction = direction;
                    self.position = start;
                    self.restored = None;
                    self.exhausted = false;
                    self.stop_requested = false;
                    self.stopped_after = false;
                    self.ended = false;
                    self.send(EngineCommand::Load {
                        item,
                        start,
                        paused: false,
                    });
                    return;
                }
                Err((path, reason)) => {
                    self.failed += 1;
                    self.unplayable.insert(entry);
                    self.problem(Some(entry), path, EngineError::Unplayable { reason });
                    (target, direction) = self.skip_from(direction);
                }
            }
        }
        // Nothing left to play.
        self.current = None;
        self.pending = None;
        self.exhausted = true;
        if self.is_stopped() {
            self.ended = false;
            self.end_if_stopped();
        } else {
            self.send(EngineCommand::Stop);
        }
    }

    /// The entry to try after the current one failed, moving in `direction` (and
    /// forward when nothing is left backward). Moves the queue there.
    fn skip_from(&mut self, direction: Direction) -> (Option<EntryId>, Direction) {
        if direction == Direction::Backward {
            let unplayable = &self.unplayable;
            let suspects = &self.suspects;
            if let Some(previous) = self
                .queue
                .peek_previous(|e| unplayable.contains(&e.id) || suspects.contains(&e.track))
            {
                self.queue.set_current(previous);
                return (Some(previous), Direction::Backward);
            }
        }
        let next = self.peek_next(Advance::User);
        if let Some(next) = next {
            self.queue.advance_to(next);
        }
        (next, Direction::Forward)
    }

    fn resume_position(&self, track: Option<TrackId>) -> Option<MediaTime> {
        if !self.prefs.resume_long_tracks {
            return None;
        }
        let position = self.library.as_ref()?.resume_position(track?).ok()??;
        Some(MediaTime::from_duration(position))
    }

    /// Names the entry to follow the current one, or none, when the engine has asked
    /// for it and the answer changed.
    fn refresh_next(&mut self) {
        if !self.near_end || self.current.is_none() || self.in_flight.is_some() {
            return;
        }
        let wanted = self.wanted_next();
        if wanted != self.pending.as_ref().map(|p| p.entry) {
            self.set_next(wanted);
        }
    }

    fn wanted_next(&mut self) -> Option<EntryId> {
        if self.stop_after == StopAfter::Track {
            return None;
        }
        self.peek_next(Advance::Auto)
    }

    fn set_next(&mut self, mut wanted: Option<EntryId>) {
        while let Some(entry) = wanted {
            match self.item_for(entry) {
                Ok((item, slot)) => {
                    self.pending = Some(slot);
                    self.send(EngineCommand::SetNext(Some(item)));
                    return;
                }
                Err((path, reason)) => {
                    self.failed += 1;
                    self.unplayable.insert(entry);
                    self.problem(Some(entry), path, EngineError::Unplayable { reason });
                    wanted = self.wanted_next();
                }
            }
        }
        self.pending = None;
        self.send(EngineCommand::SetNext(None));
    }

    fn finish_listen(&mut self, end: ListenEnd) {
        let Some(listen) = self.listen.take() else {
            return;
        };
        let (track, listen) = listen.finish(end);
        if let Some(recorder) = &self.recorder {
            recorder.record(track, listen, self.prefs.play_rules);
        }
    }

    // --- Engine events ------------------------------------------------------------

    fn engine_event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::StateChanged(state) => {
                self.engine_state = state;
                self.emit_state();
                self.end_if_stopped();
            }
            EngineEvent::TrackStarted { item, spec, length } => {
                self.track_started(item, spec, length);
            }
            EngineEvent::Position { item, position } => {
                if self.current.as_ref().is_some_and(|slot| slot.id == item) {
                    self.position = position;
                    if let Some(listen) = self.listen.as_mut().filter(|l| l.item == item) {
                        listen.position(position);
                    }
                    let entry = self.current.as_ref().map(|s| s.entry);
                    let length = self.current.as_ref().and_then(|s| s.length);
                    let index = entry.and_then(|e| self.queue.index_of(e)).unwrap_or(0);
                    self.emit(PlayerEvent::Position {
                        index,
                        position,
                        length,
                    });
                }
            }
            EngineEvent::NearEnd { item } => {
                if self.current.as_ref().is_some_and(|slot| slot.id == item) {
                    self.near_end = true;
                    let wanted = self.wanted_next();
                    self.set_next(wanted);
                }
            }
            EngineEvent::TrackEnded { item, reason } => self.track_ended(item, reason),
            EngineEvent::Error { item, error } => {
                let slot = item.and_then(|id| {
                    [&self.current, &self.pending]
                        .into_iter()
                        .flatten()
                        .find(|slot| slot.id == id)
                        .cloned()
                });
                match slot {
                    Some(slot) => self.problem(Some(slot.entry), Some(slot.path), error),
                    None => self.problem(None, None, error),
                }
            }
            EngineEvent::DevicesChanged { backend, devices } => {
                self.emit(PlayerEvent::DevicesChanged { backend, devices });
            }
        }
    }

    fn track_started(&mut self, item: ItemId, spec: AudioSpec, length: Option<MediaTime>) {
        if self.in_flight == Some(item) {
            self.in_flight = None;
        }
        let mut slot = match (&self.current, &self.pending) {
            (Some(current), _) if current.id == item => current.clone(),
            (_, Some(pending)) if pending.id == item => {
                let pending = pending.clone();
                self.pending = None;
                self.queue.advance_to(pending.entry);
                pending
            }
            _ => return,
        };
        slot.length = length;
        self.current = Some(slot.clone());
        self.near_end = false;
        self.played += 1;
        self.position = MediaTime::ZERO;
        self.direction = Direction::Forward;
        if let Some(track) = slot.library_track {
            self.listen = Some(ListenTracker::new(
                item,
                track,
                self.clock.now(),
                length,
                Some(context_label(self.queue.context())),
            ));
        }
        let index = self.queue.index_of(slot.entry).unwrap_or(0);
        let now_playing = NowPlaying {
            entry: slot.entry,
            track: slot.track.clone(),
            path: slot.path.clone(),
            spec,
            length,
        };
        self.shared
            .update_status(|status| status.now_playing = Some(now_playing));
        self.emit(PlayerEvent::TrackChanged {
            index,
            count: self.queue.len(),
            entry: slot.entry,
            track: slot.track,
            path: slot.path,
            spec,
            length,
        });
    }

    fn track_ended(&mut self, item: ItemId, reason: TrackEndReason) {
        if reason == TrackEndReason::Failed
            && let Some(pending) = self.pending.take_if(|slot| slot.id == item)
        {
            // The engine asks for another one with `NearEnd`.
            self.failed += 1;
            self.unplayable.insert(pending.entry);
            return;
        }
        let Some(current) = self.current.clone().filter(|slot| slot.id == item) else {
            return;
        };
        if self.listen.as_ref().is_some_and(|l| l.item == item) {
            self.finish_listen(match reason {
                TrackEndReason::Finished => ListenEnd::Finished,
                _ => ListenEnd::Stopped,
            });
        }
        if reason == TrackEndReason::Finished && self.suspects.remove(&current.track) {
            self.strikes.remove(&current.track);
            self.mark_dirty();
        }
        if reason == TrackEndReason::Failed {
            self.failed += 1;
        }
        if self.in_flight == Some(item) {
            // The entry did not open: skip it.
            self.in_flight = None;
            if reason == TrackEndReason::Failed && !self.stop_requested {
                self.unplayable.insert(current.entry);
                let (next, direction) = self.skip_from(self.direction);
                if let Some(next) = next {
                    self.load(next, direction, None);
                    return;
                }
                self.exhausted = true;
            }
        } else if reason == TrackEndReason::Finished
            && self.pending.is_none()
            && self.stop_after == StopAfter::Track
        {
            self.stopped_after = true;
            self.stop_after = StopAfter::Off;
            self.emit(PlayerEvent::StopAfterChanged(StopAfter::Off));
            if let Some(next) = self.peek_next(Advance::Auto) {
                self.queue.advance_to(next);
            }
            self.current = None;
        } else if reason != TrackEndReason::Interrupted && self.pending.is_none() {
            self.exhausted = true;
        }
        self.end_if_stopped();
    }

    /// Sends `Ended` once the engine has stopped for good.
    fn end_if_stopped(&mut self) {
        if self.ended || !self.is_stopped() {
            return;
        }
        self.ended = true;
        self.finish_listen(ListenEnd::Stopped);
        self.pending = None;
        let reason = if self.stop_requested {
            EndReason::Stopped
        } else if self.stopped_after {
            EndReason::StopAfter
        } else if self.exhausted {
            EndReason::QueueFinished
        } else {
            EndReason::OutputFailed
        };
        self.mark_dirty();
        self.emit(PlayerEvent::Ended {
            reason,
            played: self.played,
            failed: self.failed,
        });
    }
}

/// How a context is named in the listening history.
fn context_label(context: &QueueContext) -> String {
    match &context.source {
        ContextSource::Album(id) => format!("album:{id}"),
        ContextSource::Artist(id) => format!("artist:{id}"),
        ContextSource::Genre(id) => format!("genre:{id}"),
        ContextSource::Playlist(id) => format!("playlist:{id}"),
        ContextSource::Query(text) => format!("search:{text}"),
        ContextSource::Folder(path) => format!("folder:{}", path.display()),
        ContextSource::Files => "files".into(),
    }
}
