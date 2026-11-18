// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Playback group of the tag editor's File info tab: "Leave out when
//! shuffling", "Play only when I start it myself" and "Volume for this track".
//! They are kept in the library, not in the file: each change goes to the
//! player at once and is not part of "Save". Only library tracks have them.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::common::switch_row;
use crate::backend::{Backend, PlayerRequest, TrackFlags, TrackKey};
use crate::i18n::tr;
use crate::widgets::bipolar_scale::decibels;
use crate::widgets::{BipolarScale, label};

/// The group for `track`, or `None` when it is not in the library. The
/// controls are insensitive until the flags have been read.
pub(crate) fn playback_group(backend: &Backend, track: TrackKey) -> Option<gtk::Box> {
    if track.0 == 0 {
        return None;
    }
    let group = gtk::Box::new(gtk::Orientation::Vertical, 8);
    group.set_widget_name("track-playback");
    group.append(&label(&tr("Playback"), &["hk-section-label"]));
    let (skip_row, skip) = switch_row(&tr("Leave out when shuffling"), None);
    group.append(&skip_row);
    let (manual_row, manual) = switch_row(&tr("Play only when I start it myself"), None);
    group.append(&manual_row);

    let volume = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = tr("Volume for this track");
    let name = label(&title, &[]);
    name.set_hexpand(true);
    head.append(&name);
    let shown = label(&decibels(0.0), &["hk-mono", "hk-strong"]);
    head.append(&shown);
    volume.append(&head);
    let scale = BipolarScale::default();
    scale.update_property(&[gtk::accessible::Property::Label(&title)]);
    volume.append(&scale);
    let ends = gtk::CenterBox::new();
    ends.set_start_widget(Some(&label("−12", &["hk-mono-meta", "hk-t3"])));
    ends.set_center_widget(Some(&label(&decibels(0.0), &["hk-mono-meta", "hk-t3"])));
    ends.set_end_widget(Some(&label("+12", &["hk-mono-meta", "hk-t3"])));
    volume.append(&ends);
    group.append(&volume);
    for widget in [
        skip_row.upcast_ref::<gtk::Widget>(),
        manual_row.upcast_ref(),
        volume.upcast_ref(),
    ] {
        widget.set_sensitive(false);
    }

    let flags = Rc::new(Cell::new(TrackFlags::default()));
    let send = {
        let backend = backend.clone();
        let flags = flags.clone();
        move |change: &dyn Fn(&mut TrackFlags)| {
            let mut now = flags.get();
            change(&mut now);
            if now != flags.get() {
                flags.set(now);
                backend.send(PlayerRequest::SetTrackFlags { track, flags: now });
            }
        }
    };
    let s = send.clone();
    skip.connect_active_notify(move |switch| {
        let on = switch.is_active();
        s(&|f| f.skip_in_shuffle = on);
    });
    let s = send.clone();
    manual.connect_active_notify(move |switch| {
        let on = switch.is_active();
        s(&|f| f.manual_only = on);
    });
    let text = shown.clone();
    scale.connect_changed(move |db| {
        text.set_text(&decibels(db));
        send(&|f| *f = f.with_volume_db(db));
    });

    let services = backend.services();
    let backend = backend.clone();
    let rows = (skip_row, manual_row, volume);
    glib::spawn_future_local(async move {
        let read = backend
            .work(move || services.track_flags(track))
            .await
            .flatten();
        let Some(read) = read else { return };
        // The controls show the flags before they are connected to the
        // player: setting them sends nothing new.
        flags.set(read);
        skip.set_active(read.skip_in_shuffle);
        manual.set_active(read.manual_only);
        scale.set_value(read.volume_db());
        shown.set_text(&decibels(read.volume_db()));
        rows.0.set_sensitive(true);
        rows.1.set_sensitive(true);
        rows.2.set_sensitive(true);
    });
    Some(group)
}
