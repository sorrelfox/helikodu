// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The full-output confirmation: before the first playback to a device whose
//! volume is fixed at 100 %, and before a change while playing would move the
//! output there from a lower volume, the player waits and the user is asked,
//! naming the device. The answer is kept in the device's profile, so each
//! device is asked about once.
//!
//! Requests that would start playback pass a gate on the backend; while the
//! question is open nothing is sent to the player, so no sample reaches the
//! device until "Play at full output" is chosen. Cancel is the default and
//! Escape cancels. Playback asked for from outside the window passes the same
//! gate: files opened with the application (`GApplication` open) are sent with
//! [`Backend::send`], and the service backend hands MPRIS `Play`, `PlayPause`
//! and `OpenUri` to the window as [`Update::Request`](crate::backend::Update)
//! instead of to the player.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use helikodu_model::settings::VolumeControl;

use super::common::{dialog_window, note};
use crate::backend::services::{DeviceId, DeviceKey, DeviceSelection, OutputSelection, Settings};
use crate::backend::{Backend, PlaybackState, PlayerRequest};
use crate::i18n::{fill, tr};
use crate::playback::Playback;
use crate::widgets::label;

/// The name the dialog is known by among the toplevels.
pub(crate) const NAME: &str = "volume-confirm";

/// The key of `selection`'s settings profile.
pub(crate) fn device_key(backend: &Backend, selection: &OutputSelection) -> Option<DeviceKey> {
    let kind = backend.services().resolve_backend(selection.backend);
    let id = match &selection.device {
        DeviceSelection::Device(id) => id.clone(),
        DeviceSelection::SystemDefault => DeviceId::new("default").ok()?,
    };
    Some(DeviceKey::new(kind, id))
}

/// The volume control `settings` use for the device of `key`.
pub(crate) fn volume_control(settings: &Settings, key: &DeviceKey) -> VolumeControl {
    settings
        .output
        .profiles
        .get(key)
        .and_then(|p| p.volume.clone())
        .unwrap_or_else(|| settings.output.volume.clone())
}

/// Whether playing to the device of `key` with `settings` would be at a fixed
/// full volume that the user has not accepted for it.
pub(crate) fn needs_confirmation(settings: &Settings, key: &DeviceKey) -> bool {
    volume_control(settings, key) == VolumeControl::Fixed
        && !settings
            .output
            .profiles
            .get(key)
            .is_some_and(|p| p.fixed_volume_confirmed)
}

/// Whether `request` would make sound start while the player is in `state`.
pub(crate) fn starts_playback(request: &PlayerRequest, state: PlaybackState) -> bool {
    let playing = state == PlaybackState::Playing;
    match request {
        PlayerRequest::PlayPause | PlayerRequest::Play => !playing,
        PlayerRequest::PlayFiles { enqueue, .. } => !enqueue,
        PlayerRequest::PlayAlbum { .. }
        | PlayerRequest::PlayTracks { .. }
        | PlayerRequest::PlayPlaylist { .. }
        | PlayerRequest::PlayArtist { .. }
        | PlayerRequest::PlayGenre { .. }
        | PlayerRequest::PlayComposer { .. }
        | PlayerRequest::PlayYears { .. }
        | PlayerRequest::PlayEntry(_) => true,
        _ => false,
    }
}

/// How a volume control reads in the dialog's readout.
fn control_name(control: &VolumeControl) -> String {
    match control {
        VolumeControl::System => tr("system volume"),
        VolumeControl::Software => tr("software volume"),
        VolumeControl::Hardware(_) => tr("hardware volume"),
        VolumeControl::Fixed => tr("fixed output"),
    }
}

/// Records that the device of `key` may play at full output.
fn remember(backend: &Backend, key: &DeviceKey) -> Result<(), String> {
    backend
        .services()
        .update_settings(&mut |s| {
            s.output
                .profiles
                .entry(key.clone())
                .or_default()
                .fixed_volume_confirmed = true;
        })
        .map_err(|e| e.0)
}

/// The answer to the question, taken by whichever comes first: a button or
/// closing the dialog.
type Answer = Rc<RefCell<Option<Box<dyn FnOnce(bool)>>>>;

/// Asks whether `device` may play at full output, coming from `volume` (0 to
/// 1) with `control`. `answer` gets the choice; closing the dialog cancels.
pub(crate) fn ask(
    parent: Option<&gtk::Window>,
    device: &str,
    volume: f64,
    control: &VolumeControl,
    answer: impl FnOnce(bool) + 'static,
) -> gtk::Window {
    let window = dialog_window(
        parent,
        NAME,
        &tr("Play at full output level?"),
        (480, -1),
        true,
    );
    window.set_resizable(false);
    let answer: Answer = Rc::new(RefCell::new(Some(Box::new(answer))));

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("hk-dialog-body");
    body.add_css_class("hk-confirm");
    // 480 × 300 as designed; the buttons sit at the bottom.
    body.set_size_request(480, 300);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    // The icon centres itself in its tile; the extra height goes below.
    top.set_vexpand(false);
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("hk-icon-tile");
    tile.set_valign(gtk::Align::Start);
    let icon = gtk::Image::from_icon_name("audio-volume-high-symbolic");
    icon.set_vexpand(true);
    icon.set_valign(gtk::Align::Center);
    tile.append(&icon);
    top.append(&tile);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 7);
    text.set_hexpand(true);
    let question = gtk::Label::new(Some(&tr("Play at full output level?")));
    question.set_xalign(0.0);
    question.add_css_class("hk-dialog-title");
    text.append(&question);
    let explanation = gtk::Label::new(None);
    explanation.set_xalign(0.0);
    explanation.set_wrap(true);
    explanation.set_max_width_chars(46);
    explanation.add_css_class("hk-t2");
    let pattern = tr(
        "{} will receive the sound at a fixed 100 %. The volume control in Helikodu will no longer lower it, so turn your amplifier or headphones down first.",
    );
    let t1 = crate::style::current().palette.t1.hex();
    let name = format!(
        "<span weight=\"600\" foreground=\"{t1}\">{}</span>",
        glib_escape(device)
    );
    explanation.set_markup(&fill(&glib_escape(&pattern), &[&name]));
    explanation.add_css_class("hk-confirm-text");
    text.append(&explanation);
    top.append(&text);
    body.append(&top);

    let readout = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    readout.add_css_class("hk-readout");
    readout.add_css_class("hk-glass");
    readout.add_css_class("hk-confirm-readout");
    let column = |title: &str, value: &str| {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 3);
        column.set_hexpand(true);
        column.append(&label(title, &["hk-micro-label"]));
        column.append(&label(value, &["hk-mono", "hk-confirm-value"]));
        column
    };
    let percent = (volume * 100.0).round() as i64;
    readout.append(&column(
        &tr("Now"),
        &format!("{percent} % · {}", control_name(control)),
    ));
    let arrow = gtk::Image::from_icon_name("go-next-symbolic");
    arrow.add_css_class("hk-t3");
    arrow.set_valign(gtk::Align::Center);
    readout.append(&arrow);
    readout.append(&column(
        &tr("After"),
        &format!("100 % · {}", control_name(&VolumeControl::Fixed)),
    ));
    body.append(&readout);
    let space = gtk::Box::new(gtk::Orientation::Vertical, 0);
    space.set_vexpand(true);
    body.append(&space);

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.add_css_class("hk-dialog-buttons");
    let hint = note(&tr("Paused until you answer.\nAsked once for each device."));
    hint.set_hexpand(true);
    hint.set_valign(gtk::Align::Center);
    bottom.append(&hint);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("hk-btn");
    cancel.set_valign(gtk::Align::Center);
    let play = gtk::Button::with_label(&tr("Play at full output"));
    play.add_css_class("hk-btn");
    play.set_valign(gtk::Align::Center);
    bottom.append(&cancel);
    bottom.append(&play);
    body.append(&bottom);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&cancel));

    for (button, yes) in [(&cancel, false), (&play, true)] {
        let answer = answer.clone();
        let weak = window.downgrade();
        button.connect_clicked(move |_| {
            if let Some(answer) = answer.borrow_mut().take() {
                answer(yes);
            }
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        });
    }
    window.connect_close_request(move |_| {
        if let Some(answer) = answer.borrow_mut().take() {
            answer(false);
        }
        gtk::glib::Propagation::Proceed
    });
    window.present();
    cancel.grab_focus();
    // The safe answer shows its focus ring from the start.
    window.set_focus_visible(true);
    window
}

fn glib_escape(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

/// Puts the confirmation in front of requests that would start playback.
pub(crate) fn install_gate(window: &gtk::Window, backend: &Backend, playback: &Rc<Playback>) {
    let weak_window = window.downgrade();
    let weak_playback = Rc::downgrade(playback);
    let gated = backend.clone();
    let open = Rc::new(RefCell::new(false));
    let gate = Rc::new(move |request: &PlayerRequest| -> bool {
        let Some(playback) = weak_playback.upgrade() else {
            return true;
        };
        let view = playback.view();
        if !starts_playback(request, view.state) {
            return true;
        }
        let services = gated.services();
        let output = services.output();
        let Some(key) = device_key(&gated, &output) else {
            return true;
        };
        let settings = services.settings();
        if !needs_confirmation(&settings, &key) {
            return true;
        }
        if *open.borrow() {
            return false;
        }
        open.replace(true);
        let window = weak_window.upgrade();
        let backend = gated.clone();
        let request = request.clone();
        let flag = open.clone();
        let control = settings.output.volume.clone();
        ask(
            window.as_ref(),
            &view.output.device,
            view.volume,
            &if control == VolumeControl::Fixed {
                VolumeControl::Software
            } else {
                control
            },
            move |yes| {
                flag.replace(false);
                if yes {
                    match remember(&backend, &key) {
                        Ok(()) => backend.send_ungated(request),
                        Err(e) => eprintln!("helikodu: cannot keep the answer: {e}"),
                    }
                }
            },
        );
        false
    });
    backend.set_gate(Some(gate));
}

/// A change of the output or its settings, before it is made.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Proposal<'a> {
    /// Where the output would play.
    pub(crate) target: &'a OutputSelection,
    /// That device's name, for the question.
    pub(crate) device: &'a str,
    /// The settings after the change.
    pub(crate) after: &'a Settings,
}

/// Applies `proposal`, asking first when it would mean an unconfirmed fixed
/// full volume. While the question is open, playback pauses; it resumes after
/// the answer. `apply` makes the change; `cancel` puts the controls back.
pub(crate) fn guard_change(
    parent: Option<&gtk::Window>,
    backend: &Backend,
    playback: Option<&Rc<Playback>>,
    proposal: Proposal<'_>,
    apply: impl FnOnce() + 'static,
    cancel: impl FnOnce() + 'static,
) {
    let Proposal {
        target,
        device,
        after,
    } = proposal;
    let Some(key) = device_key(backend, target) else {
        apply();
        return;
    };
    let view = playback.map(|p| p.view());
    let playing = view
        .as_ref()
        .is_some_and(|v| v.state == PlaybackState::Playing);
    if !needs_confirmation(after, &key) || !playing {
        // Nothing sounds now; the gate asks before playback starts.
        apply();
        return;
    }
    backend.send_ungated(PlayerRequest::PlayPause);
    let services = backend.services();
    let current = services.output();
    let now = device_key(backend, &current).map_or(VolumeControl::System, |k| {
        volume_control(&services.settings(), &k)
    });
    let volume = view.as_ref().map_or(1.0, |v| v.volume);
    let backend = backend.clone();
    ask(parent, device, volume, &now, move |yes| {
        if yes {
            if let Err(e) = remember(&backend, &key) {
                eprintln!("helikodu: cannot keep the answer: {e}");
            }
            apply();
        } else {
            cancel();
        }
        backend.send_ungated(PlayerRequest::PlayPause);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::services::OutputBackendKind;

    fn key() -> DeviceKey {
        DeviceKey::new(
            OutputBackendKind::Alsa,
            DeviceId::new("hw:CARD=D10s,DEV=0").unwrap(),
        )
    }

    #[test]
    fn fixed_volume_needs_a_yes_per_device() {
        let mut settings = Settings::default();
        assert!(!needs_confirmation(&settings, &key()));
        settings.output.volume = VolumeControl::Fixed;
        assert!(needs_confirmation(&settings, &key()));
        settings
            .output
            .profiles
            .entry(key())
            .or_default()
            .fixed_volume_confirmed = true;
        assert!(!needs_confirmation(&settings, &key()));
        let other = DeviceKey::new(OutputBackendKind::Alsa, DeviceId::new("hw:0").unwrap());
        assert!(needs_confirmation(&settings, &other));
    }

    #[test]
    fn a_profile_can_fix_the_volume() {
        let mut settings = Settings::default();
        settings.output.profiles.entry(key()).or_default().volume = Some(VolumeControl::Fixed);
        assert!(needs_confirmation(&settings, &key()));
    }

    #[test]
    fn only_starting_requests_are_held() {
        assert!(starts_playback(
            &PlayerRequest::PlayPause,
            PlaybackState::Paused
        ));
        assert!(!starts_playback(
            &PlayerRequest::PlayPause,
            PlaybackState::Playing
        ));
        assert!(!starts_playback(&PlayerRequest::Stop, PlaybackState::Idle));
        assert!(starts_playback(
            &PlayerRequest::PlayEntry(3),
            PlaybackState::Idle
        ));
        assert!(!starts_playback(
            &PlayerRequest::PlayFiles {
                files: Vec::new(),
                enqueue: true
            },
            PlaybackState::Idle
        ));
    }
}
