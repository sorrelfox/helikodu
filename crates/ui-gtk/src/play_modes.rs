// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The play modes in words and in menus: shuffle, repeat, stop after, play
//! only, the queue's modes, the A-B loop and the album commands. The
//! application actions behind the menu items live here too; their states
//! follow the player (the window keeps them current).
//!
//! The Playback menu is filled here, so that the four mode items can carry
//! their state in the label ("Shuffle: Albums"): a menu item has no second
//! column.

use std::cell::Cell;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::{
    Backend, PlayModes, PlayScope, PlaybackState, PlayerRequest, RepeatMode, ShuffleMode, StopAfter,
};
use crate::i18n::{fill, tr};
use crate::playback::PlayerView;

/// The action's target for a shuffle mode.
pub(crate) fn shuffle_id(mode: ShuffleMode) -> &'static str {
    match mode {
        ShuffleMode::Tracks => "tracks",
        ShuffleMode::Albums => "albums",
        ShuffleMode::Folders => "folders",
        ShuffleMode::Groupings => "groupings",
        ShuffleMode::WithinAlbum => "within-album",
        _ => "off",
    }
}

/// The action's target for a repeat mode.
pub(crate) fn repeat_id(mode: RepeatMode) -> &'static str {
    match mode {
        RepeatMode::One => "one",
        RepeatMode::Album => "album",
        RepeatMode::All => "all",
        RepeatMode::Endless => "endless",
        RepeatMode::AutoDj => "auto-dj",
        _ => "off",
    }
}

/// The action's target for a stop-after choice.
pub(crate) fn stop_after_id(mode: StopAfter) -> &'static str {
    match mode {
        StopAfter::Track => "track",
        StopAfter::Album => "album",
        StopAfter::Queue => "queue",
        _ => "off",
    }
}

/// The action's target for a "Play only" choice.
pub(crate) fn scope_id(scope: PlayScope) -> &'static str {
    match scope {
        PlayScope::Artist => "artist",
        PlayScope::Album => "album",
        _ => "everything",
    }
}

/// The shuffle mode as the menu names it.
pub(crate) fn shuffle_name(mode: ShuffleMode) -> String {
    match mode {
        ShuffleMode::Tracks => tr("Tracks"),
        ShuffleMode::Albums => tr("Albums"),
        ShuffleMode::Folders => tr("Folders"),
        ShuffleMode::Groupings => tr("Groupings"),
        ShuffleMode::WithinAlbum => tr("Tracks Within Each Album"),
        _ => tr("Off"),
    }
}

/// The repeat mode as the menu names it.
pub(crate) fn repeat_name(mode: RepeatMode) -> String {
    match mode {
        RepeatMode::One => tr("This Track"),
        RepeatMode::Album => tr("This Album"),
        RepeatMode::All => tr("The Whole Queue"),
        RepeatMode::Endless => tr("Continue with Related Tracks"),
        RepeatMode::AutoDj => tr("Auto-DJ"),
        _ => tr("Off"),
    }
}

/// The repeat mode as the label of the Repeat submenu names it: the long
/// one shorter ("Repeat: Related Tracks").
pub(crate) fn repeat_label(mode: RepeatMode) -> String {
    match mode {
        RepeatMode::Endless => tr("Related Tracks"),
        other => repeat_name(other),
    }
}

/// The repeat button's icon: a "1" for one track, a plus between the arrows
/// for the two modes that add tracks.
pub(crate) fn repeat_icon(mode: RepeatMode) -> &'static str {
    match mode {
        RepeatMode::One => "helikodu-repeat-one-symbolic",
        RepeatMode::Endless | RepeatMode::AutoDj => "helikodu-repeat-plus-symbolic",
        _ => "helikodu-repeat-symbolic",
    }
}

/// The stop-after choice as the menu names it.
pub(crate) fn stop_after_name(mode: StopAfter) -> String {
    match mode {
        StopAfter::Track => tr("This Track"),
        StopAfter::Album => tr("This Album"),
        StopAfter::Queue => tr("The Queue"),
        _ => tr("Off"),
    }
}

/// The "Play only" choice as the menu names it.
pub(crate) fn scope_name(scope: PlayScope) -> String {
    match scope {
        PlayScope::Artist => tr("This Artist"),
        PlayScope::Album => tr("This Album"),
        _ => tr("Everything"),
    }
}

/// The shuffle button's tooltip and accessible name.
pub(crate) fn shuffle_tooltip(mode: ShuffleMode) -> String {
    match mode {
        ShuffleMode::Tracks => tr("Shuffle: tracks"),
        ShuffleMode::Albums => tr("Shuffle: albums"),
        ShuffleMode::Folders => tr("Shuffle: folders"),
        ShuffleMode::Groupings => tr("Shuffle: groupings"),
        ShuffleMode::WithinAlbum => tr("Shuffle: tracks within each album"),
        _ => tr("Shuffle: off"),
    }
}

/// The repeat button's tooltip and accessible name.
pub(crate) fn repeat_tooltip(mode: RepeatMode) -> String {
    match mode {
        RepeatMode::One => tr("Repeat: this track"),
        RepeatMode::Album => tr("Repeat: this album"),
        RepeatMode::All => tr("Repeat: the whole queue"),
        RepeatMode::Endless => tr("Repeat: continue with related tracks"),
        RepeatMode::AutoDj => tr("Repeat: Auto-DJ"),
        _ => tr("Repeat: off"),
    }
}

/// What the play button adds to its name while stop-after is on.
pub(crate) fn stop_after_note(mode: StopAfter) -> Option<String> {
    match mode {
        StopAfter::Track => Some(tr("stops after this track")),
        StopAfter::Album => Some(tr("stops after this album")),
        StopAfter::Queue => Some(tr("stops at the end of the queue")),
        _ => None,
    }
}

thread_local! {
    /// The shuffle and repeat modes used last, for the deck's buttons, which
    /// switch between off and them.
    static LAST: Cell<(ShuffleMode, RepeatMode)> =
        const { Cell::new((ShuffleMode::Tracks, RepeatMode::All)) };
}

/// Remembers the modes in use as the ones used last.
pub(crate) fn remember(modes: &PlayModes) {
    LAST.with(|last| {
        let (mut shuffle, mut repeat) = last.get();
        if modes.shuffle != ShuffleMode::Off {
            shuffle = modes.shuffle;
        }
        if modes.repeat != RepeatMode::Off {
            repeat = modes.repeat;
        }
        last.set((shuffle, repeat));
    });
}

/// The shuffle mode a click on the deck's button sets: off, or the mode used
/// last.
pub(crate) fn toggled_shuffle(now: ShuffleMode) -> ShuffleMode {
    if now == ShuffleMode::Off {
        LAST.with(|last| last.get().0)
    } else {
        ShuffleMode::Off
    }
}

/// The repeat mode a click on the deck's button sets.
pub(crate) fn toggled_repeat(now: RepeatMode) -> RepeatMode {
    if now == RepeatMode::Off {
        LAST.with(|last| last.get().1)
    } else {
        RepeatMode::Off
    }
}

fn radio(menu: &gio::Menu, label: &str, action: &str, target: &str) {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    menu.append_item(&item);
}

/// The Shuffle submenu (radio items).
pub(crate) fn shuffle_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for mode in ShuffleMode::ALL {
        radio(&menu, &shuffle_name(mode), "app.shuffle", shuffle_id(mode));
    }
    menu
}

/// The Repeat submenu.
pub(crate) fn repeat_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for mode in RepeatMode::ALL {
        radio(&menu, &repeat_name(mode), "app.repeat", repeat_id(mode));
    }
    menu
}

/// The Stop after submenu.
pub(crate) fn stop_after_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for mode in StopAfter::ALL {
        radio(
            &menu,
            &stop_after_name(mode),
            "app.stop-after-mode",
            stop_after_id(mode),
        );
    }
    menu
}

/// The Play only submenu.
pub(crate) fn scope_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for scope in PlayScope::ALL {
        radio(&menu, &scope_name(scope), "app.play-scope", scope_id(scope));
    }
    menu
}

/// The Loop a section submenu.
pub(crate) fn loop_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some(&tr("Set the Start Here")), Some("app.loop-start"));
    menu.append(Some(&tr("Set the End Here")), Some("app.loop-end"));
    menu.append(Some(&tr("Clear the Loop")), Some("app.loop-clear"));
    menu
}

/// The Queue submenu.
pub(crate) fn queue_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let modes = gio::Menu::new();
    modes.append(
        Some(&tr("Remove Tracks After They Play")),
        Some("app.consume"),
    );
    modes.append(
        Some(&tr("Continue from the Selected Track")),
        Some("app.follow-cursor"),
    );
    modes.append(Some(&tr("Intro Scan")), Some("app.intro-scan"));
    menu.append_section(None, &modes);
    let edits = gio::Menu::new();
    edits.append(Some(&tr("Save as Playlist…")), Some("win.save-queue"));
    edits.append(Some(&tr("Clear")), Some("app.clear-queue"));
    menu.append_section(None, &edits);
    menu
}

/// Fills the Playback menu for the player as it is now.
pub(crate) fn fill_playback_menu(menu: &gio::Menu, view: &PlayerView) {
    menu.remove_all();
    let transport = gio::Menu::new();
    let pause = if view.state == PlaybackState::Playing {
        tr("Pause")
    } else {
        tr("Play")
    };
    transport.append(Some(&pause), Some("app.play-pause"));
    transport.append(Some(&tr("Stop")), Some("app.stop"));
    menu.append_section(None, &transport);

    let moves = gio::Menu::new();
    moves.append(Some(&tr("Previous")), Some("app.previous"));
    moves.append(Some(&tr("Next")), Some("app.next"));
    moves.append(Some(&tr("Previous Album")), Some("app.previous-album"));
    moves.append(Some(&tr("Next Album")), Some("app.next-album"));
    moves.append(Some(&tr("Random Album")), Some("app.random-album"));
    moves.append(Some(&tr("Radio Random")), Some("app.radio-random"));
    moves.append(Some(&tr("Revert to the Skipped Track")), Some("app.revert"));
    menu.append_section(None, &moves);

    let seeks = gio::Menu::new();
    seeks.append(Some(&tr("Seek Back")), Some("app.seek-back"));
    seeks.append(Some(&tr("Seek Forward")), Some("app.seek-forward"));
    seeks.append_submenu(Some(&tr("Loop a Section")), &loop_menu());
    menu.append_section(None, &seeks);

    let modes = gio::Menu::new();
    let m = &view.modes;
    modes.append_submenu(
        Some(&fill(&tr("Shuffle: {}"), &[&shuffle_name(m.shuffle)])),
        &shuffle_menu(),
    );
    modes.append_submenu(
        Some(&fill(&tr("Repeat: {}"), &[&repeat_label(m.repeat)])),
        &repeat_menu(),
    );
    modes.append_submenu(
        Some(&fill(
            &tr("Stop After: {}"),
            &[&stop_after_name(view.stop_after_mode)],
        )),
        &stop_after_menu(),
    );
    modes.append_submenu(
        Some(&fill(&tr("Play Only: {}"), &[&scope_name(m.scope)])),
        &scope_menu(),
    );
    modes.append_submenu(Some(&tr("Queue")), &queue_menu());
    menu.append_section(None, &modes);

    let rest = gio::Menu::new();
    rest.append(Some(&tr("Sleep Timer…")), Some("win.sleep-timer"));
    // While the speed is not 1× its chip on the deck opens the popover.
    if view.speed.is_normal() {
        rest.append(Some(&tr("Speed…")), Some("win.speed"));
    }
    let output = gio::Menu::new();
    output.append(
        Some(&tr("Choose the Output Device…")),
        Some("win.output-switcher"),
    );
    rest.append_submenu(Some(&tr("Output Device")), &output);
    menu.append_section(None, &rest);
}

/// Sets the states of the mode actions of `application` from `view`.
pub(crate) fn sync_actions(application: &gtk::Application, view: &PlayerView) {
    let set = |name: &str, state: glib::Variant| {
        if let Some(action) = application
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
            && action.state().as_ref() != Some(&state)
        {
            action.set_state(&state);
        }
    };
    let m = &view.modes;
    set("shuffle", shuffle_id(m.shuffle).to_variant());
    set("repeat", repeat_id(m.repeat).to_variant());
    set(
        "stop-after-mode",
        stop_after_id(view.stop_after_mode).to_variant(),
    );
    set("play-scope", scope_id(m.scope).to_variant());
    set("consume", m.consume.to_variant());
    set("follow-cursor", m.follow_cursor.to_variant());
    set("intro-scan", m.intro_scan.to_variant());
}

/// Enables "Revert to the Skipped Track" only while the player has a track of
/// the queue to go back to.
pub(crate) fn sync_revert(application: &gtk::Application, view: &PlayerView) {
    if let Some(action) = application
        .lookup_action("revert")
        .and_downcast::<gio::SimpleAction>()
    {
        action.set_enabled(view.queue.can_revert);
    }
}

/// A stateful action whose target names a choice; activating it sends the
/// request `request` makes of the target. The state follows the player.
fn choice_action(
    application: &gtk::Application,
    backend: &Backend,
    name: &str,
    initial: &str,
    request: fn(&str) -> Option<PlayerRequest>,
) {
    let action =
        gio::SimpleAction::new_stateful(name, Some(glib::VariantTy::STRING), &initial.to_variant());
    let b = backend.clone();
    action.connect_activate(move |_, target| {
        if let Some(request) = target
            .and_then(|t| t.get::<String>())
            .and_then(|t| request(&t))
        {
            b.send(request);
        }
    });
    application.add_action(&action);
}

/// A stateful on-off action that sends `request` with the new state.
fn switch_action(
    application: &gtk::Application,
    backend: &Backend,
    name: &str,
    request: fn(bool) -> PlayerRequest,
) {
    let action = gio::SimpleAction::new_stateful(name, None, &false.to_variant());
    let b = backend.clone();
    action.connect_activate(move |action, _| {
        let on = action
            .state()
            .and_then(|s| s.get::<bool>())
            .unwrap_or(false);
        b.send(request(!on));
    });
    application.add_action(&action);
}

/// Adds the play-mode, album and loop actions to `application`.
pub(crate) fn install_actions(application: &gtk::Application, backend: &Backend) {
    for (name, request) in [
        ("previous-album", PlayerRequest::PreviousAlbum),
        ("next-album", PlayerRequest::NextAlbum),
        ("random-album", PlayerRequest::RandomAlbum),
        ("radio-random", PlayerRequest::RadioRandom),
        ("revert", PlayerRequest::Revert),
        ("loop-start", PlayerRequest::LoopStartHere),
        ("loop-end", PlayerRequest::LoopEndHere),
        ("loop-clear", PlayerRequest::SetLoop(None)),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let b = backend.clone();
        action.connect_activate(move |_, _| b.send(request.clone()));
        // Nothing to revert to until "Next" has moved on from a track.
        if name == "revert" {
            action.set_enabled(false);
        }
        application.add_action(&action);
    }
    choice_action(application, backend, "shuffle", "off", |id| {
        ShuffleMode::ALL
            .into_iter()
            .find(|m| shuffle_id(*m) == id)
            .map(PlayerRequest::SetShuffleMode)
    });
    choice_action(application, backend, "repeat", "off", |id| {
        RepeatMode::ALL
            .into_iter()
            .find(|m| repeat_id(*m) == id)
            .map(PlayerRequest::SetRepeatMode)
    });
    choice_action(application, backend, "stop-after-mode", "off", |id| {
        StopAfter::ALL
            .into_iter()
            .find(|m| stop_after_id(*m) == id)
            .map(PlayerRequest::SetStopAfterMode)
    });
    choice_action(application, backend, "play-scope", "everything", |id| {
        PlayScope::ALL
            .into_iter()
            .find(|m| scope_id(*m) == id)
            .map(PlayerRequest::SetPlayScope)
    });
    switch_action(application, backend, "consume", PlayerRequest::SetConsume);
    switch_action(
        application,
        backend,
        "follow-cursor",
        PlayerRequest::SetFollowCursor,
    );
    switch_action(
        application,
        backend,
        "intro-scan",
        PlayerRequest::SetIntroScan,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_has_its_own_target() {
        for mode in ShuffleMode::ALL {
            let id = shuffle_id(mode);
            assert_eq!(
                ShuffleMode::ALL.into_iter().find(|m| shuffle_id(*m) == id),
                Some(mode)
            );
        }
        for mode in RepeatMode::ALL {
            let id = repeat_id(mode);
            assert_eq!(
                RepeatMode::ALL.into_iter().find(|m| repeat_id(*m) == id),
                Some(mode)
            );
        }
    }

    #[test]
    fn a_click_toggles_between_off_and_the_last_mode() {
        assert_eq!(toggled_shuffle(ShuffleMode::Off), ShuffleMode::Tracks);
        remember(&PlayModes::new(ShuffleMode::Albums, RepeatMode::One));
        assert_eq!(toggled_shuffle(ShuffleMode::Off), ShuffleMode::Albums);
        assert_eq!(toggled_shuffle(ShuffleMode::Albums), ShuffleMode::Off);
        assert_eq!(toggled_repeat(RepeatMode::Off), RepeatMode::One);
        assert_eq!(toggled_repeat(RepeatMode::One), RepeatMode::Off);
    }
}
