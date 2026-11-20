// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The managed media folder for the front ends: its setting (`[library.managed]`),
//! planning and carrying out imports into it and consolidations, and runs that
//! were cut short. The calls block while files are read or moved, so the front
//! ends make them on threads of their own and announce the change with
//! [`LibraryService::announce_change`](super::LibraryService::announce_change)
//! when they are done.

use std::path::{Path, PathBuf};

use helikodu_library::{
    CancelToken, CharacterSet, ManagedOptions, ManagedPlan, ManagedReport, ManagedRunId,
    ManagedRunSummary, NamingPattern, Progress, Transfer,
};
use helikodu_model::settings::{LibrarySettings, ManagedFolderSettings, ManagedMode};

use super::{LibraryError, MusicLibrary, scan_options};

/// The library folder of `folders` that holds `folder` (the deepest one), or
/// `None`.
fn library_folder_of(folders: &[PathBuf], folder: &Path) -> Option<PathBuf> {
    folders
        .iter()
        .filter(|root| folder.starts_with(root))
        .max_by_key(|root| root.as_os_str().len())
        .cloned()
}

/// Sets the managed folder of `settings` to `folder` (made absolute and created
/// if it does not exist), with `pattern` and `mode` when given; `characters` is
/// an identifier of [`CharacterSet`]. A folder outside every library folder
/// becomes a library folder. Returns whether it did. `None` for `folder` keeps
/// the folder; [`turn_off_managed_folder`] turns it off.
pub fn set_managed_folder(
    settings: &mut LibrarySettings,
    folder: Option<&Path>,
    pattern: Option<&str>,
    mode: Option<ManagedMode>,
    characters: Option<&str>,
) -> Result<bool, LibraryError> {
    if let Some(pattern) = pattern {
        NamingPattern::parse(pattern).map_err(|e| LibraryError::ManagedFolder(e.to_string()))?;
        settings.managed.pattern = pattern.to_owned();
    }
    if let Some(characters) = characters {
        if CharacterSet::from_id(characters).is_none() {
            return Err(LibraryError::ManagedFolder(format!(
                "{characters} is not a character set (portable, as-is or ascii)"
            )));
        }
        characters.clone_into(&mut settings.managed.characters);
    }
    if let Some(mode) = mode {
        settings.managed.mode = mode;
    }
    let mut added = false;
    if let Some(folder) = folder {
        let folder = std::path::absolute(folder).map_err(|source| LibraryError::Io {
            context: format!("cannot use {}", folder.display()),
            source,
        })?;
        std::fs::create_dir_all(&folder).map_err(|source| LibraryError::Io {
            context: format!("cannot create {}", folder.display()),
            source,
        })?;
        if library_folder_of(&settings.folders, &folder).is_none() {
            settings.folders.push(folder.clone());
            added = true;
        }
        settings.managed.folder = Some(folder);
    }
    Ok(added)
}

/// Turns the managed folder of `settings` off. Its files stay where they are and
/// in the library.
pub fn turn_off_managed_folder(settings: &mut LibrarySettings) {
    settings.managed.folder = None;
}

/// The managed folder of `settings` as the library works with it, moving or
/// copying by `transfer` (the setting's mode when `None`). `unknown` is the word
/// written for an empty field.
pub fn managed_options(
    settings: &LibrarySettings,
    transfer: Option<Transfer>,
    unknown: &str,
) -> Result<ManagedOptions, LibraryError> {
    let managed: &ManagedFolderSettings = &settings.managed;
    let folder = managed
        .folder
        .clone()
        .ok_or(LibraryError::ManagedFolderOff)?;
    let library_folder = library_folder_of(&settings.folders, &folder).ok_or_else(|| {
        LibraryError::ManagedFolder(format!("{} is not in a library folder", folder.display()))
    })?;
    Ok(ManagedOptions {
        folder,
        library_folder,
        pattern: managed.pattern.clone(),
        characters: CharacterSet::from_id(&managed.characters).unwrap_or(CharacterSet::Windows),
        unknown: unknown.to_owned(),
        transfer: transfer.unwrap_or(match managed.mode {
            ManagedMode::Copy => Transfer::Copy,
            ManagedMode::Move => Transfer::Move,
        }),
    })
}

impl MusicLibrary {
    /// Plans importing `paths` into the managed folder of `settings` (see
    /// [`managed_options`]); the files are read with the worker program.
    pub fn plan_managed_import(
        &self,
        paths: &[PathBuf],
        settings: &LibrarySettings,
        options: &ManagedOptions,
        worker_program: Option<&Path>,
        cancel: &CancelToken,
    ) -> Result<ManagedPlan, LibraryError> {
        let scan = scan_options(settings, worker_program)?;
        Ok(self
            .inner()
            .plan_managed_import(paths, options, &scan, cancel)?)
    }

    /// How many present library files are outside the managed folder `folder`.
    pub fn managed_files_outside(&self, folder: &Path) -> Result<usize, LibraryError> {
        Ok(self.inner().managed_files_outside(folder)?)
    }

    /// Plans consolidating the library files outside the managed folder into it.
    pub fn plan_consolidate(&self, options: &ManagedOptions) -> Result<ManagedPlan, LibraryError> {
        Ok(self.inner().plan_consolidate(options)?)
    }

    /// Carries out `plan`; the library reads the new files with the scan rules of
    /// `settings` and the worker program.
    pub fn apply_managed(
        &self,
        plan: &ManagedPlan,
        settings: &LibrarySettings,
        worker_program: Option<&Path>,
        progress: Progress<'_>,
        cancel: &CancelToken,
    ) -> Result<ManagedReport, LibraryError> {
        let scan = scan_options(settings, worker_program)?;
        Ok(self.inner().apply_managed(plan, &scan, progress, cancel)?)
    }

    /// The run of the managed folder that was cut short, if there is one.
    pub fn interrupted_managed_run(&self) -> Result<Option<ManagedRunSummary>, LibraryError> {
        Ok(self.inner().interrupted_managed_run()?)
    }

    /// Continues the run `run`, which was cut short.
    pub fn resume_managed_run(
        &self,
        run: ManagedRunId,
        settings: &LibrarySettings,
        worker_program: Option<&Path>,
        progress: Progress<'_>,
        cancel: &CancelToken,
    ) -> Result<ManagedReport, LibraryError> {
        let scan = scan_options(settings, worker_program)?;
        Ok(self
            .inner()
            .resume_managed_run(run, &scan, progress, cancel)?)
    }

    /// Rolls back the run `run`, which was cut short.
    pub fn roll_back_managed_run(
        &self,
        run: ManagedRunId,
        progress: Progress<'_>,
    ) -> Result<ManagedReport, LibraryError> {
        Ok(self.inner().roll_back_managed_run(run, progress)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_outside_the_library_becomes_a_library_folder() {
        let base = std::env::temp_dir().join(format!("helikodu-managed-{}", std::process::id()));
        let mut settings = LibrarySettings {
            folders: vec![base.join("Music")],
            ..LibrarySettings::default()
        };
        assert!(matches!(
            managed_options(&settings, None, "Unknown"),
            Err(LibraryError::ManagedFolderOff)
        ));
        let inside = base.join("Music/Library");
        assert!(!set_managed_folder(&mut settings, Some(&inside), None, None, None).unwrap());
        let options = managed_options(&settings, None, "Unknown").unwrap();
        assert_eq!(options.library_folder, base.join("Music"));
        assert_eq!(options.transfer, Transfer::Copy);
        assert_eq!(options.pattern, ManagedFolderSettings::DEFAULT_PATTERN);
        let outside = base.join("Managed");
        assert!(
            set_managed_folder(
                &mut settings,
                Some(&outside),
                Some("{artist}/{title}"),
                Some(ManagedMode::Move),
                Some("ascii"),
            )
            .unwrap()
        );
        assert!(settings.folders.contains(&outside));
        let options = managed_options(&settings, None, "Unknown").unwrap();
        assert_eq!(options.library_folder, outside);
        assert_eq!(options.transfer, Transfer::Move);
        assert_eq!(options.characters, CharacterSet::Ascii);
        assert!(set_managed_folder(&mut settings, None, Some("{"), None, None).is_err());
        assert!(set_managed_folder(&mut settings, None, None, None, Some("x")).is_err());
        turn_off_managed_folder(&mut settings);
        assert!(!settings.managed.is_on());
        let _ = std::fs::remove_dir_all(&base);
    }
}
