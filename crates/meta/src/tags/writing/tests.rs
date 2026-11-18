// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Crash safety of the write path, with a file built in memory: a failed check
//! leaves the original as it was and no temporary copy behind.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

/// A short silent WAV file: 0.1 s, 8 kHz, mono, 16 bit.
fn wav_bytes() -> Vec<u8> {
    let pcm = vec![0u8; 1600];
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&8000u32.to_le_bytes());
    out.extend_from_slice(&16000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("helikodu-meta-writing-{}", std::process::id()))
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test folder");
    dir
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("list the folder")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn title_change() -> TagChanges {
    let mut changes = TagChanges::new();
    changes.set_one(TagField::Title, "New title");
    changes
}

#[test]
fn a_failed_check_leaves_the_original_and_no_copy() {
    let dir = folder("failed-check");
    let path = dir.join("a.wav");
    fs::write(&path, wav_bytes()).expect("write the file");
    let before = fs::read(&path).expect("read the file");
    let result = write_checked(
        &path,
        &title_change(),
        None,
        &WriteOptions::default(),
        &|p| {
            assert!(p != path && p.parent() == path.parent(), "checks the copy");
            Err(WriteError::VerificationFailed {
                reason: "test".into(),
                in_place: false,
            })
        },
    );
    assert!(matches!(
        result,
        Err(WriteError::VerificationFailed {
            in_place: false,
            ..
        })
    ));
    assert_eq!(fs::read(&path).expect("read the file"), before);
    assert_eq!(entries(&dir), ["a.wav"]);
}

#[test]
fn a_successful_write_replaces_the_file_and_leaves_no_copy() {
    let dir = folder("replaced");
    let path = dir.join("a.wav");
    fs::write(&path, wav_bytes()).expect("write the file");
    let outcome = write_tags(&path, &title_change(), None, &WriteOptions::default())
        .expect("the write succeeds");
    assert_eq!(outcome.mode, WriteMode::Replaced);
    assert_eq!(outcome.id3_version, Some(Id3Version::V2_4));
    assert_eq!(entries(&dir), ["a.wav"]);
    let read = crate::tags::read_tags(&path, &Default::default()).expect("read back");
    assert_eq!(read.tags.title.as_deref(), Some("New title"));
}

#[test]
fn a_stale_stamp_is_refused_before_anything_is_written() {
    let dir = folder("stale");
    let path = dir.join("a.wav");
    fs::write(&path, wav_bytes()).expect("write the file");
    let mut stamp = EditStamp::of(&path).expect("stamp");
    stamp.size += 1;
    let result = write_tags(
        &path,
        &title_change(),
        Some(stamp),
        &WriteOptions::default(),
    );
    assert!(matches!(result, Err(WriteError::ChangedSinceRead)));
    assert_eq!(fs::read(&path).expect("read"), wav_bytes());
    assert_eq!(entries(&dir), ["a.wav"]);
}

#[test]
fn stale_temporaries_are_removed() {
    let dir = folder("stale-temporaries");
    fs::write(dir.join(".a.wav.1-0.helikodu-tmp"), b"partial").expect("write");
    fs::write(dir.join("a.wav"), b"x").expect("write");
    assert_eq!(
        remove_stale_temporaries(&dir, std::time::Duration::ZERO).expect("clean"),
        1
    );
    assert_eq!(entries(&dir), ["a.wav"]);
}

#[test]
fn not_audio_is_a_read_only_format() {
    let dir = folder("not-audio");
    let path = dir.join("a.txt");
    fs::write(&path, b"just text, not audio").expect("write");
    assert!(matches!(
        write_tags(&path, &title_change(), None, &WriteOptions::default()),
        Err(WriteError::ReadOnlyFormat)
    ));
}
