// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The global shortcuts against a fake GlobalShortcuts portal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helikodu_desktop::global_shortcuts::{
    BoundShortcut, GlobalShortcuts, ShortcutEvent, ShortcutSpec,
};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Variant, interface};

use super::fake_portal::{DESKTOP_PATH, respond, session_results};
use super::private_bus::PrivateBus;

const WAIT: Duration = Duration::from_secs(5);

type Shortcuts = Vec<(String, HashMap<String, OwnedVariant>)>;

#[derive(Default)]
struct FakeShortcuts {
    session: Arc<Mutex<Option<String>>>,
    configured: Arc<AtomicUsize>,
}

fn request_path() -> OwnedObjectPath {
    OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/x/y").unwrap()
}

#[interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl FakeShortcuts {
    async fn create_session(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        options: HashMap<String, OwnedVariant>,
    ) -> OwnedObjectPath {
        let (session, results) = session_results(&header, &options);
        *self.session.lock().unwrap() = Some(session);
        respond(connection, &header, &options, results).await;
        request_path()
    }

    async fn bind_shortcuts(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _session: OwnedObjectPath,
        shortcuts: Shortcuts,
        _window: &str,
        options: HashMap<String, OwnedVariant>,
    ) -> OwnedObjectPath {
        // The desktop gives the first shortcut keys and leaves the rest unset.
        let bound: Vec<(String, HashMap<&str, Variant<'_>>)> = shortcuts
            .iter()
            .enumerate()
            .map(|(i, (id, info))| {
                let description = info
                    .get("description")
                    .and_then(|d| d.downcast_ref::<&str>().ok())
                    .unwrap_or_default()
                    .to_owned();
                let mut values = HashMap::new();
                values.insert("description", Variant::from(description));
                let trigger = if i == 0 { "Meta+F8" } else { "" };
                values.insert("trigger_description", Variant::from(trigger));
                (id.clone(), values)
            })
            .collect();
        let mut results = HashMap::new();
        results.insert("shortcuts", Variant::from(bound));
        respond(connection, &header, &options, results).await;
        request_path()
    }

    fn configure_shortcuts(
        &self,
        _session: OwnedObjectPath,
        _window: &str,
        _options: HashMap<String, OwnedVariant>,
    ) {
        self.configured.fetch_add(1, Ordering::SeqCst);
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

#[test]
fn binds_and_reports_presses() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let session = Arc::new(Mutex::new(None));
    let configured = Arc::new(AtomicUsize::new(0));
    let portal = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(
                DESKTOP_PATH,
                FakeShortcuts {
                    session: Arc::clone(&session),
                    configured: Arc::clone(&configured),
                },
            )
            .name("org.freedesktop.portal.Desktop")
            .build(),
    )
    .unwrap();
    let (sender, events) = mpsc::channel();
    let shortcuts = GlobalShortcuts::start_at(&bus.address, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    assert!(shortcuts.can_configure());
    let handle = session.lock().unwrap().clone().expect("a session");

    shortcuts.bind(vec![
        ShortcutSpec {
            id: "play-pause".into(),
            description: "Play or pause".into(),
        },
        ShortcutSpec {
            id: "next".into(),
            description: "Next track".into(),
        },
    ]);
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        ShortcutEvent::Bound(vec![
            BoundShortcut {
                id: "play-pause".into(),
                description: "Play or pause".into(),
                trigger: "Meta+F8".into(),
            },
            BoundShortcut {
                id: "next".into(),
                description: "Next track".into(),
                trigger: String::new(),
            },
        ])
    );

    zbus::block_on(portal.emit_signal(
        None::<BusName<'_>>,
        DESKTOP_PATH,
        "org.freedesktop.portal.GlobalShortcuts",
        "Activated",
        &(
            OwnedObjectPath::try_from(handle.as_str()).unwrap(),
            "next",
            1234u64,
            HashMap::<String, OwnedVariant>::new(),
        ),
    ))
    .unwrap();
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        ShortcutEvent::Activated("next".into())
    );

    shortcuts.configure(None);
    let deadline = std::time::Instant::now() + WAIT;
    while configured.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "ConfigureShortcuts not called"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(shortcuts);
}
