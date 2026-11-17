// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The gapless sets: the parts of a split reference, played back to back through
//! the transport into the WAV sink as fast as it takes them, give the reference
//! again: bit-exact for lossless codecs, with the exact total length for lossy
//! ones.

use std::collections::BTreeMap;

use helikodu_model::player::{EngineEvent, TrackEndReason, TransportState};

use crate::decoding;
use crate::manifest::{Corpus, Entry, is_lossless};
use crate::media;
use crate::report::Table;
use crate::session::{Session, item, wav_config};

/// The parts of each encoder's set, by folder (`gapless/<encoder>/`), in order.
fn sets(corpus: &Corpus) -> BTreeMap<String, Vec<Entry>> {
    let mut sets: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for entry in corpus.set("gapless") {
        let mut parts = entry.path.splitn(3, '/');
        let (Some(_), Some(folder), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
            continue; // a reference
        };
        sets.entry(folder.to_owned()).or_default().push(entry);
    }
    for parts in sets.values_mut() {
        parts.sort_by(|a, b| a.path.cmp(&b.path));
    }
    sets
}

/// Plays `parts` as a queue into a WAV file. Returns what was written, or why the
/// events were not those of a gapless queue.
fn play(corpus: &Corpus, parts: &[Entry], tag: &str) -> Result<media::Written, String> {
    let out = media::path(&format!("corpus-gapless-{tag}.wav"));
    let mut session = Session::new(wav_config(&out));
    let items: Vec<_> = parts
        .iter()
        .enumerate()
        .map(|(i, part)| item(i as u64 + 1, &corpus.path(part)))
        .collect();
    session.play_queue(&items);
    session.wait_state(TransportState::Stopped);
    let ends: Vec<(u64, Option<TrackEndReason>)> = session
        .milestones()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::TrackStarted { item, .. } => Some((item.get(), None)),
            EngineEvent::TrackEnded { item, reason } => Some((item.get(), Some(reason))),
            _ => None,
        })
        .collect();
    let wanted: Vec<_> = (1..=parts.len() as u64)
        .flat_map(|i| [(i, None), (i, Some(TrackEndReason::Finished))])
        .collect();
    let reopened = session
        .log
        .contains(&EngineEvent::StateChanged(TransportState::Reconfiguring));
    drop(session);
    if ends != wanted {
        return Err(format!("events {ends:?}"));
    }
    if reopened {
        return Err("the device was reopened between parts".into());
    }
    Ok(media::read_output(&out))
}

#[test]
fn every_gapless_set_joins_without_a_gap() {
    let Some(corpus) = Corpus::load("gapless") else {
        return;
    };
    let mut table = Table::new(
        "gapless",
        &["set", "codec", "parts", "frames", "reference", "check"],
    );
    let mut references = BTreeMap::new();
    for (folder, parts) in sets(&corpus) {
        let expect = parts[0].expect();
        let codec = expect.codec.clone().unwrap_or_default();
        let reference_name = match expect.sample_rate {
            Some(48_000) => "gapless/reference-48k.wav",
            _ => "gapless/reference-44k1.wav",
        };
        let reference = references.entry(reference_name).or_insert_with(|| {
            let path = corpus.dir.join(reference_name);
            decoding::decode(&path).expect("the gapless reference decodes")
        });
        let lossless = is_lossless(&codec);
        let exact_length = parts.iter().all(|p| p.expect().exact_length);
        let mut cells = vec![
            format!("gapless/{folder}"),
            codec.clone(),
            parts.len().to_string(),
        ];
        let written = match play(&corpus, &parts, &folder) {
            Ok(written) => written,
            Err(problem) => {
                cells.extend(["-".into(), reference.frames().to_string(), "-".into()]);
                table.row(cells, Some(problem));
                continue;
            }
        };
        let frames = written.frames();
        cells.extend([frames.to_string(), reference.frames().to_string()]);
        let problem = if written.rate != reference.rate || written.channels != reference.channels {
            Some(format!(
                "{} Hz {} ch written, the reference is {} Hz {} ch",
                written.rate, written.channels, reference.rate, reference.channels
            ))
        } else if lossless {
            cells.push("bit-exact".into());
            let got: Vec<f32> = if written.ints.is_empty() {
                written.floats.clone()
            } else {
                written
                    .ints
                    .iter()
                    .map(|&v| f32::from(v) / 32_768.0)
                    .collect()
            };
            match got.iter().zip(&reference.samples).position(|(a, b)| a != b) {
                _ if got.len() != reference.samples.len() => {
                    Some(format!("{frames} frames instead of {}", reference.frames()))
                }
                Some(i) => Some(format!(
                    "first difference at frame {}",
                    i / reference.channels
                )),
                None => None,
            }
        } else if exact_length {
            cells.push("exact length".into());
            (frames != reference.frames())
                .then(|| format!("{frames} frames instead of {}", reference.frames()))
        } else {
            // No delay or padding information in the format (AAC in ADTS): the
            // joins cannot be exact; report how far off the total is.
            let off = frames as i64 - reference.frames() as i64;
            cells.push(format!("not gapless by format ({off:+} frames)"));
            None
        };
        table.row(cells, problem);
    }
    table.finish();
}
