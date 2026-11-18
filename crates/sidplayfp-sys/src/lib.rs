// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw bindings to libsidplayfp through a small C++ shim.
//!
//! libsidplayfp is a C++ library; `shim/shim.cpp` wraps the parts the player uses in
//! C functions (declared in `shim/shim.h`), and no C++ exception leaves them. With
//! libsidplayfp 2.14 and newer the shim takes each chip's samples (`play(cycles)` and
//! `buffers`) and mixes them itself, without the random dither of the library's
//! mixer; older versions use `play(buffer, count)`.
//!
//! See the crate's README for its place in the workspace.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_long, c_uint};

/// A loaded tune with its emulator (opaque).
#[repr(C)]
#[derive(Debug)]
pub struct hk_sid {
    _private: [u8; 0],
}

/// An opened song-length database (opaque).
#[repr(C)]
#[derive(Debug)]
pub struct hk_sid_db {
    _private: [u8; 0],
}

/// The length of the tune's MD5 in hex digits; buffers for [`hk_sid_md5`] need one
/// byte more.
pub const MD5_LENGTH: usize = 32;
/// The size of the KERNAL and BASIC ROM images.
pub const ROM_8K: usize = 8192;
/// The size of the character ROM image.
pub const CHARGEN_LEN: usize = 4096;

unsafe extern "C" {
    pub fn hk_sid_open(
        data: *const u8,
        length: u32,
        rate: u32,
        error: *mut c_char,
        error_len: usize,
    ) -> *mut hk_sid;
    pub fn hk_sid_free(sid: *mut hk_sid);
    pub fn hk_sid_songs(sid: *const hk_sid) -> c_uint;
    pub fn hk_sid_start_song(sid: *const hk_sid) -> c_uint;
    pub fn hk_sid_info_string(sid: *const hk_sid, index: c_uint) -> *const c_char;
    pub fn hk_sid_format(sid: *const hk_sid) -> *const c_char;
    pub fn hk_sid_md5(sid: *mut hk_sid, out: *mut c_char) -> c_int;
    pub fn hk_sid_set_roms(
        sid: *mut hk_sid,
        kernal: *const u8,
        basic: *const u8,
        chargen: *const u8,
    );
    pub fn hk_sid_select(
        sid: *mut hk_sid,
        song: c_uint,
        error: *mut c_char,
        error_len: usize,
    ) -> c_int;
    pub fn hk_sid_render(sid: *mut hk_sid, out: *mut i16, frames: usize) -> c_long;
    pub fn hk_sid_error(sid: *const hk_sid) -> *const c_char;

    pub fn hk_sid_db_open(
        path: *const c_char,
        error: *mut c_char,
        error_len: usize,
    ) -> *mut hk_sid_db;
    pub fn hk_sid_db_free(db: *mut hk_sid_db);
    pub fn hk_sid_db_length_ms(db: *mut hk_sid_db, sid: *mut hk_sid, song: c_uint) -> c_long;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_is_not_a_tune() {
        let data = [0u8; 200];
        let mut error = [0 as c_char; 256];
        // SAFETY: the buffers are valid for the lengths given.
        let sid =
            unsafe { hk_sid_open(data.as_ptr(), 200, 44_100, error.as_mut_ptr(), error.len()) };
        assert!(sid.is_null());
        // SAFETY: the shim wrote a NUL-terminated message into the buffer.
        let message = unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) };
        assert!(!message.to_bytes().is_empty());
    }

    #[test]
    fn a_missing_database_fails() {
        let mut error = [0 as c_char; 256];
        // SAFETY: the path is NUL-terminated; the buffer is valid for its length.
        let db = unsafe {
            hk_sid_db_open(
                c"/nonexistent/Songlengths.md5".as_ptr(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert!(db.is_null());
    }
}
