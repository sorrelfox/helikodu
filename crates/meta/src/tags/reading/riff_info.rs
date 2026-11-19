// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RIFF `INFO` list of a WAV file, read from its bytes.
//!
//! RIFF INFO has no text encoding of its own, and most old taggers wrote it in
//! the system's legacy charset. The tag library drops every value that is not
//! UTF-8, so such a file would seem untagged. Here a value that is not UTF-8 is
//! kept as Latin-1 text, one character per byte, for the charset repair to
//! decode, like an ID3v1 tag.

use std::io::{Read, Seek, SeekFrom};

/// The largest `LIST` chunk read; larger ones are not tags.
const MAX_LIST_BYTES: u32 = 1 << 20;
/// The most chunks looked at before giving up.
const MAX_CHUNKS: usize = 1024;

/// The `INFO` items of the WAV file in `reader` as key and text, in file order;
/// `None` when the file has no `INFO` list or is not a RIFF WAVE file.
pub(super) fn read<R: Read + Seek>(reader: &mut R) -> Option<Vec<(String, String)>> {
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut header = [0u8; 12];
    reader.read_exact(&mut header).ok()?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return None;
    }
    let mut items: Option<Vec<(String, String)>> = None;
    for _ in 0..MAX_CHUNKS {
        let mut chunk = [0u8; 8];
        if reader.read_exact(&mut chunk).is_err() {
            break;
        }
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        let padded = i64::from(size) + i64::from(size & 1);
        if &chunk[..4] != b"LIST" || !(4..=MAX_LIST_BYTES).contains(&size) {
            reader.seek(SeekFrom::Current(padded)).ok()?;
            continue;
        }
        let mut body = vec![0; size as usize];
        reader.read_exact(&mut body).ok()?;
        if size & 1 == 1 {
            reader.seek(SeekFrom::Current(1)).ok()?;
        }
        if let Some(info) = body.strip_prefix(b"INFO") {
            items.get_or_insert_with(Vec::new).extend(parse_items(info));
        }
    }
    items
}

fn parse_items(mut bytes: &[u8]) -> Vec<(String, String)> {
    let mut items = Vec::new();
    while bytes.len() >= 8 {
        let id = &bytes[..4];
        let size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let Some(value) = bytes.get(8..8 + size) else {
            break;
        };
        if id.iter().all(u8::is_ascii_alphanumeric) {
            let end = value.iter().position(|&b| b == 0).unwrap_or(value.len());
            let value = &value[..end];
            let text = match std::str::from_utf8(value) {
                Ok(text) => text.to_owned(),
                Err(_) => value.iter().copied().map(char::from).collect(),
            };
            items.push((String::from_utf8_lossy(id).into_owned(), text));
        }
        bytes = bytes.get(8 + size + (size & 1)..).unwrap_or_default();
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn wav(info: &[u8]) -> Vec<u8> {
        let mut list = b"INFO".to_vec();
        list.extend_from_slice(info);
        let mut out = b"RIFF\0\0\0\0WAVEdata\x02\0\0\0\0\0LIST".to_vec();
        out.extend_from_slice(&(list.len() as u32).to_le_bytes());
        out.extend_from_slice(&list);
        out
    }

    #[test]
    fn legacy_bytes_are_kept_as_latin1() {
        let file = wav(b"INAM\x05\0\0\0\xca\xe8\xed\xee\0\0IART\x02\0\0\0A\0");
        let items = read(&mut Cursor::new(file)).unwrap();
        assert_eq!(
            items,
            [
                ("INAM".to_owned(), "\u{ca}\u{e8}\u{ed}\u{ee}".to_owned()),
                ("IART".to_owned(), "A".to_owned())
            ]
        );
    }

    #[test]
    fn utf8_stays_and_damage_is_survived() {
        let file = wav("INAM\x07\0\0\0Motörh\0".as_bytes());
        assert_eq!(read(&mut Cursor::new(file)).unwrap()[0].1, "Motörh");
        let truncated = wav(b"INAM\xff\0\0\0abc");
        assert_eq!(read(&mut Cursor::new(truncated)), Some(Vec::new()));
        assert_eq!(read(&mut Cursor::new(b"RIFF".to_vec())), None);
    }
}
