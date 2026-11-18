// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What MPRIS asks of the library: the details of the queue's entries for the
//! track list, and the playlists. Both run on zbus's thread.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use helikodu_desktop::mpris::{MprisPlaylist, PlaylistSource, TrackDetails};
use helikodu_model::library::TrackRef;
use helikodu_service::Service;
use helikodu_service::library::ItemContent;

use super::super::{PlayerRequest, PlaylistKey, TrackKey, Update};

/// How long a client waits for the details of the track list at most.
const DESCRIBE_WAIT: Duration = Duration::from_secs(2);

/// The details of `tracks`, as the queue panel shows them; empty when the
/// library does not answer in time.
pub(super) fn describe(service: &Service, tracks: &[TrackRef]) -> Vec<TrackDetails> {
    let Some(library) = service.library() else {
        return Vec::new();
    };
    let (sender, receiver) = mpsc::channel();
    if library
        .describe(tracks.to_vec(), move |infos| {
            let _ = sender.send(infos);
        })
        .is_err()
    {
        return Vec::new();
    }
    match receiver.recv_timeout(DESCRIBE_WAIT) {
        Ok(Ok(infos)) => infos.iter().map(TrackDetails::from).collect(),
        _ => Vec::new(),
    }
}

/// The library's playlists for MPRIS; activating one plays it as the window
/// would, through the confirmation for a fixed-volume output.
pub(super) struct Playlists {
    pub(super) service: Arc<Service>,
    pub(super) updates: async_channel::Sender<Update>,
}

impl PlaylistSource for Playlists {
    fn playlists(&self) -> Vec<MprisPlaylist> {
        let Some(library) = self.service.library() else {
            return Vec::new();
        };
        library
            .library()
            .playlists()
            .unwrap_or_default()
            .into_iter()
            .map(|p| MprisPlaylist {
                id: u64::try_from(p.id.get()).unwrap_or(0),
                name: p.name,
                created: Some(p.created),
                modified: Some(p.modified),
            })
            .collect()
    }

    fn activate(&self, id: u64) {
        let Some(library) = self.service.library() else {
            return;
        };
        let music = library.library();
        let Some(playlist) = music
            .playlists()
            .unwrap_or_default()
            .into_iter()
            .find(|p| u64::try_from(p.id.get()).ok() == Some(id))
        else {
            return;
        };
        let tracks: Vec<TrackKey> = music
            .playlist_items(playlist.id)
            .unwrap_or_default()
            .iter()
            .filter_map(|item| match &item.content {
                ItemContent::Track(track) => Some(TrackKey(track.id.get())),
                ItemContent::Dead(_) => None,
            })
            .collect();
        let _ = self
            .updates
            .try_send(Update::Request(PlayerRequest::PlayPlaylist {
                key: PlaylistKey(id),
                tracks,
                start: 0,
                title: playlist.name,
                shuffle: false,
            }));
    }
}
