# helikodu-ffmpeg-bridge

A thin safe layer over FFmpeg: custom I/O locked to local files, probing by content, audio stream selection, decoding, seeking, metadata, chapters and attached pictures, and later the encoders used by the converter. This is one of the few crates allowed to contain `unsafe`; every `unsafe` block carries a `SAFETY:` comment.

- **May depend on:** `model`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.
- **FFmpeg:** the raw `ffmpeg-sys-next` bindings, linked dynamically to the system FFmpeg (7.1 or newer) through pkg-config. Only libavformat, libavcodec and libavutil are used; libavdevice is never linked.

## Using it

```rust
use helikodu_ffmpeg_bridge::{MediaFile, PictureLimits, StreamChoice};
use helikodu_model::audio::MediaTime;

let file = MediaFile::open(path)?;                 // the path is only used to open the file
let title = file.tags().get("title");
let covers = file.attached_pictures(&PictureLimits::default());
let mut decoder = file.into_decoder(StreamChoice::Auto)?;
decoder.seek(MediaTime::from_millis(30_000))?;     // lands on the exact frame
while let Some(block) = decoder.read()? {
    // block.spec(), block.position(), block.samples()
}
```

| Type | What it is |
|---|---|
| `MediaFile` | An opened, probed file: `streams()`, `audio_streams()`, `tags()`, `tags_with_stream()`, `chapters()`, `attached_pictures()`, `duration()`, `choose_stream()`, `into_decoder()` |
| `StreamInfo`, `StreamKind`, `Disposition` | One stream: kind, codec name, disposition flags, language, title, tags, rate, channels, significant bits, duration |
| `StreamChoice` | `Auto`, or `Index(n)` for a user's per-file choice |
| `AudioDecoder` | Decodes the chosen stream: `read()`, `seek()`, `seek_precision()`, `spec()`, `duration()`, `position()` |
| `PcmBuffer`, `PcmSamples` | One block of interleaved samples with its `AudioSpec` and the position of its first frame |
| `Tags`, `Chapter`, `AttachedPicture`, `PictureLimits` | Metadata as FFmpeg reads it; pictures are bytes only, never decoded |
| `Error`, `AvError`, `AvErrorKind` | The error type; FFmpeg's `AVERROR` codes are mapped to kinds |

### Output format

Samples come out interleaved, in the native byte order, in the format FFmpeg decodes to: 8-, 16- and 32-bit integers stay integers and keep their exact values; floating-point formats become `f32`. A 24-bit source arrives as 32-bit words with the sample in the top 24 bits (`StreamInfo::bits_per_sample` says 24). Channel layouts are converted to the model's WAVE-order `ChannelLayout`; layouts with positions the model does not name have only a channel count.

## Offline locks

A local file can still make FFmpeg reach out: an HLS playlist or a concat script names other files or URLs, and a QuickTime file can carry external data references. Every open applies five locks together, because none of them is enough alone:

1. **Custom I/O and no file name.** The bridge opens the file itself (`std::fs::File`, the path as bytes) and gives FFmpeg an `AVIOContext`. FFmpeg probes by content only and has no path to resolve references against.
2. **`protocol_whitelist=file`.** Custom I/O removes the limit FFmpeg applies when it opens a path itself, so this one is always set with it.
3. **A deny-all `io_open`.** Every nested open fails with a permission error.
4. **`format_whitelist`.** Only demuxers that read one self-contained file and recognise it by its content. `hls`, `dash`, `concat`, `imf`, `vobsub`, `dvdvideo`, the image-sequence demuxers, `sdp`, `rtsp` and device inputs are left out. This is the only lock that stops `concat`, which opens its files through a context of its own. Names are FFmpeg's demuxer names, which are not always the format's usual name: the MPEG program stream (`.mpg`, `.vob`) is `mpeg`, NIST SPHERE is `nistsphere`, Westwood AUD is `wsaud`, Yamaha SMAF is `mmf`; a unit test checks that the running FFmpeg knows these names.
5. **`codec_whitelist` = every audio decoder** of the running FFmpeg build. `avformat_find_stream_info` opens decoders even for discarded streams; with the whitelist, a video or cover-art decoder is refused before it is initialised.

FFmpeg silently ignores an option it does not know, so after opening the bridge checks that the option dictionary has nothing left over and that the format context carries each whitelist; otherwise the open fails.

After probing, one audio stream is chosen (the explicit index; otherwise the stream marked default when only some are; otherwise FFmpeg's best audio stream). Every other stream is discarded in the demuxer, and packets that still arrive from them (a cover picture queued while opening, packets buffered while probing) are dropped by stream index.

## Format notes

- **Raw, headerless streams are not supported as files**: Bluetooth SBC, aptX, aptX HD, DFPWM, G.722, G.723.1, G.726, G.729 and raw Codec 2 (`sbc`, `aptx`, `aptx_hd`, `dfpwm`, `g722`, `g723_1`, `g726`, `g726le`, `g729`, `codec2raw`). Their bytes carry nothing to recognise them by; FFmpeg picks these demuxers only by file name extension. The bridge never gives FFmpeg a file name, and names never decide what a file is, so such files are refused as unknown data. They are left off the format whitelist so it lists only demuxers that can ever be chosen. Where a codec also comes in a container with a header, that plays (G.726 in WAV, Codec 2 in its `.c2` format).
- Demuxers added for single-file formats with a header: `ircam`, `nistsphere`, `avr`, `hcom`, `mmf`, `adx`, `wsaud`, `lc3`, `ilbc`, `codec2`, and the program stream `mpeg` (audio only; video streams are discarded like in any other video file). Each was checked to read only its own file: none opens another file, URL or format context.
- Westwood AUD reports a short read at the end of the file as invalid data; the bridge treats an error at the end of the input as the end of the stream.
- HCOM and Westwood AUD cannot seek; a seek decodes from the start, which these short files allow.

## Gapless playback and seeking

- FFmpeg trims encoder delay and padding where the file says how much (MP3 LAME/Info tags, MP4 edit lists, Ogg Opus pre-skip and end granule, Matroska `CodecDelay`). The bridge keeps packet side data and sets `pkt_timebase` so it can.
- **MP4 end padding is trimmed by the bridge.** FFmpeg 7.1 leaves the end padding of AAC in MP4, and 9.0 that of Opus in MP4. The bridge stops at the edit-list duration, or at the sample count of an `iTunSMPB` tag, whichever is smaller.
- Positions count frames from the first frame after the encoder delay. `seek()` moves the demuxer to a point before the target (at least 4096 frames, more when the codec asks for a longer pre-roll), flushes the decoder, and drops decoded audio up to the exact target frame. If the demuxer lands after the target, it tries further back, and finally from the start: back to the first packet with a freshly opened decoder (a flush does not reset every decoder's state), counting frames as when the file was opened.
- **The first frame after a seek needs an exact position**, and time stamps do not always give it (`src/decode/anchor.rs`):
  - *Ogg FLAC*: a granule position marks the end of the last packet completed on a page, and FFmpeg has no packet-duration callback for FLAC in Ogg, so after a seek it stamps the first packet with a time up to a page late (8192 frames for a page of two 4096-frame packets). *FLAC anywhere* (Ogg, Matroska, …) is therefore anchored on its frame headers, which carry the frame or sample number (validated by the header CRC-8).
  - *Compressed blocks in WAV, Wave64 and AIFF* (MS and IMA ADPCM, GSM): after a seek FFmpeg derives the time stamp from the byte position and the header's rounded average byte rate, one or two frames off. The packet's byte position gives the exact block number instead.
  - *RealAudio 14.4* files written by FFmpeg's RealMedia muxer stamp every packet with time 0, so a seek by time cannot work and the stream seemed to end early. When the first two packets carry the same time stamp, the bridge seeks by byte: packets have a fixed size and stride, and the byte position gives the frame position.
  - *Millisecond time stamps* (Matroska, WebM, ASF, FLV) are rounded or cut to the millisecond. Where every packet holds whole codec frames of a fixed size — Opus (2.5 ms steps), WMA (512 to 2048 frames), Nellymoser (256), SWF ADPCM (4096) and codecs with a fixed `frame_size` — the position is snapped to that grid, which is exact while the time stamp is off by less than half a frame. FLAC in Matroska uses its frame headers.
  - Otherwise, when the time base is coarser than one frame and nothing anchors the position (Vorbis in Matroska or WebM, whose packet lengths vary), exactness is impossible without reading every packet from the start. The guarantee is then: **a seek lands within one time-stamp tick plus one frame** of its target (±46 frames for Vorbis at 44.1 kHz in WebM), and the reported position is the target. `AudioDecoder::seek_precision()` says how many frames that is; it is 0 everywhere else, and always 0 for lossless codecs in the corpus.
- A damaged packet is skipped; only a long run of errors stops decoding.
- The Opus header output gain is applied by FFmpeg's decoder; `R128_*` tags are not applied and are left to the engine.

Known gaps: raw ADTS AAC, MP3 files without an Info tag and MP4 files without an edit list or `iTunSMPB` carry no gapless information; `iTunSMPB` in an MP3's ID3 tag is ignored; AAC in Matroska is not trimmed at the end.

## Rules for code in this crate

- **C enums are integers.** bindgen models `AVCodecID`, `AVMediaType`, `AVSampleFormat` and others as Rust enums, and a value the build headers do not know (a newer runtime library) would be undefined behaviour. Enum fields are read through `ffi::enum_field`, and functions that take an enum are declared again in `ffi::raw` with an integer parameter.
- **No panic crosses into C.** Every callback FFmpeg calls (read, seek, `io_open`) runs inside `catch_unwind`; a panic becomes `AVERROR_EXTERNAL`.
- **Ownership.** With custom I/O, `avformat_close_input` does not free the `AVIOContext`: the format context is closed first, then the I/O buffer (FFmpeg may have replaced it) and the context are freed, also when opening fails.
- Every FFmpeg object has exactly one owner whose `Drop` frees it.

## Layout

| Folder | Contents |
|---|---|
| `src/io/` | Custom I/O and the offline locks |
| `src/demux/` | Opening and probing, the stream list, stream choice |
| `src/decode/` | The decoder, exact seek anchors, PCM conversion, MP4 end trimming |
| `src/metadata/` | Tags, chapters, attached pictures |
| `src/ffi.rs` | Enum handling, integer-typed declarations, strings, dictionaries, time stamps |

## Tests

```sh
cargo test -p helikodu-ffmpeg-bridge
```

The integration tests in `tests/bridge/` generate their media at test time: a reference signal (two chirps and noise, with frame counts that are no multiple of any codec frame size) written as WAV by the tests, then encoded with the `ffmpeg` tool, and with `lame` and `opusenc` when they are installed, into `target/tmp/ffmpeg-bridge-media/`. A test whose tool or encoder is missing prints `SKIPPED:` and passes.

They check:

- exact length and bit-exact samples for WAV and FLAC (16 and 24 bit);
- exact length and alignment (cross-correlation lag 0) for MP3 (lame and ffmpeg), Opus and Vorbis in Ogg, AAC in M4A, and Opus in MP4, which needs the bridge's own end trimming;
- that no video or cover-art decoder is ever opened: the test binary defines its own `avcodec_open2`, which libavformat's internal calls bind to, records every call and forwards it to libavcodec; the same probe runs over the video set of the generated test corpus (`cargo xtask corpus`) when it exists, and is skipped otherwise;
- the offline locks: HLS, ffconcat, M3U and SDP files pointing at `http://` URLs and at local files are refused, and a local TCP listener sees no connection;
- stream listing, dispositions and stream choice, including the audio of video files;
- seeking to exact frames (bit-exact for lossless, aligned for lossy), backwards, forwards and past the end, including FLAC in Ogg and Matroska, ADPCM in WAV, Opus in WebM and WMA, and the stated precision for Vorbis in WebM;
- tags, chapters, attached pictures with their size and count limits, non-UTF-8 file names, damaged and non-media input, and panics inside a callback (unit test).

The fuzz target `ffmpeg_bridge_open` in the top-level `fuzz/` folder feeds arbitrary bytes through opening, metadata, decoding and seeking.
