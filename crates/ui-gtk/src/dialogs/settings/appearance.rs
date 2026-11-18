// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Appearance: colour scheme and accent (applied at once), window
//! style (from the next start), the menu bar, striped tables, the language
//! and notifications. The tray rows come in a later version and are left out.

use std::rc::Rc;

use gtk::prelude::*;
use helikodu_model::settings::{Accent, ColorScheme, WindowStyle};

use super::{Context, page_body};
use crate::backend::services::Settings;
use crate::dialogs::common::{
    bind_segmented, divider, dropdown, key_caps, note, option_card, row, segmented, switch_row,
    with_note,
};
use crate::i18n::{fill, tr};
use crate::style::{AccentChoice, SchemeChoice, set_choice};
use crate::widgets::label;

/// Applies the scheme and accent of `settings` to every window.
pub(crate) fn apply_look(settings: &Settings) {
    let scheme = match settings.appearance.color_scheme {
        ColorScheme::System => SchemeChoice::System,
        ColorScheme::Light => SchemeChoice::Light,
        ColorScheme::Dark => SchemeChoice::Dark,
    };
    let accent = match settings.appearance.accent {
        Accent::Desktop => AccentChoice::System,
        Accent::Amber => AccentChoice::Amber,
        Accent::Blue => AccentChoice::Blue,
        Accent::Green => AccentChoice::Green,
        Accent::Violet => AccentChoice::Violet,
    };
    set_choice(scheme, accent);
}

fn scheme_note(scheme: ColorScheme) -> String {
    match scheme {
        ColorScheme::System => tr("Follows the light or dark setting of your desktop."),
        ColorScheme::Light => tr("Always light, whatever the desktop uses."),
        ColorScheme::Dark => tr("Always dark, whatever the desktop uses."),
    }
}

fn scheme(context: &Rc<Context>) -> gtk::Box {
    const SCHEMES: [ColorScheme; 3] = [ColorScheme::System, ColorScheme::Light, ColorScheme::Dark];
    let (group, buttons) = segmented(&[tr("System"), tr("Light"), tr("Dark")]);
    let current = context.settings().appearance.color_scheme;
    let explanation = note(&scheme_note(current));
    let text = explanation.downgrade();
    let weak = Rc::downgrade(context);
    bind_segmented(
        &buttons,
        SCHEMES.iter().position(|s| *s == current).unwrap_or(0),
        move |i| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            if context.update(|s| s.appearance.color_scheme = SCHEMES[i]) {
                apply_look(&context.settings());
                if let Some(text) = text.upgrade() {
                    text.set_text(&scheme_note(SCHEMES[i]));
                }
            }
        },
    );
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.append(&group);
    column.append(&explanation);
    column
}

fn accent(context: &Rc<Context>) -> gtk::Box {
    let current = context.settings().appearance.accent;
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let desktop = gtk::ToggleButton::new();
    desktop.add_css_class("hk-toggle");
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    inner.append(&gtk::Image::from_icon_name("video-display-symbolic"));
    inner.append(&gtk::Label::new(Some(&tr("From the desktop"))));
    desktop.set_child(Some(&inner));
    desktop.update_property(&[gtk::accessible::Property::Label(&tr("From the desktop"))]);
    desktop.set_valign(gtk::Align::Center);
    line.append(&desktop);
    let bar = gtk::Separator::new(gtk::Orientation::Vertical);
    bar.add_css_class("hk-divider");
    bar.set_margin_top(6);
    bar.set_margin_bottom(6);
    line.append(&bar);
    let mut choices = vec![(desktop.clone(), Accent::Desktop)];
    let names = [tr("Amber"), tr("Blue"), tr("Green"), tr("Violet")];
    for (preset, name) in Accent::PRESETS.iter().zip(names) {
        let swatch = gtk::ToggleButton::new();
        swatch.add_css_class("hk-swatch");
        swatch.add_css_class(match preset {
            Accent::Amber => "amber",
            Accent::Blue => "blue",
            Accent::Green => "green",
            _ => "violet",
        });
        swatch.set_tooltip_text(Some(&name));
        swatch.update_property(&[gtk::accessible::Property::Label(&name)]);
        swatch.set_group(Some(&desktop));
        swatch.set_valign(gtk::Align::Center);
        line.append(&swatch);
        choices.push((swatch, *preset));
    }
    for (button, value) in &choices {
        if *value == current {
            button.set_active(true);
        }
        let weak = Rc::downgrade(context);
        let value = *value;
        button.connect_toggled(move |button| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            if button.is_active()
                && context.settings().appearance.accent != value
                && context.update(|s| s.appearance.accent = value)
            {
                apply_look(&context.settings());
            }
        });
    }
    with_note(
        &line,
        &tr(
            "Uses the accent colour of your desktop, or amber if the desktop has none. Helikodu adjusts it to stay readable.",
        ),
    )
}

/// A small drawing of a window layout for the style cards.
fn thumbnail(classic: bool) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(112);
    area.set_content_height(70);
    area.set_valign(gtk::Align::Start);
    area.set_draw_func(move |_, cr, w, h| {
        let look = crate::style::current();
        let p = &look.palette;
        let set = |cr: &gtk::cairo::Context, c: crate::style::color::Rgba| {
            let c = c.to_gdk();
            cr.set_source_rgba(
                f64::from(c.red()),
                f64::from(c.green()),
                f64::from(c.blue()),
                f64::from(c.alpha()),
            );
        };
        let (w, h) = (f64::from(w), f64::from(h));
        set(cr, p.side);
        cr.rectangle(0.0, 0.0, w, h);
        let _ = cr.fill();
        set(cr, p.line2);
        cr.rectangle(0.5, 0.5, w - 1.0, h - 1.0);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        let top = if classic {
            // The desktop's title bar and the menu bar.
            set(cr, p.line);
            cr.rectangle(1.0, 1.0, w - 2.0, 8.0);
            let _ = cr.fill();
            set(cr, p.t3);
            for i in 0..6 {
                cr.rectangle(4.0 + f64::from(i) * 9.0, 12.0, 6.0, 1.5);
            }
            let _ = cr.fill();
            16.0
        } else {
            set(cr, p.t3);
            cr.rectangle(w - 16.0, 4.0, 2.0, 2.0);
            cr.rectangle(w - 11.0, 4.0, 2.0, 2.0);
            cr.rectangle(w - 6.0, 4.0, 2.0, 2.0);
            let _ = cr.fill();
            6.0
        };
        set(cr, p.t3);
        for i in 0..4 {
            cr.rectangle(5.0, top + 6.0 + f64::from(i) * 6.0, 14.0, 1.5);
        }
        let _ = cr.fill();
        set(cr, p.line2);
        for i in 0..2 {
            for j in 0..4 {
                cr.rectangle(
                    28.0 + f64::from(j) * 19.0,
                    top + 6.0 + f64::from(i) * 17.0,
                    13.0,
                    13.0,
                );
            }
        }
        let _ = cr.fill();
        set(cr, look.accent.fill);
        cr.arc(w / 2.0, h - 7.0, 2.5, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        set(cr, p.line2);
        cr.rectangle(8.0, h - 7.5, w / 2.0 - 16.0, 1.0);
        cr.rectangle(w / 2.0 + 8.0, h - 7.5, w / 2.0 - 16.0, 1.0);
        let _ = cr.fill();
    });
    crate::widgets::redraw_on_theme_change(&area);
    area
}

fn window_style(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.set_homogeneous(true);
    let current = context.settings().appearance.window_style;
    let integrated_on = current == WindowStyle::Integrated
        || current == WindowStyle::Auto && crate::window::integrated_by_default();
    let (integrated, integrated_check) = option_card(
        &tr("Integrated"),
        &tr("The window buttons sit in Helikodu's own header."),
        None,
    );
    let (classic, classic_check) = option_card(
        &tr("Classic"),
        &tr("The title bar of your desktop, with an optional menu bar."),
        Some(&integrated_check),
    );
    for (card, classic_style, default_note) in [
        (&integrated, false, tr("Default on GNOME and KDE Plasma")),
        (&classic, true, tr("Default on other desktops")),
    ] {
        card.prepend(&thumbnail(classic_style));
        if let Some(text) = card.last_child().and_downcast::<gtk::Box>() {
            let line = label(&default_note, &["hk-t3", "hk-secondary"]);
            line.set_ellipsize(gtk::pango::EllipsizeMode::None);
            line.set_wrap(true);
            text.append(&line);
        }
    }
    if integrated_on {
        integrated_check.set_active(true);
    } else {
        classic_check.set_active(true);
    }
    for (check, style) in [
        (&integrated_check, WindowStyle::Integrated),
        (&classic_check, WindowStyle::Classic),
    ] {
        let weak = Rc::downgrade(context);
        check.connect_toggled(move |check| {
            if let Some(context) = weak.upgrade()
                && check.is_active()
                && context.update(|s| s.appearance.window_style = style)
            {
                context.rebuild();
            }
        });
    }
    line.append(&integrated);
    line.append(&classic);
    with_note(
        &line,
        &tr("A new window style is used when Helikodu starts again."),
    )
}

fn menu_bar(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let classic = match settings.appearance.window_style {
        WindowStyle::Classic => true,
        WindowStyle::Integrated => false,
        WindowStyle::Auto => !crate::window::integrated_by_default(),
    };
    let explanation = if classic {
        tr("Ctrl+M shows or hides it at any time.")
    } else {
        tr("Only in the Classic window style. Integrated uses the main menu instead.")
    };
    let (line, switch) = switch_row(&tr("Show the menu bar"), Some(&explanation));
    line.insert_child_after(&key_caps(&["Ctrl", "M"]), line.first_child().as_ref());
    switch.set_active(settings.appearance.menu_bar);
    switch.set_sensitive(classic);
    let weak = Rc::downgrade(context);
    switch.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if !context.update(|s| s.appearance.menu_bar = on) {
            switch.set_active(!on);
            return;
        }
        if let Some(main) = context.main.upgrade() {
            main.set_menubar_shown(on);
        }
    });
    line
}

fn switch_setting(
    context: &Rc<Context>,
    title: &str,
    explanation: &str,
    value: bool,
    set: fn(&mut Settings, bool),
    after: fn(&Context, bool),
) -> gtk::Box {
    let (line, switch) = switch_row(title, Some(explanation));
    switch.set_active(value);
    let weak = Rc::downgrade(context);
    switch.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if context.update(|s| set(s, on)) {
            after(&context, on);
        } else {
            switch.set_active(!on);
        }
    });
    line
}

/// A language's own name, from a locale such as `sv_SE.UTF-8`; the locale
/// itself for languages without a name here.
fn language_name(locale: &str) -> String {
    let code = locale.split(['_', '.', '@']).next().unwrap_or_default();
    match code {
        "" | "C" | "POSIX" | "en" => "English",
        "de" => "Deutsch",
        "es" => "Español",
        "et" => "Eesti",
        "fi" => "Suomi",
        "fr" => "Français",
        "it" => "Italiano",
        "nl" => "Nederlands",
        "pl" => "Polski",
        "pt" => "Português",
        "sv" => "Svenska",
        _ => return locale.split('.').next().unwrap_or(locale).to_owned(),
    }
    .to_owned()
}

fn language(context: &Rc<Context>) -> gtk::Box {
    let locale = gtk::glib::language_names()
        .first()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let name = language_name(&locale);
    let choice = dropdown(&[fill(&tr("Same as the desktop ({})"), &[&name])], 0, 320);
    choice.update_property(&[gtk::accessible::Property::Label(&tr("Language"))]);
    let _ = context;
    with_note(
        &choice,
        &tr(
            "After a change, the library is sorted again for the new language. This runs in the background.",
        ),
    )
}

/// The Appearance page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let settings = context.settings();
    let (scroller, body) = page_body();
    body.append(&row(&tr("Colour scheme"), None, &scheme(context)));
    body.append(&row(&tr("Accent colour"), None, &accent(context)));
    body.append(&row(&tr("Window style"), None, &window_style(context)));
    body.append(&row(&tr("Menu bar"), None, &menu_bar(context)));
    body.append(&divider());
    body.append(&row(
        &tr("Tables"),
        None,
        &switch_setting(
            context,
            &tr("Shade every second row"),
            &tr("Makes long rows easier to follow across wide tables."),
            settings.appearance.stripe_rows,
            |s, on| s.appearance.stripe_rows = on,
            |_, _| {},
        ),
    ));
    body.append(&row(&tr("Language"), None, &language(context)));
    body.append(&divider());
    body.append(&row(
        &tr("Notifications"),
        None,
        &switch_setting(
            context,
            &tr("Show a notification when the track changes"),
            &tr("Not shown while the Helikodu window is in front."),
            settings.appearance.notifications,
            |s, on| s.appearance.notifications = on,
            |context, on| {
                if let Some(main) = context.main.upgrade() {
                    main.set_notifications(on);
                }
            },
        ),
    ));
    scroller.upcast()
}
