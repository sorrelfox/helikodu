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
//! - With shuffle and repeat-all, the order of the next cycle is drawn when it is
//!   first needed (at the end of the current one, to name the next track early for
//!   a gapless join), stored, and applied when its first entry starts.
//! - Operations change the queue's version, which front ends compare to find out
//!   whether to fetch it again.

use helikodu_model::library::TrackRef;
use helikodu_model::player::{
    EnqueueAt, EntryId, PlayModes, QueueContext, QueueEntry, RepeatMode, ShuffleMode,
};

use super::rng::Rng;

/// Whether a move to the next entry is the player's own (a track ended) or the
/// user's ("Next"). Repeat-one only holds the player on the same entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Advance {
    Auto,
    User,
}

#[derive(Debug, Clone)]
pub(crate) struct Queue {
    items: Vec<QueueEntry>,
    current: Option<usize>,
    linear: Vec<EntryId>,
    context: QueueContext,
    modes: PlayModes,
    /// The play order of the next shuffled cycle, once drawn.
    next_cycle: Option<Vec<EntryId>>,
    last_id: u64,
    rng: Rng,
    version: u64,
}

impl Queue {
    pub(crate) fn new(rng: Rng) -> Self {
        Self {
            items: Vec::new(),
            current: None,
            linear: Vec::new(),
            context: QueueContext::files(),
            modes: PlayModes::default(),
            next_cycle: None,
            last_id: 0,
            rng,
            version: 0,
        }
    }

    /// A queue restored from a saved session. Entries keep their identifiers; the
    /// next new one follows the largest. Inconsistent parts (a context order that
    /// does not match the entries, a current index out of range) are repaired.
    pub(crate) fn restore(
        rng: Rng,
        context: QueueContext,
        items: Vec<QueueEntry>,
        linear: Vec<EntryId>,
        current: Option<usize>,
        modes: PlayModes,
    ) -> Self {
        let mut seen = std::collections::HashSet::new();
        let items: Vec<QueueEntry> = items.into_iter().filter(|e| seen.insert(e.id)).collect();
        let mut linear_ok = linear.len() == items.len();
        if linear_ok {
            let ids: std::collections::HashSet<EntryId> = items.iter().map(|e| e.id).collect();
            linear_ok = linear.iter().all(|id| ids.contains(id))
                && linear
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == linear.len();
        }
        let linear = if linear_ok {
            linear
        } else {
            items.iter().map(|e| e.id).collect()
        };
        let last_id = items.iter().map(|e| e.id.get()).max().unwrap_or(0);
        let current = current.filter(|&i| i < items.len());
        Self {
            items,
            current,
            linear,
            context,
            modes,
            next_cycle: None,
            last_id,
            rng,
            version: 1,
        }
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

    // --- Moving through it ------------------------------------------------------

    /// The entry that plays after the current one: the next in play order, the same
    /// one under repeat-one for [`Advance::Auto`], the first of the next cycle under
    /// repeat-all. Entries for which `skip` is true are passed over (but not under
    /// repeat-one, which names the current entry). `None` at the end of the queue.
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
        let mut cycle: Vec<EntryId> = self.items.iter().map(|e| e.id).collect();
        self.rng.shuffle(&mut cycle);
        if cycle.len() > 1 && cycle[0] == last {
            let swap = 1 + self.rng.below(cycle.len() - 1);
            cycle.swap(0, swap);
        }
        self.next_cycle = Some(cycle.clone());
        cycle
    }

    /// The entry before the current one in play order; under repeat-all without
    /// shuffle the last entry before the first. `None` at the start.
    pub(crate) fn peek_previous(&self, skip: impl Fn(&QueueEntry) -> bool) -> Option<EntryId> {
        let current = self.current?;
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

    /// Makes `id` the current entry as playback reaches it by moving forward
    /// ([`peek_next`](Self::peek_next)): the first entry of a new shuffled cycle
    /// starts that cycle.
    pub(crate) fn advance_to(&mut self, id: EntryId) -> bool {
        let at_end = self.current.is_some_and(|c| c + 1 == self.items.len());
        if at_end
            && let Some(cycle) = self.next_cycle.take()
            && cycle.contains(&id)
        {
            let mut by_id: std::collections::HashMap<EntryId, QueueEntry> =
                self.items.drain(..).map(|e| (e.id, e)).collect();
            self.items = cycle.iter().filter_map(|id| by_id.remove(id)).collect();
            self.items.extend(by_id.into_values());
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
    /// jumps over still play in this cycle and it does not replay the history.
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
            self.next_cycle = None;
            self.version += 1;
            return true;
        }
        self.set_current(id)
    }

    // --- Changing it ------------------------------------------------------------

    fn new_entries(&mut self, tracks: Vec<TrackRef>, from_context: bool) -> Vec<QueueEntry> {
        tracks
            .into_iter()
            .map(|track| {
                self.last_id += 1;
                QueueEntry {
                    id: EntryId::new(self.last_id),
                    track,
                    from_context,
                }
            })
            .collect()
    }

    /// Replaces the queue with `tracks` from `context`; the entry at `start` becomes
    /// current (none if `start` is out of range). With shuffle, the start entry
    /// comes first and the rest follow in a random order.
    pub(crate) fn replace(
        &mut self,
        context: QueueContext,
        tracks: Vec<TrackRef>,
        start: usize,
    ) -> Option<EntryId> {
        let entries = self.new_entries(tracks, true);
        self.linear = entries.iter().map(|e| e.id).collect();
        self.items = entries;
        self.context = context;
        self.next_cycle = None;
        self.version += 1;
        if start >= self.items.len() {
            self.current = None;
            if self.modes.shuffle != ShuffleMode::Off {
                self.rng.shuffle(&mut self.items);
            }
            return None;
        }
        if self.modes.shuffle != ShuffleMode::Off {
            self.items.swap(0, start);
            self.rng.shuffle(&mut self.items[1..]);
            self.current = Some(0);
        } else {
            self.current = Some(start);
        }
        self.current().map(|e| e.id)
    }

    /// Adds `tracks` from `context` to the end and makes `context` the queue's
    /// context. Returns the first new entry.
    pub(crate) fn append_context(
        &mut self,
        context: QueueContext,
        tracks: Vec<TrackRef>,
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
    /// With shuffle, "at the end" is a random place among the upcoming entries.
    pub(crate) fn enqueue(&mut self, tracks: Vec<TrackRef>, at: EnqueueAt) -> Vec<EntryId> {
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
                if self.modes.shuffle == ShuffleMode::Off {
                    self.items.extend(entries);
                } else {
                    for entry in entries {
                        let at = upcoming + self.rng.below(self.items.len() - upcoming + 1);
                        self.items.insert(at, entry);
                    }
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
            let mut by_id: std::collections::HashMap<EntryId, QueueEntry> =
                self.items.drain(..).map(|e| (e.id, e)).collect();
            self.items = self
                .linear
                .iter()
                .filter_map(|id| by_id.remove(id))
                .collect();
            self.current = current_id.and_then(|c| self.index_of(c));
        } else {
            let from = self.current.map_or(0, |c| c + 1);
            self.rng.shuffle(&mut self.items[from..]);
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

    proptest! {
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

        /// "Previous" walks back exactly the order played, with or without shuffle.
        #[test]
        fn previous_retraces_the_history(
            n in 2u64..30,
            seed in any::<u64>(),
            shuffled in any::<bool>(),
            forward in 1usize..30,
        ) {
            let mut queue = Queue::new(Rng::new(seed));
            if shuffled {
                queue.set_shuffle(ShuffleMode::Tracks);
            }
            queue.replace(QueueContext::files(), tracks(n), 0);
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
            queue.replace(QueueContext::files(), tracks(n), 0);
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
                        let mode = if a % 2 == 0 { ShuffleMode::Off } else { ShuffleMode::Tracks };
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
                        queue.set_repeat(if a % 2 == 0 { RepeatMode::All } else { RepeatMode::Off });
                    }
                }
                prop_assert!(queue.version() >= version);
                assert_consistent(&queue);
            }
        }
    }
}
