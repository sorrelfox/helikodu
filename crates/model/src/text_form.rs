// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text forms of values: the error for text that does not parse, and serde through
//! `Display` and `FromStr`, so that settings, the command line and JSON all use the
//! same spelling.

use thiserror::Error;

/// Text that is not the text form of the expected value.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid {expected}: {value:?}")]
pub struct ParseError {
    expected: &'static str,
    value: String,
}

impl ParseError {
    pub(crate) fn new(expected: &'static str, value: &str) -> Self {
        Self {
            expected,
            value: value.to_owned(),
        }
    }

    /// The kind of value that was expected, for example `"sample format"`.
    pub fn expected(&self) -> &'static str {
        self.expected
    }

    /// The text that could not be parsed.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Implements `Serialize` and `Deserialize` for a type through its `Display` and
/// `FromStr` implementations.
macro_rules! serde_as_text {
    ($ty:ty) => {
        impl ::serde::Serialize for $ty {
            fn serialize<S: ::serde::Serializer>(
                &self,
                serializer: S,
            ) -> ::std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $ty {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> ::std::result::Result<Self, D::Error> {
                let text =
                    <::std::string::String as ::serde::Deserialize>::deserialize(deserializer)?;
                text.parse()
                    .map_err(<D::Error as ::serde::de::Error>::custom)
            }
        }
    };
}

pub(crate) use serde_as_text;
