// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Light/dark and accent colour without libadwaita.
//!
//! Two sources are watched: GtkSettings (`gtk-interface-color-scheme` since
//! GTK 4.20, `gtk-accent-color` since 4.24, both fed from the Settings portal
//! on Wayland), and the Settings portal itself over D-Bus, which also covers
//! older GTK versions and X11. The stylesheet is switched live by swapping the
//! `hk-dark` / `hk-light` class on the window and reloading a small second
//! provider that holds the accent variants.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::stats::SharedRecorder;

pub const BASE_CSS: &str = include_str!("../style/helikodu.css");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Override {
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb(pub f64, pub f64, pub f64);

impl Rgb {
    pub const AMBER: Rgb = Rgb(
        0xF0 as f64 / 255.0,
        0xB2 as f64 / 255.0,
        0x3C as f64 / 255.0,
    );

    fn hex(self) -> String {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", c(self.0), c(self.1), c(self.2))
    }

    fn lum(self) -> f64 {
        let l = |v: f64| {
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * l(self.0) + 0.7152 * l(self.1) + 0.0722 * l(self.2)
    }

    fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.lum(), other.lum());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn mix(self, o: Rgb, t: f64) -> Rgb {
        Rgb(
            self.0 + (o.0 - self.0) * t,
            self.1 + (o.1 - self.1) * t,
            self.2 + (o.2 - self.2) * t,
        )
    }

    /// Steps toward white (dark surface) or black (light surface) until the
    /// contrast reaches `min`. Simplified to sRGB steps for the spike; the real
    /// crate interpolates in OKLab.
    fn ensure(self, surface: Rgb, min: f64) -> Rgb {
        let target = if surface.lum() < 0.5 {
            Rgb(1.0, 1.0, 1.0)
        } else {
            Rgb(0.0, 0.0, 0.0)
        };
        let mut c = self;
        for _ in 0..20 {
            if c.contrast(surface) >= min {
                break;
            }
            c = c.mix(target, 0.05);
        }
        c
    }
}

/// CSS for the second provider: accent variants for both schemes.
pub fn accent_css(a: Rgb) -> String {
    let dark_pane = Rgb(
        0x1B as f64 / 255.0,
        0x1C as f64 / 255.0,
        0x20 as f64 / 255.0,
    );
    let dark_hover = Rgb(
        0x26 as f64 / 255.0,
        0x28 as f64 / 255.0,
        0x2E as f64 / 255.0,
    );
    let light_pane = Rgb(
        0xF8 as f64 / 255.0,
        0xF8 as f64 / 255.0,
        0xF6 as f64 / 255.0,
    );
    let light_hover = Rgb(
        0xE2 as f64 / 255.0,
        0xE2 as f64 / 255.0,
        0xDD as f64 / 255.0,
    );
    let white = Rgb(1.0, 1.0, 1.0);
    let black = Rgb(0.0, 0.0, 0.0);
    let d_fill = a.ensure(dark_pane, 3.0);
    let d_text = a.mix(white, 0.18).ensure(dark_hover, 4.5);
    let l_fill = a.mix(black, 0.20).ensure(light_pane, 3.0);
    let l_text = a.mix(black, 0.45).ensure(light_hover, 4.5);
    let on = |fill: Rgb| {
        let ink = Rgb(
            0x15 as f64 / 255.0,
            0x12 as f64 / 255.0,
            0x0B as f64 / 255.0,
        );
        if ink.contrast(fill) >= 4.5 {
            "#15120B"
        } else {
            "#FFFFFF"
        }
    };
    let rgba = |alpha: f64| {
        format!(
            "rgba({},{},{},{alpha})",
            (a.0 * 255.0).round(),
            (a.1 * 255.0).round(),
            (a.2 * 255.0).round()
        )
    };
    format!(
        ".hk-dark {{ --hk-accent: {acc}; --hk-accent-fill: {}; --hk-accent-text: {}; --hk-sel: {}; --hk-on-accent: {}; }}\n\
         .hk-light {{ --hk-accent: {acc}; --hk-accent-fill: {}; --hk-accent-text: {}; --hk-sel: {}; --hk-on-accent: {}; }}\n\
         .hk-glass {{ --hk-accent: {acc}; --hk-sel: {}; }}\n",
        d_fill.hex(),
        d_text.hex(),
        rgba(0.16),
        on(d_fill),
        l_fill.hex(),
        l_text.hex(),
        rgba(0.24),
        on(l_fill),
        rgba(0.16),
        acc = a.hex(),
    )
}

/// What the system reported, from each source.
#[derive(Debug, Default, Clone)]
pub struct SystemState {
    pub gtk_scheme: Option<String>,
    pub gtk_accent: Option<Rgb>,
    pub portal_scheme: Option<u32>,
    pub portal_accent: Option<Rgb>,
    pub portal_available: bool,
}

#[derive(Debug)]
pub struct Theme {
    pub window: glib::WeakRef<gtk::Window>,
    pub accent_provider: gtk::CssProvider,
    pub over: Cell<Override>,
    pub current: Cell<Option<Scheme>>,
    pub accent: Cell<Rgb>,
    pub state: RefCell<SystemState>,
    pub rec: SharedRecorder,
    pub log: RefCell<Vec<String>>,
    pub started: Instant,
    pub switches: Cell<u32>,
}

pub fn install_base(display: &gdk::Display, with_css: bool) -> gtk::CssProvider {
    let base = gtk::CssProvider::new();
    if with_css {
        base.load_from_string(BASE_CSS);
    }
    gtk::style_context_add_provider_for_display(
        display,
        &base,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let accent = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        display,
        &accent,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
    accent
}

impl Theme {
    pub fn new(
        window: &gtk::Window,
        accent_provider: gtk::CssProvider,
        over: Override,
        rec: SharedRecorder,
    ) -> Rc<Self> {
        let weak = glib::WeakRef::new();
        weak.set(Some(window));
        Rc::new(Self {
            window: weak,
            accent_provider,
            over: Cell::new(over),
            current: Cell::new(None),
            accent: Cell::new(Rgb::AMBER),
            state: RefCell::new(SystemState::default()),
            rec,
            log: RefCell::new(Vec::new()),
            started: Instant::now(),
            switches: Cell::new(0),
        })
    }

    fn logln(&self, s: String) {
        let line = format!("[{:7.3} s] {s}", self.started.elapsed().as_secs_f64());
        eprintln!("theme: {line}");
        self.log.borrow_mut().push(line);
    }

    /// Resolves the scheme from the override and the sources, then applies it.
    pub fn resolve(&self, reason: &str) {
        let st = self.state.borrow().clone();
        let scheme = match self.over.get() {
            Override::Dark => Scheme::Dark,
            Override::Light => Scheme::Light,
            Override::System => match (st.gtk_scheme.as_deref(), st.portal_scheme) {
                (Some("dark"), _) => Scheme::Dark,
                (Some("light"), _) => Scheme::Light,
                (_, Some(1)) => Scheme::Dark,
                (_, Some(2)) => Scheme::Light,
                // No preference reported: the player's own default.
                _ => Scheme::Dark,
            },
        };
        let accent = st.gtk_accent.or(st.portal_accent).unwrap_or(Rgb::AMBER);
        self.apply(scheme, accent, self.over.get() != Override::System, reason);
    }

    /// Applies a scheme. With `force_gtk`, GTK's own theme is also forced to
    /// the scheme (a manual override); otherwise GTK is left to follow the
    /// system by itself, which it does since 4.20 on Wayland.
    pub fn apply(&self, scheme: Scheme, accent: Rgb, force_gtk: bool, reason: &str) {
        let Some(win) = self.window.upgrade() else {
            return;
        };
        if self.current.get() == Some(scheme) && self.accent.get() == accent {
            return;
        }
        if self.current.get().is_some() {
            self.rec.borrow_mut().switch_started = Some(Instant::now());
        }
        if self.accent.get() != accent || self.current.get().is_none() {
            self.accent_provider.load_from_string(&accent_css(accent));
        }
        let (add, remove) = match scheme {
            Scheme::Dark => ("hk-dark", "hk-light"),
            Scheme::Light => ("hk-light", "hk-dark"),
        };
        // Record first, so the notify that GTK may send back is a no-op.
        self.current.set(Some(scheme));
        self.accent.set(accent);
        win.remove_css_class(remove);
        win.add_css_class(add);
        if (force_gtk || gtk::minor_version() < 20)
            && let Some(s) = gtk::Settings::default()
        {
            if gtk::minor_version() >= 20 {
                s.set_gtk_interface_color_scheme(match scheme {
                    Scheme::Dark => gtk::InterfaceColorScheme::Dark,
                    Scheme::Light => gtk::InterfaceColorScheme::Light,
                });
            } else {
                // Before 4.20 this is the only way to make stock widgets dark.
                #[allow(deprecated)]
                s.set_gtk_application_prefer_dark_theme(scheme == Scheme::Dark);
            }
        }
        self.switches.set(self.switches.get() + 1);
        self.logln(format!(
            "applied {scheme:?}, accent {} ({reason})",
            accent.hex()
        ));
    }

    /// Starts watching GtkSettings and the Settings portal.
    pub fn watch(self: &Rc<Self>) {
        if let Some(settings) = gtk::Settings::default() {
            self.read_gtk(&settings, "startup");
            let me = Rc::downgrade(self);
            settings.connect_notify_local(Some("gtk-interface-color-scheme"), move |s, _| {
                if let Some(me) = me.upgrade() {
                    me.read_gtk(s, "GtkSettings gtk-interface-color-scheme changed");
                }
            });
            let me = Rc::downgrade(self);
            settings.connect_notify_local(Some("gtk-accent-color"), move |s, _| {
                if let Some(me) = me.upgrade() {
                    me.read_gtk(s, "GtkSettings gtk-accent-color changed");
                }
            });
        }
        self.watch_portal();
    }

    fn read_gtk(&self, s: &gtk::Settings, reason: &str) {
        let scheme = s.gtk_interface_color_scheme();
        let name = match scheme {
            gtk::InterfaceColorScheme::Dark => "dark",
            gtk::InterfaceColorScheme::Light => "light",
            gtk::InterfaceColorScheme::Default => "default",
            _ => "unsupported",
        };
        let accent = if s.find_property("gtk-accent-color").is_some() {
            s.property::<Option<gdk::RGBA>>("gtk-accent-color")
                .map(|c| {
                    Rgb(
                        f64::from(c.red()),
                        f64::from(c.green()),
                        f64::from(c.blue()),
                    )
                })
        } else {
            None
        };
        {
            let mut st = self.state.borrow_mut();
            st.gtk_scheme = Some(name.to_owned());
            st.gtk_accent = accent;
        }
        self.logln(format!(
            "GtkSettings: color-scheme={name}, accent={}",
            accent.map_or("none".into(), Rgb::hex),
        ));
        self.resolve(reason);
    }

    fn watch_portal(self: &Rc<Self>) {
        let proxy = gio::DBusProxy::for_bus_sync(
            gio::BusType::Session,
            gio::DBusProxyFlags::DO_NOT_AUTO_START_AT_CONSTRUCTION,
            None,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
            gio::Cancellable::NONE,
        );
        let Ok(proxy) = proxy else {
            self.logln("Settings portal: no session bus".into());
            return;
        };
        for key in ["color-scheme", "accent-color"] {
            let r = proxy.call_sync(
                "ReadOne",
                Some(&("org.freedesktop.appearance", key).to_variant()),
                gio::DBusCallFlags::NONE,
                2000,
                gio::Cancellable::NONE,
            );
            match r {
                Ok(v) => {
                    self.state.borrow_mut().portal_available = true;
                    self.portal_value(key, &v.child_value(0), "portal startup");
                }
                Err(e) => self.logln(format!(
                    "Settings portal ReadOne({key}) failed: {}",
                    e.message()
                )),
            }
        }
        let me = Rc::downgrade(self);
        proxy.connect_local("g-signal", false, move |args| {
            let signal: String = args[2].get().ok()?;
            let params: glib::Variant = args[3].get().ok()?;
            if signal == "SettingChanged" {
                let ns: String = params.child_value(0).get()?;
                let key: String = params.child_value(1).get()?;
                if ns == "org.freedesktop.appearance"
                    && let Some(me) = me.upgrade()
                {
                    me.portal_value(&key, &params.child_value(2), "portal SettingChanged");
                }
            }
            None
        });
        // Keep the proxy alive for the lifetime of the program.
        std::mem::forget(proxy);
    }

    fn portal_value(&self, key: &str, v: &glib::Variant, reason: &str) {
        // ReadOne returns (v); peel any nested variants.
        let mut v = v.clone();
        while v.is_type(glib::VariantTy::VARIANT) {
            let Some(inner) = v.as_variant() else { break };
            v = inner;
        }
        match key {
            "color-scheme" => {
                let s = v.get::<u32>();
                self.state.borrow_mut().portal_scheme = s;
                self.logln(format!(
                    "portal color-scheme = {s:?} (0 none, 1 dark, 2 light)"
                ));
            }
            "accent-color" => {
                let a = v
                    .get::<(f64, f64, f64)>()
                    .filter(|(r, g, b)| {
                        (0.0..=1.0).contains(r)
                            && (0.0..=1.0).contains(g)
                            && (0.0..=1.0).contains(b)
                    })
                    .map(|(r, g, b)| Rgb(r, g, b));
                self.state.borrow_mut().portal_accent = a;
                self.logln(format!(
                    "portal accent-color = {}",
                    a.map_or("none".into(), Rgb::hex)
                ));
            }
            _ => return,
        }
        self.resolve(reason);
    }
}

/// A stand-in for the Settings portal on a private session bus, so the live
/// switch can be scripted without touching the user's desktop settings. It
/// flips color-scheme between dark and light and the accent between amber and
/// blue every `flip_ms` milliseconds, and emits SettingChanged like the real one.
pub fn run_fake_portal(flip_ms: u64, button_layout: String) {
    const XML: &str = r#"<node>
      <interface name="org.freedesktop.portal.Settings">
        <method name="ReadAll"><arg type="as" name="namespaces" direction="in"/><arg type="a{sa{sv}}" name="value" direction="out"/></method>
        <method name="Read"><arg type="s" name="namespace" direction="in"/><arg type="s" name="key" direction="in"/><arg type="v" name="value" direction="out"/></method>
        <method name="ReadOne"><arg type="s" name="namespace" direction="in"/><arg type="s" name="key" direction="in"/><arg type="v" name="value" direction="out"/></method>
        <signal name="SettingChanged"><arg type="s" name="namespace"/><arg type="s" name="key"/><arg type="v" name="value"/></signal>
        <property name="version" type="u" access="read"/>
      </interface>
    </node>"#;
    let main_loop = glib::MainLoop::new(None, false);
    let state = Rc::new(Cell::new(1u32));
    let st = state.clone();
    let _owner = gio::bus_own_name(
        gio::BusType::Session,
        "org.freedesktop.portal.Desktop",
        gio::BusNameOwnerFlags::NONE,
        move |conn, _| {
            let Ok(info) = gio::DBusNodeInfo::for_xml(XML) else {
                return;
            };
            let Some(iface) = info.lookup_interface("org.freedesktop.portal.Settings") else {
                return;
            };
            let st2 = st.clone();
            let layout = button_layout.clone();
            let values = move |scheme: u32| -> HashMap<String, glib::Variant> {
                let accent = if scheme == 1 {
                    (0.941, 0.698, 0.235)
                } else {
                    (0.208, 0.518, 0.894)
                };
                HashMap::from([
                    ("color-scheme".to_owned(), scheme.to_variant()),
                    ("accent-color".to_owned(), accent.to_variant()),
                    ("contrast".to_owned(), 0u32.to_variant()),
                ])
            };
            let reg = conn
                .register_object("/org/freedesktop/portal/desktop", &iface)
                .method_call(move |_, _, _, _, method, params, inv| {
                    let vals = values(st2.get());
                    match method {
                        "ReadAll" => {
                            let mut all: HashMap<String, HashMap<String, glib::Variant>> =
                                HashMap::new();
                            all.insert("org.freedesktop.appearance".into(), vals);
                            all.insert(
                                "org.gnome.desktop.wm.preferences".into(),
                                HashMap::from([("button-layout".to_owned(), layout.to_variant())]),
                            );
                            inv.return_value(Some(&(all,).to_variant()));
                        }
                        "Read" | "ReadOne" => {
                            let ns: String = params.child_value(0).get().unwrap_or_default();
                            let key: String = params.child_value(1).get().unwrap_or_default();
                            let v = if ns == "org.gnome.desktop.wm.preferences"
                                && key == "button-layout"
                            {
                                Some(layout.to_variant())
                            } else {
                                vals.get(&key)
                                    .filter(|_| ns == "org.freedesktop.appearance")
                                    .cloned()
                            };
                            match v.as_ref() {
                                Some(v) => {
                                    // ReadOne returns (v); the deprecated Read wraps once more.
                                    let out = if method == "Read" {
                                        glib::Variant::from_variant(v)
                                    } else {
                                        v.clone()
                                    };
                                    inv.return_value(Some(&(out,).to_variant()));
                                }
                                None => inv.return_dbus_error(
                                    "org.freedesktop.portal.Error.NotFound",
                                    "not found",
                                ),
                            }
                        }
                        _ => inv.return_dbus_error(
                            "org.freedesktop.DBus.Error.UnknownMethod",
                            "unknown",
                        ),
                    }
                })
                .property(|_, _, _, _, _| 2u32.to_variant())
                .build();
            if reg.is_err() {
                eprintln!("fake portal: register failed");
            }
            let st3 = st.clone();
            let conn = conn.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(flip_ms), move || {
                let next = if st3.get() == 1 { 2 } else { 1 };
                st3.set(next);
                let accent = if next == 1 {
                    (0.941, 0.698, 0.235)
                } else {
                    (0.208, 0.518, 0.894)
                };
                for (key, v) in [
                    ("color-scheme", next.to_variant()),
                    ("accent-color", accent.to_variant()),
                ] {
                    let _ = conn.emit_signal(
                        None,
                        "/org/freedesktop/portal/desktop",
                        "org.freedesktop.portal.Settings",
                        "SettingChanged",
                        Some(&("org.freedesktop.appearance", key, v).to_variant()),
                    );
                }
                eprintln!("fake portal: color-scheme -> {next}");
                glib::ControlFlow::Continue
            });
        },
        |_, _| eprintln!("fake portal: name acquired"),
        |_, _| eprintln!("fake portal: name lost"),
    );
    let _ = &state;
    main_loop.run();
}
