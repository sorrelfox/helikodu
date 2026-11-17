// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Every sample of the formats set opens and decodes to the rate, channels and
//! length its manifest entry expects.

use crate::decoding;
use crate::manifest::{Corpus, Entry};
use crate::report::Table;

/// How far a length may be off when the format does not state it exactly (no
/// encoder delay or padding information): the codec's frame and delay, at most
/// this fraction of the source plus this many seconds.
const LOOSE_FRACTION: f64 = 0.02;
const LOOSE_SECONDS: f64 = 0.1;

/// Formats of the corpus the engine does not play yet, with the reason. They are
/// reported as gaps, not failures, unless `HELIKODU_CORPUS_STRICT` is set; one that
/// starts to play is reported so it can be taken off the list.
pub(crate) const KNOWN_GAPS: &[(&str, &str)] = &[
    ("formats/pcm/ircam.sf", NOT_WHITELISTED),
    ("formats/pcm/nist-sphere.sph", NOT_WHITELISTED),
    ("formats/pcm/avr.avr", NOT_WHITELISTED),
    ("formats/pcm/hcom.hcom", NOT_WHITELISTED),
    ("formats/pcm/smaf.mmf", NOT_WHITELISTED),
    ("formats/pcm/adx.adx", NOT_WHITELISTED),
    ("formats/pcm/westwood.aud", NOT_WHITELISTED),
    ("formats/lossy/lc3.lc3", NOT_WHITELISTED),
    ("formats/speech/ilbc.lbc", NOT_WHITELISTED),
    ("formats/speech/codec2.c2", NOT_WHITELISTED),
    ("formats/lossy/sbc.sbc", HEADERLESS),
    ("formats/lossy/aptx.aptx", HEADERLESS),
    ("formats/lossy/aptx-hd.aptxhd", HEADERLESS),
    ("formats/lossy/dfpwm.dfpwm", HEADERLESS),
    ("formats/speech/g722.g722", HEADERLESS),
    ("formats/speech/g723_1.tco", HEADERLESS),
];

const NOT_WHITELISTED: &str = "demuxer not on the ffmpeg-bridge format whitelist";
const HEADERLESS: &str =
    "raw stream without a header: FFmpeg recognises it only by its file name extension";

/// Whether gaps fail the tests.
pub(crate) fn strict() -> bool {
    std::env::var_os("HELIKODU_CORPUS_STRICT").is_some()
}

/// The reason `entry` is a known gap, if it is one.
pub(crate) fn known_gap(entry: &Entry) -> Option<&'static str> {
    KNOWN_GAPS
        .iter()
        .find(|(path, _)| *path == entry.path)
        .map(|(_, reason)| *reason)
}

/// The samples to decode: every file with a format label (a WavPack correction
/// file has none; it is read with its `.wv`).
pub(crate) fn samples(corpus: &Corpus) -> Vec<Entry> {
    corpus
        .set("formats")
        .into_iter()
        .filter(|entry| entry.format.is_some())
        .collect()
}

#[test]
fn every_format_decodes_to_its_expected_length() {
    let Some(corpus) = Corpus::load("formats") else {
        return;
    };
    let mut table = Table::new(
        "formats",
        &[
            "file", "format", "codec", "rate", "ch", "frames", "expected",
        ],
    );
    for entry in samples(&corpus) {
        let expect = entry.expect();
        let format = entry.format.clone().unwrap_or_default();
        let gap = known_gap(&entry);
        let decoded = match decoding::decode(&corpus.path(&entry)) {
            Ok(decoded) => decoded,
            Err(error) => {
                let mut cells = vec![entry.display(), format, "-".into()];
                cells.extend(["-", "-", "-", "-"].map(String::from));
                match gap {
                    Some(reason) if !strict() => table.gap(cells, reason),
                    _ => table.row(cells, Some(error.to_string())),
                }
                continue;
            }
        };
        if gap.is_some() {
            println!(
                "{} plays now: take it off KNOWN_GAPS in tests/corpus/formats.rs",
                entry.path
            );
        }
        let frames = decoded.frames() as u64;
        let mut problems = Vec::new();
        if let Some(rate) = expect.sample_rate
            && rate != decoded.rate
        {
            problems.push(format!("rate {} instead of {rate}", decoded.rate));
        }
        if let Some(channels) = expect.channels
            && usize::from(channels) != decoded.channels
        {
            problems.push(format!(
                "{} channels instead of {channels}",
                decoded.channels
            ));
        }
        let expected = match (expect.samples, corpus.source_seconds(&entry)) {
            (Some(samples), _) => {
                if frames != samples {
                    problems.push(format!("{frames} frames instead of {samples}"));
                }
                samples.to_string()
            }
            (None, Some(seconds)) => {
                let want = seconds * f64::from(decoded.rate);
                let slack = want * LOOSE_FRACTION + LOOSE_SECONDS * f64::from(decoded.rate);
                if (frames as f64 - want).abs() > slack {
                    problems.push(format!(
                        "{frames} frames, not within {slack:.0} of {want:.0}"
                    ));
                }
                format!("~{want:.0}")
            }
            (None, None) => {
                if frames == 0 {
                    problems.push("no audio".into());
                }
                "> 0".into()
            }
        };
        table.row(
            vec![
                entry.display(),
                format,
                decoded.codec.clone(),
                decoded.rate.to_string(),
                decoded.channels.to_string(),
                frames.to_string(),
                expected,
            ],
            (!problems.is_empty()).then(|| problems.join("; ")),
        );
    }
    table.finish();
}
