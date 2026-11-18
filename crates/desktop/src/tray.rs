// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tray icon: a StatusNotifierItem (`org.kde.StatusNotifierItem`) with its
//! menu served as `com.canonical.dbusmenu`.
//!
//! [`Tray::start`] serves both objects on a connection of their own and
//! registers the item with the StatusNotifierWatcher. The desktop's tray (the
//! "host") draws the icon and the menu in its own style; the front end only
//! gives the menu's items and the tooltip ([`Tray::set_content`]), so every
//! text stays translated on its side.
//!
//! Whether a host is there can change at any time: the watcher may start after
//! the program, or go away when the panel restarts. [`TrayHooks::host`] is
//! called with `true` or `false` each time that changes (and once after
//! starting); the item registers again whenever a watcher appears. Without a
//! host there is no icon, and the front end hides its tray settings.
//!
//! Clicks, scrolling and menu items reach the hooks on zbus's executor thread;
//! a GLib front end sends them on to its main context.

mod menu;

use std::fmt;
use std::pin::pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use futures_util::StreamExt;
use futures_util::future::select;
use thiserror::Error;
use zbus::fdo::DBusProxy;
use zbus::message::Type as MessageType;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, MatchRule, MessageStream, OwnedObjectPath, fdo, interface};

pub use self::menu::MenuItemId;

/// The object path of the item.
pub const ITEM_PATH: &str = "/StatusNotifierItem";
/// The object path of the menu.
pub const MENU_PATH: &str = "/MenuBar";
/// The item interface.
pub const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
/// The menu interface.
pub const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
/// The watcher's bus name, object path and interface.
pub const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";

/// What a menu item does when it is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayAction {
    /// Play or pause.
    PlayPause,
    /// The next track.
    Next,
    /// The previous track.
    Previous,
    /// Turn "stop after this track" on or off.
    StopAfter,
    /// Show or hide the main window.
    ToggleWindow,
    /// Show or hide the mini player.
    MiniPlayer,
    /// Quit the program.
    Quit,
}

/// One item of the tray menu, top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayMenuItem {
    /// A line of text that cannot be chosen, such as the playing track.
    Text(String),
    /// An item that does something; `checked` makes it a check item.
    Action {
        /// What it does.
        action: TrayAction,
        /// Its label.
        label: String,
        /// Whether it can be chosen.
        enabled: bool,
        /// `Some` for a check item, with its state.
        checked: Option<bool>,
    },
    /// A separator line.
    Separator,
}

impl TrayMenuItem {
    /// An item that can be chosen.
    pub fn action(action: TrayAction, label: impl Into<String>) -> Self {
        Self::Action {
            action,
            label: label.into(),
            enabled: true,
            checked: None,
        }
    }

    /// A check item.
    pub fn check(action: TrayAction, label: impl Into<String>, checked: bool) -> Self {
        Self::Action {
            action,
            label: label.into(),
            enabled: true,
            checked: Some(checked),
        }
    }
}

/// What the icon shows: the tooltip and the menu.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayContent {
    /// The tooltip's title, such as "Title · Artist".
    pub tooltip: String,
    /// The menu.
    pub menu: Vec<TrayMenuItem>,
}

/// The fixed properties of the item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayConfig {
    /// The item's id (the application ID).
    pub id: String,
    /// The item's title (the application's name).
    pub title: String,
    /// The icon, by name from the icon theme.
    pub icon_name: String,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            id: crate::APP_ID.to_owned(),
            title: "Helikodu".to_owned(),
            icon_name: format!("{}-symbolic", crate::APP_ID),
        }
    }
}

type Hook<T> = Box<dyn Fn(T) + Send + Sync>;

/// Where the icon's clicks, scrolls and menu choices go. Every hook runs on
/// zbus's executor thread or the item's own thread.
pub struct TrayHooks {
    activate: Hook<()>,
    scroll: Hook<i32>,
    action: Hook<TrayAction>,
    host: Hook<bool>,
}

impl fmt::Debug for TrayHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrayHooks").finish_non_exhaustive()
    }
}

impl TrayHooks {
    /// Hooks that send menu choices to `action` and ignore everything else.
    pub fn new(action: impl Fn(TrayAction) + Send + Sync + 'static) -> Self {
        Self {
            activate: Box::new(|()| {}),
            scroll: Box::new(|_| {}),
            action: Box::new(action),
            host: Box::new(|_| {}),
        }
    }

    /// A primary click on the icon.
    #[must_use]
    pub fn with_activate(mut self, activate: impl Fn() + Send + Sync + 'static) -> Self {
        self.activate = Box::new(move |()| activate());
        self
    }

    /// Vertical scrolling over the icon: positive is up, in the host's steps
    /// (often 120 to a notch, sometimes 1).
    #[must_use]
    pub fn with_scroll(mut self, scroll: impl Fn(i32) + Send + Sync + 'static) -> Self {
        self.scroll = Box::new(scroll);
        self
    }

    /// Whether a tray host shows the icon, after each change.
    #[must_use]
    pub fn with_host(mut self, host: impl Fn(bool) + Send + Sync + 'static) -> Self {
        self.host = Box::new(host);
        self
    }
}

/// Why the tray icon could not be served.
#[derive(Debug, Error)]
pub enum TrayError {
    /// The bus failed.
    #[error("D-Bus: {0}")]
    Bus(#[from] zbus::Error),
    /// The thread that follows the watcher could not be started.
    #[error("thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// What the interfaces share.
struct Shared {
    config: TrayConfig,
    hooks: TrayHooks,
    content: Mutex<(TrayContent, u32)>,
}

impl Shared {
    fn content(&self) -> (TrayContent, u32) {
        self.content
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Counts the items of this process, for their bus names.
static ITEMS: AtomicU32 = AtomicU32::new(0);

/// The tray icon, shown for as long as this lives.
pub struct Tray {
    connection: Connection,
    shared: Arc<Shared>,
    bus_name: String,
    stop: async_channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for Tray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tray")
            .field("bus_name", &self.bus_name)
            .finish_non_exhaustive()
    }
}

impl Tray {
    /// Serves the icon on the session bus.
    pub fn start(config: TrayConfig, hooks: TrayHooks) -> Result<Self, TrayError> {
        Self::serve(zbus::connection::Builder::session(), config, hooks)
    }

    /// Serves the icon on the bus at `address`, such as a private bus in a test.
    pub fn start_at(
        address: &str,
        config: TrayConfig,
        hooks: TrayHooks,
    ) -> Result<Self, TrayError> {
        Self::serve(zbus::connection::Builder::address(address), config, hooks)
    }

    fn serve(
        builder: zbus::connection::Builder<'_>,
        config: TrayConfig,
        hooks: TrayHooks,
    ) -> Result<Self, TrayError> {
        let shared = Arc::new(Shared {
            config,
            hooks,
            content: Mutex::new((TrayContent::default(), 1)),
        });
        let number = ITEMS.fetch_add(1, Ordering::Relaxed) + 1;
        let bus_name = format!("org.kde.StatusNotifierItem-{}-{number}", std::process::id());
        let connection = zbus::block_on(
            builder
                .serve_at(
                    ITEM_PATH,
                    Item {
                        shared: Arc::clone(&shared),
                    },
                )
                .serve_at(
                    MENU_PATH,
                    menu::Menu {
                        shared: Arc::clone(&shared),
                    },
                )
                .name(bus_name.as_str())
                .build(),
        )?;
        let (stop, stopped) = async_channel::bounded::<()>(1);
        let watch = connection.clone();
        let watched = Arc::clone(&shared);
        let name = bus_name.clone();
        let thread = thread::Builder::new()
            .name("helikodu-tray".into())
            .spawn(move || {
                zbus::block_on(async move {
                    let follow = follow_watcher(&watch, &watched, &name);
                    let _ = select(pin!(follow), pin!(stopped.recv())).await;
                });
            })?;
        Ok(Self {
            connection,
            shared,
            bus_name,
            stop,
            thread: Some(thread),
        })
    }

    /// The bus name of the item.
    pub fn bus_name(&self) -> &str {
        &self.bus_name
    }

    /// Changes the tooltip and the menu; the host is told only what changed.
    pub fn set_content(&self, content: TrayContent) {
        let (tooltip_changed, menu_revision) = {
            let mut current = self
                .shared
                .content
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if current.0 == content {
                return;
            }
            let tooltip_changed = current.0.tooltip != content.tooltip;
            let menu_changed = current.0.menu != content.menu;
            if menu_changed {
                current.1 = current.1.wrapping_add(1);
            }
            current.0 = content;
            (tooltip_changed, menu_changed.then_some(current.1))
        };
        let connection = &self.connection;
        // A lost signal is made good by the next change; nothing to report.
        let _ = zbus::block_on(async {
            if tooltip_changed {
                connection
                    .emit_signal(
                        None::<BusName<'_>>,
                        ITEM_PATH,
                        ITEM_INTERFACE,
                        "NewToolTip",
                        &(),
                    )
                    .await?;
            }
            if let Some(revision) = menu_revision {
                connection
                    .emit_signal(
                        None::<BusName<'_>>,
                        MENU_PATH,
                        MENU_INTERFACE,
                        "LayoutUpdated",
                        &(revision, 0i32),
                    )
                    .await?;
            }
            zbus::Result::Ok(())
        });
    }

    /// The content shown.
    pub fn content(&self) -> TrayContent {
        self.shared.content().0
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What can change about the watcher and its hosts.
enum WatcherEvent {
    /// The watcher's name got an owner, or lost it.
    Owner(bool),
    /// A host came or went.
    Hosts,
}

/// Registers with the watcher whenever one appears and reports whether a host
/// shows the icon, until the connection ends.
async fn follow_watcher(connection: &Connection, shared: &Shared, name: &str) {
    let Ok(dbus) = DBusProxy::new(connection).await else {
        (shared.hooks.host)(false);
        return;
    };
    let owners = match dbus
        .receive_name_owner_changed_with_args(&[(0, WATCHER)])
        .await
    {
        Ok(owners) => owners.map(|signal| {
            let owned = signal
                .args()
                .is_ok_and(|args| args.new_owner().as_ref().is_some());
            WatcherEvent::Owner(owned)
        }),
        Err(_) => {
            (shared.hooks.host)(false);
            return;
        }
    };
    let hosts = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(WATCHER)
        .build();
    let hosts = match hosts {
        Ok(rule) => MessageStream::for_match_rule(rule, connection, None)
            .await
            .ok(),
        Err(_) => None,
    };
    let hosts = futures_util::stream::iter(hosts)
        .flatten()
        .filter_map(|message| async move {
            let message = message.ok()?;
            let member = message.header().member()?.to_string();
            member
                .starts_with("StatusNotifierHost")
                .then_some(WatcherEvent::Hosts)
        });
    let mut events = pin!(futures_util::stream::select(owners, hosts));

    let mut shown = check_in(connection, name, true).await;
    (shared.hooks.host)(shown);
    while let Some(event) = events.next().await {
        let now = match event {
            WatcherEvent::Owner(false) => false,
            WatcherEvent::Owner(true) => check_in(connection, name, true).await,
            WatcherEvent::Hosts => check_in(connection, name, false).await,
        };
        if now != shown {
            shown = now;
            (shared.hooks.host)(shown);
        }
    }
}

/// Registers the item (when `register`) and says whether a host is there.
async fn check_in(connection: &Connection, name: &str, register: bool) -> bool {
    let Ok(watcher) = zbus::Proxy::new(connection, WATCHER, WATCHER_PATH, WATCHER).await else {
        return false;
    };
    if register
        && watcher
            .call_method("RegisterStatusNotifierItem", &(name,))
            .await
            .is_err()
    {
        return false;
    }
    watcher
        .get_property::<bool>("IsStatusNotifierHostRegistered")
        .await
        .unwrap_or(false)
}

/// An icon as the protocol sends it: width, height and ARGB32 data.
type Pixmap = (i32, i32, Vec<u8>);
/// A tooltip: icon name, icon pixmaps, title and text.
type ToolTip = (String, Vec<Pixmap>, String, String);

/// The item: icon, tooltip and clicks.
struct Item {
    shared: Arc<Shared>,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> String {
        self.shared.config.id.clone()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        self.shared.config.title.clone()
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        self.shared.config.icon_name.clone()
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn tool_tip(&self) -> ToolTip {
        let (content, _) = self.shared.content();
        let title = if content.tooltip.is_empty() {
            self.shared.config.title.clone()
        } else {
            content.tooltip
        };
        (String::new(), Vec::new(), title, String::new())
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).expect("a valid object path")
    }

    fn activate(&self, _x: i32, _y: i32) {
        (self.shared.hooks.activate)(());
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {}

    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, delta: i32, orientation: &str) {
        if orientation.eq_ignore_ascii_case("vertical") && delta != 0 {
            (self.shared.hooks.scroll)(delta);
        }
    }

    #[zbus(signal)]
    async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
}

/// The error of a call naming a menu item that does not exist.
fn no_item(id: i32) -> fdo::Error {
    fdo::Error::InvalidArgs(format!("no menu item {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_config_names_the_application() {
        let config = TrayConfig::default();
        assert_eq!(config.id, crate::APP_ID);
        assert_eq!(config.icon_name, "io.github.sorrelfox.Helikodu-symbolic");
    }

    #[test]
    fn helpers_build_items() {
        assert_eq!(
            TrayMenuItem::check(TrayAction::StopAfter, "Stop", true),
            TrayMenuItem::Action {
                action: TrayAction::StopAfter,
                label: "Stop".into(),
                enabled: true,
                checked: Some(true),
            }
        );
    }
}
