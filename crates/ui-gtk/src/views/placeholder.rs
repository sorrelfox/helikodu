// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The stand-in for a library view that is not built yet: the empty-state
//! block with the view's name.

use crate::i18n::tr;
use crate::widgets::empty_state::empty_state;

/// The stand-in page of the view `title`.
pub(crate) fn placeholder(icon: &str, title: &str) -> gtk::Box {
    empty_state(
        icon,
        title,
        &tr("This view comes in a later version. Albums are ready to browse and play."),
        &[],
    )
}
