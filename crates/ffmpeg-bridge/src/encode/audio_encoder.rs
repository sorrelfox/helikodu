// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The encoder and muxer of one output file.

use std::ffi::{CStr, c_int};
use std::fs::File;
use std::ptr;

use super::output_io::OutputIo;
use super::{EncodeFormat, EncodeQuality, EncoderConfig, find_encoder, read_list};
use crate::decode::{CodecContext, Frame, Packet};
use crate::error::{Error, check};
use crate::ffi::{self, ff, sample_format as sf};
use crate::io::deny_io_open;

/// The number of frames per encoder call for encoders that take any number.
const VARIABLE_CHUNK: usize = 4096;

/// The shortest last frame FFmpeg's ALAC encoder takes with three or more
/// channels. With one or two frames of four, five or six channels it aborts the
/// process (FFmpeg 9.0; found by fuzzing), so a shorter end is padded with
/// silence: at most 0.4 ms, only in that case.
const ALAC_MIN_LAST_FRAME: usize = 16;

/// The sample formats the bridge can fill a frame with.
const FILLABLE: [c_int; 6] = [sf::S16, sf::S16P, sf::S32, sf::S32P, sf::FLT, sf::FLTP];

/// The sample rates `codec` takes; `None` for any.
///
/// # Safety
/// `codec` must be a valid encoder.
pub(super) unsafe fn supported_rates(codec: *const ff::AVCodec) -> Option<Vec<u32>> {
    let mut list: *const std::ffi::c_void = ptr::null();
    let mut count: c_int = 0;
    // SAFETY: a NULL context is allowed; the out-pointers are valid.
    let result = unsafe {
        ff::avcodec_get_supported_config(
            ptr::null(),
            codec,
            ff::AVCodecConfig::AV_CODEC_CONFIG_SAMPLE_RATE,
            0,
            &mut list,
            &mut count,
        )
    };
    if result < 0 {
        return None;
    }
    // SAFETY: FFmpeg hands back `count` ints, or NULL for any rate.
    let rates: Vec<c_int> = unsafe { read_list(list, count) }?;
    Some(
        rates
            .into_iter()
            .filter_map(|r| u32::try_from(r).ok())
            .collect(),
    )
}

/// The sample formats `codec` takes; `None` for any.
///
/// # Safety
/// `codec` must be a valid encoder.
unsafe fn supported_formats(codec: *const ff::AVCodec) -> Option<Vec<c_int>> {
    let mut list: *const std::ffi::c_void = ptr::null();
    let mut count: c_int = 0;
    // SAFETY: as in supported_rates.
    let result = unsafe {
        ff::avcodec_get_supported_config(
            ptr::null(),
            codec,
            ff::AVCodecConfig::AV_CODEC_CONFIG_SAMPLE_FORMAT,
            0,
            &mut list,
            &mut count,
        )
    };
    if result < 0 {
        return None;
    }
    // SAFETY: FFmpeg hands back `count` AVSampleFormat values, which C stores as
    // ints; they are read as ints, so an unknown value is harmless.
    unsafe { read_list(list, count) }
}

/// The sample format to give the encoder, from those it takes.
fn choose_format(supported: Option<&[c_int]>, lossless: bool, bits: u8) -> Option<c_int> {
    let order: &[c_int] = if !lossless {
        &[sf::FLTP, sf::FLT, sf::S16P, sf::S16, sf::S32P, sf::S32]
    } else if bits <= 16 {
        &[sf::S16, sf::S16P, sf::S32, sf::S32P]
    } else {
        &[sf::S32, sf::S32P, sf::S16, sf::S16P]
    };
    match supported {
        None => order.first().copied(),
        Some(list) => order
            .iter()
            .copied()
            .find(|f| list.contains(f) && FILLABLE.contains(f)),
    }
}

/// Samples waiting for a whole encoder frame, in the encoder's representation.
#[derive(Debug)]
enum Fifo {
    /// Full-scale 32-bit integers (16-bit encoders take the top bits).
    Int(Vec<i32>),
    /// Floats, full scale at ±1.0.
    Float(Vec<f32>),
}

/// Writes one audio file: encodes interleaved PCM and muxes it.
///
/// Feed samples with [`write_f32`](Self::write_f32) or
/// [`write_i32`](Self::write_i32) and end with [`finish`](Self::finish). Dropping
/// the encoder without finishing leaves an incomplete file, which the caller
/// removes.
pub struct AudioEncoder {
    codec: CodecContext,
    fmt: *mut ff::AVFormatContext,
    io: OutputIo,
    frame: Frame,
    packet: Packet,
    encoder: String,
    sample_format: c_int,
    channels: usize,
    frame_size: usize,
    pad_last: bool,
    /// The fewest frames the last frame may hold; shorter ones are padded.
    min_last: usize,
    fifo: Fifo,
    pts: i64,
    stream_time_base: ff::AVRational,
    codec_time_base: ff::AVRational,
}

// SAFETY: every FFmpeg object is owned by this value and used from one thread at a
// time; FFmpeg keeps no thread-local state for them.
unsafe impl Send for AudioEncoder {}

impl std::fmt::Debug for AudioEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEncoder")
            .field("encoder", &self.encoder)
            .field("channels", &self.channels)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

impl Drop for AudioEncoder {
    fn drop(&mut self) {
        if !self.fmt.is_null() {
            // SAFETY: allocated by avformat_alloc_output_context2. With custom I/O
            // it never frees pb, which `io` frees after this.
            unsafe { ff::avformat_free_context(self.fmt) };
            self.fmt = ptr::null_mut();
        }
    }
}

/// Owns an output format context until it is handed to the encoder.
struct OutputContext(*mut ff::AVFormatContext);

impl Drop for OutputContext {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by avformat_alloc_output_context2.
            unsafe { ff::avformat_free_context(self.0) };
        }
    }
}

/// An option dictionary that is freed on drop.
struct Options(*mut ff::AVDictionary);

impl Options {
    fn leftover(&self) -> Option<String> {
        // SAFETY: NULL or a dictionary this value owns.
        unsafe { ffi::dict_entries(self.0) }
            .into_iter()
            .next()
            .map(|(key, _)| key)
    }

    fn set(&mut self, key: &CStr, value: &str) -> Result<(), Error> {
        let value = std::ffi::CString::new(value).map_err(|_| Error::InvalidAudioFormat {
            reason: "an encoder option holds a NUL byte",
        })?;
        // SAFETY: NUL-terminated strings, copied by FFmpeg.
        check("av_dict_set", unsafe {
            ff::av_dict_set(&mut self.0, key.as_ptr(), value.as_ptr(), 0)
        })
        .map(drop)
    }
}

impl Drop for Options {
    fn drop(&mut self) {
        // SAFETY: NULL or a dictionary this value owns.
        unsafe { ff::av_dict_free(&mut self.0) };
    }
}

impl AudioEncoder {
    /// Starts writing `config.format` into `file`, which must be empty and
    /// writable. The file's name is never given to FFmpeg.
    pub fn create(file: File, config: &EncoderConfig) -> Result<Self, Error> {
        let format = config.format;
        let channels = usize::from(config.channels);
        if channels == 0 || config.channels > format.max_channels() {
            return Err(Error::InvalidAudioFormat {
                reason: "the format cannot take this many channels",
            });
        }
        let rate = c_int::try_from(config.sample_rate)
            .ok()
            .filter(|r| *r > 0)
            .ok_or(Error::InvalidAudioFormat {
                reason: "invalid sample rate",
            })?;
        let (codec, encoder) =
            find_encoder(format, config.bits).ok_or_else(|| Error::EncoderNotFound {
                format: format.name().to_owned(),
            })?;
        // SAFETY: codec is a valid encoder.
        if let Some(rates) = unsafe { supported_rates(codec) }
            && !rates.contains(&config.sample_rate)
        {
            return Err(Error::InvalidAudioFormat {
                reason: "the encoder does not take this sample rate",
            });
        }
        // SAFETY: as above.
        let supported = unsafe { supported_formats(codec) };
        let sample_format = choose_format(supported.as_deref(), format.is_lossless(), config.bits)
            .ok_or(Error::UnsupportedSampleFormat { format: -1 })?;

        // The output context, locked down before anything is written. The I/O
        // context is declared first so that it is dropped after the format context.
        let io = OutputIo::new(file)?;
        let mut out = OutputContext(ptr::null_mut());
        // SAFETY: the muxer name is one of the fixed, NUL-terminated names; no
        // file name is given.
        check("avformat_alloc_output_context2", unsafe {
            ff::avformat_alloc_output_context2(
                &mut out.0,
                ptr::null(),
                format.muxer().as_ptr(),
                ptr::null(),
            )
        })?;
        if out.0.is_null() {
            return Err(Error::out_of_memory("avformat_alloc_output_context2"));
        }
        // SAFETY: out.0 is a fresh output context. `pb` is the custom context,
        // which outlives it; every nested open is refused.
        unsafe {
            (*out.0).pb = io.as_ptr();
            (*out.0).flags |= ff::AVFMT_FLAG_CUSTOM_IO;
            (*out.0).io_open = Some(deny_io_open);
        }
        // SAFETY: an AVFormatContext is an AVClass-enabled object; strings are
        // NUL-terminated and copied.
        check("av_opt_set", unsafe {
            ff::av_opt_set(
                out.0.cast(),
                c"protocol_whitelist".as_ptr(),
                c"file".as_ptr(),
                0,
            )
        })?;
        // SAFETY: the field is NULL or a NUL-terminated string the context owns.
        let whitelist = unsafe { ffi::string((*out.0).protocol_whitelist) };
        if whitelist.as_deref() != Some("file") {
            return Err(Error::OptionNotApplied {
                key: "protocol_whitelist".to_owned(),
            });
        }
        // SAFETY: oformat was set by avformat_alloc_output_context2.
        let global_header = unsafe { (*(*out.0).oformat).flags } & ff::AVFMT_GLOBALHEADER != 0;

        // The encoder.
        let ctx = CodecContext::new(codec)?;
        let mut options = Options(ptr::null_mut());
        // SAFETY: ctx is a fresh context for codec, not opened yet. The sample
        // format is one the encoder lists, written as the int C stores.
        unsafe {
            let c = ctx.0;
            ptr::addr_of_mut!((*c).sample_fmt)
                .cast::<c_int>()
                .write(sample_format);
            (*c).sample_rate = rate;
            (*c).time_base = ff::AVRational { num: 1, den: rate };
            ff::av_channel_layout_default(&mut (*c).ch_layout, c_int::from(config.channels));
            if global_header {
                (*c).flags |= ff::AV_CODEC_FLAG_GLOBAL_HEADER as c_int;
            }
            if format.is_lossless() && matches!(sample_format, sf::S32 | sf::S32P) {
                (*c).bits_per_raw_sample = 24;
            }
            match config.quality {
                EncodeQuality::Bitrate(kbps) => {
                    (*c).bit_rate = i64::from(kbps) * 1000;
                }
                EncodeQuality::Vbr(level) if format == EncodeFormat::Mp3 => {
                    (*c).flags |= ff::AV_CODEC_FLAG_QSCALE as c_int;
                    (*c).global_quality = c_int::from(level.min(9)) * ff::FF_QP2LAMBDA;
                }
                EncodeQuality::Vbr(level) => {
                    (*c).bit_rate = i64::from(vbr_bitrate(level)) * 1000;
                }
                EncodeQuality::Lossless(Some(level)) => {
                    (*c).compression_level = c_int::from(level);
                }
                EncodeQuality::Lossless(None) => {}
            }
        }
        if encoder == "opus" {
            // FFmpeg's own Opus encoder is still marked experimental.
            options.set(c"strict", "experimental")?;
        }
        // SAFETY: ctx was allocated for codec; options is a dictionary we own,
        // which FFmpeg replaces with the options it did not use.
        check("avcodec_open2", unsafe {
            ff::avcodec_open2(ctx.0, codec, &mut options.0)
        })?;
        if let Some(key) = options.leftover() {
            return Err(Error::OptionNotApplied { key });
        }

        // SAFETY: out.0 is valid; the new stream belongs to it.
        let stream = unsafe { ff::avformat_new_stream(out.0, ptr::null()) };
        if stream.is_null() {
            return Err(Error::out_of_memory("avformat_new_stream"));
        }
        // SAFETY: stream and the opened ctx are valid.
        check("avcodec_parameters_from_context", unsafe {
            ff::avcodec_parameters_from_context((*stream).codecpar, ctx.0)
        })?;
        // SAFETY: as above.
        unsafe { (*stream).time_base = (*ctx.0).time_base };
        let mut header_options = Options(ptr::null_mut());
        // SAFETY: out.0 has its stream and pb; the dictionary is ours.
        check("avformat_write_header", unsafe {
            ff::avformat_write_header(out.0, &mut header_options.0)
        })?;
        if let Some(key) = header_options.leftover() {
            return Err(Error::OptionNotApplied { key });
        }
        // SAFETY: the opened context and its codec are valid.
        let (frame_size, capabilities, codec_time_base, stream_time_base) = unsafe {
            (
                (*ctx.0).frame_size,
                (*codec).capabilities,
                (*ctx.0).time_base,
                (*stream).time_base,
            )
        };
        let variable = capabilities as u32 & ff::AV_CODEC_CAP_VARIABLE_FRAME_SIZE != 0;
        let frame_size = match usize::try_from(frame_size) {
            Ok(n) if n > 0 && !variable => n,
            _ => VARIABLE_CHUNK,
        };
        let pad_last = !variable
            && capabilities as u32 & ff::AV_CODEC_CAP_SMALL_LAST_FRAME == 0
            && frame_size != VARIABLE_CHUNK;
        let min_last = if encoder == "alac" && channels >= 3 {
            ALAC_MIN_LAST_FRAME.min(frame_size)
        } else {
            0
        };
        let fifo = if matches!(sample_format, sf::FLT | sf::FLTP) {
            Fifo::Float(Vec::new())
        } else {
            Fifo::Int(Vec::new())
        };
        let fmt = std::mem::replace(&mut out.0, ptr::null_mut());
        Ok(Self {
            codec: ctx,
            fmt,
            io,
            frame: Frame::new()?,
            packet: Packet::new()?,
            encoder,
            sample_format,
            channels,
            frame_size,
            pad_last,
            min_last,
            fifo,
            pts: 0,
            stream_time_base,
            codec_time_base,
        })
    }

    /// The encoder in use, by FFmpeg's name (`libopus`, `flac`, …).
    pub fn encoder_name(&self) -> &str {
        &self.encoder
    }

    /// Encodes interleaved float samples, full scale at ±1.0, in WAVE channel
    /// order. Any number of whole frames. Samples that are not numbers become
    /// silence, and samples beyond ±[`MAX_FLOAT_SAMPLE`] are clipped there: some
    /// encoders (LAME) abort the process on such input.
    pub fn write_f32(&mut self, interleaved: &[f32]) -> Result<(), Error> {
        match &mut self.fifo {
            Fifo::Float(fifo) => fifo.extend(interleaved.iter().map(|&x| float_sample(x))),
            Fifo::Int(fifo) => fifo.extend(interleaved.iter().map(|&x| f32_to_i32(x))),
        }
        self.encode_whole_frames()
    }

    /// Encodes interleaved full-scale 32-bit integers (a 16-bit sample in the top
    /// 16 bits, a 24-bit one in the top 24), in WAVE channel order.
    pub fn write_i32(&mut self, interleaved: &[i32]) -> Result<(), Error> {
        match &mut self.fifo {
            Fifo::Int(fifo) => fifo.extend_from_slice(interleaved),
            Fifo::Float(fifo) => fifo.extend(
                interleaved
                    .iter()
                    .map(|&x| (f64::from(x) / 2_147_483_648.0) as f32),
            ),
        }
        self.encode_whole_frames()
    }

    /// Encodes what is left, writes the end of the file and flushes it to disk.
    /// Returns the size of the file in bytes.
    pub fn finish(mut self) -> Result<u64, Error> {
        let left = self.fifo_frames();
        if left > 0 {
            let padded = if self.pad_last {
                self.frame_size
            } else {
                left.max(self.min_last)
            };
            let frames = if padded > left {
                let missing = (padded - left) * self.channels;
                match &mut self.fifo {
                    Fifo::Int(f) => f.extend(std::iter::repeat_n(0, missing)),
                    Fifo::Float(f) => f.extend(std::iter::repeat_n(0.0, missing)),
                }
                padded
            } else {
                left
            };
            self.encode_frame(frames)?;
        }
        // SAFETY: an opened encoder; NULL starts flushing.
        let sent = unsafe { ff::avcodec_send_frame(self.codec.0, ptr::null()) };
        if sent < 0 && sent != ff::AVERROR_EOF {
            return Err(Error::ffmpeg("avcodec_send_frame", sent));
        }
        self.drain()?;
        // SAFETY: the header was written; every packet is muxed.
        check("av_write_trailer", unsafe {
            ff::av_write_trailer(self.fmt)
        })?;
        self.io.flush();
        self.io.sync()?;
        Ok(self.io.written())
    }

    fn fifo_frames(&self) -> usize {
        match &self.fifo {
            Fifo::Int(f) => f.len() / self.channels,
            Fifo::Float(f) => f.len() / self.channels,
        }
    }

    fn encode_whole_frames(&mut self) -> Result<(), Error> {
        while self.fifo_frames() >= self.frame_size {
            self.encode_frame(self.frame_size)?;
        }
        Ok(())
    }

    /// Encodes the first `frames` frames of the FIFO.
    fn encode_frame(&mut self, frames: usize) -> Result<(), Error> {
        let samples = frames * self.channels;
        let nb_samples = c_int::try_from(frames).map_err(|_| Error::InvalidAudioFormat {
            reason: "too many frames at once",
        })?;
        self.frame.unref();
        let f = self.frame.0;
        // SAFETY: an empty frame; the parameters match the opened encoder, and
        // av_frame_get_buffer allocates its planes.
        unsafe {
            (*f).nb_samples = nb_samples;
            ptr::addr_of_mut!((*f).format).write(self.sample_format);
            (*f).sample_rate = (*self.codec.0).sample_rate;
            check(
                "av_channel_layout_copy",
                ff::av_channel_layout_copy(&mut (*f).ch_layout, &(*self.codec.0).ch_layout),
            )?;
            check("av_frame_get_buffer", ff::av_frame_get_buffer(f, 0))?;
            (*f).pts = self.pts;
        }
        let planar = matches!(self.sample_format, sf::S16P | sf::S32P | sf::FLTP);
        let planes = if planar { self.channels } else { 1 };
        for plane in 0..planes {
            // SAFETY: an allocated audio frame has one data pointer per plane in
            // extended_data, each holding nb_samples samples (times the channels
            // when interleaved) of the sample format.
            let data = unsafe { *(*f).extended_data.add(plane) };
            let per_plane = if planar { frames } else { samples };
            let pick = |i: usize| {
                if planar { i * self.channels + plane } else { i }
            };
            match (&self.fifo, self.sample_format) {
                (Fifo::Int(src), sf::S16 | sf::S16P) => {
                    // SAFETY: see above; i16 samples.
                    let dst =
                        unsafe { std::slice::from_raw_parts_mut(data.cast::<i16>(), per_plane) };
                    for (i, d) in dst.iter_mut().enumerate() {
                        *d = i32_to_i16(src[pick(i)]);
                    }
                }
                (Fifo::Int(src), _) => {
                    // SAFETY: see above; i32 samples.
                    let dst =
                        unsafe { std::slice::from_raw_parts_mut(data.cast::<i32>(), per_plane) };
                    for (i, d) in dst.iter_mut().enumerate() {
                        *d = src[pick(i)];
                    }
                }
                (Fifo::Float(src), _) => {
                    // SAFETY: see above; f32 samples.
                    let dst =
                        unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), per_plane) };
                    for (i, d) in dst.iter_mut().enumerate() {
                        *d = src[pick(i)];
                    }
                }
            }
        }
        match &mut self.fifo {
            Fifo::Int(fifo) => drop(fifo.drain(..samples)),
            Fifo::Float(fifo) => drop(fifo.drain(..samples)),
        }
        self.pts += i64::from(nb_samples);
        // SAFETY: an opened encoder and a complete frame, which FFmpeg references.
        check("avcodec_send_frame", unsafe {
            ff::avcodec_send_frame(self.codec.0, self.frame.0)
        })?;
        self.drain()
    }

    /// Muxes every packet the encoder has ready.
    fn drain(&mut self) -> Result<(), Error> {
        loop {
            // SAFETY: an opened encoder and an allocated packet.
            let got = unsafe { ff::avcodec_receive_packet(self.codec.0, self.packet.0) };
            if got == ff::AVERROR(libc::EAGAIN) || got == ff::AVERROR_EOF {
                return Ok(());
            }
            check("avcodec_receive_packet", got)?;
            // SAFETY: a packet the encoder filled; the muxer takes its reference.
            unsafe {
                (*self.packet.0).stream_index = 0;
                ff::av_packet_rescale_ts(
                    self.packet.0,
                    self.codec_time_base,
                    self.stream_time_base,
                );
                let written = ff::av_interleaved_write_frame(self.fmt, self.packet.0);
                self.packet.unref();
                check("av_interleaved_write_frame", written)?;
            }
        }
    }
}

/// The largest float sample the encoder passes on, 24 dB above full scale: room
/// for the overs of any real source.
pub const MAX_FLOAT_SAMPLE: f32 = 16.0;

/// A float sample an encoder can take: NaN as silence, the rest clipped to
/// ±[`MAX_FLOAT_SAMPLE`].
fn float_sample(x: f32) -> f32 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(-MAX_FLOAT_SAMPLE, MAX_FLOAT_SAMPLE)
    }
}

/// A float sample as a full-scale 32-bit integer, clipped.
fn f32_to_i32(x: f32) -> i32 {
    let scaled = (f64::from(x) * 2_147_483_648.0).round();
    scaled.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// The top 16 bits of a full-scale 32-bit sample, rounded and clipped.
fn i32_to_i16(x: i32) -> i16 {
    let rounded = (i64::from(x) + 0x8000) >> 16;
    rounded.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

/// A bit rate in kbit/s for a LAME-style VBR level, for encoders without one.
fn vbr_bitrate(level: u8) -> u32 {
    match level {
        0 => 256,
        1 => 224,
        2 => 192,
        3 => 175,
        4 => 160,
        5 => 128,
        6 => 112,
        7 => 96,
        _ => 80,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_conversions_round_and_clip() {
        assert_eq!(f32_to_i32(1.5), i32::MAX);
        assert_eq!(f32_to_i32(-1.0), i32::MIN);
        assert_eq!(i32_to_i16(0x1234_0000), 0x1234);
        assert_eq!(i32_to_i16(i32::MAX), i16::MAX);
        assert_eq!(i32_to_i16(i32::MIN), i16::MIN);
    }

    #[test]
    fn float_samples_are_numbers_within_range() {
        assert_eq!(float_sample(f32::NAN), 0.0);
        assert_eq!(float_sample(f32::INFINITY), MAX_FLOAT_SAMPLE);
        assert_eq!(float_sample(-1e30), -MAX_FLOAT_SAMPLE);
        assert_eq!(float_sample(0.5), 0.5);
        assert_eq!(f32_to_i32(f32::NAN), 0);
    }

    #[test]
    fn lossless_formats_keep_their_depth() {
        let both = [sf::S16, sf::S32];
        assert_eq!(choose_format(Some(&both), true, 16), Some(sf::S16));
        assert_eq!(choose_format(Some(&both), true, 24), Some(sf::S32));
        assert_eq!(choose_format(Some(&[sf::FLTP]), false, 16), Some(sf::FLTP));
        assert_eq!(choose_format(Some(&[sf::DBL]), false, 16), None);
    }
}
