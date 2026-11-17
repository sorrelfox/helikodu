// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Records every `avcodec_open2` call made on the current thread, including the
//! ones FFmpeg makes inside `avformat_find_stream_info`.
//!
//! This test binary defines `avcodec_open2` itself. libavformat references the
//! symbol, so the linker exports the executable's definition, and the dynamic
//! linker binds libavformat's calls to it, because the executable comes first in
//! the lookup order. The wrapper records the call and forwards to libavcodec's
//! function, found with `dlsym(RTLD_NEXT)`. [`is_active`] checks that the wrapper
//! really is the definition in use, so the proof cannot pass silently.

use std::cell::RefCell;
use std::ffi::{CStr, c_int};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use ffmpeg_sys_next as ff;

/// One recorded call.
#[derive(Clone, Debug)]
pub(crate) struct OpenRecord {
    pub(crate) codec: String,
    /// `AVMediaType` as an integer.
    pub(crate) media_type: c_int,
    /// Negative when FFmpeg refused to open the decoder, so it never ran.
    pub(crate) ret: c_int,
}

impl OpenRecord {
    pub(crate) fn is_video(&self) -> bool {
        self.media_type == ff::AVMediaType::AVMEDIA_TYPE_VIDEO as c_int
    }

    pub(crate) fn opened(&self) -> bool {
        self.ret >= 0
    }
}

thread_local! {
    static RECORDS: RefCell<Vec<OpenRecord>> = const { RefCell::new(Vec::new()) };
}

/// Returns and clears this thread's records.
pub(crate) fn take() -> Vec<OpenRecord> {
    RECORDS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

type OpenFn = unsafe extern "C" fn(
    *mut ff::AVCodecContext,
    *const ff::AVCodec,
    *mut *mut ff::AVDictionary,
) -> c_int;

/// Whether the wrapper is the definition the process uses.
pub(crate) fn is_active() -> bool {
    // SAFETY: a lookup of a NUL-terminated symbol name.
    let global = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"avcodec_open2".as_ptr()) };
    global as usize == avcodec_open2 as OpenFn as usize
}

fn real_open() -> Option<OpenFn> {
    static REAL: OnceLock<Option<OpenFn>> = OnceLock::new();
    *REAL.get_or_init(|| {
        // SAFETY: a lookup of a NUL-terminated symbol name in the libraries after
        // this executable.
        let sym = unsafe { libc::dlsym(libc::RTLD_NEXT, c"avcodec_open2".as_ptr()) };
        if sym.is_null() {
            None
        } else {
            // SAFETY: the symbol is libavcodec's avcodec_open2, whose C signature
            // is exactly OpenFn.
            Some(unsafe { std::mem::transmute::<*mut libc::c_void, OpenFn>(sym) })
        }
    })
}

/// Stands in for libavcodec's `avcodec_open2`: same signature and contract.
///
/// # Safety
/// As `avcodec_open2`.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn avcodec_open2(
    avctx: *mut ff::AVCodecContext,
    codec: *const ff::AVCodec,
    options: *mut *mut ff::AVDictionary,
) -> c_int {
    let ret = match real_open() {
        // SAFETY: forwards the caller's arguments unchanged.
        Some(f) => unsafe { f(avctx, codec, options) },
        None => ff::AVERROR(libc::ENOSYS),
    };
    // Recording must never unwind into C.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: FFmpeg passes a valid context (NULL is checked); the media type is
        // read as an integer; the codec name is a static string.
        let (media_type, codec) = unsafe {
            let media_type = if avctx.is_null() {
                -1
            } else {
                std::ptr::addr_of!((*avctx).codec_type)
                    .cast::<c_int>()
                    .read()
            };
            let codec = if !codec.is_null() && !(*codec).name.is_null() {
                CStr::from_ptr((*codec).name).to_string_lossy().into_owned()
            } else {
                "unknown".to_owned()
            };
            (media_type, codec)
        };
        RECORDS.with(|r| {
            r.borrow_mut().push(OpenRecord {
                codec,
                media_type,
                ret,
            })
        });
    }));
    ret
}
