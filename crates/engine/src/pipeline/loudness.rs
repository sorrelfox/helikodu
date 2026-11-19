// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Loudness normalization: ReplayGain and Opus R128 gains, and the gain stage.
//!
//! Every gain is expressed relative to the ReplayGain 2.0 reference of −18 LUFS.
//! Opus files carry `R128_TRACK_GAIN` and `R128_ALBUM_GAIN` instead: signed Q7.8
//! integers (dB × 256) relative to −23 LUFS, so they become
//! `r128 / 256 + 5` dB ([`r128_to_replaygain_db`]). The output gain in the Opus header
//! is applied by the decoder, not here, so it is applied exactly once.
//!
//! [`resolve_gain`] chooses the gain for a track from its [`LoudnessInfo`] and the
//! [`LoudnessSettings`]; [`GainStage`] applies it, ramping when it changes.

use std::fmt;
use std::time::Duration;

use super::ramp::{SmoothedGain, frames_in};

/// The ReplayGain 2.0 reference loudness, in LUFS.
pub const REFERENCE_LUFS: f64 = -18.0;

/// The EBU R128 reference loudness that Opus gain tags are relative to, in LUFS.
pub const R128_REFERENCE_LUFS: f64 = -23.0;

/// An Opus `R128_*_GAIN` value (Q7.8 fixed point, relative to −23 LUFS) as a
/// ReplayGain 2.0 gain in dB (relative to −18 LUFS).
pub fn r128_to_replaygain_db(q7_8: i16) -> f64 {
    f64::from(q7_8) / 256.0 + (REFERENCE_LUFS - R128_REFERENCE_LUFS)
}

/// A ReplayGain 2.0 gain in dB as the value of an Opus `R128_*_GAIN` tag, rounded to
/// the nearest step of 1/256 dB and clamped to the tag's range.
pub fn replaygain_db_to_r128(db: f64) -> i16 {
    let q = ((db - (REFERENCE_LUFS - R128_REFERENCE_LUFS)) * 256.0).round();
    q.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

/// A gain in dB as a linear amplitude factor.
pub fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// A linear amplitude factor as a gain in dB.
pub fn linear_to_db(linear: f64) -> f64 {
    20.0 * linear.log10()
}

/// Reads a `REPLAYGAIN_*_GAIN` value such as `-6.30 dB`, `+1.2 dB` or `-3.5`.
pub fn parse_gain_db(text: &str) -> Option<f64> {
    let text = text.trim();
    let number = text
        .strip_suffix("dB")
        .or_else(|| text.strip_suffix("db"))
        .or_else(|| text.strip_suffix("DB"))
        .unwrap_or(text)
        .trim();
    let value: f64 = number.strip_prefix('+').unwrap_or(number).parse().ok()?;
    // Real gains stay well inside ±100 dB; anything else is a broken tag.
    (value.is_finite() && value.abs() <= 100.0).then_some(value)
}

/// Reads a `REPLAYGAIN_*_PEAK` value: a linear peak such as `0.988525`.
pub fn parse_peak(text: &str) -> Option<f64> {
    let value: f64 = text.trim().parse().ok()?;
    (value.is_finite() && value > 0.0 && value < 100.0).then_some(value)
}

/// The loudness information of one track, from its tags or a scan.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LoudnessInfo {
    /// The track gain in dB relative to −18 LUFS.
    pub track_gain_db: Option<f64>,
    /// The track's linear sample peak, or true peak when [`true_peak`](Self::true_peak) is set.
    pub track_peak: Option<f64>,
    /// The album gain in dB relative to −18 LUFS.
    pub album_gain_db: Option<f64>,
    /// The album's linear peak.
    pub album_peak: Option<f64>,
    /// Whether the peaks are true peaks (measured with oversampling).
    pub true_peak: bool,
}

impl LoudnessInfo {
    /// The information carried by Opus `R128_TRACK_GAIN` and `R128_ALBUM_GAIN`
    /// tags. Opus has no peak tags.
    pub fn from_r128(track: Option<i16>, album: Option<i16>) -> Self {
        Self {
            track_gain_db: track.map(r128_to_replaygain_db),
            album_gain_db: album.map(r128_to_replaygain_db),
            ..Self::default()
        }
    }

    /// Whether any gain is known.
    pub fn has_gain(&self) -> bool {
        self.track_gain_db.is_some() || self.album_gain_db.is_some()
    }
}

/// Which gain is applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum LoudnessMode {
    /// No loudness normalization.
    #[default]
    Off,
    /// The track gain, or the album gain if the track has none.
    Track,
    /// The album gain, or the track gain if the track has none.
    Album,
    /// Album gain while consecutive tracks come from the same album in album order,
    /// track gain otherwise (for example in shuffle).
    Auto,
}

/// How clipping caused by a positive gain is prevented.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ClippingPrevention {
    /// The gain is reduced so that `peak × gain ≤ 1.0`, where the peak is known.
    #[default]
    Peak,
    /// The gain is left as it is; a limiter later in the chain catches the peaks.
    Limiter,
}

/// The loudness settings.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LoudnessSettings {
    /// Which gain is applied.
    pub mode: LoudnessMode,
    /// Added to the gain of tracks that have one, in dB.
    pub preamp_db: f64,
    /// The gain of tracks without loudness information, in dB.
    pub untagged_preamp_db: f64,
    /// How clipping is prevented.
    pub clipping: ClippingPrevention,
    /// Whether tracks without loudness information are evened out by the
    /// [`Leveller`](super::Leveller) instead of the untagged preamp.
    pub leveller: bool,
}

impl From<helikodu_model::settings::LoudnessSettings> for LoudnessSettings {
    /// The `[loudness]` settings as the path plan takes them.
    fn from(settings: helikodu_model::settings::LoudnessSettings) -> Self {
        use helikodu_model::settings::{ClippingPrevention as Clipping, LoudnessMode as Mode};
        Self {
            mode: match settings.mode {
                Mode::Off => LoudnessMode::Off,
                Mode::Track => LoudnessMode::Track,
                Mode::Album => LoudnessMode::Album,
                Mode::Auto => LoudnessMode::Auto,
            },
            preamp_db: settings.preamp_db,
            untagged_preamp_db: settings.untagged_preamp_db,
            clipping: match settings.clipping {
                Clipping::LowerGain => ClippingPrevention::Peak,
                Clipping::Limiter => ClippingPrevention::Limiter,
            },
            leveller: settings.leveller,
        }
    }
}

/// Where a track's gain came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GainSource {
    /// The track gain.
    Track,
    /// The album gain.
    Album,
    /// The track has no loudness information; the gain is the untagged preamp.
    Untagged,
    /// Loudness normalization is off; the gain is the track's own volume
    /// adjustment.
    TrackVolume,
    /// The track has no loudness information and the leveller evens it out; the
    /// gain is the track's own volume adjustment, and the leveller rides on it.
    Leveller,
}

/// The gain chosen for one track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainDecision {
    /// The gain in dB, after the preamp and any clipping prevention.
    pub db: f64,
    /// Where it came from.
    pub source: GainSource,
    /// Whether clipping prevention lowered it.
    pub peak_limited: bool,
    /// The track's own volume adjustment included in `db`, in dB.
    pub track_volume_db: f64,
}

impl GainDecision {
    /// The gain as a linear amplitude factor.
    pub fn linear(&self) -> f64 {
        db_to_linear(self.db)
    }

    /// Whether the gain changes the samples at all.
    pub fn is_unity(&self) -> bool {
        self.db == 0.0
    }
}

impl fmt::Display for GainDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = match self.source {
            GainSource::Track => "track",
            GainSource::Album => "album",
            GainSource::Untagged => "untagged",
            GainSource::Leveller => {
                f.write_str("leveller (no loudness values")?;
                if self.track_volume_db != 0.0 {
                    write!(f, ", track volume {:+.1} dB", self.track_volume_db)?;
                }
                if self.peak_limited {
                    f.write_str(", peak-limited")?;
                }
                return f.write_str(")");
            }
            GainSource::TrackVolume => {
                write!(f, "track volume {:+.2} dB", self.db)?;
                if self.peak_limited {
                    f.write_str(" (peak-limited)")?;
                }
                return Ok(());
            }
        };
        write!(f, "ReplayGain {:+.2} dB ({source}", self.db)?;
        if self.track_volume_db != 0.0 {
            write!(f, ", track volume {:+.1} dB", self.track_volume_db)?;
        }
        if self.peak_limited {
            f.write_str(", peak-limited")?;
        }
        f.write_str(")")
    }
}

/// Chooses the gain for a track, or `None` when loudness normalization is off.
///
/// `album_context` is whether the track plays as part of its album (the previous
/// queue item is the previous track of the same album); it decides
/// [`LoudnessMode::Auto`].
pub fn resolve_gain(
    info: &LoudnessInfo,
    settings: &LoudnessSettings,
    album_context: bool,
) -> Option<GainDecision> {
    resolve_track_gain(info, settings, album_context, 0.0)
}

/// [`resolve_gain`] with the track's own volume adjustment `track_volume_db` added
/// before clipping prevention. With loudness normalization off, the adjustment
/// alone is the gain; `None` when that is zero too.
pub fn resolve_track_gain(
    info: &LoudnessInfo,
    settings: &LoudnessSettings,
    album_context: bool,
    track_volume_db: f64,
) -> Option<GainDecision> {
    let track_volume_db = if track_volume_db.is_finite() {
        track_volume_db
    } else {
        0.0
    };
    let prefer_album = match settings.mode {
        LoudnessMode::Off if track_volume_db == 0.0 => return None,
        LoudnessMode::Off => {
            let peak = info.track_peak.or(info.album_peak);
            let (db, peak_limited) = limit_by_peak(track_volume_db, peak, settings);
            return Some(GainDecision {
                db,
                source: GainSource::TrackVolume,
                peak_limited,
                track_volume_db,
            });
        }
        LoudnessMode::Track => false,
        LoudnessMode::Album => true,
        LoudnessMode::Auto => album_context,
    };
    let track = info
        .track_gain_db
        .map(|db| (db, info.track_peak, GainSource::Track));
    let album = info
        .album_gain_db
        .map(|db| (db, info.album_peak, GainSource::Album));
    let chosen = if prefer_album {
        album.or(track)
    } else {
        track.or(album)
    };
    let Some((gain, peak, source)) = chosen else {
        if settings.leveller {
            // The leveller replaces the untagged preamp; the track volume stays.
            let peak = info.track_peak.or(info.album_peak);
            let (db, peak_limited) = limit_by_peak(track_volume_db, peak, settings);
            return Some(GainDecision {
                db,
                source: GainSource::Leveller,
                peak_limited,
                track_volume_db,
            });
        }
        let db = settings.untagged_preamp_db + track_volume_db;
        // The untagged preamp alone is not limited; a track volume is, where a
        // peak is known.
        let peak = info.track_peak.or(info.album_peak);
        let (db, peak_limited) = if track_volume_db == 0.0 {
            (db, false)
        } else {
            limit_by_peak(db, peak, settings)
        };
        return Some(GainDecision {
            db,
            source: GainSource::Untagged,
            peak_limited,
            track_volume_db,
        });
    };
    // A track without its own peak may still have the album's; the album peak is
    // never below the track's, so it is a safe limit.
    let peak = peak.or(info.album_peak);
    let (db, peak_limited) =
        limit_by_peak(gain + settings.preamp_db + track_volume_db, peak, settings);
    Some(GainDecision {
        db,
        source,
        peak_limited,
        track_volume_db,
    })
}

/// `db`, lowered so that `peak` stays at full scale when the settings prevent
/// clipping by the peak, and whether it was lowered.
fn limit_by_peak(db: f64, peak: Option<f64>, settings: &LoudnessSettings) -> (f64, bool) {
    if settings.clipping == ClippingPrevention::Peak
        && let Some(peak) = peak
    {
        let limit = -linear_to_db(peak);
        if db > limit {
            return (limit, true);
        }
    }
    (db, false)
}

/// Applies a track's gain to interleaved `f32` frames, in 64-bit arithmetic.
///
/// A new gain (for example after a settings change) is reached over a short ramp.
#[derive(Debug, Clone)]
pub struct GainStage {
    channels: usize,
    gain: SmoothedGain,
}

impl GainStage {
    /// How long a gain change takes.
    pub const RAMP: Duration = Duration::from_millis(20);

    /// A stage for `channels` channels at `rate` frames per second, starting at the
    /// linear gain `linear`.
    pub fn new(rate: u32, channels: usize, linear: f64) -> Self {
        Self {
            channels,
            gain: SmoothedGain::new(linear, frames_in(Self::RAMP, rate)),
        }
    }

    /// The linear gain the stage rests at or moves to.
    pub fn gain(&self) -> f64 {
        self.gain.target()
    }

    /// Moves to the linear gain `linear`.
    pub fn set_gain(&mut self, linear: f64) {
        self.gain.set_target(linear);
    }

    /// Applies the gain to `block`.
    pub fn apply(&mut self, block: &mut [f32]) {
        self.gain.apply(block, self.channels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn r128_converts_to_the_replaygain_reference() {
        // 0 dB relative to −23 LUFS is +5 dB relative to −18 LUFS.
        assert!(close(r128_to_replaygain_db(0), 5.0));
        // −2560 / 256 = −10 dB, so −5 dB on the ReplayGain scale.
        assert!(close(r128_to_replaygain_db(-2560), -5.0));
        assert!(close(r128_to_replaygain_db(384), 6.5));
        assert!(close(r128_to_replaygain_db(-1), 5.0 - 1.0 / 256.0));
        for q in [-32768i16, -2560, -1, 0, 1, 777, 32767] {
            assert_eq!(replaygain_db_to_r128(r128_to_replaygain_db(q)), q);
        }
        assert_eq!(replaygain_db_to_r128(500.0), i16::MAX);
        let info = LoudnessInfo::from_r128(Some(-2560), None);
        assert_eq!(info.track_gain_db, Some(-5.0));
        assert!(info.has_gain());
    }

    #[test]
    fn decibels_and_linear() {
        assert!(close(db_to_linear(0.0), 1.0));
        assert!(close(db_to_linear(-6.0), 0.501_187_233_627_272_3));
        assert!(close(db_to_linear(20.0), 10.0));
        assert!(close(linear_to_db(0.5), -6.020_599_913_279_624));
    }

    #[test]
    fn parses_tag_values() {
        assert_eq!(parse_gain_db("-6.30 dB"), Some(-6.3));
        assert_eq!(parse_gain_db(" +1.25 dB "), Some(1.25));
        assert_eq!(parse_gain_db("-3.5"), Some(-3.5));
        assert_eq!(parse_gain_db("2 db"), Some(2.0));
        assert_eq!(parse_gain_db("loud"), None);
        assert_eq!(parse_gain_db("NaN dB"), None);
        assert_eq!(parse_peak("0.988525"), Some(0.988_525));
        assert_eq!(parse_peak("-1"), None);
    }

    fn tagged() -> LoudnessInfo {
        LoudnessInfo {
            track_gain_db: Some(-6.3),
            track_peak: Some(0.9),
            album_gain_db: Some(-7.5),
            album_peak: Some(0.99),
            true_peak: false,
        }
    }

    fn settings(mode: LoudnessMode) -> LoudnessSettings {
        LoudnessSettings {
            mode,
            ..LoudnessSettings::default()
        }
    }

    #[test]
    fn modes_choose_track_or_album() {
        let info = tagged();
        assert_eq!(
            resolve_gain(&info, &settings(LoudnessMode::Off), true),
            None
        );
        let track = resolve_gain(&info, &settings(LoudnessMode::Track), true).unwrap();
        assert_eq!((track.db, track.source), (-6.3, GainSource::Track));
        let album = resolve_gain(&info, &settings(LoudnessMode::Album), false).unwrap();
        assert_eq!((album.db, album.source), (-7.5, GainSource::Album));
        let auto = resolve_gain(&info, &settings(LoudnessMode::Auto), true).unwrap();
        assert_eq!(auto.source, GainSource::Album);
        let auto = resolve_gain(&info, &settings(LoudnessMode::Auto), false).unwrap();
        assert_eq!(auto.source, GainSource::Track);
    }

    #[test]
    fn falls_back_to_the_other_gain_and_to_the_untagged_preamp() {
        let only_track = LoudnessInfo {
            album_gain_db: None,
            album_peak: None,
            ..tagged()
        };
        let gain = resolve_gain(&only_track, &settings(LoudnessMode::Album), true).unwrap();
        assert_eq!(gain.source, GainSource::Track);

        let untagged = LoudnessSettings {
            mode: LoudnessMode::Track,
            preamp_db: 3.0,
            untagged_preamp_db: -4.0,
            ..LoudnessSettings::default()
        };
        let gain = resolve_gain(&LoudnessInfo::default(), &untagged, false).unwrap();
        assert_eq!((gain.db, gain.source), (-4.0, GainSource::Untagged));

        // The leveller takes the place of the untagged preamp, and only there.
        let levelled = LoudnessSettings {
            leveller: true,
            ..untagged
        };
        let gain = resolve_gain(&LoudnessInfo::default(), &levelled, false).unwrap();
        assert_eq!((gain.db, gain.source), (0.0, GainSource::Leveller));
        assert_eq!(gain.to_string(), "leveller (no loudness values)");
        let gain = resolve_gain(&tagged(), &levelled, false).unwrap();
        assert_eq!(gain.source, GainSource::Track);
        let off = LoudnessSettings {
            mode: LoudnessMode::Off,
            ..levelled
        };
        assert_eq!(resolve_gain(&LoudnessInfo::default(), &off, false), None);
    }

    #[test]
    fn preamp_is_added_and_clipping_is_prevented_by_the_peak() {
        let info = LoudnessInfo {
            track_gain_db: Some(2.0),
            track_peak: Some(0.9),
            ..LoudnessInfo::default()
        };
        let mut s = LoudnessSettings {
            mode: LoudnessMode::Track,
            preamp_db: 1.0,
            ..LoudnessSettings::default()
        };
        let gain = resolve_gain(&info, &s, false).unwrap();
        // 3 dB would push a peak of 0.9 to 1.27; the limit is 1 / 0.9.
        assert!(gain.peak_limited);
        assert!(close(gain.linear() * 0.9, 1.0));

        s.clipping = ClippingPrevention::Limiter;
        let gain = resolve_gain(&info, &s, false).unwrap();
        assert!(!gain.peak_limited);
        assert!(close(gain.db, 3.0));

        // A negative gain that keeps the peak below full scale is not touched.
        let quiet = LoudnessInfo {
            track_gain_db: Some(-6.0),
            track_peak: Some(1.5),
            ..LoudnessInfo::default()
        };
        s.clipping = ClippingPrevention::Peak;
        let gain = resolve_gain(&quiet, &s, false).unwrap();
        assert!(!gain.peak_limited);
        assert!(close(gain.db, -5.0));
    }

    #[test]
    fn gain_stage_applies_and_ramps() {
        let mut stage = GainStage::new(1000, 2, db_to_linear(-6.0));
        let mut block = [1.0f32, 0.5];
        stage.apply(&mut block);
        assert!((f64::from(block[0]) - 0.501_187).abs() < 1e-6);
        stage.set_gain(1.0);
        let mut block = vec![1.0f32; 2 * 40];
        stage.apply(&mut block);
        assert!(block[0] < 0.6 && block[0] == block[1]);
        assert_eq!(block[78], 1.0);
    }

    #[test]
    fn the_track_volume_is_added_before_clipping_prevention() {
        let info = LoudnessInfo {
            track_gain_db: Some(-6.0),
            track_peak: Some(0.5),
            ..LoudnessInfo::default()
        };
        let track = settings(LoudnessMode::Track);
        let gain = resolve_track_gain(&info, &track, false, 3.5).unwrap();
        assert!(close(gain.db, -2.5) && !gain.peak_limited);
        assert_eq!(
            gain.to_string(),
            "ReplayGain -2.50 dB (track, track volume +3.5 dB)"
        );
        // A peak of 0.5 leaves 6.02 dB of room.
        let gain = resolve_track_gain(&info, &track, false, 13.0).unwrap();
        assert!(gain.peak_limited && close(gain.linear() * 0.5, 1.0));
        // Normalization off: the adjustment alone, limited the same way.
        let off = settings(LoudnessMode::Off);
        assert_eq!(resolve_track_gain(&info, &off, false, 0.0), None);
        let gain = resolve_track_gain(&info, &off, false, -4.0).unwrap();
        assert_eq!((gain.db, gain.source), (-4.0, GainSource::TrackVolume));
        assert_eq!(gain.to_string(), "track volume -4.00 dB");
        let gain = resolve_track_gain(&info, &off, false, 9.0).unwrap();
        assert!(gain.peak_limited && close(gain.linear() * 0.5, 1.0));
    }

    #[test]
    fn display_names_the_source() {
        let gain = GainDecision {
            db: -6.3,
            source: GainSource::Album,
            peak_limited: true,
            track_volume_db: 0.0,
        };
        assert_eq!(
            gain.to_string(),
            "ReplayGain -6.30 dB (album, peak-limited)"
        );
    }
}
