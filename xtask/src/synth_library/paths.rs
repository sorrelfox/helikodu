// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! File and folder names: made safe, shortened, and for some albums written
//! in a legacy 8-bit encoding so that the name is not valid UTF-8, as a disc
//! ripped on an old system would be.

/// Longest file or folder name, in bytes, before the extension. File systems
/// allow 255; the margin leaves room for the track number and extension.
const MAX_COMPONENT: usize = 180;

/// A name usable as one path component: no `/` or NUL, no leading dot or
/// trailing space, at most [`MAX_COMPONENT`] bytes, never empty.
pub(crate) fn component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for character in name.chars() {
        let character = match character {
            '/' | '\0' => '_',
            other if other.is_control() => '_',
            other => other,
        };
        if out.len() + character.len_utf8() > MAX_COMPONENT {
            break;
        }
        out.push(character);
    }
    let trimmed = out.trim_start_matches('.').trim_end();
    if trimmed.is_empty() {
        String::from("_")
    } else {
        trimmed.to_owned()
    }
}

/// A legacy 8-bit encoding of file names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Legacy {
    Latin1,
    Cp1251,
}

/// `name` in `legacy`, or `None` if a character has no byte there. ASCII
/// stays as it is.
pub(crate) fn encode(name: &str, legacy: Legacy) -> Option<Vec<u8>> {
    name.chars()
        .map(|character| {
            let code = u32::from(character);
            if code < 0x80 {
                return Some(code as u8);
            }
            match legacy {
                Legacy::Latin1 => (0xa0..=0xff).contains(&code).then_some(code as u8),
                Legacy::Cp1251 => match code {
                    // А…я
                    0x410..=0x44f => Some((code - 0x410 + 0xc0) as u8),
                    0x401 => Some(0xa8), // Ё
                    0x451 => Some(0xb8), // ё
                    _ => None,
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_safe() {
        assert_eq!(component("AC/DC"), "AC_DC");
        assert_eq!(component("..hidden "), "hidden");
        assert_eq!(component("..."), "_");
        assert_eq!(component("a\nb"), "a_b");
        let long = component(&"ä".repeat(200));
        assert!(long.len() <= MAX_COMPONENT);
        assert!(long.chars().all(|c| c == 'ä'));
    }

    #[test]
    fn latin1() {
        assert_eq!(
            encode("Jüri Õie", Legacy::Latin1).unwrap(),
            b"J\xfcri \xd5ie"
        );
        assert_eq!(encode("Šmidt", Legacy::Latin1), None);
        let bytes = encode("Björk", Legacy::Latin1).unwrap();
        assert!(std::str::from_utf8(&bytes).is_err());
    }

    #[test]
    fn cp1251() {
        assert_eq!(
            encode("Ёж ночь", Legacy::Cp1251).unwrap(),
            b"\xa8\xe6 \xed\xee\xf7\xfc"
        );
        assert_eq!(encode("Привет", Legacy::Cp1251).unwrap()[0], 0xcf);
        assert_eq!(encode("ä", Legacy::Cp1251), None);
    }
}
