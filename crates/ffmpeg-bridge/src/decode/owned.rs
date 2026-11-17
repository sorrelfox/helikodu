// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Owners of the FFmpeg objects a decoder uses; each frees its object on drop.

use crate::error::Error;
use crate::ffi::ff;

/// An `AVCodecContext`.
pub(crate) struct CodecContext(pub(crate) *mut ff::AVCodecContext);

impl CodecContext {
    pub(crate) fn new(codec: *const ff::AVCodec) -> Result<Self, Error> {
        // SAFETY: codec is a valid decoder; the result is checked.
        let ctx = unsafe { ff::avcodec_alloc_context3(codec) };
        if ctx.is_null() {
            Err(Error::out_of_memory("avcodec_alloc_context3"))
        } else {
            Ok(Self(ctx))
        }
    }
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        // SAFETY: allocated by avcodec_alloc_context3; this also closes it.
        unsafe { ff::avcodec_free_context(&mut self.0) };
    }
}

/// An `AVPacket`.
pub(crate) struct Packet(pub(crate) *mut ff::AVPacket);

impl Packet {
    pub(crate) fn new() -> Result<Self, Error> {
        // SAFETY: plain allocation; checked.
        let p = unsafe { ff::av_packet_alloc() };
        if p.is_null() {
            Err(Error::out_of_memory("av_packet_alloc"))
        } else {
            Ok(Self(p))
        }
    }

    pub(crate) fn unref(&mut self) {
        // SAFETY: an allocated packet; unreferencing an empty one is allowed.
        unsafe { ff::av_packet_unref(self.0) };
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: allocated by av_packet_alloc; this also unreferences it.
        unsafe { ff::av_packet_free(&mut self.0) };
    }
}

/// An `AVFrame`.
pub(crate) struct Frame(pub(crate) *mut ff::AVFrame);

impl Frame {
    pub(crate) fn new() -> Result<Self, Error> {
        // SAFETY: plain allocation; checked.
        let f = unsafe { ff::av_frame_alloc() };
        if f.is_null() {
            Err(Error::out_of_memory("av_frame_alloc"))
        } else {
            Ok(Self(f))
        }
    }

    pub(crate) fn unref(&mut self) {
        // SAFETY: an allocated frame; unreferencing an empty one is allowed.
        unsafe { ff::av_frame_unref(self.0) };
    }

    /// The frame's contents.
    pub(crate) fn get(&self) -> &ff::AVFrame {
        // SAFETY: allocated and non-NULL for the life of self.
        unsafe { &*self.0 }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: allocated by av_frame_alloc; this also unreferences it.
        unsafe { ff::av_frame_free(&mut self.0) };
    }
}
