// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tracker modules through libopenmpt.
//!
//! libopenmpt recognises about fifty module formats by their content
//! ([`probe`]). A module plays its subsong once, as libopenmpt plays it, and ends
//! where the song ends or would jump back; modules have no fade. Seeking moves to the
//! row libopenmpt finds for the time and renders from there up to the exact frame.

use std::ffi::{CStr, c_char, c_int};
use std::ptr::{self, NonNull};
use std::time::Duration;

use helikodu_openmpt_sys as ffi;

use crate::song::text;
use crate::song::{SongDecoder, SongError, SongInfo, SongOptions, SubsongInfo};

/// Frames rendered at a time while moving forward to a seek target.
const SKIP_BLOCK: usize = 4096;

/// Whether `header`, the first bytes of a file, is a module libopenmpt can open.
/// A short file whose header leaves it open is not claimed.
pub fn probe(header: &[u8]) -> bool {
    if header.is_empty() {
        return false;
    }
    // SAFETY: the buffer is valid for its length; the callbacks are the library's
    // own silent ones; the error outputs may be null.
    let result = unsafe {
        ffi::openmpt_probe_file_header_without_filesize(
            ffi::OPENMPT_PROBE_FILE_HEADER_FLAGS_DEFAULT,
            header.as_ptr().cast(),
            header.len(),
            Some(ffi::openmpt_log_func_silent),
            ptr::null_mut(),
            Some(ffi::openmpt_error_func_ignore),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    result == ffi::OPENMPT_PROBE_FILE_HEADER_RESULT_SUCCESS
}

/// The library's version as text, such as `0.8.10`.
pub fn library_version() -> String {
    // SAFETY: no arguments; returns a plain number.
    let v = unsafe { ffi::openmpt_get_library_version() };
    format!("{}.{}.{}", v >> 24, (v >> 16) & 0xff, v & 0xffff)
}

/// An opened module.
struct Module(NonNull<ffi::openmpt_module>);

// SAFETY: a libopenmpt module has no thread affinity; it is used from one thread at
// a time, through `&mut` (or `&` for reads that do not change it).
unsafe impl Send for Module {}

impl Drop for Module {
    fn drop(&mut self) {
        // SAFETY: the module was created by libopenmpt and is destroyed once.
        unsafe { ffi::openmpt_module_destroy(self.0.as_ptr()) };
    }
}

impl std::fmt::Debug for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Module")
    }
}

impl Module {
    fn as_ptr(&self) -> *mut ffi::openmpt_module {
        self.0.as_ptr()
    }

    /// A metadata value, such as `title`, `artist` or `type_long`.
    fn metadata(&self, key: &CStr) -> Option<String> {
        // SAFETY: the module is open and the key NUL-terminated; the result is a
        // string the caller frees with openmpt_free_string.
        let value = unsafe { ffi::openmpt_module_get_metadata(self.as_ptr(), key.as_ptr()) };
        take_string(value).and_then(|s| text(&s))
    }

    fn select(&self, subsong: u32) -> Result<(), SongError> {
        let index = i32::try_from(subsong).unwrap_or(i32::MAX);
        // SAFETY: the module is open.
        if unsafe { ffi::openmpt_module_select_subsong(self.as_ptr(), index) } == 0 {
            return Err(SongError::Render(format!(
                "libopenmpt cannot select subsong {subsong}"
            )));
        }
        Ok(())
    }

    fn duration(&self) -> f64 {
        // SAFETY: the module is open.
        unsafe { ffi::openmpt_module_get_duration_seconds(self.as_ptr()) }
    }
}

/// Copies and frees a string libopenmpt returned.
fn take_string(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: libopenmpt returns NUL-terminated strings, valid until freed.
    let text = unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the string came from libopenmpt and is freed once.
    unsafe { ffi::openmpt_free_string(value) };
    Some(text)
}

/// A subsong of a tracker module, rendered to stereo `f32`.
#[derive(Debug)]
pub struct OpenMptDecoder {
    module: Module,
    info: SongInfo,
    subsong: u32,
    rate: u32,
    frames: u64,
    position: u64,
    scratch: Vec<f32>,
}

impl OpenMptDecoder {
    /// Opens the module in `data` at `subsong` (counted from 0; `None` for the first).
    pub fn open(
        data: &[u8],
        subsong: Option<u32>,
        options: &SongOptions,
    ) -> Result<Self, SongError> {
        let mut error: c_int = 0;
        let mut message: *const c_char = ptr::null();
        // SAFETY: the data is valid for its length (libopenmpt copies what it keeps);
        // the callbacks are the library's own; `error` and `message` receive the
        // failure; no initial ctls (null is allowed).
        let raw = unsafe {
            ffi::openmpt_module_create_from_memory2(
                data.as_ptr().cast(),
                data.len(),
                Some(ffi::openmpt_log_func_silent),
                ptr::null_mut(),
                Some(ffi::openmpt_error_func_ignore),
                ptr::null_mut(),
                &raw mut error,
                &raw mut message,
                ptr::null(),
            )
        };
        let reason = take_string(message);
        let Some(raw) = NonNull::new(raw) else {
            return Err(SongError::Unsupported(reason.unwrap_or_else(|| {
                format!("libopenmpt cannot open the file (error {error})")
            })));
        };
        let module = Module(raw);
        // Each subsong plays once and stops where it ends (repeat count 0), without
        // the tail some versions render after the end (`play.at_end`, 0.7 and newer;
        // older versions refuse the ctl, and `read` stops at the length anyway).
        // SAFETY: the module is open; the ctl and its value are NUL-terminated; the
        // parameters are documented render params.
        unsafe {
            ffi::openmpt_module_ctl_set_text(
                module.as_ptr(),
                c"play.at_end".as_ptr(),
                c"stop".as_ptr(),
            );
            ffi::openmpt_module_set_repeat_count(module.as_ptr(), 0);
            ffi::openmpt_module_set_render_param(
                module.as_ptr(),
                ffi::OPENMPT_MODULE_RENDER_STEREOSEPARATION_PERCENT,
                i32::try_from(options.stereo_separation.min(200)).unwrap_or(100),
            );
            ffi::openmpt_module_set_render_param(
                module.as_ptr(),
                ffi::OPENMPT_MODULE_RENDER_INTERPOLATIONFILTER_LENGTH,
                options.interpolation.taps(),
            );
        }

        // SAFETY: the module is open.
        let count = unsafe { ffi::openmpt_module_get_num_subsongs(module.as_ptr()) }.max(1);
        let count = u32::try_from(count).unwrap_or(1);
        let mut subsongs = Vec::with_capacity(count as usize);
        for index in 0..count {
            module.select(index)?;
            // SAFETY: the module is open; the name is freed by take_string.
            let title = take_string(unsafe {
                ffi::openmpt_module_get_subsong_name(module.as_ptr(), index as i32)
            })
            .and_then(|s| text(&s));
            subsongs.push(SubsongInfo {
                index,
                title,
                length: seconds(module.duration()),
            });
        }
        let info = SongInfo {
            format: module
                .metadata(c"type_long")
                .unwrap_or_else(|| "Tracker module".to_owned()),
            title: module.metadata(c"title"),
            artist: module.metadata(c"artist"),
            album: None,
            date: module.metadata(c"date"),
            comment: module.metadata(c"message"),
            subsongs,
            default_subsong: 0,
        };
        let subsong = subsong.unwrap_or(0);
        if subsong >= count {
            return Err(SongError::NoSuchSubsong {
                index: subsong,
                count,
            });
        }
        module.select(subsong)?;
        let frames = (module.duration().max(0.0) * f64::from(options.rate)).round() as u64;
        Ok(Self {
            module,
            info,
            subsong,
            rate: options.rate,
            frames,
            position: 0,
            scratch: Vec::new(),
        })
    }

    fn render(&mut self, frames: usize, out: &mut Vec<f32>) -> usize {
        out.resize(frames * 2, 0.0);
        let rate = i32::try_from(self.rate).unwrap_or(44_100);
        // SAFETY: the module is open and `out` holds `frames` stereo frames.
        let got = unsafe {
            ffi::openmpt_module_read_interleaved_float_stereo(
                self.module.as_ptr(),
                rate,
                frames,
                out.as_mut_ptr(),
            )
        };
        let got = got.min(frames);
        out.truncate(got * 2);
        got
    }
}

/// A duration in seconds from libopenmpt; `None` when it is not a sensible length.
fn seconds(value: f64) -> Option<Duration> {
    (value.is_finite() && value > 0.0).then(|| Duration::from_secs_f64(value))
}

impl SongDecoder for OpenMptDecoder {
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
        2
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn read(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<usize, SongError> {
        out.clear();
        let left = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(usize::MAX);
        let got = self.render(max_frames.min(left), out);
        self.position += got as u64;
        Ok(got)
    }

    fn seek(&mut self, frame: u64) -> Result<(), SongError> {
        let target = frame.min(self.frames);
        let seconds = target as f64 / f64::from(self.rate);
        // SAFETY: the module is open.
        let landed =
            unsafe { ffi::openmpt_module_set_position_seconds(self.module.as_ptr(), seconds) };
        let mut at = ((landed.max(0.0)) * f64::from(self.rate)).round() as u64;
        if at > target {
            // The row starts after the target (rounding): start the subsong over.
            self.module.select(self.subsong)?;
            at = 0;
        }
        let mut scratch = std::mem::take(&mut self.scratch);
        while at < target {
            let want = usize::try_from(target - at)
                .unwrap_or(SKIP_BLOCK)
                .min(SKIP_BLOCK);
            let got = self.render(want, &mut scratch);
            if got == 0 {
                break;
            }
            at += got as u64;
        }
        self.scratch = scratch;
        self.position = at;
        Ok(())
    }
}
