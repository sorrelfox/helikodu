// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The library settings: the `[library]` table.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SettingsError, check_range};

/// The library folders and how they are scanned: the `[library]` table.
///
/// ```toml
/// [library]
/// folders = ["/home/me/Music"]   # a path that is not UTF-8 is an array of bytes
/// watch = true
/// rescan_interval_min = 30       # folders that cannot be watched
/// skip = ["*.part", "Podcasts/"]
/// follow_symlinks = false
/// include_video = false
/// hide_missing_drives = true
/// missing_grace_days = 30        # a missing track leaves the library after this
/// import_playlists = false       # pick up playlist files found in the folders
/// analyze_in_background = true   # the track analysis after scans
///
/// [library.duplicates]
/// on_scan = "add"                # or "skip": copies get no tracks of their own
/// show_one = false               # show only the main copy of each group
///
/// [library.managed]               # off while `folder` is not set
/// folder = "/home/me/Music/Library"
/// pattern = "{albumartist}/{album}/[{disc}-]{track} {title}"
/// mode = "copy"                  # or "move": what importing does with the files
/// characters = "portable"        # or "as-is", "ascii"
///
/// [library.storage]
/// covers_mib = 2048
/// analysis_mib = 1024
/// logs_mib = 50
/// undo_days = 90
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySettings {
    /// The library folders, as the bytes the kernel returned.
    #[serde(with = "crate::path_serde::text_list")]
    pub folders: Vec<PathBuf>,
    /// Whether the folders are watched for changes while the program runs.
    pub watch: bool,
    /// How often a folder that cannot be watched (network and FUSE file systems) is
    /// scanned again, in minutes (1 to 1440).
    pub rescan_interval_min: u32,
    /// Name patterns to leave out, with `*` and `?`; a pattern with a `/` is a path.
    /// The settings page shows them separated by semicolons.
    pub skip: Vec<String>,
    /// Whether symbolic links are followed out of the library folders.
    pub follow_symlinks: bool,
    /// Whether the audio of video files is added to the library.
    pub include_video: bool,
    /// Whether the tracks of a library folder on a drive that is not connected are
    /// hidden until it is back.
    pub hide_missing_drives: bool,
    /// How many days (1 to 3650) a track whose file disappeared stays in the
    /// library, in case it comes back.
    pub missing_grace_days: u32,
    /// Whether playlist files (M3U, M3U8, PLS, XSPF) found in the library folders
    /// become playlists that follow their files.
    pub import_playlists: bool,
    /// Whether the tracks are analysed in the background (silence and loudness
    /// envelope), one at a time at the lowest priority, after scans and at start.
    pub analyze_in_background: bool,
    /// How copies (byte-identical files) are treated.
    pub duplicates: DuplicateSettings,
    /// Size limits of the caches and stores.
    pub storage: StorageLimits,
    /// The managed media folder, which imported files are copied or moved into.
    pub managed: ManagedFolderSettings,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            watch: true,
            rescan_interval_min: 30,
            skip: Vec::new(),
            follow_symlinks: false,
            include_video: false,
            hide_missing_drives: true,
            missing_grace_days: 30,
            import_playlists: false,
            analyze_in_background: true,
            duplicates: DuplicateSettings::default(),
            storage: StorageLimits::default(),
            managed: ManagedFolderSettings::default(),
        }
    }
}

impl LibrarySettings {
    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_range(
            "library.rescan_interval_min",
            self.rescan_interval_min,
            1..=1440,
        )?;
        check_range(
            "library.missing_grace_days",
            self.missing_grace_days,
            1..=3650,
        )?;
        if let Some(relative) = self.folders.iter().find(|path| !path.is_absolute()) {
            return Err(SettingsError {
                key: "library.folders".into(),
                problem: format!(
                    "holds {}, which is not an absolute path",
                    relative.display()
                ),
            });
        }
        if self.skip.iter().any(|pattern| pattern.trim().is_empty()) {
            return Err(SettingsError {
                key: "library.skip".into(),
                problem: "holds an empty pattern".into(),
            });
        }
        self.storage.validate()?;
        self.managed.validate()
    }
}

/// What importing into the managed folder does with the files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedMode {
    /// The files are copied; the originals stay where they are.
    #[default]
    Copy,
    /// The files are moved: copied, checked, and only then removed from where
    /// they were.
    Move,
}

/// The managed media folder: the `[library.managed]` table. Off by default; on
/// while [`folder`](Self::folder) is set. The folder is in a library folder, or
/// is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedFolderSettings {
    /// The folder; `None` while the managed folder is off.
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::path_serde::opt_text"
    )]
    pub folder: Option<PathBuf>,
    /// The naming pattern the files get below the folder (the organizer's).
    pub pattern: String,
    /// Whether importing copies or moves the files.
    pub mode: ManagedMode,
    /// Which characters names may hold: `portable` (names that Windows can
    /// read), `as-is` or `ascii`.
    pub characters: String,
}

impl ManagedFolderSettings {
    /// The pattern the managed folder starts with: the organizer's first.
    pub const DEFAULT_PATTERN: &'static str = "{albumartist}/{album}/[{disc}-]{track} {title}";

    /// Whether the managed folder is on.
    pub fn is_on(&self) -> bool {
        self.folder.is_some()
    }

    fn validate(&self) -> Result<(), SettingsError> {
        if let Some(folder) = &self.folder
            && !folder.is_absolute()
        {
            return Err(SettingsError {
                key: "library.managed.folder".into(),
                problem: format!("is {}, which is not an absolute path", folder.display()),
            });
        }
        if self.pattern.trim().is_empty() {
            return Err(SettingsError {
                key: "library.managed.pattern".into(),
                problem: "is empty".into(),
            });
        }
        Ok(())
    }
}

impl Default for ManagedFolderSettings {
    fn default() -> Self {
        Self {
            folder: None,
            pattern: Self::DEFAULT_PATTERN.to_owned(),
            mode: ManagedMode::Copy,
            characters: "portable".to_owned(),
        }
    }
}

/// What a scan does with a copy: a file whose bytes are identical to another
/// library file's (content and tags).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DuplicateScan {
    /// Copies get tracks like any other file.
    #[default]
    Add,
    /// Copies are recorded, but get no tracks: they appear in no list, count or
    /// search, and one of them takes over when the main copy disappears.
    Skip,
}

/// How copies are treated: the `[library.duplicates]` table.
///
/// Copies of one file share one track identity (ratings, plays, history, playlist
/// entries), and one of them is the main copy, which plays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct DuplicateSettings {
    /// What a scan does with copies. A change applies to the library at once.
    pub on_scan: DuplicateScan,
    /// Whether lists, counts, search and statistics show only the main copy of each
    /// group (meaningful with [`DuplicateScan::Add`]). The folder view still shows
    /// every file.
    pub show_one: bool,
}

/// How much the caches and stores may hold: the `[library.storage]` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageLimits {
    /// Cover thumbnails, in MiB.
    pub covers_mib: u32,
    /// Analysis data (waveforms), in MiB.
    pub analysis_mib: u32,
    /// Log files, in MiB.
    pub logs_mib: u32,
    /// How long undo history is kept, in days.
    pub undo_days: u32,
}

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            covers_mib: 2048,
            analysis_mib: 1024,
            logs_mib: 50,
            undo_days: 90,
        }
    }
}

impl StorageLimits {
    fn validate(&self) -> Result<(), SettingsError> {
        check_range("library.storage.covers_mib", self.covers_mib, 16..=1 << 20)?;
        check_range(
            "library.storage.analysis_mib",
            self.analysis_mib,
            16..=1 << 20,
        )?;
        check_range("library.storage.logs_mib", self.logs_mib, 1..=1 << 14)?;
        check_range("library.storage.undo_days", self.undo_days, 1..=3650)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use super::*;
    use crate::path_serde::from_bytes;
    use crate::settings::Settings;

    #[test]
    fn folders_keep_their_bytes() {
        let mut settings = Settings::default();
        settings.library.folders = vec![
            PathBuf::from("/home/me/Music"),
            from_bytes(b"/mnt/Caf\xe9".to_vec()),
        ];
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains(r#"folders = ["/home/me/Music", ["#), "{text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back, settings);
        assert_eq!(
            back.library.folders[1].as_os_str().as_bytes(),
            b"/mnt/Caf\xe9"
        );
    }

    #[test]
    fn the_design_defaults() {
        let library = LibrarySettings::default();
        assert!(library.watch && library.hide_missing_drives);
        assert!(!library.follow_symlinks && !library.include_video);
        assert_eq!(library.rescan_interval_min, 30);
        assert_eq!(library.storage.covers_mib, 2048);
        assert_eq!(library.validate(), Ok(()));
    }

    #[test]
    fn duplicate_settings_read_and_write() {
        let settings: Settings =
            toml::from_str("[library.duplicates]\non_scan = \"skip\"\nshow_one = true\n").unwrap();
        assert_eq!(
            settings.library.duplicates,
            DuplicateSettings {
                on_scan: DuplicateScan::Skip,
                show_one: true,
            }
        );
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("[library.duplicates]"), "{text}");
        assert!(text.contains("on_scan = \"skip\""), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
        assert_eq!(
            LibrarySettings::default().duplicates,
            DuplicateSettings {
                on_scan: DuplicateScan::Add,
                show_one: false,
            }
        );
    }

    #[test]
    fn the_managed_folder_is_off_by_default_and_reads_back() {
        let library = LibrarySettings::default();
        assert!(!library.managed.is_on());
        assert_eq!(library.managed.mode, ManagedMode::Copy);
        assert_eq!(
            library.managed.pattern,
            ManagedFolderSettings::DEFAULT_PATTERN
        );
        let text = toml::to_string(&Settings::default()).unwrap();
        assert!(!text.contains("folder = "), "{text}");
        let settings: Settings =
            toml::from_str("[library.managed]\nfolder = \"/music/Library\"\nmode = \"move\"\n")
                .unwrap();
        assert_eq!(
            settings.library.managed.folder.as_deref(),
            Some(std::path::Path::new("/music/Library"))
        );
        assert_eq!(settings.library.managed.mode, ManagedMode::Move);
        assert_eq!(settings.library.managed.characters, "portable");
        let text = toml::to_string(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
        let mut relative = settings.library.clone();
        relative.managed.folder = Some("Library".into());
        assert_eq!(
            relative.validate().unwrap_err().key,
            "library.managed.folder"
        );
    }

    #[test]
    fn invalid_values_are_named() {
        let mut library = LibrarySettings {
            folders: vec!["Music".into()],
            ..LibrarySettings::default()
        };
        assert_eq!(library.validate().unwrap_err().key, "library.folders");
        library.folders.clear();
        library.rescan_interval_min = 0;
        assert_eq!(
            library.validate().unwrap_err().key,
            "library.rescan_interval_min"
        );
        library.rescan_interval_min = 30;
        library.skip = vec![" ".into()];
        assert_eq!(library.validate().unwrap_err().key, "library.skip");
    }
}
