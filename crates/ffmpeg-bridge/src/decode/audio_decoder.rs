// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The decoder of one audio stream: decoding, gapless trimming and seeking.

use std::ffi::c_int;
use std::ptr;

use helikodu_model::audio::{
    AudioSpec, ChannelLayout, Endianness, MediaTime, SampleFormat, SampleRate,
};

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
        let ctx = CodecContext::new(codec)?;
        // SAFETY: ctx is a fresh context for codec, par the stream's parameters.
        check("avcodec_parameters_to_context", unsafe {
            ff::avcodec_parameters_to_context(ctx.0, par)
        })?;
        // SAFETY: ctx is valid and not opened yet. FFmpeg needs the packet time
        // base to trim encoder delay and padding exactly.
        unsafe { (*ctx.0).pkt_timebase = time_base };
        // SAFETY: ctx was allocated for codec; NULL options are allowed.
        check("avcodec_open2", unsafe {
            ff::avcodec_open2(ctx.0, codec, ptr::null_mut())
        })?;
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
            discard_until: None,
            packet_pending: false,
            draining: false,
            finished: false,
            errors_in_a_row: 0,
            buffer: PcmBuffer::new(PLACEHOLDER_SPEC),
        };

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
    /// audio before the target is decoded and dropped. When the demuxer lands after
    /// the target, the seek is repeated further back, and as a last resort from the
    /// start.
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
            self.demuxer_seek(start)?;
            if !self.receive_frame()? {
                // Landed at the end; only right if the target is there too.
                if start == 0 {
                    break;
                }
                back = back.saturating_mul(4).saturating_add(u64::from(self.rate));
                continue;
            }
            match frame_timestamp(self.frame.get()) {
                Some(ts) => {
                    let position = self.ts_to_samples(ts) - self.origin;
                    if position <= target_i || start == 0 {
                        self.frame_ready = true;
                        self.frame_position = Some(position);
                        self.discard_until = Some(target);
                        return Ok(());
                    }
                }
                None => break,
            }
            back = back.saturating_mul(4).saturating_add(u64::from(self.rate));
        }
        // From the start, counting frames.
        self.demuxer_seek(0)?;
        self.frame_position = Some(0);
        self.discard_until = Some(target);
        Ok(())
    }

    /// Seeks the demuxer to a key point at or before `position`, flushes the
    /// decoder and resets the reading state.
    fn demuxer_seek(&mut self, position: u64) -> Result<(), Error> {
        let samples = i64::try_from(position)
            .unwrap_or(i64::MAX)
            .saturating_add(self.origin);
        let ts = if position == 0 {
            self.origin_ts
        } else {
            ffi::rescale(samples, self.sample_time_base(), self.time_base)
        };
        let index = c_int::try_from(self.stream_index).unwrap_or(c_int::MAX);
        // SAFETY: fmt is open and index is one of its streams.
        let mut ret =
            unsafe { ff::avformat_seek_file(self.media.input.fmt, index, i64::MIN, ts, ts, 0) };
        if ret < 0 {
            // SAFETY: as above.
            ret = unsafe {
                ff::av_seek_frame(self.media.input.fmt, index, ts, ff::AVSEEK_FLAG_BACKWARD)
            };
        }
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

    /// Hands the decoder the next packet of the chosen stream, or starts draining at
    /// the end of the file.
    fn send_packet(&mut self) -> Result<(), Error> {
        loop {
            if !self.packet_pending {
                // SAFETY: fmt is open and the packet is allocated and empty.
                let ret = unsafe { ff::av_read_frame(self.media.input.fmt, self.packet.0) };
                if ret == ff::AVERROR_EOF {
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

/// A frame's time stamp in the packet time base, if it has one.
fn frame_timestamp(frame: &ff::AVFrame) -> Option<i64> {
    [frame.pts, frame.best_effort_timestamp]
        .into_iter()
        .find(|&ts| ts != ff::AV_NOPTS_VALUE)
}
