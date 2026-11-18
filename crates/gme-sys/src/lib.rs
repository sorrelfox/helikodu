// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw bindings to libgme (Game_Music_Emu).
//!
//! Hand-written from `gme/gme.h` (0.6.2 and newer): only the functions the player
//! uses. Every function is the C function of the same name; see the header for its
//! contract. Functions returning [`gme_err_t`] return null on success and a static
//! message otherwise.
//!
//! See the crate's README for its place in the workspace.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_long, c_short, c_void};

/// An emulator with a loaded file (opaque).
#[repr(C)]
#[derive(Debug)]
pub struct Music_Emu {
    _private: [u8; 0],
}

/// Null on success, otherwise a static error message.
pub type gme_err_t = *const c_char;

/// The type of an emulator (opaque pointer to a static description).
pub type gme_type_t = *const c_void;

/// What a file says about one of its tracks. Lengths are in milliseconds, -1 when
/// the file does not say.
#[repr(C)]
#[derive(Debug)]
pub struct gme_info_t {
    /// The total length, if the file states it.
    pub length: c_int,
    /// The length up to the looping section.
    pub intro_length: c_int,
    /// The length of the looping section.
    pub loop_length: c_int,
    /// The length to play: `length` if known, otherwise intro plus two loops,
    /// otherwise 150 000 ms.
    pub play_length: c_int,
    /// The fade length to use, -1 when the file does not state one (0.6.4 and
    /// newer; reserved and 0 before).
    pub fade_length: c_int,
    /// Reserved.
    pub i5: [c_int; 11],
    /// The system, such as "Nintendo NES".
    pub system: *const c_char,
    /// The game.
    pub game: *const c_char,
    /// The song's title.
    pub song: *const c_char,
    /// The composer.
    pub author: *const c_char,
    /// The copyright line.
    pub copyright: *const c_char,
    /// A comment.
    pub comment: *const c_char,
    /// Who ripped the file.
    pub dumper: *const c_char,
    /// Reserved.
    pub s7: [*const c_char; 9],
}

unsafe extern "C" {
    pub fn gme_open_data(
        data: *const c_void,
        size: c_long,
        out: *mut *mut Music_Emu,
        sample_rate: c_int,
    ) -> gme_err_t;
    pub fn gme_delete(emu: *mut Music_Emu);
    pub fn gme_identify_header(header: *const c_void) -> *const c_char;
    pub fn gme_type(emu: *const Music_Emu) -> gme_type_t;
    pub fn gme_type_system(kind: gme_type_t) -> *const c_char;
    pub fn gme_track_count(emu: *const Music_Emu) -> c_int;
    pub fn gme_track_info(
        emu: *const Music_Emu,
        out: *mut *mut gme_info_t,
        track: c_int,
    ) -> gme_err_t;
    pub fn gme_free_info(info: *mut gme_info_t);
    pub fn gme_start_track(emu: *mut Music_Emu, index: c_int) -> gme_err_t;
    pub fn gme_play(emu: *mut Music_Emu, count: c_int, out: *mut c_short) -> gme_err_t;
    pub fn gme_track_ended(emu: *const Music_Emu) -> c_int;
    pub fn gme_set_fade(emu: *mut Music_Emu, start_msec: c_int);
    pub fn gme_ignore_silence(emu: *mut Music_Emu, ignore: c_int);
    pub fn gme_set_autoload_playback_limit(emu: *mut Music_Emu, do_autoload_limit: c_int);
    pub fn gme_enable_accuracy(emu: *mut Music_Emu, enabled: c_int);
    pub fn gme_warning(emu: *mut Music_Emu) -> *const c_char;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_headers() {
        let mut header = [0u8; 4];
        header.copy_from_slice(b"NESM");
        // SAFETY: gme_identify_header reads four bytes.
        let found = unsafe { gme_identify_header(header.as_ptr().cast()) };
        // SAFETY: the result is a static, NUL-terminated string.
        let found = unsafe { std::ffi::CStr::from_ptr(found) };
        assert_eq!(found.to_bytes(), b"NSF");
        header.copy_from_slice(b"RIFF");
        // SAFETY: as above.
        let found =
            unsafe { std::ffi::CStr::from_ptr(gme_identify_header(header.as_ptr().cast())) };
        assert!(found.to_bytes().is_empty());
    }

    #[test]
    fn refuses_garbage() {
        let data = [0u8; 64];
        let mut emu = std::ptr::null_mut();
        // SAFETY: the buffer is valid for its length; `emu` receives the result.
        let error = unsafe { gme_open_data(data.as_ptr().cast(), 64, &raw mut emu, 44_100) };
        assert!(!error.is_null());
        assert!(emu.is_null());
    }
}
