// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dark or light, the accent, high contrast and reduced motion: where they come
//! from and how they reach the windows.
//!
//! Two sources are watched: `GtkSettings` (`gtk-interface-color-scheme` since
//! GTK 4.20, `gtk-enable-animations`) and the Settings portal
//! (`color-scheme`, `accent-color`, `contrast`, `reduced-motion`), which also
//! covers GTK 4.18 and X11. Changes are applied once per main-loop turn, from
//! an idle callback, because the portal sends scheme and accent separately.
//! `gtk-application-prefer-dark-theme` is never set: it is deprecated and
//! feeds back into the scheme notification. Only a manual Light or Dark choice
//! writes `gtk-interface-color-scheme`, so stock dialogs and menus agree.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use helikodu_desktop::portals::{
    Appearance, AppearanceMonitor, ColorScheme as PortalScheme, Contrast,
};

use super::color::{AccentSet, Rgba, palette};
use super::css::Providers;
use super::tokens::{self, Palette, accent};

/// The user's choice of colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchemeChoice {
    /// Follow the desktop.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// The user's choice of accent colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccentChoice {
    /// The desktop's accent, or amber when it reports none.
    #[default]
    System,
    /// Amber.
    Amber,
    /// Blue.
    Blue,
    /// Green.
    Green,
    /// Violet.
    Violet,
}

impl AccentChoice {
    fn preset(self) -> Option<Rgba> {
        let name = match self {
            Self::System => return None,
            Self::Amber => "Amber",
            Self::Blue => "Blue",
            Self::Green => "Green",
            Self::Violet => "Violet",
        };
        accent::EXAMPLES
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.accent)
    }
}

/// What the windows look like now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Look {
    pub(crate) dark: bool,
    pub(crate) high_contrast: bool,
    pub(crate) reduced_motion: bool,
    pub(crate) palette: Palette,
    pub(crate) accent: AccentSet,
}

impl Look {
    fn new(dark: bool, high_contrast: bool, reduced_motion: bool, raw: Rgba) -> Self {
        Self {
            dark,
            high_contrast,
            reduced_motion,
            palette: palette(dark, high_contrast),
            accent: AccentSet::new(raw, dark, high_contrast),
        }
    }
}

impl Default for Look {
    fn default() -> Self {
        Self::new(true, false, false, accent::DEFAULT)
    }
}

/// What the desktop reported, from each source.
#[derive(Debug, Default, Clone, Copy)]
struct Reported {
    gtk_scheme: Option<bool>,
    gtk_accent: Option<Rgba>,
    gtk_animations: Option<bool>,
    portal: bool,
    portal_scheme: Option<bool>,
    portal_accent: Option<Rgba>,
    portal_contrast: bool,
    portal_reduced_motion: bool,
}

type Listener = Box<dyn Fn(&Look)>;

#[derive(Default)]
struct Theme {
    windows: RefCell<Vec<glib::WeakRef<gtk::Window>>>,
    providers: RefCell<Option<Providers>>,
    scheme: Cell<SchemeChoice>,
    accent: Cell<AccentChoice>,
    reported: Cell<Reported>,
    look: Cell<Option<Look>>,
    pending: Cell<bool>,
    listeners: RefCell<Vec<Listener>>,
}

thread_local! {
    static THEME: Rc<Theme> = Rc::new(Theme::default());
}

fn theme() -> Rc<Theme> {
    THEME.with(Rc::clone)
}

/// Loads the stylesheet and starts following the desktop. Call once, after GTK
/// is initialised.
pub(crate) fn install(display: &gdk::Display) {
    let theme = theme();
    if theme.providers.borrow().is_some() {
        return;
    }
    *theme.providers.borrow_mut() = Some(Providers::install(display));
    gtk::IconTheme::for_display(display)
        .add_resource_path(&format!("{}/icons", crate::RESOURCE_BASE));
    if let Some(settings) = gtk::Settings::default() {
        read_gtk(&settings);
        for name in [
            "gtk-interface-color-scheme",
            "gtk-accent-color",
            "gtk-enable-animations",
        ] {
            if settings.find_property(name).is_some() {
                settings.connect_notify_local(Some(name), |s, _| read_gtk(s));
            }
        }
    }
    watch_portal();
    apply_now();
}

/// Sets the user's choices and restyles.
pub fn set_choice(scheme: SchemeChoice, accent: AccentChoice) {
    let theme = theme();
    let previous = theme.scheme.replace(scheme);
    theme.accent.set(accent);
    if previous != scheme
        && let Some(settings) = gtk::Settings::default()
        && settings
            .find_property("gtk-interface-color-scheme")
            .is_some()
    {
        let value = match scheme {
            SchemeChoice::System => "default",
            SchemeChoice::Light => "light",
            SchemeChoice::Dark => "dark",
        };
        if let Some(pspec) = settings.find_property("gtk-interface-color-scheme")
            && let Some(class) = glib::EnumClass::with_type(pspec.value_type())
            && let Some(value) = class.to_value_by_nick(value)
        {
            settings.set_property_from_value("gtk-interface-color-scheme", &value);
        }
    }
    schedule();
}

/// Gives `window` the classes of the current look, now and on every change.
pub(crate) fn register_window(window: &impl IsA<gtk::Window>) {
    let theme = theme();
    let window = window.upcast_ref::<gtk::Window>();
    window.add_css_class("hk");
    theme.windows.borrow_mut().push(window.downgrade());
    classes(window, &current());
}

/// The current look.
pub(crate) fn current() -> Look {
    theme().look.get().unwrap_or_default()
}

/// Calls `listener` after every change of the look.
pub(crate) fn connect_changed(listener: impl Fn(&Look) + 'static) {
    theme().listeners.borrow_mut().push(Box::new(listener));
}

/// Gives `widget` the dark glass surface while the scheme is dark, and leaves
/// it on the light surfaces of the window while it is light: the player deck
/// and the popovers opened from it.
pub(crate) fn glass_when_dark(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    let set = |widget: &gtk::Widget, look: &Look| {
        if look.dark {
            widget.add_css_class("hk-glass");
        } else {
            widget.remove_css_class("hk-glass");
        }
    };
    set(widget, &current());
    let weak = widget.downgrade();
    connect_changed(move |look| {
        if let Some(widget) = weak.upgrade() {
            set(&widget, look);
        }
    });
}

fn classes(window: &gtk::Window, look: &Look) {
    let (add, remove) = if look.dark {
        ("hk-dark", "hk-light")
    } else {
        ("hk-light", "hk-dark")
    };
    window.remove_css_class(remove);
    window.add_css_class(add);
}

fn schedule() {
    if theme().pending.replace(true) {
        return;
    }
    glib::idle_add_local_once(|| {
        theme().pending.set(false);
        apply_now();
    });
}

fn resolve(theme: &Theme) -> Look {
    let r = theme.reported.get();
    let dark = match theme.scheme.get() {
        SchemeChoice::Dark => true,
        SchemeChoice::Light => false,
        // The desktop's preference; with none, the player's own default is dark.
        SchemeChoice::System => r.portal_scheme.or(r.gtk_scheme).unwrap_or(true),
    };
    let system_accent = if r.portal {
        r.portal_accent
    } else {
        r.gtk_accent
    };
    let raw = theme
        .accent
        .get()
        .preset()
        .or(system_accent)
        .unwrap_or(accent::DEFAULT);
    let reduced = r.portal_reduced_motion || r.gtk_animations == Some(false);
    Look::new(dark, r.portal_contrast, reduced, raw)
}

fn apply_now() {
    let theme = theme();
    let look = resolve(&theme);
    let changed = theme.look.get() != Some(look);
    if !changed {
        return;
    }
    theme.look.set(Some(look));
    if let Some(providers) = theme.providers.borrow().as_ref() {
        providers.update(&look);
    }
    theme.windows.borrow_mut().retain(|w| {
        let Some(window) = w.upgrade() else {
            return false;
        };
        classes(&window, &look);
        true
    });
    for listener in theme.listeners.borrow().iter() {
        listener(&look);
    }
}

fn update(change: impl FnOnce(&mut Reported)) {
    let theme = theme();
    let mut reported = theme.reported.get();
    change(&mut reported);
    theme.reported.set(reported);
    schedule();
}

fn read_gtk(settings: &gtk::Settings) {
    let scheme = settings
        .find_property("gtk-interface-color-scheme")
        .and_then(|_| {
            let value = settings.property_value("gtk-interface-color-scheme");
            glib::EnumValue::from_value(&value).map(|(_, v)| v.nick().to_owned())
        });
    // Only a scheme the desktop chose counts; while the user's override is
    // written there, the override decides anyway.
    let scheme = match scheme.as_deref() {
        Some("dark") => Some(true),
        Some("light") => Some(false),
        _ => None,
    };
    let accent = settings
        .find_property("gtk-accent-color")
        .and_then(|_| settings.property::<Option<gdk::RGBA>>("gtk-accent-color"))
        .and_then(|c| {
            Rgba::from_floats(
                f64::from(c.red()),
                f64::from(c.green()),
                f64::from(c.blue()),
            )
        });
    let animations = settings.is_gtk_enable_animations();
    update(|r| {
        if theme().scheme.get() == SchemeChoice::System {
            r.gtk_scheme = scheme;
        }
        r.gtk_accent = accent;
        r.gtk_animations = Some(animations);
    });
}

/// Follows the Settings portal through the desktop crate's monitor, on a thread
/// of its own; its values come back to the main loop.
fn watch_portal() {
    let (sender, receiver) = async_channel::unbounded::<Appearance>();
    let spawned = std::thread::Builder::new()
        .name("helikodu-appearance".into())
        .spawn(move || {
            let changes = sender.clone();
            let Ok((monitor, first)) = AppearanceMonitor::start(move |appearance| {
                let _ = changes.send_blocking(appearance);
            }) else {
                // No portal: GtkSettings is the only source.
                return;
            };
            if sender.send_blocking(first).is_err() {
                return;
            }
            // The monitor follows the portal for as long as the window listens.
            while !sender.is_closed() {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            drop(monitor);
        });
    if spawned.is_err() {
        return;
    }
    glib::spawn_future_local(async move {
        while let Ok(appearance) = receiver.recv().await {
            portal_values(&appearance);
        }
    });
}

fn portal_values(appearance: &Appearance) {
    let scheme = match appearance.color_scheme {
        PortalScheme::PreferDark => Some(true),
        PortalScheme::PreferLight => Some(false),
        _ => None,
    };
    let accent = appearance
        .accent_color
        .and_then(|c| Rgba::from_floats(c.red, c.green, c.blue));
    let contrast = appearance.contrast == Contrast::High;
    let reduced = appearance.reduced_motion;
    update(|r| {
        r.portal = true;
        r.portal_scheme = scheme;
        r.portal_accent = accent;
        r.portal_contrast = contrast;
        r.portal_reduced_motion = reduced;
    });
}

/// The CSS of the second provider: the accent variants, high contrast and
/// reduced motion.
pub(crate) fn look_css(look: &Look) -> String {
    let mut css = String::new();
    for dark in [true, false] {
        let set = AccentSet::new(look.accent.raw, dark, look.high_contrast);
        let base = if dark { tokens::DARK } else { tokens::LIGHT };
        css.push_str(&format!(
            ".hk-{} {{ --hk-accent: {}; --hk-accent-fill: {}; --hk-accent-text: {}; --hk-sel: {}; --hk-on-accent: {}; --hk-on-raw: {};",
            if dark { "dark" } else { "light" },
            set.raw.hex(),
            set.fill.hex(),
            set.text.hex(),
            set.sel.css(),
            set.on_fill.hex(),
            set.on_raw.hex(),
        ));
        if look.high_contrast {
            css.push_str(&format!(
                " --hk-line: {}; --hk-t3: {};",
                base.line2.hex(),
                base.t2.hex()
            ));
        }
        css.push_str(" }\n");
    }
    let dark = AccentSet::new(look.accent.raw, true, look.high_contrast);
    css.push_str(&format!(
        ".hk-glass {{ --hk-accent: {}; --hk-accent-fill: {}; --hk-accent-text: {}; --hk-sel: {}; --hk-on-accent: {}; --hk-on-raw: {};",
        dark.raw.hex(),
        dark.glass_fill.hex(),
        dark.glass_fill.hex(),
        dark.sel.css(),
        super::color::AccentSet::new(dark.glass_fill, true, false).on_raw.hex(),
        dark.on_raw.hex(),
    ));
    if look.high_contrast {
        css.push_str(&format!(
            " --hk-line: {}; --hk-t3: {};",
            tokens::glass::LINE2.hex(),
            tokens::glass::T2.hex()
        ));
    }
    css.push_str(" }\n");
    // The deck is light in the light scheme: the signal colours, made for
    // glass, are darkened until they read on its surface.
    let surface = super::color::palette(false, look.high_contrast).win;
    css.push_str(&format!(
        ".hk-light .hk-deck, .hk-light .hk-mini {{ --hk-signal-bit-perfect-text: {}; --hk-signal-processed-text: {}; }}\n",
        tokens::signal::bit_perfect::TEXT.ensure(surface, 4.5).hex(),
        tokens::signal::processed::TEXT.ensure(surface, 4.5).hex(),
    ));
    if look.reduced_motion {
        css.push_str(".hk, .hk * { transition-duration: 0; animation-duration: 0; }\n");
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_wins_over_the_desktop() {
        let theme = Theme::default();
        theme.reported.set(Reported {
            portal: true,
            portal_scheme: Some(false),
            ..Reported::default()
        });
        assert!(!resolve(&theme).dark);
        theme.scheme.set(SchemeChoice::Dark);
        assert!(resolve(&theme).dark);
    }

    #[test]
    fn the_portal_accent_and_the_presets() {
        let theme = Theme::default();
        assert_eq!(resolve(&theme).accent.raw, accent::DEFAULT);
        let blue = Rgba::rgb(0x35, 0x84, 0xE4);
        theme.reported.set(Reported {
            portal: true,
            portal_accent: Some(blue),
            gtk_accent: Some(Rgba::WHITE),
            ..Reported::default()
        });
        assert_eq!(resolve(&theme).accent.raw, blue);
        theme.accent.set(AccentChoice::Violet);
        assert_eq!(resolve(&theme).accent.raw, Rgba::rgb(0xB5, 0x9D, 0xF5));
    }

    #[test]
    fn high_contrast_and_reduced_motion_reach_the_css() {
        let look = Look::new(true, true, true, accent::DEFAULT);
        let css = look_css(&look);
        assert!(css.contains("--hk-line: #3B3E46"));
        assert!(css.contains("transition-duration: 0"));
        let plain = look_css(&Look::default());
        assert!(!plain.contains("transition"));
        assert!(plain.contains("--hk-accent-text: #F3C169"));
    }
}
