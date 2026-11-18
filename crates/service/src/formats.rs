// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What this build can play: the decoders it was built with and the families of
//! formats each of them reads, for the "Supported formats" page and for bug
//! reports.
//!
//! The list is read from the engine's decoder registry when it is asked for, so a
//! package built without a decoder shows that decoder as absent. Front ends name
//! the families and decoders in their own language; this module gives stable
//! identifiers and the file extensions, which need no translation.

use helikodu_engine::decode::DecoderRegistry;

/// A decoder library or reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Decoder {
    /// FFmpeg, through the bridge: most formats.
    Ffmpeg,
    /// The DSF and DFF reader.
    Dsd,
    /// libopenmpt: tracker modules.
    Openmpt,
    /// libgme: game music.
    Gme,
    /// libsidplayfp: C64 SID tunes.
    Sidplayfp,
    /// vgmstream: game audio streams.
    Vgmstream,
    /// rustysynth: MIDI with a SoundFont.
    Rustysynth,
}

impl Decoder {
    /// Every decoder, in the order the page lists them.
    pub const ALL: [Self; 7] = [
        Self::Ffmpeg,
        Self::Dsd,
        Self::Openmpt,
        Self::Gme,
        Self::Sidplayfp,
        Self::Vgmstream,
        Self::Rustysynth,
    ];

    /// The library's own name, which is not translated.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "FFmpeg",
            Self::Dsd => "DSF/DFF",
            Self::Openmpt => "libopenmpt",
            Self::Gme => "libgme",
            Self::Sidplayfp => "libsidplayfp",
            Self::Vgmstream => "vgmstream",
            Self::Rustysynth => "rustysynth",
        }
    }

    /// The name of the engine's backend for this decoder.
    const fn backend(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Dsd => "dsd",
            Self::Openmpt => "openmpt",
            Self::Gme => "gme",
            Self::Sidplayfp => "sidplayfp",
            Self::Vgmstream => "vgmstream",
            Self::Rustysynth => "rustysynth",
        }
    }
}

/// A family of formats, as the page groups them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum FormatFamily {
    /// MP3, AAC, Vorbis, Opus and the like.
    MainstreamLossy,
    /// FLAC, ALAC, WAV, AIFF.
    Lossless,
    /// WavPack, Monkey's Audio, TAK, TTA, TrueHD, DTS-HD MA.
    ExtendedLossless,
    /// Musepack, WMA, AC-3, DTS, ATRAC, AMR.
    LegacyAndSpeech,
    /// Speex.
    Speex,
    /// RF64, BW64, Wave64, CAF, AU.
    LargePcmContainers,
    /// The audio of MP4, MKV, WebM, AVI, MOV and MPEG-TS files.
    VideoAudio,
    /// DSF and DFF.
    Dsd,
    /// MOD, S3M, XM, IT, MPTM and more.
    TrackerModules,
    /// NSF, SPC, GBS, VGM, AY, HES, KSS, SAP.
    GameMusic,
    /// PSID and RSID.
    Sid,
    /// ADX, HCA, BRSTM, WEM, FSB, VAG and many more.
    GameAudioStreams,
    /// Standard MIDI files.
    Midi,
}

impl FormatFamily {
    /// The formats of the family, as their usual names or extensions. Not
    /// translated.
    pub const fn formats(self) -> &'static [&'static str] {
        match self {
            Self::MainstreamLossy => &["MP3", "MP2", "AAC", "HE-AAC", "Vorbis", "Opus"],
            Self::Lossless => &["FLAC", "ALAC", "WAV", "AIFF"],
            Self::ExtendedLossless => &["WavPack", "APE", "TAK", "TTA", "TrueHD", "DTS-HD MA"],
            Self::LegacyAndSpeech => &["Musepack", "WMA", "AC-3", "E-AC-3", "DTS", "ATRAC", "AMR"],
            Self::Speex => &["SPX", "Speex in Ogg"],
            Self::LargePcmContainers => &["RF64", "BW64", "Wave64", "CAF", "AU"],
            Self::VideoAudio => &["MP4", "MKV", "WebM", "AVI", "MOV", "MPEG-TS"],
            Self::Dsd => &["DSF", "DFF"],
            Self::TrackerModules => &["MOD", "S3M", "XM", "IT", "MPTM"],
            Self::GameMusic => &["NSF", "SPC", "GBS", "VGM", "AY", "HES", "KSS", "SAP"],
            Self::Sid => &["PSID", "RSID"],
            Self::GameAudioStreams => &["ADX", "HCA", "BRSTM", "WEM", "FSB", "VAG"],
            Self::Midi => &["MID", "MIDI", "KAR", "RMI"],
        }
    }

    /// The decoders that read the family, the preferred one first.
    pub const fn decoders(self) -> &'static [Decoder] {
        match self {
            Self::Dsd => &[Decoder::Dsd, Decoder::Ffmpeg],
            Self::TrackerModules => &[Decoder::Openmpt],
            Self::GameMusic => &[Decoder::Gme],
            Self::Sid => &[Decoder::Sidplayfp],
            Self::GameAudioStreams => &[Decoder::Vgmstream],
            Self::Midi => &[Decoder::Rustysynth],
            _ => &[Decoder::Ffmpeg],
        }
    }

    /// Every family, in the order the page lists them.
    pub const ALL: [Self; 13] = [
        Self::MainstreamLossy,
        Self::Lossless,
        Self::ExtendedLossless,
        Self::LegacyAndSpeech,
        Self::Speex,
        Self::LargePcmContainers,
        Self::VideoAudio,
        Self::Dsd,
        Self::TrackerModules,
        Self::GameMusic,
        Self::Sid,
        Self::GameAudioStreams,
        Self::Midi,
    ];
}

/// Whether a decoder or a family can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Availability {
    /// It plays.
    Available,
    /// This build was made without it.
    NotBuilt,
}

/// One decoder of the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecoderStatus {
    /// The decoder.
    pub decoder: Decoder,
    /// Whether it is in the build.
    pub availability: Availability,
    /// Its version, when it reports one.
    pub version: Option<String>,
}

/// One family of formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyStatus {
    /// The family.
    pub family: FormatFamily,
    /// The decoder that reads it, or the one it would need.
    pub decoder: Decoder,
    /// Whether it plays.
    pub availability: Availability,
}

/// What this build can play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatReport {
    /// Every decoder, in the order of [`Decoder::ALL`].
    pub decoders: Vec<DecoderStatus>,
    /// Every family, in the order of [`FormatFamily::ALL`].
    pub families: Vec<FamilyStatus>,
}

impl FormatReport {
    /// The families that play.
    pub fn available(&self) -> usize {
        self.families
            .iter()
            .filter(|f| f.availability == Availability::Available)
            .count()
    }
}

/// What this build can play, from the engine's decoder registry.
pub fn supported_formats() -> FormatReport {
    let registry = DecoderRegistry::with_default_backends();
    let built: Vec<&str> = registry.factories().map(|f| f.name()).collect();
    report(&built)
}

fn report(built: &[&str]) -> FormatReport {
    let has = |decoder: Decoder| built.contains(&decoder.backend());
    let decoders = Decoder::ALL
        .iter()
        .map(|&decoder| DecoderStatus {
            decoder,
            availability: if has(decoder) {
                Availability::Available
            } else {
                Availability::NotBuilt
            },
            version: None,
        })
        .collect();
    let families = FormatFamily::ALL
        .iter()
        .map(|&family| {
            let candidates = family.decoders();
            let found = candidates.iter().copied().find(|&d| has(d));
            FamilyStatus {
                family,
                decoder: found.unwrap_or(candidates[0]),
                availability: if found.is_some() {
                    Availability::Available
                } else {
                    Availability::NotBuilt
                },
            }
        })
        .collect();
    FormatReport { decoders, families }
}

/// The version of the SQLite library the library database uses.
pub fn sqlite_version() -> &'static str {
    helikodu_library::raw::rusqlite::version()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_follow_the_built_decoders() {
        let both = report(&["dsd", "ffmpeg"]);
        assert_eq!(both.decoders.len(), Decoder::ALL.len());
        let dsd = both
            .families
            .iter()
            .find(|f| f.family == FormatFamily::Dsd)
            .unwrap();
        assert_eq!(dsd.decoder, Decoder::Dsd);
        let midi = both
            .families
            .iter()
            .find(|f| f.family == FormatFamily::Midi)
            .unwrap();
        assert_eq!(midi.availability, Availability::NotBuilt);
        assert_eq!(both.available(), 8);

        let only_ffmpeg = report(&["ffmpeg"]);
        let dsd = only_ffmpeg
            .families
            .iter()
            .find(|f| f.family == FormatFamily::Dsd)
            .unwrap();
        assert_eq!(dsd.decoder, Decoder::Ffmpeg);
        assert_eq!(dsd.availability, Availability::Available);
    }

    #[test]
    fn this_build_has_ffmpeg() {
        let report = supported_formats();
        assert!(
            report
                .decoders
                .iter()
                .any(|d| d.decoder == Decoder::Ffmpeg && d.availability == Availability::Available)
        );
        assert!(!sqlite_version().is_empty());
    }
}
