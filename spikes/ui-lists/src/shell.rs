// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window shell of the design: no title bar, three top strips that drag
//! the window, the desktop's window buttons placed by GtkWindowControls, and
//! the player deck along the bottom.

use gtk::prelude::*;

#[derive(Debug)]
pub struct Shell {
    pub window: gtk::ApplicationWindow,
    pub controls_start: gtk::WindowControls,
    pub controls_end: gtk::WindowControls,
    /// Empty parts of the content header and the panel strip (drag areas).
    pub header_gap: Option<gtk::Widget>,
    pub strip_gap: Option<gtk::Widget>,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn icon_button(icon: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("hk-icon");
    b
}

pub fn build(
    app: &gtk::Application,
    content: &impl IsA<gtk::Widget>,
    title: &str,
    width: i32,
    height: i32,
) -> Shell {
    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some("Helikodu UI spike"));
    window.set_default_size(width, height);
    window.add_css_class("hk");
    window.add_css_class("hk-dark");
    // Integrated style: a hidden, empty title bar widget, so GTK draws no header
    // bar and the compositor draws no title bar either.
    let empty = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    empty.set_visible(false);
    window.set_titlebar(Some(&empty));

    let controls_start = gtk::WindowControls::new(gtk::PackType::Start);
    let controls_end = gtk::WindowControls::new(gtk::PackType::End);

    // Sidebar.
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.add_css_class("hk-sidebar");
    sidebar.set_size_request(228, -1);
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    brand.add_css_class("hk-header");
    brand.append(&controls_start);
    let name = label("Helikodu", &[]);
    name.set_hexpand(true);
    brand.append(&name);
    brand.append(&icon_button("sidebar-show-symbolic"));
    let brand_handle = gtk::WindowHandle::new();
    brand_handle.set_child(Some(&brand));
    sidebar.append(&brand_handle);
    let nav = gtk::Box::new(gtk::Orientation::Vertical, 2);
    nav.set_margin_start(8);
    nav.set_margin_end(8);
    nav.append(&label("Library", &["hk-section-label"]));
    let entries = gtk::StringList::new(&[
        "Home",
        "Albums",
        "Artists",
        "Tracks",
        "Genres",
        "Composers",
        "Folders",
    ]);
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, li| {
        if let Some(li) = li.downcast_ref::<gtk::ListItem>() {
            li.set_child(Some(&label("", &[])));
        }
    });
    factory.connect_bind(|_, li| {
        let Some(li) = li.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(l), Some(s)) = (
            li.child().and_downcast::<gtk::Label>(),
            li.item().and_downcast::<gtk::StringObject>(),
        ) else {
            return;
        };
        l.set_text(&s.string());
    });
    let navlist = gtk::ListView::new(
        Some(gtk::SingleSelection::new(Some(entries))),
        Some(factory),
    );
    navlist.add_css_class("hk-list");
    nav.append(&navlist);
    sidebar.append(&nav);

    // Content: header strip and the view.
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.add_css_class("hk-content");
    content_box.set_hexpand(true);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("hk-header");
    header.append(&icon_button("go-previous-symbolic"));
    header.append(&icon_button("go-next-symbolic"));
    let header_gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header_gap.set_size_request(80, 30);
    header.append(&header_gap);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search library…"));
    search.add_css_class("hk-input");
    search.add_css_class("search");
    search.set_hexpand(true);
    search.set_halign(gtk::Align::Center);
    search.set_size_request(440, -1);
    header.append(&search);
    header.append(&icon_button("open-menu-symbolic"));
    let header_handle = gtk::WindowHandle::new();
    header_handle.set_child(Some(&header));
    content_box.append(&header_handle);
    let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title_row.set_margin_start(24);
    title_row.set_margin_top(14);
    title_row.set_margin_bottom(10);
    title_row.append(&label(title, &["hk-view-title"]));
    content_box.append(&title_row);
    content_box.append(content);

    // Panel with tabs and the end window buttons.
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("hk-panel");
    panel.set_size_request(320, -1);
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    strip.add_css_class("hk-header");
    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    tabs.add_css_class("hk-tabs");

    let queue = gtk::ToggleButton::with_label("Queue");
    queue.set_active(true);
    tabs.append(&queue);
    tabs.append(&gtk::ToggleButton::with_label("Lyrics"));
    tabs.append(&gtk::ToggleButton::with_label("Signal path"));
    strip.append(&tabs);
    let strip_gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    strip_gap.set_hexpand(true);
    strip.append(&strip_gap);
    strip.append(&controls_end);
    let strip_handle = gtk::WindowHandle::new();
    strip_handle.set_child(Some(&strip));
    panel.append(&strip_handle);
    for i in 0..8 {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row.set_margin_start(16);
        row.set_margin_top(6);
        row.append(&label(
            &format!("{} Queued track", i + 1),
            if i == 2 { &["hk-playing"] } else { &[] },
        ));
        row.append(&label("Artist", &["hk-t3", "hk-secondary"]));
        panel.append(&row);
    }

    let main = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    main.set_vexpand(true);
    main.append(&sidebar);
    main.append(&content_box);
    main.append(&panel);

    // Deck: always dark.
    let deck = gtk::CenterBox::new();
    deck.add_css_class("hk-deck");
    deck.add_css_class("hk-glass");
    deck.set_size_request(-1, 105);
    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.001);
    seek.add_css_class("hk-slider");
    seek.set_value(0.37);
    seek.set_size_request(400, -1);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 2);
    left.set_margin_start(16);
    left.set_valign(gtk::Align::Center);
    left.append(&label("Now playing title", &[]));
    left.append(&label("1:23 / 4:56", &["hk-mono", "hk-t2"]));
    deck.set_start_widget(Some(&left));
    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    transport.set_valign(gtk::Align::Center);
    let prev = gtk::Button::from_icon_name("media-skip-backward-symbolic");
    prev.add_css_class("hk-transport");
    let play = gtk::Button::from_icon_name("media-playback-start-symbolic");
    play.add_css_class("hk-play");
    let next = gtk::Button::from_icon_name("media-skip-forward-symbolic");
    next.add_css_class("hk-transport");
    transport.append(&prev);
    transport.append(&play);
    transport.append(&next);
    transport.append(&seek);
    deck.set_center_widget(Some(&transport));
    let out = gtk::Button::with_label("Output · BIT-PERFECT");
    out.add_css_class("hk-output");
    out.set_valign(gtk::Align::Center);
    out.set_margin_end(16);
    deck.set_end_widget(Some(&out));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&main);
    root.append(&deck);
    window.set_child(Some(&root));

    Shell {
        window,
        controls_start,
        controls_end,
        header_gap: Some(header_gap.upcast()),
        strip_gap: Some(strip_gap.upcast()),
    }
}

/// A plain description of how the shell came out on this desktop.
pub fn describe(shell: &Shell) -> Vec<String> {
    let mut out = Vec::new();
    let w = &shell.window;
    let settings = gtk::Settings::default();
    let layout = settings
        .as_ref()
        .and_then(|s| s.gtk_decoration_layout())
        .map(|s| s.to_string());
    out.push(format!("gtk-decoration-layout = {layout:?}"));
    out.push(format!(
        "WindowControls start: empty={}, layout={:?}; end: empty={}, layout={:?}",
        shell.controls_start.is_empty(),
        shell.controls_start.decoration_layout(),
        shell.controls_end.is_empty(),
        shell.controls_end.decoration_layout()
    ));
    let buttons = |c: &gtk::WindowControls| {
        let mut names = Vec::new();
        let mut child = c.first_child();
        while let Some(ch) = child {
            if ch.is_visible() {
                names.push(
                    ch.css_classes()
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join("."),
                );
            }
            child = ch.next_sibling();
        }
        names
    };
    out.push(format!(
        "start buttons: {:?}",
        buttons(&shell.controls_start)
    ));
    out.push(format!("end buttons: {:?}", buttons(&shell.controls_end)));
    out.push(format!(
        "window css classes: {:?}",
        w.css_classes()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    ));
    out.push(format!(
        "decorated={} titlebar-hidden={}",
        w.is_decorated(),
        w.titlebar().is_some_and(|t| !t.is_visible())
    ));
    if let Some(theme) = settings.as_ref().and_then(|s| s.gtk_theme_name()) {
        out.push(format!(
            "gtk-theme-name = {theme}, icon theme = {:?}, font = {:?}",
            settings.as_ref().and_then(|s| s.gtk_icon_theme_name()),
            settings.as_ref().and_then(|s| s.gtk_font_name())
        ));
    }
    out
}
