// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask check-potfiles`: checks that `po/POTFILES.in` lists exactly
//! the files with translatable strings, so that the template (`helikodu.pot`)
//! holds every string the user can see.
//!
//! A Rust file under `crates/*/src` or `apps/*/src` has strings when it calls
//! `tr(`, `ntr(`, `trc(` or `ntrc(` (the gettext helpers of the GTK front end); a GtkBuilder
//! file under `crates/*/resources` when it marks a property
//! `translatable="yes"`. The data files (the desktop entry and the metainfo)
//! are listed by hand and only need to exist. Fails on a file with strings
//! that is not listed, and on a listed file that does not exist or has no
//! strings.
//!
//! It also checks that every call of a gettext helper (`tr`, `ntr`, `trc`,
//! `ntrc`, and gettext's own `gettext`, `ngettext`, `pgettext`,
//! `npgettext`) passes its context and messages as string literals: xgettext
//! only extracts literals, so `tr(name)` or `tr(match … { … })` would leave
//! the text out of the template and untranslated in every language.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

const POTFILES: &str = "po/POTFILES.in";

pub(crate) fn run(args: &[String]) -> Result<()> {
    if let Some(other) = args.first() {
        bail!("unknown option `{other}` for `check-potfiles`");
    }
    let root = crate::repository_root()?;
    let listed_text = fs::read_to_string(root.join(POTFILES))
        .with_context(|| format!("cannot read {POTFILES}"))?;
    let listed: BTreeSet<String> = listed_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();

    let mut found = BTreeSet::new();
    let mut not_literal = Vec::new();
    for top in ["crates", "apps"] {
        let Ok(entries) = fs::read_dir(root.join(top)) else {
            continue;
        };
        let mut packages: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        packages.sort();
        for package in packages {
            for (sub, extension) in [("src", "rs"), ("resources", "ui")] {
                let mut files = Vec::new();
                collect(&package.join(sub), extension, &mut files);
                for file in files {
                    let text = fs::read_to_string(&file).unwrap_or_default();
                    let strings = if extension == "rs" {
                        let path = relative(&root, &file);
                        for (line, name) in non_literal_calls(&text) {
                            if ALLOWED.contains(&(path.as_str(), name)) {
                                continue;
                            }
                            not_literal.push(format!(
                                "{}:{line}: `{name}(` is not given string literals",
                                relative(&root, &file)
                            ));
                        }
                        calls_gettext(&text)
                    } else {
                        text.contains("translatable=\"yes\"")
                    };
                    if strings {
                        found.insert(relative(&root, &file));
                    }
                }
            }
        }
    }

    if !not_literal.is_empty() {
        bail!(
            "xgettext cannot extract these messages:\n  {}\n(pass each message to the helper as a string literal, \
             for example one `tr(\"…\")` per match arm)",
            not_literal.join("\n  ")
        );
    }

    let mut problems = Vec::new();
    for file in found.difference(&listed) {
        problems.push(format!("{file} has translatable strings but is not listed"));
    }
    for file in listed.difference(&found) {
        if !root.join(file).is_file() {
            problems.push(format!("{file} is listed but does not exist"));
        } else if !file.starts_with("data/") {
            problems.push(format!("{file} is listed but has no translatable strings"));
        }
    }
    if !problems.is_empty() {
        bail!(
            "{POTFILES} is out of date:\n  {}\n(one path per line, relative to the repository root)",
            problems.join("\n  ")
        );
    }
    println!(
        "check-potfiles: {} files listed, all with translatable strings",
        listed.len()
    );
    Ok(())
}

/// Whether Rust source calls `tr(`, `ntr(`, `trc(` or `ntrc(` (not as part
/// of a longer name).
fn calls_gettext(text: &str) -> bool {
    ["tr(", "trc("].iter().any(|needle| calls(text, needle))
}

fn calls(text: &str, needle: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find(needle) {
        let start = from + at;
        let name_start = if start > 0 && bytes[start - 1] == b'n' {
            start - 1
        } else {
            start
        };
        let before = name_start.checked_sub(1).map(|i| bytes[i]);
        if !before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.') {
            return true;
        }
        from = start + needle.len();
    }
    false
}

/// Calls that may pass on a message they were given: the helpers themselves,
/// which wrap gettext's functions, and their tests, which must not add
/// messages to the template.
const ALLOWED: [(&str, &str); 6] = [
    ("crates/ui-gtk/src/i18n.rs", "gettext"),
    ("crates/ui-gtk/src/i18n.rs", "ngettext"),
    ("crates/ui-gtk/src/i18n.rs", "pgettext"),
    ("crates/ui-gtk/src/i18n.rs", "npgettext"),
    ("crates/ui-gtk/src/i18n.rs", "trc"),
    ("crates/ui-gtk/src/i18n.rs", "ntrc"),
];

/// The gettext helpers and the arguments of each that xgettext reads as
/// messages (and contexts): these must be string literals.
const HELPERS: [(&str, usize); 8] = [
    ("tr", 1),
    ("ntr", 2),
    ("trc", 2),
    ("ntrc", 3),
    ("gettext", 1),
    ("ngettext", 2),
    ("pgettext", 2),
    ("npgettext", 3),
];

/// The calls of a gettext helper in Rust source whose first arguments (the
/// ones xgettext extracts) are not all string literals, as line number and
/// helper name. Comments, strings, method calls (`x.tr(`) and the
/// definitions (`fn tr(`) are skipped.
fn non_literal_calls(text: &str) -> Vec<(usize, &'static str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut previous_word = "";
    let mut previous_char = b' ';
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(end) = skip_comment(bytes, i) {
            i = end;
            continue;
        }
        if let Some(end) = skip_string(bytes, i) {
            i = end;
            previous_char = b'"';
            previous_word = "";
            continue;
        }
        if b == b'\'' {
            i = skip_char_or_lifetime(text, i);
            previous_char = b'\'';
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            let helper = HELPERS.iter().find(|(name, _)| *name == word);
            if let Some(&(name, literals)) = helper {
                let open = skip_space(bytes, i);
                if previous_char != b'.'
                    && previous_word != "fn"
                    && bytes.get(open) == Some(&b'(')
                    && !literal_arguments(bytes, open + 1, literals)
                {
                    let line = text[..start].bytes().filter(|&c| c == b'\n').count() + 1;
                    out.push((line, name));
                }
            }
            previous_word = word;
            previous_char = b'a';
            continue;
        }
        if !b.is_ascii_whitespace() {
            previous_char = b;
            previous_word = "";
        }
        i += 1;
    }
    out
}

/// Whether `count` string literals separated by commas start at `at`.
fn literal_arguments(bytes: &[u8], mut at: usize, count: usize) -> bool {
    for n in 0..count {
        at = skip_space(bytes, at);
        if n > 0 {
            if bytes.get(at) != Some(&b',') {
                return false;
            }
            at = skip_space(bytes, at + 1);
        }
        match skip_string(bytes, at) {
            Some(end) => at = end,
            None => return false,
        }
    }
    true
}

/// The position after white space and comments from `at`.
fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        match skip_comment(bytes, at) {
            Some(end) => at = end,
            None => return at,
        }
    }
}

/// The end of a comment that starts at `at`.
fn skip_comment(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'/') {
        return None;
    }
    match bytes.get(at + 1) {
        Some(b'/') => Some(
            bytes[at..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(bytes.len(), |p| at + p + 1),
        ),
        Some(b'*') => {
            let mut depth = 0usize;
            let mut i = at;
            while i + 1 < bytes.len() {
                if bytes[i] == b'/' && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        return Some(i);
                    }
                } else {
                    i += 1;
                }
            }
            Some(bytes.len())
        }
        _ => None,
    }
}

/// The end of a string literal (`"…"`, `r"…"`, `r#"…"#`) that starts at `at`.
fn skip_string(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at) {
        Some(b'"') => {
            let mut i = at + 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' => i += 2,
                    b'"' => return Some(i + 1),
                    _ => i += 1,
                }
            }
            Some(bytes.len())
        }
        Some(b'r') => {
            if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
                return None;
            }
            let hashes = bytes[at + 1..].iter().take_while(|&&c| c == b'#').count();
            if bytes.get(at + 1 + hashes) != Some(&b'"') {
                return None;
            }
            let mut closing = vec![b'"'];
            closing.extend(std::iter::repeat_n(b'#', hashes));
            let body = at + 2 + hashes;
            Some(
                bytes[body..]
                    .windows(closing.len())
                    .position(|w| w == closing.as_slice())
                    .map_or(bytes.len(), |p| body + p + closing.len()),
            )
        }
        _ => None,
    }
}

/// The end of a character literal or a lifetime that starts at `at`.
fn skip_char_or_lifetime(text: &str, at: usize) -> usize {
    let rest = &text[at + 1..];
    if let Some(escaped) = rest.strip_prefix('\\') {
        return escaped.find('\'').map_or(text.len(), |p| at + 2 + p + 1);
    }
    let mut chars = rest.char_indices();
    match (chars.next(), chars.next()) {
        (Some(_), Some((p, '\''))) => at + 1 + p + 1,
        _ => at + 1,
    }
}

fn collect(dir: &Path, extension: &str, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, extension, out);
        } else if path.extension().is_some_and(|e| e == extension) {
            out.push(path);
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{calls_gettext, non_literal_calls};

    #[test]
    fn finds_the_gettext_calls_only() {
        assert!(calls_gettext("let a = tr(\"Play\");"));
        assert!(calls_gettext("&ntr(\"{} track\", \"{} tracks\", n)"));
        assert!(calls_gettext("i18n::tr(\"x\")"));
        assert!(calls_gettext("trc(\"track number\", \"Track\")"));
        assert!(calls_gettext("ntrc(\"c\", \"a\", \"b\", n)"));
        assert!(!calls_gettext(
            "let s = attr(x); str(y); self.tr(z); my_tr(w)"
        ));
        assert!(!calls_gettext("no calls here"));
    }

    #[test]
    fn messages_must_be_literals() {
        let ok = r##"
            // tr(name) in a comment
            let a = tr("Play");
            let b = ntr(
                // Translators: a count.
                "{} track",
                "{} tracks",
                n,
            );
            let c = trc("track number", "Track");
            let d = gettextrs::pgettext("decimal separator", ".");
            let e = tr(r#"say "hi""#);
            let f = "tr(x)";
            let g = self.tr(x);
            fn tr(text: &str) -> String { gettextrs::gettext(text) }
            let h: &'static str = 'x'.to_string(); let i = '\''; tr("ok");
        "##;
        assert_eq!(non_literal_calls(ok), vec![(15, "gettext")]);
        let bad = "let a = tr(name);\nlet b = tr(match x { _ => \"a\" });\n\
                   let c = ntr(\"{} a\", plural, n);\nlet d = trc(CONTEXT, \"x\");\n\
                   let e = i18n::tr(&text);";
        assert_eq!(
            non_literal_calls(bad),
            vec![(1, "tr"), (2, "tr"), (3, "ntr"), (4, "trc"), (5, "tr")]
        );
    }
}
