// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's appearance settings through the XDG Settings portal
//! (`org.freedesktop.portal.Settings`, namespace `org.freedesktop.appearance`):
//! colour scheme, accent colour, contrast and reduced motion.
//!
//! GTK 4.20 and newer follow the colour scheme themselves (4.24 also the accent
//! colour) through `GtkSettings`; this reader serves older GTK versions and the
//! parts of the window that use the values directly, such as the player deck.
//!
//! [`AppearanceMonitor::start`] reads the settings once and then calls back
//! with the new [`Appearance`] after every change, on a thread of its own (a GLib
//! front end sends it on to its main context). Without a portal the call fails;
//! the front end then keeps [`Appearance::default`] (no preference).

use std::pin::pin;
use std::thread::{self, JoinHandle};

use ashpd::desktop::settings::Settings;
use futures_util::StreamExt;
use futures_util::future::select;
use thiserror::Error;
use zbus::{Connection, OwnedVariant, Variant};

/// The settings namespace read.
pub const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";

/// The colour scheme the user prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ColorScheme {
    /// No preference: the application's default (light, for Helikodu's design).
    #[default]
    NoPreference,
    /// Dark.
    PreferDark,
    /// Light.
    PreferLight,
}

/// The contrast the user prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Contrast {
    /// No preference.
    #[default]
    Normal,
    /// Higher contrast.
    High,
}

/// An sRGB colour, each channel from 0.0 to 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rgb {
    /// Red.
    pub red: f64,
    /// Green.
    pub green: f64,
    /// Blue.
    pub blue: f64,
}

impl Rgb {
    /// The colour as CSS `#rrggbb`.
    pub fn to_hex(self) -> String {
        let channel = |value: f64| {
            // In range by construction; rounding to the nearest of 256 steps.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            byte
        };
        format!(
            "#{:02x}{:02x}{:02x}",
            channel(self.red),
            channel(self.green),
            channel(self.blue)
        )
    }
}

/// The appearance settings of the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Appearance {
    /// `color-scheme`.
    pub color_scheme: ColorScheme,
    /// `accent-color`; `None` when the desktop has none (or sends one out of
    /// range, which means the same).
    pub accent_color: Option<Rgb>,
    /// `contrast`.
    pub contrast: Contrast,
    /// `reduced-motion`: animations should be avoided.
    pub reduced_motion: bool,
}

impl Appearance {
    /// Takes the value of one key of the appearance namespace. Returns whether
    /// anything changed; unknown keys and values of the wrong type change nothing.
    pub fn apply(&mut self, key: &str, value: &Variant<'_>) -> bool {
        let value = unwrap_variant(value);
        let before = *self;
        match key {
            "color-scheme" => {
                if let Some(n) = as_u32(value) {
                    self.color_scheme = match n {
                        1 => ColorScheme::PreferDark,
                        2 => ColorScheme::PreferLight,
                        _ => ColorScheme::NoPreference,
                    };
                }
            }
            "accent-color" => {
                if let Ok((red, green, blue)) = value.downcast_ref::<(f64, f64, f64)>() {
                    let in_range = [red, green, blue].iter().all(|c| (0.0..=1.0).contains(c));
                    self.accent_color = in_range.then_some(Rgb { red, green, blue });
                }
            }
            "contrast" => {
                if let Some(n) = as_u32(value) {
                    self.contrast = if n == 1 {
                        Contrast::High
                    } else {
                        Contrast::Normal
                    };
                }
            }
            "reduced-motion" => {
                if let Some(n) = as_u32(value) {
                    self.reduced_motion = n == 1;
                }
            }
            _ => {}
        }
        *self != before
    }
}

/// Some portals wrap the value in one more variant.
fn unwrap_variant<'a>(value: &'a Variant<'a>) -> &'a Variant<'a> {
    match value {
        Variant::Variant(inner) => unwrap_variant(inner),
        other => other,
    }
}

fn as_u32(value: &Variant<'_>) -> Option<u32> {
    match value {
        Variant::U32(n) => Some(*n),
        Variant::I32(n) => u32::try_from(*n).ok(),
        _ => None,
    }
}

/// Why the settings could not be read.
#[derive(Debug, Error)]
pub enum PortalError {
    /// No portal, or it failed.
    #[error("Settings portal: {0}")]
    Portal(#[from] ashpd::Error),
    /// The bus failed.
    #[error("D-Bus: {0}")]
    Bus(#[from] zbus::Error),
    /// The thread could not be started.
    #[error("thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// Follows the appearance settings until dropped.
#[derive(Debug)]
pub struct AppearanceMonitor {
    stop: async_channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl AppearanceMonitor {
    /// Reads the settings from the portal on the session bus once.
    pub fn read() -> Result<Appearance, PortalError> {
        zbus::block_on(async {
            let connection = Connection::session().await?;
            let settings = Settings::with_connection(connection).await?;
            read_all(&settings).await
        })
    }

    /// Reads the settings from the portal on the session bus and follows them:
    /// `on_change` gets the whole appearance after every change.
    pub fn start(
        on_change: impl Fn(Appearance) + Send + 'static,
    ) -> Result<(Self, Appearance), PortalError> {
        let connection = zbus::block_on(Connection::session())?;
        Self::start_on(connection, on_change)
    }

    /// The same on the bus at `address`, such as a private bus in a test.
    pub fn start_at(
        address: &str,
        on_change: impl Fn(Appearance) + Send + 'static,
    ) -> Result<(Self, Appearance), PortalError> {
        let connection = zbus::block_on(zbus::connection::Builder::address(address).build())?;
        Self::start_on(connection, on_change)
    }

    fn start_on(
        connection: Connection,
        on_change: impl Fn(Appearance) + Send + 'static,
    ) -> Result<(Self, Appearance), PortalError> {
        let (stop, stopped) = async_channel::bounded::<()>(1);
        let (first, first_read) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("helikodu-appearance".into())
            .spawn(move || {
                zbus::block_on(async move {
                    let settings = match Settings::with_connection(connection).await {
                        Ok(settings) => settings,
                        Err(error) => {
                            let _ = first.send(Err(error.into()));
                            return;
                        }
                    };
                    // Listen before reading, so that no change falls in between.
                    let changes = match settings.receive_setting_changed().await {
                        Ok(changes) => changes,
                        Err(error) => {
                            let _ = first.send(Err(error.into()));
                            return;
                        }
                    };
                    let mut current = match read_all(&settings).await {
                        Ok(appearance) => appearance,
                        Err(error) => {
                            let _ = first.send(Err(error));
                            return;
                        }
                    };
                    let _ = first.send(Ok(current));
                    let watch = async {
                        let mut changes = pin!(changes);
                        while let Some(setting) = changes.next().await {
                            if setting.namespace() == APPEARANCE_NAMESPACE
                                && current.apply(setting.key(), setting.value())
                            {
                                on_change(current);
                            }
                        }
                    };
                    let stop = stopped.recv();
                    let _ = select(pin!(watch), pin!(stop)).await;
                });
            })?;
        let monitor = Self {
            stop,
            thread: Some(thread),
        };
        match first_read.recv() {
            Ok(Ok(appearance)) => Ok((monitor, appearance)),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(PortalError::Thread(std::io::Error::other(
                "the settings thread ended",
            ))),
        }
    }
}

impl Drop for AppearanceMonitor {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn read_all(settings: &Settings) -> Result<Appearance, PortalError> {
    let all = settings.read_all(&[APPEARANCE_NAMESPACE]).await?;
    let mut appearance = Appearance::default();
    if let Some(values) = all.get(APPEARANCE_NAMESPACE) {
        for (key, value) in values {
            let value: &OwnedVariant = value;
            appearance.apply(key, value);
        }
    }
    Ok(appearance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_map_to_the_appearance() {
        let mut appearance = Appearance::default();
        assert!(appearance.apply("color-scheme", &Variant::from(1u32)));
        assert_eq!(appearance.color_scheme, ColorScheme::PreferDark);
        assert!(!appearance.apply("color-scheme", &Variant::from(1u32)));
        // Wrapped once more, as `Read` returns it.
        let wrapped = Variant::Variant(Box::new(Variant::from(2u32)));
        assert!(appearance.apply("color-scheme", &wrapped));
        assert_eq!(appearance.color_scheme, ColorScheme::PreferLight);
        assert!(appearance.apply("color-scheme", &Variant::from(7u32)));
        assert_eq!(appearance.color_scheme, ColorScheme::NoPreference);

        assert!(appearance.apply("accent-color", &Variant::from((0.2f64, 0.4f64, 1.0f64))));
        assert_eq!(appearance.accent_color.unwrap().to_hex(), "#3366ff");
        // Out of range means no accent colour.
        assert!(appearance.apply("accent-color", &Variant::from((-1.0f64, -1.0f64, -1.0f64))));
        assert_eq!(appearance.accent_color, None);

        assert!(appearance.apply("contrast", &Variant::from(1u32)));
        assert_eq!(appearance.contrast, Contrast::High);
        assert!(appearance.apply("reduced-motion", &Variant::from(1u32)));
        assert!(appearance.reduced_motion);

        // Unknown keys and wrong types change nothing.
        assert!(!appearance.apply("cursor-size", &Variant::from(1u32)));
        assert!(!appearance.apply("contrast", &Variant::from("high")));
    }
}
