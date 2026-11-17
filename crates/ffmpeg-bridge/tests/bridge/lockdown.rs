// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files that try to make FFmpeg open URLs or other local files are refused, and
//! nothing connects anywhere.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use helikodu_ffmpeg_bridge::MediaFile;

use crate::media::{self, Source};

/// A local listener standing in for the network: every connection is counted.
fn canary() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the canary");
    let port = listener.local_addr().expect("address").port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (port, hits)
}

fn wait_for(hits: &AtomicUsize, n: usize) -> bool {
    for _ in 0..100 {
        if hits.load(Ordering::SeqCst) >= n {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn playlists_and_scripts_are_refused_and_nothing_connects() {
    let Some(segment) = media::ffmpeg(
        "lockdown-seg0.ts",
        Source::S44k16,
        &["aac"],
        &["-c:a", "aac"],
        "mpegts",
    ) else {
        return;
    };
    // The canary itself must see a connection, or the zero below proves nothing.
    let (port, hits) = canary();
    drop(TcpStream::connect(("127.0.0.1", port)).expect("connect to the canary"));
    assert!(wait_for(&hits, 1), "the canary does not count connections");
    hits.store(0, Ordering::SeqCst);

    // The segment is a valid local file: the attack works if it gets played.
    assert!(
        MediaFile::open(&segment).is_ok(),
        "the segment itself is playable"
    );

    let url = format!("http://127.0.0.1:{port}/seg0.ts");
    let local = segment.to_string_lossy().into_owned();
    let hls = |target: &str| {
        format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:5.0,\n{target}\n#EXT-X-ENDLIST\n"
        )
    };
    let attacks: Vec<(&str, String)> = vec![
        ("hls-remote.m3u8", hls(&url)),
        ("hls-local.m3u8", hls(&local)),
        ("hls-relative.m3u8", hls("lockdown-seg0.ts")),
        (
            "concat-remote.ffconcat",
            format!("ffconcat version 1.0\nfile '{url}'\n"),
        ),
        (
            "concat-local.ffconcat",
            format!("ffconcat version 1.0\nfile '{local}'\n"),
        ),
        (
            "concat-relative.ffconcat",
            "ffconcat version 1.0\nfile 'lockdown-seg0.ts'\n".to_owned(),
        ),
        (
            "plain-remote.m3u",
            format!("#EXTM3U\n#EXTINF:5,Remote\n{url}\n"),
        ),
        (
            "plain-local.m3u",
            format!("#EXTM3U\n#EXTINF:5,Local\n{local}\n"),
        ),
        (
            "sdp-remote.sdp",
            format!(
                "v=0\no=- 0 0 IN IP4 127.0.0.1\ns=x\nc=IN IP4 127.0.0.1\nt=0 0\nm=audio {port} RTP/AVP 0\n"
            ),
        ),
    ];
    for (name, body) in &attacks {
        let path: PathBuf = media::dir().join(name);
        std::fs::write(&path, body).expect("write the attack file");
        let result = MediaFile::open(&path);
        assert!(result.is_err(), "{name} was opened: {result:?}");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "FFmpeg connected to the network"
    );
}
