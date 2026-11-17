// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! File names as raw bytes.
//!
//! Recipes and the manifest are UTF-8 text, but some corpus files have names
//! that are not valid UTF-8. Both therefore write names percent-encoded: `%XX`
//! stands for the byte `0xXX`, every other character stands for its UTF-8
//! bytes. A literal `%` is written `%25`.

use anyhow::{Result, bail};

/// Decodes a percent-encoded name into its bytes.
pub(crate) fn decode(name: &str) -> Result<Vec<u8>> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(hex) = name.get(index + 1..index + 3) else {
                bail!("`{name}`: `%` must be followed by two hex digits");
            };
            let Ok(byte) = u8::from_str_radix(hex, 16) else {
                bail!("`{name}`: `%{hex}` is not a byte in hex");
            };
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Ok(out)
}

/// Encodes bytes as a percent-encoded name. Valid UTF-8 stays readable;
/// invalid bytes, control characters and `%` are escaped.
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for character in chunk.valid().chars() {
            if character == '%' || character.is_control() {
                let mut buffer = [0; 4];
                for byte in character.encode_utf8(&mut buffer).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            } else {
                out.push(character);
            }
        }
        for byte in chunk.invalid() {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Pads the last path component of `name` to exactly `length` bytes by
/// inserting `é` (two bytes) and, if needed, one `x` before the extension.
pub(crate) fn pad(name: &[u8], length: usize) -> Result<Vec<u8>> {
    let file_start = name
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1);
    let file = &name[file_start..];
    let dot = file
        .iter()
        .rposition(|&byte| byte == b'.')
        .unwrap_or(file.len());
    if file.len() > length {
        bail!("`{}` is already longer than {length} bytes", encode(name));
    }
    let missing = length - file.len();
    let mut padding = "é".repeat(missing / 2).into_bytes();
    if missing % 2 == 1 {
        padding.push(b'x');
    }
    let split = file_start + dot;
    let mut out = name[..split].to_vec();
    out.extend_from_slice(&padding);
    out.extend_from_slice(&name[split..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let raw = b"names/caf\xe9 100%\n.flac".to_vec();
        let encoded = encode(&raw);
        assert_eq!(encoded, "names/caf%E9 100%25%0A.flac");
        assert_eq!(decode(&encoded).unwrap(), raw);
    }

    #[test]
    fn utf8_stays_readable() {
        assert_eq!(encode("Björk.flac".as_bytes()), "Björk.flac");
    }

    #[test]
    fn pads_to_length() {
        let padded = pad(b"names/long.flac", 255).unwrap();
        let file = &padded[b"names/".len()..];
        assert_eq!(file.len(), 255);
        assert!(file.ends_with(b".flac"));
        assert!(file.starts_with(b"long"));
    }

    #[test]
    fn rejects_bad_escapes() {
        assert!(decode("a%G1").is_err());
        assert!(decode("a%1").is_err());
    }
}
