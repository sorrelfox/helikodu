// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's part in the desktop: the mini player, the tray icon and its
//! menu, the shortcuts that work in other applications, and pausing when the
//! screen locks or the computer goes to sleep.
//!
//! The D-Bus services (`helikodu-desktop`) start only with
//! [`MainWindow::start_desktop_services`], which the application calls for
//! the window it shows; windows made in tests have none. Their callbacks come
//! from other threads and reach the window through a channel.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use helikodu_desktop::global_shortcuts::{
    BoundShortcut, GlobalShortcuts, ShortcutEvent, ShortcutSpec,
};
use helikodu_desktop::session_watch::{SessionChange, SessionWatch};
use helikodu_desktop::tray::{Tray, TrayAction, TrayConfig, TrayContent, TrayHooks, TrayMenuItem};

use super::MainWindow;
use crate::backend::{Backend, PlaybackState, PlayerRequest};
use crate::i18n::{fill, tr};
use crate::mini_player::MiniPlayer;
use crate::models::CoverCache;
use crate::models::lyrics::LyricsModel;
use crate::playback::{Change, Playback};

/// What the desktop services report, on the main thread.
enum Event {
    TrayAction(TrayAction),
    TrayActivate,
    TrayScroll(i32),
    TrayHost(bool),
    /// Whether the desktop has a tray, asked while the icon is off.
    TrayDesktop(bool),
    Shortcuts(Result<GlobalShortcuts, String>),
    Shortcut(ShortcutEvent),
    Session(SessionChange),
    SessionReady(SessionWatch),
}

/// The state of the shortcuts in other applications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlobalShortcutState {
    /// Whether the desktop's dialog can be opened again.
    pub(crate) can_configure: bool,
    /// The shortcuts as the desktop bound them, once bound in this session.
    pub(crate) bound: Option<Vec<BoundShortcut>>,
}

/// The shortcuts handed to the desktop, with their names.
pub(crate) fn global_shortcuts() -> Vec<ShortcutSpec> {
    [
        ("play-pause", tr("Play or pause")),
        ("next", tr("Next track")),
        ("previous", tr("Previous track")),
        ("show-window", tr("Show Helikodu")),
    ]
    .into_iter()
    .map(|(id, description)| ShortcutSpec {
        id: id.into(),
        description,
    })
    .collect()
}

/// The window's desktop parts.
pub(crate) struct Integration {
    backend: Backend,
    playback: Rc<Playback>,
    lyrics: Rc<LyricsModel>,
    covers: Rc<CoverCache>,
    mini: RefCell<Option<Rc<MiniPlayer>>>,
    tray: RefCell<Option<Tray>>,
    /// Whether a tray shows the icon: the icon is registered and a host is there.
    tray_host: Cell<bool>,
    /// Whether the desktop has a tray, icon or not.
    tray_desktop: Cell<bool>,
    /// Whether closing the window has left it in the tray in this run.
    kept_in_tray: Cell<bool>,
    shortcuts: RefCell<Option<GlobalShortcuts>>,
    bound: RefCell<Option<Vec<BoundShortcut>>>,
    session: RefCell<Option<SessionWatch>>,
    events: async_channel::Sender<Event>,
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
    started: Cell<bool>,
}

impl std::fmt::Debug for Integration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Integration")
            .field("tray_host", &self.tray_host.get())
            .finish_non_exhaustive()
    }
}

impl MainWindow {
    pub(super) fn setup_desktop(
        &self,
        backend: &Backend,
        playback: &Rc<Playback>,
        lyrics: &Rc<LyricsModel>,
        covers: &Rc<CoverCache>,
    ) {
        let (events, received) = async_channel::unbounded();
        let integration = Rc::new(Integration {
            backend: backend.clone(),
            playback: playback.clone(),
            lyrics: lyrics.clone(),
            covers: covers.clone(),
            mini: RefCell::new(None),
            tray: RefCell::new(None),
            tray_host: Cell::new(false),
            tray_desktop: Cell::new(false),
            kept_in_tray: Cell::new(false),
            shortcuts: RefCell::new(None),
            bound: RefCell::new(None),
            session: RefCell::new(None),
            events,
            listeners: RefCell::new(Vec::new()),
            started: Cell::new(false),
        });
        self.imp().desktop.replace(Some(integration));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            while let Ok(event) = received.recv().await {
                let Some(window) = weak.upgrade() else { break };
                window.desktop_event(event);
            }
        });
        let weak = self.downgrade();
        playback.connect(move |_, change| {
            if let Some(window) = weak.upgrade()
                && matches!(change, Change::Track | Change::State | Change::Modes)
            {
                window.refresh_tray();
            }
        });
        self.connect_visible_notify(|window| window.refresh_tray());
    }

    fn integration(&self) -> Option<Rc<Integration>> {
        self.imp().desktop.borrow().clone()
    }

    /// Starts the tray icon (where the settings want it), the shortcuts in
    /// other applications and the lock and sleep watch.
    pub fn start_desktop_services(&self) {
        let Some(integration) = self.integration() else {
            return;
        };
        if integration.started.replace(true) {
            return;
        }
        self.apply_tray_setting();
        // Opening a portal session waits for the portal: off the main thread.
        let events = integration.events.clone();
        let started = std::thread::Builder::new()
            .name("helikodu-desktop-start".into())
            .spawn(move || {
                // Whether to offer the tray icon in the settings.
                let _ = events
                    .send_blocking(Event::TrayDesktop(helikodu_desktop::tray::host_present()));
                let forward = events.clone();
                let shortcuts = GlobalShortcuts::start(move |event| {
                    let _ = forward.send_blocking(Event::Shortcut(event));
                })
                .map_err(|e| e.to_string());
                let _ = events.send_blocking(Event::Shortcuts(shortcuts));
                let forward = events.clone();
                let watch = SessionWatch::start(move |change| {
                    let _ = forward.send_blocking(Event::Session(change));
                });
                // The watch lives on the main thread from now on.
                let _ = events.send_blocking(Event::SessionReady(watch));
            });
        if let Err(error) = started {
            log::warn!("cannot start the desktop services: {error}");
        }
    }

    fn desktop_event(&self, event: Event) {
        let Some(integration) = self.integration() else {
            return;
        };
        match event {
            Event::TrayAction(action) => self.tray_action(action),
            Event::TrayActivate => self.tray_action(TrayAction::ToggleWindow),
            Event::TrayScroll(delta) => {
                let step = if delta > 0 { 0.05 } else { -0.05 };
                self.imp().deck.adjust_volume(step);
            }
            Event::TrayHost(shown) => {
                integration.tray_host.set(shown);
                integration.tray_desktop.set(shown);
                if !shown {
                    // The tray is gone: a window left there comes back.
                    self.show_from_tray();
                }
                self.desktop_changed();
            }
            Event::TrayDesktop(present) => {
                integration
                    .tray_desktop
                    .set(present || integration.tray_host.get());
                self.desktop_changed();
            }
            Event::Shortcuts(result) => {
                match result {
                    Ok(shortcuts) => {
                        if self.settings_value(|s| s.shortcuts.global) {
                            shortcuts.bind(global_shortcuts());
                        }
                        integration.shortcuts.replace(Some(shortcuts));
                    }
                    Err(error) => {
                        log::warn!("no shortcuts in other applications: {error}");
                    }
                }
                self.desktop_changed();
            }
            Event::Shortcut(ShortcutEvent::Activated(id)) => {
                let application = self.application();
                match id.as_str() {
                    "show-window" => self.present(),
                    "play-pause" | "next" | "previous" => {
                        if let Some(application) = application {
                            application.activate_action(&id, None);
                        }
                    }
                    _ => {}
                }
            }
            Event::Shortcut(ShortcutEvent::Bound(bound)) => {
                integration.bound.replace(Some(bound));
                self.desktop_changed();
            }
            Event::Shortcut(ShortcutEvent::Failed(error)) => {
                log::warn!("the shortcuts were not set up: {error}");
            }
            Event::Session(change) => self.session_changed(change),
            Event::SessionReady(watch) => {
                watch.set_delay_sleep(self.settings_value(|s| s.playback.pause_before_sleep));
                integration.session.replace(Some(watch));
            }
        }
    }

    fn session_changed(&self, change: SessionChange) {
        let Some(integration) = self.integration() else {
            return;
        };
        let playing = integration.playback.view().state == PlaybackState::Playing;
        let pause = match change {
            SessionChange::Locked => self.settings_value(|s| s.playback.pause_on_lock),
            SessionChange::GoingToSleep => self.settings_value(|s| s.playback.pause_before_sleep),
            SessionChange::Unlocked | SessionChange::Woke => false,
        };
        if pause && playing {
            integration.backend.send(PlayerRequest::PlayPause);
        }
    }

    /// A value of the settings.
    fn settings_value<T>(&self, get: impl Fn(&crate::backend::services::Settings) -> T) -> T {
        let settings = self
            .integration()
            .map(|i| i.backend.services().settings())
            .unwrap_or_default();
        get(&settings)
    }

    /// Follows a change of "Pause before the computer goes to sleep".
    pub(crate) fn apply_power_settings(&self) {
        let delay = self.settings_value(|s| s.playback.pause_before_sleep);
        if let Some(integration) = self.integration()
            && let Some(watch) = integration.session.borrow().as_ref()
        {
            watch.set_delay_sleep(delay);
        }
    }

    /// Shows or removes the tray icon as the settings say.
    pub(crate) fn apply_tray_setting(&self) {
        let Some(integration) = self.integration() else {
            return;
        };
        let wanted = self.settings_value(|s| s.appearance.tray_icon);
        if !wanted {
            integration.tray.replace(None);
            integration.tray_host.set(false);
            self.show_from_tray();
            self.desktop_changed();
            return;
        }
        if integration.tray.borrow().is_some() || !integration.started.get() {
            return;
        }
        let send = |event: fn(i32) -> Event| {
            let events = integration.events.clone();
            move |value: i32| {
                let _ = events.send_blocking(event(value));
            }
        };
        let actions = integration.events.clone();
        let activate = integration.events.clone();
        let host = integration.events.clone();
        let hooks = TrayHooks::new(move |action| {
            let _ = actions.send_blocking(Event::TrayAction(action));
        })
        .with_activate(move || {
            let _ = activate.send_blocking(Event::TrayActivate);
        })
        .with_scroll(send(Event::TrayScroll))
        .with_host(move |shown| {
            let _ = host.send_blocking(Event::TrayHost(shown));
        });
        let config = TrayConfig {
            title: tr("Helikodu"),
            ..TrayConfig::default()
        };
        match Tray::start(config, hooks) {
            Ok(tray) => {
                integration.tray.replace(Some(tray));
                self.refresh_tray();
            }
            Err(error) => log::warn!("no tray icon: {error}"),
        }
    }

    /// Whether a tray shows the icon now.
    pub(crate) fn tray_available(&self) -> bool {
        self.integration().is_some_and(|i| i.tray_host.get())
    }

    /// Whether the desktop has a tray, so that the settings offer the icon.
    pub(crate) fn tray_offered(&self) -> bool {
        self.integration()
            .is_some_and(|i| i.tray_desktop.get() || i.tray_host.get())
    }

    /// Whether closing the window keeps playing in the tray: only when the
    /// settings ask for it and a tray shows the icon right now, or the window
    /// could not be found again.
    pub(super) fn close_to_tray(&self) -> bool {
        self.tray_available()
            && self.settings_value(|s| s.appearance.tray_icon && s.appearance.close_to_tray)
    }

    /// Hides the window to the tray, where playing goes on; the first time in
    /// a run, a notification says where it went and how to quit.
    pub(super) fn hide_to_tray(&self) {
        self.set_visible(false);
        let Some(integration) = self.integration() else {
            return;
        };
        if integration.kept_in_tray.replace(true) {
            return;
        }
        if let Some(application) = self.application() {
            let notification = gio::Notification::new(&tr("Helikodu"));
            notification.set_body(Some(&tr(
                "Helikodu keeps playing in the tray. Quit from the tray menu or with Ctrl+Q.",
            )));
            notification.set_default_action("app.activate");
            application.send_notification(Some("tray"), &notification);
        }
    }

    /// Whether closing the window has left it in the tray in this run (for
    /// tests).
    #[doc(hidden)]
    pub fn kept_in_tray(&self) -> bool {
        self.integration().is_some_and(|i| i.kept_in_tray.get())
    }

    /// Shows the window again when it waits in a tray that is gone, unless
    /// the mini player stands in for it.
    fn show_from_tray(&self) {
        let mini = self.mini_player().is_some_and(|m| m.is_shown());
        if !self.is_visible() && !mini && self.is_realized() {
            self.present();
        }
    }

    /// The tray menu and tooltip for what plays now.
    fn tray_content(&self) -> TrayContent {
        let Some(integration) = self.integration() else {
            return TrayContent::default();
        };
        let view = integration.playback.view();
        let mut menu = Vec::new();
        let tooltip = match &view.now {
            Some(now) => {
                menu.push(TrayMenuItem::Text(now.track.title.clone()));
                let line = if now.album_title.is_empty() {
                    now.track.artist.clone()
                } else {
                    fill(&tr("{} · {}"), &[&now.track.artist, &now.album_title])
                };
                menu.push(TrayMenuItem::Text(line));
                if now.track.artist.is_empty() {
                    now.track.title.clone()
                } else {
                    fill(&tr("{} · {}"), &[&now.track.title, &now.track.artist])
                }
            }
            None => {
                menu.push(TrayMenuItem::Text(tr("Nothing playing")));
                tr("Helikodu")
            }
        };
        let has_track = view.now.is_some();
        let playing = view.state == PlaybackState::Playing;
        menu.push(TrayMenuItem::Separator);
        let item = |action, label: String, enabled| TrayMenuItem::Action {
            action,
            label,
            enabled,
            checked: None,
        };
        menu.push(item(
            TrayAction::PlayPause,
            if playing { tr("Pause") } else { tr("Play") },
            has_track,
        ));
        menu.push(item(TrayAction::Next, tr("Next"), has_track));
        menu.push(item(TrayAction::Previous, tr("Previous"), has_track));
        menu.push(TrayMenuItem::check(
            TrayAction::StopAfter,
            tr("Stop after this track"),
            view.stop_after,
        ));
        menu.push(TrayMenuItem::Separator);
        menu.push(TrayMenuItem::action(
            TrayAction::ToggleWindow,
            if self.is_visible() {
                tr("Hide the window")
            } else {
                tr("Show the window")
            },
        ));
        menu.push(TrayMenuItem::action(
            TrayAction::MiniPlayer,
            tr("Mini player"),
        ));
        menu.push(TrayMenuItem::Separator);
        menu.push(TrayMenuItem::action(TrayAction::Quit, tr("Quit")));
        TrayContent { tooltip, menu }
    }

    /// Gives the tray what plays now.
    pub(super) fn refresh_tray(&self) {
        let Some(integration) = self.integration() else {
            return;
        };
        if integration.tray.borrow().is_none() {
            return;
        }
        let content = self.tray_content();
        if let Some(tray) = integration.tray.borrow().as_ref() {
            tray.set_content(content);
        }
    }

    fn tray_action(&self, action: TrayAction) {
        let application = self.application();
        let activate = |name: &str| {
            if let Some(application) = &application {
                application.activate_action(name, None);
            }
        };
        match action {
            TrayAction::PlayPause => activate("play-pause"),
            TrayAction::Next => activate("next"),
            TrayAction::Previous => activate("previous"),
            TrayAction::StopAfter => activate("stop-after"),
            TrayAction::MiniPlayer => self.toggle_mini_player(),
            TrayAction::Quit => activate("quit"),
            TrayAction::ToggleWindow => {
                if self.is_visible() {
                    self.set_visible(false);
                } else {
                    if let Some(mini) = self.mini_player() {
                        mini.hide();
                    }
                    self.present();
                }
            }
        }
    }

    /// The mini player, once opened.
    pub(crate) fn mini_player(&self) -> Option<Rc<MiniPlayer>> {
        self.integration()?.mini.borrow().clone()
    }

    /// Opens the mini player in place of the window, or closes it and shows
    /// the window again.
    pub fn toggle_mini_player(&self) {
        let Some(integration) = self.integration() else {
            return;
        };
        if let Some(mini) = self.mini_player()
            && mini.is_shown()
        {
            mini.back_to_full();
            return;
        }
        let existing = integration.mini.borrow().clone();
        let mini = existing.unwrap_or_else(|| {
            let mini = MiniPlayer::new(
                self,
                &integration.backend,
                &integration.playback,
                &integration.lyrics,
                &integration.covers,
            );
            integration.mini.replace(Some(mini.clone()));
            mini
        });
        mini.present();
        self.set_visible(false);
        self.update_position_following();
        self.refresh_tray();
    }

    /// After the mini player was hidden: with the window hidden too, that
    /// closes the program's window.
    pub(crate) fn mini_player_hidden(&self) {
        self.update_position_following();
        self.refresh_tray();
        if self.is_visible() {
            return;
        }
        if !self.close_to_tray()
            && let Some(application) = self.application()
        {
            application.quit();
        }
    }

    /// Shows the Now Playing view in the window.
    pub(crate) fn show_now_playing_view(&self) {
        if !self.now_playing_shown() {
            self.set_now_playing(true);
        }
    }

    /// The playing position is followed while the window or the mini player
    /// can be seen.
    pub(super) fn update_position_following(&self) {
        let mini = self.mini_player().is_some_and(|m| m.is_shown());
        let shown = (self.is_visible() && !self.is_suspended()) || mini;
        if let Some(backend) = self.imp().backend.borrow().as_ref() {
            backend.follow_position(shown);
        }
    }

    /// The shortcuts in other applications, or `None` where the desktop
    /// offers none.
    pub(crate) fn global_shortcut_state(&self) -> Option<GlobalShortcutState> {
        let integration = self.integration()?;
        let shortcuts = integration.shortcuts.borrow();
        let shortcuts = shortcuts.as_ref()?;
        Some(GlobalShortcutState {
            can_configure: shortcuts.can_configure(),
            bound: integration.bound.borrow().clone(),
        })
    }

    /// "Set up in the desktop…": hands the shortcuts to the desktop the first
    /// time, opens the desktop's dialog after that.
    pub(crate) fn set_up_global_shortcuts(&self) {
        let Some(integration) = self.integration() else {
            return;
        };
        let shortcuts = integration.shortcuts.borrow();
        let Some(shortcuts) = shortcuts.as_ref() else {
            return;
        };
        if integration.bound.borrow().is_some() && shortcuts.can_configure() {
            shortcuts.configure(None);
        } else {
            shortcuts.bind(global_shortcuts());
        }
        let services = integration.backend.services();
        if !services.settings().shortcuts.global {
            let _ = services.update_settings(&mut |s| s.shortcuts.global = true);
        }
    }

    /// Calls `listener` when the tray or the shortcuts in other applications
    /// come or go, for as long as `owner` lives.
    pub(crate) fn connect_desktop_changed(
        &self,
        owner: &impl IsA<glib::Object>,
        listener: impl Fn(&Self) + 'static,
    ) {
        let Some(integration) = self.integration() else {
            return;
        };
        let owner = owner.upcast_ref::<glib::Object>().downgrade();
        let window = self.downgrade();
        integration.listeners.borrow_mut().push(Box::new(move || {
            if owner.upgrade().is_some()
                && let Some(window) = window.upgrade()
            {
                listener(&window);
            }
        }));
    }

    fn desktop_changed(&self) {
        let Some(integration) = self.integration() else {
            return;
        };
        for listener in integration.listeners.borrow().iter() {
            listener();
        }
    }

    /// Feeds the window a tray host coming or going, as if from the desktop
    /// (for tests).
    #[doc(hidden)]
    pub fn demo_tray_host(&self, shown: bool) {
        self.desktop_event(Event::TrayHost(shown));
    }

    /// The tray menu's labels, as the tray would show them (for tests).
    #[doc(hidden)]
    pub fn tray_menu_labels(&self) -> Vec<String> {
        self.tray_content()
            .menu
            .into_iter()
            .map(|item| match item {
                TrayMenuItem::Text(text) => text,
                TrayMenuItem::Action { label, .. } => label,
                TrayMenuItem::Separator => "-".into(),
            })
            .collect()
    }

    /// Runs a tray menu item, as if chosen in the tray (for tests).
    #[doc(hidden)]
    pub fn demo_tray_action(&self, name: &str) {
        let action = match name {
            "play-pause" => TrayAction::PlayPause,
            "next" => TrayAction::Next,
            "previous" => TrayAction::Previous,
            "stop-after" => TrayAction::StopAfter,
            "window" => TrayAction::ToggleWindow,
            "mini-player" => TrayAction::MiniPlayer,
            _ => TrayAction::Quit,
        };
        self.desktop_event(Event::TrayAction(action));
    }

    /// The mini player's title, artist, drawer note and queue rows (for tests).
    #[doc(hidden)]
    pub fn mini_player_texts(&self) -> Option<(String, String, String, Vec<String>)> {
        self.mini_player().map(|m| m.texts())
    }

    /// The mini player's window (for tests and screenshots).
    #[doc(hidden)]
    pub fn mini_player_window(&self) -> Option<gtk::Window> {
        self.mini_player().map(|m| m.window().clone())
    }

    /// The rows marked as playing in the mini player and whether each playing
    /// indicator moves (for tests).
    #[doc(hidden)]
    pub fn mini_player_playing_marks(&self) -> (usize, Vec<bool>) {
        self.mini_player_window().map_or((0, Vec::new()), |w| {
            super::pages::playing_marks_in(w.upcast())
        })
    }

    /// Opens or closes the mini player's drawer at `tab` (for tests).
    #[doc(hidden)]
    pub fn set_mini_player_drawer(&self, open: bool, tab: &str) {
        if let Some(mini) = self.mini_player() {
            mini.set_drawer(open, tab);
        }
    }

    /// Feeds the window a lock or sleep, as if from the desktop (for tests).
    #[doc(hidden)]
    pub fn demo_session_change(&self, change: &str) {
        let change = match change {
            "lock" => SessionChange::Locked,
            "unlock" => SessionChange::Unlocked,
            "sleep" => SessionChange::GoingToSleep,
            _ => SessionChange::Woke,
        };
        self.session_changed(change);
    }
}
