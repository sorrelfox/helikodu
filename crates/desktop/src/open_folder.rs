// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! "Open containing folder": shows a file in the file manager.
//!
//! Outside a sandbox the file manager is asked through
//! `org.freedesktop.FileManager1.ShowItems`, which opens the folder with the
//! file selected (Files, Dolphin, Nemo, Caja, Thunar and others implement it,
//! and the bus starts them when needed). When that fails, and always inside a
//! sandbox, the OpenURI portal's `OpenDirectory` is used, which needs no
//! permission: the file is passed as a descriptor and the portal opens its
//! folder in the default file manager.
//!
//! Both calls can take a while when the file manager has to start, so
//! [`show_in_folder`] blocks; [`show_in_folder_later`] runs it on a thread of its
//! own. A Wayland activation token lets the file manager's window take focus.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use ashpd::desktop::open_uri::OpenDirectoryRequest;
use thiserror::Error;
use zbus::Connection;

use crate::mpris::file_uri;

/// How long the file manager or portal may take to answer a call (the file
/// manager may have to start first).
const METHOD_TIMEOUT: Duration = Duration::from_secs(10);

/// Why the folder could not be shown.
#[derive(Debug, Error)]
pub enum OpenFolderError {
    /// The file cannot be opened (for the portal) or has no absolute path.
    #[error("{path}: {source}")]
    File {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The portal failed (after the file manager, if it was tried).
    #[error("OpenURI portal: {0}")]
    Portal(#[from] ashpd::Error),
    /// The session bus could not be used.
    #[error("D-Bus: {0}")]
    Bus(#[from] zbus::Error),
}

/// Shows `path` in the file manager. Blocks until the request is accepted.
pub fn show_in_folder(path: &Path, activation_token: Option<&str>) -> Result<(), OpenFolderError> {
    let path = std::path::absolute(path).map_err(|source| OpenFolderError::File {
        path: path.to_owned(),
        source,
    })?;
    zbus::block_on(async {
        let connection = zbus::connection::Builder::session()
            .method_timeout(METHOD_TIMEOUT)
            .build()
            .await?;
        if !ashpd::is_sandboxed()
            && show_items(&connection, &path, activation_token)
                .await
                .is_ok()
        {
            return Ok(());
        }
        let file = File::open(&path).map_err(|source| OpenFolderError::File {
            path: path.clone(),
            source,
        })?;
        OpenDirectoryRequest::default()
            .activation_token(activation_token.map(ashpd::ActivationToken::from))
            .connection(Some(connection))
            .send(&file)
            .await?;
        Ok(())
    })
}

/// [`show_in_folder`] on a thread of its own; `done` gets the result there.
pub fn show_in_folder_later(
    path: PathBuf,
    activation_token: Option<String>,
    done: impl FnOnce(Result<(), OpenFolderError>) + Send + 'static,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("helikodu-open-folder".into())
        .spawn(move || done(show_in_folder(&path, activation_token.as_deref())))?;
    Ok(())
}

/// `org.freedesktop.FileManager1.ShowItems`.
async fn show_items(
    connection: &Connection,
    path: &Path,
    activation_token: Option<&str>,
) -> zbus::Result<()> {
    let uri = file_uri(path).ok_or_else(|| zbus::Error::Failure("relative path".into()))?;
    connection
        .call_method(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            Some("org.freedesktop.FileManager1"),
            "ShowItems",
            &(vec![uri], activation_token.unwrap_or_default()),
        )
        .await?;
    Ok(())
}
