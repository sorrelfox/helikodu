// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw bindings to libvgmstream.
//!
//! Hand-written from the public C API of vgmstream, `libvgmstream.h` and
//! `libvgmstream_streamfile.h`, API version 1.1: only what the player uses. Every
//! function is the C function of the same name; see the headers for their contracts.
//!
//! The library is linked only with the `link` feature and when the build script
//! finds it ([`LINKED`]). Otherwise every function is a stub that fails: the
//! constructors return null and `libvgmstream_open_stream` returns -1, so a caller
//! that checks results, as it must anyway, needs no special case.
//!
//! See the crate's README for its place in the workspace.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};

/// Whether the library is linked. When false, the functions are stubs that fail.
pub const LINKED: bool = cfg!(vgmstream_linked);

/// The API major version these bindings are written for.
pub const API_VERSION_MAJOR: u32 = 1;

/// Output sample formats (`libvgmstream_sfmt_t`), as integers.
pub type libvgmstream_sfmt_t = c_int;
/// 16-bit signed integers.
pub const LIBVGMSTREAM_SFMT_PCM16: libvgmstream_sfmt_t = 1;
/// 24-bit signed integers in three bytes.
pub const LIBVGMSTREAM_SFMT_PCM24: libvgmstream_sfmt_t = 2;
/// 32-bit signed integers.
pub const LIBVGMSTREAM_SFMT_PCM32: libvgmstream_sfmt_t = 3;
/// 32-bit floats.
pub const LIBVGMSTREAM_SFMT_FLOAT: libvgmstream_sfmt_t = 4;

/// The format of the opened stream.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct libvgmstream_format_t {
    pub channels: c_int,
    pub sample_rate: c_int,
    pub sample_format: libvgmstream_sfmt_t,
    pub sample_size: c_int,
    pub channel_layout: u32,
    /// 0 for none, N for subsong N counted from 1.
    pub subsong_index: c_int,
    /// 0 when the format has no subsongs.
    pub subsong_count: c_int,
    pub input_channels: c_int,
    pub stream_samples: i64,
    pub loop_start: i64,
    pub loop_end: i64,
    pub loop_flag: bool,
    pub play_forever: bool,
    /// The frames that play, after the loop and fade configuration.
    pub play_samples: i64,
    pub stream_bitrate: c_int,
    pub codec_name: [c_char; 128],
    pub layout_name: [c_char; 128],
    pub meta_name: [c_char; 128],
    pub stream_name: [c_char; 256],
    pub format_id: c_int,
}

/// The decoder's buffer after `libvgmstream_render`.
#[repr(C)]
#[derive(Debug)]
pub struct libvgmstream_decoder_t {
    pub buf: *mut c_void,
    /// Frames in `buf`.
    pub buf_samples: c_int,
    pub buf_bytes: c_int,
    pub done: bool,
}

/// A library handle.
#[repr(C)]
#[derive(Debug)]
pub struct libvgmstream_t {
    pub priv_: *mut c_void,
    pub format: *const libvgmstream_format_t,
    pub decoder: *mut libvgmstream_decoder_t,
}

/// Playback configuration.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libvgmstream_config_t {
    pub disable_config_override: bool,
    pub allow_play_forever: bool,
    pub play_forever: bool,
    pub ignore_loop: bool,
    pub force_loop: bool,
    pub really_force_loop: bool,
    pub ignore_fade: bool,
    pub loop_count: f64,
    pub fade_time: f64,
    pub fade_delay: f64,
    pub stereo_track: c_int,
    pub auto_downmix_channels: c_int,
    pub force_sfmt: libvgmstream_sfmt_t,
}

/// A stream file: vgmstream's I/O interface.
#[repr(C)]
#[derive(Debug)]
pub struct libstreamfile_t {
    pub user_data: *mut c_void,
    pub read: Option<
        unsafe extern "C" fn(
            user_data: *mut c_void,
            dst: *mut u8,
            offset: i64,
            length: c_int,
        ) -> c_int,
    >,
    pub get_size: Option<unsafe extern "C" fn(user_data: *mut c_void) -> i64>,
    pub get_name: Option<unsafe extern "C" fn(user_data: *mut c_void) -> *const c_char>,
    pub open: Option<
        unsafe extern "C" fn(
            user_data: *mut c_void,
            filename: *const c_char,
        ) -> *mut libstreamfile_t,
    >,
    pub close: Option<unsafe extern "C" fn(libsf: *mut libstreamfile_t)>,
}

/// Options of [`libvgmstream_is_valid`].
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct libvgmstream_valid_t {
    pub is_extension: bool,
    pub skip_standard: bool,
    pub reject_extensionless: bool,
    pub accept_unknown: bool,
    pub accept_common: bool,
}

/// Silences the library's log.
pub const LIBVGMSTREAM_LOG_LEVEL_NONE: c_int = 100;

/// The library's log callback.
pub type libvgmstream_log_callback =
    Option<unsafe extern "C" fn(level: c_int, message: *const c_char)>;

#[cfg(vgmstream_linked)]
unsafe extern "C" {
    pub fn libvgmstream_get_version() -> u32;
    pub fn libvgmstream_init() -> *mut libvgmstream_t;
    pub fn libvgmstream_free(lib: *mut libvgmstream_t);
    pub fn libvgmstream_setup(lib: *mut libvgmstream_t, cfg: *mut libvgmstream_config_t);
    pub fn libvgmstream_open_stream(
        lib: *mut libvgmstream_t,
        libsf: *mut libstreamfile_t,
        subsong: c_int,
    ) -> c_int;
    pub fn libvgmstream_close_stream(lib: *mut libvgmstream_t);
    pub fn libvgmstream_render(lib: *mut libvgmstream_t) -> c_int;
    pub fn libvgmstream_get_play_position(lib: *mut libvgmstream_t) -> i64;
    pub fn libvgmstream_seek(lib: *mut libvgmstream_t, sample: i64);
    pub fn libvgmstream_reset(lib: *mut libvgmstream_t);
    pub fn libvgmstream_set_log(level: c_int, callback: libvgmstream_log_callback);
    pub fn libvgmstream_is_valid(filename: *const c_char, cfg: *mut libvgmstream_valid_t) -> bool;
    pub fn libstreamfile_open_from_stdio(filename: *const c_char) -> *mut libstreamfile_t;
    pub fn libstreamfile_close(libsf: *mut libstreamfile_t);
}

#[cfg(not(vgmstream_linked))]
pub use stubs::*;

/// Stand-ins for the library's functions when it is not linked: they fail.
#[cfg(not(vgmstream_linked))]
mod stubs {
    #![allow(clippy::missing_safety_doc, unreachable_pub)]

    use super::*;

    pub unsafe extern "C" fn libvgmstream_get_version() -> u32 {
        0
    }
    pub unsafe extern "C" fn libvgmstream_init() -> *mut libvgmstream_t {
        std::ptr::null_mut()
    }
    pub unsafe extern "C" fn libvgmstream_free(_lib: *mut libvgmstream_t) {}
    pub unsafe extern "C" fn libvgmstream_setup(
        _lib: *mut libvgmstream_t,
        _cfg: *mut libvgmstream_config_t,
    ) {
    }
    pub unsafe extern "C" fn libvgmstream_open_stream(
        _lib: *mut libvgmstream_t,
        _libsf: *mut libstreamfile_t,
        _subsong: c_int,
    ) -> c_int {
        -1
    }
    pub unsafe extern "C" fn libvgmstream_close_stream(_lib: *mut libvgmstream_t) {}
    pub unsafe extern "C" fn libvgmstream_render(_lib: *mut libvgmstream_t) -> c_int {
        -1
    }
    pub unsafe extern "C" fn libvgmstream_get_play_position(_lib: *mut libvgmstream_t) -> i64 {
        0
    }
    pub unsafe extern "C" fn libvgmstream_seek(_lib: *mut libvgmstream_t, _sample: i64) {}
    pub unsafe extern "C" fn libvgmstream_reset(_lib: *mut libvgmstream_t) {}
    pub unsafe extern "C" fn libvgmstream_set_log(
        _level: c_int,
        _callback: libvgmstream_log_callback,
    ) {
    }
    pub unsafe extern "C" fn libvgmstream_is_valid(
        _filename: *const c_char,
        _cfg: *mut libvgmstream_valid_t,
    ) -> bool {
        false
    }
    pub unsafe extern "C" fn libstreamfile_open_from_stdio(
        _filename: *const c_char,
    ) -> *mut libstreamfile_t {
        std::ptr::null_mut()
    }
    pub unsafe extern "C" fn libstreamfile_close(_libsf: *mut libstreamfile_t) {}
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::*;

    #[test]
    fn layouts_match_the_header_on_64_bit_linux() {
        // Offsets as a C compiler lays out the structs of libvgmstream.h 1.1 on
        // x86_64 and aarch64 Linux.
        if size_of::<usize>() != 8 {
            return;
        }
        assert_eq!(offset_of!(libvgmstream_format_t, stream_samples), 32);
        assert_eq!(offset_of!(libvgmstream_format_t, loop_flag), 56);
        assert_eq!(offset_of!(libvgmstream_format_t, play_samples), 64);
        assert_eq!(offset_of!(libvgmstream_format_t, codec_name), 76);
        assert_eq!(offset_of!(libvgmstream_format_t, format_id), 716);
        assert_eq!(size_of::<libvgmstream_format_t>(), 720);
        assert_eq!(offset_of!(libvgmstream_config_t, loop_count), 8);
        assert_eq!(offset_of!(libvgmstream_config_t, force_sfmt), 40);
        assert_eq!(size_of::<libvgmstream_config_t>(), 48);
        assert_eq!(size_of::<libstreamfile_t>(), 48);
    }

    #[test]
    fn stubs_fail_without_the_library() {
        if LINKED {
            return;
        }
        // SAFETY: the stubs take no pointers they dereference.
        assert!(unsafe { libvgmstream_init() }.is_null());
    }
}
