// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A minimal safe layer over the raw FFmpeg bindings: open a local file through
//! custom I/O with the offline locks, choose one audio stream, decode it to PCM.
//!
//! Every pointer FFmpeg hands back is owned by exactly one Rust value whose
//! `Drop` releases it. Callbacks that FFmpeg calls catch panics and turn them
//! into error codes, because unwinding into C is undefined behaviour.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::ptr;
use std::sync::Mutex;

use ffmpeg_sys_next as ff;

use crate::probe;

/// Demuxers the player accepts. Every one reads a single self-contained file.
/// Left out on purpose: hls, dash, concat, imf, image sequences, mlv, vobsub,
/// dvdvideo, avisynth, vapoursynth, sap, rtsp, sdp and all device inputs.
pub const FORMAT_WHITELIST: &str = "aac,ac3,aiff,amr,ape,asf,au,avi,caf,dsf,dts,eac3,flac,flv,\
     iff,matroska,mov,mp3,mpc,mpc8,mpegps,mpegts,mxf,nut,ogg,rm,shn,tak,tta,voc,w64,wav,wv";

/// The FFmpeg error code with the operation that produced it.
#[derive(Debug, Clone)]
pub struct FfError {
    pub during: &'static str,
    pub code: c_int,
}

impl fmt::Display for FfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.during, err_string(self.code))
    }
}

fn check(during: &'static str, code: c_int) -> Result<c_int, FfError> {
    if code < 0 {
        Err(FfError { during, code })
    } else {
        Ok(code)
    }
}

pub fn err_string(code: c_int) -> String {
    let mut buf = [0 as c_char; 128];
    // SAFETY: buf is writable for its full length; av_strerror always
    // NUL-terminates within errbuf_size.
    unsafe { ff::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    // SAFETY: av_strerror wrote a NUL-terminated string into buf.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn cstr_or(p: *const c_char, fallback: &str) -> String {
    if p.is_null() {
        fallback.to_owned()
    } else {
        // SAFETY: FFmpeg returns static or context-owned NUL-terminated names.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

pub fn ffmpeg_version() -> String {
    // SAFETY: returns a static NUL-terminated string.
    let v = cstr_or(unsafe { ff::av_version_info() }, "?");
    // SAFETY: plain version getters without arguments.
    let (c, f, u) = unsafe {
        (
            ff::avcodec_version(),
            ff::avformat_version(),
            ff::avutil_version(),
        )
    };
    let s = |x: u32| format!("{}.{}.{}", x >> 16, (x >> 8) & 0xff, x & 0xff);
    format!(
        "FFmpeg {v} (libavcodec {}, libavformat {}, libavutil {})",
        s(c),
        s(f),
        s(u)
    )
}

/// Which of the four offline locks are applied. The real bridge always applies
/// all four; the spike switches them one by one to show what each one stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lockdown {
    pub custom_io: bool,
    pub protocol_whitelist: bool,
    pub format_whitelist: bool,
    pub deny_io_open: bool,
}

impl Lockdown {
    pub const ALL: Lockdown = Lockdown {
        custom_io: true,
        protocol_whitelist: true,
        format_whitelist: true,
        deny_io_open: true,
    };
    pub const NONE: Lockdown = Lockdown {
        custom_io: false,
        protocol_whitelist: false,
        format_whitelist: false,
        deny_io_open: false,
    };
}

#[derive(Clone, Debug)]
pub struct OpenOptions {
    pub lockdown: Lockdown,
    /// With custom I/O, a file name to show FFmpeg anyway. The bridge never sets
    /// it; the lock-down test uses it to let the hls demuxer through its
    /// extension check so the io_open lock can be tested on its own.
    pub url_hint: Option<String>,
    /// Test hook: make the custom read callback panic.
    pub inject_read_panic: bool,
    /// Set `AVDISCARD_ALL` on every non-audio stream *before*
    /// `avformat_find_stream_info`.
    pub discard_before_probe: bool,
    /// `codec_whitelist` on the format context: limits the decoders that
    /// `avformat_find_stream_info` may open internally.
    pub codec_whitelist: Option<String>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions {
            lockdown: Lockdown::ALL,
            discard_before_probe: true,
            codec_whitelist: Some(audio_decoder_whitelist()),
            url_hint: None,
            inject_read_panic: false,
        }
    }
}

/// Every audio decoder this FFmpeg build has, as a `codec_whitelist` value.
///
/// `avformat_find_stream_info` opens decoders internally to fill in missing
/// stream parameters, also for streams marked `AVDISCARD_ALL`. With this list
/// it still calls `avcodec_open2` for video streams, but the call fails before
/// the decoder is initialised.
pub fn audio_decoder_whitelist() -> String {
    let mut names = Vec::new();
    let mut it: *mut c_void = ptr::null_mut();
    loop {
        // SAFETY: av_codec_iterate walks FFmpeg's static codec table with an
        // opaque cursor that starts as NULL.
        let c = unsafe { ff::av_codec_iterate(&mut it) };
        if c.is_null() {
            break;
        }
        // SAFETY: c points to a static AVCodec; the type is read as c_int.
        let (is_dec, t, name) = unsafe {
            (
                ff::av_codec_is_decoder(c) != 0,
                ptr::addr_of!((*c).type_).cast::<c_int>().read(),
                (*c).name,
            )
        };
        if is_dec && t == AUDIO {
            names.push(cstr_or(name, ""));
        }
    }
    names.join(",")
}

// ---------------------------------------------------------------------------
// Custom I/O

struct FileIo {
    file: File,
    size: u64,
    /// Test hook: panic inside the callback to show the panic stays in Rust.
    panic_on_read: bool,
}

unsafe extern "C" fn io_read(opaque: *mut c_void, buf: *mut u8, size: c_int) -> c_int {
    let r = catch_unwind(AssertUnwindSafe(|| {
        if size <= 0 {
            // A read callback must never return 0; that would look like progress.
            return ff::AVERROR(libc::EINVAL);
        }
        // SAFETY: opaque is the Box<FileIo> pointer given to avio_alloc_context;
        // the box outlives the AVIOContext (see Input::drop) and FFmpeg calls
        // the callbacks of one AVIOContext from one thread at a time.
        let io = unsafe { &mut *opaque.cast::<FileIo>() };
        // SAFETY: FFmpeg guarantees buf is writable for size bytes.
        let dst = unsafe { std::slice::from_raw_parts_mut(buf, size as usize) };
        if io.panic_on_read {
            panic!("injected panic in the read callback");
        }
        match io.file.read(dst) {
            Ok(0) => ff::AVERROR_EOF,
            Ok(n) => n as c_int,
            Err(e) => ff::AVERROR(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }));
    r.unwrap_or(ff::AVERROR_EXTERNAL)
}

unsafe extern "C" fn io_seek(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    let r = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: as in io_read.
        let io = unsafe { &mut *opaque.cast::<FileIo>() };
        if whence & ff::AVSEEK_SIZE != 0 {
            return io.size as i64;
        }
        let pos = match whence & !ff::AVSEEK_FORCE {
            libc::SEEK_SET if offset >= 0 => SeekFrom::Start(offset as u64),
            libc::SEEK_CUR => SeekFrom::Current(offset),
            libc::SEEK_END => SeekFrom::End(offset),
            _ => return i64::from(ff::AVERROR(libc::EINVAL)),
        };
        match io.file.seek(pos) {
            Ok(p) => p as i64,
            Err(e) => i64::from(ff::AVERROR(e.raw_os_error().unwrap_or(libc::EIO))),
        }
    }));
    r.unwrap_or(i64::from(ff::AVERROR_EXTERNAL))
}

// ---------------------------------------------------------------------------
// Nested opens

type IoOpenFn = unsafe extern "C" fn(
    *mut ff::AVFormatContext,
    *mut *mut ff::AVIOContext,
    *const c_char,
    c_int,
    *mut *mut ff::AVDictionary,
) -> c_int;

/// Shared by every format context of one input, including nested ones (FFmpeg
/// copies `opaque` into the contexts that demuxers such as hls create).
struct CallbackState {
    deny: bool,
    default_io_open: Option<IoOpenFn>,
    attempts: Mutex<Vec<String>>,
}

/// Records every nested open. With `deny` it refuses all of them; otherwise it
/// forwards to FFmpeg's default so the spike can show what an unprotected
/// open would have done.
unsafe extern "C" fn io_open_cb(
    s: *mut ff::AVFormatContext,
    pb: *mut *mut ff::AVIOContext,
    url: *const c_char,
    flags: c_int,
    options: *mut *mut ff::AVDictionary,
) -> c_int {
    let denied = ff::AVERROR(libc::EPERM);
    let r = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: FFmpeg passes a valid format context. Its opaque is either NULL
        // or our CallbackState, which lives until the context is closed.
        let state = unsafe {
            let opaque = (*s).opaque;
            if opaque.is_null() {
                return denied;
            }
            &*opaque.cast::<CallbackState>()
        };
        if let Ok(mut a) = state.attempts.lock() {
            a.push(cstr_or(url, "<null>"));
        }
        if state.deny {
            return denied;
        }
        match state.default_io_open {
            // SAFETY: forwards FFmpeg's own arguments unchanged to FFmpeg's own
            // default callback.
            Some(f) => unsafe { f(s, pb, url, flags, options) },
            None => denied,
        }
    }));
    r.unwrap_or(denied)
}

// ---------------------------------------------------------------------------
// Input

#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub index: usize,
    pub media_type: String,
    pub codec: String,
    pub default: bool,
    pub attached_pic: bool,
}

#[derive(Clone, Debug)]
pub struct InputInfo {
    pub format: String,
    pub streams: Vec<StreamInfo>,
    /// The audio stream the bridge chose.
    pub chosen: usize,
    /// What `av_find_best_stream` alone would have chosen.
    pub best_stream: i32,
    /// Option keys FFmpeg did not recognise (they would be silently ignored).
    pub unused_options: Vec<String>,
}

pub struct Input {
    fmt: *mut ff::AVFormatContext,
    avio: *mut ff::AVIOContext,
    _file: Option<Box<FileIo>>,
    state: Box<CallbackState>,
    pub info: InputInfo,
}

impl Drop for Input {
    fn drop(&mut self) {
        // SAFETY: fmt is NULL or a context opened by avformat_open_input; with
        // AVFMT_FLAG_CUSTOM_IO it does not free our AVIOContext.
        unsafe { ff::avformat_close_input(&mut self.fmt) };
        if !self.avio.is_null() {
            // SAFETY: avio was created by avio_alloc_context and is no longer
            // referenced by any format context. Its buffer may have been
            // replaced by FFmpeg, so the current one is freed, then the context.
            unsafe {
                ff::av_freep(ptr::addr_of_mut!((*self.avio).buffer).cast());
                ff::avio_context_free(&mut self.avio);
            }
        }
    }
}

const IO_BUFFER: usize = 64 * 1024;

fn media_type_of(par: *const ff::AVCodecParameters) -> c_int {
    // SAFETY: par is a valid codecpar of a stream; read as c_int so an unknown
    // value can never become an invalid Rust enum.
    unsafe { ptr::addr_of!((*par).codec_type).cast::<c_int>().read() }
}

fn codec_id_of(par: *const ff::AVCodecParameters) -> c_int {
    // SAFETY: as in media_type_of.
    unsafe { ptr::addr_of!((*par).codec_id).cast::<c_int>().read() }
}

/// The bindgen output models C enums as Rust enums. A value the bindings do
/// not know (a codec id added in a newer minor release of the runtime library)
/// would be undefined behaviour the moment it exists as a Rust enum. These
/// declarations take the id as a plain integer, so it never becomes one.
mod raw {
    use std::ffi::{c_char, c_int};
    unsafe extern "C" {
        pub fn avcodec_get_name(id: c_int) -> *const c_char;
        pub fn avcodec_find_decoder(id: c_int) -> *const ffmpeg_sys_next::AVCodec;
    }
}

fn media_type_name(t: c_int) -> String {
    match t {
        x if x == ff::AVMediaType::AVMEDIA_TYPE_VIDEO as c_int => "video",
        x if x == ff::AVMediaType::AVMEDIA_TYPE_AUDIO as c_int => "audio",
        x if x == ff::AVMediaType::AVMEDIA_TYPE_SUBTITLE as c_int => "subtitle",
        x if x == ff::AVMediaType::AVMEDIA_TYPE_DATA as c_int => "data",
        x if x == ff::AVMediaType::AVMEDIA_TYPE_ATTACHMENT as c_int => "attachment",
        _ => "unknown",
    }
    .to_owned()
}

const AUDIO: c_int = ff::AVMediaType::AVMEDIA_TYPE_AUDIO as c_int;

impl Input {
    pub fn open(path: &Path, opts: &OpenOptions) -> Result<Input, FfError> {
        let ld = opts.lockdown;
        let mut input = Input {
            fmt: ptr::null_mut(),
            avio: ptr::null_mut(),
            _file: None,
            state: Box::new(CallbackState {
                deny: ld.deny_io_open,
                default_io_open: None,
                attempts: Mutex::new(Vec::new()),
            }),
            info: InputInfo {
                format: String::new(),
                streams: Vec::new(),
                chosen: 0,
                best_stream: -1,
                unused_options: Vec::new(),
            },
        };

        // SAFETY: plain allocation; NULL is checked.
        input.fmt = unsafe { ff::avformat_alloc_context() };
        if input.fmt.is_null() {
            return Err(FfError {
                during: "avformat_alloc_context",
                code: ff::AVERROR(libc::ENOMEM),
            });
        }

        let url: CString = if ld.custom_io {
            let file = File::open(path).map_err(|e| FfError {
                during: "open file",
                code: ff::AVERROR(e.raw_os_error().unwrap_or(libc::EIO)),
            })?;
            let size = file.metadata().map(|m| m.len()).unwrap_or(0);
            let mut boxed = Box::new(FileIo {
                file,
                size,
                panic_on_read: opts.inject_read_panic,
            });
            let opaque: *mut FileIo = &mut *boxed;
            input._file = Some(boxed);
            // SAFETY: av_malloc result is checked; ownership passes to the
            // AVIOContext, which Input::drop frees.
            unsafe {
                let buf = ff::av_malloc(IO_BUFFER).cast::<u8>();
                if buf.is_null() {
                    return Err(FfError {
                        during: "av_malloc",
                        code: ff::AVERROR(libc::ENOMEM),
                    });
                }
                input.avio = ff::avio_alloc_context(
                    buf,
                    IO_BUFFER as c_int,
                    0,
                    opaque.cast(),
                    Some(io_read),
                    None,
                    Some(io_seek),
                );
                if input.avio.is_null() {
                    ff::av_free(buf.cast());
                    return Err(FfError {
                        during: "avio_alloc_context",
                        code: ff::AVERROR(libc::ENOMEM),
                    });
                }
                (*input.fmt).pb = input.avio;
                (*input.fmt).flags |= ff::AVFMT_FLAG_CUSTOM_IO;
            }
            // No file name reaches FFmpeg: it probes by content only and has no
            // path to resolve relative references against.
            CString::new(opts.url_hint.clone().unwrap_or_default()).map_err(|_| FfError {
                during: "url hint",
                code: ff::AVERROR(libc::EINVAL),
            })?
        } else {
            CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| FfError {
                during: "path",
                code: ff::AVERROR(libc::EINVAL),
            })?
        };

        // SAFETY: fmt is a freshly allocated context. The state box outlives it
        // (Input::drop closes fmt before the box is dropped).
        unsafe {
            input.state.default_io_open = (*input.fmt).io_open;
            (*input.fmt).opaque = ptr::addr_of!(*input.state).cast_mut().cast();
            (*input.fmt).io_open = Some(io_open_cb);
        }

        let mut dict = Dict::default();
        if ld.protocol_whitelist {
            dict.set(c"protocol_whitelist", c"file")?;
        }
        if ld.format_whitelist {
            let w = CString::new(FORMAT_WHITELIST).map_err(|_| FfError {
                during: "whitelist",
                code: -1,
            })?;
            dict.set(c"format_whitelist", &w)?;
        }
        if let Some(cw) = &opts.codec_whitelist {
            let w = CString::new(cw.as_str()).map_err(|_| FfError {
                during: "whitelist",
                code: -1,
            })?;
            dict.set(c"codec_whitelist", &w)?;
        }

        // SAFETY: fmt is valid; on failure FFmpeg frees it and writes NULL, and
        // Input::drop still frees our AVIOContext. dict is consumed/refilled
        // with the options FFmpeg did not use.
        let ret = unsafe {
            ff::avformat_open_input(&mut input.fmt, url.as_ptr(), ptr::null(), &mut dict.0)
        };
        check("avformat_open_input", ret)?;
        input.info.unused_options = dict.keys();

        // SAFETY: fmt is open; streams has nb_streams valid entries.
        let streams = unsafe { stream_slice(input.fmt) };
        if opts.discard_before_probe {
            for &st in streams {
                // SAFETY: st is a valid stream of an open context.
                unsafe {
                    if media_type_of((*st).codecpar) != AUDIO {
                        (*st).discard = ff::AVDiscard::AVDISCARD_ALL;
                    }
                }
            }
        }

        // SAFETY: fmt is open; NULL options are allowed.
        check("avformat_find_stream_info", unsafe {
            ff::avformat_find_stream_info(input.fmt, ptr::null_mut())
        })?;

        // SAFETY: fmt is open; the stream list may have grown during probing.
        let streams = unsafe { stream_slice(input.fmt) };
        let mut infos = Vec::new();
        for (i, &st) in streams.iter().enumerate() {
            // SAFETY: valid stream and codecpar; codec ids are read as c_int and
            // only passed back to FFmpeg.
            let (t, codec, disp) = unsafe {
                let par = (*st).codecpar;
                let name = raw::avcodec_get_name(codec_id_of(par));
                (media_type_of(par), cstr_or(name, "?"), (*st).disposition)
            };
            infos.push(StreamInfo {
                index: i,
                media_type: media_type_name(t),
                codec,
                default: disp & ff::AV_DISPOSITION_DEFAULT != 0,
                attached_pic: disp & ff::AV_DISPOSITION_ATTACHED_PIC != 0,
            });
        }

        // SAFETY: fmt is open; we do not ask for a decoder.
        let best = unsafe {
            ff::av_find_best_stream(
                input.fmt,
                ff::AVMediaType::AVMEDIA_TYPE_AUDIO,
                -1,
                -1,
                ptr::null_mut(),
                0,
            )
        };
        let audio: Vec<&StreamInfo> = infos.iter().filter(|s| s.media_type == "audio").collect();
        // Choice order: (a user's per-file choice would come first) the audio
        // stream marked default when there are several, otherwise FFmpeg's best.
        let chosen = if audio.len() > 1 {
            audio.iter().find(|s| s.default).map(|s| s.index)
        } else {
            None
        }
        .or_else(|| usize::try_from(best).ok())
        .ok_or(FfError {
            during: "select audio stream",
            code: best,
        })?;

        for (i, &st) in streams.iter().enumerate() {
            // SAFETY: valid stream of an open context.
            unsafe {
                (*st).discard = if i == chosen {
                    ff::AVDiscard::AVDISCARD_DEFAULT
                } else {
                    ff::AVDiscard::AVDISCARD_ALL
                };
            }
        }

        // SAFETY: iformat is set after a successful open.
        input.info.format = cstr_or(unsafe { (*(*input.fmt).iformat).name }, "?");
        input.info.streams = infos;
        input.info.chosen = chosen;
        input.info.best_stream = best;
        Ok(input)
    }

    /// URLs FFmpeg tried to open besides the main input.
    pub fn nested_open_attempts(&self) -> Vec<String> {
        self.state
            .attempts
            .lock()
            .map(|a| a.clone())
            .unwrap_or_default()
    }
}

/// Opens only far enough to see what FFmpeg tries; used by the lock-down
/// check, where `Input::open` failing is the expected result.
pub fn try_open(path: &Path, opts: &OpenOptions) -> (Result<Input, FfError>, Vec<String>) {
    // The attempts live in the Input's state; on failure that state is dropped
    // with it, so collect them through a thread-local side channel instead.
    let res = Input::open(path, opts);
    let attempts = match &res {
        Ok(i) => i.nested_open_attempts(),
        Err(_) => LAST_ATTEMPTS.with(|l| l.take()),
    };
    (res, attempts)
}

thread_local! {
    static LAST_ATTEMPTS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Drop for CallbackState {
    fn drop(&mut self) {
        let a = self.attempts.lock().map(|a| a.clone()).unwrap_or_default();
        LAST_ATTEMPTS.with(|l| *l.borrow_mut() = a);
    }
}

/// # Safety
/// `fmt` must be an open format context; the slice must not outlive it or
/// survive a call that may add streams.
unsafe fn stream_slice<'a>(fmt: *mut ff::AVFormatContext) -> &'a [*mut ff::AVStream] {
    // SAFETY: guaranteed by the caller.
    unsafe {
        let n = (*fmt).nb_streams as usize;
        if n == 0 || (*fmt).streams.is_null() {
            &[]
        } else {
            std::slice::from_raw_parts((*fmt).streams, n)
        }
    }
}

#[derive(Default)]
struct Dict(*mut ff::AVDictionary);

impl Dict {
    fn set(&mut self, k: &CStr, v: &CStr) -> Result<(), FfError> {
        // SAFETY: both strings are NUL-terminated; FFmpeg copies them.
        check("av_dict_set", unsafe {
            ff::av_dict_set(&mut self.0, k.as_ptr(), v.as_ptr(), 0)
        })
        .map(|_| ())
    }
    fn keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut e: *const ff::AVDictionaryEntry = ptr::null();
        loop {
            // SAFETY: iterating a valid (or NULL) dictionary with the documented
            // empty-key/ignore-suffix idiom.
            e = unsafe { ff::av_dict_get(self.0, c"".as_ptr(), e, ff::AV_DICT_IGNORE_SUFFIX) };
            if e.is_null() {
                break;
            }
            // SAFETY: entries have NUL-terminated keys.
            out.push(cstr_or(unsafe { (*e).key }, "?"));
        }
        out
    }
}

impl Drop for Dict {
    fn drop(&mut self) {
        // SAFETY: frees a dictionary we own (NULL is fine).
        unsafe { ff::av_dict_free(&mut self.0) };
    }
}

// ---------------------------------------------------------------------------
// Decoding

#[derive(Clone, Debug, Default)]
pub struct DecoderOptions {
    /// Set `pkt_timebase` from the stream (required for exact trimming).
    pub set_pkt_timebase: bool,
    /// Force a decoder by name, e.g. "libopus" instead of the native "opus".
    pub force: Option<String>,
}

pub struct Decoder {
    ctx: *mut ff::AVCodecContext,
    pub name: String,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: ctx is NULL or allocated by avcodec_alloc_context3.
        unsafe { ff::avcodec_free_context(&mut self.ctx) };
    }
}

impl Decoder {
    pub fn open(input: &Input, opts: &DecoderOptions) -> Result<Decoder, FfError> {
        // SAFETY: input is open and `chosen` is a valid stream index.
        let st = unsafe { *(*input.fmt).streams.add(input.info.chosen) };
        // SAFETY: codecpar is valid; codec_id read as c_int and only handed to FFmpeg.
        let codec = unsafe {
            match &opts.force {
                Some(n) => {
                    let n = CString::new(n.as_str()).map_err(|_| FfError {
                        during: "decoder name",
                        code: -1,
                    })?;
                    ff::avcodec_find_decoder_by_name(n.as_ptr())
                }
                None => raw::avcodec_find_decoder(codec_id_of((*st).codecpar)),
            }
        };
        if codec.is_null() {
            return Err(FfError {
                during: "avcodec_find_decoder",
                code: ff::AVERROR_DECODER_NOT_FOUND,
            });
        }
        // SAFETY: codec is a valid, static AVCodec.
        let (ctx, name) = unsafe { (ff::avcodec_alloc_context3(codec), (*codec).name) };
        let dec = Decoder {
            ctx,
            name: cstr_or(name, "?"),
        };
        if dec.ctx.is_null() {
            return Err(FfError {
                during: "avcodec_alloc_context3",
                code: ff::AVERROR(libc::ENOMEM),
            });
        }
        // SAFETY: ctx and codecpar are valid.
        check("avcodec_parameters_to_context", unsafe {
            ff::avcodec_parameters_to_context(dec.ctx, (*st).codecpar)
        })?;
        if opts.set_pkt_timebase {
            // SAFETY: ctx is valid and not opened yet.
            unsafe { (*dec.ctx).pkt_timebase = (*st).time_base };
        }
        let ctx = dec.ctx;
        let ret = probe::as_bridge(|| {
            // SAFETY: ctx is an allocated, unopened context and codec matches it.
            unsafe { ff::avcodec_open2(ctx, codec, ptr::null_mut()) }
        });
        check("avcodec_open2", ret)?;
        Ok(dec)
    }
}

/// Decoded audio as interleaved samples, normalised to [-1, 1) by a power of
/// two so integer formats stay exact.
#[derive(Clone, Debug, Default)]
pub struct Pcm {
    pub rate: u32,
    pub channels: usize,
    pub sample_fmt: String,
    pub samples: Vec<f64>,
    /// Packets returned by av_read_frame, per stream index.
    pub packets_per_stream: Vec<u64>,
    /// First decode error, if decoding stopped early.
    pub error: Option<String>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
    }
}

struct Packet(*mut ff::AVPacket);
impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: allocated by av_packet_alloc (or NULL).
        unsafe { ff::av_packet_free(&mut self.0) };
    }
}
struct Frame(*mut ff::AVFrame);
impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: allocated by av_frame_alloc (or NULL).
        unsafe { ff::av_frame_free(&mut self.0) };
    }
}

pub fn decode_all(input: &mut Input, dec: &mut Decoder) -> Result<Pcm, FfError> {
    // SAFETY: plain allocations, checked below.
    let pkt = Packet(unsafe { ff::av_packet_alloc() });
    // SAFETY: as above.
    let frame = Frame(unsafe { ff::av_frame_alloc() });
    if pkt.0.is_null() || frame.0.is_null() {
        return Err(FfError {
            during: "alloc",
            code: ff::AVERROR(libc::ENOMEM),
        });
    }
    let mut pcm = Pcm {
        packets_per_stream: vec![0; input.info.streams.len()],
        ..Pcm::default()
    };
    let chosen = input.info.chosen as c_int;
    loop {
        // SAFETY: fmt is open, pkt is allocated and unreferenced.
        let ret = unsafe { ff::av_read_frame(input.fmt, pkt.0) };
        if ret == ff::AVERROR_EOF {
            break;
        }
        if ret < 0 {
            pcm.error = Some(
                FfError {
                    during: "av_read_frame",
                    code: ret,
                }
                .to_string(),
            );
            break;
        }
        // SAFETY: pkt holds a packet just returned by av_read_frame.
        let idx = unsafe { (*pkt.0).stream_index };
        if let Some(c) = pcm.packets_per_stream.get_mut(idx as usize) {
            *c += 1;
        }
        if idx == chosen {
            // SAFETY: ctx is an open decoder; pkt is a valid packet.
            let r = unsafe { ff::avcodec_send_packet(dec.ctx, pkt.0) };
            // SAFETY: drop our reference; the decoder made its own.
            unsafe { ff::av_packet_unref(pkt.0) };
            if r < 0 && r != ff::AVERROR(libc::EAGAIN) {
                pcm.error = Some(
                    FfError {
                        during: "avcodec_send_packet",
                        code: r,
                    }
                    .to_string(),
                );
                break;
            }
            receive(dec, &frame, &mut pcm)?;
        } else {
            // SAFETY: as above.
            unsafe { ff::av_packet_unref(pkt.0) };
        }
    }
    // SAFETY: a NULL packet starts draining.
    unsafe { ff::avcodec_send_packet(dec.ctx, ptr::null()) };
    receive(dec, &frame, &mut pcm)?;
    Ok(pcm)
}

fn receive(dec: &mut Decoder, frame: &Frame, pcm: &mut Pcm) -> Result<(), FfError> {
    loop {
        // SAFETY: ctx is open and frame is allocated.
        let r = unsafe { ff::avcodec_receive_frame(dec.ctx, frame.0) };
        if r == ff::AVERROR(libc::EAGAIN) || r == ff::AVERROR_EOF {
            return Ok(());
        }
        check("avcodec_receive_frame", r)?;
        // SAFETY: frame holds a decoded audio frame.
        unsafe { append_frame(&*frame.0, pcm) };
        // SAFETY: release the frame's buffers before reuse.
        unsafe { ff::av_frame_unref(frame.0) };
    }
}

/// # Safety
/// `f` must be a decoded audio frame with valid data pointers.
unsafe fn append_frame(f: &ff::AVFrame, pcm: &mut Pcm) {
    use ff::AVSampleFormat as S;
    let n = f.nb_samples.max(0) as usize;
    let ch = f.ch_layout.nb_channels.max(0) as usize;
    pcm.rate = f.sample_rate as u32;
    pcm.channels = ch;
    let fmt = f.format;
    let base = |s: S| s as c_int;
    let (packed, planar_of) = match fmt {
        x if x == base(S::AV_SAMPLE_FMT_U8) || x == base(S::AV_SAMPLE_FMT_U8P) => {
            ("u8", x == base(S::AV_SAMPLE_FMT_U8P))
        }
        x if x == base(S::AV_SAMPLE_FMT_S16) || x == base(S::AV_SAMPLE_FMT_S16P) => {
            ("s16", x == base(S::AV_SAMPLE_FMT_S16P))
        }
        x if x == base(S::AV_SAMPLE_FMT_S32) || x == base(S::AV_SAMPLE_FMT_S32P) => {
            ("s32", x == base(S::AV_SAMPLE_FMT_S32P))
        }
        x if x == base(S::AV_SAMPLE_FMT_FLT) || x == base(S::AV_SAMPLE_FMT_FLTP) => {
            ("flt", x == base(S::AV_SAMPLE_FMT_FLTP))
        }
        x if x == base(S::AV_SAMPLE_FMT_DBL) || x == base(S::AV_SAMPLE_FMT_DBLP) => {
            ("dbl", x == base(S::AV_SAMPLE_FMT_DBLP))
        }
        _ => {
            pcm.error = Some(format!("unsupported sample format {fmt}"));
            return;
        }
    };
    pcm.sample_fmt = format!("{packed}{}", if planar_of { "p" } else { "" });
    let read = |plane: usize, i: usize| -> f64 {
        // SAFETY: FFmpeg guarantees extended_data[c] (planar) or extended_data[0]
        // (packed) holds n (or n * ch) samples of the reported format, and the
        // loop below stays within those bounds.
        unsafe {
            let p = *f.extended_data.add(plane);
            match packed {
                "u8" => (f64::from(*p.add(i)) - 128.0) / 128.0,
                "s16" => f64::from(*p.cast::<i16>().add(i)) / 32768.0,
                "s32" => f64::from(*p.cast::<i32>().add(i)) / 2147483648.0,
                "flt" => f64::from(*p.cast::<f32>().add(i)),
                _ => *p.cast::<f64>().add(i),
            }
        }
    };
    pcm.samples.reserve(n * ch);
    for s in 0..n {
        for c in 0..ch {
            pcm.samples.push(if planar_of {
                read(c, s)
            } else {
                read(0, s * ch + c)
            });
        }
    }
}

/// Opens, decodes and closes in one go.
pub fn decode_file(
    path: &Path,
    open: &OpenOptions,
    dec: &DecoderOptions,
) -> Result<(InputInfo, String, Pcm), FfError> {
    let mut input = Input::open(path, open)?;
    let mut d = Decoder::open(&input, dec)?;
    let pcm = decode_all(&mut input, &mut d)?;
    Ok((input.info.clone(), d.name.clone(), pcm))
}
