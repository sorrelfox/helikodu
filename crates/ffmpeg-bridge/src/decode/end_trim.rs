// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The end of an MP4 stream, which the bridge trims itself.
//!
//! FFmpeg removes the encoder delay at the start of MP4 audio, but not always the
//! padding at the end: 7.1 leaves the end padding of AAC, and 9.0 that of Opus. The
//! container knows the real length: the edit list's duration, which FFmpeg reports
//! as the stream duration, or else the sample count in an `iTunSMPB` tag. Decoding
//! stops there. Where FFmpeg already trims, the two agree.

use crate::demux::MediaFile;
use crate::ffi::{self, ff};

/// The number of frames an MP4 stream really has, counted from the first frame
/// after the encoder delay, at `rate`. `None` when the container does not say.
pub(crate) fn mp4_length(media: &MediaFile, stream: usize, rate: u32) -> Option<u64> {
    let st = media.raw_stream(stream)?;
    // SAFETY: a valid stream of the open file.
    let (duration, time_base) = unsafe { ((*st).duration, (*st).time_base) };
    let from_edit_list = if duration > 0 && duration != ff::AV_NOPTS_VALUE {
        let rate = i32::try_from(rate).ok()?;
        u64::try_from(ffi::rescale(
            duration,
            time_base,
            ff::AVRational { num: 1, den: rate },
        ))
        .ok()
    } else {
        None
    };
    let from_tag = media
        .tags_with_stream(stream)
        .get("iTunSMPB")
        .and_then(itunsmpb_length);
    // Without an edit list the stream duration still counts the padding, so it is
    // never shorter than the tag's length; the smaller value is the right one.
    match (from_edit_list, from_tag) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The original sample count in an `iTunSMPB` value: hexadecimal fields for a
/// reserved word, the encoder delay, the padding and the sample count.
fn itunsmpb_length(value: &str) -> Option<u64> {
    let field = value.split_whitespace().nth(3)?;
    u64::from_str_radix(field, 16).ok().filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_itunsmpb() {
        let v = " 00000000 00000840 000001CA 00000000000D8BA6 00000000 00000000";
        assert_eq!(itunsmpb_length(v), Some(0xD8BA6));
        assert_eq!(itunsmpb_length("00000000 00000840"), None);
        assert_eq!(itunsmpb_length(" 0 0 0 0"), None);
        assert_eq!(itunsmpb_length(" 0 0 0 zz"), None);
    }
}
