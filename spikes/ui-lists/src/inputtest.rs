// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks the window shell with real pointer input: double-click and drag on
//! the top strips, and the desktop's window buttons. The pointer is driven
//! through mutter's remote-desktop D-Bus API, so this runs only against a
//! (headless) mutter or GNOME Shell on a private session bus, never on the
//! user's desktop.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::shell::Shell;

const BTN_LEFT: i32 = 0x110;

struct Pointer {
    session: gio::DBusProxy,
    stream: String,
    /// Screen position of window coordinate (0, 0).
    offset: Cell<(f64, f64)>,
    /// Last pointer position seen by the window, in window coordinates.
    seen: Rc<Cell<Option<(f64, f64)>>>,
    pub trace: RefCell<Vec<String>>,
}

async fn call(
    p: &gio::DBusProxy,
    method: &str,
    args: Option<glib::Variant>,
) -> Result<glib::Variant, String> {
    p.call_future(method, args.as_ref(), gio::DBusCallFlags::NONE, 5000)
        .await
        .map_err(|e| format!("{method}: {}", e.message()))
}

async fn proxy(name: &str, path: &str, iface: &str) -> Result<gio::DBusProxy, String> {
    gio::DBusProxy::for_bus_future(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None,
        name,
        path,
        iface,
    )
    .await
    .map_err(|e| format!("proxy {iface}: {}", e.message()))
}

async fn wait(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

impl Pointer {
    async fn connect(seen: Rc<Cell<Option<(f64, f64)>>>) -> Result<Self, String> {
        let rd = proxy(
            "org.gnome.Mutter.RemoteDesktop",
            "/org/gnome/Mutter/RemoteDesktop",
            "org.gnome.Mutter.RemoteDesktop",
        )
        .await?;
        let path = call(&rd, "CreateSession", None)
            .await?
            .child_value(0)
            .str()
            .map(str::to_owned)
            .ok_or("no session path")?;
        let session = proxy(
            "org.gnome.Mutter.RemoteDesktop",
            &path,
            "org.gnome.Mutter.RemoteDesktop.Session",
        )
        .await?;
        let id: String = session
            .cached_property("SessionId")
            .and_then(|v| v.get())
            .ok_or("no SessionId")?;
        let sc = proxy(
            "org.gnome.Mutter.ScreenCast",
            "/org/gnome/Mutter/ScreenCast",
            "org.gnome.Mutter.ScreenCast",
        )
        .await?;
        let props = HashMap::from([("remote-desktop-session-id".to_owned(), id.to_variant())]);
        let sc_path = call(&sc, "CreateSession", Some((props,).to_variant()))
            .await?
            .child_value(0)
            .str()
            .map(str::to_owned)
            .ok_or("no screencast session")?;
        let sc_session = proxy(
            "org.gnome.Mutter.ScreenCast",
            &sc_path,
            "org.gnome.Mutter.ScreenCast.Session",
        )
        .await?;
        let empty: HashMap<String, glib::Variant> = HashMap::new();
        let stream = call(&sc_session, "RecordMonitor", Some(("", empty).to_variant()))
            .await?
            .child_value(0)
            .str()
            .map(str::to_owned)
            .ok_or("no stream")?;
        call(&session, "Start", None).await?;
        wait(500).await;
        Ok(Self {
            session,
            stream,
            offset: Cell::new((0.0, 0.0)),
            seen,
            trace: RefCell::new(Vec::new()),
        })
    }

    async fn move_screen(&self, x: f64, y: f64) -> Result<(), String> {
        call(
            &self.session,
            "NotifyPointerMotionAbsolute",
            Some((self.stream.as_str(), x, y).to_variant()),
        )
        .await?;
        wait(60).await;
        Ok(())
    }

    /// Moves to a window coordinate, correcting the offset from what the window reports.
    async fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
        for _ in 0..3 {
            let (ox, oy) = self.offset.get();
            self.seen.set(None);
            self.move_screen(x + ox, y + oy).await?;
            self.trace.borrow_mut().push(format!(
                "move_to({x:.0},{y:.0}) offset ({ox:.0},{oy:.0}) seen {:?}",
                self.seen.get()
            ));
            match self.seen.get() {
                Some((sx, sy)) if (sx - x).abs() < 1.0 && (sy - y).abs() < 1.0 => return Ok(()),
                Some((sx, sy)) => self.offset.set((ox + x - sx, oy + y - sy)),
                None => {
                    // The window moved away from under the pointer: find it again.
                    self.move_screen(950.0, 530.0).await?;
                    self.seen.set(None);
                    self.move_screen(960.0, 540.0).await?;
                    if let Some((sx, sy)) = self.seen.get() {
                        self.offset.set((960.0 - sx, 540.0 - sy));
                    }
                }
            }
        }
        Ok(())
    }

    async fn button(&self, down: bool) -> Result<(), String> {
        call(
            &self.session,
            "NotifyPointerButton",
            Some((BTN_LEFT, down).to_variant()),
        )
        .await?;
        wait(30).await;
        Ok(())
    }

    async fn click(&self) -> Result<(), String> {
        self.button(true).await?;
        self.button(false).await
    }
}

fn point_in(
    window: &gtk::Window,
    w: &impl IsA<gtk::Widget>,
    fx: f32,
    fy: f32,
) -> Option<(f64, f64)> {
    let b = w.compute_bounds(window)?;
    Some((
        f64::from(b.x() + b.width() * fx),
        f64::from(b.y() + b.height() * fy),
    ))
}

fn find_button(c: &gtk::WindowControls, class: &str) -> Option<gtk::Widget> {
    let mut child = c.first_child();
    while let Some(ch) = child {
        if ch.has_css_class(class) && ch.is_visible() {
            return Some(ch);
        }
        child = ch.next_sibling();
    }
    None
}

/// Runs the checks and returns one line per check.
pub async fn run(shell: Rc<Shell>) -> Vec<String> {
    let mut out = Vec::new();
    let window: gtk::Window = shell.window.clone().upcast();
    let seen: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = seen.clone();
    motion.connect_motion(move |_, x, y| s.set(Some((x, y))));
    let s = seen.clone();
    motion.connect_enter(move |_, x, y| s.set(Some((x, y))));
    window.add_controller(motion);
    let closed = Rc::new(Cell::new(false));
    let c = closed.clone();
    window.connect_close_request(move |_| {
        c.set(true);
        glib::Propagation::Stop
    });
    let states: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    if let Some(top) = window.surface().and_downcast::<gdk::Toplevel>() {
        let st = states.clone();
        top.connect_state_notify(move |t| st.borrow_mut().push(format!("{:?}", t.state())));
    }

    let p = match Pointer::connect(seen.clone()).await {
        Ok(p) => p,
        Err(e) => {
            out.push(format!("remote-desktop input not available: {e}"));
            return out;
        }
    };
    // Find where the window is: aim at the screen centre and read back.
    let _ = p.move_screen(950.0, 530.0).await;
    let _ = p.move_screen(960.0, 540.0).await;
    match seen.get() {
        Some((x, y)) => p.offset.set((960.0 - x, 540.0 - y)),
        None => out.push("window did not see the pointer at the screen centre".into()),
    }

    // 1. Double-click on an empty part of the content header (a GtkWindowHandle).
    let header_point = shell
        .header_gap
        .as_ref()
        .and_then(|w| point_in(&window, w, 0.5, 0.5));
    if let Some((x, y)) = header_point {
        let before = window.is_maximized();
        let _ = p.move_to(x, y).await;
        let _ = p.click().await;
        let _ = p.click().await;
        wait(700).await;
        let after = window.is_maximized();
        out.push(format!(
            "double-click on content header strip: maximized {before} -> {after} ({})",
            if after != before {
                "works"
            } else {
                "NO EFFECT"
            }
        ));
        if let Some((x, y)) = shell
            .header_gap
            .as_ref()
            .and_then(|w| point_in(&window, w, 0.5, 0.5))
        {
            let _ = p.move_to(x, y).await;
            let _ = p.click().await;
            let _ = p.click().await;
            wait(700).await;
            out.push(format!(
                "double-click again: maximized -> {}",
                window.is_maximized()
            ));
        }
    }

    // 2. Drag on an empty part of the panel strip.
    if let Some((x, y)) = shell
        .strip_gap
        .as_ref()
        .and_then(|w| point_in(&window, w, 0.5, 0.5))
    {
        let _ = p.move_to(x, y).await;
        let (ox, oy) = p.offset.get();
        let _ = p.button(true).await;
        for i in 1..=10 {
            let _ = p
                .move_screen(x + ox - 15.0 * f64::from(i), y + oy + 8.0 * f64::from(i))
                .await;
        }
        let _ = p.button(false).await;
        wait(300).await;
        seen.set(None);
        let _ = p.move_screen(x + ox - 150.0 + 1.0, y + oy + 80.0).await;
        match seen.get() {
            Some((sx, sy)) => {
                // If the window followed, the pointer is back near the press point;
                // if not, it is about (-149, +80) away from it.
                let near = ((sx - x - 1.0).powi(2) + (sy - y).powi(2)).sqrt();
                let far = ((sx - x + 149.0).powi(2) + (sy - y - 80.0).powi(2)).sqrt();
                let moved = near < far;
                out.push(format!(
                    "drag on panel strip by (-150, +80): pointer now at window ({sx:.0}, {sy:.0}), pressed at ({x:.0}, {y:.0}) -> {}",
                    if moved { "window moved with the pointer (works)" } else { "window did NOT move" }
                ));
                if moved {
                    p.offset.set((ox - 150.0, oy + 80.0));
                }
            }
            None => out.push("drag: window did not see the pointer afterwards".into()),
        }
    }

    // 3. The desktop's window buttons. GTK rebuilds them when the window
    // state changes, so look them up again before every click.
    let find = |class: &str| {
        find_button(&shell.controls_end, class)
            .or_else(|| find_button(&shell.controls_start, class))
    };
    let target = |class: &str| find(class).and_then(|b| point_in(&window, &b, 0.5, 0.5));
    if target("maximize").is_some() {
        for round in ["maximize button", "maximize button again"] {
            let before = window.is_maximized();
            if let Some((x, y)) = target("maximize") {
                let _ = p.move_to(x, y).await;
                let _ = p.click().await;
                wait(700).await;
                out.push(format!(
                    "{round}: maximized {before} -> {}",
                    window.is_maximized()
                ));
            }
        }
    } else {
        out.push("no maximize button in this layout".into());
    }
    if let Some((x, y)) = target("close") {
        let _ = p.move_to(x, y).await;
        let _ = p.click().await;
        wait(500).await;
        out.push(format!(
            "close button: close-request received = {}",
            closed.get()
        ));
    }
    if let Some((x, y)) = target("minimize") {
        states.borrow_mut().clear();
        let _ = p.move_to(x, y).await;
        let _ = p.click().await;
        wait(700).await;
        let suspended = window
            .surface()
            .and_downcast::<gdk::Toplevel>()
            .is_some_and(|t| t.state().contains(gdk::ToplevelState::SUSPENDED));
        out.push(format!(
            "minimize button: toplevel state changes afterwards {:?}, suspended now = {suspended}",
            states.borrow()
        ));
    }
    if std::env::var_os("HK_TRACE").is_some() {
        out.extend(p.trace.borrow().iter().map(|t| format!("trace: {t}")));
    }
    out
}
