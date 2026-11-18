// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The long-tail set: tracker modules, game music, SID tunes and MIDI files the
//! project writes itself. The folder below `longtail/` names the backend that must
//! claim each file whatever its name; every file decodes to the length it states
//! and is not silent, and each of its subsongs opens. MIDI files play with the
//! corpus's own SoundFont (`longtail/soundfont/sine.sf2`).

use helikodu_engine::decode::{DecoderRegistry, ProbeScore};
use helikodu_model::audio::MediaTime;
use helikodu_model::library::{MediaSource, SourceSection};
use helikodu_model::settings::DecoderSettings;

use crate::decoding;
use crate::manifest::{Corpus, Entry};
use crate::report::Table;

const SOUNDFONT: &str = "longtail/soundfont/sine.sf2";

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

/// Checks one file; the summary for the table, or what is wrong.
fn check(registry: &DecoderRegistry, corpus: &Corpus, entry: &Entry) -> Result<String, String> {
    let backend = entry
        .path
        .strip_prefix("longtail/")
        .and_then(|rest| rest.split('/').next())
        .ok_or("not in a backend's folder")?;
    let path = corpus.path(entry);
    let candidates = registry.candidates(&path).map_err(|e| e.to_string())?;
    let (first, score) = candidates.first().ok_or("no backend recognises it")?;
    if (first.name(), *score) != (backend, ProbeScore::SIGNATURE) {
        return Err(format!(
            "claimed by {} ({}), not by {backend}",
            first.name(),
            score.value()
        ));
    }
    let mut decoder = registry
        .open(&MediaSource::file(&path))
        .map_err(|e| e.to_string())?;
    let mut samples = Vec::new();
    decoding::read_to_end(decoder.as_mut(), &mut samples).map_err(|e| e.to_string())?;
    let channels = usize::from(decoder.spec().channel_count());
    let frames = (samples.len() / channels) as u64;
    let stated = decoder.duration().ok_or("no length")?;
    if MediaTime::from_frames(frames, decoder.spec().rate) != stated {
        return Err(format!("{frames} frames, but {stated} stated"));
    }
    if rms(&samples) < 0.005 {
        return Err("silent".into());
    }
    let mut subsongs = 0;
    while let Ok(decoder) =
        registry.open(&MediaSource::file(&path).with_section(SourceSection::Subsong(subsongs)))
    {
        if decoder.info().subsong != Some(subsongs) {
            return Err(format!(
                "subsong {subsongs} opened as {:?}",
                decoder.info().subsong
            ));
        }
        subsongs += 1;
        if subsongs > 64 {
            break;
        }
    }
    Ok(format!(
        "{} · {:.2} s · {subsongs} subsong(s)",
        decoder.info().container,
        frames as f64 / f64::from(decoder.spec().rate.hz())
    ))
}

#[test]
fn long_tail_files_play_with_their_own_backends() {
    let Some(corpus) = Corpus::load("longtail") else {
        return;
    };
    let files = corpus.set("longtail");
    let Some(font) = files.iter().find(|f| f.path == SOUNDFONT) else {
        eprintln!("SKIPPED: longtail: the corpus has no long-tail set (make it again)");
        return;
    };
    let registry = DecoderRegistry::with_settings(&DecoderSettings {
        soundfont: Some(corpus.path(font)),
        default_length_s: 10,
        fade_ms: 1000,
        ..DecoderSettings::default()
    });
    let mut table = Table::new("longtail", &["file", "result"]);
    for entry in files.iter().filter(|f| f.path != SOUNDFONT) {
        match check(&registry, &corpus, entry) {
            Ok(summary) => table.row(vec![entry.display(), summary], None),
            Err(problem) => table.row(vec![entry.display(), String::new()], Some(problem)),
        }
    }
    table.finish();
}
