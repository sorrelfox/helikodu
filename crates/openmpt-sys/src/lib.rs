// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw bindings to libopenmpt.
//!
//! Hand-written from `libopenmpt/libopenmpt.h` (API version 0.6 and newer): only the
//! functions the player uses. Every function is the C function of the same name;
//! see the library's documentation for its contract.
//!
//! See the crate's README for its place in the workspace.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_double, c_int, c_void};

/// An opened module (opaque).
#[repr(C)]
#[derive(Debug)]
pub struct openmpt_module {
    _private: [u8; 0],
}

/// A log callback.
pub type openmpt_log_func = Option<unsafe extern "C" fn(message: *const c_char, user: *mut c_void)>;
/// An error callback; returns a combination of the `OPENMPT_ERROR_FUNC_RESULT_*` flags.
pub type openmpt_error_func =
    Option<unsafe extern "C" fn(error: c_int, user: *mut c_void) -> c_int>;

/// One initial ctl and its value, for [`openmpt_module_create_from_memory2`]; an
/// array of them ends with a pair of null pointers.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct openmpt_module_initial_ctl {
    /// The ctl's name.
    pub ctl: *const c_char,
    /// Its value as text.
    pub value: *const c_char,
}

/// No error.
pub const OPENMPT_ERROR_OK: c_int = 0;

/// Probe for module formats.
pub const OPENMPT_PROBE_FILE_HEADER_FLAGS_MODULES: u64 = 0x1;
/// Probe for container formats (compressed or wrapped modules).
pub const OPENMPT_PROBE_FILE_HEADER_FLAGS_CONTAINERS: u64 = 0x2;
/// Modules and containers.
pub const OPENMPT_PROBE_FILE_HEADER_FLAGS_DEFAULT: u64 =
    OPENMPT_PROBE_FILE_HEADER_FLAGS_MODULES | OPENMPT_PROBE_FILE_HEADER_FLAGS_CONTAINERS;
/// The header is that of a supported format.
pub const OPENMPT_PROBE_FILE_HEADER_RESULT_SUCCESS: c_int = 1;
/// The header is not that of a supported format.
pub const OPENMPT_PROBE_FILE_HEADER_RESULT_FAILURE: c_int = 0;
/// More bytes are needed to tell.
pub const OPENMPT_PROBE_FILE_HEADER_RESULT_WANTMOREDATA: c_int = -1;
/// The probe failed.
pub const OPENMPT_PROBE_FILE_HEADER_RESULT_ERROR: c_int = -255;

/// Master gain in millibel.
pub const OPENMPT_MODULE_RENDER_MASTERGAIN_MILLIBEL: c_int = 1;
/// Stereo separation in percent (0 to 200, default 100).
pub const OPENMPT_MODULE_RENDER_STEREOSEPARATION_PERCENT: c_int = 2;
/// Interpolation filter length in taps: 0 (the default), 1 (none), 2 (linear), 4
/// (cubic) or 8 (windowed sinc).
pub const OPENMPT_MODULE_RENDER_INTERPOLATIONFILTER_LENGTH: c_int = 3;
/// Volume ramping strength (-1 default, 0 off, up to 10).
pub const OPENMPT_MODULE_RENDER_VOLUMERAMPING_STRENGTH: c_int = 4;

unsafe extern "C" {
    pub fn openmpt_get_library_version() -> u32;
    pub fn openmpt_free_string(str: *const c_char);
    pub fn openmpt_log_func_silent(message: *const c_char, user: *mut c_void);
    pub fn openmpt_error_func_ignore(error: c_int, user: *mut c_void) -> c_int;

    pub fn openmpt_probe_file_header_without_filesize(
        flags: u64,
        data: *const c_void,
        size: usize,
        logfunc: openmpt_log_func,
        loguser: *mut c_void,
        errfunc: openmpt_error_func,
        erruser: *mut c_void,
        error: *mut c_int,
        error_message: *mut *const c_char,
    ) -> c_int;

    pub fn openmpt_module_create_from_memory2(
        filedata: *const c_void,
        filesize: usize,
        logfunc: openmpt_log_func,
        loguser: *mut c_void,
        errfunc: openmpt_error_func,
        erruser: *mut c_void,
        error: *mut c_int,
        error_message: *mut *const c_char,
        ctls: *const openmpt_module_initial_ctl,
    ) -> *mut openmpt_module;
    pub fn openmpt_module_destroy(module: *mut openmpt_module);

    pub fn openmpt_module_select_subsong(module: *mut openmpt_module, subsong: i32) -> c_int;
    pub fn openmpt_module_get_selected_subsong(module: *mut openmpt_module) -> i32;
    pub fn openmpt_module_get_num_subsongs(module: *mut openmpt_module) -> i32;
    pub fn openmpt_module_get_subsong_name(
        module: *mut openmpt_module,
        index: i32,
    ) -> *const c_char;
    pub fn openmpt_module_set_repeat_count(module: *mut openmpt_module, repeat_count: i32)
    -> c_int;
    pub fn openmpt_module_get_duration_seconds(module: *mut openmpt_module) -> c_double;
    pub fn openmpt_module_set_position_seconds(
        module: *mut openmpt_module,
        seconds: c_double,
    ) -> c_double;
    pub fn openmpt_module_get_position_seconds(module: *mut openmpt_module) -> c_double;
    pub fn openmpt_module_set_render_param(
        module: *mut openmpt_module,
        param: c_int,
        value: i32,
    ) -> c_int;
    pub fn openmpt_module_read_interleaved_float_stereo(
        module: *mut openmpt_module,
        samplerate: i32,
        count: usize,
        interleaved_stereo: *mut f32,
    ) -> usize;
    pub fn openmpt_module_get_metadata(
        module: *mut openmpt_module,
        key: *const c_char,
    ) -> *const c_char;
    pub fn openmpt_module_ctl_set_text(
        module: *mut openmpt_module,
        ctl: *const c_char,
        value: *const c_char,
    ) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_reports_a_version() {
        // SAFETY: no arguments; returns a plain number.
        let version = unsafe { openmpt_get_library_version() };
        // 0xMMmmpppp: major 0, minor 6 or newer.
        assert!(version >= 0x0006_0000, "{version:#x}");
    }

    #[test]
    fn probes_a_header() {
        let mut header = vec![0u8; 1084];
        header[1080..1084].copy_from_slice(b"M.K.");
        // SAFETY: the buffer is valid for its length; the callbacks are the
        // library's own; error outputs may be null.
        let result = unsafe {
            openmpt_probe_file_header_without_filesize(
                OPENMPT_PROBE_FILE_HEADER_FLAGS_DEFAULT,
                header.as_ptr().cast(),
                header.len(),
                Some(openmpt_log_func_silent),
                std::ptr::null_mut(),
                Some(openmpt_error_func_ignore),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_ne!(result, OPENMPT_PROBE_FILE_HEADER_RESULT_ERROR);
    }
}
