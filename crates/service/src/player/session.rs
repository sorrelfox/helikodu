// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session on disk, and the crash sentinel.
//!
//! - `session.json` holds the queue (entries, context order, context, current
//!   entry), the position, whether it was playing, the play modes, the volume, the
//!   output, the tracks marked suspect, and the entries endless play, the Auto-DJ
//!   or a radio added with the radio's seed. It is written by atomic replace.
//! - `playing.json` is the **crash sentinel**: it names the tracks being loaded or
//!   played (the current one, and the next one once the engine opened it for a
//!   gapless join), each with the number of abnormal ends in a row that named it
//!   before. It is removed when nothing plays and when the program ends normally, so
//!   finding it at the start means the last run ended abnormally. Each track it
//!   names gets one more strike; a track with two strikes is suspect: automatic
//!   playback passes over it until it plays to its end.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use helikodu_library::{AlbumId, PersonId, PlaySeed};
use helikodu_model::audio::OutputSelection;
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::{EntryId, PlayModes, QueueContext, QueueEntry, StopAfter, Volume};
use serde::{Deserialize, Serialize};

use crate::files::{read_if_exists, remove_if_exists, write_atomic};

/// The file name of the session.
pub const SESSION_FILE: &str = "session.json";

/// The file name of the crash sentinel.
pub const SENTINEL_FILE: &str = "playing.json";

/// How many abnormal ends in a row make a track suspect.
pub const SUSPECT_STRIKES: u32 = 2;

/// The version of the session file's layout.
pub(crate) const SESSION_VERSION: u32 = 1;

/// What is saved of a player.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedSession {
    pub(crate) version: u32,
    pub(crate) context: QueueContext,
    /// The entries in play order.
    pub(crate) entries: Vec<QueueEntry>,
    /// The entries in context order.
    pub(crate) linear: Vec<EntryId>,
    pub(crate) current: Option<usize>,
    /// The position in the current entry, in milliseconds.
    pub(crate) position_ms: u64,
    pub(crate) modes: PlayModes,
    pub(crate) volume: Volume,
    pub(crate) output: OutputSelection,
    #[serde(default)]
    pub(crate) suspects: Vec<TrackRef>,
    /// Stop-after, which stays set until it stops playback.
    #[serde(default)]
    pub(crate) stop_after: StopAfter,
    /// Whether the current entry was playing when the session was saved (paused
    /// and stopped are not playing). Files written before it existed read as not
    /// playing.
    #[serde(default)]
    pub(crate) playing: bool,
    /// What endless play, the Auto-DJ or a radio added to the queue, and the
    /// radio's seed. Files written before it existed read as nothing added.
    #[serde(default)]
    pub(crate) smart: SavedSmart,
}

/// What endless play, the Auto-DJ or a radio added to the saved queue.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct SavedSmart {
    /// The entries they added, in play order.
    #[serde(default)]
    pub(crate) added: Vec<EntryId>,
    /// The seed of the radio that endless play continues from.
    #[serde(default)]
    pub(crate) radio_seed: Option<SavedSeed>,
}

/// The seed of a radio, by its number in the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SavedSeed {
    Track(u64),
    Album(i64),
    Artist(i64),
}

impl From<PlaySeed> for SavedSeed {
    fn from(seed: PlaySeed) -> Self {
        match seed {
            PlaySeed::Track(track) => Self::Track(track.get()),
            PlaySeed::Album(album) => Self::Album(album.get()),
            PlaySeed::Artist(person) => Self::Artist(person.get()),
        }
    }
}

impl From<SavedSeed> for PlaySeed {
    fn from(seed: SavedSeed) -> Self {
        match seed {
            SavedSeed::Track(track) => Self::Track(TrackId::new(track)),
            SavedSeed::Album(album) => Self::Album(AlbumId::from_raw(album)),
            SavedSeed::Artist(person) => Self::Artist(PersonId::from_raw(person)),
        }
    }
}

/// One track the sentinel names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Strike {
    pub(crate) track: TrackRef,
    /// The abnormal ends in a row that named it before this run.
    pub(crate) strikes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct Sentinel {
    tracks: Vec<Strike>,
}

/// What the start-up found about the last run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CrashReport {
    /// Whether the last run ended abnormally.
    pub(crate) crashed: bool,
    /// The tracks the sentinel named, with their strikes including the last end.
    pub(crate) strikes: HashMap<TrackRef, u32>,
}

impl CrashReport {
    /// The tracks with enough strikes to be suspect.
    pub(crate) fn suspects(&self) -> impl Iterator<Item = &TrackRef> {
        self.strikes
            .iter()
            .filter(|(_, strikes)| **strikes >= SUSPECT_STRIKES)
            .map(|(track, _)| track)
    }
}

/// The session's files in one folder.
#[derive(Debug, Clone)]
pub(crate) struct SessionFiles {
    dir: PathBuf,
    /// What the sentinel names now, to write it only when that changes.
    named: Vec<TrackRef>,
}

impl SessionFiles {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            named: Vec::new(),
        }
    }

    fn session_path(&self) -> PathBuf {
        self.dir.join(SESSION_FILE)
    }

    fn sentinel_path(&self) -> PathBuf {
        self.dir.join(SENTINEL_FILE)
    }

    /// The saved session; `None` when there is none or it cannot be read (a broken
    /// session is not worth keeping the program from starting).
    pub(crate) fn load(&self) -> Option<SavedSession> {
        let bytes = read_if_exists(&self.session_path()).ok()??;
        let session: SavedSession = serde_json::from_slice(&bytes).ok()?;
        (session.version <= SESSION_VERSION).then_some(session)
    }

    pub(crate) fn save(&self, session: &SavedSession) -> io::Result<()> {
        let bytes = serde_json::to_vec(session).map_err(io::Error::other)?;
        write_atomic(&self.session_path(), &bytes)
    }

    /// Reads and removes the sentinel left by the last run. Removing it at once
    /// means that a later abnormal end before anything plays names nothing.
    pub(crate) fn take_crash_report(&mut self) -> CrashReport {
        let path = self.sentinel_path();
        let Ok(Some(bytes)) = read_if_exists(&path) else {
            return CrashReport::default();
        };
        let _ = remove_if_exists(&path);
        // An unreadable sentinel still says that the last run ended abnormally.
        let sentinel: Sentinel = serde_json::from_slice(&bytes).unwrap_or_default();
        CrashReport {
            crashed: true,
            strikes: sentinel
                .tracks
                .into_iter()
                .map(|strike| (strike.track, strike.strikes.saturating_add(1)))
                .collect(),
        }
    }

    /// Names `tracks` as being loaded or played, each with its strikes so far; an
    /// empty list removes the sentinel.
    pub(crate) fn name(&mut self, tracks: Vec<Strike>) -> io::Result<()> {
        let named: Vec<TrackRef> = tracks.iter().map(|s| s.track.clone()).collect();
        if named == self.named {
            return Ok(());
        }
        self.named = named;
        if tracks.is_empty() {
            remove_if_exists(&self.sentinel_path())
        } else {
            let bytes = serde_json::to_vec(&Sentinel { tracks }).map_err(io::Error::other)?;
            write_atomic(&self.sentinel_path(), &bytes)
        }
    }

    /// Removes the sentinel: the program ends normally.
    pub(crate) fn clear(&mut self) -> io::Result<()> {
        self.named.clear();
        remove_if_exists(&self.sentinel_path())
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::library::{MediaSource, TrackId};
    use helikodu_model::player::{PlayScope, RepeatMode, ShuffleMode};

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("hk-session-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_session_round_trips() {
        let files = SessionFiles::new(&dir("round_trip"));
        assert_eq!(files.load(), None);
        let entries = vec![QueueEntry {
            id: EntryId::new(4),
            track: TrackRef::File(MediaSource::file("/m/a.flac")),
            from_context: true,
        }];
        let session = SavedSession {
            version: SESSION_VERSION,
            context: QueueContext::files(),
            entries,
            linear: vec![EntryId::new(4)],
            current: Some(0),
            position_ms: 1234,
            modes: PlayModes {
                consume: true,
                follow_cursor: true,
                scope: PlayScope::Album,
                ..PlayModes::new(ShuffleMode::Albums, RepeatMode::Album)
            },
            volume: Volume::new(0.5).unwrap(),
            output: OutputSelection::default(),
            suspects: vec![TrackRef::Library(TrackId::new(9))],
            stop_after: StopAfter::Album,
            playing: true,
            smart: SavedSmart {
                added: vec![EntryId::new(4)],
                radio_seed: Some(SavedSeed::Artist(7)),
            },
        };
        files.save(&session).unwrap();
        assert_eq!(files.load(), Some(session));
    }

    #[test]
    fn an_older_session_reads_as_not_playing_with_nothing_added() {
        let dir = dir("older");
        std::fs::create_dir_all(&dir).unwrap();
        let mut value = serde_json::to_value(SavedSession {
            version: SESSION_VERSION,
            context: QueueContext::files(),
            entries: Vec::new(),
            linear: Vec::new(),
            current: None,
            position_ms: 0,
            modes: PlayModes::default(),
            volume: Volume::FULL,
            output: OutputSelection::default(),
            suspects: Vec::new(),
            stop_after: StopAfter::Off,
            playing: true,
            smart: SavedSmart {
                added: vec![EntryId::new(1)],
                radio_seed: Some(SavedSeed::Track(3)),
            },
        })
        .unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("playing");
        object.remove("smart");
        std::fs::write(dir.join(SESSION_FILE), value.to_string()).unwrap();
        let loaded = SessionFiles::new(&dir).load().unwrap();
        assert!(!loaded.playing);
        assert_eq!(loaded.smart, SavedSmart::default());
    }

    #[test]
    fn a_radio_seed_converts_both_ways() {
        for seed in [
            PlaySeed::Track(TrackId::new(5)),
            PlaySeed::Album(AlbumId::from_raw(6)),
            PlaySeed::Artist(PersonId::from_raw(7)),
        ] {
            assert_eq!(PlaySeed::from(SavedSeed::from(seed)), seed);
        }
    }

    #[test]
    fn strikes_add_up_over_abnormal_ends() {
        let dir = dir("strikes");
        let track = TrackRef::Library(TrackId::new(1));
        let other = TrackRef::Library(TrackId::new(2));

        let mut first_run = SessionFiles::new(&dir);
        assert!(!first_run.take_crash_report().crashed);
        first_run
            .name(vec![Strike {
                track: track.clone(),
                strikes: 0,
            }])
            .unwrap();
        // The run ends abnormally: the sentinel stays.

        let mut second_run = SessionFiles::new(&dir);
        let report = second_run.take_crash_report();
        assert!(report.crashed);
        assert_eq!(report.strikes[&track], 1);
        assert_eq!(report.suspects().count(), 0);
        second_run
            .name(vec![
                Strike {
                    track: track.clone(),
                    strikes: 1,
                },
                Strike {
                    track: other.clone(),
                    strikes: 0,
                },
            ])
            .unwrap();

        let mut third_run = SessionFiles::new(&dir);
        let report = third_run.take_crash_report();
        assert_eq!(report.suspects().collect::<Vec<_>>(), vec![&track]);
        assert_eq!(report.strikes[&other], 1);

        // Nothing was named: a fourth start finds no crash.
        third_run.clear().unwrap();
        assert!(!SessionFiles::new(&dir).take_crash_report().crashed);
    }
}
