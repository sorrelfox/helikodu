// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening a file: custom I/O, the locks, probing by content and the stream list.

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::Path;
use std::ptr;

use helikodu_model::audio::MediaTime;

use crate::decode::AudioDecoder;
use crate::demux::{StreamChoice, StreamInfo};
use crate::error::{Error, check};
use crate::ffi::{self, ff, media_type};
use crate::io::{ByteSource, CustomIo, LockOptions, deny_io_open, verify_locks};
use crate::metadata::{AttachedPicture, Chapter, PictureLimits, Tags, chapters, pictures};

/// An open format context together with the custom I/O it reads through.
///
/// The format context is closed before the I/O context is freed: `Drop` closes it,
/// then the fields drop.
pub(crate) struct Input {
    pub(crate) fmt: *mut ff::AVFormatContext,
    io: CustomIo,
}

// SAFETY: the format context and its I/O are reached only through this value;
// FFmpeg contexts may move between threads as long as they are not used from two
// threads at once, which `&mut` access guarantees.
unsafe impl Send for Input {}

impl Drop for Input {
    fn drop(&mut self) {
        // SAFETY: fmt is NULL or a context opened by avformat_open_input. With
        // AVFMT_FLAG_CUSTOM_IO it leaves the AVIOContext alone; `io` frees that
        // after this.
        unsafe { ff::avformat_close_input(&mut self.fmt) };
    }
}

impl Input {
    /// # Safety
    /// The returned slice must not outlive the context or a call that may add
    /// streams (`avformat_find_stream_info`, `av_read_frame`).
    pub(crate) unsafe fn streams<'a>(&self) -> &'a [*mut ff::AVStream] {
        // SAFETY: fmt is an open context; `streams` has `nb_streams` entries.
        unsafe {
            let n = (*self.fmt).nb_streams as usize;
            if n == 0 || (*self.fmt).streams.is_null() {
                &[]
            } else {
                std::slice::from_raw_parts((*self.fmt).streams, n)
            }
        }
    }

    /// The stream at `index`, if there is one.
    pub(crate) fn stream(&self, index: usize) -> Option<*mut ff::AVStream> {
        // SAFETY: the slice is used at once.
        unsafe { self.streams() }.get(index).copied()
    }

    /// Marks every stream except `keep` with `AVDISCARD_ALL`, so the demuxer
    /// drops their packets. With `keep == None`, only non-audio streams.
    fn discard_others(&mut self, keep: Option<usize>) {
        // SAFETY: the slice is used at once, with no call that adds streams.
        for (i, &st) in unsafe { self.streams() }.iter().enumerate() {
            // SAFETY: a valid stream of the open context; only known enum values
            // are written, the existing value is never read as an enum.
            unsafe {
                let wanted = match keep {
                    Some(k) => i == k,
                    None => ffi::codec_type((*st).codecpar) == media_type::AUDIO,
                };
                (*st).discard = if wanted {
                    ff::AVDiscard::AVDISCARD_DEFAULT
                } else {
                    ff::AVDiscard::AVDISCARD_ALL
                };
            }
        }
    }
}

/// An opened, probed media file. Read its streams, tags, chapters and pictures, or
/// turn it into an [`AudioDecoder`].
///
/// Every open goes through custom I/O with all five offline locks: see the crate
/// README.
pub struct MediaFile {
    pub(crate) input: Input,
    format_name: String,
    streams: Vec<StreamInfo>,
    tags: Tags,
}

impl std::fmt::Debug for MediaFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaFile")
            .field("format_name", &self.format_name)
            .field("streams", &self.streams)
            .finish_non_exhaustive()
    }
}

impl MediaFile {
    /// Opens the file at `path`. The path is used as bytes to open the file and
    /// never reaches FFmpeg.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_file(File::open(path)?)
    }

    /// Opens an already open file, from its start.
    pub fn open_file(mut file: File) -> Result<Self, Error> {
        file.seek(SeekFrom::Start(0))?;
        let size = file
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len());
        Self::open_source(Box::new(file), size)
    }

    pub(crate) fn open_source(
        source: Box<dyn ByteSource>,
        size: Option<u64>,
    ) -> Result<Self, Error> {
        let mut input = Input {
            fmt: ptr::null_mut(),
            io: CustomIo::new(source, size)?,
        };
        // SAFETY: plain allocation; checked below.
        input.fmt = unsafe { ff::avformat_alloc_context() };
        if input.fmt.is_null() {
            return Err(Error::out_of_memory("avformat_alloc_context"));
        }
        // SAFETY: fmt is a fresh context and io outlives it (see Input).
        unsafe {
            (*input.fmt).pb = input.io.as_ptr();
            (*input.fmt).flags |= ff::AVFMT_FLAG_CUSTOM_IO;
            (*input.fmt).io_open = Some(deny_io_open);
        }

        let mut options = LockOptions::new()?;
        // No file name: FFmpeg probes by content only and has nothing to resolve
        // relative references against.
        // SAFETY: fmt is a fresh context with pb set. On failure FFmpeg frees it and
        // writes NULL; Input::drop then still frees the custom I/O. The dictionary
        // is replaced by one holding the options FFmpeg did not use.
        let ret = unsafe {
            ff::avformat_open_input(
                &mut input.fmt,
                c"".as_ptr(),
                ptr::null(),
                options.as_mut_ptr(),
            )
        };
        check("avformat_open_input", ret)?;
        options.check_leftovers()?;
        // SAFETY: fmt was opened just now.
        unsafe { verify_locks(input.fmt) }?;

        // Keep non-audio streams out of probing as far as FFmpeg allows. It still
        // calls avcodec_open2 for them; the codec whitelist refuses those calls.
        input.discard_others(None);
        // SAFETY: fmt is open; NULL options are allowed.
        check("avformat_find_stream_info", unsafe {
            ff::avformat_find_stream_info(input.fmt, ptr::null_mut())
        })?;
        // Probing may have added streams.
        input.discard_others(None);

        // SAFETY: the slice is used at once; each stream is valid.
        let streams = unsafe { input.streams() }
            .iter()
            .enumerate()
            .map(|(i, &st)| unsafe { StreamInfo::read(i, st) })
            .collect();
        // SAFETY: iformat is set after a successful open; its name is static.
        let format_name = unsafe { ffi::string((*(*input.fmt).iformat).name) }.unwrap_or_default();
        // SAFETY: the format's dictionary is valid while the context is open.
        let tags = Tags::from_entries(unsafe { ffi::dict_entries((*input.fmt).metadata) });
        Ok(Self {
            input,
            format_name,
            streams,
            tags,
        })
    }

    /// FFmpeg's name of the demuxer that recognised the file, such as `flac` or
    /// `mov,mp4,m4a,3gp,3g2,mj2`.
    pub fn format_name(&self) -> &str {
        &self.format_name
    }

    /// Every stream of the file, in file order.
    pub fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    /// The streams that can be played as audio (attached pictures excluded).
    pub fn audio_streams(&self) -> impl Iterator<Item = &StreamInfo> {
        self.streams.iter().filter(|s| s.is_playable_audio())
    }

    /// The file's tags (the container's own; see also [`StreamInfo::tags`]).
    pub fn tags(&self) -> &Tags {
        &self.tags
    }

    /// The tags that describe the audio: the container's, followed by those of
    /// `stream`. Ogg and some other containers keep the file's tags on the stream.
    pub fn tags_with_stream(&self, stream: usize) -> Tags {
        let mut tags = self.tags.clone();
        if let Some(s) = self.streams.get(stream) {
            tags.extend(&s.tags);
        }
        tags
    }

    /// The duration the container states for the whole file.
    pub fn duration(&self) -> Option<MediaTime> {
        // SAFETY: fmt is open.
        let duration = unsafe { (*self.input.fmt).duration };
        ffi::media_time(
            duration,
            ff::AVRational {
                num: 1,
                den: ff::AV_TIME_BASE,
            },
        )
    }

    /// The file's chapters, in file order.
    pub fn chapters(&self) -> Vec<Chapter> {
        chapters::read(&self.input)
    }

    /// The pictures embedded in the file: attached-picture streams (ID3, FLAC,
    /// MP4 covers) and Matroska image attachments. Pictures over the limits are
    /// left out.
    pub fn attached_pictures(&self, limits: &PictureLimits) -> Vec<AttachedPicture> {
        pictures::read(&self.input, &self.streams, limits)
    }

    /// The audio stream `choice` selects: the explicit index; otherwise the audio
    /// stream marked default, when only some are; otherwise FFmpeg's best audio
    /// stream.
    pub fn choose_stream(&self, choice: StreamChoice) -> Result<usize, Error> {
        if let StreamChoice::Index(index) = choice {
            return match self.streams.get(index) {
                Some(s) if s.is_playable_audio() => Ok(index),
                _ => Err(Error::NotAnAudioStream { index }),
            };
        }
        let audio: Vec<&StreamInfo> = self.audio_streams().collect();
        let defaults: Vec<&&StreamInfo> = audio
            .iter()
            .filter(|s| s.disposition.is_default())
            .collect();
        // When every stream carries the default flag (Matroska sets it unless told
        // otherwise), it says nothing.
        if !defaults.is_empty() && defaults.len() < audio.len() {
            return Ok(defaults[0].index);
        }
        // SAFETY: fmt is open; no decoder is requested, so nothing is opened.
        let best = unsafe {
            ff::av_find_best_stream(
                self.input.fmt,
                ff::AVMediaType::AVMEDIA_TYPE_AUDIO,
                -1,
                -1,
                ptr::null_mut(),
                0,
            )
        };
        match usize::try_from(best) {
            Ok(i)
                if self
                    .streams
                    .get(i)
                    .is_some_and(StreamInfo::is_playable_audio) =>
            {
                Ok(i)
            }
            _ => audio.first().map(|s| s.index).ok_or(Error::NoAudioStream),
        }
    }

    /// Chooses an audio stream, discards every other stream and opens its decoder.
    pub fn into_decoder(self, choice: StreamChoice) -> Result<AudioDecoder, Error> {
        AudioDecoder::new(self, choice)
    }

    /// Discards every stream except `keep`.
    pub(crate) fn keep_only(&mut self, keep: usize) {
        self.input.discard_others(Some(keep));
    }

    /// The format context's flags of interest to the decoder: whether this is the
    /// QuickTime/MP4 demuxer, whose end padding the bridge trims itself.
    pub(crate) fn is_mp4_family(&self) -> bool {
        self.format_name.split(',').any(|n| n == "mov")
    }

    /// The raw `AVStream` of `index`.
    pub(crate) fn raw_stream(&self, index: usize) -> Option<*mut ff::AVStream> {
        self.input.stream(index)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom};

    use super::*;
    use crate::error::AvErrorKind;

    struct Panicking;

    impl Read for Panicking {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("a panic while FFmpeg reads");
        }
    }

    impl Seek for Panicking {
        fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
            Ok(0)
        }
    }

    #[test]
    fn a_panic_in_a_callback_stays_in_rust() {
        let err = MediaFile::open_source(Box::new(Panicking), None).expect_err("must fail");
        match err {
            Error::Ffmpeg { error, .. } => assert_eq!(error.kind(), AvErrorKind::External),
            other => panic!("unexpected error {other}"),
        }
    }

    #[test]
    fn garbage_is_refused() {
        let data = std::io::Cursor::new(vec![0x55u8; 4096]);
        assert!(MediaFile::open_source(Box::new(data), Some(4096)).is_err());
    }

    #[test]
    fn empty_input_is_refused() {
        let data = std::io::Cursor::new(Vec::<u8>::new());
        assert!(MediaFile::open_source(Box::new(data), Some(0)).is_err());
    }
}
