// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! UI spike: large lists, async covers, light/dark and the window shell in
//! GTK 4 without libadwaita. Runs a scripted scroll, prints frame-time and
//! memory tables, and exits by itself.

#![forbid(unsafe_code)]

mod catalog;
mod covers;
mod grid;
mod inputtest;
mod model;
mod shell;
mod stats;
mod table;
mod theme;

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, glib};

use stats::{Recorder, SharedRecorder};

#[derive(Debug, Clone)]
struct Args {
    view: String,
    rows: u32,
    albums: u32,
    columns: usize,
    width: i32,
    height: i32,
    scheme: theme::Override,
    css: bool,
    out: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    covers_dir: PathBuf,
    covers: u32,
    workers: usize,
    cache: usize,
    scale: f64,
    hold: f64,
    label: String,
    follow_only: bool,
    max_columns: u32,
    input_test: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut a = Args {
            view: "table".into(),
            rows: 200_000,
            albums: 20_000,
            columns: 8,
            width: 1440,
            height: 900,
            scheme: theme::Override::System,
            css: true,
            out: None,
            screenshot: None,
            covers_dir: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/covers")),
            covers: 4000,
            workers: 4,
            cache: 600,
            scale: 1.0,
            hold: 0.0,
            label: String::new(),
            follow_only: false,
            max_columns: 12,
            input_test: false,
        };
        let mut it = std::env::args().skip(1);
        while let Some(k) = it.next() {
            let mut v = || it.next().ok_or(format!("missing value for {k}"));
            match k.as_str() {
                "--view" => a.view = v()?,
                "--rows" => a.rows = v()?.parse().map_err(|e| format!("{e}"))?,
                "--albums" => a.albums = v()?.parse().map_err(|e| format!("{e}"))?,
                "--columns" => a.columns = v()?.parse().map_err(|e| format!("{e}"))?,
                "--size" => {
                    let s = v()?;
                    let (w, h) = s.split_once('x').ok_or("size is WxH")?;
                    a.width = w.parse().map_err(|e| format!("{e}"))?;
                    a.height = h.parse().map_err(|e| format!("{e}"))?;
                }
                "--scheme" => {
                    a.scheme = match v()?.as_str() {
                        "dark" => theme::Override::Dark,
                        "light" => theme::Override::Light,
                        _ => theme::Override::System,
                    }
                }
                "--no-css" => a.css = false,
                "--out" => a.out = Some(v()?.into()),
                "--screenshot" => a.screenshot = Some(v()?.into()),
                "--covers-dir" => a.covers_dir = v()?.into(),
                "--covers" => a.covers = v()?.parse().map_err(|e| format!("{e}"))?,
                "--workers" => a.workers = v()?.parse().map_err(|e| format!("{e}"))?,
                "--cache" => a.cache = v()?.parse().map_err(|e| format!("{e}"))?,
                "--time-scale" => a.scale = v()?.parse().map_err(|e| format!("{e}"))?,
                "--hold" => a.hold = v()?.parse().map_err(|e| format!("{e}"))?,
                "--label" => a.label = v()?,
                "--follow-only" => a.follow_only = true,
                "--input-test" => a.input_test = true,
                "--max-columns" => a.max_columns = v()?.parse().map_err(|e| format!("{e}"))?,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(a)
    }
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Idle,
    /// Scroll at this many pixels per second.
    Scroll(f64),
    /// Jump to a random position every frame.
    Jump,
    /// Switch dark/light every this many seconds.
    Theme(f64),
}

#[derive(Debug, Clone, Copy)]
struct Step {
    name: &'static str,
    secs: f64,
    action: Action,
}

fn script(view: &str, follow_only: bool) -> Vec<Step> {
    if follow_only {
        // Only watch: the stand-in portal flips the scheme.
        return vec![
            Step {
                name: "warm-up (idle)",
                secs: 2.0,
                action: Action::Idle,
            },
            Step {
                name: "following the portal",
                secs: 8.0,
                action: Action::Idle,
            },
        ];
    }
    // Rows are 30 px in the table; grid rows are about 230 px.
    let (slow, fast) = if view == "grid" {
        (900.0, 9000.0)
    } else {
        (1500.0, 15000.0)
    };
    let mut s = vec![
        Step {
            name: "warm-up (idle)",
            secs: 2.0,
            action: Action::Idle,
        },
        Step {
            name: "slow scroll",
            secs: 8.0,
            action: Action::Scroll(slow),
        },
        Step {
            name: "fast scroll",
            secs: 6.0,
            action: Action::Scroll(fast),
        },
    ];
    if view == "grid" {
        s.push(Step {
            name: "after fast (covers fill)",
            secs: 2.0,
            action: Action::Idle,
        });
    }
    s.push(Step {
        name: "random jumps",
        secs: 5.0,
        action: Action::Jump,
    });
    if view == "grid" {
        s.push(Step {
            name: "after jumps (covers fill)",
            secs: 2.0,
            action: Action::Idle,
        });
    }
    s.push(Step {
        name: "theme switch every 0.5 s",
        secs: 4.0,
        action: Action::Theme(0.5),
    });
    s.push(Step {
        name: "idle",
        secs: 2.0,
        action: Action::Idle,
    });
    s
}

fn renderer_name(w: &gtk::Window) -> String {
    w.renderer()
        .map_or("none".into(), |r| r.type_().name().to_string())
}

fn screenshot(w: &gtk::Window, path: &std::path::Path) {
    let paintable = gtk::WidgetPaintable::new(Some(w));
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, f64::from(w.width()), f64::from(w.height()));
    if let (Some(node), Some(r)) = (snap.to_node(), w.renderer()) {
        let tex = r.render_texture(&node, None);
        if let Err(e) = tex.save_to_png(path) {
            eprintln!("screenshot failed: {e}");
        }
    }
}

fn main() -> glib::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("fake-portal") {
        let ms = std::env::args()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1500);
        let layout = std::env::args()
            .nth(3)
            .unwrap_or_else(|| "appmenu:close".into());
        theme::run_fake_portal(ms, layout);
        return glib::ExitCode::SUCCESS;
    }
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return glib::ExitCode::FAILURE;
        }
    };
    if args.view == "grid"
        && let Err(e) = covers::write_covers(&args.covers_dir, args.covers)
    {
        eprintln!("cannot write covers: {e}");
        return glib::ExitCode::FAILURE;
    }
    let rss_start = stats::rss_kib().0;
    let app = gtk::Application::builder()
        .application_id("org.helikodu.SpikeUiLists")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let a = args.clone();
    app.connect_activate(move |app| activate(app, &a, rss_start));
    app.run_with_args(&["ui-lists-spike"])
}

fn activate(app: &gtk::Application, args: &Args, rss_start: u64) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let accent_provider = theme::install_base(&display, args.css);
    let rss_gtk = stats::rss_kib().0;
    let t_build = Instant::now();
    let (content, thumbs, title): (gtk::ScrolledWindow, Option<Rc<grid::Thumbs>>, String) =
        match args.view.as_str() {
            "grid" => {
                let (sw, th) = grid::build(
                    args.albums,
                    args.covers_dir.clone(),
                    args.covers,
                    args.workers,
                    args.cache,
                    args.max_columns,
                );
                (sw, Some(th), format!("Albums ({})", args.albums))
            }
            _ => (
                table::build(args.rows, args.columns),
                None,
                format!("Tracks ({})", args.rows),
            ),
        };
    let sh = shell::build(app, &content, &title, args.width, args.height);
    let window: gtk::Window = sh.window.clone().upcast();
    let rec: SharedRecorder = Rc::new(RefCell::new(Recorder::default()));
    let th = theme::Theme::new(&window, accent_provider, args.scheme, rec.clone());
    th.watch();
    if !args.css {
        window.remove_css_class("hk");
    }
    let rss_built = stats::rss_kib().0;
    let build_ms = t_build.elapsed().as_secs_f64() * 1000.0;
    rec.borrow_mut().presented_at = Some(Instant::now());
    sh.window.present();

    let first_frame: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
    let steps = script(&args.view, args.follow_only);
    let step_idx = Rc::new(Cell::new(usize::MAX));
    let step_start = Rc::new(Cell::new(0i64));
    let last_ft = Rc::new(Cell::new(0i64));
    let last_toggle = Rc::new(Cell::new(0i64));
    let fill_started: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let rng = Rc::new(RefCell::new(catalog::Rng::new(42)));
    let shell_info = Rc::new(sh);
    if args.input_test {
        let sh = shell_info.clone();
        let app = app.clone();
        let out_path = args.out.clone();
        let label = args.label.clone();
        glib::spawn_future_local(async move {
            glib::timeout_future(std::time::Duration::from_millis(1500)).await;
            let lines = inputtest::run(sh.clone()).await;
            let mut out = format!("## window shell with injected pointer input {label}\n");
            for l in shell::describe(&sh) {
                out.push_str(&format!("- {l}\n"));
            }
            for l in lines {
                out.push_str(&format!("- {l}\n"));
            }
            println!("{out}");
            if let Some(p) = out_path {
                let mut prev = std::fs::read_to_string(&p).unwrap_or_default();
                prev.push_str(&out);
                prev.push('\n');
                let _ = std::fs::write(&p, prev);
            }
            app.quit();
        });
        return;
    }
    let args = args.clone();
    let scale = args.scale;
    let wrapped = Rc::new(Cell::new(0u32));

    let rec_c = rec.clone();
    let win_c = window.clone();
    let attached = Rc::new(Cell::new(false));
    let start_instant = Instant::now();
    content.add_tick_callback(move |sw, clock| {
        let ft = clock.frame_time();
        if !attached.get() {
            stats::attach(clock, &rec_c);
            attached.set(true);
            first_frame.set(Some(start_instant.elapsed().as_secs_f64() * 1000.0));
            step_start.set(ft);
            last_ft.set(ft);
            step_idx.set(0);
            rec_c.borrow_mut().start_phase(steps[0].name);
            return glib::ControlFlow::Continue;
        }
        let dt = (ft - last_ft.get()) as f64 / 1e6;
        last_ft.set(ft);
        let mut i = step_idx.get();
        let elapsed = (ft - step_start.get()) as f64 / 1e6;
        if elapsed >= steps[i].secs * scale {
            // Phase notes before moving on.
            if let Some(th) = &thumbs {
                let vis = th.prioritise_visible(sw.upcast_ref());
                rec_c.borrow_mut().note(format!(
                    "at end of phase: {vis} visible tiles without cover, {} bound tiles without cover",
                    th.unfilled()
                ));
            }
            i += 1;
            if i >= steps.len() {
                rec_c.borrow_mut().finish();
                finish(&args, &rec_c.borrow(), &thumbs, &shell_info, &th, &win_c, first_frame.get(), build_ms, (rss_start, rss_gtk, rss_built), wrapped.get());
                if args.hold > 0.0 {
                    let w = win_c.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_secs_f64(args.hold), move || w.close());
                } else {
                    win_c.close();
                }
                return glib::ControlFlow::Break;
            }
            step_idx.set(i);
            step_start.set(ft);
            rec_c.borrow_mut().start_phase(steps[i].name);
            fill_started.set(match steps[i].action {
                Action::Idle => Some(Instant::now()),
                _ => None,
            });
        }
        let adj = sw.vadjustment();
        let max = adj.upper() - adj.page_size();
        let visible_unfilled = thumbs.as_ref().map(|th| th.prioritise_visible(sw.upcast_ref()));
        match steps[i].action {
            Action::Idle => {
                if let (Some(t0), Some(0)) = (fill_started.get(), visible_unfilled) {
                    let ms = t0.elapsed().as_secs_f64() * 1000.0;
                    rec_c.borrow_mut().note(format!("all visible covers present {ms:.0} ms after scrolling stopped"));
                    fill_started.set(None);
                }
            }
            Action::Scroll(pps) => {
                let mut v = adj.value() + pps * dt;
                if v > max {
                    v = 0.0;
                    wrapped.set(wrapped.get() + 1);
                }
                adj.set_value(v);
            }
            Action::Jump => {
                let v = rng.borrow_mut().unit() * max;
                adj.set_value(v);
            }
            Action::Theme(every) => {
                if (ft - last_toggle.get()) as f64 / 1e6 >= every {
                    last_toggle.set(ft);
                    // A manual Light/Dark override, as from the settings.
                    let next = if th.current.get() == Some(theme::Scheme::Dark) { theme::Override::Light } else { theme::Override::Dark };
                    th.over.set(next);
                    th.resolve("scripted override");
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

#[allow(clippy::too_many_arguments)]
fn finish(
    args: &Args,
    rec: &Recorder,
    thumbs: &Option<Rc<grid::Thumbs>>,
    sh: &shell::Shell,
    th: &theme::Theme,
    win: &gtk::Window,
    first_frame: Option<f64>,
    build_ms: f64,
    rss: (u64, u64, u64),
    wrapped: u32,
) {
    let mut out = String::new();
    let display = WidgetExt::display(win);
    let _ = writeln!(out, "## {} {}", args.view, args.label);
    let _ = writeln!(
        out,
        "GTK {}.{}.{}, display {}, renderer {}, scale {} (surface {:.2}), window {}x{}, stylesheet {}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        display.type_().name(),
        renderer_name(win),
        win.scale_factor(),
        win.surface().map_or(0.0, |s| s.scale()),
        win.width(),
        win.height(),
        if args.css { "on" } else { "off" }
    );
    match args.view.as_str() {
        "grid" => {
            let _ = writeln!(
                out,
                "albums {}, cover files {}, workers {}, texture cache {}, grid max-columns {}",
                args.albums, args.covers, args.workers, args.cache, args.max_columns
            );
        }
        _ => {
            let _ = writeln!(
                out,
                "rows {}, columns {}",
                args.rows,
                if args.columns >= 12 {
                    12
                } else {
                    args.columns.min(8)
                }
            );
        }
    }
    let _ = writeln!(
        out,
        "model+view build {build_ms:.1} ms, first frame painted {:.0} ms after the window was presented",
        rec.first_paint_ms.unwrap_or(0.0)
    );
    let _ = first_frame;
    out.push_str(&stats::report(rec));
    let (rss_now, hwm) = stats::rss_kib();
    let mib = |k: u64| k as f64 / 1024.0;
    let _ = writeln!(
        out,
        "RSS MiB: process start {:.1}, after GTK init {:.1}, after building views {:.1}, end {:.1}, peak {:.1}",
        mib(rss.0),
        mib(rss.1),
        mib(rss.2),
        mib(rss_now),
        mib(hwm)
    );
    let _ = writeln!(
        out,
        "items materialised by the model: {}, rows fetched (data built): {}, wrapped to top: {wrapped}",
        model::ITEMS_MATERIALISED.with(Cell::get),
        model::ROWS_FETCHED.with(Cell::get)
    );
    if args.view == "grid" {
        let _ = writeln!(
            out,
            "tile widgets created: {}, most tiles bound at once: {}",
            grid::TILES_CREATED.with(Cell::get),
            grid::TILES_BOUND_MAX.with(Cell::get)
        );
    } else {
        let _ = writeln!(
            out,
            "cell widgets created: {}, cell binds: {}",
            table::CELLS_CREATED.with(Cell::get),
            table::CELL_BINDS.with(Cell::get)
        );
    }
    if let Some(th) = thumbs {
        let lat = stats::summarize(&th.latencies.borrow());
        let dec = stats::summarize(&th.decode_ms.borrow());
        let s = th
            .loader
            .stats
            .lock()
            .map(|s| format!("{s:?}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "covers: {s}, cache hits {}, stale results {}, textures cached {}",
            th.cache_hits.get(),
            th.stale_results.get(),
            th.cache.borrow().len()
        );
        let _ = writeln!(
            out,
            "cover decode+scale per image (worker): mean {:.1} ms, p95 {:.1}, worst {:.1} (n={})",
            dec.mean, dec.p95, dec.max, dec.n
        );
        let _ = writeln!(
            out,
            "bind-to-cover latency: p50 {:.1} ms, p95 {:.1}, p99 {:.1}, worst {:.1} (n={})",
            lat.p50, lat.p95, lat.p99, lat.max, lat.n
        );
    }
    let sw = stats::summarize(&rec.switch_costs);
    let _ = writeln!(
        out,
        "stylesheet switch to end of next frame: mean {:.1} ms, p95 {:.1}, worst {:.1} (n={})",
        sw.mean, sw.p95, sw.max, sw.n
    );
    let _ = writeln!(out, "theme log:");
    for l in th.log.borrow().iter() {
        let _ = writeln!(out, "  {l}");
    }
    let _ = writeln!(out, "shell:");
    for l in shell::describe(sh) {
        let _ = writeln!(out, "  {l}");
    }
    println!("{out}");
    if let Some(p) = &args.out {
        let mut prev = std::fs::read_to_string(p).unwrap_or_default();
        prev.push_str(&out);
        prev.push('\n');
        let _ = std::fs::write(p, prev);
    }
    if let Some(p) = &args.screenshot {
        screenshot(win, p);
    }
}
