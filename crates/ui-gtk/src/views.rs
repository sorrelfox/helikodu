// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The views of the content area.

pub(crate) mod actions;
pub(crate) mod album;
pub(crate) mod album_tile;
pub(crate) mod albums;
pub(crate) mod artists;
pub(crate) mod drag;
pub(crate) mod folders;
pub(crate) mod genres;
pub(crate) mod history;
pub(crate) mod now_playing;
pub(crate) mod placeholder;
pub(crate) mod playlist;
pub(crate) mod search;
pub(crate) mod table;
pub(crate) mod tracks;

pub(crate) use album::AlbumPage;
pub(crate) use albums::AlbumsView;
