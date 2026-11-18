// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Game audio streams through vgmstream's public API (`libvgmstream`): ADX, HCA,
//! BRSTM/BCSTM/BFSTM, DSP, Wwise WEM/BNK, FSB, XWB, AT3/AT9, VAG, XA, MSF and
//! hundreds more.
//!
//! Most of these formats have no signature, and vgmstream itself first goes by the
//! file name's extension; [`probe`] asks it whether it would try the file, and
//! vgmstream then checks the content when it opens it. A file it does not
//! recognise is left to the next decoder. Common extensions that other decoders
//! handle (`wav`, `ogg`, `mp4`, ...) are not claimed. vgmstream reads the file itself,
//! and companion files next to it (some formats keep header and data apart).
//!
//! A looping stream plays its loop [`SongOptions::loops`] times and fades out over
//! [`SongOptions::fade`]; vgmstream applies both. Banks with several streams have
//! subsongs. vgmstream seeks to the exact frame.
//!
//! The library is linked only when the crate is built with the `vgmstream` feature
//! and the build finds it; otherwise [`available`] is false and opening fails with
//! [`SongError::Unavailable`].

use std::ffi::{CString, c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Once;

use helikodu_vgmstream_sys as ffi;

use crate::song::text;
use crate::song::{Envelope, SongDecoder, SongError, SongInfo, SongOptions, SubsongInfo};

/// Whether vgmstream is linked into this build.
pub fn available() -> bool {
    ffi::LINKED
}

/// Whether vgmstream would try to open the file at `path`. Only the name is
/// available to vgmstream before it opens a file; the content is checked then.
pub fn probe(path: &Path) -> bool {
    if !available() {
        return false;
    }
    let Ok(name) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut options = ffi::libvgmstream_valid_t {
        reject_extensionless: true,
        ..ffi::libvgmstream_valid_t::default()
    };
    // SAFETY: the name is NUL-terminated and the options live across the call.
    unsafe { ffi::libvgmstream_is_valid(name.as_ptr(), &raw mut options) }
}

/// Silences the library's log, once.
fn quiet() {
    static QUIET: Once = Once::new();
    // SAFETY: a null callback with the "none" level only stores the level.
    QUIET
        .call_once(|| unsafe { ffi::libvgmstream_set_log(ffi::LIBVGMSTREAM_LOG_LEVEL_NONE, None) });
}

/// A library handle with a stream open.
struct Lib(NonNull<ffi::libvgmstream_t>);

// SAFETY: a libvgmstream handle has no thread affinity; it is used from one thread
// at a time, through `&mut`.
unsafe impl Send for Lib {}

impl Drop for Lib {
    fn drop(&mut self) {
        // SAFETY: the handle came from libvgmstream_init and is freed once (which
        // closes the stream).
        unsafe { ffi::libvgmstream_free(self.0.as_ptr()) };
    }
}

impl std::fmt::Debug for Lib {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lib")
    }
}

impl Lib {
    /// The stream's format, valid while a stream is open.
    fn format(&self) -> ffi::libvgmstream_format_t {
        // SAFETY: after a successful open, `format` points to the library's current
        // format description.
        unsafe { *(*self.0.as_ptr()).format }
    }
}

fn name(field: &[c_char]) -> Option<String> {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    text(&String::from_utf8_lossy(&bytes))
}

/// One stream of a game audio file, rendered to `f32`.
#[derive(Debug)]
pub struct VgmstreamDecoder {
    lib: Lib,
    info: SongInfo,
    subsong: u32,
    rate: u32,
    channels: u16,
    envelope: Envelope,
    position: u64,
    /// Rendered frames not handed out yet, from `pending_at`.
    pending: Vec<f32>,
    pending_at: usize,
    /// vgmstream has rendered its last batch.
    done: bool,
}

impl VgmstreamDecoder {
    /// Opens stream `subsong` (counted from 0; `None` for the first) of the file at
    /// `path`.
    pub fn open(
        path: &Path,
        subsong: Option<u32>,
        options: &SongOptions,
    ) -> Result<Self, SongError> {
        if !available() {
            return Err(SongError::Unavailable("vgmstream"));
        }
        quiet();
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| SongError::Unsupported("the path holds a NUL byte".into()))?;
        // SAFETY: plain allocation; checked below.
        let lib = Lib(NonNull::new(unsafe { ffi::libvgmstream_init() })
            .ok_or(SongError::Unavailable("vgmstream"))?);
        let mut config = ffi::libvgmstream_config_t {
            loop_count: f64::from(options.loops.max(1)),
            fade_time: options.fade.as_secs_f64(),
            force_sfmt: ffi::LIBVGMSTREAM_SFMT_FLOAT,
            ..ffi::libvgmstream_config_t::default()
        };
        // SAFETY: the handle is valid; the config lives across the call.
        unsafe { ffi::libvgmstream_setup(lib.0.as_ptr(), &raw mut config) };
        // SAFETY: the path is NUL-terminated.
        let file = unsafe { ffi::libstreamfile_open_from_stdio(c_path.as_ptr()) };
        if file.is_null() {
            return Err(SongError::Io(std::io::Error::other(
                "vgmstream cannot open the file",
            )));
        }
        let index = subsong.map_or(0, |s| c_int::try_from(s).unwrap_or(c_int::MAX - 1) + 1);
        // SAFETY: the handle and the stream file are valid; vgmstream reopens what it
        // needs, so the stream file is closed right after.
        let opened = unsafe {
            let opened = ffi::libvgmstream_open_stream(lib.0.as_ptr(), file, index);
            ffi::libstreamfile_close(file);
            opened
        };
        if opened < 0 {
            return Err(SongError::Unsupported(
                "vgmstream does not recognise the file".into(),
            ));
        }
        let format = lib.format();
        if format.sample_format != ffi::LIBVGMSTREAM_SFMT_FLOAT || format.channels <= 0 {
            return Err(SongError::Unsupported(
                "vgmstream gave an unexpected format".into(),
            ));
        }
        let rate = u32::try_from(format.sample_rate).unwrap_or(0);
        let channels = u16::try_from(format.channels).unwrap_or(0);
        if rate == 0 || channels == 0 {
            return Err(SongError::Unsupported(
                "vgmstream gave an unexpected format".into(),
            ));
        }
        let count = u32::try_from(format.subsong_count).unwrap_or(0).max(1);
        let selected = u32::try_from(format.subsong_index)
            .unwrap_or(0)
            .saturating_sub(1);
        let frames = u64::try_from(format.play_samples).ok().filter(|&f| f > 0);
        let length = frames.map(|f| std::time::Duration::from_secs_f64(f as f64 / f64::from(rate)));
        let mut subsongs: Vec<SubsongInfo> = (0..count)
            .map(|index| SubsongInfo {
                index,
                title: None,
                length: None,
            })
            .collect();
        if let Some(entry) = subsongs.get_mut(selected as usize) {
            entry.title = name(&format.stream_name);
            entry.length = length;
        }
        let info = SongInfo {
            format: name(&format.meta_name).unwrap_or_else(|| "vgmstream".to_owned()),
            title: if count == 1 {
                name(&format.stream_name)
            } else {
                None
            },
            subsongs,
            default_subsong: 0,
            ..SongInfo::default()
        };
        // vgmstream applies the loops and the fade; the envelope only marks the end.
        let envelope = frames.map_or(Envelope::OPEN, |f| Envelope::ending_at(f, 0));
        Ok(Self {
            lib,
            info,
            subsong: selected,
            rate,
            channels,
            envelope,
            position: 0,
            pending: Vec::new(),
            pending_at: 0,
            done: false,
        })
    }

    /// Renders the next batch into `pending`.
    fn render(&mut self) -> Result<(), SongError> {
        let channels = usize::from(self.channels);
        // SAFETY: the handle has a stream open; after a render `decoder` points to
        // the description of the library's buffer, valid until the next render.
        let (done, buf, frames) = unsafe {
            if ffi::libvgmstream_render(self.lib.0.as_ptr()) < 0 {
                return Err(SongError::Render("vgmstream failed while decoding".into()));
            }
            let decoder = &*(*self.lib.0.as_ptr()).decoder;
            (
                decoder.done,
                decoder.buf.cast::<f32>().cast_const(),
                usize::try_from(decoder.buf_samples).unwrap_or(0),
            )
        };
        self.pending.clear();
        self.pending_at = 0;
        if frames > 0 && !buf.is_null() {
            // SAFETY: the buffer holds `buf_samples` frames of `channels` floats.
            let samples = unsafe { std::slice::from_raw_parts(buf, frames * channels) };
            self.pending.extend_from_slice(samples);
        }
        self.done = done;
        Ok(())
    }
}

impl SongDecoder for VgmstreamDecoder {
    fn info(&self) -> &SongInfo {
        &self.info
    }

    fn subsong(&self) -> u32 {
        self.subsong
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn frames(&self) -> Option<u64> {
        self.envelope.end()
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn read(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<usize, SongError> {
        out.clear();
        let channels = usize::from(self.channels);
        while self.pending_at >= self.pending.len() {
            if self.done {
                return Ok(0);
            }
            self.render()?;
        }
        let available = (self.pending.len() - self.pending_at) / channels;
        let frames = self.envelope.room(self.position, available.min(max_frames));
        let end = self.pending_at + frames * channels;
        out.extend_from_slice(&self.pending[self.pending_at..end]);
        self.pending_at = end;
        self.position += frames as u64;
        if frames == 0 {
            // The end the format states is reached.
            self.done = true;
            self.pending.clear();
            self.pending_at = 0;
        }
        Ok(frames)
    }

    fn seek(&mut self, frame: u64) -> Result<(), SongError> {
        let target = self.envelope.end().map_or(frame, |end| frame.min(end));
        let sample = i64::try_from(target).unwrap_or(i64::MAX);
        // SAFETY: the handle has a stream open.
        let at = unsafe {
            ffi::libvgmstream_seek(self.lib.0.as_ptr(), sample);
            ffi::libvgmstream_get_play_position(self.lib.0.as_ptr())
        };
        self.position = u64::try_from(at).unwrap_or(target);
        self.pending.clear();
        self.pending_at = 0;
        self.done = false;
        Ok(())
    }
}
