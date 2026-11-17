// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The low-level pieces every module shares: FFmpeg's C enums as integers, the
//! few functions declared again with integer parameters, strings, dictionaries and
//! time stamps.
//!
//! bindgen models C enums such as `AVCodecID` and `AVMediaType` as Rust enums. A
//! value the build headers do not know (a codec added in a newer runtime library,
//! for example after a Flatpak runtime update) would be undefined behaviour the
//! moment it exists as such an enum. So every enum field FFmpeg hands back is read
//! through a pointer cast to `c_int`, and the functions that take one are declared
//! here with an integer parameter. Converting a *known* variant to an integer, as
//! the constants below do, is always fine.

use std::ffi::{CStr, c_char, c_int};
use std::num::NonZeroU32;
use std::ptr;

pub(crate) use ffmpeg_sys_next as ff;
use helikodu_model::audio::MediaTime;

/// `AVMediaType` values.
pub(crate) mod media_type {
    use super::{c_int, ff};

    pub(crate) const VIDEO: c_int = ff::AVMediaType::AVMEDIA_TYPE_VIDEO as c_int;
    pub(crate) const AUDIO: c_int = ff::AVMediaType::AVMEDIA_TYPE_AUDIO as c_int;
    pub(crate) const DATA: c_int = ff::AVMediaType::AVMEDIA_TYPE_DATA as c_int;
    pub(crate) const SUBTITLE: c_int = ff::AVMediaType::AVMEDIA_TYPE_SUBTITLE as c_int;
    pub(crate) const ATTACHMENT: c_int = ff::AVMediaType::AVMEDIA_TYPE_ATTACHMENT as c_int;
}

/// `AVSampleFormat` values.
pub(crate) mod sample_format {
    use super::{c_int, ff};
    use ff::AVSampleFormat as F;

    pub(crate) const U8: c_int = F::AV_SAMPLE_FMT_U8 as c_int;
    pub(crate) const S16: c_int = F::AV_SAMPLE_FMT_S16 as c_int;
    pub(crate) const S32: c_int = F::AV_SAMPLE_FMT_S32 as c_int;
    pub(crate) const FLT: c_int = F::AV_SAMPLE_FMT_FLT as c_int;
    pub(crate) const DBL: c_int = F::AV_SAMPLE_FMT_DBL as c_int;
    pub(crate) const S64: c_int = F::AV_SAMPLE_FMT_S64 as c_int;
    pub(crate) const U8P: c_int = F::AV_SAMPLE_FMT_U8P as c_int;
    pub(crate) const S16P: c_int = F::AV_SAMPLE_FMT_S16P as c_int;
    pub(crate) const S32P: c_int = F::AV_SAMPLE_FMT_S32P as c_int;
    pub(crate) const FLTP: c_int = F::AV_SAMPLE_FMT_FLTP as c_int;
    pub(crate) const DBLP: c_int = F::AV_SAMPLE_FMT_DBLP as c_int;
    pub(crate) const S64P: c_int = F::AV_SAMPLE_FMT_S64P as c_int;
}

/// `AVChannelOrder` values.
pub(crate) mod channel_order {
    use super::{c_int, ff};

    pub(crate) const NATIVE: c_int = ff::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE as c_int;
}

/// `AVCodecID` values of the picture codecs cover art comes in.
pub(crate) mod codec_id {
    use super::{c_int, ff};
    use ff::AVCodecID as C;

    pub(crate) const MJPEG: c_int = C::AV_CODEC_ID_MJPEG as c_int;
    pub(crate) const PNG: c_int = C::AV_CODEC_ID_PNG as c_int;
    pub(crate) const BMP: c_int = C::AV_CODEC_ID_BMP as c_int;
    pub(crate) const GIF: c_int = C::AV_CODEC_ID_GIF as c_int;
    pub(crate) const TIFF: c_int = C::AV_CODEC_ID_TIFF as c_int;
    pub(crate) const WEBP: c_int = C::AV_CODEC_ID_WEBP as c_int;
}

/// FFmpeg functions whose bindgen declarations take a C enum, declared again with
/// a plain integer. The C ABI passes an enum as an `int`, so the two declarations
/// call the same function the same way.
pub(crate) mod raw {
    use std::ffi::{c_char, c_int};

    unsafe extern "C" {
        /// `const char *avcodec_get_name(enum AVCodecID id)`; never returns NULL.
        pub(crate) fn avcodec_get_name(id: c_int) -> *const c_char;
        /// `const AVCodec *avcodec_find_decoder(enum AVCodecID id)`.
        pub(crate) fn avcodec_find_decoder(id: c_int) -> *const ffmpeg_sys_next::AVCodec;
    }
}

/// Reads a C enum field as the integer it is.
///
/// # Safety
/// `field` must point to a readable, initialised field of a C enum type, which C
/// stores as an `int`.
pub(crate) unsafe fn enum_field<T>(field: *const T) -> c_int {
    const { assert!(size_of::<T>() == size_of::<c_int>()) };
    // SAFETY: guaranteed by the caller; the size is checked above, and reading the
    // bytes as c_int accepts every value.
    unsafe { field.cast::<c_int>().read() }
}

/// The media type of a stream's parameters.
///
/// # Safety
/// `par` must point to valid `AVCodecParameters`.
pub(crate) unsafe fn codec_type(par: *const ff::AVCodecParameters) -> c_int {
    // SAFETY: guaranteed by the caller.
    unsafe { enum_field(ptr::addr_of!((*par).codec_type)) }
}

/// The codec id of a stream's parameters.
///
/// # Safety
/// `par` must point to valid `AVCodecParameters`.
pub(crate) unsafe fn codec_id(par: *const ff::AVCodecParameters) -> c_int {
    // SAFETY: guaranteed by the caller.
    unsafe { enum_field(ptr::addr_of!((*par).codec_id)) }
}

/// FFmpeg's short name of a codec id, such as `flac` or `aac`.
pub(crate) fn codec_name(id: c_int) -> String {
    // SAFETY: avcodec_get_name accepts any id and returns a static string ("none"
    // or "unknown_codec" for ids it does not know), never NULL; checked anyway.
    let name = unsafe { raw::avcodec_get_name(id) };
    // SAFETY: a static NUL-terminated string or NULL.
    unsafe { string(name) }.unwrap_or_else(|| "unknown".to_owned())
}

/// Copies a C string; `None` for NULL. Invalid UTF-8 is replaced.
///
/// # Safety
/// `p` must be NULL or point to a NUL-terminated string.
pub(crate) unsafe fn string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        // SAFETY: guaranteed by the caller.
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// Every entry of a dictionary, in order.
///
/// # Safety
/// `dict` must be NULL or a valid dictionary that is not changed during the call.
pub(crate) unsafe fn dict_entries(dict: *const ff::AVDictionary) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut entry: *const ff::AVDictionaryEntry = ptr::null();
    loop {
        // SAFETY: the documented way to walk every entry: an empty key with
        // AV_DICT_IGNORE_SUFFIX, continuing from the previous entry. A NULL
        // dictionary has no entries.
        entry = unsafe { ff::av_dict_get(dict, c"".as_ptr(), entry, ff::AV_DICT_IGNORE_SUFFIX) };
        if entry.is_null() {
            return out;
        }
        // SAFETY: entries of a valid dictionary have NUL-terminated key and value.
        let (key, value) = unsafe { (string((*entry).key), string((*entry).value)) };
        if let Some(key) = key {
            out.push((key, value.unwrap_or_default()));
        }
    }
}

/// A time stamp in `time_base` as a [`MediaTime`]; `None` for `AV_NOPTS_VALUE`, a
/// negative time or an invalid time base.
///
/// The tick rate stays the time base's when that is `1/n`, so the value is exact;
/// other time bases are converted to nanoseconds.
pub(crate) fn media_time(ts: i64, time_base: ff::AVRational) -> Option<MediaTime> {
    if ts == ff::AV_NOPTS_VALUE || ts < 0 || time_base.num <= 0 || time_base.den <= 0 {
        return None;
    }
    let ticks = u64::try_from(ts).ok()?;
    if time_base.num == 1 {
        let rate = NonZeroU32::new(u32::try_from(time_base.den).ok()?)?;
        return Some(MediaTime::new(ticks, rate));
    }
    let nanos =
        i128::from(ts) * i128::from(time_base.num) * 1_000_000_000 / i128::from(time_base.den);
    let nanos = u64::try_from(nanos).ok()?;
    Some(MediaTime::new(nanos, NonZeroU32::new(1_000_000_000)?))
}

/// Rescales `value` from one time base to another, rounding to the nearest.
pub(crate) fn rescale(value: i64, from: ff::AVRational, to: ff::AVRational) -> i64 {
    // SAFETY: pure arithmetic on plain values; FFmpeg handles every input,
    // including zero denominators, without undefined behaviour.
    unsafe { ff::av_rescale_q(value, from, to) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_time_keeps_simple_time_bases_exact() {
        let tb = ff::AVRational {
            num: 1,
            den: 44_100,
        };
        let t = media_time(44_100, tb).expect("valid");
        assert_eq!(t.ticks(), 44_100);
        assert_eq!(t.ticks_per_second().get(), 44_100);
        assert!(media_time(ff::AV_NOPTS_VALUE, tb).is_none());
        assert!(media_time(-1, tb).is_none());
    }

    #[test]
    fn media_time_converts_other_time_bases() {
        let tb = ff::AVRational {
            num: 1001,
            den: 30_000,
        };
        let t = media_time(30_000, tb).expect("valid");
        assert_eq!(t.ticks(), 1_001_000_000_000);
    }

    #[test]
    fn codec_names_accept_unknown_ids() {
        assert_eq!(codec_name(ff::AVCodecID::AV_CODEC_ID_FLAC as c_int), "flac");
        // An id far outside the known range is not undefined behaviour here.
        assert!(!codec_name(0x7fff_0000).is_empty());
    }
}
