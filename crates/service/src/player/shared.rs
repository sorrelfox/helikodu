// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What front ends read of the player without asking its thread: the queue and the
//! status, replaced as a whole after every change.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use helikodu_model::audio::{AudioSpec, MediaTime, OutputSelection};
use helikodu_model::library::TrackRef;
use helikodu_model::player::{
    EntryId, PlayModes, PlaybackState, QueueContext, QueueEntry, StopAfter, TransportState, Volume,
};

/// The queue as it is at one version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueSnapshot {
    /// The version; [`PlayerEvent::QueueChanged`](super::PlayerEvent::QueueChanged)
    /// carries the same number.
    pub version: u64,
    /// Where the tracks came from.
    pub context: QueueContext,
    /// The entries in play order.
    pub entries: Vec<QueueEntry>,
    /// The index of the current entry.
    pub current: Option<usize>,
    /// Shuffle and repeat.
    pub modes: PlayModes,
}

impl QueueSnapshot {
    /// The entries before the current one: the history.
    pub fn history(&self) -> &[QueueEntry] {
        &self.entries[..self.current.unwrap_or(0)]
    }

    /// The current entry.
    pub fn current_entry(&self) -> Option<&QueueEntry> {
        self.entries.get(self.current?)
    }

    /// The entries after the current one (all of them when none is current).
    pub fn upcoming(&self) -> &[QueueEntry] {
        &self.entries[self.current.map_or(0, |c| c + 1)..]
    }

    /// The one change that turns `old` into `self` for a list model: the entries
    /// that differ form one run at `position`, `removed` long in `old` and `added`
    /// long here. `None` when the entries are the same.
    pub fn splice_from(&self, old: &Self) -> Option<Splice> {
        let ids = |s: &Self| s.entries.iter().map(|e| e.id).collect::<Vec<EntryId>>();
        let (new, old) = (ids(self), ids(old));
        let prefix = new.iter().zip(&old).take_while(|(a, b)| a == b).count();
        if prefix == new.len() && prefix == old.len() {
            return None;
        }
        let suffix = new[prefix..]
            .iter()
            .rev()
            .zip(old[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        Some(Splice {
            position: prefix,
            removed: old.len() - prefix - suffix,
            added: new.len() - prefix - suffix,
        })
    }
}

/// A change of a list: at `position`, `removed` items gave way to `added` ones (the
/// arguments of GTK's `items-changed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Splice {
    /// Where.
    pub position: usize,
    /// How many items went.
    pub removed: usize,
    /// How many came.
    pub added: usize,
}

/// What is playing.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    /// The entry.
    pub entry: EntryId,
    /// Its track.
    pub track: TrackRef,
    /// Its file.
    pub path: PathBuf,
    /// The format of its decoded audio.
    pub spec: AudioSpec,
    /// Its length, if known.
    pub length: Option<MediaTime>,
}

/// The player's state for the transport controls.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerStatus {
    /// As the controls show it.
    pub playback: PlaybackState,
    /// The engine's transport.
    pub transport: TransportState,
    /// What plays.
    pub now_playing: Option<NowPlaying>,
    /// The position in it.
    pub position: MediaTime,
    /// The volume.
    pub volume: Volume,
    /// Where audio goes.
    pub output: OutputSelection,
    /// Shuffle and repeat.
    pub modes: PlayModes,
    /// Stop-after.
    pub stop_after: StopAfter,
}

impl Default for PlayerStatus {
    fn default() -> Self {
        Self {
            playback: PlaybackState::Stopped,
            transport: TransportState::Stopped,
            now_playing: None,
            position: MediaTime::ZERO,
            volume: Volume::FULL,
            output: OutputSelection::default(),
            modes: PlayModes::default(),
            stop_after: StopAfter::Off,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Shared {
    queue: Mutex<Arc<QueueSnapshot>>,
    status: Mutex<PlayerStatus>,
}

impl Shared {
    pub(crate) fn queue(&self) -> Arc<QueueSnapshot> {
        Arc::clone(&self.queue.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn set_queue(&self, snapshot: QueueSnapshot) {
        *self.queue.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(snapshot);
    }

    pub(crate) fn status(&self) -> PlayerStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn update_status(&self, change: impl FnOnce(&mut PlayerStatus)) {
        change(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::library::TrackId;

    use super::*;

    fn snapshot(ids: &[u64]) -> QueueSnapshot {
        QueueSnapshot {
            entries: ids
                .iter()
                .map(|&id| QueueEntry {
                    id: EntryId::new(id),
                    track: TrackRef::Library(TrackId::new(id)),
                    from_context: true,
                })
                .collect(),
            current: Some(1),
            ..QueueSnapshot::default()
        }
    }

    #[test]
    fn splices_cover_the_changed_run() {
        let old = snapshot(&[1, 2, 3, 4]);
        assert_eq!(snapshot(&[1, 2, 3, 4]).splice_from(&old), None);
        assert_eq!(
            snapshot(&[1, 9, 3, 4]).splice_from(&old),
            Some(Splice {
                position: 1,
                removed: 1,
                added: 1
            })
        );
        assert_eq!(
            snapshot(&[1, 2, 3, 4, 5]).splice_from(&old),
            Some(Splice {
                position: 4,
                removed: 0,
                added: 1
            })
        );
        assert_eq!(
            snapshot(&[1, 4]).splice_from(&old),
            Some(Splice {
                position: 1,
                removed: 2,
                added: 0
            })
        );
        let moved = snapshot(&[1, 4, 2, 3]);
        assert_eq!(moved.history().len(), 1);
        assert_eq!(moved.current_entry().unwrap().id, EntryId::new(4));
        assert_eq!(moved.upcoming().len(), 2);
    }
}
