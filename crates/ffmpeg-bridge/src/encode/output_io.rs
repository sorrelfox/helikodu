// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! An `AVIOContext` whose write and seek callbacks go to a file the caller opened
//! itself, so a muxer never resolves a path or URL for its output.

use std::ffi::{c_int, c_void};
use std::fs::File;
use std::io::{ErrorKind, Seek, SeekFrom, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use crate::error::Error;
use crate::ffi::ff;

/// The size of the buffer FFmpeg writes through.
const BUFFER_SIZE: usize = 64 * 1024;

/// The state behind the callbacks' `opaque` pointer.
struct OutputState {
    file: File,
    /// The current position.
    position: u64,
    /// The end of what was written.
    end: u64,
}

/// A writable, seekable `AVIOContext` over a file, with its buffer and state.
///
/// A format context with `AVFMT_FLAG_CUSTOM_IO` never frees its `pb`, so this value
/// must outlive the format context that uses it, and is freed after it.
pub(crate) struct OutputIo {
    ctx: *mut ff::AVIOContext,
    state: *mut OutputState,
}

// SAFETY: the context and the state are only reached through this value (and the
// format context its owner frees first); a File is Send.
unsafe impl Send for OutputIo {}

impl std::fmt::Debug for OutputIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputIo").finish_non_exhaustive()
    }
}

impl OutputIo {
    /// Writes into `file` from its start.
    pub(crate) fn new(file: File) -> Result<Self, Error> {
        let state = Box::into_raw(Box::new(OutputState {
            file,
            position: 0,
            end: 0,
        }));
        let mut io = Self {
            ctx: ptr::null_mut(),
            state,
        };
        // SAFETY: plain allocation; the result is checked.
        let buffer = unsafe { ff::av_malloc(BUFFER_SIZE) }.cast::<u8>();
        if buffer.is_null() {
            return Err(Error::out_of_memory("av_malloc"));
        }
        // SAFETY: buffer is an av_malloc'ed block of BUFFER_SIZE bytes, which the
        // context takes over on success. The opaque pointer stays valid until Drop,
        // which frees the context first. write_flag 1 makes it an output context;
        // a seek callback makes it seekable, which the MP4 muxer needs.
        io.ctx = unsafe {
            ff::avio_alloc_context(
                buffer,
                BUFFER_SIZE as c_int,
                1,
                state.cast(),
                None,
                Some(write_callback),
                Some(seek_callback),
            )
        };
        if io.ctx.is_null() {
            // SAFETY: the context was not created, so the buffer is still ours.
            unsafe { ff::av_free(buffer.cast()) };
            return Err(Error::out_of_memory("avio_alloc_context"));
        }
        Ok(io)
    }

    /// The context, for `AVFormatContext::pb`.
    pub(crate) fn as_ptr(&self) -> *mut ff::AVIOContext {
        self.ctx
    }

    /// Writes out what FFmpeg still buffers.
    pub(crate) fn flush(&mut self) {
        // SAFETY: ctx is a valid output context.
        unsafe { ff::avio_flush(self.ctx) };
    }

    /// The number of bytes written, to the furthest point.
    pub(crate) fn written(&self) -> u64 {
        // SAFETY: state is valid until Drop; no callback runs during this call.
        unsafe { (*self.state).end }
    }

    /// Flushes the file to the disk.
    pub(crate) fn sync(&mut self) -> std::io::Result<()> {
        // SAFETY: as in `written`.
        unsafe { (*self.state).file.sync_all() }
    }
}

impl Drop for OutputIo {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            // SAFETY: ctx came from avio_alloc_context and no format context uses it
            // any more. FFmpeg may have replaced the buffer, so the current one is
            // freed, then the context, which does not free its buffer itself.
            unsafe {
                ff::av_freep(ptr::addr_of_mut!((*self.ctx).buffer).cast());
                ff::avio_context_free(&mut self.ctx);
            }
        }
        if !self.state.is_null() {
            // SAFETY: state came from Box::into_raw in new, and the context that
            // pointed at it is gone.
            drop(unsafe { Box::from_raw(self.state) });
        }
    }
}

fn io_error_code(e: &std::io::Error) -> c_int {
    ff::AVERROR(e.raw_os_error().unwrap_or(libc::EIO))
}

/// The write callback: writes all of `buf` at the current position.
unsafe extern "C" fn write_callback(opaque: *mut c_void, buf: *const u8, size: c_int) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Ok(len) = usize::try_from(size) else {
            return ff::AVERROR(libc::EINVAL);
        };
        if opaque.is_null() || (buf.is_null() && len > 0) {
            return ff::AVERROR(libc::EINVAL);
        }
        if len == 0 {
            return 0;
        }
        // SAFETY: opaque is the OutputState given to avio_alloc_context, alive
        // until the context is freed; FFmpeg calls one context's callbacks from one
        // thread at a time.
        let state = unsafe { &mut *opaque.cast::<OutputState>() };
        // SAFETY: FFmpeg passes a buffer readable for `size` bytes.
        let src = unsafe { std::slice::from_raw_parts(buf, len) };
        loop {
            match state.file.write_all(src) {
                Ok(()) => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return io_error_code(&e),
            }
        }
        state.position += len as u64;
        state.end = state.end.max(state.position);
        size
    }));
    result.unwrap_or(ff::AVERROR_EXTERNAL)
}

/// The seek callback: answers `AVSEEK_SIZE` and ignores `AVSEEK_FORCE`.
unsafe extern "C" fn seek_callback(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if opaque.is_null() {
            return i64::from(ff::AVERROR(libc::EINVAL));
        }
        // SAFETY: as in write_callback.
        let state = unsafe { &mut *opaque.cast::<OutputState>() };
        if whence & ff::AVSEEK_SIZE != 0 {
            return i64::try_from(state.end).unwrap_or(i64::MAX);
        }
        let pos = match whence & !ff::AVSEEK_FORCE {
            libc::SEEK_SET => match u64::try_from(offset) {
                Ok(o) => SeekFrom::Start(o),
                Err(_) => return i64::from(ff::AVERROR(libc::EINVAL)),
            },
            libc::SEEK_CUR => SeekFrom::Current(offset),
            libc::SEEK_END => SeekFrom::End(offset),
            _ => return i64::from(ff::AVERROR(libc::EINVAL)),
        };
        match state.file.seek(pos) {
            Ok(p) => {
                state.position = p;
                i64::try_from(p).unwrap_or(i64::from(ff::AVERROR(libc::EOVERFLOW)))
            }
            Err(e) => i64::from(io_error_code(&e)),
        }
    }));
    result.unwrap_or(i64::from(ff::AVERROR_EXTERNAL))
}
