// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The smarter playback of the actor: the shuffle filters and weights, endless play
//! and the Auto-DJ (tracks added to the end of the queue through the queue's own
//! `enqueue`), Revert, Radio Random and intro scan.

use std::collections::HashSet;
use std::time::Duration;

use helikodu_library::{NoSoundSimilarity, PlaySeed, SoundSimilarity};
use helikodu_model::audio::MediaTime;
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::{EnqueueAt, EntryId, QueueContext, RepeatMode};
use helikodu_model::settings::{SeedKind, SmartPlaySettings};

use super::{Actor, ContextRequest, Direction, Notice, PlayerEvent, Slot};
use crate::player::SmartQueue;

/// How many entries "Revert" remembers.
const REMEMBERED_SKIPS: usize = 50;

/// How many recently started tracks are remembered, at most the largest window.
const RECENT_LIMIT: usize = 10_000;

impl Actor {
    /// Takes the smarter playback settings at the start.
    pub(super) fn with_smart_play(mut self, settings: SmartPlaySettings) -> Self {
        self.apply_smart_play(settings);
        self
    }

    /// Takes new smarter playback settings while running.
    pub(super) fn configure_smart_play(&mut self, settings: SmartPlaySettings) {
        if settings == self.smart {
            return;
        }
        self.apply_smart_play(settings);
        self.set_nothing_to_add(false);
        let tracks: Vec<TrackRef> = self.queue.items().iter().map(|e| e.track.clone()).collect();
        let keys = self.keys_for(&tracks);
        self.queue.rekey(|_| keys);
        self.continue_queue();
        self.refresh_next();
    }

    fn apply_smart_play(&mut self, settings: SmartPlaySettings) {
        self.shuffle_skips = match &self.library {
            Some(library) => match library.tracks_matching(&settings.shuffle.skip_query) {
                Ok(found) => found.unwrap_or_default(),
                Err(error) => {
                    self.emit(PlayerEvent::Notice(Notice::ContextUnavailable {
                        reason: format!("playback.shuffle.skip_query: {error}"),
                    }));
                    HashSet::new()
                }
            },
            None => HashSet::new(),
        };
        self.queue.set_weights(settings.shuffle.weight_by.clone());
        self.smart = settings;
    }

    /// How alike two tracks sound, for endless play: the library's similarity
    /// index while the sound likeness has a weight, otherwise nothing (the index
    /// is then not built at all).
    fn sound_similarity(&self) -> std::sync::Arc<dyn SoundSimilarity + Send + Sync> {
        match &self.library {
            Some(library) if self.smart.endless.weights.sound > 0 => library.sound_similarity(),
            _ => std::sync::Arc::new(NoSoundSimilarity),
        }
    }

    /// How many entries after the current one would play.
    fn upcoming(&self) -> usize {
        let skip = self.skipper();
        let from = self.queue.current_index().map_or(0, |c| c + 1);
        self.queue.items()[from.min(self.queue.len())..]
            .iter()
            .filter(|e| !skip.skips(e) && !skip.suspects.contains(&e.id))
            .count()
    }

    /// The tracks not to add again: those in the queue and the last `window` that
    /// started.
    fn avoided(&self, window: u32) -> HashSet<TrackId> {
        let mut avoid: HashSet<TrackId> = self
            .queue
            .items()
            .iter()
            .filter_map(|e| match e.track {
                TrackRef::Library(id) => Some(id),
                TrackRef::File(_) => None,
            })
            .collect();
        avoid.extend(self.recent.iter().rev().take(window as usize));
        avoid
    }

    /// The library track endless play starts from: the last of the queue, or the
    /// last that started.
    fn last_library_track(&self) -> Option<TrackId> {
        self.queue
            .items()
            .iter()
            .rev()
            .find_map(|e| match e.track {
                TrackRef::Library(id) => Some(id),
                TrackRef::File(_) => None,
            })
            .or_else(|| self.recent.back().copied())
    }

    /// The seed of endless play from the last track, as `playback.endless.seed`
    /// says.
    fn seed_from_queue(&self) -> Option<PlaySeed> {
        let track = self.last_library_track()?;
        let library = self.library.as_ref()?;
        let facts = library.track_facts(&[track]).ok()?.into_iter().next();
        Some(match (self.smart.endless.seed, facts) {
            (SeedKind::Album, Some(facts)) if facts.album.is_some() => {
                PlaySeed::Album(facts.album?)
            }
            (SeedKind::Artist, Some(facts)) if !facts.artists.is_empty() => {
                PlaySeed::Artist(facts.artists[0])
            }
            _ => PlaySeed::Track(track),
        })
    }

    /// With endless play, adds related tracks to the end of the queue when nothing
    /// is left to play; with the Auto-DJ, tops the queue up to the tracks it keeps
    /// ahead. Does nothing in other repeat modes or without a library.
    pub(super) fn continue_queue(&mut self) {
        let repeat = self.queue.modes().repeat;
        if !repeat.continues_from_library() {
            return;
        }
        let Some(library) = self.library.clone() else {
            return;
        };
        let upcoming = self.upcoming();
        let draw = self.draws.next_seed();
        let picked = if repeat == RepeatMode::Endless {
            if upcoming > 0 {
                return;
            }
            // A radio's seed stays; otherwise each batch follows the queue's
            // last track as it is then.
            let Some(seed) = self.endless_seed.or_else(|| self.seed_from_queue()) else {
                return;
            };
            let settings = self.smart.endless.clone();
            let avoid = self.avoided(settings.window);
            let sound = self.sound_similarity();
            library.related_tracks(
                seed,
                &settings,
                &avoid,
                settings.batch as usize,
                draw,
                sound.as_ref(),
            )
        } else {
            let ahead = self.smart.auto_dj.ahead as usize;
            if upcoming >= ahead {
                return;
            }
            let settings = self.smart.auto_dj.clone();
            let avoid = self.avoided(settings.window);
            library.auto_dj_tracks(&settings, &avoid, ahead - upcoming, draw)
        };
        match picked {
            Ok(tracks) if !tracks.is_empty() => {
                let tracks = self.keyed(tracks.into_iter().map(TrackRef::Library).collect());
                let ids = self.queue.enqueue(tracks, EnqueueAt::End);
                self.added.extend(ids);
                self.smart_dirty = true;
                self.set_nothing_to_add(false);
            }
            Ok(_) => self.set_nothing_to_add(true),
            Err(error) => self.emit(PlayerEvent::Notice(Notice::ContextUnavailable {
                reason: error.to_string(),
            })),
        }
    }

    /// Notes whether endless play or the Auto-DJ found nothing to add.
    pub(super) fn set_nothing_to_add(&mut self, nothing: bool) {
        if self.nothing_to_add != nothing {
            self.nothing_to_add = nothing;
            self.smart_dirty = true;
        }
    }

    /// What endless play and the Auto-DJ did to the queue, for its snapshot;
    /// forgets the added entries that left the queue.
    pub(super) fn smart_queue(&mut self) -> SmartQueue {
        self.smart_dirty = false;
        let added: Vec<EntryId> = if self.added.is_empty() {
            Vec::new()
        } else {
            self.queue
                .items()
                .iter()
                .map(|e| e.id)
                .filter(|id| self.added.contains(id))
                .collect()
        };
        if added.len() != self.added.len() {
            self.added = added.iter().copied().collect();
        }
        SmartQueue {
            added,
            radio_seed: self.endless_seed,
            nothing_to_add: self.nothing_to_add,
        }
    }

    /// Notes what started: the recent tracks, Radio Random's jump into the track,
    /// and the Auto-DJ's top-up.
    pub(super) fn started_smart(&mut self, slot: &Slot, length: Option<MediaTime>) {
        if let Some(track) = slot.library_track {
            self.recent.push_back(track);
            if self.recent.len() > RECENT_LIMIT {
                self.recent.pop_front();
            }
        }
        if let Some((_, share)) = self.radio_seek.take_if(|(entry, _)| *entry == slot.entry)
            && let Some(length) = length
        {
            let to = length.as_duration().mul_f64(share);
            self.seek(MediaTime::from_duration(to));
        }
        if self.queue.modes().repeat == RepeatMode::AutoDj {
            self.continue_queue();
        }
    }

    /// Notes the playing entry for "Revert" as "Next" moves on from it.
    pub(super) fn remember_skip(&mut self) {
        if self.is_stopped() {
            return;
        }
        if let Some(slot) = &self.current {
            self.skipped.push((slot.entry, self.position));
            if self.skipped.len() > REMEMBERED_SKIPS {
                self.skipped.remove(0);
            }
        }
    }

    /// Whether "Revert" has an entry still in the queue to go back to.
    pub(super) fn can_revert(&self) -> bool {
        self.skipped
            .iter()
            .any(|(entry, _)| self.queue.index_of(*entry).is_some())
    }

    /// "Revert": back to the entry "Next" last moved on from, where it was.
    pub(super) fn revert(&mut self) {
        while let Some((entry, at)) = self.skipped.pop() {
            if self.queue.index_of(entry).is_none() {
                continue;
            }
            let old = self.current.as_ref().map(|s| s.entry);
            if self.queue.jump_to(entry) {
                self.unplayable.remove(&entry);
                self.load(entry, Direction::Forward, Some(at));
                self.consume(old);
            }
            return;
        }
    }

    /// "Radio Random": a random entry other than the current one, from a point
    /// between a tenth and seven tenths of the way in.
    pub(super) fn radio_random(&mut self) {
        let skip = self.skipper();
        let current = self.queue.current().map(|e| e.id);
        let choices: Vec<EntryId> = self
            .queue
            .items()
            .iter()
            .filter(|e| Some(e.id) != current && !skip.skips(e) && !skip.suspects.contains(&e.id))
            .map(|e| e.id)
            .collect();
        if choices.is_empty() {
            return;
        }
        let entry = choices[self.draws.below(choices.len())];
        let share = 0.1 + 0.6 * self.draws.unit();
        let old = self.current.as_ref().map(|s| s.entry);
        if self.queue.jump_to(entry) {
            self.radio_seek = Some((entry, share));
            self.load(entry, Direction::Forward, None);
            self.consume(old);
        }
    }

    /// Intro scan: moves on once the playing entry has played its first seconds,
    /// of listening time: at another speed they cover more or less of the track.
    pub(super) fn check_intro_scan(&mut self, position: MediaTime) {
        let scan = Duration::from_secs(self.smart.intro_scan_s.into())
            .mul_f64(self.speed().effective_rate());
        if self.queue.modes().intro_scan
            && !self.exhausted
            && self.in_flight.is_none()
            && self.pending.is_none()
            && position.as_duration() >= scan
        {
            self.next_from(false);
        }
    }

    /// Starts endless play from `seed`.
    pub(super) fn start_radio(&mut self, seed: PlaySeed) {
        let Some(library) = self.library.clone() else {
            self.emit(PlayerEvent::Notice(Notice::ContextUnavailable {
                reason: "there is no library".into(),
            }));
            return;
        };
        let started = match seed {
            PlaySeed::Album(album) => self
                .resolve_context(&ContextRequest::Album(album), None)
                .map(|(context, tracks)| (context, tracks.into_iter().collect::<Vec<_>>())),
            PlaySeed::Track(_) | PlaySeed::Artist(_) => {
                let first: Vec<TrackId> = match seed {
                    PlaySeed::Track(track) => vec![track],
                    _ => Vec::new(),
                };
                let settings = self.smart.endless.clone();
                let avoid: HashSet<TrackId> = first.iter().copied().collect();
                let draw = self.draws.next_seed();
                let sound = self.sound_similarity();
                library
                    .related_tracks(
                        seed,
                        &settings,
                        &avoid,
                        settings.batch as usize,
                        draw,
                        sound.as_ref(),
                    )
                    .map_err(|e| e.to_string())
                    .map(|related| {
                        let context = match seed {
                            PlaySeed::Artist(person) => {
                                QueueContext::new(helikodu_model::player::ContextSource::Artist(
                                    u64::try_from(person.get()).unwrap_or(0),
                                ))
                            }
                            _ => QueueContext::files(),
                        };
                        let tracks = first
                            .into_iter()
                            .chain(related)
                            .map(TrackRef::Library)
                            .collect();
                        (context, tracks)
                    })
            }
        };
        match started {
            Ok((context, tracks)) if !tracks.is_empty() => {
                if self.queue.modes().repeat != RepeatMode::Endless {
                    self.queue.set_repeat(RepeatMode::Endless);
                    self.emit(PlayerEvent::ModesChanged(self.queue.modes()));
                }
                let before: HashSet<EntryId> = self.queue.items().iter().map(|e| e.id).collect();
                self.play_tracks(context, tracks, 0);
                self.endless_seed = Some(seed);
                // The related tracks were added by the radio; the seed track and
                // an album's own tracks were not.
                if !matches!(seed, PlaySeed::Album(_)) {
                    let seed_track = match seed {
                        PlaySeed::Track(track) => Some(TrackRef::Library(track)),
                        _ => None,
                    };
                    let related: Vec<EntryId> = self
                        .queue
                        .items()
                        .iter()
                        .filter(|e| {
                            !before.contains(&e.id) && Some(&e.track) != seed_track.as_ref()
                        })
                        .map(|e| e.id)
                        .collect();
                    self.added.extend(related);
                }
                self.smart_dirty = true;
            }
            Ok(_) => self.emit(PlayerEvent::Notice(Notice::ContextUnavailable {
                reason: "nothing in the library is related to it".into(),
            })),
            Err(reason) => self.emit(PlayerEvent::Notice(Notice::ContextUnavailable { reason })),
        }
    }
}
