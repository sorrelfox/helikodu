// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding spike: a minimal FFmpeg bridge on the raw bindings, and the
//! measurements that decide whether that approach is good enough.
//!
//! `cargo run --release -- [all|formats|video|panic|gapless|opus-gain|lockdown]`
//! prints Markdown tables. Test media is written to target/media.

mod bridge;
mod media;
mod ogg;
mod probe;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bridge::{DecoderOptions, Lockdown, OpenOptions};
use media::{CASES, Case, Source};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let what = args.first().map(String::as_str).unwrap_or("all");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/media");

    // Keep FFmpeg's own log quiet unless asked; refusals are reported below.
    let level = if std::env::var_os("SPIKE_FFMPEG_LOG").is_some() {
        32
    } else {
        -8
    };
    // SAFETY: sets a global integer; no other thread uses FFmpeg yet.
    unsafe { ffmpeg_sys_next::av_log_set_level(level) };

    println!("# Decoding spike results\n");
    println!("- {}", bridge::ffmpeg_version());
    println!("- avcodec_open2 probe active: {}\n", probe::is_active());
    if !probe::is_active() {
        eprintln!(
            "the avcodec_open2 probe is not interposed; the video proof would be meaningless"
        );
        std::process::exit(2);
    }

    if let Err(e) = media::create_all(&dir) {
        eprintln!("creating the test media failed: {e}");
        std::process::exit(1);
    }

    let mut failures = 0;
    let all = what == "all";
    if all || what == "formats" {
        failures += check_formats(&dir);
    }
    if all || what == "video" {
        failures += check_video(&dir);
    }
    if all || what == "panic" {
        failures += check_panic(&dir);
    }
    if all || what == "gapless" {
        failures += check_gapless(&dir);
    }
    if all || what == "lockdown" {
        failures += check_lockdown(&dir);
    }
    if all || what == "opus-gain" {
        failures += check_opus_gain(&dir);
    }
    println!("\n**Failed checks: {failures}**");
    if failures > 0 {
        std::process::exit(1);
    }
}

fn pass(ok: bool) -> &'static str {
    if ok { "pass" } else { "FAIL" }
}

fn case(file: &str) -> &'static Case {
    CASES
        .iter()
        .find(|c| c.file == file)
        .unwrap_or_else(|| panic!("no case {file}"))
}

fn exact_dec() -> DecoderOptions {
    DecoderOptions {
        set_pkt_timebase: true,
        force: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Formats, including the audio of video files

fn check_formats(dir: &Path) -> usize {
    println!("## 1. Decoding to PCM, all offline locks on\n");
    println!(
        "| File | Demuxer | Streams | Chosen (best) | Decoder | Format | Rate/ch | Frames | Packets of other streams returned | Decoders opened by bridge / avcodec_open2 calls inside FFmpeg (succeeded) / video decoders opened | Result |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let files = [
        "mp3_lame_cbr.mp3",
        "aac_fdk.m4a",
        "opus_opusenc.opus",
        "flac_16.flac",
        "flac_24.flac",
        "wavpack.wv",
        "vorbis.ogg",
        "mp3_cover.mp3",
        "flac_cover.flac",
        "video_mpeg4_aac.mp4",
        "video_vp8_2audio.mkv",
        "video_flac_vp9.mkv",
    ];
    let mut failures = 0;
    for f in files {
        let c = case(f);
        if !media::path_of(dir, c).exists() {
            println!("| {f} | skipped: could not be encoded on this system |");
            continue;
        }
        probe::take();
        let res = bridge::decode_file(
            &media::path_of(dir, c),
            &OpenOptions::default(),
            &exact_dec(),
        );
        let opens = probe::take();
        match res {
            Ok((info, dec, pcm)) => {
                let streams: Vec<String> = info
                    .streams
                    .iter()
                    .map(|s| {
                        format!(
                            "{}:{}{}{}",
                            s.index,
                            s.media_type,
                            if s.attached_pic { "(pic)" } else { "" },
                            format_args!(" {}", s.codec)
                        )
                    })
                    .collect();
                // Packets of other streams the demuxer still returned (the bridge drops them).
                let other: Vec<String> = pcm
                    .packets_per_stream
                    .iter()
                    .enumerate()
                    .filter(|&(i, &n)| i != info.chosen && n > 0)
                    .map(|(i, n)| format!("s{i}: {n}"))
                    .collect();
                let bridge_n = opens.iter().filter(|o| o.by_bridge && o.opened()).count();
                let inner_calls = opens.iter().filter(|o| !o.by_bridge).count();
                let inner_opened = opens.iter().filter(|o| !o.by_bridge && o.opened()).count();
                let video_opened = opens.iter().filter(|o| o.is_video() && o.opened()).count();
                // An option key FFmpeg did not recognise would silently switch a lock off.
                if !info.unused_options.is_empty() {
                    println!("unrecognised options: {:?}", info.unused_options);
                }
                let ok = pcm.error.is_none()
                    && pcm.frames() == c.source.frames()
                    && video_opened == 0
                    && bridge_n == 1
                    && info.unused_options.is_empty();
                failures += usize::from(!ok);
                println!(
                    "| {f} | {} | {} | {} ({}) | {dec} | {} | {}/{} | {} | {} | {bridge_n} / {inner_calls} ({inner_opened}) / {video_opened} | {} |",
                    info.format,
                    streams.join(", "),
                    info.chosen,
                    info.best_stream,
                    pcm.sample_fmt,
                    pcm.rate,
                    pcm.channels,
                    pcm.frames(),
                    if other.is_empty() {
                        "none".to_owned()
                    } else {
                        other.join(", ")
                    },
                    pass(ok)
                );
            }
            Err(e) => {
                failures += 1;
                println!("| {f} | | | | | | | | | | FAIL: {e} |");
            }
        }
    }
    println!();
    failures
}

/// A panic inside a callback must come back as an error, not abort or unwind into C.
fn check_panic(dir: &Path) -> usize {
    println!("## 1c. A panic inside an FFmpeg callback\n");
    let opts = OpenOptions {
        inject_read_panic: true,
        ..OpenOptions::default()
    };
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let res = bridge::Input::open(&media::path_of(dir, case("flac_16.flac")), &opts);
    std::panic::set_hook(prev);
    let ok = matches!(&res, Err(e) if e.code == ffmpeg_sys_next::AVERROR_EXTERNAL);
    println!(
        "The read callback panics on its first call. Result: {} → {}\n",
        match &res {
            Ok(_) => "opened (unexpected)".to_owned(),
            Err(e) => format!("`{e}`"),
        },
        pass(ok)
    );
    usize::from(!ok)
}

/// Does `avformat_find_stream_info` open video decoders, and which setting stops it?
fn check_video(dir: &Path) -> usize {
    println!("## 1b. Which settings keep FFmpeg from opening a video decoder\n");
    println!(
        "`avcodec_open2` calls made *inside* `avformat_find_stream_info`; the bridge's own call is not listed. `ok` = the call succeeded and the decoder was initialised, `refused` = it returned an error before initialising anything.\n"
    );
    println!(
        "| File | discard before probing | codec_whitelist | Internal avcodec_open2 calls | Video decoders opened | Decoded frames |"
    );
    println!("|---|---|---|---|---|---|");
    let audio_only = bridge::audio_decoder_whitelist();
    let mut failures = 0;
    for f in [
        "video_mpeg4_aac.mp4",
        "video_vp8_2audio.mkv",
        "video_flac_vp9.mkv",
        "mp3_cover.mp3",
        "flac_cover.flac",
    ] {
        let c = case(f);
        if !media::path_of(dir, c).exists() {
            println!("| {f} | skipped: could not be encoded on this system |");
            continue;
        }
        for (discard, wl) in [
            (false, None),
            (true, None),
            (false, Some(&audio_only)),
            (true, Some(&audio_only)),
        ] {
            let opts = OpenOptions {
                discard_before_probe: discard,
                codec_whitelist: wl.cloned(),
                ..OpenOptions::default()
            };
            probe::take();
            let res = bridge::decode_file(&media::path_of(dir, c), &opts, &exact_dec());
            let opens: Vec<_> = probe::take().into_iter().filter(|o| !o.by_bridge).collect();
            let video = opens.iter().filter(|o| o.is_video() && o.opened()).count();
            let list: Vec<String> = opens
                .iter()
                .map(|o| {
                    format!(
                        "{} ({}, {})",
                        o.codec,
                        if o.is_video() { "video" } else { "audio" },
                        if o.opened() { "ok" } else { "refused" }
                    )
                })
                .collect();
            let frames = match &res {
                Ok((_, _, pcm)) => pcm.frames().to_string(),
                Err(e) => format!("error: {e}"),
            };
            // The configuration the bridge uses (discard + audio-only whitelist) must open none.
            if discard && wl.is_some() && (video > 0 || res.is_err()) {
                failures += 1;
            }
            println!(
                "| {f} | {} | {} | {} | {video} | {frames} |",
                if discard { "yes" } else { "no" },
                if wl.is_some() {
                    "all audio decoders"
                } else {
                    "unset"
                },
                if list.is_empty() {
                    "none".to_owned()
                } else {
                    list.join(", ")
                },
            );
        }
    }
    println!();
    failures
}

// ---------------------------------------------------------------------------
// 2. Gapless

/// Best lag (decoded relative to reference) on channel 0, by normalised
/// cross-correlation over a window in the middle of the signal.
fn best_lag(reference: &[f64], decoded: &[f64], channels: usize) -> (i64, f64) {
    let r: Vec<f64> = reference.iter().step_by(channels).copied().collect();
    let d: Vec<f64> = decoded.iter().step_by(channels).copied().collect();
    let (start, len, max_lag) = (20_000usize, 30_000usize, 3_000i64);
    let mut best = (0i64, f64::MIN);
    for lag in -max_lag..=max_lag {
        let (mut num, mut e1, mut e2) = (0.0, 0.0, 0.0);
        for i in start..start + len {
            let j = i as i64 + lag;
            if j < 0 || j as usize >= d.len() || i >= r.len() {
                continue;
            }
            let (a, b) = (r[i], d[j as usize]);
            num += a * b;
            e1 += a * a;
            e2 += b * b;
        }
        let c = if e1 > 0.0 && e2 > 0.0 {
            num / (e1 * e2).sqrt()
        } else {
            0.0
        };
        if c > best.1 {
            best = (lag, c);
        }
    }
    best
}

fn check_gapless(dir: &Path) -> usize {
    println!("## 2. Gapless: decoded length against the source\n");
    println!(
        "Source: stereo, {} frames at 44.1 kHz or {} frames at 48 kHz. 'Lag' is the offset of the decoded signal found by cross-correlation (0 = start trimmed exactly).\n",
        media::FRAMES_44K,
        media::FRAMES_48K
    );
    println!(
        "| Case | Expected frames | Decoded frames | Difference | Lag / correlation | Bit-exact | Without pkt_timebase | Expected | Result |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut failures = 0;
    for c in CASES {
        if !media::path_of(dir, c).exists() {
            println!(
                "| {} | skipped: could not be encoded on this system |",
                c.name
            );
            continue;
        }
        let path = media::path_of(dir, c);
        let reference = media::reference(c.source);
        let expected = c.source.frames();
        let res = bridge::decode_file(&path, &OpenOptions::default(), &exact_dec());
        let no_tb = bridge::decode_file(
            &path,
            &OpenOptions::default(),
            &DecoderOptions {
                set_pkt_timebase: false,
                force: None,
            },
        )
        .map(|(_, _, p)| p.frames().to_string())
        .unwrap_or_else(|e| e.to_string());
        let (pcm, dec) = match res {
            Ok((_, d, p)) => (p, d),
            Err(e) => {
                failures += 1;
                println!("| {} | {expected} | error: {e} | | | | | | FAIL |", c.name);
                continue;
            }
        };
        let frames = pcm.frames();
        let diff = frames as i64 - expected as i64;
        let (lag, corr) = best_lag(&reference, &pcm.samples, pcm.channels.max(1));
        let bit_exact = c.lossless.then(|| pcm.samples == reference);
        let exact =
            diff == 0 && lag == 0 && pcm.rate == c.source.rate() && bit_exact.unwrap_or(true);
        // A known gap passes when it behaves as documented (not exact).
        let ok = if c.expect_exact { exact } else { true };
        failures += usize::from(!ok);
        println!(
            "| {} ({dec}) | {expected} | {frames} | {diff:+} | {lag} / {corr:.4} | {} | {no_tb} | {} | {} |",
            c.name,
            match bit_exact {
                Some(true) => "yes",
                Some(false) => "NO",
                None => "n/a",
            },
            if c.expect_exact { "exact" } else { "known gap" },
            if c.expect_exact {
                pass(ok)
            } else if exact {
                "exact anyway"
            } else {
                "not exact, as documented"
            }
        );
    }
    println!();
    failures
}

// ---------------------------------------------------------------------------
// 4. Opus output gain

fn rms(x: &[f64]) -> f64 {
    (x.iter().map(|v| v * v).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn check_opus_gain(dir: &Path) -> usize {
    println!("## 4. Opus header output gain\n");
    let base = media::path_of(dir, case("opus_opusenc.opus"));
    let mut variants: Vec<(String, PathBuf, f64)> =
        vec![("header gain 0 dB (opusenc)".into(), base.clone(), 0.0)];
    for (q78, tag) in [(1536i16, "p6"), (-896i16, "m3_5")] {
        let p = dir.join(format!("opus_gain_{tag}.opus"));
        let result = std::fs::read(&base)
            .map_err(|e| e.to_string())
            .and_then(|mut bytes| {
                ogg::set_opus_output_gain(&mut bytes, q78)?;
                std::fs::write(&p, bytes).map_err(|e| e.to_string())
            });
        if let Err(e) = result {
            println!("could not patch the header: {e}\n");
            return 1;
        }
        let db = f64::from(q78) / 256.0;
        variants.push((format!("header gain {db:+} dB, Ogg"), p.clone(), db));
        if q78 > 0 {
            let mka = dir.join(format!("opus_gain_{tag}.mka"));
            if !mka.exists()
                && let Err(e) = media::ffmpeg(&["-i", &media::s(&p), "-c", "copy", &media::s(&mka)])
            {
                println!("remux failed: {e}\n");
                return 1;
            }
            variants.push((
                format!("header gain {db:+} dB, remuxed to Matroska"),
                mka,
                db,
            ));
        }
    }
    // An R128 tag is a playback hint for the player, not for the decoder.
    let r128 = dir.join("opus_r128_tag.opus");
    if !r128.exists() {
        let status = std::process::Command::new("opusenc")
            .args(["--quiet", "--comment", "R128_TRACK_GAIN=-2560"])
            .arg(dir.join(Source::S48k16.wav_name()))
            .arg(&r128)
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            println!("opusenc failed\n");
            return 1;
        }
    }
    variants.push((
        "R128_TRACK_GAIN=-2560 tag (-10 dB), header 0 dB".into(),
        r128,
        0.0,
    ));

    println!(
        "Level change against the 0 dB file, measured as RMS ratio. 'Applied once' means the measured change equals the header value; twice would double it.\n"
    );
    println!(
        "| Input | Decoder | Header gain | Measured change | Max deviation from base x gain | Result |"
    );
    println!("|---|---|---|---|---|---|");
    let mut failures = 0;
    for decoder in ["opus", "libopus"] {
        let dopts = DecoderOptions {
            set_pkt_timebase: true,
            force: Some(decoder.into()),
        };
        let base_pcm = match bridge::decode_file(&base, &OpenOptions::default(), &dopts) {
            Ok((_, _, p)) => p,
            Err(e) => {
                println!("| base | {decoder} | | error: {e} | | FAIL |");
                failures += 1;
                continue;
            }
        };
        let base_rms = rms(&base_pcm.samples);
        for (name, path, db) in &variants {
            match bridge::decode_file(path, &OpenOptions::default(), &dopts) {
                Ok((_, _, p)) => {
                    let measured = 20.0 * (rms(&p.samples) / base_rms).log10();
                    let g = 10f64.powf(db / 20.0);
                    let dev = p
                        .samples
                        .iter()
                        .zip(&base_pcm.samples)
                        .map(|(a, b)| (a - b * g).abs())
                        .fold(0.0, f64::max);
                    let ok = (measured - db).abs() < 0.01 && p.frames() == base_pcm.frames();
                    failures += usize::from(!ok);
                    println!(
                        "| {name} | {decoder} | {db:+.2} dB | {measured:+.4} dB | {dev:.2e} | {} |",
                        pass(ok)
                    );
                }
                Err(e) => {
                    failures += 1;
                    println!("| {name} | {decoder} | | error: {e} | | FAIL |");
                }
            }
        }
    }
    println!();
    failures
}

// ---------------------------------------------------------------------------
// 3. Offline lock-down

/// A local TCP listener standing in for "the network": every connection to it
/// is counted, so an open that reaches out is visible, not just assumed.
fn canary() -> std::io::Result<(u16, Arc<AtomicUsize>)> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    let port = l.local_addr()?.port();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = Arc::clone(&hits);
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            h.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    Ok((port, hits))
}

fn check_lockdown(dir: &Path) -> usize {
    println!("## 3. Offline lock-down\n");
    let ld_dir = dir.join("lockdown");
    let (port, hits) = match std::fs::create_dir_all(&ld_dir).and_then(|_| canary()) {
        Ok(x) => x,
        Err(e) => {
            println!("setup failed: {e}\n");
            return 1;
        }
    };
    let seg = ld_dir.join("seg0.ts");
    if !seg.exists()
        && let Err(e) = media::ffmpeg(&[
            "-i",
            &media::s(&dir.join(Source::S44k16.wav_name())),
            "-c:a",
            "aac",
            "-f",
            "mpegts",
            &media::s(&seg),
        ])
    {
        println!("creating the segment failed: {e}\n");
        return 1;
    }
    let url = format!("http://127.0.0.1:{port}/seg0.ts");
    let hls = |target: &str| {
        format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:5.0,\n{target}\n#EXT-X-ENDLIST\n"
        )
    };
    let attacks: Vec<(&str, String)> = vec![
        ("hls_remote.m3u8", hls(&url)),
        ("hls_local.m3u8", hls("seg0.ts")),
        (
            "concat_remote.ffconcat",
            format!("ffconcat version 1.0\nfile '{url}'\n"),
        ),
        (
            "concat_local.ffconcat",
            "ffconcat version 1.0\nfile 'seg0.ts'\n".into(),
        ),
        (
            "plain_remote.m3u",
            format!("#EXTM3U\n#EXTINF:5,Remote\n{url}\n"),
        ),
    ];
    for (name, body) in &attacks {
        if let Err(e) = std::fs::write(ld_dir.join(name), body) {
            println!("write failed: {e}\n");
            return 1;
        }
    }
    // Relative references resolve against the working directory when FFmpeg
    // has no file name (custom I/O). Run from the attack folder so the local
    // side file is reachable in every configuration and the test is fair.
    let _ = std::env::set_current_dir(&ld_dir);

    // The third field shows FFmpeg the file name despite custom I/O. The real
    // bridge never does that; here it gets the hls demuxer past its extension
    // check so that the io_open lock is tested on its own.
    let custom = Lockdown {
        custom_io: true,
        ..Lockdown::NONE
    };
    let configs: [(&str, Lockdown, bool); 8] = [
        ("none (plain path open)", Lockdown::NONE, false),
        ("custom I/O only", custom, false),
        ("custom I/O, file name shown", custom, true),
        (
            "protocol_whitelist only",
            Lockdown {
                protocol_whitelist: true,
                ..Lockdown::NONE
            },
            false,
        ),
        (
            "format_whitelist only",
            Lockdown {
                format_whitelist: true,
                ..Lockdown::NONE
            },
            false,
        ),
        (
            "custom I/O + deny-all io_open",
            Lockdown {
                deny_io_open: true,
                ..custom
            },
            false,
        ),
        (
            "custom I/O, file name shown + deny-all io_open",
            Lockdown {
                deny_io_open: true,
                ..custom
            },
            true,
        ),
        ("all four", Lockdown::ALL, false),
    ];

    // Control: the canary must see a connection when FFmpeg is simply asked to
    // open an http URL, or a zero below would prove nothing.
    println!("Canary listener on 127.0.0.1:{port}.\n");
    println!("| Control | Outcome | Canary connections |");
    println!("|---|---|---|");
    let mut failures = 0;
    for (label, ld) in [
        ("http URL, no locks", Lockdown::NONE),
        (
            "http URL, protocol_whitelist=file",
            Lockdown {
                protocol_whitelist: true,
                ..Lockdown::NONE
            },
        ),
    ] {
        let before = hits.load(Ordering::SeqCst);
        let opts = OpenOptions {
            lockdown: ld,
            ..OpenOptions::default()
        };
        let res = bridge::Input::open(Path::new(&url), &opts);
        let conns = hits.load(Ordering::SeqCst) - before;
        if ld == Lockdown::NONE && conns == 0 {
            failures += 1;
        }
        println!(
            "| {label} | {} | {conns} |",
            match res {
                Ok(_) => "opened".to_owned(),
                Err(e) => format!("refused: {e}"),
            }
        );
    }
    println!();

    println!("Every row opens the attack file and, if that succeeds, decodes it.\n");
    println!(
        "| Attack file | Locks | Outcome | Nested open attempts seen by io_open | Canary connections |"
    );
    println!("|---|---|---|---|---|");
    for (name, _) in &attacks {
        for (label, ld, hint) in configs {
            let before = hits.load(Ordering::SeqCst);
            let opts = OpenOptions {
                lockdown: ld,
                url_hint: hint.then(|| (*name).to_owned()),
                ..OpenOptions::default()
            };
            let (res, mut attempts) = bridge::try_open(&ld_dir.join(name), &opts);
            let outcome = match res {
                Ok(mut input) => {
                    let o = match bridge::Decoder::open(&input, &exact_dec()) {
                        Ok(mut d) => match bridge::decode_all(&mut input, &mut d) {
                            Ok(p) => format!(
                                "opened as {}, decoded {} frames{}",
                                input.info.format,
                                p.frames(),
                                p.error.map(|e| format!(" then {e}")).unwrap_or_default()
                            ),
                            Err(e) => format!("opened as {}, decode error {e}", input.info.format),
                        },
                        Err(e) => format!("opened as {}, no decoder: {e}", input.info.format),
                    };
                    // Segments may be opened lazily while decoding.
                    attempts = input.nested_open_attempts();
                    o
                }
                Err(e) => format!("refused: {e}"),
            };
            let conns = hits.load(Ordering::SeqCst) - before;
            // The main input itself is one io_open call in a plain path open.
            let nested: Vec<&String> = attempts.iter().filter(|a| !a.ends_with(name)).collect();
            let safe = conns == 0
                && (outcome.starts_with("refused") || outcome.contains("decoded 0 frames"));
            if ld == Lockdown::ALL && !safe {
                failures += 1;
            }
            println!(
                "| {name} | {label} | {outcome} | {} | {conns}{} |",
                if nested.is_empty() {
                    "none".to_owned()
                } else {
                    nested
                        .iter()
                        .map(|s| format!("`{s}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                if ld == Lockdown::ALL {
                    format!(" ({})", pass(safe))
                } else {
                    String::new()
                }
            );
        }
    }
    println!();
    failures
}
