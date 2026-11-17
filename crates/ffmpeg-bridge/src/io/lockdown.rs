// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The locks that keep FFmpeg on the one local file it was given.
//!
//! A local file can still make FFmpeg reach out: an HLS playlist or a concat script
//! names other files or URLs, and a QuickTime file can carry external data
//! references. No single setting stops all of these, so every open applies five
//! together:
//!
//! 1. **Custom I/O** with **no file name** ([`CustomIo`](super::CustomIo)): FFmpeg
//!    probes by content only and has no path to resolve references against.
//! 2. **`protocol_whitelist = file`**: custom I/O removes the `file,crypto,data`
//!    limit FFmpeg applies when it opens a path itself, so this restores a limit.
//! 3. **A deny-all `io_open`** ([`deny_io_open`]): every nested open fails.
//! 4. **`format_whitelist`** ([`FORMAT_WHITELIST`]): only demuxers that read one
//!    self-contained file. This is the only lock that stops the `concat` demuxer,
//!    which opens its files through a format context of its own.
//! 5. **`codec_whitelist` = every audio decoder**: `avformat_find_stream_info`
//!    opens decoders even for discarded streams, video and cover art included; with
//!    the whitelist those calls fail before the decoder is initialised.
//!
//! FFmpeg silently ignores an option it does not know, which would turn a lock off
//! without notice, so [`LockOptions::check_leftovers`] and [`verify_locks`] check
//! that every lock arrived.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::OnceLock;

use crate::error::{Error, check};
use crate::ffi::{self, ff, media_type};

/// The demuxers the player accepts. Each reads one self-contained file.
///
/// Left out on purpose because they open other resources: `hls`, `dash`, `concat`,
/// `imf`, the image-sequence demuxers, `mlv`, `vobsub`, `dvdvideo`, `avisynth`,
/// `vapoursynth`, `sap`, `rtsp`, `sdp` and every device input. A name FFmpeg does
/// not know is harmless here: it matches nothing.
pub(crate) const FORMAT_WHITELIST: &CStr = c"aac,ac3,aea,aiff,amr,amrnb,amrwb,ape,asf,au,avi,\
bit,caf,dsf,dts,dtshd,dv,eac3,flac,flv,g722,g723_1,g726,g726le,g729,gsm,iff,loas,matroska,mlp,\
mov,mp3,mpc,mpc8,mpegps,mpegts,mxf,nut,oma,ogg,rm,shn,tak,truehd,tta,voc,vqf,w64,wav,wsd,wv,xwma";

/// The only protocol nested opens could use, if one ever got past `io_open`.
const PROTOCOL_WHITELIST: &CStr = c"file";

/// Every audio decoder of the FFmpeg build that is running, comma-separated, as
/// the value of `codec_whitelist`. Built once with `av_codec_iterate`.
pub(crate) fn audio_decoder_whitelist() -> &'static CStr {
    static LIST: OnceLock<CString> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut names: Vec<Vec<u8>> = Vec::new();
        let mut cursor: *mut c_void = ptr::null_mut();
        loop {
            // SAFETY: av_codec_iterate walks FFmpeg's static codec table with an
            // opaque cursor that starts as NULL.
            let codec = unsafe { ff::av_codec_iterate(&mut cursor) };
            if codec.is_null() {
                break;
            }
            // SAFETY: codec points to a static AVCodec. Its type is read as an
            // integer, and its name is a static NUL-terminated string.
            let (is_decoder, kind, name) = unsafe {
                (
                    ff::av_codec_is_decoder(codec) != 0,
                    ffi::enum_field(ptr::addr_of!((*codec).type_)),
                    (*codec).name,
                )
            };
            if is_decoder && kind == media_type::AUDIO && !name.is_null() {
                // SAFETY: checked for NULL; a static NUL-terminated string.
                names.push(unsafe { CStr::from_ptr(name) }.to_bytes().to_vec());
            }
        }
        // Names never contain NUL; an empty list still yields a valid string, which
        // then allows no decoder at all.
        CString::new(names.join(&b',')).unwrap_or_else(|_| CString::from(c"none"))
    })
}

/// The options handed to `avformat_open_input`.
#[derive(Debug)]
pub(crate) struct LockOptions {
    dict: *mut ff::AVDictionary,
}

impl LockOptions {
    /// The three whitelists.
    pub(crate) fn new() -> Result<Self, Error> {
        let mut options = Self {
            dict: ptr::null_mut(),
        };
        options.set(c"protocol_whitelist", PROTOCOL_WHITELIST)?;
        options.set(c"format_whitelist", FORMAT_WHITELIST)?;
        options.set(c"codec_whitelist", audio_decoder_whitelist())?;
        Ok(options)
    }

    fn set(&mut self, key: &CStr, value: &CStr) -> Result<(), Error> {
        // SAFETY: both strings are NUL-terminated and FFmpeg copies them; dict is
        // NULL or a dictionary this value owns.
        check("av_dict_set", unsafe {
            ff::av_dict_set(&mut self.dict, key.as_ptr(), value.as_ptr(), 0)
        })
        .map(drop)
    }

    /// The dictionary pointer for `avformat_open_input`, which replaces it with a
    /// dictionary of the options it did not use.
    pub(crate) fn as_mut_ptr(&mut self) -> *mut *mut ff::AVDictionary {
        &mut self.dict
    }

    /// Fails if FFmpeg left any option unused.
    pub(crate) fn check_leftovers(&self) -> Result<(), Error> {
        // SAFETY: dict is NULL or the dictionary FFmpeg handed back.
        match unsafe { ffi::dict_entries(self.dict) }.into_iter().next() {
            Some((key, _)) => Err(Error::OptionNotApplied { key }),
            None => Ok(()),
        }
    }
}

impl Drop for LockOptions {
    fn drop(&mut self) {
        // SAFETY: frees a dictionary this value owns; NULL is allowed.
        unsafe { ff::av_dict_free(&mut self.dict) };
    }
}

/// Checks that an opened format context carries every whitelist.
///
/// # Safety
/// `fmt` must be a format context opened by `avformat_open_input`.
pub(crate) unsafe fn verify_locks(fmt: *const ff::AVFormatContext) -> Result<(), Error> {
    // SAFETY: guaranteed by the caller; the fields are NULL or NUL-terminated.
    let fields: [(&str, *const c_char, &CStr); 3] = unsafe {
        [
            (
                "protocol_whitelist",
                (*fmt).protocol_whitelist,
                PROTOCOL_WHITELIST,
            ),
            (
                "format_whitelist",
                (*fmt).format_whitelist,
                FORMAT_WHITELIST,
            ),
            (
                "codec_whitelist",
                (*fmt).codec_whitelist,
                audio_decoder_whitelist(),
            ),
        ]
    };
    for (key, actual, expected) in fields {
        // SAFETY: NULL or a NUL-terminated string owned by the context.
        let matches = !actual.is_null() && unsafe { CStr::from_ptr(actual) } == expected;
        if !matches {
            return Err(Error::OptionNotApplied {
                key: key.to_owned(),
            });
        }
    }
    Ok(())
}

/// The format context's `io_open`: refuses every nested open.
pub(crate) unsafe extern "C" fn deny_io_open(
    _s: *mut ff::AVFormatContext,
    pb: *mut *mut ff::AVIOContext,
    _url: *const c_char,
    _flags: c_int,
    _options: *mut *mut ff::AVDictionary,
) -> c_int {
    let denied = ff::AVERROR(libc::EPERM);
    catch_unwind(AssertUnwindSafe(|| {
        if !pb.is_null() {
            // SAFETY: FFmpeg passes a valid out-pointer; leave no context behind.
            unsafe { *pb = ptr::null_mut() };
        }
        denied
    }))
    .unwrap_or(denied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_whitelist_has_audio_decoders_only() {
        let list = audio_decoder_whitelist().to_str().expect("ASCII names");
        let names: Vec<&str> = list.split(',').collect();
        for audio in ["flac", "pcm_s16le", "mp3float", "aac", "opus", "vorbis"] {
            assert!(names.contains(&audio), "{audio} missing");
        }
        for other in ["h264", "png", "mjpeg", "vp8", "mpeg4", "subrip"] {
            assert!(!names.contains(&other), "{other} must not be listed");
        }
    }

    #[test]
    fn format_whitelist_excludes_demuxers_that_open_other_resources() {
        let list = FORMAT_WHITELIST.to_str().expect("ASCII");
        let names: Vec<&str> = list.split(',').collect();
        for bad in [
            "hls", "dash", "concat", "image2", "sdp", "rtsp", "imf", "mlv",
        ] {
            assert!(!names.contains(&bad), "{bad} must not be listed");
        }
    }

    #[test]
    fn io_open_refuses() {
        let mut pb: *mut ff::AVIOContext = ptr::NonNull::dangling().as_ptr();
        // SAFETY: the callback only writes through pb.
        let ret = unsafe {
            deny_io_open(
                ptr::null_mut(),
                &mut pb,
                c"http://example.invalid/".as_ptr(),
                0,
                ptr::null_mut(),
            )
        };
        assert_eq!(ret, ff::AVERROR(libc::EPERM));
        assert!(pb.is_null());
    }
}
