// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Records every `avcodec_open2` call made in this process.
//!
//! The spike defines `avcodec_open2` itself and exports it from the executable
//! (see `build.rs`). The dynamic linker then binds every caller to this
//! definition, including the calls libavformat makes internally from
//! `avformat_find_stream_info`, because the executable comes first in the
//! symbol lookup order. The wrapper records the codec and forwards to the real
//! function, found with `dlsym(RTLD_NEXT, ...)`.
//!
//! This is how the spike proves that no video decoder is ever opened, rather
//! than only showing that the bridge itself never asks for one.

use std::cell::Cell;
use std::ffi::{CStr, c_int};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock};

use ffmpeg_sys_next as ff;

/// One recorded `avcodec_open2` call.
#[derive(Clone, Debug)]
pub struct OpenRecord {
    pub codec: String,
    /// `AVMediaType` as a plain integer (0 = video, 1 = audio, 3 = subtitle).
    pub media_type: c_int,
    /// True when the bridge opened the decoder; false for calls made inside FFmpeg.
    pub by_bridge: bool,
    /// The return value: negative means FFmpeg refused to open the decoder
    /// (for example because of `codec_whitelist`), so it never ran.
    pub ret: c_int,
}

impl OpenRecord {
    pub fn opened(&self) -> bool {
        self.ret >= 0
    }

    pub fn is_video(&self) -> bool {
        self.media_type == ff::AVMediaType::AVMEDIA_TYPE_VIDEO as c_int
    }
}

static RECORDS: Mutex<Vec<OpenRecord>> = Mutex::new(Vec::new());

type OpenFn = unsafe extern "C" fn(
    *mut ff::AVCodecContext,
    *const ff::AVCodec,
    *mut *mut ff::AVDictionary,
) -> c_int;

static REAL_OPEN: OnceLock<Option<OpenFn>> = OnceLock::new();

thread_local! {
    static BRIDGE_OPENING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` with calls to `avcodec_open2` attributed to the bridge.
pub fn as_bridge<T>(f: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BRIDGE_OPENING.with(|b| b.set(false));
        }
    }
    BRIDGE_OPENING.with(|b| b.set(true));
    let _reset = Reset;
    f()
}

/// Clears the record list and returns what was recorded since the last call.
pub fn take() -> Vec<OpenRecord> {
    match RECORDS.lock() {
        Ok(mut v) => std::mem::take(&mut *v),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    }
}

/// True when the wrapper below is the definition the process actually uses.
/// Checked once at start-up so a silently ineffective probe cannot pass.
pub fn is_active() -> bool {
    // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name is always safe
    // to call; it only looks the symbol up.
    let global = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"avcodec_open2".as_ptr()) };
    global as usize == avcodec_open2 as OpenFn as usize
}

fn real_open() -> Option<OpenFn> {
    *REAL_OPEN.get_or_init(|| {
        // SAFETY: RTLD_NEXT lookup of a NUL-terminated symbol name; the result is
        // either NULL or the address of libavcodec's avcodec_open2.
        let sym = unsafe { libc::dlsym(libc::RTLD_NEXT, c"avcodec_open2".as_ptr()) };
        if sym.is_null() {
            None
        } else {
            // SAFETY: the symbol is libavcodec's avcodec_open2, whose C
            // signature matches OpenFn exactly.
            Some(unsafe { std::mem::transmute::<*mut libc::c_void, OpenFn>(sym) })
        }
    })
}

/// Interposes libavcodec's `avcodec_open2`. Same signature and contract.
///
/// # Safety
/// Same as `avcodec_open2`: `avctx` must be a valid codec context, `codec` a
/// valid codec or NULL, and `options` NULL or a valid dictionary pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn avcodec_open2(
    avctx: *mut ff::AVCodecContext,
    codec: *const ff::AVCodec,
    options: *mut *mut ff::AVDictionary,
) -> c_int {
    let ret = match real_open() {
        // SAFETY: forwards the caller's arguments unchanged to the real function.
        Some(f) => unsafe { f(avctx, codec, options) },
        None => ff::AVERROR(libc::ENOSYS),
    };
    // Recording must never unwind into C.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: FFmpeg passes a valid AVCodecContext (or NULL, which is checked).
        // Enum fields are read as raw c_int, because a value unknown to the
        // bindings would be undefined behaviour as a Rust enum.
        let (name, media_type) = unsafe {
            let media_type = if avctx.is_null() {
                -1
            } else {
                std::ptr::addr_of!((*avctx).codec_type)
                    .cast::<c_int>()
                    .read()
            };
            let name = if !codec.is_null() && !(*codec).name.is_null() {
                CStr::from_ptr((*codec).name).to_string_lossy().into_owned()
            } else {
                "<unknown>".to_owned()
            };
            (name, media_type)
        };
        let rec = OpenRecord {
            codec: name,
            media_type,
            by_bridge: BRIDGE_OPENING.with(Cell::get),
            ret,
        };
        if let Ok(mut v) = RECORDS.lock() {
            v.push(rec);
        }
    }));
    ret
}
