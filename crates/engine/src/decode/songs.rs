// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The synthesizing backends of `helikodu-extra-decoders`: tracker modules
//! (libopenmpt), game music (libgme), SID tunes (libsidplayfp), game audio streams
//! (vgmstream) and MIDI files (rustysynth).
//!
//! Each recognises its files by content: libopenmpt's and libgme's own header
//! probes, the `PSID`/`RSID` and `MThd`/`RMID` signatures. vgmstream is the
//! exception: most of its formats have no signature, so it scores files by name
//! ([`ProbeScore::EXTENSION`]) and checks the content when it opens them; a file it
//! does not recognise falls through to FFmpeg. Subsongs are the files' own
//! (module subsongs, game music tracks, SID songs, bank streams). Blocks are
//! interleaved `f32` at the decoder's rate (44.1 kHz for the synthesized formats).

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use helikodu_extra_decoders::song::{SongDecoder, SongError, SongOptions};
use helikodu_model::audio::{AudioSpec, ChannelLayout, MediaTime, SampleRate};

use super::block::{BlockSamples, PcmBlock};
use super::decoder::{Decoder, DecoderInfo};
use super::error::DecodeError;
use super::factory::{DecoderFactory, OpenRequest, Probe, ProbeScore, SourceInput};
use super::options::SharedOptions;

/// Frames per block.
const BLOCK_FRAMES: usize = 4096;

/// The largest file a synthesizing backend reads; such files are kilobytes, so
/// anything near this is not one.
const MAX_FILE: u64 = 256 << 20;

/// The library behind a [`SongFactory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SongLibrary {
    /// Tracker modules through libopenmpt.
    #[cfg(feature = "openmpt")]
    OpenMpt,
    /// Game console music through libgme.
    #[cfg(feature = "gme")]
    Gme,
    /// SID tunes through libsidplayfp.
    #[cfg(feature = "sid")]
    Sid,
    /// Game audio streams through vgmstream.
    #[cfg(feature = "vgmstream")]
    Vgmstream,
    /// MIDI files through rustysynth.
    #[cfg(feature = "midi")]
    Midi,
}

impl SongLibrary {
    /// Every library of this build, in registration order.
    pub fn all() -> Vec<Self> {
        vec![
            #[cfg(feature = "openmpt")]
            Self::OpenMpt,
            #[cfg(feature = "gme")]
            Self::Gme,
            #[cfg(feature = "sid")]
            Self::Sid,
            #[cfg(feature = "midi")]
            Self::Midi,
            #[cfg(feature = "vgmstream")]
            Self::Vgmstream,
        ]
    }

    /// The backend's name.
    pub fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "openmpt")]
            Self::OpenMpt => "openmpt",
            #[cfg(feature = "gme")]
            Self::Gme => "gme",
            #[cfg(feature = "sid")]
            Self::Sid => "sidplayfp",
            #[cfg(feature = "vgmstream")]
            Self::Vgmstream => "vgmstream",
            #[cfg(feature = "midi")]
            Self::Midi => "rustysynth",
        }
    }
}

/// The SoundFont MIDI files are played with under `settings`: the chosen one if it
/// exists, otherwise one installed on the system; `None` when MIDI files need one to
/// be chosen.
#[cfg(feature = "midi")]
pub fn midi_soundfont(
    settings: &helikodu_model::settings::DecoderSettings,
) -> Option<std::path::PathBuf> {
    helikodu_extra_decoders::midi::soundfont_for(&SongOptions::from_settings(settings))
        .filter(|path| path.is_file())
}

/// The version of the library behind `library`, when it reports one.
pub fn library_version(library: SongLibrary) -> Option<String> {
    match library {
        #[cfg(feature = "openmpt")]
        SongLibrary::OpenMpt => Some(helikodu_extra_decoders::openmpt::library_version()),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// A synthesizing backend.
#[derive(Debug, Clone)]
pub struct SongFactory {
    library: SongLibrary,
    options: SharedOptions,
}

impl SongFactory {
    /// The backend of `library`, opening files with `options`.
    pub fn new(library: SongLibrary, options: SharedOptions) -> Self {
        Self { library, options }
    }

    /// The library.
    pub fn library(&self) -> SongLibrary {
        self.library
    }

    fn song_options(&self) -> SongOptions {
        SongOptions::from_settings(&self.options.get().settings)
    }
}

/// Maps an error of a song decoder while opening.
fn open_error(backend: &'static str, error: SongError) -> DecodeError {
    match error {
        SongError::Io(e) => DecodeError::Io(e),
        other => DecodeError::Open {
            backend,
            reason: other.to_string(),
        },
    }
}

/// The bytes of the file: those read into memory, or read now.
fn file_bytes(input: &SourceInput<'_>) -> Result<Arc<[u8]>, DecodeError> {
    if let Some(data) = input.data() {
        return Ok(Arc::clone(data));
    }
    let file = File::open(input.path())?;
    let len = file.metadata()?.len();
    if len > MAX_FILE {
        return Err(DecodeError::Open {
            backend: "song",
            reason: "the file is too large for its format".into(),
        });
    }
    let mut data = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(MAX_FILE).read_to_end(&mut data)?;
    Ok(data.into())
}

impl DecoderFactory for SongFactory {
    fn name(&self) -> &'static str {
        self.library.name()
    }

    fn probe(&self, probe: &Probe<'_>) -> ProbeScore {
        let header = probe.header();
        let claimed = match self.library {
            #[cfg(feature = "openmpt")]
            SongLibrary::OpenMpt => helikodu_extra_decoders::openmpt::probe(header),
            #[cfg(feature = "gme")]
            SongLibrary::Gme => helikodu_extra_decoders::gme::probe(header),
            #[cfg(feature = "sid")]
            SongLibrary::Sid => helikodu_extra_decoders::sid::probe(header),
            #[cfg(feature = "midi")]
            SongLibrary::Midi => helikodu_extra_decoders::midi::probe(header),
            #[cfg(feature = "vgmstream")]
            SongLibrary::Vgmstream => {
                return if helikodu_extra_decoders::vgmstream::probe(probe.path()) {
                    ProbeScore::EXTENSION
                } else {
                    ProbeScore::NONE
                };
            }
        };
        if claimed {
            ProbeScore::SIGNATURE
        } else {
            ProbeScore::NONE
        }
    }

    fn open(&self, path: &Path, subsong: Option<u32>) -> Result<Box<dyn Decoder>, DecodeError> {
        self.open_input(
            &SourceInput::file(path),
            &OpenRequest {
                subsong,
                audio_stream: None,
            },
        )
    }

    fn open_input(
        &self,
        input: &SourceInput<'_>,
        request: &OpenRequest,
    ) -> Result<Box<dyn Decoder>, DecodeError> {
        let backend = self.library.name();
        let options = self.song_options();
        let subsong = request.subsong;
        let decoder: Box<dyn SongDecoder> = match self.library {
            #[cfg(feature = "vgmstream")]
            SongLibrary::Vgmstream => Box::new(
                helikodu_extra_decoders::vgmstream::VgmstreamDecoder::open(
                    input.path(),
                    subsong,
                    &options,
                )
                .map_err(|e| open_error(backend, e))?,
            ),
            #[allow(unreachable_patterns)]
            library => {
                let data = file_bytes(input).map_err(|e| match e {
                    DecodeError::Open { reason, .. } => DecodeError::Open { backend, reason },
                    other => other,
                })?;
                open_song(library, &data, subsong, &options).map_err(|e| open_error(backend, e))?
            }
        };
        Ok(Box::new(SongSource::new(
            backend,
            decoder,
            input.data().is_some(),
        )?))
    }
}

/// Opens `data` with `library`, for the libraries that read from memory.
#[allow(unused_variables)]
fn open_song(
    library: SongLibrary,
    data: &[u8],
    subsong: Option<u32>,
    options: &SongOptions,
) -> Result<Box<dyn SongDecoder>, SongError> {
    Ok(match library {
        #[cfg(feature = "openmpt")]
        SongLibrary::OpenMpt => Box::new(helikodu_extra_decoders::openmpt::OpenMptDecoder::open(
            data, subsong, options,
        )?),
        #[cfg(feature = "gme")]
        SongLibrary::Gme => Box::new(helikodu_extra_decoders::gme::GmeDecoder::open(
            data, subsong, options,
        )?),
        #[cfg(feature = "sid")]
        SongLibrary::Sid => Box::new(helikodu_extra_decoders::sid::SidDecoder::open(
            data, subsong, options,
        )?),
        #[cfg(feature = "midi")]
        SongLibrary::Midi => Box::new(helikodu_extra_decoders::midi::MidiDecoder::open(
            data, subsong, options,
        )?),
        #[cfg(feature = "vgmstream")]
        SongLibrary::Vgmstream => return Err(SongError::Unavailable("vgmstream")),
    })
}

/// A subsong rendered by one of the synthesizing decoders.
#[derive(Debug)]
struct SongSource {
    decoder: Box<dyn SongDecoder>,
    info: DecoderInfo,
    rate: SampleRate,
    layout: ChannelLayout,
}

impl SongSource {
    fn new(
        backend: &'static str,
        decoder: Box<dyn SongDecoder>,
        in_memory: bool,
    ) -> Result<Self, DecodeError> {
        let bad = || DecodeError::Open {
            backend,
            reason: "the decoder gave an unusable format".into(),
        };
        let rate = SampleRate::new(decoder.rate()).ok_or_else(bad)?;
        let layout = ChannelLayout::default_for_count(decoder.channels()).ok_or_else(bad)?;
        let song = decoder.info();
        let mut tags = Vec::new();
        let subsong = decoder.subsong();
        let subsong_title = song
            .subsongs
            .get(subsong as usize)
            .and_then(|s| s.title.clone());
        for (key, value) in [
            ("title", subsong_title.or_else(|| song.title.clone())),
            ("artist", song.artist.clone()),
            ("album", song.album.clone()),
            ("date", song.date.clone()),
            ("comment", song.comment.clone()),
        ] {
            if let Some(value) = value {
                tags.push((key.to_owned(), value));
            }
        }
        let info = DecoderInfo {
            backend,
            container: song.format.clone(),
            codec: backend.to_owned(),
            tags,
            subsong: Some(subsong),
            in_memory,
            ..DecoderInfo::default()
        };
        Ok(Self {
            decoder,
            info,
            rate,
            layout,
        })
    }
}

impl Decoder for SongSource {
    fn info(&self) -> &DecoderInfo {
        &self.info
    }

    fn spec(&self) -> AudioSpec {
        AudioSpec::new(
            helikodu_model::audio::SampleFormat::F32(helikodu_model::audio::Endianness::NATIVE),
            self.rate,
            self.layout,
        )
    }

    fn duration(&self) -> Option<MediaTime> {
        self.decoder
            .frames()
            .map(|frames| MediaTime::from_frames(frames, self.rate))
    }

    fn position(&self) -> u64 {
        self.decoder.position()
    }

    fn read(&mut self, block: &mut PcmBlock) -> Result<bool, DecodeError> {
        let mut samples = match block.take_samples() {
            BlockSamples::F32(v) => v,
            _ => Vec::new(),
        };
        let position = self.decoder.position();
        let frames =
            self.decoder
                .read(BLOCK_FRAMES, &mut samples)
                .map_err(|e| DecodeError::Decode {
                    backend: self.info.backend,
                    reason: e.to_string(),
                })?;
        block.set(self.rate, self.layout, position, BlockSamples::F32(samples));
        Ok(frames > 0)
    }

    fn seek(&mut self, to: MediaTime) -> Result<(), DecodeError> {
        self.decoder
            .seek(to.frames_at(self.rate))
            .map_err(|e| DecodeError::Decode {
                backend: self.info.backend,
                reason: e.to_string(),
            })
    }
}
