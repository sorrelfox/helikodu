// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text values on their way from a tag to the builder: charset repair and the
//! splitting of NUL-separated multiple values.

use std::borrow::Cow;

use encoding_rs::Encoding;

use crate::tags::charset::CharsetRepair;

/// Applies the charset repair of one tag at a time and remembers whether it
/// changed anything.
#[derive(Debug)]
pub(crate) struct TextFixer {
    enabled: bool,
    preferred: Option<&'static Encoding>,
    current: CharsetRepair,
    repaired: bool,
}

impl TextFixer {
    pub(crate) fn new(enabled: bool, preferred: Option<&'static Encoding>) -> Self {
        Self {
            enabled,
            preferred,
            current: CharsetRepair::default(),
            repaired: false,
        }
    }

    /// Prepares for the next tag, whose text values are `texts`.
    pub(crate) fn begin_tag<'a>(&mut self, texts: impl IntoIterator<Item = &'a str>) {
        self.current = if self.enabled {
            CharsetRepair::detect(texts, self.preferred)
        } else {
            CharsetRepair::default()
        };
    }

    /// `text`, repaired if needed.
    pub(crate) fn fix<'a>(&mut self, text: &'a str) -> Cow<'a, str> {
        if !self.enabled {
            return Cow::Borrowed(text);
        }
        match self.current.repair(text) {
            Some(repaired) => {
                self.repaired = true;
                Cow::Owned(repaired)
            }
            None => Cow::Borrowed(text),
        }
    }

    /// The repaired values of a text that may hold several values separated by NUL
    /// characters (ID3v2.4 text frames, APEv2 items).
    pub(crate) fn values(&mut self, text: &str) -> Vec<String> {
        text.split('\0')
            .map(|v| self.fix(v).into_owned())
            .filter(|v| !v.trim().is_empty())
            .collect()
    }

    /// Whether any text was repaired.
    pub(crate) fn repaired(&self) -> bool {
        self.repaired
    }
}
