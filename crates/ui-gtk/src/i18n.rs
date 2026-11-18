// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Translations through gettext. Every string the user sees goes through
//! [`tr`] or [`ntr`]; templates mark theirs `translatable="yes"`.
//!
//! The interface language is the desktop's, or the one chosen in Settings ›
//! Appearance (`[appearance] language`). gettext, for Helikodu's strings and
//! for GTK's own, takes it from the `LANGUAGE` variable, and a running
//! program cannot change its environment safely; so
//! [`follow_language_setting`] starts the program again at once (`exec`, the
//! same process) with `LANGUAGE` set, before GTK starts.

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gettextrs::{bind_textdomain_codeset, bindtextdomain, textdomain};

/// The gettext domain of the program and its templates.
pub const DOMAIN: &str = "helikodu";

/// Where the compiled translations are installed, unless the build says otherwise.
const LOCALE_DIR: &str = match option_env!("HELIKODU_LOCALEDIR") {
    Some(dir) => dir,
    None => "/usr/share/locale",
};

/// Names a folder of compiled translations at run time, to try translations
/// that are not installed.
const LOCALE_DIR_VARIABLE: &str = "HELIKODU_LOCALEDIR";

/// The variable gettext takes the interface language from.
const LANGUAGE_VARIABLE: &str = "LANGUAGE";

/// Holds the desktop's `LANGUAGE` (empty when it had none) while Helikodu runs
/// in a language of its own, so that a later start in the desktop's language
/// can go back to it.
const DESKTOP_LANGUAGE_VARIABLE: &str = "HELIKODU_DESKTOP_LANGUAGE";

/// The `[appearance] language` setting this run started with.
static LANGUAGE_AT_START: OnceLock<String> = OnceLock::new();

/// Selects the program's translations. GTK selects the user's locale when it
/// starts. Missing translations are not an error: the English source strings
/// show.
pub fn init() {
    let _ = bindtextdomain(DOMAIN, locale_dir());
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);
}

/// The folder with the compiled translations (`<language>/LC_MESSAGES/helikodu.mo`):
/// the folder named by `HELIKODU_LOCALEDIR` when it is set; else `share/locale`
/// beside the program's `bin` folder when it holds Helikodu's translations, as
/// in an unpacked ready-built release; else where the build installs them.
fn locale_dir() -> PathBuf {
    choose_locale_dir(
        env::var_os(LOCALE_DIR_VARIABLE),
        env::current_exe().ok().as_deref(),
        Path::new(LOCALE_DIR),
        has_translations,
    )
}

fn choose_locale_dir(
    variable: Option<OsString>,
    program: Option<&Path>,
    built_in: &Path,
    has_translations: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(dir) = variable.filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    let beside = program
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|prefix| prefix.join("share").join("locale"));
    match beside {
        Some(dir) if dir != built_in && has_translations(&dir) => dir,
        _ => built_in.to_path_buf(),
    }
}

/// Whether `dir` holds Helikodu's translations for at least one language.
fn has_translations(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .path()
            .join("LC_MESSAGES")
            .join(format!("{DOMAIN}.mo"))
            .is_file()
    })
}

/// Makes the program run in the interface language `language` (the
/// `[appearance] language` setting: a locale name such as `de` or `pt_BR`, or
/// empty for the desktop's). When the environment does not say that language
/// yet, starts the program again at once with the same arguments, `LANGUAGE`
/// set and the desktop's kept in `HELIKODU_DESKTOP_LANGUAGE`; this returns only
/// when nothing had to change or the new start failed.
///
/// Call it at the very start of `main`, before GTK starts.
pub fn follow_language_setting(language: &str) {
    let language = language.trim();
    let _ = LANGUAGE_AT_START.set(language.to_owned());
    let Some(changes) = language_changes(
        language,
        env::var_os(LANGUAGE_VARIABLE).as_deref(),
        env::var_os(DESKTOP_LANGUAGE_VARIABLE).as_deref(),
    ) else {
        return;
    };
    let Ok(program) = env::current_exe() else {
        return;
    };
    let mut args = env::args_os();
    let mut command = std::process::Command::new(program);
    if let Some(name) = args.next() {
        command.arg0(name);
    }
    command.args(args);
    for (variable, value) in changes {
        match value {
            Some(value) => command.env(variable, value),
            None => command.env_remove(variable),
        };
    }
    // Returns only if the program could not be started.
    let error = command.exec();
    log::warn!("cannot start again in the chosen language: {error}");
}

/// A change of an environment variable: its new value, or `None` to remove it.
type Change = (&'static str, Option<OsString>);

/// The changes to the environment that make gettext use `language` (empty: the
/// desktop's), given `LANGUAGE` and `HELIKODU_DESKTOP_LANGUAGE` as they are;
/// `None` when it uses it already.
fn language_changes(
    language: &str,
    current: Option<&OsStr>,
    desktop: Option<&OsStr>,
) -> Option<Vec<Change>> {
    let current = current.unwrap_or_default();
    if language.is_empty() {
        // Back to the desktop's language after a run in another.
        let desktop = desktop?;
        if current == desktop {
            return None;
        }
        let restored = (!desktop.is_empty()).then(|| desktop.to_os_string());
        return Some(vec![
            (LANGUAGE_VARIABLE, restored),
            (DESKTOP_LANGUAGE_VARIABLE, None),
        ]);
    }
    if current == OsStr::new(language) {
        return None;
    }
    let kept = desktop.unwrap_or(current).to_os_string();
    Some(vec![
        (LANGUAGE_VARIABLE, Some(OsString::from(language))),
        (DESKTOP_LANGUAGE_VARIABLE, Some(kept)),
    ])
}

/// The `[appearance] language` setting this run started with: the one
/// [`follow_language_setting`] was given, or else `now`, the first time this
/// is asked.
pub(crate) fn language_at_start(now: &str) -> String {
    LANGUAGE_AT_START.get_or_init(|| now.to_owned()).clone()
}

/// The desktop's language as a locale name (`de_DE.UTF-8`, `pt_BR`), whatever
/// Helikodu itself runs in: the first entry of the desktop's `LANGUAGE`, else
/// the locale of `LC_ALL`, `LC_MESSAGES` or `LANG`, the first that is set; `C`
/// when none is, or when it is the C locale (which ignores `LANGUAGE`).
pub(crate) fn desktop_language() -> String {
    let variable = |name: &str| env::var(name).unwrap_or_default();
    let language = match env::var_os(DESKTOP_LANGUAGE_VARIABLE) {
        Some(desktop) => desktop.to_string_lossy().into_owned(),
        None => variable(LANGUAGE_VARIABLE),
    };
    desktop_language_from(
        &language,
        &[
            variable("LC_ALL"),
            variable("LC_MESSAGES"),
            variable("LANG"),
        ],
    )
}

fn desktop_language_from(language: &str, locales: &[String]) -> String {
    let locale = locales
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("C");
    if locale == "C" || locale == "POSIX" {
        return "C".to_owned();
    }
    language
        .split(':')
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(locale)
        .to_owned()
}

/// The translation of `text`.
pub(crate) fn tr(text: &str) -> String {
    gettextrs::gettext(text)
}

/// The translation of `singular` or `plural` for `n`.
pub(crate) fn ntr(singular: &str, plural: &str, n: usize) -> String {
    gettextrs::ngettext(singular, plural, u32::try_from(n).unwrap_or(u32::MAX))
}

/// Fills the placeholders of a translated pattern. Translators see the whole
/// sentence with its placeholders.
///
/// `{}` takes the next of `values` in order. `{0}`, `{1}`, … take the first,
/// second, … value wherever they stand, as in Rust's format strings, so that
/// a translation can change the order (a pattern uses one kind or the
/// other). A number without a value gives nothing; other text in braces, such
/// as `{skip}`, stays as it is.
pub(crate) fn fill(pattern: &str, values: &[&str]) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut next = values.iter();
    let mut rest = pattern;
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if after.as_bytes().get(digits) != Some(&b'}') {
            out.push('{');
            rest = after;
            continue;
        }
        let value = if digits == 0 {
            next.next().copied()
        } else {
            after[..digits]
                .parse::<usize>()
                .ok()
                .and_then(|i| values.get(i).copied())
        };
        out.push_str(value.unwrap_or(""));
        rest = &after[digits + 1..];
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
        assert_eq!(fill("{} and {}", &["a"]), "a and ");
    }

    #[test]
    fn numbered_placeholders_name_the_values() {
        assert_eq!(fill("{1} von {0}", &["1:42", "4:55"]), "4:55 von 1:42");
        assert_eq!(fill("{0}{0}", &["ab"]), "abab");
        assert_eq!(
            fill("{2}: {0} / {1}", &["44.1", "16", "FLAC"]),
            "FLAC: 44.1 / 16"
        );
        assert_eq!(fill("{00}", &["a"]), "a");
        // Numbers without a value give nothing.
        assert_eq!(fill("[{2}|{3}]", &["a", "b"]), "[|]");
        assert_eq!(fill("[{99999999999999999999999}]", &["a"]), "[]");
    }

    #[test]
    fn other_braces_stay() {
        assert_eq!(
            fill("{albumartist}/{} - {skip}", &["1999"]),
            "{albumartist}/1999 - {skip}"
        );
        assert_eq!(fill("{ } {1a} {", &["x"]), "{ } {1a} {");
        assert_eq!(fill("{{}}", &["x"]), "{x}");
        assert_eq!(fill("ä{1}ö{}ü", &["x", "y"]), "äyöxü");
    }

    #[test]
    fn the_translations_folder_is_found() {
        let built_in = Path::new("/usr/local/share/locale");
        let program = Path::new("/opt/helikodu-0.9.0/bin/helikodu");
        let none = |_: &Path| false;
        let all = |_: &Path| true;
        // The variable comes first, unless it is empty.
        assert_eq!(
            choose_locale_dir(Some("/tmp/try".into()), Some(program), built_in, all),
            PathBuf::from("/tmp/try")
        );
        assert_eq!(
            choose_locale_dir(Some("".into()), None, built_in, all),
            built_in
        );
        // Beside the program when it has translations there.
        assert_eq!(
            choose_locale_dir(None, Some(program), built_in, all),
            PathBuf::from("/opt/helikodu-0.9.0/share/locale")
        );
        assert_eq!(
            choose_locale_dir(None, Some(program), built_in, none),
            built_in
        );
        assert_eq!(choose_locale_dir(None, None, built_in, all), built_in);
    }

    #[test]
    fn translations_are_recognised_by_their_files() {
        let dir = env::temp_dir().join(format!("helikodu-i18n-test-{}", std::process::id()));
        let messages = dir.join("de").join("LC_MESSAGES");
        std::fs::create_dir_all(&messages).unwrap();
        assert!(!has_translations(&dir));
        std::fs::write(messages.join("gtk40.mo"), b"").unwrap();
        assert!(!has_translations(&dir));
        std::fs::write(messages.join("helikodu.mo"), b"").unwrap();
        assert!(has_translations(&dir));
        assert!(!has_translations(&dir.join("missing")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn os(text: &str) -> Option<&OsStr> {
        Some(OsStr::new(text))
    }

    fn changed(changes: Option<Vec<Change>>) -> Vec<(&'static str, Option<String>)> {
        changes
            .expect("a change")
            .into_iter()
            .map(|(name, value)| (name, value.map(|v| v.to_string_lossy().into_owned())))
            .collect()
    }

    #[test]
    fn a_chosen_language_sets_language_once() {
        // From the desktop, which had no LANGUAGE.
        assert_eq!(
            changed(language_changes("de", None, None)),
            [
                ("LANGUAGE", Some("de".to_owned())),
                ("HELIKODU_DESKTOP_LANGUAGE", Some(String::new()))
            ]
        );
        // The desktop's own LANGUAGE is kept.
        assert_eq!(
            changed(language_changes("pt_BR", os("sv_SE:sv"), None)),
            [
                ("LANGUAGE", Some("pt_BR".to_owned())),
                ("HELIKODU_DESKTOP_LANGUAGE", Some("sv_SE:sv".to_owned()))
            ]
        );
        // Started again in it: nothing to do.
        assert!(language_changes("de", os("de"), os("")).is_none());
        // The desktop already says it.
        assert!(language_changes("de", os("de"), None).is_none());
        // Another choice after a restart keeps the desktop's value.
        assert_eq!(
            changed(language_changes("fr", os("de"), os("sv_SE:sv"))),
            [
                ("LANGUAGE", Some("fr".to_owned())),
                ("HELIKODU_DESKTOP_LANGUAGE", Some("sv_SE:sv".to_owned()))
            ]
        );
    }

    #[test]
    fn the_desktop_language_comes_back() {
        // Never changed: nothing to do.
        assert!(language_changes("", None, None).is_none());
        assert!(language_changes("", os("sv"), None).is_none());
        // After a run in German, the desktop had no LANGUAGE.
        assert_eq!(
            changed(language_changes("", os("de"), os(""))),
            [("LANGUAGE", None), ("HELIKODU_DESKTOP_LANGUAGE", None)]
        );
        // ... or had its own.
        assert_eq!(
            changed(language_changes("", os("de"), os("sv_SE:sv"))),
            [
                ("LANGUAGE", Some("sv_SE:sv".to_owned())),
                ("HELIKODU_DESKTOP_LANGUAGE", None)
            ]
        );
        assert!(language_changes("", os("sv"), os("sv")).is_none());
    }

    #[test]
    fn the_desktop_language_is_named() {
        let locales = |all: &str, messages: &str, lang: &str| {
            vec![all.to_owned(), messages.to_owned(), lang.to_owned()]
        };
        assert_eq!(
            desktop_language_from("", &locales("", "", "de_DE.UTF-8")),
            "de_DE.UTF-8"
        );
        assert_eq!(
            desktop_language_from("", &locales("", "fr_FR.UTF-8", "de_DE.UTF-8")),
            "fr_FR.UTF-8"
        );
        assert_eq!(
            desktop_language_from(":pt_BR:pt", &locales("", "", "de_DE.UTF-8")),
            "pt_BR"
        );
        // The C locale ignores LANGUAGE.
        assert_eq!(desktop_language_from("de", &locales("C", "", "")), "C");
        assert_eq!(desktop_language_from("", &locales("", "", "")), "C");
    }
}
