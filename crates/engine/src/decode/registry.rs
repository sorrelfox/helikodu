// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The registry of decoder backends and the choice of one for a file.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use helikodu_model::library::{MediaSource, SourceSection};
use helikodu_model::settings::DecoderSettings;

use super::decoder::Decoder;
use super::error::DecodeError;
use super::factory::{DecoderFactory, OpenRequest, Probe, ProbeScore, SourceInput};
use super::ffmpeg::FfmpegFactory;
use super::options::{DecodeOptions, SharedOptions};
use super::section::SectionDecoder;

/// The decoder backends, and the choice of one for each file.
///
/// Every factory probes the first bytes of the file; they are tried from the
/// highest [`ProbeScore`] down, and on a tie in the order they were registered. The
/// first that opens the file wins. A section of the [`MediaSource`] is applied on
/// top: a subsong is passed to the backend, a span trims its output.
///
/// The registry's [`DecodeOptions`] are shared with the backends it built: the
/// decoder settings ([`configure`](Self::configure)), DoP for DSD files
/// ([`set_dop`](Self::set_dop)) and whole-file buffering, which reads a file of up
/// to [`DecodeOptions::memory_limit`] bytes into memory before any backend opens
/// it.
#[derive(Debug)]
pub struct DecoderRegistry {
    factories: Vec<Box<dyn DecoderFactory>>,
    options: SharedOptions,
}

impl Default for DecoderRegistry {
    fn default() -> Self {
        Self::with_default_backends()
    }
}

impl DecoderRegistry {
    /// A registry without backends.
    pub fn empty() -> Self {
        Self {
            factories: Vec::new(),
            options: SharedOptions::default(),
        }
    }

    /// A registry with every backend this build has: the DSF/DFF reader (`dsd`
    /// feature), the synthesizing decoders (`openmpt`, `gme`, `sid`, `midi` and
    /// `vgmstream` features) and FFmpeg.
    pub fn with_default_backends() -> Self {
        let mut registry = Self::empty();
        #[cfg(feature = "dsd")]
        registry.register(Box::new(super::dsd::DsdFactory::new(
            registry.options.clone(),
        )));
        #[cfg(feature = "song")]
        for library in super::songs::SongLibrary::all() {
            registry.register(Box::new(super::songs::SongFactory::new(
                library,
                registry.options.clone(),
            )));
        }
        registry.register(Box::new(FfmpegFactory));
        registry
    }

    /// The default backends, opening files with `settings`.
    pub fn with_settings(settings: &DecoderSettings) -> Self {
        let registry = Self::with_default_backends();
        registry.configure(settings);
        registry
    }

    /// The options files are opened with, shared with the backends.
    pub fn options(&self) -> DecodeOptions {
        self.options.get()
    }

    /// Replaces the decoder settings. They apply to the files opened from now on.
    pub fn configure(&self, settings: &DecoderSettings) {
        self.options.update(|o| o.settings = settings.clone());
    }

    /// Whether DSD files are opened as DSD over PCM (DoP) from now on.
    pub fn set_dop(&self, dop: bool) {
        self.options.update(|o| o.dop = dop);
    }

    /// Lowers or raises the size above which files are never read into memory
    /// whole (for tests; the settings never go above 512 MiB).
    pub fn set_memory_limit(&self, bytes: u64) {
        self.options.update(|o| o.memory_limit = bytes);
    }

    /// Adds a backend. On equal scores, earlier backends are tried first.
    pub fn register(&mut self, factory: Box<dyn DecoderFactory>) {
        self.factories.push(factory);
    }

    /// The registered backends, in order.
    pub fn factories(&self) -> impl Iterator<Item = &dyn DecoderFactory> {
        self.factories.iter().map(AsRef::as_ref)
    }

    /// The backends that recognise the file at `path`, best first, with their
    /// scores. Backends that score [`ProbeScore::NONE`] are left out.
    pub fn candidates(
        &self,
        path: &Path,
    ) -> Result<Vec<(&dyn DecoderFactory, ProbeScore)>, DecodeError> {
        let header = read_header(path)?;
        Ok(self.rank(&Probe::new(path, &header)))
    }

    fn rank(&self, probe: &Probe<'_>) -> Vec<(&dyn DecoderFactory, ProbeScore)> {
        let mut ranked: Vec<_> = self
            .factories()
            .map(|f| (f, f.probe(probe)))
            .filter(|(_, score)| *score > ProbeScore::NONE)
            .collect();
        // A stable sort keeps the registration order on ties.
        ranked.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        ranked
    }

    /// Opens a decoder for `source` with the best backend that can open it.
    ///
    /// Fails with [`DecodeError::Io`] when the file cannot be read, with
    /// [`DecodeError::NoDecoder`] (listing each backend's reason) when no backend
    /// opens it, and with [`DecodeError::InvalidSection`] when a span lies outside
    /// the file.
    pub fn open(&self, source: &MediaSource) -> Result<Box<dyn Decoder>, DecodeError> {
        self.open_stream(source, None)
    }

    /// Opens a decoder for `source` like [`open`](Self::open), playing the audio
    /// stream with the index `audio_stream` of a file that has several (`None`: the
    /// default one).
    pub fn open_stream(
        &self,
        source: &MediaSource,
        audio_stream: Option<u32>,
    ) -> Result<Box<dyn Decoder>, DecodeError> {
        let path = source.path();
        let request = OpenRequest {
            subsong: match &source.section {
                Some(SourceSection::Subsong(index)) => Some(*index),
                _ => None,
            },
            audio_stream,
        };
        let data = self.read_whole(path)?;
        let header = match &data {
            Some(data) => data[..data.len().min(Probe::HEADER_LEN)].to_vec(),
            None => read_header(path)?,
        };
        let input = match &data {
            Some(data) => SourceInput::in_memory(path, data),
            None => SourceInput::file(path),
        };
        let mut attempts = Vec::new();
        for (factory, _) in self.rank(&Probe::new(path, &header)) {
            match factory.open_input(&input, &request) {
                Ok(decoder) => {
                    return match &source.section {
                        Some(SourceSection::Span { start, end }) => {
                            Ok(Box::new(SectionDecoder::new(decoder, *start, *end)?))
                        }
                        _ => Ok(decoder),
                    };
                }
                // The file went away or cannot be read: no other backend will do
                // better.
                Err(DecodeError::Io(e)) => return Err(DecodeError::Io(e)),
                Err(e) => attempts.push((factory.name(), e.to_string())),
            }
        }
        Err(DecodeError::NoDecoder { attempts })
    }
}

impl DecoderRegistry {
    /// The whole file, when the settings ask for whole-file buffering and it is no
    /// larger than the limit.
    fn read_whole(&self, path: &Path) -> Result<Option<Arc<[u8]>>, DecodeError> {
        let options = self.options.get();
        if !options.settings.load_into_memory {
            return Ok(None);
        }
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len > options.memory_limit {
            return Ok(None);
        }
        let mut data = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        file.take(options.memory_limit).read_to_end(&mut data)?;
        Ok(Some(data.into()))
    }
}

/// The first bytes of the file, for probing.
fn read_header(path: &Path) -> Result<Vec<u8>, DecodeError> {
    let file = File::open(path)?;
    let mut header = Vec::with_capacity(Probe::HEADER_LEN);
    file.take(Probe::HEADER_LEN as u64)
        .read_to_end(&mut header)?;
    Ok(header)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[derive(Debug)]
    struct Fixed(&'static str, ProbeScore);

    impl DecoderFactory for Fixed {
        fn name(&self) -> &'static str {
            self.0
        }
        fn probe(&self, _probe: &Probe<'_>) -> ProbeScore {
            self.1
        }
        fn open(
            &self,
            _path: &Path,
            _subsong: Option<u32>,
        ) -> Result<Box<dyn Decoder>, DecodeError> {
            Err(DecodeError::Open {
                backend: self.0,
                reason: "refused".into(),
            })
        }
    }

    #[test]
    fn ranks_by_score_then_registration_order() {
        let mut registry = DecoderRegistry::empty();
        registry.register(Box::new(Fixed("a", ProbeScore::FALLBACK)));
        registry.register(Box::new(Fixed("b", ProbeScore::SIGNATURE)));
        registry.register(Box::new(Fixed("c", ProbeScore::NONE)));
        registry.register(Box::new(Fixed("d", ProbeScore::FALLBACK)));
        let path = PathBuf::from("x");
        let names: Vec<_> = registry
            .rank(&Probe::new(&path, b""))
            .into_iter()
            .map(|(f, _)| f.name())
            .collect();
        assert_eq!(names, ["b", "a", "d"]);
    }

    #[test]
    fn default_backends() {
        let names: Vec<_> = DecoderRegistry::default()
            .factories()
            .map(|f| f.name())
            .collect();
        let mut expected = Vec::new();
        if cfg!(feature = "dsd") {
            expected.push("dsd");
        }
        for (feature, name) in [
            (cfg!(feature = "openmpt"), "openmpt"),
            (cfg!(feature = "gme"), "gme"),
            (cfg!(feature = "sid"), "sidplayfp"),
            (cfg!(feature = "midi"), "rustysynth"),
            (cfg!(feature = "vgmstream"), "vgmstream"),
        ] {
            if feature {
                expected.push(name);
            }
        }
        expected.push("ffmpeg");
        assert_eq!(names, expected);
    }
}
