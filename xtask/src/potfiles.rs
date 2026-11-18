// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask check-potfiles`: checks that `po/POTFILES.in` lists exactly
//! the files with translatable strings, so that the template (`helikodu.pot`)
//! holds every string the user can see.
//!
//! A Rust file under `crates/*/src` or `apps/*/src` has strings when it calls
//! `tr(` or `ntr(` (the gettext helpers of the GTK front end); a GtkBuilder
//! file under `crates/*/resources` when it marks a property
//! `translatable="yes"`. The data files (the desktop entry and the metainfo)
//! are listed by hand and only need to exist. Fails on a file with strings
//! that is not listed, and on a listed file that does not exist or has no
//! strings.

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

/// Whether Rust source calls `tr(` or `ntr(` (not as part of a longer name).
fn calls_gettext(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find("tr(") {
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
        from = start + 3;
    }
    false
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
    use super::calls_gettext;

    #[test]
    fn finds_the_gettext_calls_only() {
        assert!(calls_gettext("let a = tr(\"Play\");"));
        assert!(calls_gettext("&ntr(\"{} track\", \"{} tracks\", n)"));
        assert!(calls_gettext("i18n::tr(\"x\")"));
        assert!(!calls_gettext(
            "let s = attr(x); str(y); self.tr(z); my_tr(w)"
        ));
        assert!(!calls_gettext("no calls here"));
    }
}
