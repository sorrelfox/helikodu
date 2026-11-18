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
use helikodu_service::logging::{self, LogConfig};
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
    /// A program that started this one to replace it, and must end first.
    after: Option<u32>,
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
            "--after" => {
                parsed.after = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--after needs a process number".to_owned())?,
                );
            }
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

/// Starts the log and the crash reports as the settings ask. With the log off
/// nothing is written, unless the program crashes (the report says so).
fn start_logging() {
    let Ok(dirs) = AppDirs::from_env() else {
        return;
    };
    let store = SettingsStore::open(dirs.config.join(SETTINGS_FILE), EventBus::new());
    let settings = store.get();
    logging::install(LogConfig::new(
        &dirs.state,
        settings.advanced.log_detail,
        settings.library.storage.logs_mib,
    ));
    helikodu_service::crash::install_panic_hook(&dirs.state);
}

/// Waits until the program `pid`, which started this one to replace it, has
/// ended, so that it has let go of the library database.
fn wait_for(pid: u32) {
    let alive = std::path::PathBuf::from(format!("/proc/{pid}"));
    for _ in 0..600 {
        if !alive.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Starts the program again, with the options it was started with but not
/// its files, once this one has ended.
fn restart(arguments: &Arguments) {
    let Ok(program) = std::env::current_exe() else {
        return;
    };
    let files: Vec<&String> = arguments
        .pass_on
        .iter()
        .filter(|a| !a.starts_with('-'))
        .collect();
    let mut kept = Vec::new();
    let mut skip_value = false;
    for arg in std::env::args().skip(1) {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        if arg == "--after" {
            skip_value = true;
            continue;
        }
        if !files.contains(&&arg) {
            kept.push(arg);
        }
    }
    let started = std::process::Command::new(program)
        .args(kept)
        .arg("--after")
        .arg(std::process::id().to_string())
        .spawn();
    if let Err(error) = started {
        log::error!("cannot start Helikodu again: {error}");
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
    if let Some(pid) = arguments.after {
        wait_for(pid);
    }
    if arguments.demo {
        let options = options(&arguments, None);
        return helikodu_ui_gtk::run(options, &arguments.pass_on, || {
            Ok(sample::backend(&sample::SampleOptions::default()))
        });
    }
    start_logging();
    let options = options(&arguments, appearance());
    // GApplication calls this in the primary instance only.
    let code = helikodu_ui_gtk::run(
        options,
        &arguments.pass_on,
        || -> Result<Backend, BackendError> {
            service::connect(start()?, service::ServiceOptions::default())
        },
    );
    // A backup chosen to be put back waits for the next start.
    if helikodu_ui_gtk::restart_requested() {
        restart(&arguments);
    }
    code
}
