// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Following the screen lock and the system going to sleep, so that playback
//! can pause.
//!
//! - **Screen lock:** the Inhibit portal's session monitor
//!   (`org.freedesktop.portal.Inhibit.CreateMonitor`, `screensaver-active` in
//!   `StateChanged`), which works inside a sandbox; and logind's session on the
//!   system bus (`Lock` and `Unlock`, and the `LockedHint` property that
//!   GNOME and KDE Plasma set). Both may report the same lock; only changes
//!   are passed on.
//! - **Sleep:** logind's `PrepareForSleep` on the system bus. So that playback
//!   has paused before the system sleeps, a "delay" inhibitor lock is held
//!   while [`SessionWatch::set_delay_sleep`] asks for it: when sleep comes the
//!   front end is told, the watch waits [`SLEEP_GRACE`] and then lets go of the
//!   lock. After waking the lock is taken again.
//!
//! A source that is not there (no portal, no system bus, no logind) is left
//! out; the watch itself always starts. The callback runs on the watch's own
//! thread.

use std::fmt;
use std::pin::{Pin, pin};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ashpd::desktop::inhibit::{CreateMonitorOptions, InhibitProxy, SessionState};
use futures_util::{Stream, StreamExt};
use zbus::{Connection, OwnedObjectPath};

/// How long the watch waits after announcing sleep before it lets the system
/// go to sleep: time for the pause and its fade.
pub const SLEEP_GRACE: Duration = Duration::from_millis(400);

const LOGIN1: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";

/// What happened to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionChange {
    /// The screen locked.
    Locked,
    /// The screen was unlocked.
    Unlocked,
    /// The system is about to sleep.
    GoingToSleep,
    /// The system woke up.
    Woke,
}

enum Command {
    DelaySleep(bool),
    Stop,
}

/// Follows the screen lock and sleep until dropped.
pub struct SessionWatch {
    commands: async_channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for SessionWatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionWatch").finish_non_exhaustive()
    }
}

impl SessionWatch {
    /// Follows the session on the session and system buses.
    pub fn start(on_change: impl Fn(SessionChange) + Send + 'static) -> Self {
        let session = zbus::block_on(Connection::session()).ok();
        let system = zbus::block_on(Connection::system()).ok();
        Self::start_on(session, system, on_change)
    }

    /// The same on the buses at the given addresses, such as private buses in
    /// a test (the same address may be given twice).
    pub fn start_at(
        session: Option<&str>,
        system: Option<&str>,
        on_change: impl Fn(SessionChange) + Send + 'static,
    ) -> Self {
        let connect = |address: &str| {
            zbus::block_on(zbus::connection::Builder::address(address).build()).ok()
        };
        Self::start_on(
            session.and_then(connect),
            system.and_then(connect),
            on_change,
        )
    }

    fn start_on(
        session: Option<Connection>,
        system: Option<Connection>,
        on_change: impl Fn(SessionChange) + Send + 'static,
    ) -> Self {
        let (commands, received) = async_channel::unbounded::<Command>();
        let (ready, started) = std::sync::mpsc::sync_channel::<()>(1);
        let thread = thread::Builder::new()
            .name("helikodu-session".into())
            .spawn(move || {
                zbus::block_on(watch(session, system, received, ready, on_change));
            })
            .ok();
        // Every source listens before this returns, so that no change made
        // right after it is missed.
        if thread.is_some() {
            let _ = started.recv_timeout(Duration::from_secs(5));
        }
        Self { commands, thread }
    }

    /// Whether sleep waits for the front end to pause (see [`SLEEP_GRACE`]).
    pub fn set_delay_sleep(&self, on: bool) {
        let _ = self.commands.try_send(Command::DelaySleep(on));
    }
}

impl Drop for SessionWatch {
    fn drop(&mut self) {
        let _ = self.commands.try_send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One thing for the watch to act on.
enum Step {
    /// The screen saver or the lock started (`true`) or ended.
    Lock(bool),
    /// `PrepareForSleep`.
    Sleep(bool),
    /// The portal asks whether the session may end.
    QueryEnd,
    Command(Command),
}

type Steps = Pin<Box<dyn Stream<Item = Step> + Send>>;

async fn watch(
    session: Option<Connection>,
    system: Option<Connection>,
    commands: async_channel::Receiver<Command>,
    ready: std::sync::mpsc::SyncSender<()>,
    on_change: impl Fn(SessionChange),
) {
    let mut sources: Vec<Steps> = vec![Box::pin(commands.map(Step::Command))];
    let mut monitor = None;
    if let Some(session) = session
        && let Some((portal, session_monitor, steps)) = portal_monitor(session).await
    {
        monitor = Some((portal, session_monitor));
        sources.push(steps);
    }
    let manager = match &system {
        Some(system) => logind(system, &mut sources).await,
        None => None,
    };
    let _ = ready.send(());

    let mut steps = pin!(futures_util::stream::select_all(sources));
    let mut locked = false;
    let mut delay = false;
    let mut hold: Option<zbus::OwnedFd> = None;
    while let Some(step) = steps.next().await {
        match step {
            Step::Lock(now) => {
                if now != locked {
                    locked = now;
                    on_change(if now {
                        SessionChange::Locked
                    } else {
                        SessionChange::Unlocked
                    });
                }
            }
            Step::Sleep(true) => {
                on_change(SessionChange::GoingToSleep);
                if hold.take().is_some() {
                    // Time for the pause, then let the system sleep.
                    thread::sleep(SLEEP_GRACE);
                }
            }
            Step::Sleep(false) => {
                on_change(SessionChange::Woke);
                if delay && let Some(manager) = &manager {
                    hold = delay_lock(manager).await;
                }
            }
            Step::QueryEnd => {
                if let Some((portal, monitor)) = &monitor {
                    let _ = portal.query_end_response(monitor).await;
                }
            }
            Step::Command(Command::DelaySleep(on)) => {
                delay = on;
                if !on {
                    hold = None;
                } else if hold.is_none()
                    && let Some(manager) = &manager
                {
                    hold = delay_lock(manager).await;
                }
            }
            Step::Command(Command::Stop) => break,
        }
    }
    if let Some((_, monitor)) = monitor {
        let _ = monitor.close().await;
    }
}

type Monitor = (InhibitProxy, ashpd::desktop::Session<InhibitProxy>, Steps);

/// The Inhibit portal's session monitor, if there is one.
async fn portal_monitor(connection: Connection) -> Option<Monitor> {
    let portal = InhibitProxy::with_connection(connection).await.ok()?;
    let states = portal.receive_state_changed().await.ok()?;
    let monitor = portal
        .create_monitor(None, CreateMonitorOptions::default())
        .await
        .ok()?;
    let steps = states.flat_map(|state| {
        let mut steps = vec![Step::Lock(state.screensaver_active())];
        if state.session_state() == SessionState::QueryEnd {
            steps.push(Step::QueryEnd);
        }
        futures_util::stream::iter(steps)
    });
    Some((portal, monitor, Box::pin(steps)))
}

/// Adds logind's sleep and lock signals to `sources`; returns the manager.
async fn logind<'a>(system: &'a Connection, sources: &mut Vec<Steps>) -> Option<zbus::Proxy<'a>> {
    let manager = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(system)
        .destination(LOGIN1)
        .path(LOGIN1_PATH)
        .interface(MANAGER)
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .ok()?;
    if let Ok(sleep) = manager.receive_signal("PrepareForSleep").await {
        sources.push(Box::pin(sleep.filter_map(|message| async move {
            message.body().deserialize::<bool>().ok().map(Step::Sleep)
        })));
    }
    if let Some(path) = own_session(&manager).await
        && let Ok(session) = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(system)
            .destination(LOGIN1)
            .path(path)
            .interface(SESSION)
            .build()
            .await
    {
        for (member, locked) in [("Lock", true), ("Unlock", false)] {
            if let Ok(signals) = session.receive_signal(member).await {
                sources.push(Box::pin(signals.map(move |_| Step::Lock(locked))));
            }
        }
        let hint = session.receive_property_changed::<bool>("LockedHint").await;
        sources.push(Box::pin(hint.filter_map(|change| async move {
            change.get().await.ok().map(Step::Lock)
        })));
    }
    Some(manager)
}

/// The logind session this process belongs to, or the user's display session.
async fn own_session(manager: &zbus::Proxy<'_>) -> Option<OwnedObjectPath> {
    if let Ok(reply) = manager
        .call_method("GetSessionByPID", &(std::process::id(),))
        .await
        && let Ok(path) = reply.body().deserialize::<OwnedObjectPath>()
    {
        return Some(path);
    }
    let reply = manager.call_method("GetSession", &("auto",)).await.ok()?;
    reply.body().deserialize::<OwnedObjectPath>().ok()
}

/// Takes a logind "delay" lock on sleep.
async fn delay_lock(manager: &zbus::Proxy<'_>) -> Option<zbus::OwnedFd> {
    let reply = manager
        .call_method(
            "Inhibit",
            &(
                "sleep",
                "Helikodu",
                "Pausing playback before sleep",
                "delay",
            ),
        )
        .await
        .ok()?;
    reply.body().deserialize::<zbus::OwnedFd>().ok()
}
