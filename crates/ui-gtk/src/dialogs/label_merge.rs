// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Merging a label into another, opened by "Merge Into…" in the Labels view and
//! when a label is renamed to the name of another: everything that has the
//! label gets the one chosen, and the label disappears. The merge can be
//! undone from its toast.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use super::common::{dialog_window, note};
use crate::backend::LabelRow;
use crate::format;
use crate::i18n::{fill, ntr, tr};

/// The name the dialog is known by among the toplevels.
pub(crate) const NAME: &str = "label-merge";

type Answer = Rc<RefCell<Option<Box<dyn FnOnce(Option<String>)>>>>;

/// Asks into which of `others` the label `from` is merged, with `chosen` chosen
/// first. `answer` gets the name of the label chosen, or `None` for Cancel.
pub(crate) fn ask(
    parent: Option<&gtk::Window>,
    from: &LabelRow,
    others: &[LabelRow],
    chosen: Option<&str>,
    answer: impl FnOnce(Option<String>) + 'static,
) -> gtk::Window {
    let title = fill(&tr("Merge “{}” into another label"), &[&from.name]);
    let window = dialog_window(parent, NAME, &title, (480, -1), true);
    window.set_resizable(false);
    let answer: Answer = Rc::new(RefCell::new(Some(Box::new(answer))));

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("hk-dialog-body");
    body.add_css_class("hk-confirm");
    body.set_size_request(480, 300);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("hk-icon-tile");
    tile.set_valign(gtk::Align::Start);
    let icon = gtk::Image::from_icon_name("helikodu-label-symbolic");
    icon.set_vexpand(true);
    icon.set_valign(gtk::Align::Center);
    tile.append(&icon);
    top.append(&tile);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 7);
    text.set_hexpand(true);
    let heading = gtk::Label::new(Some(&title));
    heading.set_xalign(0.0);
    heading.set_wrap(true);
    heading.add_css_class("hk-dialog-title");
    text.append(&heading);
    let explanation = gtk::Label::new(Some(&fill(
        &tr("Everything that has “{}” gets the label you choose, and “{}” disappears."),
        &[&from.name, &from.name],
    )));
    explanation.set_xalign(0.0);
    explanation.set_wrap(true);
    explanation.set_max_width_chars(46);
    explanation.add_css_class("hk-t2");
    text.append(&explanation);
    let choose = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    choose.set_margin_top(16);
    let caption = gtk::Label::new(Some(&tr("Merge into")));
    caption.add_css_class("hk-t2");
    choose.append(&caption);
    let names: Vec<String> = others
        .iter()
        .map(|o| {
            fill(
                &ntr("{} · {} track", "{} · {} tracks", o.tracks),
                &[&o.name, &format::count(o.tracks)],
            )
        })
        .collect();
    let strings: Vec<&str> = names.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&strings);
    dropdown.add_css_class("hk-input");
    dropdown.set_hexpand(true);
    dropdown.update_property(&[gtk::accessible::Property::Label(&tr("Merge into"))]);
    if let Some(index) = chosen.and_then(|c| {
        others
            .iter()
            .position(|o| o.name.to_lowercase() == c.to_lowercase())
    }) {
        dropdown.set_selected(u32::try_from(index).unwrap_or(0));
    }
    choose.append(&dropdown);
    text.append(&choose);
    top.append(&text);
    body.append(&top);
    let space = gtk::Box::new(gtk::Orientation::Vertical, 0);
    space.set_vexpand(true);
    body.append(&space);

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.add_css_class("hk-dialog-buttons");
    let affected = fill(
        &ntr(
            "{} track is affected.",
            "{} tracks are affected.",
            from.tracks,
        ),
        &[&format::count(from.tracks)],
    );
    let hint = note(&format!("{affected}\n{}", tr("This can be undone.")));
    hint.set_hexpand(true);
    hint.set_valign(gtk::Align::Center);
    bottom.append(&hint);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("hk-btn");
    cancel.set_valign(gtk::Align::Center);
    let merge = gtk::Button::with_label(&tr("Merge"));
    merge.add_css_class("hk-btn");
    merge.add_css_class("primary");
    merge.set_valign(gtk::Align::Center);
    merge.set_sensitive(!others.is_empty());
    bottom.append(&cancel);
    bottom.append(&merge);
    body.append(&bottom);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&merge));

    let names: Vec<String> = others.iter().map(|o| o.name.clone()).collect();
    for (button, yes) in [(&cancel, false), (&merge, true)] {
        let answer = answer.clone();
        let weak = window.downgrade();
        let dropdown = dropdown.clone();
        let names = names.clone();
        button.connect_clicked(move |_| {
            let chosen = yes
                .then(|| {
                    usize::try_from(dropdown.selected())
                        .ok()
                        .and_then(|i| names.get(i).cloned())
                })
                .flatten();
            if let Some(answer) = answer.borrow_mut().take() {
                answer(chosen);
            }
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        });
    }
    window.connect_close_request(move |_| {
        if let Some(answer) = answer.borrow_mut().take() {
            answer(None);
        }
        gtk::glib::Propagation::Proceed
    });
    window.present();
    dropdown.grab_focus();
    window
}
