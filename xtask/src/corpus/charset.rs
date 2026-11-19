// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text in the charsets old taggers and CUE sheet writers used.

use anyhow::{Result, bail};

/// `text` in `charset`: `utf-8`, `utf-8-bom` (with a byte order mark), `latin1`
/// (ISO 8859-1, one byte per character up to U+00FF) or any WHATWG label such
/// as `windows-1251`, `shift_jis` or `gbk`. Fails when a character has no
/// encoding there, so that a recipe never writes something else than it says.
pub(crate) fn encode(text: &str, charset: &str) -> Result<Vec<u8>> {
    match charset.to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" => Ok(text.as_bytes().to_vec()),
        "utf-8-bom" => {
            let mut out = vec![0xef, 0xbb, 0xbf];
            out.extend_from_slice(text.as_bytes());
            Ok(out)
        }
        "latin1" | "iso-8859-1" => text
            .chars()
            .map(|character| {
                u8::try_from(u32::from(character))
                    .map_err(|_| anyhow::anyhow!("`{character}` has no encoding in Latin-1"))
            })
            .collect(),
        label => {
            let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) else {
                bail!("unknown charset `{charset}`");
            };
            let (bytes, used, unmappable) = encoding.encode(text);
            if unmappable || used != encoding {
                bail!("`{text}` cannot be written in {charset}");
            }
            Ok(bytes.into_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_charsets() {
        assert_eq!(encode("Кино", "windows-1251").unwrap(), b"\xca\xe8\xed\xee");
        assert_eq!(
            encode("テスト", "shift_jis").unwrap(),
            b"\x83\x65\x83\x58\x83\x67"
        );
        assert_eq!(encode("café", "latin1").unwrap(), b"caf\xe9");
        assert_eq!(encode("a", "utf-8-bom").unwrap(), b"\xef\xbb\xbfa");
        assert!(encode("Кино", "latin1").is_err());
        assert!(encode("Кино", "shift_jis").is_ok());
        assert!(encode("ひ", "windows-1251").is_err());
    }
}
