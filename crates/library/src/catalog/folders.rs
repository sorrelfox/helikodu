// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What folder names say about albums: disc folders such as `CD1`, `Disc 2` or
//! `Disc 2 - Live`, and album folders whose name ends with a disc, such as
//! `Album (CD2)`.

/// A disc found in a folder name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscFolder {
    /// The disc number.
    pub(crate) number: u32,
    /// The text after the number in a disc folder, such as `Live` in `Disc 2 - Live`.
    pub(crate) subtitle: Option<String>,
    /// Whether the whole name is a disc (`CD1`, `Disc 2 - Live`): the album is the
    /// folder above. Otherwise the disc ends the name of the album's own folder.
    pub(crate) whole_name: bool,
}

/// The words that name a disc.
const DISC_WORDS: [&str; 4] = ["disc", "disk", "cd", "dvd"];

/// Characters between the word, the number and a subtitle.
fn is_separator(c: char) -> bool {
    matches!(c, ' ' | '-' | '_' | '.' | ':' | '–' | '—' | '#')
}

/// The disc that the folder name `name` gives, if it gives one.
pub(crate) fn disc_folder(name: &str) -> Option<DiscFolder> {
    whole_disc_name(name).or_else(|| trailing_disc(name))
}

/// `CD1`, `cd 01`, `Disc 2`, `Disk_3`, `Disc 2 - Live in Tartu`.
fn whole_disc_name(name: &str) -> Option<DiscFolder> {
    let trimmed = name.trim();
    let rest = DISC_WORDS.iter().find_map(|word| {
        let head = trimmed.get(..word.len())?;
        head.eq_ignore_ascii_case(word)
            .then(|| &trimmed[word.len()..])
    })?;
    let rest = rest.trim_start_matches(is_separator);
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let number: u32 = rest[..digits].parse().ok()?;
    let after = &rest[digits..];
    let subtitle = match after.chars().next() {
        None => None,
        Some(c) if is_separator(c) || c == '(' || c == '[' => {
            let text = after
                .trim_start_matches(is_separator)
                .trim()
                .trim_start_matches(['(', '['])
                .trim_end_matches([')', ']'])
                .trim();
            (!text.is_empty()).then(|| text.to_owned())
        }
        Some(_) => return None,
    };
    (number > 0).then_some(DiscFolder {
        number,
        subtitle,
        whole_name: true,
    })
}

/// `Album (CD2)`, `Album [Disc 1]`, `Album - Disc 3`, `Album CD1`.
fn trailing_disc(name: &str) -> Option<DiscFolder> {
    let trimmed = name.trim().trim_end_matches([')', ']']).trim_end();
    let digits = trimmed
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let (head, number) = trimmed.split_at(trimmed.len() - digits);
    let number: u32 = number.parse().ok()?;
    let head = head.trim_end_matches(is_separator);
    let word = DISC_WORDS.iter().find(|word| {
        head.len() >= word.len()
            && head
                .get(head.len() - word.len()..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(word))
    })?;
    let before = &head[..head.len() - word.len()];
    // The word must stand alone: `Album (CD2)`, not `ABCD2`.
    let stands_alone = before
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_alphanumeric());
    (stands_alone && number > 0).then_some(DiscFolder {
        number,
        subtitle: None,
        whole_name: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(number: u32, subtitle: Option<&str>) -> Option<DiscFolder> {
        Some(DiscFolder {
            number,
            subtitle: subtitle.map(str::to_owned),
            whole_name: true,
        })
    }

    #[test]
    fn disc_folders() {
        assert_eq!(disc_folder("CD1"), whole(1, None));
        assert_eq!(disc_folder("cd 02"), whole(2, None));
        assert_eq!(disc_folder("Disc 2"), whole(2, None));
        assert_eq!(disc_folder("DISK_3"), whole(3, None));
        assert_eq!(
            disc_folder("Disc 2 - Live in Tartu"),
            whole(2, Some("Live in Tartu"))
        );
        assert_eq!(
            disc_folder("CD1 (Remastered)"),
            whole(1, Some("Remastered"))
        );
        assert_eq!(disc_folder("Disc #4"), whole(4, None));
    }

    #[test]
    fn album_folders_ending_with_a_disc() {
        let trailing = |n| {
            Some(DiscFolder {
                number: n,
                subtitle: None,
                whole_name: false,
            })
        };
        assert_eq!(disc_folder("Album (CD2)"), trailing(2));
        assert_eq!(disc_folder("Album [Disc 1]"), trailing(1));
        assert_eq!(disc_folder("Album - disc 3"), trailing(3));
        assert_eq!(disc_folder("Album CD1"), trailing(1));
    }

    #[test]
    fn other_names() {
        for name in [
            "Discography",
            "Disco 2000",
            "CDs",
            "ABCD2",
            "Album 2",
            "CD",
            "Disc 0",
            "1999",
            "Discovery",
            "Live at the Cdx 1",
        ] {
            assert_eq!(disc_folder(name), None, "{name}");
        }
    }
}
