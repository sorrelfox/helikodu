// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The appearance reader against a fake Settings portal.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use helikodu_desktop::portals::{Appearance, AppearanceMonitor, ColorScheme, Contrast};
use zbus::object_server::SignalEmitter;
use zbus::{OwnedVariant, Variant, interface};

use super::private_bus::PrivateBus;

const PATH: &str = "/org/freedesktop/portal/desktop";
const NAMESPACE: &str = "org.freedesktop.appearance";

struct FakeSettings;

#[interface(name = "org.freedesktop.portal.Settings")]
impl FakeSettings {
    fn read_all(&self, namespaces: Vec<String>) -> HashMap<String, HashMap<String, OwnedVariant>> {
        assert_eq!(namespaces, [NAMESPACE]);
        let mut values = HashMap::new();
        values.insert("color-scheme".to_owned(), OwnedVariant::from(1u32));
        values.insert(
            "accent-color".to_owned(),
            OwnedVariant::try_from(Variant::from((0.2f64, 0.4f64, 1.0f64))).unwrap(),
        );
        values.insert("contrast".to_owned(), OwnedVariant::from(0u32));
        let mut all = HashMap::new();
        all.insert(NAMESPACE.to_owned(), values);
        all
    }

    #[zbus(signal)]
    async fn setting_changed(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: Variant<'_>,
    ) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

#[test]
fn reads_the_settings_and_follows_changes() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let portal = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .serve_at(PATH, FakeSettings)
            .name("org.freedesktop.portal.Desktop")
            .build(),
    )
    .unwrap();

    let (sender, changes) = mpsc::channel();
    let (monitor, appearance) = AppearanceMonitor::start_at(&bus.address, move |appearance| {
        let _ = sender.send(appearance);
    })
    .unwrap();
    assert_eq!(appearance.color_scheme, ColorScheme::PreferDark);
    assert_eq!(appearance.accent_color.unwrap().to_hex(), "#3366ff");
    assert_eq!(appearance.contrast, Contrast::Normal);
    assert!(!appearance.reduced_motion);

    let emit = |namespace: &str, key: &str, value: Variant<'_>| {
        zbus::block_on(async {
            let emitter = SignalEmitter::new(&portal, PATH).unwrap();
            FakeSettings::setting_changed(&emitter, namespace, key, value)
                .await
                .unwrap();
        });
    };
    // Another namespace and an unchanged value are not reported.
    emit(
        "org.gnome.desktop.interface",
        "color-scheme",
        Variant::from(2u32),
    );
    emit(NAMESPACE, "color-scheme", Variant::from(1u32));
    emit(NAMESPACE, "contrast", Variant::from(1u32));
    let changed: Appearance = changes.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(changed.contrast, Contrast::High);
    assert_eq!(changed.color_scheme, ColorScheme::PreferDark);
    emit(NAMESPACE, "reduced-motion", Variant::from(1u32));
    let changed = changes.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(changed.reduced_motion);
    assert!(changes.recv_timeout(Duration::from_millis(200)).is_err());

    // Dropping the monitor ends its thread.
    drop(monitor);
}
