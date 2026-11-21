// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The stylesheet providers: the tokens and the stylesheet from the resource
//! bundle, and a small second provider that [`theme`](super::theme) rewrites
//! when the look changes.

use gtk::gdk;

use super::theme::{Look, look_css};

/// The providers, added to the display once.
#[derive(Debug)]
pub(crate) struct Providers {
    look: gtk::CssProvider,
}

impl Providers {
    pub(crate) fn install(display: &gdk::Display) -> Self {
        let priority = gtk::STYLE_PROVIDER_PRIORITY_APPLICATION;
        for file in [
            "tokens.css",
            "helikodu.css",
            "dialogs.css",
            "views.css",
            "lyrics.css",
            "playmodes.css",
            "tools.css",
            "tags.css",
            "browse.css",
            "playlists.css",
            "generators.css",
            "desktop.css",
            "insights.css",
            "interchange.css",
            "managed.css",
            "labels.css",
            "smartplay.css",
            "dsp.css",
        ] {
            let provider = gtk::CssProvider::new();
            provider.load_from_resource(&format!("{}/style/{file}", crate::RESOURCE_BASE));
            gtk::style_context_add_provider_for_display(display, &provider, priority);
        }
        let look = gtk::CssProvider::new();
        look.load_from_string(&look_css(&Look::default()));
        gtk::style_context_add_provider_for_display(display, &look, priority + 1);
        Self { look }
    }

    pub(crate) fn update(&self, look: &Look) {
        self.look.load_from_string(&look_css(look));
    }
}
