// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the library database lives, and the sort order it is opened with.

use std::ffi::OsString;
use std::path::PathBuf;

use helikodu_library::LocaleCollation;

use super::LibraryError;

/// The application's folder inside the XDG data folder.
pub const DATA_FOLDER: &str = "helikodu";

/// The file name of the library database.
pub const DATABASE_FILE: &str = "library.db";

/// `$XDG_DATA_HOME/helikodu/library.db`, or `~/.local/share/helikodu/library.db`
/// when `XDG_DATA_HOME` is unset, empty or not an absolute path.
pub fn default_database_path() -> Result<PathBuf, LibraryError> {
    database_path_in(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

fn database_path_in(
    data_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, LibraryError> {
    let data_home = data_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".local/share"))
        })
        .ok_or(LibraryError::NoDataFolder)?;
    Ok(data_home.join(DATA_FOLDER).join(DATABASE_FILE))
}

/// The sort order for the interface language `language` (the `[appearance]
/// language` setting, a locale name such as `de` or `pt_BR`). When it is empty
/// or not a locale, the order of the environment's locale: `LC_ALL`,
/// `LC_COLLATE` or `LANG`, the first that is set and not empty. The root order
/// when none is, or when the locale is unknown.
pub(super) fn collation(language: &str) -> LocaleCollation {
    let language = language.trim();
    if !language.is_empty()
        && let Ok(collation) = LocaleCollation::new(language)
    {
        return collation;
    }
    ["LC_ALL", "LC_COLLATE", "LANG"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .and_then(|value| value.into_string().ok())
        .and_then(|locale| LocaleCollation::new(&locale).ok())
        .unwrap_or_else(LocaleCollation::root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_home_comes_first() {
        let path = database_path_in(Some("/data".into()), Some("/home/a".into())).unwrap();
        assert_eq!(path, PathBuf::from("/data/helikodu/library.db"));
    }

    #[test]
    fn home_is_the_fallback() {
        for data_home in [None, Some("".into()), Some("relative".into())] {
            let path = database_path_in(data_home, Some("/home/a".into())).unwrap();
            assert_eq!(
                path,
                PathBuf::from("/home/a/.local/share/helikodu/library.db")
            );
        }
        assert!(matches!(
            database_path_in(None, None),
            Err(LibraryError::NoDataFolder)
        ));
    }

    #[test]
    fn the_chosen_language_sorts() {
        assert_eq!(collation("de").locale(), "de");
        assert_eq!(collation(" pt_BR ").locale(), "pt-BR");
        assert_eq!(collation("zh_CN").locale(), "zh-CN");
        // Empty or not a locale: the environment's.
        let environment = collation("").locale().to_owned();
        assert_eq!(collation("not a locale!").locale(), environment);
    }
}
