// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The broken set: truncated files, empty files, valid headers with garbage,
//! noise with an audio extension and a misnamed file. Nothing panics, every file
//! either plays (what is there of it) or fails with a clean error, and playback
//! continues with the next item.

use std::panic::{AssertUnwindSafe, catch_unwind};

use helikodu_model::player::{EngineError, EngineEvent, TrackEndReason, TransportState};

use crate::decoding;
use crate::manifest::{Corpus, Entry};
use crate::report::Table;
use crate::session::{Session, item, null_config};

/// A good file played between the broken ones.
const GOOD: &str = "formats/pcm/wav-s16.wav";

/// What decoding one file through the registry gives.
fn decode_outcome(corpus: &Corpus, entry: &Entry) -> Result<String, String> {
    let path = corpus.path(entry);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut decoder = match decoding::open(&path) {
            Ok(decoder) => decoder,
            Err(error) => {
                let kind = match error.to_engine_error() {
                    EngineError::Unplayable { .. } => "unplayable",
                    EngineError::Decode { .. } => "decode error",
                    _ => "other error",
                };
                return format!("{kind}: {error}");
            }
        };
        let mut samples = Vec::new();
        let channels = usize::from(decoder.spec().channel_count()).max(1);
        match decoding::read_to_end(decoder.as_mut(), &mut samples) {
            Ok(()) => format!("plays {} frames", samples.len() / channels),
            Err(error) => format!(
                "decode error after {} frames: {error}",
                samples.len() / channels
            ),
        }
    }));
    result.map_err(|_| "panicked".to_owned())
}

#[test]
fn broken_files_fail_cleanly_and_playback_goes_on() {
    let Some(corpus) = Corpus::load("broken") else {
        return;
    };
    let broken = corpus.set("broken");
    let good = corpus
        .files
        .iter()
        .find(|f| f.path == GOOD)
        .cloned()
        .expect("the corpus has the 16-bit WAV sample");

    // A queue: good, broken, good, broken, ..., good.
    let mut queue = vec![good.clone()];
    for entry in &broken {
        queue.push(entry.clone());
        queue.push(good.clone());
    }
    let mut session = Session::new(null_config());
    let items: Vec<_> = queue
        .iter()
        .enumerate()
        .map(|(i, f)| item(i as u64 + 1, &corpus.path(f)))
        .collect();
    session.play_queue(&items);
    session.wait_state(TransportState::Stopped);
    let ended = |id: u64| {
        session.log.iter().find_map(|e| match e {
            EngineEvent::TrackEnded { item, reason } if item.get() == id => Some(*reason),
            _ => None,
        })
    };
    let errors = |id: u64| {
        session
            .log
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Error {
                    item: Some(item),
                    error,
                } if item.get() == id => Some(error.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    let mut table = Table::new("broken", &["file", "registry", "transport"]);
    // The good items around the broken ones all play to their end.
    for (i, entry) in queue.iter().enumerate() {
        if entry.path == GOOD {
            let id = i as u64 + 1;
            assert_eq!(
                ended(id),
                Some(TrackEndReason::Finished),
                "good item {id} did not play: {:#?}",
                session.milestones()
            );
        }
    }
    for (n, entry) in broken.iter().enumerate() {
        let id = 2 * n as u64 + 2;
        let outcome = decode_outcome(&corpus, entry);
        let reason = ended(id);
        let transport = match (reason, errors(id).first()) {
            (Some(reason), Some(error)) => format!("{reason:?}: {error}"),
            (Some(reason), None) => format!("{reason:?}"),
            (None, Some(error)) => format!("skipped: {error}"),
            (None, None) => "no end, no error".into(),
        };
        let mut problems = Vec::new();
        let registry = match outcome {
            Ok(text) => text,
            Err(panic) => {
                problems.push(panic.clone());
                panic
            }
        };
        match reason {
            Some(TrackEndReason::Finished | TrackEndReason::Failed) => {}
            None if !errors(id).is_empty() => {}
            other => problems.push(format!("the transport ended it with {other:?}")),
        }
        // A file that fails says so with an error event.
        if reason == Some(TrackEndReason::Failed) && errors(id).is_empty() {
            problems.push("failed without an error event".into());
        }
        table.row(
            vec![entry.display(), registry, transport],
            (!problems.is_empty()).then(|| problems.join("; ")),
        );
    }
    drop(session);
    table.finish();
}
