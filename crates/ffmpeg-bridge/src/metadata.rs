// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tags, chapters and attached pictures, as FFmpeg reads them.

pub(crate) mod chapters;
pub(crate) mod pictures;
mod tags;

pub use chapters::Chapter;
pub use pictures::{AttachedPicture, PictureLimits, PictureSource};
pub use tags::Tags;
