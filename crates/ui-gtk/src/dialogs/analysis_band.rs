// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The "Analysis" band of the tag editor's File info tab: what the track
//! analysis found for the track, so that a user can see why a crossfade starts
//! where it does. Under the section label, either the loudness curve with three
//! facts under it (where the sound is, the silences inside, the fade-out), or
//! one sentence: not analysed yet, or the file could not be read. Nothing here
//! can be changed, and nothing is part of "Save".

use std::time::Duration;

use gtk::prelude::*;

use crate::backend::services::AnalysisFacts;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::loudness_curve::LoudnessCurve;
use crate::widgets::{label, wrapped_label};

/// The seconds each bar of the curve stands for.
const BAR_SECONDS: f64 = 2.0;
/// How far under the loudest bar a bar can be and still be drawn above the
/// lowest height, in LU.
const RANGE_LU: f64 = 24.0;
/// The silent stretches the facts name one by one; the rest are counted.
const NAMED_SILENCES: usize = 3;
/// A fade-out shorter than this is no fade: the analysis puts the fade point
/// half a second inside the sound when there is none.
const NO_FADE_SECONDS: f64 = 1.0;

/// A position as minutes, seconds and hundredths: `5:24.10`.
pub(crate) fn precise(seconds: f64) -> String {
    let hundredths = (seconds.max(0.0) * 100.0).round() as u64;
    let (s, cs) = (hundredths / 100, hundredths % 100);
    if s >= 3600 {
        format!("{}:{:02}:{:02}.{cs:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}.{cs:02}", s / 60, s % 60)
    }
}

/// A position in whole seconds: `2:10`.
fn whole(seconds: f64) -> String {
    crate::format::duration(Duration::from_secs_f64(seconds.max(0.0).floor()))
}

/// The facts of one analysis, in words.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Facts {
    /// Where the sound starts and ends.
    pub(crate) sound: String,
    /// The silent stretches inside the sound.
    pub(crate) silences: String,
    /// Where the fade-out starts.
    pub(crate) fade: String,
    /// What the curve shows, for its accessible name.
    pub(crate) picture: String,
}

/// The facts of `analysis` in words.
pub(crate) fn facts(analysis: &helikodu_model::analysis::TrackAnalysis) -> Facts {
    let rate = f64::from(analysis.rate.max(1));
    let at = |frame: u64| frame as f64 / rate;
    let sound = match &analysis.audio {
        Some(audio) => fill(
            &tr("{} to {}"),
            &[&precise(at(audio.start)), &precise(at(audio.end))],
        ),
        None => tr("None: the track is silent"),
    };
    let spans: Vec<String> = analysis
        .silences
        .iter()
        .map(|s| fill(&tr("{} to {}"), &[&whole(at(s.start)), &whole(at(s.end))]))
        .collect();
    let silences = match spans.as_slice() {
        [] => tr("None"),
        [_] => {
            let from = analysis.silences[0];
            fill(
                &tr("1, from {} to {}"),
                &[&whole(at(from.start)), &whole(at(from.end))],
            )
        }
        many => {
            let named = many[..many.len().min(NAMED_SILENCES)].join(", ");
            let rest = many.len().saturating_sub(NAMED_SILENCES);
            let list = if rest == 0 {
                named
            } else {
                fill(
                    &ntr("{} and {} more", "{} and {} more", rest),
                    &[&named, &rest.to_string()],
                )
            };
            fill(&tr("{}: {}"), &[&many.len().to_string(), &list])
        }
    };
    let fade_at = fade_start(analysis);
    let fade = match fade_at {
        Some(seconds) => fill(&tr("From {}, at the dashed line"), &[&whole(seconds)]),
        None => tr("None: the track ends at once"),
    };
    let mut picture = vec![tr("How loud the track is over its length.")];
    if let Some(first) = analysis.silences.first() {
        picture.push(fill(
            &tr("Silence from {} to {}."),
            &[&whole(at(first.start)), &whole(at(first.end))],
        ));
    }
    if let Some(seconds) = fade_at {
        picture.push(fill(&tr("The fade-out starts at {}."), &[&whole(seconds)]));
    }
    Facts {
        sound,
        silences,
        fade,
        picture: picture.join(" "),
    }
}

/// Where the fade-out starts, in seconds; `None` without a fade-out.
fn fade_start(analysis: &helikodu_model::analysis::TrackAnalysis) -> Option<f64> {
    let rate = f64::from(analysis.rate.max(1));
    let start = analysis.fade_out_start?;
    let end = analysis.audio.as_ref()?.end;
    (end.saturating_sub(start) as f64 / rate >= NO_FADE_SECONDS).then(|| start as f64 / rate)
}

/// The curve's bars, one for every two seconds of the track: each a share of
/// the loudest bar, `None` for silence (no sound in the envelope, before or
/// after the sound, or in a silent stretch).
pub(crate) fn bars(analysis: &helikodu_model::analysis::TrackAnalysis) -> Vec<Option<f32>> {
    let rate = f64::from(analysis.rate.max(1));
    let length = analysis.frames as f64 / rate;
    let count = (length / BAR_SECONDS).ceil().max(0.0) as usize;
    let step = f64::from(analysis.envelope.step_secs());
    let silent = |seconds: f64| {
        let frame = (seconds * rate) as u64;
        analysis
            .audio
            .as_ref()
            .is_none_or(|audio| frame < audio.start || frame >= audio.end)
            || analysis
                .silences
                .iter()
                .any(|s| frame >= s.start && frame < s.end)
    };
    let lufs: Vec<Option<f64>> = (0..count)
        .map(|i| {
            let from = i as f64 * BAR_SECONDS;
            let middle = from + BAR_SECONDS / 2.0;
            if silent(middle) {
                return None;
            }
            let first = (from / step).floor() as usize;
            let last = ((from + BAR_SECONDS) / step).ceil().max(first as f64 + 1.0) as usize;
            (first..last)
                .filter_map(|s| analysis.envelope.lufs(s))
                .reduce(f64::max)
        })
        .collect();
    let loudest = lufs.iter().flatten().copied().reduce(f64::max);
    lufs.into_iter()
        .map(|value| {
            let (value, loudest) = (value?, loudest?);
            Some((1.0 - (loudest - value) / RANGE_LU).clamp(0.0, 1.0) as f32)
        })
        .collect()
}

/// One fact: a `t3` label over its value.
fn fact(name: &str, value: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_hexpand(true);
    column.append(&label(name, &["hk-t3"]));
    let value = wrapped_label(value, &[]);
    value.set_widget_name("analysis-fact");
    column.append(&value);
    column
}

/// The band for a track with `analysis`.
pub(crate) fn band(analysis: &AnalysisFacts) -> gtk::Box {
    let band = gtk::Box::new(gtk::Orientation::Vertical, 8);
    band.add_css_class("hk-analysis-band");
    band.set_widget_name("analysis-band");
    band.append(&label(&tr("Analysis"), &["hk-section-label"]));
    match analysis {
        AnalysisFacts::Analysed(analysis) => {
            let facts = facts(analysis);
            let curve = LoudnessCurve::default();
            curve.set_widget_name("loudness-curve");
            let length = analysis.frames as f64;
            let fade = fade_start(analysis)
                .filter(|_| length > 0.0)
                .map(|seconds| (seconds * f64::from(analysis.rate.max(1)) / length) as f32);
            curve.set_curve(bars(analysis), fade, &facts.picture);
            band.append(&curve);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
            row.set_homogeneous(true);
            row.append(&fact(&tr("Sound"), &facts.sound));
            row.append(&fact(&tr("Silence inside"), &facts.silences));
            row.append(&fact(&tr("Fade-out"), &facts.fade));
            band.append(&row);
        }
        AnalysisFacts::NotYet => {
            let text = wrapped_label(
                &tr(
                    "Not analysed yet. Helikodu analyses tracks in the background; until then a \
                     crossfade uses its fixed length and no silence is left out of this track.",
                ),
                &["hk-t2"],
            );
            text.set_widget_name("analysis-note");
            text.set_max_width_chars(70);
            band.append(&text);
        }
        AnalysisFacts::Failed => {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
            icon.add_css_class("hk-t2");
            icon.set_valign(gtk::Align::Start);
            line.append(&icon);
            let text = wrapped_label(
                &tr(
                    "This file could not be read for analysis. Helikodu tries again when the \
                     file changes.",
                ),
                &["hk-t2"],
            );
            text.set_widget_name("analysis-note");
            text.set_max_width_chars(70);
            line.append(&text);
            band.append(&line);
        }
    }
    band
}

#[cfg(test)]
mod tests {
    use helikodu_model::analysis::{LoudnessEnvelope, SilentSpan, TrackAnalysis};

    use super::*;

    const RATE: u32 = 44_100;

    fn frames(seconds: f64) -> u64 {
        (seconds * f64::from(RATE)).round() as u64
    }

    /// The track of the design: 5:26, sound from 0:00.42 to 5:24.10, a silence
    /// from 2:10 to 2:22 and a fade-out from 5:09.
    fn track() -> TrackAnalysis {
        TrackAnalysis {
            rate: RATE,
            frames: frames(326.0),
            audio: Some(frames(0.42)..frames(324.10)),
            silences: vec![SilentSpan {
                start: frames(130.0),
                end: frames(142.0),
            }],
            fade_out_start: Some(frames(309.5)),
            envelope: LoudnessEnvelope::from_lufs(1, (0..326).map(|i| -14.0 - f64::from(i % 7))),
            ..TrackAnalysis::default()
        }
    }

    #[test]
    fn the_facts_read_as_the_design_has_them() {
        let facts = facts(&track());
        assert_eq!(facts.sound, "0:00.42 to 5:24.10");
        assert_eq!(facts.silences, "1, from 2:10 to 2:22");
        assert_eq!(facts.fade, "From 5:09, at the dashed line");
        assert_eq!(
            facts.picture,
            "How loud the track is over its length. Silence from 2:10 to 2:22. \
             The fade-out starts at 5:09."
        );
    }

    #[test]
    fn several_silences_name_three_and_count_the_rest() {
        let mut analysis = track();
        analysis.silences = (0..5)
            .map(|i| SilentSpan {
                start: frames(20.0 + 20.0 * f64::from(i)),
                end: frames(23.0 + 20.0 * f64::from(i)),
            })
            .collect();
        assert_eq!(
            facts(&analysis).silences,
            "5: 0:20 to 0:23, 0:40 to 0:43, 1:00 to 1:03 and 2 more"
        );
        analysis.silences.clear();
        assert_eq!(facts(&analysis).silences, "None");
    }

    #[test]
    fn a_fade_point_at_the_end_of_the_sound_is_no_fade() {
        let mut analysis = track();
        analysis.fade_out_start = Some(frames(323.6));
        assert_eq!(facts(&analysis).fade, "None: the track ends at once");
    }

    #[test]
    fn one_bar_for_every_two_seconds_with_silence_flat() {
        let bars = bars(&track());
        assert_eq!(bars.len(), 163);
        // 2:10 to 2:22 is silent: the bars of 130 s to 142 s.
        assert!(bars[65..71].iter().all(Option::is_none));
        assert!(bars[64].is_some() && bars[71].is_some());
        // The last bar lies after the sound.
        assert!(bars[162].is_none());
        assert!(
            bars.iter()
                .flatten()
                .any(|b| (*b - 1.0).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn positions_have_hundredths() {
        assert_eq!(precise(0.42), "0:00.42");
        assert_eq!(precise(3725.5), "1:02:05.50");
    }
}
