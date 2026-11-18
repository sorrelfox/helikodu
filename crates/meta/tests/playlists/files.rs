// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading files shaped like those found in the wild.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use helikodu_meta::playlists::{
    EncodingSource, EntryLocation, ParseOptions, PlaylistError, PlaylistKind, PlaylistLimits,
    PlaylistWarningKind, read_playlist,
};

use crate::{folder, write};

fn local(path: impl AsRef<Path>) -> EntryLocation {
    EntryLocation::Local(path.as_ref().to_owned())
}

fn cp1251(text: &str) -> Vec<u8> {
    let (bytes, _, errors) = encoding_rs::WINDOWS_1251.encode(text);
    assert!(!errors);
    bytes.into_owned()
}

#[test]
fn windows_m3u_with_bom_and_crlf() {
    let dir = folder("windows");
    let file = "\u{feff}#EXTM3U\r\n\
        #EXTINF:245,Daft Punk - One More Time\r\n\
        Daft Punk\\Discovery\\01 One More Time.mp3\r\n\
        #EXTINF:320,Daft Punk - Aerodynamic\r\n\
        C:\\Users\\me\\Music\\Daft Punk\\Discovery\\02 Aerodynamic.mp3\r\n\
        \\\\NAS\\music\\Daft Punk\\03.mp3\r\n\
        ..\\Other\\04.flac\r\n";
    let path = write(&dir, "Discovery.m3u", file);
    let parsed = read_playlist(&path, &ParseOptions::default()).unwrap();
    assert_eq!(parsed.kind, PlaylistKind::M3u);
    assert_eq!(parsed.encoding.source, EncodingSource::ByteOrderMark);
    let entries = &parsed.playlist.entries;
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries[0].location,
        local(dir.join("Daft Punk/Discovery/01 One More Time.mp3"))
    );
    assert_eq!(entries[0].hints.artist.as_deref(), Some("Daft Punk"));
    assert_eq!(entries[0].hints.title.as_deref(), Some("One More Time"));
    assert_eq!(entries[0].hints.duration, Some(Duration::from_secs(245)));
    assert_eq!(
        entries[1].location,
        EntryLocation::Foreign(
            b"C:/Users/me/Music/Daft Punk/Discovery/02 Aerodynamic.mp3".to_vec()
        )
    );
    assert_eq!(
        entries[2].location,
        EntryLocation::Foreign(b"//NAS/music/Daft Punk/03.mp3".to_vec())
    );
    assert_eq!(
        entries[3].location,
        local(dir.parent().unwrap().join("Other/04.flac"))
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn cp1251_m3u_keeps_both_names() {
    let dir = folder("cp1251");
    let mut file = cp1251(
        "#EXTM3U\r\n#EXTINF:281,Кино - Группа крови\r\nКино\\Группа крови\\01 Группа крови.mp3\r\n",
    );
    file.extend_from_slice(&cp1251(
        "#EXTINF:240,Кино - Закрой за мной дверь\r\nКино\\02 Закрой за мной дверь.mp3\r\n",
    ));
    let path = write(&dir, "Кино.m3u", &file);
    let parsed = read_playlist(&path, &ParseOptions::default()).unwrap();
    assert_eq!(parsed.encoding.name(), "windows-1251");
    assert_eq!(parsed.encoding.source, EncodingSource::Detected);
    let entry = &parsed.playlist.entries[0];
    assert_eq!(entry.hints.artist.as_deref(), Some("Кино"));
    assert_eq!(entry.hints.title.as_deref(), Some("Группа крови"));
    // The decoded name, as the files are named today…
    assert_eq!(
        entry.location,
        local(dir.join("Кино/Группа крови/01 Группа крови.mp3"))
    );
    // …and the original bytes, as an old collection may still name them.
    let mut undecoded = dir.as_os_str().as_bytes().to_vec();
    undecoded.extend_from_slice(b"/");
    undecoded.extend_from_slice(&cp1251("Кино/Группа крови/01 Группа крови.mp3"));
    assert_eq!(
        entry.undecoded_path.as_deref(),
        Some(Path::new(OsStr::from_bytes(&undecoded)))
    );
    assert_eq!(entry.local_paths().count(), 2);

    // The charset the user chose replaces the guess.
    let koi8 = read_playlist(
        &path,
        &ParseOptions {
            legacy_charset: Some("koi8-r".into()),
            ..ParseOptions::default()
        },
    )
    .unwrap();
    assert_eq!(koi8.encoding.name(), "KOI8-R");
    assert_eq!(koi8.encoding.source, EncodingSource::Chosen);

    // An M3U8 file is UTF-8 whatever it holds: the bytes are the name.
    let path = write(&dir, "Кино.m3u8", &file);
    let parsed = read_playlist(&path, &ParseOptions::default()).unwrap();
    assert_eq!(parsed.encoding.name(), "UTF-8");
    assert!(parsed.encoding.had_errors);
    assert_eq!(
        parsed.playlist.entries[0].location,
        EntryLocation::Local(PathBuf::from(OsStr::from_bytes(&undecoded)))
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| matches!(w.kind, PlaylistWarningKind::UndecodableText { .. }))
    );
}

#[test]
fn utf16_m3u_from_a_windows_tool() {
    let dir = folder("utf16");
    let mut file = vec![0xFF, 0xFE];
    for unit in "#EXTM3U\r\n#EXTINF:1,Björk - Jóga\r\nBjörk\\Jóga.flac\r\n".encode_utf16() {
        file.extend_from_slice(&unit.to_le_bytes());
    }
    let path = write(&dir, "list.m3u", &file);
    let parsed = read_playlist(&path, &ParseOptions::default()).unwrap();
    assert_eq!(parsed.encoding.name(), "UTF-16LE");
    assert_eq!(
        parsed.playlist.entries[0].location,
        local(dir.join("Björk/Jóga.flac"))
    );
    assert_eq!(parsed.playlist.entries[0].undecoded_path, None);
}

#[test]
fn relative_absolute_and_file_url_entries() {
    let dir = folder("kinds");
    let file = "#EXTM3U\n\
        sub/01.flac\n\
        ./sub/../02.flac\n\
        /srv/music/03.flac\n\
        file:///srv/music/04%20Four.flac\n\
        file://localhost/srv/music/Caf%E9.flac\n\
        FILE:///C:/Music/06.flac\n";
    let path = write(&dir, "kinds.m3u8", file);
    let parsed = read_playlist(&path, &ParseOptions::default()).unwrap();
    let locations: Vec<_> = parsed
        .playlist
        .entries
        .iter()
        .map(|e| e.location.clone())
        .collect();
    assert_eq!(
        locations,
        [
            local(dir.join("sub/01.flac")),
            local(dir.join("02.flac")),
            local("/srv/music/03.flac"),
            local("/srv/music/04 Four.flac"),
            local(Path::new(OsStr::from_bytes(b"/srv/music/Caf\xe9.flac"))),
            EntryLocation::Foreign(b"C:/Music/06.flac".to_vec()),
        ]
    );
}

#[test]
fn urls_are_kept_unsupported_and_never_fetched() {
    let dir = folder("urls");
    // Hosts that cannot resolve: if anything tried to fetch them, the offline audit's
    // network namespace would make it fail, and the test would hang or err.
    let m3u = "#EXTM3U\n#EXTINF:-1,Radio\nhttp://radio.invalid/stream.mp3\nhttps://x.invalid/a.ogg\n\
        rtsp://x.invalid/b\nmms://x.invalid/c\nlocal.mp3\n";
    let pls = "[playlist]\nFile1=http://radio.invalid:8000/live\nTitle1=Live\nNumberOfEntries=1\n";
    let xspf = "<playlist xmlns=\"http://xspf.org/ns/0/\"><trackList>\
        <track><location>http://x.invalid/a.mp3</location></track>\
        <track><location>smb://nas.invalid/b.flac</location></track></trackList></playlist>";
    for (name, file, remote) in [
        ("urls.m3u", m3u, 4),
        ("urls.pls", pls, 1),
        ("urls.xspf", xspf, 2),
    ] {
        let path = write(&dir, name, file);
        let parsed = read_playlist(&path, &ParseOptions::default()).unwrap();
        let entries = &parsed.playlist.entries;
        let urls = entries
            .iter()
            .filter(|e| matches!(e.location, EntryLocation::Remote(_)))
            .count();
        assert_eq!(urls, remote, "{name}");
        let warned = parsed
            .warnings
            .iter()
            .filter(|w| matches!(w.kind, PlaylistWarningKind::RemoteEntry(_)))
            .count();
        assert_eq!(warned, remote, "{name}");
    }
}

#[test]
fn nested_and_looping_playlists_are_not_followed() {
    let dir = folder("nested");
    let a = write(
        &dir,
        "a.m3u",
        "one.mp3\nb.m3u\nB.PLS\nc.xspf\nfile:///x/d.m3u8\ntwo.mp3\n",
    );
    write(&dir, "b.m3u", "a.m3u\nthree.mp3\n");
    write(&dir, "self.m3u", "self.m3u\n");
    let parsed = read_playlist(&a, &ParseOptions::default()).unwrap();
    let locations: Vec<_> = parsed
        .playlist
        .entries
        .iter()
        .map(|e| e.location.clone())
        .collect();
    assert_eq!(
        locations,
        [local(dir.join("one.mp3")), local(dir.join("two.mp3"))]
    );
    let nested = parsed
        .warnings
        .iter()
        .filter(|w| matches!(w.kind, PlaylistWarningKind::NestedPlaylist(_)))
        .count();
    assert_eq!(nested, 4);
    let looping = read_playlist(&dir.join("self.m3u"), &ParseOptions::default()).unwrap();
    assert!(looping.playlist.entries.is_empty());
}

#[test]
fn huge_files_are_bounded() {
    let dir = folder("huge");
    // Over the size limit: refused before parsing.
    let limits = PlaylistLimits {
        max_bytes: 1024 * 1024,
        ..PlaylistLimits::default()
    };
    let big = write(&dir, "big.m3u", vec![b'a'; 1024 * 1024 + 1]);
    let options = ParseOptions {
        limits,
        ..ParseOptions::default()
    };
    assert!(matches!(
        read_playlist(&big, &options),
        Err(PlaylistError::TooLarge { .. })
    ));

    // Within the size limit but over the entry limit: the rest is skipped once.
    let mut many = String::new();
    for i in 0..300_000 {
        many.push_str(&format!("{i}.mp3\n"));
    }
    let many = write(&dir, "many.m3u", many);
    let parsed = read_playlist(&many, &ParseOptions::default()).unwrap();
    assert_eq!(
        parsed.playlist.entries.len(),
        PlaylistLimits::default().max_entries
    );
    assert_eq!(parsed.warnings.len(), 1);
    assert_eq!(parsed.warnings[0].kind, PlaylistWarningKind::TooManyEntries);

    // One endless line: skipped, not kept.
    let mut long = vec![b'x'; 100_000];
    long.extend_from_slice(b"\nok.mp3\n");
    let long = write(&dir, "long.m3u", long);
    let parsed = read_playlist(&long, &ParseOptions::default()).unwrap();
    assert_eq!(parsed.playlist.entries.len(), 1);
    assert_eq!(parsed.warnings[0].kind, PlaylistWarningKind::LineTooLong);

    // Many warnings: at most 256 and a note.
    let bad = write(
        &dir,
        "bad.pls",
        "[playlist]\n".to_owned() + &"junk\n".repeat(10_000),
    );
    let parsed = read_playlist(&bad, &ParseOptions::default()).unwrap();
    assert_eq!(parsed.warnings.len(), 257);
    assert_eq!(
        parsed.warnings[256].kind,
        PlaylistWarningKind::TooManyWarnings
    );
}

#[test]
fn vlc_xspf() {
    let dir = folder("vlc");
    let file = r#"<?xml version="1.0" encoding="UTF-8"?>
<playlist xmlns="http://xspf.org/ns/0/" xmlns:vlc="http://www.videolan.org/vlc/playlist/ns/0/" version="1">
	<title>Playlist</title>
	<trackList>
		<track>
			<location>file:///home/me/Music/Sigur%20R%C3%B3s/%28%29/01.flac</location>
			<title>Untitled #1</title>
			<creator>Sigur Rós</creator>
			<duration>395000</duration>
			<extension application="http://www.videolan.org/vlc/playlist/0">
				<vlc:id>0</vlc:id>
			</extension>
		</track>
		<track>
			<location>Sigur%20R%C3%B3s/02.flac</location>
		</track>
	</trackList>
	<extension application="http://www.videolan.org/vlc/playlist/0">
		<vlc:item tid="0"/>
	</extension>
</playlist>
"#;
    let path = write(&dir, "vlc.xspf", file);
    let parsed = read_playlist(&path, &ParseOptions::default()).unwrap();
    assert_eq!(parsed.kind, PlaylistKind::Xspf);
    let entries = &parsed.playlist.entries;
    assert_eq!(
        entries[0].location,
        local("/home/me/Music/Sigur Rós/()/01.flac")
    );
    assert_eq!(entries[0].hints.artist.as_deref(), Some("Sigur Rós"));
    assert_eq!(entries[0].hints.duration, Some(Duration::from_secs(395)));
    assert_eq!(entries[0].line, Some(6));
    assert_eq!(entries[1].location, local(dir.join("Sigur Rós/02.flac")));
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn unknown_extensions_are_sniffed() {
    let dir = folder("sniff");
    let pls = write(&dir, "radio.txt", "[playlist]\nFile1=a.mp3\n");
    assert_eq!(
        read_playlist(&pls, &ParseOptions::default()).unwrap().kind,
        PlaylistKind::Pls
    );
    let xspf = write(&dir, "list", "<?xml version=\"1.0\"?><playlist/>");
    assert_eq!(
        read_playlist(&xspf, &ParseOptions::default()).unwrap().kind,
        PlaylistKind::Xspf
    );
    let missing = dir.join("missing.m3u");
    assert!(matches!(
        read_playlist(&missing, &ParseOptions::default()),
        Err(PlaylistError::Io { .. })
    ));
}
