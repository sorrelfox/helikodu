// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop application: starts the service with the desktop's device
//! reservation, then runs the GTK front end over it. No domain logic lives here.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use helikodu_desktop::reservation::{DeviceReserver, ReserveError};
use helikodu_model::settings::{
    Accent, AppearanceSettings, ColorScheme, WindowStyle as StyleSetting,
};
use helikodu_service::bus::EventBus;
use helikodu_service::files::AppDirs;
use helikodu_service::settings::{SETTINGS_FILE, SettingsStore};
use helikodu_service::{
    DeviceReservation, ReservationError, ReservationGuard, ReservedCard, Service, ServiceConfig,
    SharedReservation,
};
use helikodu_ui_gtk::backend::{Backend, BackendError, sample, service};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{RunOptions, WindowStyle};

const USAGE: &str = "\
usage: helikodu [options] [file...]

  --scheme S          system, light or dark (default: from the settings)
  --accent A          system, amber, blue, green or violet (default: from the settings)
  --style S           integrated or classic (default: from the settings)
  --demo              show invented sample content and play nothing
  --help              print this message

Files are played; the output device and everything else come from
$XDG_CONFIG_HOME/helikodu/settings.toml.
";

/// The desktop's device reservation, as the engine asks for it.
#[derive(Debug, Clone, Default)]
struct DesktopReservation(DeviceReserver);

impl DeviceReservation for DesktopReservation {
    fn acquire(&self, card: &ReservedCard) -> Result<ReservationGuard, ReservationError> {
        match self.0.acquire(card.index, &card.device) {
            Ok(device) => {
                let flag = device.release_requested_flag();
                Ok(ReservationGuard::new(device, flag))
            }
            Err(ReserveError::Refused { holder }) => Err(ReservationError::Refused { holder }),
            Err(error) => Err(ReservationError::Failed(error.to_string())),
        }
    }
}

#[derive(Debug, Default)]
struct Arguments {
    scheme: Option<SchemeChoice>,
    accent: Option<AccentChoice>,
    style: Option<WindowStyle>,
    demo: bool,
    /// Passed on to GApplication: files and its own options.
    pass_on: Vec<String>,
}

fn arguments() -> Result<Option<Arguments>, String> {
    let mut parsed = Arguments::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--scheme" => {
                parsed.scheme = Some(match value()?.as_str() {
                    "system" => SchemeChoice::System,
                    "light" => SchemeChoice::Light,
                    "dark" => SchemeChoice::Dark,
                    other => return Err(format!("unknown scheme `{other}`")),
                });
            }
            "--accent" => {
                parsed.accent = Some(match value()?.as_str() {
                    "system" => AccentChoice::System,
                    "amber" => AccentChoice::Amber,
                    "blue" => AccentChoice::Blue,
                    "green" => AccentChoice::Green,
                    "violet" => AccentChoice::Violet,
                    other => return Err(format!("unknown accent `{other}`")),
                });
            }
            "--style" => {
                parsed.style = Some(match value()?.as_str() {
                    "integrated" => WindowStyle::Integrated,
                    "classic" => WindowStyle::Classic,
                    other => return Err(format!("unknown window style `{other}`")),
                });
            }
            "--demo" => parsed.demo = true,
            "--help" | "-h" => return Ok(None),
            other if other.starts_with("--gapplication") || !other.starts_with('-') => {
                parsed.pass_on.push(arg.clone());
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(Some(parsed))
}

/// Starts the service with the desktop's device reservation.
fn start() -> Result<Service, BackendError> {
    let dirs = AppDirs::from_env().map_err(|e| BackendError(e.to_string()))?;
    let mut config = ServiceConfig::new(dirs);
    config.reservation = SharedReservation::new(DesktopReservation::default());
    Service::start(config).map_err(|e| BackendError(e.to_string()))
}

/// The appearance settings, read without starting the service (a second
/// instance only hands its files to the first).
fn appearance() -> Option<AppearanceSettings> {
    let dirs = AppDirs::from_env().ok()?;
    let store = SettingsStore::open(dirs.config.join(SETTINGS_FILE), EventBus::new());
    Some(store.get().appearance.clone())
}

/// The look and behaviour the settings ask for, unless overridden.
fn options(arguments: &Arguments, appearance: Option<AppearanceSettings>) -> RunOptions {
    let scheme = arguments
        .scheme
        .unwrap_or(match appearance.as_ref().map(|a| a.color_scheme) {
            Some(ColorScheme::Light) => SchemeChoice::Light,
            Some(ColorScheme::Dark) => SchemeChoice::Dark,
            _ => SchemeChoice::System,
        });
    let accent = arguments
        .accent
        .unwrap_or(match appearance.as_ref().map(|a| a.accent) {
            Some(Accent::Amber) => AccentChoice::Amber,
            Some(Accent::Blue) => AccentChoice::Blue,
            Some(Accent::Green) => AccentChoice::Green,
            Some(Accent::Violet) => AccentChoice::Violet,
            _ => AccentChoice::System,
        });
    let style = arguments
        .style
        .or(match appearance.as_ref().map(|a| a.window_style) {
            Some(StyleSetting::Integrated) => Some(WindowStyle::Integrated),
            Some(StyleSetting::Classic) => Some(WindowStyle::Classic),
            _ => None,
        });
    RunOptions {
        style,
        scheme,
        accent,
        notifications: appearance.is_some_and(|a| a.notifications),
    }
}

fn main() -> ExitCode {
    let arguments = match arguments() {
        Ok(Some(arguments)) => arguments,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprint!("helikodu: {error}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if arguments.demo {
        let options = options(&arguments, None);
        return helikodu_ui_gtk::run(options, &arguments.pass_on, || {
            Ok(sample::backend(&sample::SampleOptions::default()))
        });
    }
    let options = options(&arguments, appearance());
    // GApplication calls this in the primary instance only.
    helikodu_ui_gtk::run(
        options,
        &arguments.pass_on,
        || -> Result<Backend, BackendError> {
            service::connect(start()?, service::ServiceOptions::default())
        },
    )
}
