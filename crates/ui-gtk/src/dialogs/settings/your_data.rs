// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! "Your data", the third row of the library database in Settings › Advanced:
//! ratings, play counts, history, playlists, labels and fields in one file.
//! "Export…" asks where to save the file and says so in a toast with "Show";
//! "Import…" opens the Import window with a data file to choose.

use std::rc::Rc;

use gtk::prelude::*;

use super::Context;
use crate::dialogs::common::compact_button;
use crate::i18n::tr;
use crate::widgets::label;

/// The row, with its two buttons.
pub(super) fn row(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.add_css_class("hk-folder-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(&tr("Your data"), &[]));
    let explanation = label(
        &tr("Ratings, play counts, history, playlists, labels and fields in one file."),
        &["hk-t3", "hk-secondary"],
    );
    explanation.set_wrap(true);
    explanation.set_single_line_mode(false);
    explanation.set_ellipsize(gtk::pango::EllipsizeMode::None);
    text.append(&explanation);
    line.append(&text);
    let export = compact_button(&tr("Export…"));
    export.set_widget_name("export-user-data");
    let weak = Rc::downgrade(context);
    export.connect_clicked(move |_| {
        if let Some(context) = weak.upgrade()
            && let Some(main) = context.main.upgrade()
        {
            crate::tools::import::export_user_data_over(&main, &context.window);
        }
    });
    line.append(&export);
    let import = compact_button(&tr("Import…"));
    import.set_widget_name("import-user-data");
    let weak = Rc::downgrade(context);
    import.connect_clicked(move |_| {
        if let Some(context) = weak.upgrade()
            && let Some(main) = context.main.upgrade()
        {
            crate::tools::import::open(&main, Some(crate::tools::import::Start::DataFile));
        }
    });
    line.append(&import);
    line
}
