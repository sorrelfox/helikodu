// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shortcuts that work while another window is in front, through the XDG
//! GlobalShortcuts portal (`org.freedesktop.portal.GlobalShortcuts`).
//!
//! The desktop chooses the keys: the program only names its shortcuts and
//! describes them. [`GlobalShortcuts::start`] opens a portal session without
//! asking anything, so that starting the program never shows a dialog;
//! [`GlobalShortcuts::bind`] then hands over the shortcuts, which on most
//! desktops asks the user once to confirm or choose the keys. The front end
//! binds at start only after the user has set the shortcuts up before.
//! [`GlobalShortcuts::configure`] opens the desktop's own dialog again
//! (version 2 of the portal).
//!
//! What happens arrives as [`ShortcutEvent`]s on the session's own thread:
//! a shortcut pressed, or the keys the desktop now gives the shortcuts.
//! Without a portal [`GlobalShortcuts::start`] fails, and the front end hides
//! everything about these shortcuts.

use std::fmt;
use std::pin::pin;
use std::thread::{self, JoinHandle};

use ashpd::desktop::global_shortcuts::{GlobalShortcuts as Portal, NewShortcut, Shortcut};
use ashpd::desktop::{CreateSessionOptions, Session};
use futures_util::StreamExt;
use thiserror::Error;
use zbus::Connection;

/// A shortcut the program offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutSpec {
    /// Its id, such as `play-pause`.
    pub id: String,
    /// What it does, in the user's language.
    pub description: String,
}

/// A shortcut as the desktop has bound it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundShortcut {
    /// Its id.
    pub id: String,
    /// What it does.
    pub description: String,
    /// The keys, as the desktop describes them (such as `Meta+F8`); empty when
    /// none are set.
    pub trigger: String,
}

impl From<&Shortcut> for BoundShortcut {
    fn from(shortcut: &Shortcut) -> Self {
        Self {
            id: shortcut.id().to_owned(),
            description: shortcut.description().to_owned(),
            trigger: shortcut.trigger_description().to_owned(),
        }
    }
}

/// What the portal reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutEvent {
    /// The shortcut with this id was pressed.
    Activated(String),
    /// The shortcuts and their keys, after binding or after the user changed
    /// them in the desktop's settings.
    Bound(Vec<BoundShortcut>),
    /// Binding failed or was refused.
    Failed(String),
}

/// Why the portal session could not be started.
#[derive(Debug, Error)]
pub enum ShortcutsError {
    /// No portal, or it failed.
    #[error("GlobalShortcuts portal: {0}")]
    Portal(#[from] ashpd::Error),
    /// The bus failed.
    #[error("D-Bus: {0}")]
    Bus(#[from] zbus::Error),
    /// The thread could not be started.
    #[error("thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// A request to the session's thread.
enum Command {
    Bind(Vec<ShortcutSpec>),
    Configure(Option<String>),
    Stop,
}

/// A GlobalShortcuts portal session, open for as long as this lives.
pub struct GlobalShortcuts {
    commands: async_channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    version: u32,
}

impl fmt::Debug for GlobalShortcuts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlobalShortcuts")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl GlobalShortcuts {
    /// Opens a session with the portal on the session bus. `on_event` gets what
    /// the portal reports, on the session's thread.
    pub fn start(
        on_event: impl Fn(ShortcutEvent) + Send + 'static,
    ) -> Result<Self, ShortcutsError> {
        let connection = zbus::block_on(Connection::session())?;
        Self::start_on(connection, on_event)
    }

    /// The same on the bus at `address`, such as a private bus in a test.
    pub fn start_at(
        address: &str,
        on_event: impl Fn(ShortcutEvent) + Send + 'static,
    ) -> Result<Self, ShortcutsError> {
        let connection = zbus::block_on(zbus::connection::Builder::address(address).build())?;
        Self::start_on(connection, on_event)
    }

    fn start_on(
        connection: Connection,
        on_event: impl Fn(ShortcutEvent) + Send + 'static,
    ) -> Result<Self, ShortcutsError> {
        let (commands, received) = async_channel::unbounded::<Command>();
        let (first, first_result) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("helikodu-shortcuts".into())
            .spawn(move || {
                zbus::block_on(async move {
                    let opened = open(connection).await;
                    let (portal, session, activated, changed) = match opened {
                        Ok(opened) => opened,
                        Err(error) => {
                            let _ = first.send(Err(error));
                            return;
                        }
                    };
                    let _ = first.send(Ok(portal.version()));
                    // Signals of the portal reach only the session's owner, and
                    // this connection has one session.
                    let activated =
                        activated.map(|signal| Step::Activated(signal.shortcut_id().to_owned()));
                    let changed = changed.map(|signal| {
                        Step::Bound(signal.shortcuts().iter().map(BoundShortcut::from).collect())
                    });
                    let commands = received.map(Step::Command);
                    let mut steps = pin!(futures_util::stream::select(
                        futures_util::stream::select(activated, changed),
                        commands
                    ));
                    while let Some(step) = steps.next().await {
                        match step {
                            Step::Activated(id) => on_event(ShortcutEvent::Activated(id)),
                            Step::Bound(bound) => on_event(ShortcutEvent::Bound(bound)),
                            Step::Command(Command::Bind(specs)) => {
                                on_event(bind(&portal, &session, &specs).await);
                            }
                            Step::Command(Command::Configure(token)) => {
                                let mut options = ashpd::desktop::global_shortcuts::ConfigureShortcutsOptions::default();
                                if let Some(token) = token {
                                    options = options.set_activation_token(
                                        ashpd::ActivationToken::from(token),
                                    );
                                }
                                if let Err(error) =
                                    portal.configure_shortcuts(&session, None, options).await
                                {
                                    on_event(ShortcutEvent::Failed(error.to_string()));
                                }
                            }
                            Step::Command(Command::Stop) => break,
                        }
                    }
                    let _ = session.close().await;
                });
            })?;
        match first_result.recv() {
            Ok(Ok(version)) => Ok(Self {
                commands,
                thread: Some(thread),
                version,
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => Err(ShortcutsError::Thread(std::io::Error::other(
                "the shortcuts thread ended",
            ))),
        }
    }

    /// Hands the shortcuts to the desktop, which may ask the user to confirm
    /// them or choose keys. The result arrives as [`ShortcutEvent::Bound`] or
    /// [`ShortcutEvent::Failed`].
    pub fn bind(&self, shortcuts: Vec<ShortcutSpec>) {
        let _ = self.commands.try_send(Command::Bind(shortcuts));
    }

    /// Whether the desktop's dialog can be opened again ([`Self::configure`]).
    pub fn can_configure(&self) -> bool {
        self.version >= 2
    }

    /// Opens the desktop's dialog for the shortcuts; `activation_token` lets it
    /// take the focus on Wayland.
    pub fn configure(&self, activation_token: Option<String>) {
        let _ = self.commands.try_send(Command::Configure(activation_token));
    }
}

impl Drop for GlobalShortcuts {
    fn drop(&mut self) {
        let _ = self.commands.try_send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One thing for the session's thread to do.
enum Step {
    Activated(String),
    Bound(Vec<BoundShortcut>),
    Command(Command),
}

type Opened = (
    Portal,
    Session<Portal>,
    std::pin::Pin<
        Box<dyn futures_util::Stream<Item = ashpd::desktop::global_shortcuts::Activated> + Send>,
    >,
    std::pin::Pin<
        Box<
            dyn futures_util::Stream<Item = ashpd::desktop::global_shortcuts::ShortcutsChanged>
                + Send,
        >,
    >,
);

/// Opens the portal and a session, listening before anything can happen.
async fn open(connection: Connection) -> Result<Opened, ShortcutsError> {
    let portal = Portal::with_connection(connection).await?;
    let activated = portal.receive_activated().await?;
    let changed = portal.receive_shortcuts_changed().await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    Ok((portal, session, Box::pin(activated), Box::pin(changed)))
}

async fn bind(portal: &Portal, session: &Session<Portal>, specs: &[ShortcutSpec]) -> ShortcutEvent {
    let shortcuts: Vec<NewShortcut> = specs
        .iter()
        .map(|spec| NewShortcut::new(spec.id.as_str(), spec.description.as_str()))
        .collect();
    let result = async {
        portal
            .bind_shortcuts(session, &shortcuts, None, Default::default())
            .await?
            .response()
    }
    .await;
    match result {
        Ok(bound) => {
            ShortcutEvent::Bound(bound.shortcuts().iter().map(BoundShortcut::from).collect())
        }
        Err(error) => ShortcutEvent::Failed(error.to_string()),
    }
}
