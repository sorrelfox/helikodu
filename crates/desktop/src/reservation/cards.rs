// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The ALSA card index behind a card ID string: the `N` of `Audio<N>`.

use std::fs;

/// Where the kernel lists the sound cards.
const CARDS_FILE: &str = "/proc/asound/cards";

/// The index of the ALSA card with this ID string (for example `Generic_1`, as
/// in `hw:CARD=Generic_1,DEV=0`) or index, from `/proc/asound/cards`.
pub fn card_index(card: &str) -> Option<u32> {
    if let Ok(index) = card.parse() {
        return Some(index);
    }
    parse(&fs::read_to_string(CARDS_FILE).ok()?, card)
}

/// Finds `id` in the text of `/proc/asound/cards`, whose card lines read
/// ` 3 [Generic_1      ]: HDA-Intel - HD-Audio Generic`.
fn parse(cards: &str, id: &str) -> Option<u32> {
    cards.lines().find_map(|line| {
        let (index, rest) = line.trim_start().split_once(' ')?;
        let index = index.parse().ok()?;
        let name = rest.trim_start().strip_prefix('[')?.split_once(']')?.0;
        (name.trim_end() == id).then_some(index)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: &str = " 0 [AMS22          ]: USB-Audio - AMS-22
                      ZOOM Corporation AMS-22 at usb-0000:79:00.0-1, high speed
 3 [Generic_1      ]: HDA-Intel - HD-Audio Generic
                      HD-Audio Generic at 0xde180000 irq 192
12 [CODEC          ]: USB-Audio - USB Audio CODEC
                      Burr-Brown from TI USB Audio CODEC at usb-0000:12:00.0-3, full speed
";

    #[test]
    fn finds_cards_by_id_string() {
        assert_eq!(parse(CARDS, "AMS22"), Some(0));
        assert_eq!(parse(CARDS, "Generic_1"), Some(3));
        assert_eq!(parse(CARDS, "CODEC"), Some(12));
        assert_eq!(parse(CARDS, "Generic"), None);
        assert_eq!(parse(CARDS, "HD-Audio"), None);
    }

    #[test]
    fn an_index_is_its_own_index() {
        assert_eq!(card_index("4"), Some(4));
    }
}
