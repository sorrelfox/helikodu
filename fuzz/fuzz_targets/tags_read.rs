// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads arbitrary bytes as a media file the way a scan worker does: the quick
//! hash, the tags (through the tag library, and FFmpeg when it gives up), the
//! embedded CUE sheet, chapters and the embedded cover. The first byte picks the
//! file's extension, which decides whether chapters are read. Any panic that
//! reaches the target is a bug, and so is a report that does not travel through
//! the worker protocol unchanged.
//!
//! The readers catch panics inside the tag library and refuse the file instead, so
//! this target lets caught panics pass: it prints them (they are bugs of the
//! library, worth reporting upstream) instead of aborting at once. A panic that is
//! not caught still ends the run.

#![no_main]

use std::io::Write;
use std::path::PathBuf;
use std::sync::Once;

use helikodu_meta::worker::{Message, Request, RequestOptions, Response, read_file};
use libfuzzer_sys::fuzz_target;

const EXTENSIONS: [&str; 12] = [
    "flac", "mp3", "ogg", "opus", "m4a", "wav", "aiff", "ape", "wv", "mka", "dsf", "mpc",
];

fn folder() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helikodu-fuzz-tags-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fuzz_target!(|data: &[u8]| {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| eprintln!("panic: {info}")));
    });
    helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
    let Some((&choice, body)) = data.split_first() else {
        return;
    };
    let extension = EXTENSIONS[usize::from(choice) % EXTENSIONS.len()];
    let path = folder().join(format!("track.{extension}"));
    {
        let Ok(mut file) = std::fs::File::create(&path) else {
            return;
        };
        if file.write_all(body).is_err() {
            return;
        }
    }
    let request = Request {
        id: 7,
        path: path.clone(),
        options: RequestOptions {
            legacy_charset: (choice & 0x80 != 0).then(|| "windows-1251".to_owned()),
            ..RequestOptions::default()
        },
    };
    let result = read_file(&request);
    let _ = std::fs::remove_file(&path);
    let response = Message::Response(Box::new(Response { id: 7, result }));
    let bytes = response.encode();
    let back = Message::decode(&bytes).expect("a report decodes");
    assert_eq!(back.encode(), bytes);
});
