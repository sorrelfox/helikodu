// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tray icon against a fake StatusNotifierWatcher, and its menu read and
//! clicked as a tray host does.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helikodu_desktop::tray::{
    ITEM_INTERFACE, ITEM_PATH, MENU_INTERFACE, MENU_PATH, Tray, TrayAction, TrayConfig,
    TrayContent, TrayHooks, TrayMenuItem, host_present_at,
};
use zbus::{Connection, OwnedVariant, interface};

use super::private_bus::{PrivateBus, Signals};

const WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct FakeWatcher {
    items: Arc<Mutex<Vec<String>>>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl FakeWatcher {
    fn register_status_notifier_item(&self, service: String) {
        self.items.lock().unwrap().push(service);
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }
}

/// What the hooks saw.
#[derive(Debug, PartialEq)]
enum Seen {
    Activate,
    Scroll(i32),
    Action(TrayAction),
    Host(bool),
}

fn start(bus: &PrivateBus) -> (Tray, Receiver<Seen>) {
    let (sender, seen) = mpsc::channel();
    let (a, s, h) = (sender.clone(), sender.clone(), sender.clone());
    let hooks = TrayHooks::new(move |action| {
        let _ = sender.send(Seen::Action(action));
    })
    .with_activate(move || {
        let _ = a.send(Seen::Activate);
    })
    .with_scroll(move |delta| {
        let _ = s.send(Seen::Scroll(delta));
    })
    .with_host(move |shown| {
        let _ = h.send(Seen::Host(shown));
    });
    let tray = Tray::start_at(&bus.address, TrayConfig::default(), hooks).unwrap();
    (tray, seen)
}

fn call<B>(
    client: &Connection,
    name: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> zbus::Message
where
    B: zbus::export::serde::Serialize + zbus::Type,
{
    zbus::block_on(client.call_method(Some(name), path, Some(interface), method, body))
        .expect("the call succeeds")
}

/// A tooltip as the item sends it.
type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

type Node = (i32, HashMap<String, OwnedVariant>, Vec<OwnedVariant>);

fn labels(layout: &Node) -> Vec<String> {
    layout
        .2
        .iter()
        .map(|child| {
            let (_, properties, _): Node = child.try_clone().unwrap().try_into().unwrap();
            match properties.get("label") {
                Some(label) => label.downcast_ref::<&str>().unwrap().to_owned(),
                None => "-".to_owned(),
            }
        })
        .collect()
}

fn menu() -> Vec<TrayMenuItem> {
    vec![
        TrayMenuItem::Text("Shoreline, 6 a.m.".into()),
        TrayMenuItem::Text("Mari Kask · Tidewater Rooms".into()),
        TrayMenuItem::Separator,
        TrayMenuItem::action(TrayAction::PlayPause, "Pause"),
        TrayMenuItem::action(TrayAction::Next, "Next"),
        TrayMenuItem::check(TrayAction::StopAfter, "Stop after this track", false),
        TrayMenuItem::Separator,
        TrayMenuItem::action(TrayAction::Quit, "Quit"),
    ]
}

#[test]
fn a_host_is_found_without_registering() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    assert!(!host_present_at(&bus.address));
    let items = Arc::new(Mutex::new(Vec::new()));
    let _watcher = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(
                "/StatusNotifierWatcher",
                FakeWatcher {
                    items: Arc::clone(&items),
                },
            )
            .name("org.kde.StatusNotifierWatcher")
            .build(),
    )
    .unwrap();
    assert!(host_present_at(&bus.address));
    assert!(items.lock().unwrap().is_empty());
}

#[test]
fn registers_with_the_unique_name_where_the_item_name_is_refused() {
    let dir = std::env::temp_dir().join(format!("helikodu-tray-bus-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let Some(bus) = PrivateBus::start_owning_only("org.kde.StatusNotifierWatcher", &dir) else {
        return;
    };
    let items = Arc::new(Mutex::new(Vec::new()));
    let _watcher = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(
                "/StatusNotifierWatcher",
                FakeWatcher {
                    items: Arc::clone(&items),
                },
            )
            .name("org.kde.StatusNotifierWatcher")
            .build(),
    )
    .unwrap();
    let (tray, seen) = start(&bus);
    assert_eq!(seen.recv_timeout(WAIT).unwrap(), Seen::Host(true));
    assert!(tray.bus_name().starts_with(':'), "{}", tray.bus_name());
    assert_eq!(*items.lock().unwrap(), [tray.bus_name().to_owned()]);

    // A host reads the item through that name.
    let client = bus.connect();
    let reply = call(
        &client,
        tray.bus_name(),
        ITEM_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(ITEM_INTERFACE, "Status"),
    );
    let status: OwnedVariant = reply.body().deserialize().unwrap();
    assert!(status.downcast_ref::<&str>().is_ok());
    drop(tray);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn registers_when_a_watcher_appears_and_serves_the_menu() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let (tray, seen) = start(&bus);
    // No watcher yet: no host.
    assert_eq!(seen.recv_timeout(WAIT).unwrap(), Seen::Host(false));

    let items = Arc::new(Mutex::new(Vec::new()));
    let _watcher = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(
                "/StatusNotifierWatcher",
                FakeWatcher {
                    items: Arc::clone(&items),
                },
            )
            .name("org.kde.StatusNotifierWatcher")
            .build(),
    )
    .unwrap();
    assert_eq!(seen.recv_timeout(WAIT).unwrap(), Seen::Host(true));
    assert_eq!(*items.lock().unwrap(), [tray.bus_name().to_owned()]);

    let client = bus.connect();
    let name = tray.bus_name().to_owned();
    let signals = Signals::record(&client, MENU_PATH);
    let item_signals = Signals::record(&client, ITEM_PATH);
    tray.set_content(TrayContent {
        tooltip: "Shoreline, 6 a.m. · Mari Kask".into(),
        menu: menu(),
    });
    let updated = signals.next("LayoutUpdated", WAIT).expect("LayoutUpdated");
    let (revision, parent): (u32, i32) = updated.body().deserialize().unwrap();
    assert_eq!(parent, 0);
    assert!(item_signals.next("NewToolTip", WAIT).is_some());

    // The layout as a host reads it.
    let reply = call(
        &client,
        &name,
        MENU_PATH,
        MENU_INTERFACE,
        "GetLayout",
        &(0i32, -1i32, Vec::<String>::new()),
    );
    let (now, layout): (u32, Node) = reply.body().deserialize().unwrap();
    assert_eq!(now, revision);
    assert_eq!(
        labels(&layout),
        [
            "Shoreline, 6 a.m.",
            "Mari Kask · Tidewater Rooms",
            "-",
            "Pause",
            "Next",
            "Stop after this track",
            "-",
            "Quit"
        ]
    );

    // The same content again announces nothing.
    tray.set_content(TrayContent {
        tooltip: "Shoreline, 6 a.m. · Mari Kask".into(),
        menu: menu(),
    });
    assert!(
        signals
            .next("LayoutUpdated", Duration::from_millis(200))
            .is_none()
    );

    // Clicks: the info line does nothing, the items reach the hook. The ids
    // are the layout's: each revision numbers its items afresh.
    let ids: Vec<i32> = layout
        .2
        .iter()
        .map(|child| {
            let (id, _, _): Node = child.try_clone().unwrap().try_into().unwrap();
            id
        })
        .collect();
    for id in [ids[0], ids[3], ids[5]] {
        call(
            &client,
            &name,
            MENU_PATH,
            MENU_INTERFACE,
            "Event",
            &(id, "clicked", zbus::Variant::from(0i32), 0u32),
        );
    }
    assert_eq!(
        seen.recv_timeout(WAIT).unwrap(),
        Seen::Action(TrayAction::PlayPause)
    );
    assert_eq!(
        seen.recv_timeout(WAIT).unwrap(),
        Seen::Action(TrayAction::StopAfter)
    );
    // An unknown item is reported back by EventGroup.
    let reply = call(
        &client,
        &name,
        MENU_PATH,
        MENU_INTERFACE,
        "EventGroup",
        &(vec![(99i32, "clicked", zbus::Variant::from(0i32), 0u32)],),
    );
    let missing: Vec<i32> = reply.body().deserialize().unwrap();
    assert_eq!(missing, [99]);

    // A changed menu numbers its items afresh: no id of the old layout comes
    // back, so a host that keeps items by id cannot mix them up, and a click
    // on an old id is an unknown item.
    let mut changed = menu();
    changed.remove(0);
    tray.set_content(TrayContent {
        tooltip: "Shoreline, 6 a.m. · Mari Kask".into(),
        menu: changed,
    });
    assert!(signals.next("LayoutUpdated", WAIT).is_some());
    let reply = call(
        &client,
        &name,
        MENU_PATH,
        MENU_INTERFACE,
        "GetLayout",
        &(0i32, -1i32, Vec::<String>::new()),
    );
    let (_, relaid): (u32, Node) = reply.body().deserialize().unwrap();
    assert_eq!(labels(&relaid).len(), ids.len() - 1);
    for child in &relaid.2 {
        let (id, _, _): Node = child.try_clone().unwrap().try_into().unwrap();
        assert!(!ids.contains(&id), "id {id} was used by the old layout");
    }
    let reply = call(
        &client,
        &name,
        MENU_PATH,
        MENU_INTERFACE,
        "EventGroup",
        &(vec![(ids[3], "clicked", zbus::Variant::from(0i32), 0u32)],),
    );
    let missing: Vec<i32> = reply.body().deserialize().unwrap();
    assert_eq!(missing, [ids[3]]);

    // The icon itself: click, scroll and the tooltip.
    call(
        &client,
        &name,
        ITEM_PATH,
        ITEM_INTERFACE,
        "Activate",
        &(0i32, 0i32),
    );
    assert_eq!(seen.recv_timeout(WAIT).unwrap(), Seen::Activate);
    call(
        &client,
        &name,
        ITEM_PATH,
        ITEM_INTERFACE,
        "Scroll",
        &(120i32, "horizontal"),
    );
    call(
        &client,
        &name,
        ITEM_PATH,
        ITEM_INTERFACE,
        "Scroll",
        &(-120i32, "vertical"),
    );
    assert_eq!(seen.recv_timeout(WAIT).unwrap(), Seen::Scroll(-120));
    let reply = call(
        &client,
        &name,
        ITEM_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(ITEM_INTERFACE, "ToolTip"),
    );
    let tooltip: OwnedVariant = reply.body().deserialize().unwrap();
    let (_, _, title, _): ToolTip = tooltip.try_into().unwrap();
    assert_eq!(title, "Shoreline, 6 a.m. · Mari Kask");
    let reply = call(
        &client,
        &name,
        ITEM_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(ITEM_INTERFACE, "IconName"),
    );
    let icon: OwnedVariant = reply.body().deserialize().unwrap();
    assert_eq!(
        icon.downcast_ref::<&str>().unwrap(),
        "io.github.sorrelfox.Helikodu-symbolic"
    );

    // The watcher goes away: no host.
    drop(_watcher);
    assert_eq!(seen.recv_timeout(WAIT).unwrap(), Seen::Host(false));
    drop(tray);
}
