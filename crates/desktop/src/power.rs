// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keeping the system from suspending while music plays.
//!
//! [`SuspendInhibition`] follows the player: it inhibits suspend when playback
//! starts and releases it when playback pauses or stops. Opening or seeking
//! (`Buffering`) changes nothing, so a seek does not drop and take the
//! inhibition again.
//!
//! [`SystemInhibitor`] asks the XDG Inhibit portal (`org.freedesktop.portal.Inhibit`,
//! flag "suspend"), which works inside and outside a sandbox wherever a portal
//! runs. Outside a sandbox, when no portal answers, it takes a logind "sleep"
//! lock in "block" mode on the system bus instead
//! (`org.freedesktop.login1.Manager.Inhibit`). The screen still blanks and locks
//! as usual; only suspend waits.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ashpd::desktop::Request;
use ashpd::desktop::inhibit::{InhibitFlags, InhibitOptions, InhibitProxy};
use helikodu_model::player::PlaybackState;
use helikodu_service::bus::Subscription;
use helikodu_service::player::PlayerEvent;
use thiserror::Error;
use zbus::Connection;

/// The reason shown by the desktop for the inhibition.
pub const DEFAULT_REASON: &str = "Playing music";

/// Why suspend could not be inhibited or released.
#[derive(Debug, Error)]
pub enum PowerError {
    /// The Inhibit portal refused or failed.
    #[error("Inhibit portal: {0}")]
    Portal(#[from] ashpd::Error),
    /// The bus failed (logind, or connecting at all).
    #[error("D-Bus: {0}")]
    Bus(#[from] zbus::Error),
    /// No way to inhibit suspend is available.
    #[error("suspend cannot be inhibited: {0}")]
    Unavailable(String),
}

/// Something that can keep the system from suspending.
pub trait Inhibitor: Send {
    /// Starts inhibiting suspend, giving `reason`. Called only when not
    /// inhibiting.
    fn inhibit(&mut self, reason: &str) -> Result<(), PowerError>;
    /// Ends the inhibition. Called only while inhibiting.
    fn release(&mut self) -> Result<(), PowerError>;
}

/// What holds the system awake.
enum Hold {
    /// An Inhibit portal request; closing it ends the inhibition.
    Portal(Box<Request<()>>),
    /// A logind inhibitor lock; closing the descriptor ends it.
    Logind(zbus::OwnedFd),
}

/// The Inhibit portal, with logind as the fallback outside a sandbox.
pub struct SystemInhibitor {
    session: Option<Connection>,
    hold: Option<Hold>,
    sandboxed: bool,
}

impl fmt::Debug for SystemInhibitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hold = match &self.hold {
            Some(Hold::Portal(_)) => "portal",
            Some(Hold::Logind(_)) => "logind",
            None => "none",
        };
        f.debug_struct("SystemInhibitor")
            .field("hold", &hold)
            .field("sandboxed", &self.sandboxed)
            .finish_non_exhaustive()
    }
}

impl SystemInhibitor {
    /// An inhibitor that connects to the session bus when first used.
    pub fn new() -> Self {
        Self {
            session: None,
            hold: None,
            sandboxed: ashpd::is_sandboxed(),
        }
    }

    /// Whether the inhibition is held through the portal (`Some(true)`), logind
    /// (`Some(false)`) or not at all.
    pub fn through_portal(&self) -> Option<bool> {
        self.hold
            .as_ref()
            .map(|hold| matches!(hold, Hold::Portal(_)))
    }

    fn session(&mut self) -> Result<Connection, PowerError> {
        if let Some(connection) = &self.session {
            return Ok(connection.clone());
        }
        let connection = zbus::block_on(Connection::session())?;
        self.session = Some(connection.clone());
        Ok(connection)
    }

    fn inhibit_portal(&mut self, reason: &str) -> Result<Hold, PowerError> {
        let connection = self.session()?;
        let request = zbus::block_on(async {
            let proxy = InhibitProxy::with_connection(connection).await?;
            proxy
                .inhibit(
                    None,
                    InhibitFlags::SUSPEND,
                    InhibitOptions::default().set_reason(reason),
                )
                .await
        })?;
        Ok(Hold::Portal(Box::new(request)))
    }
}

impl Default for SystemInhibitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Inhibitor for SystemInhibitor {
    fn inhibit(&mut self, reason: &str) -> Result<(), PowerError> {
        let hold = match self.inhibit_portal(reason) {
            Ok(hold) => hold,
            Err(error) if self.sandboxed => return Err(error),
            Err(portal) => match inhibit_logind(reason) {
                Ok(fd) => Hold::Logind(fd),
                Err(logind) => {
                    return Err(PowerError::Unavailable(format!(
                        "{portal}; logind: {logind}"
                    )));
                }
            },
        };
        self.hold = Some(hold);
        Ok(())
    }

    fn release(&mut self) -> Result<(), PowerError> {
        match self.hold.take() {
            Some(Hold::Portal(request)) => zbus::block_on(request.close())?,
            // Closing the descriptor releases the lock.
            Some(Hold::Logind(fd)) => drop(fd),
            None => {}
        }
        Ok(())
    }
}

/// Takes a logind "sleep" inhibitor lock in "block" mode.
fn inhibit_logind(reason: &str) -> Result<zbus::OwnedFd, zbus::Error> {
    zbus::block_on(async {
        let system = Connection::system().await?;
        let reply = system
            .call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "Inhibit",
                &("sleep", "Helikodu", reason, "block"),
            )
            .await?;
        reply.body().deserialize::<zbus::OwnedFd>()
    })
}

/// Inhibits suspend while the player plays.
#[derive(Debug)]
pub struct SuspendInhibition<I: Inhibitor = SystemInhibitor> {
    inhibitor: I,
    reason: String,
    held: bool,
}

impl SuspendInhibition<SystemInhibitor> {
    /// Through the portal or logind, with [`DEFAULT_REASON`].
    pub fn system() -> Self {
        Self::new(SystemInhibitor::new())
    }
}

impl<I: Inhibitor> SuspendInhibition<I> {
    /// Through `inhibitor`, with [`DEFAULT_REASON`].
    pub fn new(inhibitor: I) -> Self {
        Self {
            inhibitor,
            reason: DEFAULT_REASON.to_owned(),
            held: false,
        }
    }

    /// The same with another reason (translated by the front end).
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    /// Whether suspend is inhibited now.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// The inhibitor.
    pub fn inhibitor(&self) -> &I {
        &self.inhibitor
    }

    /// Inhibits or releases. A failed attempt is tried again at the next change.
    pub fn set_playing(&mut self, playing: bool) -> Result<(), PowerError> {
        if playing == self.held {
            return Ok(());
        }
        if playing {
            self.inhibitor.inhibit(&self.reason)?;
        } else {
            // Even if releasing fails, there is nothing to hold on to.
            self.held = false;
            self.inhibitor.release()?;
        }
        self.held = playing;
        Ok(())
    }

    /// Follows one event of the player.
    pub fn handle(&mut self, event: &PlayerEvent) -> Result<(), PowerError> {
        match event {
            PlayerEvent::State { playback, .. } => match playback {
                PlaybackState::Playing => self.set_playing(true),
                PlaybackState::Paused | PlaybackState::Stopped => self.set_playing(false),
                PlaybackState::Buffering => Ok(()),
            },
            PlayerEvent::Ended { .. } => self.set_playing(false),
            _ => Ok(()),
        }
    }
}

impl<I: Inhibitor + 'static> SuspendInhibition<I> {
    /// Follows `events` on a thread of its own until the returned handle is
    /// dropped (which releases the inhibition) or the subscription ends.
    /// `on_error` hears of failures, for a log.
    pub fn follow(
        mut self,
        events: Subscription<PlayerEvent>,
        on_error: impl Fn(PowerError) + Send + 'static,
    ) -> std::io::Result<InhibitionFollower> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("helikodu-inhibit".into())
            .spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    match events.recv_timeout(Duration::from_millis(100)) {
                        Ok(event) => {
                            if let Err(error) = self.handle(&event) {
                                on_error(error);
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                if let Err(error) = self.set_playing(false) {
                    on_error(error);
                }
            })?;
        Ok(InhibitionFollower {
            stop,
            thread: Some(thread),
        })
    }
}

impl<I: Inhibitor> Drop for SuspendInhibition<I> {
    fn drop(&mut self) {
        if self.held {
            let _ = self.inhibitor.release();
        }
    }
}

/// The thread of [`SuspendInhibition::follow`]; dropping it releases the
/// inhibition and ends the thread.
#[derive(Debug)]
pub struct InhibitionFollower {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for InhibitionFollower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use helikodu_model::player::TransportState;
    use helikodu_service::bus::EventBus;
    use helikodu_service::player::EndReason;

    use super::*;

    /// Records what it was asked; fails the next inhibit when told to.
    #[derive(Debug, Default, Clone)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        fail_next: Arc<AtomicBool>,
    }

    impl Inhibitor for Fake {
        fn inhibit(&mut self, reason: &str) -> Result<(), PowerError> {
            if self.fail_next.swap(false, Ordering::AcqRel) {
                return Err(PowerError::Unavailable("no portal".into()));
            }
            self.calls.lock().unwrap().push(format!("inhibit {reason}"));
            Ok(())
        }

        fn release(&mut self) -> Result<(), PowerError> {
            self.calls.lock().unwrap().push("release".into());
            Ok(())
        }
    }

    fn state(playback: PlaybackState) -> PlayerEvent {
        PlayerEvent::State {
            playback,
            transport: TransportState::Playing,
        }
    }

    #[test]
    fn inhibits_while_playing_only() {
        let fake = Fake::default();
        let mut inhibition = SuspendInhibition::new(fake.clone()).with_reason("Music");
        for event in [
            state(PlaybackState::Buffering),
            state(PlaybackState::Playing),
            state(PlaybackState::Buffering),
            state(PlaybackState::Playing),
            state(PlaybackState::Paused),
            state(PlaybackState::Paused),
            state(PlaybackState::Playing),
            PlayerEvent::Ended {
                reason: EndReason::QueueFinished,
                played: 1,
                failed: 0,
            },
        ] {
            inhibition.handle(&event).unwrap();
        }
        assert!(!inhibition.is_held());
        assert_eq!(
            *fake.calls.lock().unwrap(),
            ["inhibit Music", "release", "inhibit Music", "release"]
        );
    }

    #[test]
    fn a_failure_is_tried_again_and_drop_releases() {
        let fake = Fake::default();
        fake.fail_next.store(true, Ordering::Release);
        let mut inhibition = SuspendInhibition::new(fake.clone());
        assert!(inhibition.handle(&state(PlaybackState::Playing)).is_err());
        assert!(!inhibition.is_held());
        inhibition.handle(&state(PlaybackState::Playing)).unwrap();
        assert!(inhibition.is_held());
        drop(inhibition);
        assert_eq!(
            *fake.calls.lock().unwrap(),
            [format!("inhibit {DEFAULT_REASON}"), "release".to_owned()]
        );
    }

    #[test]
    fn follows_the_event_bus() {
        let fake = Fake::default();
        let bus = EventBus::<PlayerEvent>::new();
        let events = bus.subscribe_with(|event: &PlayerEvent| Some(event.clone()), None);
        let follower = SuspendInhibition::new(fake.clone())
            .follow(events, |error| panic!("{error}"))
            .unwrap();
        bus.publish(&state(PlaybackState::Playing));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while fake.calls.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        drop(follower);
        assert_eq!(
            *fake.calls.lock().unwrap(),
            [format!("inhibit {DEFAULT_REASON}"), "release".to_owned()]
        );
    }
}
