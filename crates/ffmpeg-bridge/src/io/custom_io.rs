// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! An `AVIOContext` whose read and seek callbacks go to a file the player opened
//! itself, so FFmpeg never resolves a path or URL for the main input.

use std::ffi::{c_int, c_void};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use crate::error::Error;
use crate::ffi::ff;

/// The size of the buffer FFmpeg reads through.
const BUFFER_SIZE: usize = 64 * 1024;

/// What custom I/O reads from: a file, or in unit tests anything seekable.
pub(crate) trait ByteSource: Read + Seek + Send {}

impl<T: Read + Seek + Send> ByteSource for T {}

/// The state behind the callbacks' `opaque` pointer.
struct IoState {
    source: Box<dyn ByteSource>,
    /// The total size, for `AVSEEK_SIZE`; `None` when unknown.
    size: Option<u64>,
}

/// An `AVIOContext` with its buffer and its state, owned together.
///
/// A format context opened with `AVFMT_FLAG_CUSTOM_IO` never frees its `pb`, so this
/// value must outlive the format context that uses it, and is freed after it.
pub(crate) struct CustomIo {
    ctx: *mut ff::AVIOContext,
    state: *mut IoState,
}

// SAFETY: the context and the state are only reached through this value (and the
// format context that its owner drops first). FFmpeg keeps no thread-local state
// for an AVIOContext, and the byte source is Send.
unsafe impl Send for CustomIo {}

impl std::fmt::Debug for CustomIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomIo").finish_non_exhaustive()
    }
}

impl CustomIo {
    /// Wraps `source`. `size` answers FFmpeg's size queries.
    pub(crate) fn new(source: Box<dyn ByteSource>, size: Option<u64>) -> Result<Self, Error> {
        let state = Box::into_raw(Box::new(IoState { source, size }));
        // From here on, Drop frees whatever has been set up so far.
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
        // which frees the context first. The context is read-only (write_flag 0).
        io.ctx = unsafe {
            ff::avio_alloc_context(
                buffer,
                BUFFER_SIZE as c_int,
                0,
                state.cast(),
                Some(read_callback),
                None,
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
}

impl Drop for CustomIo {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            // SAFETY: ctx came from avio_alloc_context and no format context uses
            // it any more (its owner closes the format context first). FFmpeg may
            // have replaced the buffer, so the current one is freed, then the
            // context, which does not free its buffer itself.
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

/// Maps an I/O error to an `AVERROR` code.
fn io_error_code(e: &std::io::Error) -> c_int {
    ff::AVERROR(e.raw_os_error().unwrap_or(libc::EIO))
}

/// The read callback: fills `buf` with up to `size` bytes.
///
/// Never returns 0: FFmpeg reads that as progress without data. A panic becomes
/// `AVERROR_EXTERNAL`, because unwinding into C is undefined behaviour.
unsafe extern "C" fn read_callback(opaque: *mut c_void, buf: *mut u8, size: c_int) -> c_int {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Ok(len) = usize::try_from(size) else {
            return ff::AVERROR(libc::EINVAL);
        };
        if len == 0 || buf.is_null() || opaque.is_null() {
            return ff::AVERROR(libc::EINVAL);
        }
        // SAFETY: opaque is the IoState given to avio_alloc_context, alive until
        // the context is freed. FFmpeg calls the callbacks of one context from one
        // thread at a time, so this is the only reference.
        let state = unsafe { &mut *opaque.cast::<IoState>() };
        // SAFETY: FFmpeg passes a buffer writable for `size` bytes.
        let dst = unsafe { std::slice::from_raw_parts_mut(buf, len) };
        loop {
            match state.source.read(dst) {
                Ok(0) => return ff::AVERROR_EOF,
                // n <= len, which came from a c_int.
                Ok(n) => return c_int::try_from(n).unwrap_or(size),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return io_error_code(&e),
            }
        }
    }));
    result.unwrap_or(ff::AVERROR_EXTERNAL)
}

/// The seek callback: answers `AVSEEK_SIZE` and ignores `AVSEEK_FORCE`.
unsafe extern "C" fn seek_callback(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if opaque.is_null() {
            return i64::from(ff::AVERROR(libc::EINVAL));
        }
        // SAFETY: as in read_callback.
        let state = unsafe { &mut *opaque.cast::<IoState>() };
        if whence & ff::AVSEEK_SIZE != 0 {
            return match state.size.and_then(|s| i64::try_from(s).ok()) {
                Some(size) => size,
                None => i64::from(ff::AVERROR(libc::ENOSYS)),
            };
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
        match state.source.seek(pos) {
            Ok(p) => i64::try_from(p).unwrap_or(i64::from(ff::AVERROR(libc::EOVERFLOW))),
            Err(e) => i64::from(io_error_code(&e)),
        }
    }));
    result.unwrap_or(i64::from(ff::AVERROR_EXTERNAL))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    struct Panicking;

    impl Read for Panicking {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("a panic inside the read callback");
        }
    }

    impl Seek for Panicking {
        fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
            panic!("a panic inside the seek callback");
        }
    }

    fn state_of(io: &CustomIo) -> *mut c_void {
        io.state.cast()
    }

    #[test]
    fn reads_seeks_and_reports_the_size() {
        let io =
            CustomIo::new(Box::new(Cursor::new(vec![1u8, 2, 3, 4])), Some(4)).expect("allocated");
        let mut buf = [0u8; 8];
        // SAFETY: the opaque pointer and buffer are valid for the call.
        let n = unsafe { read_callback(state_of(&io), buf.as_mut_ptr(), 8) };
        assert_eq!(n, 4);
        assert_eq!(&buf[..4], &[1, 2, 3, 4]);
        // SAFETY: as above.
        let n = unsafe { read_callback(state_of(&io), buf.as_mut_ptr(), 8) };
        assert_eq!(n, ff::AVERROR_EOF);
        // SAFETY: as above.
        let size = unsafe { seek_callback(state_of(&io), 0, ff::AVSEEK_SIZE) };
        assert_eq!(size, 4);
        // SAFETY: as above.
        let pos = unsafe { seek_callback(state_of(&io), 1, libc::SEEK_SET | ff::AVSEEK_FORCE) };
        assert_eq!(pos, 1);
        // SAFETY: as above.
        let bad = unsafe { seek_callback(state_of(&io), -1, libc::SEEK_SET) };
        assert_eq!(bad, i64::from(ff::AVERROR(libc::EINVAL)));
    }

    #[test]
    fn panics_become_error_codes() {
        let io = CustomIo::new(Box::new(Panicking), None).expect("allocated");
        let mut buf = [0u8; 8];
        // SAFETY: the opaque pointer and buffer are valid for the call.
        let n = unsafe { read_callback(state_of(&io), buf.as_mut_ptr(), 8) };
        assert_eq!(n, ff::AVERROR_EXTERNAL);
        // SAFETY: as above.
        let pos = unsafe { seek_callback(state_of(&io), 0, libc::SEEK_SET) };
        assert_eq!(pos, i64::from(ff::AVERROR_EXTERNAL));
    }
}
