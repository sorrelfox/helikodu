// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Choosing the files some decoders need, through the file chooser (the
//! portal where there is one): the SoundFont for MIDI files and the
//! song-length database of the High Voltage SID Collection. Settings ›
//! Playback and Supported formats use it.

use std::path::PathBuf;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::services::{Services, Settings};
use crate::i18n::{fill, tr};

/// A file a decoder needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecoderFile {
    /// The SoundFont MIDI files are played with.
    SoundFont,
    /// `Songlengths.md5` of the High Voltage SID Collection.
    SidSongLengths,
}

impl DecoderFile {
    /// The title of the file chooser.
    pub(crate) fn title(self) -> String {
        match self {
            Self::SoundFont => tr("Choose SoundFont"),
            Self::SidSongLengths => tr("Choose the song-length database"),
        }
    }

    fn filter(self) -> gtk::FileFilter {
        let filter = gtk::FileFilter::new();
        match self {
            Self::SoundFont => {
                filter.set_name(Some(&tr("SoundFont files (.sf2, .sf3)")));
                filter.add_suffix("sf2");
                filter.add_suffix("sf3");
            }
            Self::SidSongLengths => {
                filter.set_name(Some(&tr("Song-length database (Songlengths.md5)")));
                filter.add_suffix("md5");
                filter.add_suffix("txt");
            }
        }
        filter
    }

    /// The stored file.
    pub(crate) fn get(self, settings: &Settings) -> Option<PathBuf> {
        let decoders = &settings.playback.decoders;
        match self {
            Self::SoundFont => decoders.soundfont.clone(),
            Self::SidSongLengths => decoders.sid_song_lengths.clone(),
        }
    }

    /// Stores `path`, or forgets the file.
    pub(crate) fn set(self, settings: &mut Settings, path: Option<PathBuf>) {
        let decoders = &mut settings.playback.decoders;
        match self {
            Self::SoundFont => decoders.soundfont = path,
            Self::SidSongLengths => decoders.sid_song_lengths = path,
        }
    }
}

/// Asks for `which` over `window` and stores the file chosen. `done` gets the
/// file, or why it was not stored; nothing when the user cancelled.
pub(crate) fn choose(
    window: &gtk::Window,
    services: Arc<dyn Services>,
    which: DecoderFile,
    done: impl FnOnce(Result<PathBuf, String>) + 'static,
) {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&which.filter());
    let dialog = gtk::FileDialog::builder()
        .title(which.title())
        .modal(true)
        .filters(&filters)
        .build();
    if let Some(current) = which.get(&services.settings())
        && let Some(folder) = current.parent()
    {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    let window = window.clone();
    glib::spawn_future_local(async move {
        let Ok(file) = dialog.open_future(Some(&window)).await else {
            return;
        };
        let Some(path) = file.path() else {
            done(Err(tr("Only files on this computer can be used.")));
            return;
        };
        let stored = path.clone();
        match services.update_settings(&mut |s| which.set(s, Some(stored.clone()))) {
            Ok(()) => done(Ok(path)),
            Err(e) => done(Err(fill(&tr("This change was not saved: {}"), &[&e.0]))),
        }
    });
}
