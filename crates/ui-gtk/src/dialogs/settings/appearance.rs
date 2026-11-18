// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Appearance: colour scheme and accent (applied at once), window
//! style (from the next start), the menu bar, striped tables, the language
//! (from the next start) and notifications; the tray icon and what closing the
//! window does, shown only where the desktop has a tray.

use std::rc::Rc;

use gtk::prelude::*;
use helikodu_model::settings::{Accent, ColorScheme, WindowStyle};

use super::{Context, page_body};
use crate::backend::services::Settings;
use crate::dialogs::common::{
    bind_segmented, compact_button, divider, dropdown, key_caps, note, on_selected, option_card,
    row, segmented, switch_row, with_note,
};
use crate::i18n::{self, fill, tr};
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

/// The languages of the Language dropdown after "Same as the desktop": English
/// and the translations, each by its own name. The code is what
/// `[appearance] language` stores.
const LANGUAGES: [(&str, &str); 12] = [
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("nl", "Nederlands"),
    ("pt_BR", "Português (Brasil)"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("zh_CN", "简体中文"),
];

/// A language's own name, from a locale such as `sv_SE.UTF-8`; the locale
/// itself for languages without a name here.
fn language_name(locale: &str) -> String {
    let name = locale.split(['.', '@']).next().unwrap_or_default();
    let (code, territory) = name.split_once('_').unwrap_or((name, ""));
    match (code, territory) {
        ("" | "C" | "POSIX" | "en", _) => "English",
        ("de", _) => "Deutsch",
        ("es", _) => "Español",
        ("et", _) => "Eesti",
        ("fi", _) => "Suomi",
        ("fr", _) => "Français",
        ("it", _) => "Italiano",
        ("ja", _) => "日本語",
        ("ko", _) => "한국어",
        ("nl", _) => "Nederlands",
        ("pl", _) => "Polski",
        ("pt", "BR") => "Português (Brasil)",
        ("pt", _) => "Português",
        ("ru", _) => "Русский",
        ("sv", _) => "Svenska",
        ("uk", _) => "Українська",
        ("zh", "TW" | "HK" | "MO") => "繁體中文",
        ("zh", _) => "简体中文",
        _ => return name.to_owned(),
    }
    .to_owned()
}

/// The language: the desktop's or a chosen one, used from the next start (the
/// library is sorted for it at once, in the background). "Restart now" shows
/// while the choice differs from the language Helikodu started in.
fn language(context: &Rc<Context>) -> gtk::Box {
    let current = context.settings().appearance.language.clone();
    let at_start = i18n::language_at_start(&current);
    let desktop = language_name(&i18n::desktop_language());
    let mut names = vec![fill(&tr("Same as the desktop ({})"), &[&desktop])];
    let mut codes = vec![String::new()];
    for (code, name) in LANGUAGES {
        names.push(name.to_owned());
        codes.push(code.to_owned());
    }
    // A language written into the settings file by hand.
    if !codes.contains(&current) {
        names.push(language_name(&current));
        codes.push(current.clone());
    }
    let selected = codes.iter().position(|c| *c == current).unwrap_or(0);
    let choice = dropdown(&names, selected, 320);
    choice.set_widget_name("language");
    choice.update_property(&[gtk::accessible::Property::Label(&tr("Language"))]);
    let restart = compact_button(&tr("Restart now"));
    restart.set_widget_name("language-restart");
    restart.set_visible(current != at_start);
    restart.connect_clicked(crate::dialogs::maintenance::restart);
    let weak = Rc::downgrade(context);
    let shown = restart.downgrade();
    on_selected(&choice, move |index| {
        let (Some(context), Some(code)) = (weak.upgrade(), codes.get(index)) else {
            return;
        };
        if context.settings().appearance.language != *code
            && context.update(|s| s.appearance.language.clone_from(code))
            && let Some(restart) = shown.upgrade()
        {
            restart.set_visible(*code != at_start);
        }
    });
    let column = with_note(
        &choice,
        &tr(
            "After a change, the library is sorted again for the new language. This runs in the background.",
        ),
    );
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let next_start = note(&tr("A new language is used when Helikodu starts again."));
    next_start.set_valign(gtk::Align::Center);
    line.append(&next_start);
    line.append(&restart);
    column.append(&line);
    column
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
    tray_rows(context, &body);
    scroller.upcast()
}

/// "System tray" and "Closing the window", hidden where the desktop has no
/// tray.
fn tray_rows(context: &Rc<Context>, body: &gtk::Box) {
    let settings = context.settings();
    let tray = row(
        &tr("System tray"),
        None,
        &switch_setting(
            context,
            &tr("Show an icon in the tray"),
            &tr("Click it to show or hide the window. Scroll over it to change the volume."),
            settings.appearance.tray_icon,
            |s, on| s.appearance.tray_icon = on,
            |context, _| {
                if let Some(main) = context.main.upgrade() {
                    main.apply_tray_setting();
                }
            },
        ),
    );
    let closing = dropdown(
        &[tr("Keeps playing in the tray"), tr("Quits Helikodu")],
        usize::from(!settings.appearance.close_to_tray),
        240,
    );
    closing.set_widget_name("close-to-tray");
    let weak = Rc::downgrade(context);
    crate::dialogs::common::on_selected(&closing, move |index| {
        if let Some(context) = weak.upgrade() {
            context.update(|s| s.appearance.close_to_tray = index == 0);
        }
    });
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.append(&closing);
    let always = label(&tr("Ctrl+Q always quits."), &["hk-t3", "hk-secondary"]);
    always.set_valign(gtk::Align::Center);
    line.append(&always);
    let close_row = row(&tr("Closing the window"), None, &line);
    let shown = |main: Option<crate::window::MainWindow>| main.is_some_and(|m| m.tray_offered());
    let visible = shown(context.main.upgrade());
    tray.set_visible(visible);
    close_row.set_visible(visible);
    body.append(&tray);
    body.append(&close_row);
    if let Some(main) = context.main.upgrade() {
        let tray = tray.downgrade();
        let close_row = close_row.downgrade();
        let owner = body.clone();
        main.connect_desktop_changed(&owner, move |main| {
            let visible = main.tray_offered();
            if let Some(tray) = tray.upgrade() {
                tray.set_visible(visible);
            }
            if let Some(close_row) = close_row.upgrade() {
                close_row.set_visible(visible);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_named_in_their_own_language() {
        assert_eq!(language_name("de_DE.UTF-8"), "Deutsch");
        assert_eq!(language_name("C.UTF-8"), "English");
        assert_eq!(language_name("pt_BR.UTF-8"), "Português (Brasil)");
        assert_eq!(language_name("pt_PT"), "Português");
        assert_eq!(language_name("zh_CN.UTF-8"), "简体中文");
        assert_eq!(language_name("zh_TW"), "繁體中文");
        assert_eq!(language_name("uk_UA.UTF-8"), "Українська");
        assert_eq!(language_name("sr_RS@latin"), "sr_RS");
        for (code, name) in LANGUAGES {
            assert_eq!(language_name(code), name, "{code}");
        }
    }
}
