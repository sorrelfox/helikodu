// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The decoder of one audio stream: decoding, gapless trimming and seeking.

use std::ffi::{c_int, c_void};
use std::ptr;

use helikodu_model::audio::{
    AudioSpec, ChannelLayout, Endianness, MediaTime, SampleFormat, SampleRate,
};

use super::anchor::{self, Anchor, Grid};
use super::end_trim;
use super::owned::{CodecContext, Frame, Packet};
use super::pcm::{PcmBuffer, convert_frame, spec_of};
use crate::demux::{MediaFile, StreamChoice, StreamInfo};
use crate::error::{AvError, AvErrorKind, Error, check};
use crate::ffi::{self, ff, media_type, raw};

/// How many read or decode errors in a row are tolerated before a stream counts as
/// damaged. Single bad packets are skipped, as players usually do.
const MAX_ERRORS_IN_A_ROW: u32 = 64;

/// The least audio decoded before a seek target, in frames: decoders such as MP3
/// and Opus need some audio before their output is right again.
const MIN_SEEK_PREROLL: u64 = 4096;

/// How often a seek moves further back when it landed after its target.
const SEEK_ATTEMPTS: u32 = 5;

/// Stands in for the output format until the first frame is decoded.
const PLACEHOLDER_SPEC: AudioSpec = AudioSpec::new(
    SampleFormat::F32(Endianness::NATIVE),
    SampleRate::HZ_48000,
    ChannelLayout::STEREO,
);

/// Decodes one audio stream of a [`MediaFile`] to interleaved PCM.
///
/// Every other stream is discarded in the demuxer, and packets that still arrive
/// from them are dropped. Encoder delay and padding are trimmed: FFmpeg does it for
/// formats that carry the information (it gets the packet side data and the packet
/// time base it needs), and the bridge trims the end of MP4 audio itself.
pub struct AudioDecoder {
    // The codec context is independent of the format context; both are freed on
    // drop in field order.
    codec: CodecContext,
    packet: Packet,
    frame: Frame,
    media: MediaFile,
    stream_index: usize,
    time_base: ff::AVRational,
    /// The sample rate positions are counted in.
    rate: u32,
    spec: AudioSpec,
    /// The time stamp, in samples at `rate`, of the first frame after the encoder
    /// delay: position 0.
    origin: i64,
    /// The same, in the stream's time base.
    origin_ts: i64,
    /// The position of the next frame when nothing says otherwise.
    next_position: i64,
    /// The stream's length, when the bridge trims the end itself.
    end: Option<u64>,
    duration: Option<u64>,
    preroll: u64,
    /// `frame` holds a decoded frame that has not been returned yet.
    frame_ready: bool,
    /// The position of that frame when it is known from its time stamp.
    frame_position: Option<i64>,
    /// Where exact positions after a seek come from (see [`anchor`]).
    anchor: Anchor,
    /// The anchor of the first frame: position 0.
    anchor_origin: i64,
    /// The byte position, time stamp and size of the stream's first packet.
    first_packet: Option<FirstPacket>,
    /// Snaps positions from coarse time stamps to whole codec frames.
    grid: Option<Grid>,
    /// One tick of the stream's time base, in frames, rounded up; at least 1.
    tick: i64,
    /// How far a seek may land from its target, in frames.
    seek_precision: u64,
    /// After a seek: drop output before this position.
    discard_until: Option<u64>,
    /// `packet` holds a packet the decoder has not taken yet.
    packet_pending: bool,
    draining: bool,
    finished: bool,
    errors_in_a_row: u32,
    buffer: PcmBuffer,
}

// SAFETY: the codec context, packet and frame are owned by this value alone and
// only used through `&mut self`; MediaFile is Send. FFmpeg decoders keep no
// thread-affine state, so the whole decoder may move to another thread.
unsafe impl Send for AudioDecoder {}

impl std::fmt::Debug for AudioDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioDecoder")
            .field("stream_index", &self.stream_index)
            .field("spec", &self.spec)
            .field("end", &self.end)
            .finish_non_exhaustive()
    }
}

impl AudioDecoder {
    /// Chooses the audio stream, discards the others, opens the decoder and decodes
    /// the first block, which fixes the output format.
    pub fn new(mut media: MediaFile, choice: StreamChoice) -> Result<Self, Error> {
        let stream_index = media.choose_stream(choice)?;
        media.keep_only(stream_index);
        let st = media
            .raw_stream(stream_index)
            .ok_or(Error::NotAnAudioStream {
                index: stream_index,
            })?;
        // SAFETY: a valid stream of the open file; the codec id stays an integer.
        let (par, id, time_base) = unsafe {
            (
                (*st).codecpar,
                ffi::codec_id((*st).codecpar),
                (*st).time_base,
            )
        };
        // SAFETY: looks the id up in FFmpeg's static table; any value is allowed.
        let codec = unsafe { raw::avcodec_find_decoder(id) };
        let is_audio_decoder = !codec.is_null()
            // SAFETY: a static AVCodec, checked for NULL; its type is read as an
            // integer.
            && unsafe { ffi::enum_field(ptr::addr_of!((*codec).type_)) } == media_type::AUDIO;
        if !is_audio_decoder {
            return Err(Error::DecoderNotFound {
                codec: ffi::codec_name(id),
            });
        }
        let codec_name = ffi::codec_name(id);
        // SAFETY: par is the stream's valid parameters.
        let anchor = unsafe { choose_anchor(&media, &codec_name, par) };
        // SAFETY: codec is an audio decoder and par the stream's parameters.
        let ctx = unsafe { open_codec(codec, par, time_base, anchor != Anchor::None) }?;
        // SAFETY: an opened context; enum fields are read as integers.
        let fallback_spec = unsafe {
            spec_of(
                ffi::enum_field(ptr::addr_of!((*ctx.0).sample_fmt)),
                (*ctx.0).sample_rate,
                &(*ctx.0).ch_layout,
            )
        };
        // SAFETY: as above.
        let seek_preroll = unsafe { (*par).seek_preroll };

        let mut decoder = Self {
            codec: ctx,
            packet: Packet::new()?,
            frame: Frame::new()?,
            media,
            stream_index,
            time_base,
            rate: 0,
            spec: PLACEHOLDER_SPEC,
            origin: 0,
            origin_ts: 0,
            next_position: 0,
            end: None,
            duration: None,
            preroll: u64::try_from(seek_preroll)
                .unwrap_or(0)
                .max(MIN_SEEK_PREROLL),
            frame_ready: false,
            frame_position: None,
            anchor: Anchor::None,
            anchor_origin: 0,
            first_packet: None,
            grid: None,
            tick: 1,
            seek_precision: 0,
            discard_until: None,
            packet_pending: false,
            draining: false,
            finished: false,
            errors_in_a_row: 0,
            buffer: PcmBuffer::new(PLACEHOLDER_SPEC),
        };
        decoder.anchor = anchor;

        // Decode the first frame: it gives the output format and the time stamp of
        // position 0.
        if decoder.receive_frame()? {
            let frame = decoder.frame.get();
            // SAFETY: a decoded audio frame with a valid layout.
            decoder.spec = unsafe { spec_of(frame.format, frame.sample_rate, &frame.ch_layout) }?;
            decoder.rate = decoder.spec.rate.hz();
            decoder.origin_ts = frame_timestamp(frame).unwrap_or(0);
            decoder.origin = decoder.ts_to_samples(decoder.origin_ts);
            decoder.frame_ready = true;
            decoder.frame_position = Some(0);
            decoder.anchor_origin = anchor::from_opaque(frame.opaque).unwrap_or(0);
            let first_frames = i64::from(frame.nb_samples);
            decoder.settle_seek_precision(&codec_name, first_frames, par);
        } else {
            decoder.spec = fallback_spec?;
            decoder.rate = decoder.spec.rate.hz();
            decoder.finished = true;
        }
        decoder.buffer = PcmBuffer::new(decoder.spec);
        if decoder.media.is_mp4_family() {
            decoder.end = end_trim::mp4_length(&decoder.media, stream_index, decoder.rate);
        }
        decoder.duration = decoder.end.or_else(|| decoder.stream_length());
        Ok(decoder)
    }

    /// The index of the stream being decoded.
    pub fn stream_index(&self) -> usize {
        self.stream_index
    }

    /// The description of the stream being decoded.
    pub fn stream(&self) -> &StreamInfo {
        &self.media.streams()[self.stream_index]
    }

    /// The file, for its tags, chapters and pictures.
    pub fn media(&self) -> &MediaFile {
        &self.media
    }

    /// The format of the decoded audio, as of the first block. A stream that changes
    /// its format midway (rare: chained Ogg, some MP3 files) reports the change in
    /// each [`PcmBuffer::spec`].
    pub fn spec(&self) -> AudioSpec {
        self.spec
    }

    /// The length of the stream after gapless trimming, when known: exact for MP4
    /// (from the edit list or `iTunSMPB`), otherwise the container's statement.
    pub fn duration(&self) -> Option<MediaTime> {
        let rate = SampleRate::new(self.rate)?;
        self.duration.map(|d| MediaTime::from_frames(d, rate))
    }

    /// The stream duration from the container, in frames at the output rate.
    fn stream_length(&self) -> Option<u64> {
        let st = self.media.raw_stream(self.stream_index)?;
        // SAFETY: a valid stream of the open file.
        let (duration, initial_padding) =
            unsafe { ((*st).duration, (*(*st).codecpar).initial_padding) };
        if duration <= 0 || duration == ff::AV_NOPTS_VALUE {
            return None;
        }
        let mut frames = self.ts_to_samples(duration);
        // Ogg counts the pre-skip in its granule positions.
        if self.media.format_name() == "ogg" {
            frames -= i64::from(initial_padding);
        }
        u64::try_from(frames).ok()
    }

    /// The position of the next block, in frames from the start.
    pub fn position(&self) -> u64 {
        match (self.discard_until, self.frame_position) {
            (Some(target), _) => target,
            (None, Some(p)) if self.frame_ready => u64::try_from(p).unwrap_or(0),
            _ => u64::try_from(self.next_position).unwrap_or(0),
        }
    }

    /// Decodes the next block of audio. `None` at the end of the stream.
    pub fn read(&mut self) -> Result<Option<&PcmBuffer>, Error> {
        loop {
            if self.finished {
                return Ok(None);
            }
            if !self.frame_ready {
                if !self.receive_frame()? {
                    self.finished = true;
                    return Ok(None);
                }
                self.frame_ready = true;
            }
            self.frame_ready = false;
            let n = i64::from(self.frame.get().nb_samples);
            let start = self.frame_position.take().unwrap_or(self.next_position);
            let frame_end = start + n;
            self.next_position = frame_end;

            // Samples before position 0, or before a seek target, are dropped.
            let mut first = 0.max(start);
            if let Some(target) = self.discard_until {
                let target = i64::try_from(target).unwrap_or(i64::MAX);
                if frame_end <= target {
                    self.frame.unref();
                    continue;
                }
                first = first.max(target);
                self.discard_until = None;
            }
            let mut last = frame_end;
            if let Some(end) = self.end {
                let end = i64::try_from(end).unwrap_or(i64::MAX);
                if first >= end {
                    self.frame.unref();
                    self.finished = true;
                    return Ok(None);
                }
                last = last.min(end);
            }
            if first >= last {
                self.frame.unref();
                continue;
            }
            // All three are within 0..=n, which came from a c_int.
            let skip = usize::try_from(first - start).unwrap_or(0);
            let take = usize::try_from(last - first).unwrap_or(0);
            let position = u64::try_from(first).unwrap_or(0);
            // SAFETY: frame holds a decoded audio frame of n samples, and
            // skip + take <= n.
            let converted =
                unsafe { convert_frame(self.frame.get(), skip, take, position, &mut self.buffer) };
            self.frame.unref();
            converted?;
            return Ok(Some(&self.buffer));
        }
    }

    /// Moves to `to`, exactly: the next block starts at that frame (or the stream
    /// ends, when `to` is past its end).
    ///
    /// The demuxer seeks to a point before the target, the decoder is flushed, and
    /// audio before the target is decoded and dropped. The position of the first
    /// decoded frame comes from an exact anchor where the codec or container has
    /// one (see [`seek_precision`](Self::seek_precision)), otherwise from its time
    /// stamp. When the demuxer lands after the target, the seek is repeated further
    /// back, and as a last resort from the start.
    pub fn seek(&mut self, to: MediaTime) -> Result<(), Error> {
        let Some(rate) = SampleRate::new(self.rate) else {
            return Ok(());
        };
        let mut target = to.frames_at(rate);
        if let Some(end) = self.end {
            target = target.min(end);
        }
        let target_i = i64::try_from(target).unwrap_or(i64::MAX);
        let mut back = self.preroll;
        for _ in 0..SEEK_ATTEMPTS {
            let start = target.saturating_sub(back);
            if start == 0 {
                break;
            }
            if self.demuxer_seek(start).is_err() {
                // The demuxer cannot seek there (some formats cannot seek at all):
                // decode from the start instead.
                break;
            }
            if self.receive_frame()? {
                match self.seeked_frame_position() {
                    Some(position) if position <= target_i => {
                        self.frame_ready = true;
                        self.frame_position = Some(position);
                        self.discard_until = Some(target);
                        return Ok(());
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            // Landed after the target, or at the end: further back.
            back = back.saturating_mul(4).saturating_add(u64::from(self.rate));
        }
        self.rewind()?;
        self.discard_until = Some(target);
        Ok(())
    }

    /// How far a seek may land from its target, in frames: 0 when seeks are exact.
    ///
    /// Seeks are exact when the container's time stamps count single frames, when
    /// the codec carries its own position (FLAC frame headers), when packets are
    /// fixed-size blocks stored back to back (ADPCM in WAV, RealAudio 1.0), or when
    /// the codec's frames have a fixed size that a coarse time stamp can be snapped
    /// to (WMA in ASF, Opus in WebM, Nellymoser and ADPCM in FLV).
    /// Otherwise the position comes from a time stamp coarser than a frame (Vorbis
    /// in Matroska or WebM: milliseconds), and a seek lands within one time-stamp
    /// tick plus one frame of the target; this value says how many frames that is.
    pub fn seek_precision(&self) -> u64 {
        self.seek_precision
    }

    /// Works out the grid and the seek precision once the first frame is known.
    fn settle_seek_precision(
        &mut self,
        codec: &str,
        first_frames: i64,
        par: *const ff::AVCodecParameters,
    ) {
        let (num, den) = (i64::from(self.time_base.num), i64::from(self.time_base.den));
        let rate = i64::from(self.rate);
        let tick_exact = num > 0 && den > 0 && (rate * num) % den == 0;
        self.tick = if num > 0 && den > 0 {
            ((rate * num + den - 1) / den).max(1)
        } else {
            1
        };
        if let Anchor::Blocks {
            bytes: 0,
            byte_seek: true,
            ..
        } = self.anchor
        {
            // RealAudio 14.4: when the second packet's time stamp does not move on
            // from the first, seek by byte, with the distance between the two
            // packets as the block size.
            let second = self.peek_packet();
            self.anchor = match (self.first_packet, second) {
                (Some(first), Some(second))
                    if second.ts == first.ts && second.pos > first.pos && first_frames > 0 =>
                {
                    Anchor::Blocks {
                        bytes: second.pos - first.pos,
                        frames: first_frames,
                        byte_seek: true,
                    }
                }
                _ => Anchor::None,
            };
        }
        // SAFETY: par is the stream's valid parameters.
        let step = unsafe { grid_step(codec, self.rate, self.codec.0, par) };
        let grid = step.map(|step| Grid {
            step,
            offset: first_frames.rem_euclid(step),
        });
        // Time stamps in whole frame ticks, with every codec frame on a tick, are
        // exact.
        let ticks_are_exact = tick_exact
            && grid.is_some_and(|g| g.step % self.tick == 0 && g.offset % self.tick == 0);
        let exact = self.anchor != Anchor::None || self.tick <= 1 || ticks_are_exact;
        if !exact
            && let Some(grid) = grid
            && grid.step > 2 * (self.tick + 1)
        {
            self.grid = Some(grid);
        }
        self.seek_precision = if exact || self.grid.is_some() {
            0
        } else {
            u64::try_from(self.tick + 1).unwrap_or(u64::MAX)
        };
    }

    /// The position of the frame just decoded after a demuxer seek: from its anchor,
    /// else from its time stamp (snapped to the grid, if there is one). `None` when
    /// it cannot be known.
    fn seeked_frame_position(&self) -> Option<i64> {
        let frame = self.frame.get();
        if let Some(anchor) = anchor::from_opaque(frame.opaque) {
            return Some(anchor - self.anchor_origin);
        }
        if self.anchor.seeks_by_byte() {
            // These streams have no usable time stamps.
            return None;
        }
        let position = self.ts_to_samples(frame_timestamp(frame)?) - self.origin;
        Some(match self.grid {
            Some(grid) => {
                let snapped = grid.snap(position);
                // A grid point far from the time stamp means the grid does not fit
                // this stream; trust the time stamp then.
                if (snapped - position).abs() <= self.tick + 1 {
                    snapped
                } else {
                    position
                }
            }
            None => position,
        })
    }

    /// Goes back to the first packet and counts frames from 0, as when the file
    /// was opened.
    fn rewind(&mut self) -> Result<(), Error> {
        self.demuxer_seek(0)?;
        // A flush does not reset every decoder (HCOM and RealAudio 14.4 keep
        // their predictor), so a fresh one decodes the start exactly as at opening.
        self.reopen_codec()?;
        self.frame_position = Some(0);
        if self.receive_frame()? {
            self.frame_ready = true;
            // The first frame should be the one decoded at opening. If the demuxer
            // could not go back that far, or the decoder trimmed differently, its
            // anchor or time stamp says where it really is.
            let position = match self.seeked_frame_position() {
                Some(p) if p.abs() > self.tick + 1 || self.anchor != Anchor::None => p,
                _ => 0,
            };
            self.frame_position = Some(position);
        }
        Ok(())
    }

    /// Replaces the decoder with a freshly opened one.
    fn reopen_codec(&mut self) -> Result<(), Error> {
        let st = self
            .media
            .raw_stream(self.stream_index)
            .ok_or(Error::NotAnAudioStream {
                index: self.stream_index,
            })?;
        // SAFETY: the codec of the open context; a valid stream's parameters.
        let ctx = unsafe {
            open_codec(
                (*self.codec.0).codec,
                (*st).codecpar,
                self.time_base,
                self.anchor != Anchor::None,
            )
        }?;
        self.codec = ctx;
        Ok(())
    }

    /// Seeks the demuxer to a key point at or before `position`, flushes the
    /// decoder and resets the reading state. Position 0 seeks to the stream's first
    /// packet.
    fn demuxer_seek(&mut self, position: u64) -> Result<(), Error> {
        let index = c_int::try_from(self.stream_index).unwrap_or(c_int::MAX);
        let fmt = self.media.input.fmt;
        let ret = match (self.anchor, self.first_packet) {
            (
                Anchor::Blocks {
                    bytes,
                    frames,
                    byte_seek: true,
                },
                Some(first),
            ) => {
                // A packet's position is that of its data; a packet header (12
                // bytes in RealMedia, none in old RealAudio files) comes before.
                let header = (bytes - first.size).max(0);
                let block = i64::try_from(position).unwrap_or(i64::MAX) / frames.max(1);
                let byte = (first.pos - header).saturating_add(block.saturating_mul(bytes));
                // FFmpeg's byte seek clamps to where it thinks the data starts,
                // which for these files can lie past the first packets. Position
                // the I/O directly instead, after discarding what the demuxer
                // buffered, as avformat_flush documents for discontinuities.
                // SAFETY: fmt is open and its pb is the custom I/O context.
                unsafe {
                    let ret = ff::avformat_flush(fmt);
                    if ret < 0 {
                        ret
                    } else {
                        let pb = (*fmt).pb;
                        let landed = ff::avio_seek(pb, byte, libc::SEEK_SET);
                        c_int::try_from(landed.min(0)).unwrap_or(ff::AVERROR_EXTERNAL)
                    }
                }
            }
            _ => {
                let ts = if position == 0 {
                    self.first_packet
                        .and_then(|first| first.ts)
                        .map_or(self.origin_ts, |ts| ts.min(self.origin_ts))
                } else {
                    let samples = i64::try_from(position)
                        .unwrap_or(i64::MAX)
                        .saturating_add(self.origin);
                    ffi::rescale(samples, self.sample_time_base(), self.time_base)
                };
                // SAFETY: fmt is open and index is one of its streams.
                let mut ret = unsafe { ff::avformat_seek_file(fmt, index, i64::MIN, ts, ts, 0) };
                if ret < 0 {
                    // SAFETY: as above.
                    ret = unsafe { ff::av_seek_frame(fmt, index, ts, ff::AVSEEK_FLAG_BACKWARD) };
                }
                if ret < 0
                    && position == 0
                    && let Some(start) = self.media.data_start()
                {
                    // A demuxer without any seek support (HCOM, Westwood AUD) can
                    // still go back to where its data starts, by byte position.
                    // SAFETY: as above.
                    ret = unsafe { ff::av_seek_frame(fmt, index, start, ff::AVSEEK_FLAG_BYTE) };
                }
                ret
            }
        };
        check("av_seek_frame", ret)?;
        // SAFETY: an opened decoder context.
        unsafe { ff::avcodec_flush_buffers(self.codec.0) };
        self.packet.unref();
        self.frame.unref();
        self.packet_pending = false;
        self.frame_ready = false;
        self.frame_position = None;
        self.discard_until = None;
        self.draining = false;
        self.finished = false;
        self.errors_in_a_row = 0;
        self.next_position = i64::try_from(position).unwrap_or(i64::MAX);
        Ok(())
    }

    fn sample_time_base(&self) -> ff::AVRational {
        ff::AVRational {
            num: 1,
            den: i32::try_from(self.rate.max(1)).unwrap_or(i32::MAX),
        }
    }

    /// A time stamp in the stream's time base, in samples at the output rate.
    fn ts_to_samples(&self, ts: i64) -> i64 {
        if self.rate == 0 {
            // Before the first frame: the codec's rate is not known yet.
            return 0;
        }
        ffi::rescale(ts, self.time_base, self.sample_time_base())
    }

    /// Counts an error; fails when too many came in a row.
    fn tolerate(&mut self, operation: &'static str, code: c_int) -> Result<(), Error> {
        let error = AvError::new(code);
        if matches!(
            error.kind(),
            AvErrorKind::OutOfMemory | AvErrorKind::External
        ) {
            return Err(Error::Ffmpeg { operation, error });
        }
        self.errors_in_a_row += 1;
        if self.errors_in_a_row > MAX_ERRORS_IN_A_ROW {
            return Err(Error::Damaged { last: error });
        }
        Ok(())
    }

    /// Decodes the next frame into `self.frame`. `false` at the end of the stream.
    fn receive_frame(&mut self) -> Result<bool, Error> {
        loop {
            // SAFETY: an opened decoder and an allocated, empty frame.
            let ret = unsafe { ff::avcodec_receive_frame(self.codec.0, self.frame.0) };
            if ret >= 0 {
                if self.frame.get().nb_samples <= 0 {
                    self.frame.unref();
                    continue;
                }
                self.errors_in_a_row = 0;
                return Ok(true);
            }
            if ret == ff::AVERROR_EOF {
                return Ok(false);
            }
            if ret != ff::AVERROR(libc::EAGAIN) {
                self.tolerate("avcodec_receive_frame", ret)?;
                continue;
            }
            if self.draining {
                return Ok(false);
            }
            self.send_packet()?;
        }
    }

    /// Whether the demuxer's input has reached the end of the file.
    fn input_at_end(&self) -> bool {
        // SAFETY: fmt is open; pb is NULL or its I/O context.
        unsafe {
            let pb = (*self.media.input.fmt).pb;
            !pb.is_null() && ff::avio_feof(pb) != 0
        }
    }

    /// Hands the decoder the next packet of the chosen stream, or starts draining at
    /// the end of the file.
    fn send_packet(&mut self) -> Result<(), Error> {
        loop {
            if !self.packet_pending {
                // SAFETY: fmt is open and the packet is allocated and empty.
                let ret = unsafe { ff::av_read_frame(self.media.input.fmt, self.packet.0) };
                // Some demuxers (Westwood AUD, for one) report a short read at the
                // end of the file as invalid data rather than as its end.
                if ret == ff::AVERROR_EOF || (ret < 0 && self.input_at_end()) {
                    // SAFETY: a NULL packet starts draining an opened decoder.
                    unsafe { ff::avcodec_send_packet(self.codec.0, ptr::null()) };
                    self.draining = true;
                    return Ok(());
                }
                if ret < 0 {
                    if AvError::new(ret).kind() != AvErrorKind::InvalidData {
                        return Err(Error::ffmpeg("av_read_frame", ret));
                    }
                    self.tolerate("av_read_frame", ret)?;
                    continue;
                }
                // SAFETY: av_read_frame filled the packet.
                let index = unsafe { (*self.packet.0).stream_index };
                // Other streams' packets still arrive (a cover picture queued while
                // opening, packets buffered while probing): drop them.
                if usize::try_from(index).ok() != Some(self.stream_index) {
                    self.packet.unref();
                    continue;
                }
                self.mark_packet();
                self.packet_pending = true;
            }
            // SAFETY: an opened decoder and a packet of its stream, side data intact.
            let ret = unsafe { ff::avcodec_send_packet(self.codec.0, self.packet.0) };
            if ret == ff::AVERROR(libc::EAGAIN) {
                // The decoder wants its output read first; keep the packet.
                return Ok(());
            }
            self.packet.unref();
            self.packet_pending = false;
            if ret < 0 {
                self.tolerate("avcodec_send_packet", ret)?;
                continue;
            }
            return Ok(());
        }
    }
}

/// The stream's first packet, where reading and rewinding start.
#[derive(Debug, Clone, Copy)]
struct FirstPacket {
    /// Byte position in the file.
    pos: i64,
    /// Time stamp in the stream's time base.
    ts: Option<i64>,
    /// Size in bytes.
    size: i64,
}

impl AudioDecoder {
    /// Reads the next packet of the stream ahead, without decoding it: it stays
    /// pending and is sent to the decoder when its turn comes.
    fn peek_packet(&mut self) -> Option<FirstPacket> {
        while !self.packet_pending {
            // SAFETY: fmt is open and the packet is allocated and empty.
            let ret = unsafe { ff::av_read_frame(self.media.input.fmt, self.packet.0) };
            if ret < 0 {
                return None;
            }
            // SAFETY: av_read_frame filled the packet.
            let index = unsafe { (*self.packet.0).stream_index };
            if usize::try_from(index).ok() != Some(self.stream_index) {
                self.packet.unref();
                continue;
            }
            self.mark_packet();
            self.packet_pending = true;
        }
        // SAFETY: a packet read by av_read_frame.
        let p = unsafe { &*self.packet.0 };
        Some(FirstPacket {
            pos: p.pos,
            ts: [p.pts, p.dts]
                .into_iter()
                .find(|&t| t != ff::AV_NOPTS_VALUE),
            size: i64::from(p.size),
        })
    }

    /// Notes the first packet, and gives the packet just read its anchor.
    fn mark_packet(&mut self) {
        // SAFETY: av_read_frame filled the packet; its data is NULL or `size`
        // readable bytes owned by the packet.
        let (pos, pts, dts, data) = unsafe {
            let p = &*self.packet.0;
            let data: &[u8] = match usize::try_from(p.size) {
                Ok(len) if len > 0 && !p.data.is_null() => std::slice::from_raw_parts(p.data, len),
                _ => &[],
            };
            (p.pos, p.pts, p.dts, data)
        };
        if self.first_packet.is_none() {
            let ts = [pts, dts].into_iter().find(|&t| t != ff::AV_NOPTS_VALUE);
            self.first_packet = Some(FirstPacket {
                pos,
                ts,
                size: i64::try_from(data.len()).unwrap_or(0),
            });
        }
        if self.anchor == Anchor::None {
            return;
        }
        let value = self
            .anchor
            .of_packet(data, pos, self.first_packet.map(|f| f.pos));
        let opaque: *mut c_void = anchor::to_opaque(value);
        // SAFETY: a packet this decoder owns; opaque is a plain value FFmpeg only
        // copies (AV_CODEC_FLAG_COPY_OPAQUE), never dereferences.
        unsafe { (*self.packet.0).opaque = opaque };
    }
}

/// Allocates and opens a decoder context for `codec` with the stream's parameters.
///
/// # Safety
/// `codec` must be a decoder and `par` valid stream parameters.
unsafe fn open_codec(
    codec: *const ff::AVCodec,
    par: *const ff::AVCodecParameters,
    time_base: ff::AVRational,
    copy_opaque: bool,
) -> Result<CodecContext, Error> {
    let ctx = CodecContext::new(codec)?;
    // SAFETY: ctx is a fresh context for codec, par the stream's parameters.
    check("avcodec_parameters_to_context", unsafe {
        ff::avcodec_parameters_to_context(ctx.0, par)
    })?;
    // SAFETY: ctx is valid and not opened yet. FFmpeg needs the packet time base to
    // trim encoder delay and padding exactly.
    unsafe { (*ctx.0).pkt_timebase = time_base };
    if copy_opaque {
        // SAFETY: ctx is valid and not opened yet. With this flag the decoder
        // copies each packet's opaque value to the frames it decodes from it.
        unsafe { (*ctx.0).flags |= ff::AV_CODEC_FLAG_COPY_OPAQUE as c_int };
    }
    // SAFETY: ctx was allocated for codec; NULL options are allowed.
    check("avcodec_open2", unsafe {
        ff::avcodec_open2(ctx.0, codec, ptr::null_mut())
    })?;
    Ok(ctx)
}

/// Where exact positions after a seek come from, for a stream about to be decoded.
///
/// # Safety
/// `par` must be the valid parameters of a stream of `media`.
unsafe fn choose_anchor(
    media: &MediaFile,
    codec: &str,
    par: *const ff::AVCodecParameters,
) -> Anchor {
    // SAFETY: guaranteed by the caller.
    let (extradata, block_align) = unsafe {
        let len = usize::try_from((*par).extradata_size).unwrap_or(0);
        let extra: &[u8] = if len > 0 && !(*par).extradata.is_null() {
            std::slice::from_raw_parts((*par).extradata, len)
        } else {
            &[]
        };
        (extra, (*par).block_align)
    };
    if codec == "flac" {
        return Anchor::flac(extradata);
    }
    let format = media.format_name();
    // Compressed blocks (ADPCM, GSM) in a container whose audio data is one run of
    // bytes, read in whole blocks.
    if matches!(format, "wav" | "w64" | "aiff") && block_align > 1 {
        // SAFETY: par is valid; FFmpeg only reads it.
        let frames = unsafe { ff::av_get_audio_frame_duration2(par.cast_mut(), block_align) };
        if frames > 1 {
            return Anchor::Blocks {
                bytes: i64::from(block_align),
                frames: i64::from(frames),
                byte_seek: false,
            };
        }
    }
    // RealAudio 14.4 alone in a file: fixed-size packets back to back. Some files
    // carry no usable time stamps (FFmpeg's own RealMedia muxer writes 0 for every
    // packet); whether this one does is found out on its first two packets.
    if format == "rm" && codec == "ra_144" && media.streams().len() == 1 {
        return Anchor::Blocks {
            bytes: 0,
            frames: 0,
            byte_seek: true,
        };
    }
    Anchor::None
}

/// The size of the codec's frames, in frames at the output `rate`, when every
/// packet holds a whole number of them.
///
/// # Safety
/// `ctx` must be an opened decoder context and `par` its stream's parameters.
unsafe fn grid_step(
    codec: &str,
    rate: u32,
    ctx: *const ff::AVCodecContext,
    par: *const ff::AVCodecParameters,
) -> Option<i64> {
    let fixed = match codec {
        // The shortest Opus frame is 2.5 ms; every packet is a multiple of it.
        "opus" => rate.is_multiple_of(400).then_some(rate / 400),
        // WMA frames are 512, 1024 or 2048 frames long, by sample rate (FFmpeg's
        // ff_wma_get_frame_len_bits for versions 1 and 2).
        "wmav1" | "wmav2" => Some(if rate <= 16_000 {
            512
        } else if rate <= 22_050 || (rate <= 32_000 && codec == "wmav1") {
            1024
        } else {
            2048
        }),
        "nellymoser" => Some(256),
        // SWF ADPCM blocks hold 4096 frames.
        "adpcm_swf" => Some(4096),
        _ => None,
    };
    if let Some(step) = fixed {
        return Some(i64::from(step));
    }
    // SAFETY: guaranteed by the caller.
    let (frame_size, block_align) = unsafe { ((*ctx).frame_size, (*par).block_align) };
    if frame_size > 1 {
        return Some(i64::from(frame_size));
    }
    // SAFETY: par is valid; FFmpeg only reads it.
    let per_block = unsafe { ff::av_get_audio_frame_duration2(par.cast_mut(), block_align) };
    (per_block > 1).then(|| i64::from(per_block))
}

/// A frame's time stamp in the packet time base, if it has one.
fn frame_timestamp(frame: &ff::AVFrame) -> Option<i64> {
    [frame.pts, frame.best_effort_timestamp]
        .into_iter()
        .find(|&ts| ts != ff::AV_NOPTS_VALUE)
}
