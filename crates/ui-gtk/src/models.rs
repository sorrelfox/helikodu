// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! ID-backed list models. A model holds keys only; item objects are made when
//! GTK asks for a position, and their row data is fetched in batches on a
//! worker thread when a widget is bound to them.

pub(crate) mod album_list;
pub(crate) mod covers;

pub(crate) use album_list::{AlbumItem, AlbumListModel};
pub(crate) use covers::CoverCache;
