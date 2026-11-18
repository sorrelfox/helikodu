// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Translations through gettext. Every string the user sees goes through
//! [`tr`] or [`ntr`]; templates mark theirs `translatable="yes"`.

use gettextrs::{bind_textdomain_codeset, bindtextdomain, textdomain};

/// The gettext domain of the program and its templates.
pub const DOMAIN: &str = "helikodu";

/// Where the compiled translations are installed, unless the build says otherwise.
const LOCALE_DIR: &str = match option_env!("HELIKODU_LOCALEDIR") {
    Some(dir) => dir,
    None => "/usr/share/locale",
};

/// Selects the program's translations. GTK selects the user's locale when it
/// starts. Missing translations are not an error: the English source strings
/// show.
pub fn init() {
    let _ = bindtextdomain(DOMAIN, LOCALE_DIR);
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);
}

/// The translation of `text`.
pub(crate) fn tr(text: &str) -> String {
    gettextrs::gettext(text)
}

/// The translation of `singular` or `plural` for `n`.
pub(crate) fn ntr(singular: &str, plural: &str, n: usize) -> String {
    gettextrs::ngettext(singular, plural, u32::try_from(n).unwrap_or(u32::MAX))
}

/// Replaces `{}` placeholders in a translated pattern, in order. Translators
/// see the whole sentence with its placeholders.
pub(crate) fn fill(pattern: &str, values: &[&str]) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut values = values.iter();
    let mut rest = pattern;
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        out.push_str(values.next().copied().unwrap_or(""));
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_in_order() {
        assert_eq!(fill("{} of {}", &["1:42", "4:55"]), "1:42 of 4:55");
        assert_eq!(fill("none", &["x"]), "none");
    }
}
