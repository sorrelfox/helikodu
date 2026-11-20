// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The offer to import from another player: once, after the first library
//! scan finishes, when the library of a player Helikodu can import from is
//! found on the computer, a toast says so ("Rhythmbox’s plays and ratings can
//! be brought over") with "Import…", which opens the Import window with that
//! player chosen (`app.import-from-named-player`, given the player's short
//! name; `app.import-from-player` when several were found). The toast stays
//! until it is dismissed. That the offer was made is kept by the backend, so
//! it is made once, ever.
//!
//! The toast waits for the scan's own toast to go: a toast replaces the one
//! shown.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use super::MainWindow;
use super::toast_overlay::ToastAction;
use crate::backend::services::FoundPlayer;
use crate::backend::{Backend, Update};
use crate::i18n::{fill, tr};
use crate::playback::Playback;

/// The action that opens the Import window.
const IMPORT_ACTION: &str = "import-from-player";
/// The action that opens the Import window with one player chosen.
const NAMED_IMPORT_ACTION: &str = "import-from-named-player";

/// How long the offer waits at most for another toast to go.
const WAIT: u32 = 60;

/// The toast's words for the players `found`.
pub(crate) fn offer_text(found: &[FoundPlayer]) -> String {
    match found {
        [one] => fill(
            &tr("{}’s plays and ratings can be brought over"),
            &[&one.name],
        ),
        _ => tr("Plays and ratings of other players can be brought over"),
    }
}

impl MainWindow {
    /// Makes the offer after the first scan that finishes, if the backend
    /// finds a player's library and has not made it before.
    pub(super) fn watch_import_offer(&self, backend: &Backend, playback: &Rc<Playback>) {
        let asked = Rc::new(Cell::new(false));
        let weak = self.downgrade();
        let services = backend.services();
        playback.connect_event(move |update| {
            let Update::Scan(status) = update else { return };
            if !status.finished || status.error.is_some() || asked.replace(true) {
                return;
            }
            let services = services.clone();
            let weak = weak.clone();
            glib::spawn_future_local(async move {
                let found = {
                    let services = services.clone();
                    gio::spawn_blocking(move || services.import_offer())
                        .await
                        .unwrap_or_default()
                };
                if found.is_empty() {
                    return;
                }
                let _ = gio::spawn_blocking(move || services.mark_import_offered()).await;
                if let Some(window) = weak.upgrade() {
                    window.offer_import_when_free(found, 0);
                }
            });
        });
    }

    /// Shows the offer once no other toast is shown (or after [`WAIT`]
    /// seconds).
    fn offer_import_when_free(&self, found: Vec<FoundPlayer>, waited: u32) {
        let busy = self
            .imp()
            .toasts
            .borrow()
            .as_ref()
            .is_some_and(|t| t.text().is_some());
        if busy && waited < WAIT {
            let weak = self.downgrade();
            glib::timeout_add_local_once(Duration::from_secs(1), move || {
                if let Some(window) = weak.upgrade() {
                    window.offer_import_when_free(found, waited + 1);
                }
            });
            return;
        }
        self.show_import_offer(&found);
    }

    /// Shows the toast that offers the import from `found`.
    fn show_import_offer(&self, found: &[FoundPlayer]) {
        let player = (found.len() == 1).then(|| found[0].id.clone());
        let weak = self.downgrade();
        let action = ToastAction {
            label: tr("Import…"),
            undo: false,
            run: Box::new(move || {
                let Some(application) = weak.upgrade().and_then(|w| w.application()) else {
                    return;
                };
                match player.as_ref() {
                    Some(id) => {
                        application.activate_action(NAMED_IMPORT_ACTION, Some(&id.to_variant()));
                    }
                    None => application.activate_action(IMPORT_ACTION, None),
                }
            }),
        };
        if let Some(toasts) = self.imp().toasts.borrow().as_ref() {
            toasts.show_until_dismissed(&offer_text(found), action);
        }
    }

    /// Shows the offer to import from the player called `name` (for tests
    /// and screenshots).
    #[doc(hidden)]
    pub fn demo_import_offer(&self, name: &str) {
        self.show_import_offer(&[FoundPlayer {
            id: name.to_lowercase(),
            name: name.to_owned(),
        }]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_player_is_named_and_several_are_not() {
        let player = |name: &str| FoundPlayer {
            id: name.to_lowercase(),
            name: name.to_owned(),
        };
        assert_eq!(
            offer_text(&[player("Rhythmbox")]),
            "Rhythmbox’s plays and ratings can be brought over"
        );
        assert_eq!(
            offer_text(&[player("Rhythmbox"), player("Strawberry")]),
            "Plays and ratings of other players can be brought over"
        );
    }
}
