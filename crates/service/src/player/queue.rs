// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The play queue: entries in the order they play, the current one, and the order
//! of the context they came from.
//!
//! - `items` is the **play order**, the list the queue panel shows: the entries
//!   before the current one are its history, those after it are upcoming.
//! - `linear` is the **context order** of the same entries. Without shuffle the two
//!   are the same. With shuffle the upcoming part of `items` is a stored random
//!   permutation, so "Previous" walks back the order actually played; turning
//!   shuffle off puts `items` back into context order and keeps the current entry.
//! - Album, folder and grouping shuffles permute **blocks**: the entries that share
//!   the key ([`EntryKeys`]) stay together in context order, and the block of the
//!   current entry plays to its end first. Shuffling within albums keeps the albums
//!   in context order and permutes the tracks of each.
//! - With shuffle and repeat-all, the order of the next cycle is drawn when it is
//!   first needed (at the end of the current one, to name the next track early for
//!   a gapless join), stored, and applied when its first entry starts.
//! - Repeat-album wraps from the last entry of the current album, in play order, to
//!   its first one.
//! - Operations change the queue's version, which front ends compare to find out
//!   whether to fetch it again.

use std::collections::{HashMap, HashSet};

use helikodu_model::library::TrackRef;
use helikodu_model::player::{
    EnqueueAt, EntryId, PlayModes, QueueContext, QueueEntry, RepeatMode, ShuffleMode, StopAfter,
    TrackFlags,
};
use helikodu_model::settings::{PlayFacts, PlayWeight, play_weight};

use super::rng::Rng;

/// Whether a move to the next entry is the player's own (a track ended) or the
/// user's ("Next"). Repeat-one only holds the player on the same entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Advance {
    Auto,
    User,
}

/// What the play modes know of an entry's track: the blocks it belongs to, and its
/// per-track flags. Keys are opaque strings; entries with equal keys share the
/// block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EntryKeys {
    pub(crate) album: Option<String>,
    pub(crate) folder: Option<String>,
    pub(crate) grouping: Option<String>,
    pub(crate) artist: Option<String>,
    pub(crate) flags: TrackFlags,
    /// The statistics a weighted shuffle weighs.
    pub(crate) play: PlayFacts,
    /// Passed over while shuffling: rated below `playback.shuffle.min_rating` or
    /// found by `playback.shuffle.skip_query`.
    pub(crate) shuffle_skip: bool,
}

impl EntryKeys {
    /// What is known of `track` without the library: the folder of a file, which
    /// also stands for its album.
    pub(crate) fn of_track(track: &TrackRef) -> Self {
        match track {
            TrackRef::File(source) => {
                let folder = source.path.parent().map(|dir| format!("folder:{dir:?}"));
                Self {
                    album: folder.clone(),
                    folder,
                    ..Self::default()
                }
            }
            TrackRef::Library(_) => Self::default(),
        }
    }

    /// The album: the track's album, or its folder when it has none.
    pub(crate) fn album_key(&self) -> Option<&str> {
        self.album.as_deref().or(self.folder.as_deref())
    }

    /// The block of `mode` the track belongs to; `None` makes it a block of its own.
    fn block(&self, mode: ShuffleMode) -> Option<&str> {
        match mode {
            ShuffleMode::Albums | ShuffleMode::WithinAlbum => self.album_key(),
            ShuffleMode::Folders => self.folder.as_deref(),
            ShuffleMode::Groupings => self.grouping.as_deref(),
            _ => None,
        }
    }
}

/// A track with its keys, as the queue takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Keyed {
    pub(crate) track: TrackRef,
    pub(crate) keys: EntryKeys,
}

impl From<TrackRef> for Keyed {
    fn from(track: TrackRef) -> Self {
        let keys = EntryKeys::of_track(&track);
        Self { track, keys }
    }
}

/// No keys known.
static NO_KEYS: EntryKeys = EntryKeys {
    album: None,
    folder: None,
    grouping: None,
    artist: None,
    flags: TrackFlags {
        skip_in_shuffle: false,
        manual_only: false,
        volume_half_db: 0,
    },
    play: PlayFacts {
        rating: None,
        plays: 0,
        days_since_played: None,
    },
    shuffle_skip: false,
};

#[derive(Debug, Clone)]
pub(crate) struct Queue {
    items: Vec<QueueEntry>,
    current: Option<usize>,
    linear: Vec<EntryId>,
    keys: HashMap<EntryId, EntryKeys>,
    context: QueueContext,
    modes: PlayModes,
    /// The play order of the next shuffled cycle, once drawn.
    next_cycle: Option<Vec<EntryId>>,
    last_id: u64,
    rng: Rng,
    version: u64,
    /// What shuffling by tracks favours (`playback.shuffle.weight_by`).
    weights: Vec<PlayWeight>,
}

impl Queue {
    pub(crate) fn new(rng: Rng) -> Self {
        Self {
            items: Vec::new(),
            current: None,
            linear: Vec::new(),
            keys: HashMap::new(),
            context: QueueContext::files(),
            modes: PlayModes::default(),
            next_cycle: None,
            last_id: 0,
            rng,
            version: 0,
            weights: Vec::new(),
        }
    }

    /// A queue restored from a saved session. Entries keep their identifiers; the
    /// next new one follows the largest. Inconsistent parts (a context order that
    /// does not match the entries, a current index out of range) are repaired. The
    /// keys are those known without the library until [`rekey`](Self::rekey).
    pub(crate) fn restore(
        rng: Rng,
        context: QueueContext,
        items: Vec<QueueEntry>,
        linear: Vec<EntryId>,
        current: Option<usize>,
        modes: PlayModes,
    ) -> Self {
        let mut seen = HashSet::new();
        let items: Vec<QueueEntry> = items.into_iter().filter(|e| seen.insert(e.id)).collect();
        let mut linear_ok = linear.len() == items.len();
        if linear_ok {
            let ids: HashSet<EntryId> = items.iter().map(|e| e.id).collect();
            linear_ok = linear.iter().all(|id| ids.contains(id))
                && linear.iter().collect::<HashSet<_>>().len() == linear.len();
        }
        let linear = if linear_ok {
            linear
        } else {
            items.iter().map(|e| e.id).collect()
        };
        let last_id = items.iter().map(|e| e.id.get()).max().unwrap_or(0);
        let current = current.filter(|&i| i < items.len());
        let keys = items
            .iter()
            .map(|e| (e.id, EntryKeys::of_track(&e.track)))
            .collect();
        Self {
            items,
            current,
            linear,
            keys,
            context,
            modes,
            next_cycle: None,
            last_id,
            rng,
            version: 1,
            weights: Vec::new(),
        }
    }

    /// Puts `saved`, an earlier copy of this queue, back exactly as it was: its
    /// entries with their identifiers, the play and context orders (a shuffled
    /// order included), the current entry, the context, the modes and the next
    /// shuffled cycle. The version moves on, entries added later get identifiers
    /// not given out before, and the shuffle weights stay as they are now.
    pub(crate) fn put_back(&mut self, mut saved: Queue) {
        saved.version = self.version + 1;
        saved.last_id = saved.last_id.max(self.last_id);
        if saved.weights != self.weights {
            saved.next_cycle = None;
        }
        saved.weights = std::mem::take(&mut self.weights);
        *self = saved;
    }

    // --- Reading ----------------------------------------------------------------

    pub(crate) fn items(&self) -> &[QueueEntry] {
        &self.items
    }

    pub(crate) fn linear(&self) -> &[EntryId] {
        &self.linear
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub(crate) fn current(&self) -> Option<&QueueEntry> {
        self.items.get(self.current?)
    }

    pub(crate) fn context(&self) -> &QueueContext {
        &self.context
    }

    pub(crate) fn modes(&self) -> PlayModes {
        self.modes
    }

    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    pub(crate) fn index_of(&self, id: EntryId) -> Option<usize> {
        self.items.iter().position(|e| e.id == id)
    }

    pub(crate) fn entry(&self, id: EntryId) -> Option<&QueueEntry> {
        self.items.iter().find(|e| e.id == id)
    }

    /// The keys of the entry `id`; none known for an entry that is not there.
    pub(crate) fn keys(&self, id: EntryId) -> &EntryKeys {
        self.keys.get(&id).unwrap_or(&NO_KEYS)
    }

    /// The album key of the entry `id`.
    pub(crate) fn album_of(&self, id: EntryId) -> Option<&str> {
        self.keys(id).album_key()
    }

    /// The entries before and after `id` in play order, if there are.
    pub(crate) fn neighbours(&self, id: EntryId) -> [Option<&QueueEntry>; 2] {
        let Some(at) = self.index_of(id) else {
            return [None, None];
        };
        [
            at.checked_sub(1).and_then(|i| self.items.get(i)),
            self.items.get(at + 1),
        ]
    }

    // --- Keys -------------------------------------------------------------------

    /// Replaces the keys of every entry with what `keys_of` says of their tracks.
    /// The play order is kept.
    pub(crate) fn rekey(&mut self, keys_of: impl FnOnce(&[QueueEntry]) -> Vec<EntryKeys>) {
        let keys = keys_of(&self.items);
        self.keys = self.items.iter().map(|e| e.id).zip(keys).collect();
    }

    /// Sets what shuffling by tracks favours. The order already drawn is kept; the
    /// next one drawn follows the weights.
    pub(crate) fn set_weights(&mut self, weights: Vec<PlayWeight>) {
        if weights != self.weights {
            self.weights = weights;
            self.next_cycle = None;
        }
    }

    /// Orders `ids` at random for shuffling by tracks: evenly, or with the favoured
    /// tracks more likely to come early when weights are set.
    fn shuffle_tracks(&mut self, ids: &mut Vec<EntryId>) {
        if self.weights.is_empty() {
            self.rng.shuffle(ids);
            return;
        }
        let weights: HashMap<EntryId, f64> = ids
            .iter()
            .map(|id| (*id, play_weight(&self.weights, &self.keys(*id).play)))
            .collect();
        self.rng
            .weighted_order(ids, |id| weights.get(id).copied().unwrap_or(1.0));
    }

    /// Sets the flags of every entry of `track`.
    pub(crate) fn set_flags(&mut self, track: &TrackRef, flags: TrackFlags) {
        for entry in self.items.iter().filter(|e| e.track == *track) {
            self.keys.entry(entry.id).or_default().flags = flags;
        }
        self.next_cycle = None;
    }

    // --- Moving through it ------------------------------------------------------

    /// The entry that plays after the current one: the next in play order, the same
    /// one under repeat-one for [`Advance::Auto`], the first of the album under
    /// repeat-album after its last, the first of the next cycle under repeat-all.
    /// Entries for which `skip` is true are passed over (but not under repeat-one,
    /// which names the current entry). `None` at the end of the queue.
    pub(crate) fn peek_next(
        &mut self,
        advance: Advance,
        skip: impl Fn(&QueueEntry) -> bool,
    ) -> Option<EntryId> {
        let Some(current) = self.current else {
            return self.items.iter().find(|e| !skip(e)).map(|e| e.id);
        };
        if advance == Advance::Auto && self.modes.repeat == RepeatMode::One {
            return Some(self.items[current].id);
        }
        if self.modes.repeat == RepeatMode::Album {
            let album = self.album_of(self.items[current].id);
            let same = |e: &&QueueEntry| self.album_of(e.id) == album && !skip(e);
            return self.items[current + 1..]
                .iter()
                .find(same)
                .or_else(|| self.items[..=current].iter().find(same))
                .map(|e| e.id);
        }
        if let Some(entry) = self.items[current + 1..].iter().find(|e| !skip(e)) {
            return Some(entry.id);
        }
        if self.modes.repeat != RepeatMode::All {
            return None;
        }
        if self.modes.shuffle == ShuffleMode::Off {
            return self.items.iter().find(|e| !skip(e)).map(|e| e.id);
        }
        let current_id = self.items[current].id;
        let cycle = self.next_cycle(current_id);
        cycle.iter().copied().find(|id| {
            self.items
                .iter()
                .find(|e| e.id == *id)
                .is_some_and(|e| !skip(e))
        })
    }

    /// The order of the next shuffled cycle, drawn now if needed. Its first entry is
    /// not the one that just played, when there is another.
    fn next_cycle(&mut self, last: EntryId) -> Vec<EntryId> {
        if let Some(cycle) = &self.next_cycle {
            return cycle.clone();
        }
        let ids: Vec<EntryId> = self.items.iter().map(|e| e.id).collect();
        let mut blocks = self.arrange(ids, None);
        if blocks.first().and_then(|b| b.first()) == Some(&last) {
            if self.modes.shuffle == ShuffleMode::Tracks && blocks.len() > 1 {
                let swap = 1 + self.rng.below(blocks.len() - 1);
                blocks.swap(0, swap);
            } else if blocks.len() > 1 {
                blocks.rotate_left(1);
            } else if blocks[0].len() < 2 {
                // A single entry plays again.
            } else if self.modes.shuffle == ShuffleMode::WithinAlbum {
                blocks[0].swap(0, 1);
            } else {
                // One block: it plays on in its order from the entry after the last.
                blocks[0].rotate_left(1);
            }
        }
        let cycle: Vec<EntryId> = blocks.into_iter().flatten().collect();
        self.next_cycle = Some(cycle.clone());
        cycle
    }

    /// `ids` in the order the shuffle mode plays them, as blocks. With `lead`, the
    /// entry before them, the rest of its block comes first: for block shuffles from
    /// the entry after it in context order round to the one before it.
    fn arrange(&mut self, mut ids: Vec<EntryId>, lead: Option<EntryId>) -> Vec<Vec<EntryId>> {
        let mode = self.modes.shuffle;
        if mode == ShuffleMode::Tracks || mode == ShuffleMode::Off {
            if mode == ShuffleMode::Tracks {
                self.shuffle_tracks(&mut ids);
            }
            return ids.into_iter().map(|id| vec![id]).collect();
        }
        let position: HashMap<EntryId, usize> = self
            .linear
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        let pos = |id: &EntryId| position.get(id).copied().unwrap_or(usize::MAX);
        ids.sort_by_key(pos);
        let mut blocks: Vec<Vec<EntryId>> = Vec::new();
        let mut by_key: HashMap<&str, usize> = HashMap::new();
        for id in ids {
            match self.keys(id).block(mode) {
                Some(key) => match by_key.get(key) {
                    Some(&at) => blocks[at].push(id),
                    None => {
                        by_key.insert(key, blocks.len());
                        blocks.push(vec![id]);
                    }
                },
                None => blocks.push(vec![id]),
            }
        }
        let lead_pos = lead.map(|l| pos(&l));
        let lead_block = lead
            .and_then(|l| self.keys(l).block(mode))
            .and_then(|key| by_key.get(key).copied());
        let mut first = lead_block.map(|at| blocks.remove(at));
        let rotate = |block: &mut Vec<EntryId>| {
            if let Some(lead_pos) = lead_pos {
                let after = block.iter().position(|id| pos(id) > lead_pos);
                block.rotate_left(after.unwrap_or(0));
            }
        };
        if mode == ShuffleMode::WithinAlbum {
            for block in blocks.iter_mut().chain(first.iter_mut()) {
                self.rng.shuffle(block);
            }
            // The albums follow the current one in context order, round to the start.
            if let Some(lead_pos) = lead_pos {
                let after = blocks
                    .iter()
                    .position(|b| b.iter().map(pos).min().unwrap_or(0) > lead_pos);
                blocks.rotate_left(after.unwrap_or(0));
            }
        } else {
            if let Some(first) = &mut first {
                rotate(first);
            }
            self.rng.shuffle(&mut blocks);
        }
        first.into_iter().chain(blocks).collect()
    }

    /// Shuffles the upcoming entries for the shuffle mode.
    fn shuffle_upcoming(&mut self) {
        let from = self.current.map_or(0, |c| c + 1);
        if self.modes.shuffle == ShuffleMode::Tracks {
            if self.weights.is_empty() {
                self.rng.shuffle(&mut self.items[from..]);
            } else {
                let mut ids: Vec<EntryId> = self.items[from..].iter().map(|e| e.id).collect();
                self.shuffle_tracks(&mut ids);
                self.reorder_from(from, &ids);
            }
            return;
        }
        let lead = self.current().map(|e| e.id);
        let ids: Vec<EntryId> = self.items[from..].iter().map(|e| e.id).collect();
        let order: Vec<EntryId> = self.arrange(ids, lead).into_iter().flatten().collect();
        self.reorder_from(from, &order);
    }

    /// Puts the entries from index `from` on into `order`.
    fn reorder_from(&mut self, from: usize, order: &[EntryId]) {
        let mut by_id: HashMap<EntryId, QueueEntry> =
            self.items.drain(from..).map(|e| (e.id, e)).collect();
        self.items
            .extend(order.iter().filter_map(|id| by_id.remove(id)));
        // Anything the order missed keeps its place at the end.
        let mut rest: Vec<QueueEntry> = by_id.into_values().collect();
        rest.sort_by_key(|e| e.id);
        self.items.extend(rest);
    }

    /// Under a block shuffle, brings the upcoming entries of the current entry's
    /// block to right after it, so that the block plays on from there.
    fn gather_block(&mut self) {
        let mode = self.modes.shuffle;
        let Some(current) = self.current else {
            return;
        };
        let lead = self.items[current].id;
        let Some(key) = self.keys(lead).block(mode).map(str::to_owned) else {
            return;
        };
        let members: Vec<EntryId> = self.items[current + 1..]
            .iter()
            .filter(|e| self.keys(e.id).block(mode) == Some(key.as_str()))
            .map(|e| e.id)
            .collect();
        if members.is_empty() {
            return;
        }
        let order: Vec<EntryId> = self
            .arrange(members.clone(), Some(lead))
            .into_iter()
            .flatten()
            .collect();
        let others: Vec<EntryId> = self.items[current + 1..]
            .iter()
            .map(|e| e.id)
            .filter(|id| !members.contains(id))
            .collect();
        let all: Vec<EntryId> = order.into_iter().chain(others).collect();
        self.reorder_from(current + 1, &all);
    }

    /// The entry before the current one in play order; under repeat-all without
    /// shuffle the last entry before the first, under repeat-album the last entry of
    /// the album before its first. `None` at the start.
    pub(crate) fn peek_previous(&self, skip: impl Fn(&QueueEntry) -> bool) -> Option<EntryId> {
        let current = self.current?;
        if self.modes.repeat == RepeatMode::Album {
            let album = self.album_of(self.items[current].id);
            let same = |e: &&QueueEntry| self.album_of(e.id) == album && !skip(e);
            return self.items[..current]
                .iter()
                .rev()
                .find(same)
                .or_else(|| self.items[current + 1..].iter().rev().find(same))
                .map(|e| e.id);
        }
        if let Some(entry) = self.items[..current].iter().rev().find(|e| !skip(e)) {
            return Some(entry.id);
        }
        if self.modes.repeat == RepeatMode::All && self.modes.shuffle == ShuffleMode::Off {
            return self.items[current + 1..]
                .iter()
                .rev()
                .find(|e| !skip(e))
                .map(|e| e.id);
        }
        None
    }

    /// The first entry of the next album in play order: the first after the current
    /// one whose album differs; under repeat-all from the start of the queue when
    /// there is none after it.
    pub(crate) fn peek_next_album(&self, skip: impl Fn(&QueueEntry) -> bool) -> Option<EntryId> {
        let current = self.current?;
        let album = self.album_of(self.items[current].id);
        let other = |e: &&QueueEntry| self.album_of(e.id) != album && !skip(e);
        let found = self.items[current + 1..].iter().find(other);
        let found = match found {
            None if self.modes.repeat == RepeatMode::All => {
                self.items[..current].iter().find(other)
            }
            found => found,
        };
        found.map(|e| e.id)
    }

    /// The first entry of the album before the current one in play order, or of the
    /// current album when it is the first.
    pub(crate) fn peek_previous_album(
        &self,
        skip: impl Fn(&QueueEntry) -> bool,
    ) -> Option<EntryId> {
        let current = self.current?;
        let album_at = |i: usize| self.album_of(self.items[i].id);
        let run_start = |mut i: usize| {
            while i > 0 && album_at(i - 1) == album_at(i) {
                i -= 1;
            }
            i
        };
        let start = run_start(current);
        let (from, to) = if start == 0 {
            (0, current + 1)
        } else {
            (run_start(start - 1), start)
        };
        self.items[from..to]
            .iter()
            .find(|e| !skip(e))
            .map(|e| e.id)
            .filter(|id| *id != self.items[current].id || from != start)
    }

    /// The first entry, in play order, of an album of the queue chosen at random
    /// among those other than the current one.
    pub(crate) fn pick_random_album(
        &mut self,
        skip: impl Fn(&QueueEntry) -> bool,
    ) -> Option<EntryId> {
        let album = self
            .current()
            .and_then(|e| self.album_of(e.id))
            .map(str::to_owned);
        let mut seen = HashSet::new();
        let firsts: Vec<EntryId> = self
            .items
            .iter()
            .filter(|e| !skip(e))
            .filter(|e| {
                let key = self.album_of(e.id).map(str::to_owned);
                key != album && seen.insert(key)
            })
            .map(|e| e.id)
            .collect();
        if firsts.is_empty() {
            return None;
        }
        Some(firsts[self.rng.below(firsts.len())])
    }

    /// Makes `id` the current entry as playback reaches it by moving forward
    /// ([`peek_next`](Self::peek_next)): the first entry of a new shuffled cycle
    /// starts that cycle.
    pub(crate) fn advance_to(&mut self, id: EntryId) -> bool {
        let at_end = self.current.is_some_and(|c| c + 1 == self.items.len());
        if at_end
            && let Some(cycle) = self.next_cycle.take()
            && cycle.contains(&id)
        {
            self.reorder_from(0, &cycle);
            self.current = self.index_of(id);
            self.version += 1;
            return true;
        }
        self.set_current(id)
    }

    /// Makes `id` the current entry, as "Previous" reaches it.
    pub(crate) fn set_current(&mut self, id: EntryId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        if self.current != Some(index) {
            self.current = Some(index);
            self.version += 1;
        }
        true
    }

    /// Makes `id` the current entry because the user chose it. With shuffle, the
    /// entry is moved to right after the current one first, so that the entries it
    /// jumps over still play in this cycle and it does not replay the history; under
    /// a block shuffle the rest of its block follows it.
    pub(crate) fn jump_to(&mut self, id: EntryId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        if self.modes.shuffle != ShuffleMode::Off
            && let Some(current) = self.current
            && index != current
        {
            let entry = self.items.remove(index);
            let to = if index < current {
                current
            } else {
                current + 1
            };
            self.items.insert(to, entry);
            self.current = Some(to);
            self.gather_block();
            self.next_cycle = None;
            self.version += 1;
            return true;
        }
        self.set_current(id)
    }

    // --- Changing it ------------------------------------------------------------

    fn new_entries<K: Into<Keyed>>(
        &mut self,
        tracks: impl IntoIterator<Item = K>,
        from_context: bool,
    ) -> Vec<QueueEntry> {
        tracks
            .into_iter()
            .map(|track| {
                let Keyed { track, keys } = track.into();
                self.last_id += 1;
                let id = EntryId::new(self.last_id);
                self.keys.insert(id, keys);
                QueueEntry {
                    id,
                    track,
                    from_context,
                }
            })
            .collect()
    }

    /// Replaces the queue with `tracks` from `context`; the entry at `start` becomes
    /// current (none if `start` is out of range). With shuffle, the start entry
    /// comes first and the rest follow in the shuffle mode's order.
    pub(crate) fn replace<K: Into<Keyed>>(
        &mut self,
        context: QueueContext,
        tracks: impl IntoIterator<Item = K>,
        start: usize,
    ) -> Option<EntryId> {
        self.keys.clear();
        let entries = self.new_entries(tracks, true);
        self.linear = entries.iter().map(|e| e.id).collect();
        self.items = entries;
        self.context = context;
        self.next_cycle = None;
        self.version += 1;
        if start >= self.items.len() {
            self.current = None;
            if self.modes.shuffle != ShuffleMode::Off {
                self.shuffle_upcoming();
            }
            return None;
        }
        if self.modes.shuffle != ShuffleMode::Off {
            let entry = self.items.remove(start);
            self.items.insert(0, entry);
            self.current = Some(0);
            self.shuffle_upcoming();
        } else {
            self.current = Some(start);
        }
        self.current().map(|e| e.id)
    }

    /// Adds `tracks` from `context` to the end and makes `context` the queue's
    /// context. Returns the first new entry.
    pub(crate) fn append_context<K: Into<Keyed>>(
        &mut self,
        context: QueueContext,
        tracks: impl IntoIterator<Item = K>,
        start: usize,
    ) -> Option<EntryId> {
        let entries = self.new_entries(tracks, true);
        let first = entries.get(start).map(|e| e.id);
        self.linear.extend(entries.iter().map(|e| e.id));
        self.items.extend(entries);
        self.context = context;
        self.next_cycle = None;
        self.version += 1;
        first
    }

    /// Adds `tracks`, queued from elsewhere, after the current entry or at the end.
    /// With track shuffle, "at the end" is a random place among the upcoming
    /// entries; with a block shuffle it is the end, so that no block is split.
    pub(crate) fn enqueue<K: Into<Keyed>>(
        &mut self,
        tracks: impl IntoIterator<Item = K>,
        at: EnqueueAt,
    ) -> Vec<EntryId> {
        let entries = self.new_entries(tracks, false);
        let ids: Vec<EntryId> = entries.iter().map(|e| e.id).collect();
        if ids.is_empty() {
            return ids;
        }
        let upcoming = self.current.map_or(0, |c| c + 1);
        match at {
            EnqueueAt::Next => {
                let linear_at = self
                    .current()
                    .and_then(|c| self.linear.iter().position(|id| *id == c.id))
                    .map_or(0, |i| i + 1);
                self.linear
                    .splice(linear_at..linear_at, ids.iter().copied());
                self.items.splice(upcoming..upcoming, entries);
            }
            EnqueueAt::End => {
                self.linear.extend(ids.iter().copied());
                if self.modes.shuffle == ShuffleMode::Tracks {
                    for entry in entries {
                        let at = upcoming + self.rng.below(self.items.len() - upcoming + 1);
                        self.items.insert(at, entry);
                    }
                } else {
                    self.items.extend(entries);
                }
            }
        }
        self.next_cycle = None;
        self.version += 1;
        ids
    }

    /// Removes the entries `ids`. When the current one goes, the entry that takes its
    /// place becomes current (the one after it, or the one before at the end).
    /// Returns whether the current entry was removed.
    pub(crate) fn remove(&mut self, ids: &[EntryId]) -> bool {
        if !self.items.iter().any(|e| ids.contains(&e.id)) {
            return false;
        }
        let old_current = self.current;
        let removed_current = self.current().is_some_and(|e| ids.contains(&e.id));
        let removed_before = old_current.map_or(0, |c| {
            self.items[..c]
                .iter()
                .filter(|e| ids.contains(&e.id))
                .count()
        });
        self.items.retain(|e| !ids.contains(&e.id));
        self.linear.retain(|id| !ids.contains(id));
        for id in ids {
            self.keys.remove(id);
        }
        // After the removal, the entries before the current one moved down; when the
        // current one went, its index now holds the first entry that followed it.
        self.current = match old_current {
            _ if self.items.is_empty() => None,
            Some(c) => Some((c - removed_before).min(self.items.len() - 1)),
            None => None,
        };
        self.next_cycle = None;
        self.version += 1;
        removed_current
    }

    /// Removes the entry `id`, which playback has moved on from, without touching
    /// the current entry ("consume"). Returns whether it was there.
    pub(crate) fn consume(&mut self, id: EntryId) -> bool {
        if self.current().is_some_and(|e| e.id == id) {
            return false;
        }
        let there = self.index_of(id).is_some();
        if there {
            self.remove(&[id]);
        }
        there
    }

    /// Moves the entry `id` to `to` in play order (an index into the queue without
    /// the entry, clamped to its end). Without shuffle the context order follows.
    pub(crate) fn move_entry(&mut self, id: EntryId, to: usize) -> bool {
        let Some(from) = self.index_of(id) else {
            return false;
        };
        let current_id = self.current().map(|e| e.id);
        let entry = self.items.remove(from);
        let to = to.min(self.items.len());
        self.items.insert(to, entry);
        self.current = current_id.and_then(|c| self.index_of(c));
        if self.modes.shuffle == ShuffleMode::Off {
            self.linear = self.items.iter().map(|e| e.id).collect();
        }
        self.next_cycle = None;
        self.version += 1;
        true
    }

    /// Empties the queue, keeping the current entry when `keep_current` is true.
    pub(crate) fn clear(&mut self, keep_current: bool) {
        let kept = if keep_current {
            self.current().cloned()
        } else {
            None
        };
        self.linear = kept.iter().map(|e| e.id).collect();
        self.current = kept.as_ref().map(|_| 0);
        self.keys
            .retain(|id, _| kept.as_ref().is_some_and(|e| e.id == *id));
        self.items = kept.into_iter().collect();
        self.next_cycle = None;
        self.version += 1;
    }

    pub(crate) fn set_shuffle(&mut self, shuffle: ShuffleMode) {
        if shuffle == self.modes.shuffle {
            return;
        }
        self.modes.shuffle = shuffle;
        self.next_cycle = None;
        let current_id = self.current().map(|e| e.id);
        if shuffle == ShuffleMode::Off {
            let linear = self.linear.clone();
            self.reorder_from(0, &linear);
            self.current = current_id.and_then(|c| self.index_of(c));
        } else {
            self.shuffle_upcoming();
        }
        self.version += 1;
    }

    pub(crate) fn set_repeat(&mut self, repeat: RepeatMode) {
        if repeat != self.modes.repeat {
            self.modes.repeat = repeat;
            self.next_cycle = None;
            self.version += 1;
        }
    }

    /// The entry that plays after the current one when playback moves on by itself,
    /// with stop-after applied: `None` when playback is to stop after the current
    /// entry. After the queue, the next entry must come after the current one in
    /// play order (no wrap of repeat-all or repeat-album), though repeat-one still
    /// holds; after the album, it must also be of the current album.
    pub(crate) fn next_for(
        &mut self,
        stop_after: StopAfter,
        skip: impl Fn(&QueueEntry) -> bool,
    ) -> Option<EntryId> {
        let Some(current) = self.current else {
            return self.peek_next(Advance::Auto, skip);
        };
        let current_id = self.items[current].id;
        let next = match stop_after {
            StopAfter::Track => return None,
            _ => self.peek_next(Advance::Auto, skip)?,
        };
        let holds = next == current_id && self.modes.repeat == RepeatMode::One;
        let onwards = holds || self.index_of(next).is_some_and(|i| i > current);
        match stop_after {
            StopAfter::Album => {
                (onwards && self.album_of(next) == self.album_of(current_id)).then_some(next)
            }
            StopAfter::Queue => onwards.then_some(next),
            _ => Some(next),
        }
    }

    /// Sets the modes other than shuffle and repeat (consume, scope, cursor).
    pub(crate) fn set_other_modes(&mut self, modes: PlayModes) {
        let modes = PlayModes {
            shuffle: self.modes.shuffle,
            repeat: self.modes.repeat,
            ..modes
        };
        if modes != self.modes {
            self.modes = modes;
            self.next_cycle = None;
            self.version += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::Rng;
    use helikodu_model::library::TrackId;
    use proptest::prelude::{any, prop_assert, prop_assert_eq, prop_assert_ne, proptest};

    use super::*;

    fn tracks(n: u64) -> Vec<TrackRef> {
        (1..=n)
            .map(|i| TrackRef::Library(TrackId::new(i)))
            .collect()
    }

    fn queue(n: u64, start: usize) -> Queue {
        let mut queue = Queue::new(Rng::new(7));
        queue.replace(QueueContext::files(), tracks(n), start);
        queue
    }

    fn track_numbers(queue: &Queue) -> Vec<u64> {
        queue
            .items()
            .iter()
            .map(|e| match &e.track {
                TrackRef::Library(id) => id.get(),
                TrackRef::File(_) => 0,
            })
            .collect()
    }

    fn current_number(queue: &Queue) -> Option<u64> {
        queue.current().map(|e| match &e.track {
            TrackRef::Library(id) => id.get(),
            TrackRef::File(_) => 0,
        })
    }

    /// Plays on as the player does at the end of a track.
    fn step(queue: &mut Queue, advance: Advance) -> Option<u64> {
        let next = queue.peek_next(advance, |_| false)?;
        assert!(queue.advance_to(next));
        current_number(queue)
    }

    fn assert_consistent(queue: &Queue) {
        let ids: HashSet<EntryId> = queue.items().iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), queue.len(), "entry ids are unique");
        let linear: HashSet<EntryId> = queue.linear().iter().copied().collect();
        assert_eq!(linear, ids, "the context order holds the same entries");
        assert_eq!(queue.linear().len(), queue.len());
        if let Some(c) = queue.current_index() {
            assert!(c < queue.len());
        }
        if queue.modes().shuffle == ShuffleMode::Off {
            let order: Vec<EntryId> = queue.items().iter().map(|e| e.id).collect();
            assert_eq!(order, queue.linear(), "without shuffle the orders agree");
        }
    }

    #[test]
    fn plays_in_order_and_stops_at_the_end() {
        let mut queue = queue(3, 0);
        assert_eq!(current_number(&queue), Some(1));
        assert_eq!(step(&mut queue, Advance::Auto), Some(2));
        assert_eq!(step(&mut queue, Advance::Auto), Some(3));
        assert_eq!(queue.peek_next(Advance::Auto, |_| false), None);
        let previous = queue.peek_previous(|_| false).unwrap();
        queue.set_current(previous);
        assert_eq!(current_number(&queue), Some(2));
    }

    #[test]
    fn repeat_one_holds_only_the_player() {
        let mut queue = queue(3, 1);
        queue.set_repeat(RepeatMode::One);
        assert_eq!(step(&mut queue, Advance::Auto), Some(2));
        assert_eq!(step(&mut queue, Advance::User), Some(3));
        assert_eq!(queue.peek_next(Advance::User, |_| false), None);
    }

    #[test]
    fn repeat_all_wraps_both_ways() {
        let mut queue = queue(3, 2);
        queue.set_repeat(RepeatMode::All);
        assert_eq!(step(&mut queue, Advance::Auto), Some(1));
        let previous = queue.peek_previous(|_| false).unwrap();
        queue.set_current(previous);
        assert_eq!(current_number(&queue), Some(3));
    }

    #[test]
    fn skipped_entries_are_passed_over() {
        let mut queue = queue(4, 0);
        let skip = |e: &QueueEntry| e.track == TrackRef::Library(TrackId::new(2));
        let next = queue.peek_next(Advance::Auto, skip).unwrap();
        queue.advance_to(next);
        assert_eq!(current_number(&queue), Some(3));
        assert_eq!(
            queue
                .peek_previous(skip)
                .and_then(|id| queue.entry(id))
                .map(|e| e.track.clone()),
            Some(TrackRef::Library(TrackId::new(1)))
        );
    }

    #[test]
    fn shuffle_keeps_the_current_entry_and_off_restores_the_order() {
        let mut queue = queue(20, 4);
        queue.set_shuffle(ShuffleMode::Tracks);
        assert_eq!(current_number(&queue), Some(5));
        assert_eq!(queue.current_index(), Some(4));
        assert_eq!(&track_numbers(&queue)[..5], &[1, 2, 3, 4, 5]);
        assert_ne!(track_numbers(&queue), (1..=20).collect::<Vec<_>>());
        step(&mut queue, Advance::Auto);
        let playing = current_number(&queue);
        queue.set_shuffle(ShuffleMode::Off);
        assert_eq!(track_numbers(&queue), (1..=20).collect::<Vec<_>>());
        assert_eq!(current_number(&queue), playing);
        assert_consistent(&queue);
    }

    #[test]
    fn playing_a_context_with_shuffle_starts_with_the_chosen_track() {
        let mut queue = Queue::new(Rng::new(1));
        queue.set_shuffle(ShuffleMode::Tracks);
        queue.replace(QueueContext::files(), tracks(10), 6);
        assert_eq!(current_number(&queue), Some(7));
        assert_eq!(queue.current_index(), Some(0));
        assert_consistent(&queue);
    }

    #[test]
    fn enqueue_next_and_end() {
        let mut queue = queue(3, 0);
        let next = queue.enqueue(vec![TrackRef::Library(TrackId::new(9))], EnqueueAt::Next);
        queue.enqueue(vec![TrackRef::Library(TrackId::new(8))], EnqueueAt::End);
        assert_eq!(track_numbers(&queue), vec![1, 9, 2, 3, 8]);
        assert!(!queue.entry(next[0]).unwrap().from_context);
        assert_consistent(&queue);
    }

    #[test]
    fn removing_the_current_entry_moves_to_the_next() {
        let mut queue = queue(4, 1);
        let current = queue.current().unwrap().id;
        let first = queue.items()[0].id;
        assert!(queue.remove(&[first, current]));
        assert_eq!(track_numbers(&queue), vec![3, 4]);
        assert_eq!(current_number(&queue), Some(3));
        let last = queue.items()[1].id;
        queue.set_current(last);
        assert!(queue.remove(&[last]));
        assert_eq!(current_number(&queue), Some(3));
        let only = queue.items()[0].id;
        queue.remove(&[only]);
        assert_eq!(queue.current_index(), None);
        assert_consistent(&queue);
    }

    #[test]
    fn moving_keeps_the_current_entry() {
        let mut queue = queue(4, 1);
        let last = queue.items()[3].id;
        queue.move_entry(last, 0);
        assert_eq!(track_numbers(&queue), vec![4, 1, 2, 3]);
        assert_eq!(current_number(&queue), Some(2));
        assert_eq!(queue.current_index(), Some(2));
        assert_consistent(&queue);
    }

    #[test]
    fn clear_can_keep_the_current_entry() {
        let mut queue = queue(4, 2);
        queue.clear(true);
        assert_eq!(track_numbers(&queue), vec![3]);
        assert_eq!(queue.current_index(), Some(0));
        queue.clear(false);
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.current_index(), None);
    }

    #[test]
    fn a_shuffled_jump_keeps_the_rest_for_this_cycle() {
        let mut queue = Queue::new(Rng::new(3));
        queue.set_shuffle(ShuffleMode::Tracks);
        queue.replace(QueueContext::files(), tracks(6), 0);
        let far = queue.items()[4].id;
        queue.jump_to(far);
        assert_eq!(queue.current_index(), Some(1));
        let mut played: Vec<u64> = track_numbers(&queue)[..=1].to_vec();
        while let Some(n) = step(&mut queue, Advance::Auto) {
            played.push(n);
        }
        played.sort_unstable();
        assert_eq!(played, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn restore_repairs_what_does_not_fit() {
        let mut original = queue(3, 1);
        original.set_shuffle(ShuffleMode::Tracks);
        let restored = Queue::restore(
            Rng::new(1),
            original.context().clone(),
            original.items().to_vec(),
            vec![EntryId::new(99)],
            Some(7),
            original.modes(),
        );
        assert_eq!(restored.current_index(), None);
        assert_consistent(&Queue {
            modes: PlayModes::default(),
            ..restored.clone()
        });
        let mut restored = restored;
        let new = restored.enqueue(tracks(1), EnqueueAt::End);
        assert!(new[0].get() > 3, "new ids follow the restored ones");
    }

    /// Tracks 1 to `n` with keys: `albums` albums (in runs, or interleaved in context
    /// order), folders of three, groupings of four for two tracks in three, and two
    /// artists taking turns.
    fn keyed_tracks(n: u64, albums: u64, interleave: bool) -> Vec<Keyed> {
        let albums = albums.max(1);
        let size = n.div_ceil(albums).max(1);
        (1..=n)
            .map(|i| Keyed {
                track: TrackRef::Library(TrackId::new(i)),
                keys: EntryKeys {
                    album: Some(format!(
                        "a{}",
                        if interleave {
                            i % albums
                        } else {
                            (i - 1) / size
                        }
                    )),
                    folder: Some(format!("f{}", (i - 1) / 3)),
                    grouping: (i % 3 != 0).then(|| format!("g{}", (i - 1) / 4)),
                    artist: Some(format!("p{}", i % 2)),
                    ..EntryKeys::default()
                },
            })
            .collect()
    }

    #[test]
    fn weighted_shuffle_plays_the_favoured_early_and_everything_once() {
        let rated = |n: u64| -> Vec<Keyed> {
            (1..=n)
                .map(|i| Keyed {
                    track: TrackRef::Library(TrackId::new(i)),
                    keys: EntryKeys {
                        play: PlayFacts {
                            rating: Some(if i <= 10 { 10 } else { 0 }),
                            ..PlayFacts::default()
                        },
                        ..EntryKeys::default()
                    },
                })
                .collect()
        };
        let mut early = 0;
        for seed in 0..20 {
            let mut queue = Queue::new(Rng::new(seed));
            queue.set_weights(vec![PlayWeight::Rating]);
            queue.replace(QueueContext::files(), rated(100), 0);
            queue.set_shuffle(ShuffleMode::Tracks);
            let numbers = track_numbers(&queue);
            let mut sorted = numbers.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, (1..=100).collect::<Vec<_>>());
            early += numbers[1..21].iter().filter(|n| **n <= 10).count();
            assert_consistent(&queue);
        }
        // Ten favoured tracks in a hundred: an even shuffle puts two of them in
        // the first twenty places; the weights put most of them there.
        assert!(early > 20 * 6, "{early}");
        // The same seed draws the same order.
        let order = |seed| {
            let mut queue = Queue::new(Rng::new(seed));
            queue.set_weights(vec![PlayWeight::Rating]);
            queue.replace(QueueContext::files(), rated(30), 0);
            queue.set_shuffle(ShuffleMode::Tracks);
            track_numbers(&queue)
        };
        assert_eq!(order(3), order(3));
    }

    fn number_of(queue: &Queue, id: EntryId) -> u64 {
        match &queue.entry(id).unwrap().track {
            TrackRef::Library(track) => track.get(),
            TrackRef::File(_) => 0,
        }
    }

    /// The block key of track `number` under `mode`, as `keyed_tracks` made it.
    fn block_of(tracks: &[Keyed], mode: ShuffleMode, number: u64) -> Option<String> {
        let keys = &tracks[usize::try_from(number - 1).unwrap()].keys;
        keys.block(mode).map(str::to_owned)
    }

    const SHUFFLES: [ShuffleMode; 5] = [
        ShuffleMode::Tracks,
        ShuffleMode::Albums,
        ShuffleMode::Folders,
        ShuffleMode::Groupings,
        ShuffleMode::WithinAlbum,
    ];

    /// Every track of a cycle once; the entries of a block together; in later cycles
    /// a block shuffle keeps each block in context order, and shuffling within albums
    /// keeps the albums in context order.
    fn check_cycles(
        n: u64,
        start: usize,
        seed: u64,
        cycles: usize,
        mode: ShuffleMode,
        albums: u64,
        interleave: bool,
    ) {
        let tracks = keyed_tracks(n, albums, interleave);
        let mut queue = Queue::new(Rng::new(seed));
        queue.set_shuffle(mode);
        queue.set_repeat(RepeatMode::All);
        queue.replace(QueueContext::files(), tracks.clone(), start % n as usize);
        let mut played = vec![current_number(&queue).unwrap()];
        for _ in 1..(n as usize * cycles) {
            played.push(step(&mut queue, Advance::Auto).unwrap());
        }
        for (i, cycle) in played.chunks(n as usize).enumerate() {
            let mut sorted = cycle.to_vec();
            sorted.sort_unstable();
            assert_eq!(
                sorted,
                (1..=n).collect::<Vec<_>>(),
                "{mode:?} cycle {i}: {played:?}"
            );
            let keys: Vec<Option<String>> =
                cycle.iter().map(|t| block_of(&tracks, mode, *t)).collect();
            let mut closed = HashSet::new();
            for (k, key) in keys.iter().enumerate() {
                if let Some(key) = key
                    && k > 0
                    && keys[k - 1].as_ref() != Some(key)
                {
                    assert!(
                        closed.insert(key.clone()),
                        "{mode:?}: block {key} split in cycle {i}: {cycle:?}"
                    );
                }
                if k == 0
                    && let Some(key) = key
                {
                    closed.insert(key.clone());
                }
            }
            if i == 0 {
                continue;
            }
            match mode {
                ShuffleMode::Albums | ShuffleMode::Folders | ShuffleMode::Groupings => {
                    // In context order, or rotated once when the block is all there is.
                    let descents = cycle
                        .windows(2)
                        .filter(|pair| {
                            block_of(&tracks, mode, pair[0]).is_some()
                                && block_of(&tracks, mode, pair[0])
                                    == block_of(&tracks, mode, pair[1])
                                && pair[0] > pair[1]
                        })
                        .count();
                    let one_block = cycle
                        .iter()
                        .all(|t| block_of(&tracks, mode, *t) == block_of(&tracks, mode, cycle[0]));
                    assert!(
                        descents == 0 || (one_block && descents == 1),
                        "{mode:?}: {cycle:?}"
                    );
                }
                ShuffleMode::WithinAlbum => {
                    let mut order: Vec<u64> = Vec::new();
                    let mut seen = HashSet::new();
                    for t in cycle {
                        let key = block_of(&tracks, mode, *t);
                        if seen.insert(key.clone()) {
                            let first = (1..=n)
                                .find(|m| block_of(&tracks, mode, *m) == key)
                                .unwrap();
                            order.push(first);
                        }
                    }
                    let rotated_ok = order.windows(2).filter(|w| w[0] > w[1]).count() <= 1;
                    assert!(rotated_ok, "albums out of context order: {cycle:?}");
                }
                _ => {}
            }
        }
        if n > 1 {
            for boundary in (n as usize..played.len()).step_by(n as usize) {
                assert_ne!(played[boundary - 1], played[boundary]);
            }
        }
        assert_consistent(&queue);
    }

    /// Plays from `start` until playback stops (or for a while), as the player does
    /// when tracks end, and checks what stop-after, repeat and consume allow.
    #[allow(clippy::too_many_arguments)]
    fn check_combination(
        n: u64,
        start: usize,
        seed: u64,
        shuffle: ShuffleMode,
        repeat: RepeatMode,
        stop_after: StopAfter,
        consume: bool,
        albums: u64,
    ) {
        let tracks = keyed_tracks(n, albums, false);
        let mut queue = Queue::new(Rng::new(seed));
        queue.set_shuffle(shuffle);
        queue.set_repeat(repeat);
        queue.set_other_modes(PlayModes {
            consume,
            ..PlayModes::default()
        });
        queue.replace(QueueContext::files(), tracks, start % n as usize);
        let what = format!("{shuffle:?} {repeat:?} {stop_after:?} consume {consume}");
        let first = queue.current().unwrap().id;
        let first_album = queue.album_of(first).map(str::to_owned);
        let upcoming = queue.len() - queue.current_index().unwrap();
        let mut played = vec![current_number(&queue).unwrap()];
        let mut consumed = Vec::new();
        let mut stopped = false;
        for _ in 0..n * 3 {
            let old = queue.current().unwrap().id;
            let len = queue.len();
            let Some(next) = queue.next_for(stop_after, |_| false) else {
                stopped = true;
                break;
            };
            assert!(queue.advance_to(next), "{what}");
            if consume && next != old {
                assert!(queue.consume(old), "{what}");
                assert_eq!(queue.len(), len - 1, "{what}: consume removes");
                consumed.push(old);
            }
            played.push(current_number(&queue).unwrap());
            assert_consistent(&queue);
        }
        for id in &consumed {
            assert!(
                queue.index_of(*id).is_none(),
                "{what}: a consumed entry stays"
            );
        }
        let distinct = played.iter().collect::<HashSet<_>>().len() == played.len();
        match stop_after {
            StopAfter::Track => assert_eq!(played.len(), 1, "{what}"),
            StopAfter::Album => {
                let first_album = first_album.as_deref();
                for t in &played {
                    let album = format!("a{}", (t - 1) / n.div_ceil(albums.max(1)).max(1));
                    assert_eq!(Some(album.as_str()), first_album, "{what}: {played:?}");
                }
                if repeat == RepeatMode::One {
                    assert!(!stopped, "{what}");
                } else {
                    assert!(stopped && distinct, "{what}: {played:?}");
                }
            }
            StopAfter::Queue => {
                if repeat == RepeatMode::One {
                    assert!(!stopped, "{what}");
                } else {
                    assert!(stopped && distinct, "{what}: {played:?}");
                    assert!(played.len() <= upcoming, "{what}: {played:?}");
                }
            }
            _ => match repeat {
                // The queue alone ends under endless play and the Auto-DJ: the
                // player adds the tracks that follow.
                RepeatMode::Off | RepeatMode::Endless | RepeatMode::AutoDj => {
                    assert!(stopped && distinct, "{what}");
                    assert_eq!(played.len(), upcoming, "{what}: {played:?}");
                }
                RepeatMode::One => {
                    assert!(!stopped && played.iter().all(|t| *t == played[0]), "{what}");
                }
                _ => assert!(!stopped, "{what}"),
            },
        }
        if repeat == RepeatMode::Album && stop_after != StopAfter::Track {
            let first_album = first_album.as_deref();
            for t in &played {
                let album = format!("a{}", (t - 1) / n.div_ceil(albums.max(1)).max(1));
                assert_eq!(Some(album.as_str()), first_album, "{what}: {played:?}");
            }
        }
    }

    #[test]
    fn every_mode_combination_follows_its_rules() {
        for shuffle in ShuffleMode::ALL {
            for repeat in RepeatMode::ALL {
                for stop_after in StopAfter::ALL {
                    for consume in [false, true] {
                        for (n, start, albums) in [(1, 0, 1), (7, 2, 3), (12, 11, 4)] {
                            for seed in 0..3 {
                                check_combination(
                                    n, start, seed, shuffle, repeat, stop_after, consume, albums,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_shuffle_mode_plays_each_entry_once_per_cycle() {
        for mode in SHUFFLES {
            for (n, start, albums, interleave) in
                [(9, 4, 3, false), (10, 0, 3, true), (1, 0, 1, false)]
            {
                check_cycles(n, start, 5, 3, mode, albums, interleave);
            }
        }
    }

    #[test]
    fn album_shuffle_plays_the_current_album_on_first() {
        let mut queue = Queue::new(Rng::new(4));
        queue.set_shuffle(ShuffleMode::Albums);
        queue.replace(QueueContext::files(), keyed_tracks(9, 3, false), 1);
        let order = track_numbers(&queue);
        assert_eq!(&order[..3], &[2, 3, 1], "{order:?}");
        let rest: Vec<&[u64]> = order[3..].chunks(3).collect();
        assert!(rest.contains(&[4, 5, 6].as_slice()) && rest.contains(&[7, 8, 9].as_slice()));
        // Turning shuffle off restores the context order and keeps the current entry.
        queue.set_shuffle(ShuffleMode::Off);
        assert_eq!(track_numbers(&queue), (1..=9).collect::<Vec<_>>());
        assert_eq!(current_number(&queue), Some(2));
    }

    #[test]
    fn within_album_keeps_the_albums_in_order() {
        let mut queue = Queue::new(Rng::new(9));
        queue.set_shuffle(ShuffleMode::WithinAlbum);
        queue.replace(QueueContext::files(), keyed_tracks(9, 3, false), 4);
        let order = track_numbers(&queue);
        assert_eq!(order[0], 5);
        let album = |t: &u64| (t - 1) / 3;
        assert!(order[..3].iter().all(|t| album(t) == 1), "{order:?}");
        assert!(order[3..6].iter().all(|t| album(t) == 2), "{order:?}");
        assert!(order[6..].iter().all(|t| album(t) == 0), "{order:?}");
    }

    #[test]
    fn a_jump_under_album_shuffle_brings_its_album_along() {
        let mut queue = Queue::new(Rng::new(2));
        queue.set_shuffle(ShuffleMode::Albums);
        queue.replace(QueueContext::files(), keyed_tracks(9, 3, false), 0);
        let target = queue
            .items()
            .iter()
            .find(|e| number_of(&queue, e.id) == 8)
            .unwrap()
            .id;
        queue.jump_to(target);
        let order = track_numbers(&queue);
        let at = queue.current_index().unwrap();
        assert_eq!(&order[at..at + 3], &[8, 9, 7], "{order:?}");
    }

    #[test]
    fn repeat_album_wraps_inside_the_album() {
        let mut queue = Queue::new(Rng::new(1));
        queue.replace(QueueContext::files(), keyed_tracks(9, 3, false), 4);
        queue.set_repeat(RepeatMode::Album);
        let played: Vec<u64> = (0..5)
            .map(|_| step(&mut queue, Advance::Auto).unwrap())
            .collect();
        assert_eq!(played, vec![6, 4, 5, 6, 4]);
        let previous = queue.peek_previous(|_| false).unwrap();
        assert_eq!(
            number_of(&queue, previous),
            6,
            "previous wraps the other way"
        );
    }

    #[test]
    fn album_navigation() {
        let mut queue = Queue::new(Rng::new(1));
        queue.replace(QueueContext::files(), keyed_tracks(9, 3, false), 4);
        let next = queue.peek_next_album(|_| false).unwrap();
        assert_eq!(number_of(&queue, next), 7);
        let previous = queue.peek_previous_album(|_| false).unwrap();
        assert_eq!(number_of(&queue, previous), 1);
        queue.set_current(queue.items()[1].id);
        let previous = queue.peek_previous_album(|_| false).unwrap();
        assert_eq!(number_of(&queue, previous), 1, "the first album restarts");
        queue.set_current(queue.items()[0].id);
        assert_eq!(queue.peek_previous_album(|_| false), None);
        queue.set_current(queue.items()[8].id);
        assert_eq!(queue.peek_next_album(|_| false), None);
        queue.set_repeat(RepeatMode::All);
        let next = queue.peek_next_album(|_| false).unwrap();
        assert_eq!(number_of(&queue, next), 1, "repeat-all wraps");
        for _ in 0..10 {
            let random = queue.pick_random_album(|_| false).unwrap();
            assert!(number_of(&queue, random) == 1 || number_of(&queue, random) == 4);
        }
    }

    #[test]
    fn flags_change_the_keys_of_every_entry_of_the_track() {
        let mut queue = queue(3, 0);
        queue.enqueue(vec![TrackRef::Library(TrackId::new(2))], EnqueueAt::End);
        let flags = TrackFlags {
            skip_in_shuffle: true,
            ..TrackFlags::default()
        };
        queue.set_flags(&TrackRef::Library(TrackId::new(2)), flags);
        let flagged = queue
            .items()
            .iter()
            .filter(|e| queue.keys(e.id).flags == flags)
            .count();
        assert_eq!(flagged, 2);
    }

    proptest! {
        /// Every shuffle mode plays every entry once per cycle and keeps its blocks
        /// together, whatever the albums look like.
        #[test]
        fn every_shuffle_scope_plays_each_entry_once_per_cycle(
            n in 1u64..30,
            start in 0usize..30,
            seed in any::<u64>(),
            cycles in 1usize..4,
            mode in 0usize..5,
            albums in 1u64..6,
            interleave in any::<bool>(),
        ) {
            check_cycles(n, start, seed, cycles, SHUFFLES[mode], albums, interleave);
        }

        /// Stop-after, repeat and consume over random queues, in every combination.
        #[test]
        fn mode_combinations_follow_their_rules(
            n in 1u64..20,
            start in 0usize..20,
            seed in any::<u64>(),
            shuffle in 0usize..6,
            repeat in 0usize..4,
            stop_after in 0usize..4,
            consume in any::<bool>(),
            albums in 1u64..5,
        ) {
            check_combination(
                n,
                start,
                seed,
                ShuffleMode::ALL[shuffle],
                RepeatMode::ALL[repeat],
                StopAfter::ALL[stop_after],
                consume,
                albums,
            );
        }

        /// With shuffle and repeat-all, every entry plays once per cycle, for several
        /// cycles, and a cycle never starts with the entry that ended the last one.
        #[test]
        fn shuffle_plays_every_entry_once_per_cycle(
            n in 1u64..40,
            start in 0usize..40,
            seed in any::<u64>(),
            cycles in 1usize..4,
        ) {
            let mut queue = Queue::new(Rng::new(seed));
            queue.set_shuffle(ShuffleMode::Tracks);
            queue.set_repeat(RepeatMode::All);
            let start = start % n as usize;
            queue.replace(QueueContext::files(), tracks(n), start);
            let mut played = vec![current_number(&queue).unwrap()];
            for _ in 1..(n as usize * cycles) {
                played.push(step(&mut queue, Advance::Auto).unwrap());
            }
            for (i, cycle) in played.chunks(n as usize).enumerate() {
                let mut sorted = cycle.to_vec();
                sorted.sort_unstable();
                prop_assert_eq!(sorted, (1..=n).collect::<Vec<_>>(), "cycle {}", i);
            }
            if n > 1 {
                for boundary in (n as usize..played.len()).step_by(n as usize) {
                    prop_assert_ne!(played[boundary - 1], played[boundary]);
                }
            }
            assert_consistent(&queue);
        }

        /// "Previous" walks back exactly the order played, in every shuffle mode.
        #[test]
        fn previous_retraces_the_history(
            n in 2u64..30,
            seed in any::<u64>(),
            mode in 0usize..6,
            albums in 1u64..5,
            forward in 1usize..30,
        ) {
            let mut queue = Queue::new(Rng::new(seed));
            queue.set_shuffle(ShuffleMode::ALL[mode]);
            queue.replace(QueueContext::files(), keyed_tracks(n, albums, true), 0);
            let mut played = vec![current_number(&queue).unwrap()];
            for _ in 0..forward.min(n as usize - 1) {
                played.push(step(&mut queue, Advance::User).unwrap());
            }
            let mut walked_back = vec![current_number(&queue).unwrap()];
            while let Some(previous) = queue.peek_previous(|_| false) {
                prop_assert!(queue.set_current(previous));
                walked_back.push(current_number(&queue).unwrap());
            }
            walked_back.reverse();
            prop_assert_eq!(&walked_back[..played.len()], &played[..]);
            prop_assert_eq!(walked_back.len(), played.len());
        }

        /// Any sequence of edits keeps the queue consistent and the current entry
        /// where it was, unless it was removed.
        #[test]
        fn edits_keep_the_queue_consistent(
            n in 0u64..20,
            seed in any::<u64>(),
            ops in proptest::collection::vec((0u8..8, any::<u16>(), any::<u16>()), 0..40),
        ) {
            let mut queue = Queue::new(Rng::new(seed));
            queue.replace(QueueContext::files(), keyed_tracks(n, 3, true), 0);
            let mut extra = 100;
            for (op, a, b) in ops {
                let len = queue.len().max(1);
                let pick = |i: u16| queue.items().get(usize::from(i) % len).map(|e| e.id);
                let before = queue.current().map(|e| e.id);
                let version = queue.version();
                match op {
                    0 => {
                        extra += 1;
                        queue.enqueue(vec![TrackRef::Library(TrackId::new(extra))], EnqueueAt::Next);
                    }
                    1 => {
                        extra += 1;
                        queue.enqueue(vec![TrackRef::Library(TrackId::new(extra))], EnqueueAt::End);
                    }
                    2 => {
                        if let Some(id) = pick(a) {
                            let removed_current = queue.remove(&[id]);
                            prop_assert_eq!(removed_current, Some(id) == before);
                        }
                    }
                    3 => {
                        if let Some(id) = pick(a) {
                            queue.move_entry(id, usize::from(b) % len);
                            prop_assert_eq!(queue.current().map(|e| e.id), before);
                        }
                    }
                    4 => {
                        let mode = ShuffleMode::ALL[usize::from(a) % ShuffleMode::ALL.len()];
                        queue.set_shuffle(mode);
                        prop_assert_eq!(queue.current().map(|e| e.id), before);
                    }
                    5 => {
                        if let Some(id) = pick(a) {
                            queue.jump_to(id);
                            prop_assert_eq!(queue.current().map(|e| e.id), Some(id));
                        }
                    }
                    6 => {
                        if let Some(next) = queue.peek_next(Advance::User, |_| false) {
                            queue.advance_to(next);
                        }
                    }
                    _ => {
                        queue.set_repeat(RepeatMode::ALL[usize::from(a) % RepeatMode::ALL.len()]);
                    }
                }
                prop_assert!(queue.version() >= version);
                assert_consistent(&queue);
            }
        }
    }
}
