// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Help › User Guide (F1): opens the copy of the user guide installed with
//! the application, one HTML page, in the desktop's program for web pages.
//! It is a local file, so Help works without a network; no web address is
//! ever opened.

use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::prelude::*;

use crate::i18n::tr;
use crate::window::MainWindow;

/// The page of the guide inside its folder.
const PAGE: &str = "index.html";

/// Where the build installs the guide, unless it says otherwise.
const GUIDE_DIR: &str = match option_env!("HELIKODU_GUIDEDIR") {
    Some(dir) => dir,
    None => "/usr/share/helikodu/guide",
};

/// The installed guide: the folder the build named, or `share/helikodu/guide`
/// beside the program's own `bin/` folder (an installation moved to another
/// prefix).
pub(crate) fn installed_page() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .and_then(Path::parent)
            .map(|prefix| prefix.join("share/helikodu/guide"))
    });
    candidates(Path::new(GUIDE_DIR), beside.as_deref())
        .into_iter()
        .find(|page| page.is_file())
}

/// The pages to try, in order.
fn candidates(built: &Path, beside: Option<&Path>) -> Vec<PathBuf> {
    let mut pages = vec![built.join(PAGE)];
    if let Some(dir) = beside {
        let page = dir.join(PAGE);
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
    pages
}

/// Opens the guide over `main`, or says that it is not installed.
pub(crate) fn open(main: &MainWindow) {
    let Some(page) = installed_page() else {
        main.toast(
            &tr("The user guide is not installed. It comes with the application's installation."),
            true,
        );
        return;
    };
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&page)));
    let weak = main.downgrade();
    launcher.launch(Some(main), None::<&gio::Cancellable>, move |result| {
        if let (Err(error), Some(main)) = (result, weak.upgrade())
            && !error.matches(gtk::DialogError::Dismissed)
        {
            main.toast(&tr("The user guide could not be opened."), true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_folder_comes_first_and_is_not_repeated() {
        let built = Path::new("/usr/share/helikodu/guide");
        assert_eq!(
            candidates(built, Some(Path::new("/opt/helikodu/share/helikodu/guide"))),
            vec![
                PathBuf::from("/usr/share/helikodu/guide/index.html"),
                PathBuf::from("/opt/helikodu/share/helikodu/guide/index.html"),
            ]
        );
        assert_eq!(candidates(built, Some(built)).len(), 1);
        assert_eq!(candidates(built, None).len(), 1);
    }
}
