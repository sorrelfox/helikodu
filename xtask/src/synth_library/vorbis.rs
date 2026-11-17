// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Vorbis comment structure that FLAC and Ogg Opus share: a vendor
//! string and a list of `KEY=value` fields, each preceded by its length as a
//! 32-bit little-endian number. A key may appear more than once, which is how
//! multi-value tags are written.

use anyhow::{Result, bail};

/// The vendor string of every file made here.
pub(crate) const VENDOR: &str = "Helikodu synth-library";

/// The comment structure without any framing bit (neither FLAC nor Opus
/// uses one).
pub(crate) fn comment(vendor: &str, fields: &[(&str, String)]) -> Result<Vec<u8>> {
    let size = 8
        + vendor.len()
        + fields
            .iter()
            .map(|(k, v)| 5 + k.len() + v.len())
            .sum::<usize>();
    let mut out = Vec::with_capacity(size);
    push_string(&mut out, vendor.as_bytes())?;
    push_length(&mut out, fields.len())?;
    for (key, value) in fields {
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| (0x20..=0x7d).contains(&byte) && byte != b'=')
        {
            bail!("`{key}` is not a valid Vorbis comment key");
        }
        push_length(&mut out, key.len() + 1 + value.len())?;
        out.extend_from_slice(key.as_bytes());
        out.push(b'=');
        out.extend_from_slice(value.as_bytes());
    }
    Ok(out)
}

fn push_string(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    push_length(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn push_length(out: &mut Vec<u8>, length: usize) -> Result<()> {
    let Ok(length) = u32::try_from(length) else {
        bail!("a Vorbis comment field is longer than 4 GiB");
    };
    out.extend_from_slice(&length.to_le_bytes());
    Ok(())
}

/// Reads a comment structure back: the vendor and the fields. Used by the
/// tests, and to check the writers against each other.
#[cfg(test)]
pub(crate) fn parse(bytes: &[u8]) -> Result<(String, Vec<(String, String)>)> {
    fn take<'a>(bytes: &mut &'a [u8], count: usize) -> Result<&'a [u8]> {
        if bytes.len() < count {
            bail!("truncated comment");
        }
        let (head, tail) = bytes.split_at(count);
        *bytes = tail;
        Ok(head)
    }
    fn length(bytes: &mut &[u8]) -> Result<usize> {
        let raw = take(bytes, 4)?;
        Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize)
    }
    let mut rest = bytes;
    let vendor_length = length(&mut rest)?;
    let vendor = String::from_utf8(take(&mut rest, vendor_length)?.to_vec())?;
    let count = length(&mut rest)?;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let field_length = length(&mut rest)?;
        let field = std::str::from_utf8(take(&mut rest, field_length)?)?;
        let Some((key, value)) = field.split_once('=') else {
            bail!("field without `=`");
        };
        fields.push((key.to_owned(), value.to_owned()));
    }
    if !rest.is_empty() {
        bail!("{} bytes after the last field", rest.len());
    }
    Ok((vendor, fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_bytes() {
        let bytes = comment("v", &[("A", String::from("ä"))]).unwrap();
        assert_eq!(
            bytes,
            [
                1, 0, 0, 0, b'v', 1, 0, 0, 0, 4, 0, 0, 0, b'A', b'=', 0xc3, 0xa4
            ]
        );
    }

    #[test]
    fn round_trip_with_repeated_keys() {
        let fields = vec![
            ("ARTIST", String::from("Jüri Tamm")),
            ("ARTIST", String::from("山田 太郎")),
            ("TITLE", String::from("a=b")),
        ];
        let bytes = comment(VENDOR, &fields).unwrap();
        let (vendor, parsed) = parse(&bytes).unwrap();
        assert_eq!(vendor, VENDOR);
        let expected: Vec<(String, String)> =
            fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn rejects_bad_keys() {
        assert!(comment(VENDOR, &[("A=B", String::new())]).is_err());
        assert!(comment(VENDOR, &[("", String::new())]).is_err());
        assert!(comment(VENDOR, &[("ÄÖ", String::new())]).is_err());
    }
}
