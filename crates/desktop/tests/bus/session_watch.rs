// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The screen lock and sleep watch against a fake logind and a fake Inhibit
//! portal on a private bus.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helikodu_desktop::session_watch::{SessionChange, SessionWatch};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Variant, interface};

use super::fake_portal::{DESKTOP_PATH, respond, session_results};
use super::private_bus::PrivateBus;

const WAIT: Duration = Duration::from_secs(5);
const SESSION_PATH: &str = "/org/freedesktop/login1/session/_31";

struct FakeManager {
    inhibits: Arc<AtomicUsize>,
}

#[interface(name = "org.freedesktop.login1.Manager")]
impl FakeManager {
    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, _pid: u32) -> OwnedObjectPath {
        OwnedObjectPath::try_from(SESSION_PATH).unwrap()
    }

    fn inhibit(&self, what: &str, _who: &str, _why: &str, mode: &str) -> zbus::OwnedFd {
        assert_eq!((what, mode), ("sleep", "delay"));
        self.inhibits.fetch_add(1, Ordering::SeqCst);
        let file = std::fs::File::open("/dev/null").unwrap();
        zbus::OwnedFd::from(std::os::fd::OwnedFd::from(file))
    }
}

struct FakeSession;

#[interface(name = "org.freedesktop.login1.Session")]
impl FakeSession {
    #[zbus(property)]
    fn locked_hint(&self) -> bool {
        false
    }
}

fn changes(bus: &PrivateBus, session: bool) -> (SessionWatch, Receiver<SessionChange>) {
    let (sender, received) = mpsc::channel();
    let watch = SessionWatch::start_at(
        session.then_some(bus.address.as_str()),
        Some(bus.address.as_str()),
        move |change| {
            let _ = sender.send(change);
        },
    );
    (watch, received)
}

fn emit(
    connection: &Connection,
    path: &str,
    interface: &str,
    member: &str,
    body: &(impl zbus::export::serde::Serialize + zbus::Type),
) {
    zbus::block_on(connection.emit_signal(None::<BusName<'_>>, path, interface, member, body))
        .unwrap();
}

fn wait_for(count: &AtomicUsize, n: usize) {
    let deadline = Instant::now() + WAIT;
    while count.load(Ordering::SeqCst) < n {
        assert!(Instant::now() < deadline, "no Inhibit call");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn logind_reports_lock_and_sleep() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let inhibits = Arc::new(AtomicUsize::new(0));
    let logind = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(
                "/org/freedesktop/login1",
                FakeManager {
                    inhibits: Arc::clone(&inhibits),
                },
            )
            .serve_at(SESSION_PATH, FakeSession)
            .name("org.freedesktop.login1")
            .build(),
    )
    .unwrap();
    let (watch, changes) = changes(&bus, false);
    watch.set_delay_sleep(true);
    wait_for(&inhibits, 1);

    let session = "org.freedesktop.login1.Session";
    emit(&logind, SESSION_PATH, session, "Lock", &());
    assert_eq!(changes.recv_timeout(WAIT).unwrap(), SessionChange::Locked);
    // The same lock through the hint changes nothing; its end unlocks.
    let hint = |locked: bool| {
        let mut changed = HashMap::new();
        changed.insert("LockedHint", Variant::from(locked));
        emit(
            &logind,
            SESSION_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(session, changed, Vec::<String>::new()),
        );
    };
    hint(true);
    hint(false);
    assert_eq!(changes.recv_timeout(WAIT).unwrap(), SessionChange::Unlocked);

    let manager = "org.freedesktop.login1.Manager";
    emit(
        &logind,
        "/org/freedesktop/login1",
        manager,
        "PrepareForSleep",
        &(true,),
    );
    assert_eq!(
        changes.recv_timeout(WAIT).unwrap(),
        SessionChange::GoingToSleep
    );
    emit(
        &logind,
        "/org/freedesktop/login1",
        manager,
        "PrepareForSleep",
        &(false,),
    );
    assert_eq!(changes.recv_timeout(WAIT).unwrap(), SessionChange::Woke);
    // The delay lock is taken again after waking.
    wait_for(&inhibits, 2);
    assert!(changes.recv_timeout(Duration::from_millis(200)).is_err());
    drop(watch);
}

#[derive(Default)]
struct FakeInhibit {
    session: Arc<Mutex<Option<String>>>,
}

#[interface(name = "org.freedesktop.portal.Inhibit")]
impl FakeInhibit {
    async fn create_monitor(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _window: &str,
        options: HashMap<String, OwnedVariant>,
    ) -> OwnedObjectPath {
        let (session, results) = session_results(&header, &options);
        *self.session.lock().unwrap() = Some(session);
        respond(connection, &header, &options, results).await;
        OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/x/y").unwrap()
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        3
    }
}

#[test]
fn the_portal_reports_the_screen_saver() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let session = Arc::new(Mutex::new(None));
    let portal = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(
                DESKTOP_PATH,
                FakeInhibit {
                    session: Arc::clone(&session),
                },
            )
            .name("org.freedesktop.portal.Desktop")
            .build(),
    )
    .unwrap();
    let (watch, changes) = changes(&bus, true);
    let handle = session.lock().unwrap().clone().expect("a monitor session");
    let state = |active: bool| {
        let mut state = HashMap::new();
        state.insert("screensaver-active", Variant::from(active));
        state.insert("session-state", Variant::from(1u32));
        emit(
            &portal,
            DESKTOP_PATH,
            "org.freedesktop.portal.Inhibit",
            "StateChanged",
            &(OwnedObjectPath::try_from(handle.as_str()).unwrap(), state),
        );
    };
    state(false);
    state(true);
    assert_eq!(changes.recv_timeout(WAIT).unwrap(), SessionChange::Locked);
    state(false);
    assert_eq!(changes.recv_timeout(WAIT).unwrap(), SessionChange::Unlocked);
    drop(watch);
}
