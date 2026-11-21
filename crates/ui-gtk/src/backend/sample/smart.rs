// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Endless play, the Auto-DJ and the radio of the sample player: it adds
//! tracks of other albums, marks them as added by Helikodu, and keeps the
//! queue a radio replaced for its Undo.

use std::collections::HashSet;
use std::time::Duration;

use super::super::{
    LibrarySource, PlaybackState, RadioSeed, RepeatMode, TrackKey, TrackQuery, TrackRow, TrackScope,
};
use super::FakePlayer;

/// How many tracks the sample's endless play adds at a time, and how many the
/// sample's Auto-DJ keeps ahead.
const BATCH: usize = 5;

/// The queue a radio replaced.
#[derive(Debug, Clone)]
pub(super) struct BeforeRadio {
    queue: Vec<TrackRow>,
    context: Option<String>,
    current: Option<usize>,
    elapsed: Duration,
    state: PlaybackState,
    repeat: RepeatMode,
    added: Vec<u64>,
}

impl FakePlayer {
    /// Up to `count` library tracks not in the queue, from albums other than
    /// the playing one, in library order.
    fn related(&self, count: usize, exclude: &HashSet<TrackKey>) -> Vec<TrackKey> {
        let all = self
            .library
            .track_keys(&TrackQuery::default())
            .unwrap_or_default();
        let state = self.state.borrow();
        let playing_album = state
            .current
            .and_then(|i| state.queue.get(i))
            .map(|t| t.album);
        let queued: HashSet<TrackKey> = state.queue.iter().map(|t| t.key).collect();
        drop(state);
        all.into_iter()
            .filter(|k| !queued.contains(k) && !exclude.contains(k))
            .filter(|k| {
                self.library
                    .track(*k)
                    .is_some_and(|t| Some(t.album) != playing_album)
            })
            // Every seventh track, so the added ones come from several albums.
            .step_by(7)
            .take(count)
            .collect()
    }

    /// Adds `keys` to the end of the queue, marked as added by Helikodu.
    fn add_marked(&self, keys: &[TrackKey]) {
        let mut state = self.state.borrow_mut();
        let first = state.next_entry + 1;
        let mut rows = self.library.queue_rows(keys, first);
        state.next_entry += rows.len() as u64;
        for row in &mut rows {
            row.from_context = false;
        }
        state.added.extend(rows.iter().filter_map(|r| r.entry));
        state.nothing_to_add = rows.is_empty();
        state.queue.extend(rows);
    }

    /// After the Repeat mode changed: endless play adds tracks when nothing is
    /// left to play, the Auto-DJ tops the queue up.
    pub(super) fn continue_queue(&self) {
        let (repeat, upcoming) = {
            let state = self.state.borrow();
            let from = state.current.map_or(0, |c| c + 1);
            (state.modes.repeat, state.queue.len().saturating_sub(from))
        };
        let wanted = match repeat {
            RepeatMode::Endless if upcoming == 0 => BATCH,
            RepeatMode::AutoDj if upcoming < BATCH => BATCH - upcoming,
            RepeatMode::Endless | RepeatMode::AutoDj => return,
            _ => {
                self.state.borrow_mut().nothing_to_add = false;
                return;
            }
        };
        let keys = self.related(wanted, &HashSet::new());
        self.add_marked(&keys);
    }

    /// Starts a radio from `seed`: the queue is replaced by the seed and
    /// related tracks, and endless play goes on from it.
    pub(super) fn start_radio(&self, seed: RadioSeed) {
        let (keys, related_from) = match seed {
            RadioSeed::Track(id) => (vec![TrackKey(id)], 1),
            RadioSeed::Album(id) => {
                let keys = self
                    .library
                    .detail(super::super::AlbumKey(u64::try_from(id).unwrap_or(0)))
                    .map(|d| d.tracks().map(|t| t.key).collect())
                    .unwrap_or_default();
                (keys, usize::MAX)
            }
            RadioSeed::Artist(id) => {
                let keys = self
                    .library
                    .track_keys(&TrackQuery {
                        scope: TrackScope::Artist(super::super::ArtistKey(
                            u64::try_from(id).unwrap_or(0),
                        )),
                        ..TrackQuery::default()
                    })
                    .unwrap_or_default();
                (keys.into_iter().take(BATCH + 1).collect(), 0)
            }
        };
        if keys.is_empty() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.before_radio = Some(Box::new(BeforeRadio {
                queue: state.queue.clone(),
                context: state.context.clone(),
                current: state.current,
                elapsed: state.elapsed,
                state: state.state,
                repeat: state.modes.repeat,
                added: state.added.clone(),
            }));
            let first = state.next_entry + 1;
            state.queue = self.library.queue_rows(&keys, first);
            state.next_entry += state.queue.len() as u64;
            state.added = state
                .queue
                .iter()
                .skip(related_from)
                .filter_map(|r| r.entry)
                .collect();
            state.context = None;
            state.current = Some(0);
            state.state = PlaybackState::Playing;
            state.elapsed = Duration::ZERO;
            state.radio = Some(seed);
            state.nothing_to_add = false;
        }
        if related_from == 1 {
            let exclude: HashSet<TrackKey> = keys.iter().copied().collect();
            let more = self.related(BATCH, &exclude);
            self.add_marked(&more);
        }
        self.change_modes(|m| m.repeat = RepeatMode::Endless);
        self.announce();
    }

    /// Puts back what the last radio replaced.
    pub(super) fn undo_radio(&self) {
        let repeat = {
            let mut state = self.state.borrow_mut();
            let Some(before) = state.before_radio.take() else {
                return;
            };
            state.queue = before.queue;
            state.context = before.context;
            state.current = before.current;
            state.elapsed = before.elapsed;
            state.state = before.state;
            state.added = before.added;
            state.radio = None;
            state.nothing_to_add = false;
            before.repeat
        };
        self.change_modes(|m| m.repeat = repeat);
        self.announce();
    }

    /// Forgets what a new queue no longer has: the radio, its Undo and the
    /// marks.
    pub(super) fn forget_smart(&self) {
        let mut state = self.state.borrow_mut();
        state.radio = None;
        state.before_radio = None;
        state.nothing_to_add = false;
        let entries: HashSet<u64> = state.queue.iter().filter_map(|t| t.entry).collect();
        state.added.retain(|e| entries.contains(e));
    }
}
