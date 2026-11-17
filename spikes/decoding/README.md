# Spike: decoding through FFmpeg

## Question

Can the player decode through FFmpeg using only the raw `ffmpeg-sys-next` bindings plus a small safe layer of its own, and does that give us what the engine needs?

1. MP3, AAC, Opus, FLAC and the audio of MKV/MP4 files with a video stream decode to PCM, and no video decoder is ever opened.
2. FFmpeg's automatic encoder-delay and padding trimming gives sample-exact lengths (bit-exact for lossless).
3. Only the local file can be opened: custom I/O, `protocol_whitelist`, `format_whitelist` and a deny-all `io_open` together refuse playlists, HLS and concat scripts that point at URLs or other files.
4. The Opus header output gain is applied exactly once.
5. The same code works on every FFmpeg version we support (7.1 up to the latest release).

## Answer in short

Yes, with three corrections to the original design, all found by measurement:

- **Discarding a stream is not enough to keep its decoder closed.** `avformat_find_stream_info` opens video decoders (and the PNG decoder for cover art) even for streams marked `AVDISCARD_ALL`. Setting the format context's `codec_whitelist` to the list of audio decoders makes those calls fail before the decoder is initialised. That is now part of the lock-down.
- **The deny-all `io_open` does not cover the `concat` demuxer.** concat opens its files through a fresh format context of its own, which does not use our callback. Only the format whitelist stops it, so the four locks really are all needed.
- **Custom I/O on its own weakens FFmpeg's default protection.** When FFmpeg opens a local file by path it limits nested opens to `file,crypto,data`. With custom I/O there is no path, so there is no such limit: an HLS playlist then makes a real HTTP connection. `protocol_whitelist=file` must always come with custom I/O.

Gapless trimming is sample-exact for every format that carries the information, lossless decoding is bit-exact, and the Opus header gain is applied exactly once by both Opus decoders. One FFmpeg gap was found: the end padding of Opus in MP4 is not trimmed.

FFmpeg 7.1.5 was tested as well; there, AAC in MP4 is not trimmed at the end, which the bridge has to handle itself (see [Compatibility](#5-compatibility-ffmpeg-71)).

## How to run

```sh
cargo run --release -- all     # or: formats, video, panic, gapless, lockdown, opus-gain
SPIKE_FFMPEG_LOG=1 cargo run --release -- lockdown   # also show FFmpeg's own messages
```

The program writes a reference signal as WAV files, encodes it with `ffmpeg`, `lame`, `opusenc`, `flac` and `wavpack` into `target/media/` (nothing is committed), runs every check and prints Markdown tables. It exits non-zero if a check fails. A case whose encoder is missing is skipped, not failed. No sound is played. The whole run takes about 4 seconds.

Files:

| File | Purpose |
|---|---|
| `src/bridge.rs` | The safe layer: custom I/O, the four locks, stream choice, decoder, PCM conversion |
| `src/probe.rs` | Records every `avcodec_open2` call in the process, including FFmpeg's internal ones |
| `src/media.rs` | Reference signal and test media |
| `src/ogg.rs` | Rewrites the output gain in an Ogg Opus header (with CRC) |
| `src/main.rs` | The checks |
| `build.rs` | Exports the `avcodec_open2` wrapper so the dynamic linker binds FFmpeg's calls to it |
| `scripts/build-ffmpeg-7.1.sh` | Builds a minimal FFmpeg 7.1.5 under `target/` without root (an alternative to a Debian 13 container; not needed so far) |

## Method

**Environment.** Fedora, FFmpeg 9.0.2 (libavcodec 63.1.102, libavformat 63.1.102, libavutil 61.1.102), `ffmpeg-sys-next` 9.1.1 with only the `avformat` feature (pulls in avcodec and avutil), Rust 1.100.

**Reference signal.** Stereo, computed by the program: a rising chirp (100 Hz to 8 kHz) on the left, a falling chirp (6 kHz to 150 Hz) on the right, plus a little noise. The lengths, 220,757 frames at 44.1 kHz and 240,263 frames at 48 kHz, are not multiples of any codec frame size. Written as 16-bit and 24-bit WAV.

**Lengths and alignment.** Each encoded file is decoded through the bridge with all locks on. The decoded frame count is compared with the source. For lossy codecs, a cross-correlation over ±3,000 samples finds the offset between the decoded and the original signal: a lag of 0 means the start was trimmed exactly. The chirps make the correlation peak unambiguous. Lossless output is compared sample by sample.

**Proving no video decoder is opened.** The spike defines its own `avcodec_open2`, exports it from the executable and forwards to libavcodec's through `dlsym(RTLD_NEXT)`. Because the executable comes first in the dynamic symbol lookup, libavformat's internal calls bind to the wrapper too. The wrapper records the codec, its media type and the return value. At start-up the program checks that the wrapper really is the definition the process uses, so the proof cannot pass silently.

**Offline locks.** A TCP listener on 127.0.0.1 stands in for the network and counts connections. A control shows it does see FFmpeg connect when asked to open an `http://` URL. Each attack file is opened under eight lock combinations, from none to all four, and decoded if it opens. The `io_open` callback logs every nested open, and refuses it when the deny lock is on.

**Opus gain.** The output gain field of an `opusenc` file is rewritten to +6 dB and −3.5 dB (CRC recomputed; `opusinfo` reads "Playback gain: 6 dB"). The +6 dB file is also remuxed to Matroska. Each is decoded with FFmpeg's native `opus` decoder and with `libopus`, and the level change against the 0 dB file is measured.

## Results

On FFmpeg 9.0.2 every check passes; the program reports 0 failed checks. The tables below are condensed from its output.

### 1. Decoding to PCM, and no video decoder

All locks on. "FFmpeg-internal opens" are the `avcodec_open2` calls made inside `avformat_find_stream_info`. The number in brackets is how many of those succeeded.

| File | Streams | Chosen | Decoder, format | Frames | FFmpeg-internal opens (succeeded) | Video decoders opened | Result |
|---|---|---|---|---|---|---|---|
| MP3 (lame CBR) | audio | 0 | mp3float, fltp | 220,757 | 1 (1) | 0 | pass |
| AAC in M4A (libfdk_aac) | audio | 0 | aac, fltp | 220,757 | 1 (1) | 0 | pass |
| Opus in Ogg (opusenc) | audio | 0 | opus, fltp | 240,263 | 1 (1) | 0 | pass |
| FLAC 16-bit | audio | 0 | flac, s16 | 220,757 | 1 (1) | 0 | pass |
| FLAC 24-bit 48 kHz | audio | 0 | flac, s32 | 240,263 | 1 (1) | 0 | pass |
| WavPack | audio | 0 | wavpack, s16p | 220,757 | 1 (1) | 0 | pass |
| Vorbis in Ogg | audio | 0 | vorbis, fltp | 220,757 | 1 (1) | 0 | pass |
| MP3 with cover art | audio, PNG picture | 0 | mp3float, fltp | 220,757 | 3 (1) | 0 | pass |
| FLAC with cover art | audio, PNG picture | 0 | flac, s16 | 220,757 | 3 (1) | 0 | pass |
| MP4: MPEG-4 video + AAC | video, audio | 1 | aac, fltp | 220,757 | 2 (1) | 0 | pass |
| MKV: VP8 + AAC + Opus (default) | video, audio, audio | 2 | opus, fltp | 240,263 | 3 (2) | 0 | pass |
| MKV: FLAC first, then VP9 | audio, video | 0 | flac, s16 | 220,757 | 2 (1) | 0 | pass |

In the MKV with two audio tracks, the stream marked default (Opus) is chosen; `av_find_best_stream` picks the same one here.

Which setting keeps the video decoders closed (internal `avcodec_open2` calls; "ok" = the decoder was initialised):

| File | No settings | `AVDISCARD_ALL` before probing | `codec_whitelist` = all audio decoders |
|---|---|---|---|
| MP4: MPEG-4 + AAC | mpeg4 **ok**, aac ok | mpeg4 **ok**, aac ok | mpeg4 refused, aac ok |
| MKV: VP8 + AAC + Opus | vp8 **ok**, aac ok, opus ok | vp8 **ok**, … | vp8 refused, … |
| MKV: FLAC + VP9 | flac ok, vp9 **ok** | flac ok, vp9 **ok** | flac ok, vp9 refused |
| MP3 with cover | mp3float ok, png **ok** | mp3float ok, png **ok** | mp3float ok, png refused (×2) |
| FLAC with cover | flac ok, png **ok** | flac ok, png **ok** | flac ok, png refused (×2) |

With the whitelist, FFmpeg still *calls* `avcodec_open2` for the video stream, but the call returns `EINVAL` ("Codec (vp8) not on whitelist") before the decoder is set up. Decoding results are identical with and without it. FFmpeg's own messages confirm this (`SPIKE_FFMPEG_LOG=1`).

Packets of other streams still come out of `av_read_frame`. One is the cover picture, which libavformat queues during `avformat_open_input`, before any discard flag can be set. The other 236 are the AAC packets of the second audio track in the MKV, read and buffered during probing, before the bridge had chosen the Opus track. The bridge drops anything that is not from its chosen stream, so neither case matters. But it means filtering by stream index is required, not optional.

A panic injected into the custom read callback comes back as `AVERROR_EXTERNAL` ("Generic error in an external library") from `avformat_open_input`. It neither aborts nor unwinds into C.

### 2. Gapless

| Case | Decoded frames | Difference | Lag | Bit-exact | Result |
|---|---|---|---|---|---|
| MP3, lame −V2 (LAME tag) | 220,757 | 0 | 0 | – | pass |
| MP3, lame CBR 192 (Info tag) | 220,757 | 0 | 0 | – | pass |
| MP3, ffmpeg libmp3lame | 220,757 | 0 | 0 | – | pass |
| MP3 with cover art | 220,757 | 0 | 0 | – | pass |
| AAC-LC in M4A, ffmpeg `aac` | 220,757 | 0 | 0 | – | pass |
| AAC-LC in M4A, libfdk_aac | 220,757 | 0 | 0 | – | pass |
| AAC in MP4 with video | 220,757 | 0 | 0 | – | pass |
| Opus in Ogg, opusenc | 240,263 | 0 | 0 | – | pass |
| Opus in Ogg, ffmpeg libopus | 240,263 | 0 | 0 | – | pass |
| Opus in Matroska | 240,263 | 0 | 0 | – | pass |
| Opus in WebM | 240,263 | 0 | 0 | – | pass |
| Opus in MKV with video (default track of two) | 240,263 | 0 | 0 | – | pass |
| Vorbis in Ogg | 220,757 | 0 | 0 | – | pass |
| FLAC 16-bit (`flac -8`) | 220,757 | 0 | 0 | yes | pass |
| FLAC 24-bit 48 kHz | 240,263 | 0 | 0 | yes | pass |
| FLAC in Matroska | 220,757 | 0 | 0 | yes | pass |
| FLAC with cover art | 220,757 | 0 | 0 | yes | pass |
| FLAC in MKV with video | 220,757 | 0 | 0 | yes | pass |
| WavPack | 220,757 | 0 | 0 | yes | pass |
| *Known gaps, not exact as expected:* | | | | | |
| MP3, `lame -t` (no info tag) | 222,336 | +1,579 | 1,105 | – | as documented |
| AAC-LC in M4A without edit list | 221,781 | +1,024 | 1,024 | – | as documented |
| AAC-LC as raw ADTS | 222,208 | +1,451 | 1,024 | – | as documented |
| **Opus in MP4** | 240,648 | **+385** | 0 | – | **FFmpeg gap, see problems** |

The untrimmed cases show exactly the expected delays: 1,105 = 576 encoder + 529 decoder samples for MP3, and 1,024 priming samples for AAC.

Setting `pkt_timebase` on the decoder made no difference to any count on FFmpeg 9.0.2; every case was also decoded without it. The bridge still sets it, because FFmpeg documents it as required for exact trimming and older versions may depend on it.

### 3. Offline lock-down

Control: opening `http://127.0.0.1:<port>/seg0.ts` with no locks makes **1** connection (the canary works). With `protocol_whitelist=file` it makes 0 ("Invalid argument").

Canary connections and outcome per attack. *Refused* means `avformat_open_input` failed; *decoded* means the attack worked (the referenced file was played).

| Locks | HLS → http:// | HLS → local `seg0.ts` | ffconcat → http:// | ffconcat → local `seg0.ts` | plain M3U → http:// |
|---|---|---|---|---|---|
| none (open by path) | refused¹, 0 conn. | **decoded** | refused², 0 | **decoded** | refused³, 0 |
| custom I/O only | refused⁴, 0 | refused⁴ | refused², 0 | **decoded** | refused³, 0 |
| custom I/O, file name shown to FFmpeg | refused, **1 connection** | **decoded** | refused², 0 | **decoded** | refused³, 0 |
| `protocol_whitelist=file` only | refused¹, 0 | **decoded** | refused², 0 | **decoded** | refused³, 0 |
| `format_whitelist` only | refused⁵, 0 | refused⁵ | refused⁵, 0 | refused⁵ | refused³, 0 |
| custom I/O + deny-all `io_open` | refused⁴, 0 | refused⁴ | refused², 0 | **decoded** | refused³, 0 |
| custom I/O, file name shown + deny-all `io_open` | refused⁶, 0 | refused⁶ | refused², 0 | **decoded** | refused³, 0 |
| **all four** | refused, 0 | refused | refused, 0 | refused | refused, 0 |

1. When FFmpeg opens a local file by path it limits nested opens to `file,crypto,data`: "Protocol 'http' not on whitelist".
2. The concat demuxer's default `safe=1`: "Unsafe file name".
3. No demuxer recognises a plain M3U playlist.
4. With no file name, FFmpeg 9 does not detect HLS: "Not detecting m3u8/hls with non standard extension".
5. "Format not on whitelist".
6. Our `io_open` logged the segment URL and refused it.

What this shows about each lock:

- **Custom I/O** removes FFmpeg's implicit `file,crypto,data` limit. With a name that lets HLS through, the playlist reached the network. Never use it without `protocol_whitelist`.
- **Deny-all `io_open`** works for demuxers that open through it (HLS, footnote 6). But the concat demuxer played its local side file under every lock combination except those with the format whitelist. It opens that file in its own format context, which does not inherit our callback. Our callback saw no attempt at all.
- **`format_whitelist`** is the lock that stopped every attack here on its own. It is still not enough alone: whitelisted demuxers such as `mov` (QuickTime external data references) open other files through `io_open`.
- **All four together** refused everything, with zero connections and zero nested opens.

### 4. Opus output gain

Measured level change against the 0 dB file (RMS ratio); same frame count in every case.

| Input | Header gain | `opus` (native) | `libopus` |
|---|---|---|---|
| opusenc file | 0 dB | +0.0000 dB | +0.0000 dB |
| Header patched, Ogg | +6 dB | +6.0000 dB | +6.0000 dB |
| Header patched, remuxed to Matroska | +6 dB | +6.0000 dB | +6.0000 dB |
| Header patched, Ogg | −3.5 dB | −3.5000 dB | −3.5000 dB |
| `R128_TRACK_GAIN=-2560` tag, header 0 dB | 0 dB | +0.0000 dB | +0.0000 dB |

Each output equals the 0 dB output times the gain to within 4.5·10⁻⁸ (native) or 4.6·10⁻⁵ (libopus). The header gain is applied **once, inside the decoder**, also when the Opus stream comes from Matroska. The `R128_*` tags are **not** applied by FFmpeg; they are the player's job. So the engine must never apply the header gain itself, and must add R128 gains on top as normal ReplayGain.

### 5. Compatibility: FFmpeg 7.1

The checks were run a second time against FFmpeg **7.1.5** (Debian 13's system build: libavcodec 61.19.101, libavformat 61.7.103), inside a Debian 13 container. The spike was built against 7.1's headers and decoded the same media files that were encoded on the 9.0.2 machine.

```sh
podman run --rm --userns=keep-id --security-opt label=disable \
  -v "$PWD:/src" -v "$HOME/.cargo:/cargo:ro" -v "$HOME/.rustup:/rustup:ro" \
  -e CARGO_HOME=/cargo -e RUSTUP_HOME=/rustup -e CARGO_TARGET_DIR=/src/target/ff71 \
  -e PATH=/rustup/toolchains/1.100.0-x86_64-unknown-linux-gnu/bin:/usr/bin:/bin \
  -w /src <debian-13-image-with-ffmpeg-dev> \
  sh -c 'cargo build --release --offline && target/ff71/release/decoding-spike all'
```

| Check | 7.1.5 result |
|---|---|
| Builds against the 7.1 headers | Yes, no source change |
| Video decoders opened with `codec_whitelist` set | 0 (without it: 1 per video stream, as on 9.0.2) |
| Offline lock-down, all locks on | Every attack refused; 0 canary connections, 0 nested opens |
| Opus header gain | Applied exactly once (+6.0000 dB, −3.5000 dB; libopus +5.9994 dB) |
| Gapless: MP3, Opus in Ogg/Matroska/WebM, Vorbis, FLAC, WavPack | Same as 9.0.2 |
| Gapless: **AAC in MP4/M4A** | **Fails: 427 frames too long.** The start delay is removed, but the end padding from the edit list is not trimmed. This affects both encoders and AAC inside an MP4 with video. 9.0.2 trims it correctly |

**Failed checks on 7.1.5: 5**, all from the AAC-in-MP4 end-trimming difference.

Consequence: the engine cannot rely on FFmpeg alone for the end of AAC in MP4 on older versions. The bridge must read the edit list duration (or the iTunSMPB tag when there is no edit list) itself and stop at that sample count. That works on every version, and on 9.x it simply agrees with FFmpeg. The same mechanism fixes Opus in MP4 on 9.0.2, which leaves 385 frames of padding.

How CI should cover versions:

- A job matrix over FFmpeg versions, each building the bridge against that version's headers and running the same checks: **7.1** (the Debian 13 container), the newest **8.x**, and the newest **9.x** (Fedora or Arch container).
- The media step only needs *some* `ffmpeg`, `lame`, `flac`, `opusenc` and `wavpack`. It runs once in a fixed environment, so the version under test only decodes.
- `ffmpeg-sys-next` detects the version at build time through pkg-config and builds its bindings from those headers, so a build against 7.1 also catches API differences at compile time.
- `scripts/build-ffmpeg-7.1.sh` (a user-space build of 7.1.5) is kept as an alternative for machines without such a container; it was not needed.

## Problems found

1. **`avformat_find_stream_info` opens decoders for discarded streams**, including video and cover-art decoders. Fixed with `codec_whitelist` (built at run time from every audio decoder in the FFmpeg build). 7.1.5 behaves the same, and the whitelist fixes it there too.
2. **The concat demuxer bypasses `io_open`** (FFmpeg 9.0.2). The format whitelist covers it.
3. **Custom I/O drops FFmpeg's implicit protocol limit.** `protocol_whitelist` covers it.
4. **Opus in MP4 is not end-trimmed.** The file's edit list is correct (duration 240,263, media time 312). The FFmpeg command-line tool also decodes 240,648 frames, so this is FFmpeg, not the bridge. The start is exact (lag 0); the 385 extra samples are encoder padding at the end. This is a gapless defect for Opus-in-MP4 only, which is rare. The bridge can fix it by limiting output to the edit-list duration, or it can be reported upstream.
5. **Raw ADTS AAC, MP3 without an info tag, and M4A without an edit list** carry no gapless information. As expected.
6. **Packets of non-chosen streams still arrive**: the cover-art packet and packets buffered during probing. The bridge must filter by stream index.
7. **Rust enums from bindgen are a soundness trap.** The bindings model `AVCodecID`, `AVMediaType` and similar as Rust enums. A value the bindings do not know, for example a codec id added in a minor FFmpeg release when the runtime library is newer than the build headers (a Flatpak runtime update), is undefined behaviour as soon as it exists as a Rust enum. The spike reads such fields as raw integers and declares `avcodec_get_name` and `avcodec_find_decoder` with an integer parameter.
8. **HE-AAC was not tested.** Fedora's `libfdk_aac` is the patent-free build and refuses the HE profile, and FFmpeg's own encoder has no HE mode. The corpus needs an HE-AAC file from elsewhere.
9. **Not attacked here:** QuickTime external data references (off by default, and they go through `io_open`), SDP/RTP inputs (excluded by the format whitelist), and image-sequence patterns (impossible without a file name). A later offline audit should add crafted files for these.

## Recommendation

**Accept FFmpeg through `ffmpeg-sys-next` plus our own safe bridge.** The raw layer gave direct access to everything the checks needed: custom `AVIOContext` callbacks, `io_open`, the three whitelists, `discard`, `pkt_timebase`, stream dispositions and `av_codec_iterate`. The working bridge, including the offline locks, is about 800 lines with every `unsafe` block justified. Nothing here argued for a high-level binding: the two decisive fixes (`codec_whitelist`, and handling bindgen enums as integers) are exactly the kind of detail a high-level wrapper hides or gets wrong.

The checks also ran against FFmpeg 7.1.5 without any source change. The one behavioural difference, AAC in MP4 not trimmed at the end, is handled by trimming to the container's own duration in the bridge (see Compatibility). No API breakage was found, so the fallback (`ffmpeg-the-third`) is not needed.

## Lessons for the bridge crate

- **Lock-down is five settings, not four:** custom I/O, `protocol_whitelist=file`, `format_whitelist` (allowlist of single-file demuxers), deny-all `io_open`, and `codec_whitelist` = all audio decoders. Apply them all on every open, and keep the attack matrix above as an integration test, including the canary listener and the `avcodec_open2` probe.
- **Pass FFmpeg no file name.** Content-only probing kept HLS from even being detected. Relative references then have nothing to resolve against.
- **Check the leftover option dictionary** after `avformat_open_input`. A misspelt whitelist key is silently ignored, which would turn a lock off without notice.
- **Set `AVDISCARD_ALL` on every non-chosen stream**, and still filter packets by stream index.
- **Stream choice:** the user's per-file choice, then the stream marked default, then `av_find_best_stream`.
- **Trim the end yourself for MP4.** Read the edit-list duration (or `iTunSMPB` when there is no edit list) and stop decoding at that sample count. FFmpeg 7.1 does not trim AAC end padding in MP4, and 9.0.2 does not trim Opus end padding in MP4.
- **Set `pkt_timebase`** and never strip packet side data, even though 9.0.2 trimmed correctly without the time base.
- **Do not apply the Opus header gain** in the engine; do apply `R128_*` tags as ReplayGain.
- **Treat every C enum read from FFmpeg as an integer.** Never let an unknown value become a Rust enum. Declare the few functions that take one with an integer parameter, or convert through a checked lookup.
- **Callbacks:** each one wraps its body in `catch_unwind` and maps a panic to `AVERROR_EXTERNAL`. A read callback never returns 0. The seek callback answers `AVSEEK_SIZE` and masks `AVSEEK_FORCE`.
- **Ownership:** with custom I/O, `avformat_close_input` does not free the `AVIOContext`. Free `avio->buffer` (FFmpeg may have replaced it) and then the context, after closing the format context. The same applies when `avformat_open_input` fails.
- **Integer PCM stays exact** when normalised by a power of two. 24-bit FLAC arrives as `s32` with the sample in the top 24 bits.
- **Decoder test corpus:** keep the reference-signal approach (chirps, odd lengths, cross-correlation for the start offset). It found the Opus-in-MP4 defect at once and turns "gapless" into numbers.
