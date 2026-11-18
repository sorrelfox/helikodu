// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Measures the start-up, memory, scrolling and idle budgets of the desktop
//! application over a real library. It starts the front end the way the
//! application does (`helikodu_ui_gtk::run`, the service from the XDG folders
//! of the environment), then measures from inside the process:
//!
//! - the time from process start to the first frame, to the first frame with
//!   albums in the grid, and to the main loop being idle after that;
//! - RSS after start-up, after scrolling the album grid and after scrolling the
//!   track table;
//! - frame intervals and frame work while scrolling the album grid and the
//!   track table (slowly, fast, and jumping at random);
//! - CPU time over a minute while stopped, paused, and playing with the window
//!   hidden and minimized, with the frames painted meanwhile.
//!
//! The settings must select the null output, so nothing is ever heard. It is
//! run by `tests/ui/perf.py`, under a headless compositor:
//!
//! ```sh
//! tests/ui/perf.py --database library.db
//! ```
//!
//! Options: `--out FILE` (a JSON object per run is appended; default stdout),
//! `--label TEXT`, `--startup-only`, `--scroll-secs N` (per scroll mode,
//! default 5), `--idle-secs N` (per idle phase, default 60), `--launched-us T`
//! (the `CLOCK_MONOTONIC` time in microseconds at which the caller started this
//! process, so that the start-up times include loading the program).

#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use helikodu_service::files::AppDirs;
use helikodu_service::{Service, ServiceConfig};
use helikodu_ui_gtk::backend::{AlbumQuery, Backend, BackendError, PlayerRequest, Repeat, service};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{MainWindow, RunOptions, WindowStyle};

#[derive(Debug, Clone)]
struct Options {
    out: Option<PathBuf>,
    label: String,
    startup_only: bool,
    scroll_secs: f64,
    idle_secs: u64,
    launched_us: Option<i64>,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        out: None,
        label: String::from("run"),
        startup_only: false,
        scroll_secs: 5.0,
        idle_secs: 60,
        launched_us: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--startup-only" {
            options.startup_only = true;
            continue;
        }
        let value = args.next().ok_or(format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--out" => options.out = Some(PathBuf::from(value)),
            "--label" => options.label = value,
            "--scroll-secs" => options.scroll_secs = value.parse().map_err(|_| "bad seconds")?,
            "--idle-secs" => options.idle_secs = value.parse().map_err(|_| "bad seconds")?,
            "--launched-us" => options.launched_us = Some(value.parse().map_err(|_| "bad time")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

/// The results, written as one JSON object.
#[derive(Debug, Default)]
struct Report {
    fields: Vec<(String, String)>,
}

impl Report {
    fn number(&mut self, key: &str, value: f64) {
        let text = if value.is_finite() {
            format!("{value:.3}")
        } else {
            String::from("null")
        };
        self.fields.push((key.to_owned(), text));
    }

    fn text(&mut self, key: &str, value: &str) {
        let escaped: String = value
            .chars()
            .flat_map(|c| match c {
                '"' => vec!['\\', '"'],
                '\\' => vec!['\\', '\\'],
                c if c.is_control() => vec![' '],
                c => vec![c],
            })
            .collect();
        self.fields.push((key.to_owned(), format!("\"{escaped}\"")));
    }

    fn json(&self) -> String {
        let mut out = String::from("{");
        for (i, (key, value)) in self.fields.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "\"{key}\": {value}");
        }
        out.push('}');
        out
    }
}

/// `CLOCK_MONOTONIC` in microseconds.
fn now_us() -> i64 {
    glib::monotonic_time()
}

/// The resident set size in MiB.
fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

/// CPU time of the process and of its main thread, and the main thread's
/// context switches (its wake-ups, roughly).
#[derive(Debug, Clone, Copy, Default)]
struct Cpu {
    /// utime + stime of the whole process, in seconds (10 ms resolution, but it
    /// includes threads that ended).
    process_ticks: f64,
    /// Run time of all live threads, in seconds (nanosecond resolution).
    threads: f64,
    /// Run time of the main thread, in seconds.
    main: f64,
    /// Voluntary and involuntary context switches of the main thread.
    main_switches: u64,
    /// Context switches of all live threads.
    all_switches: u64,
}

fn stat_ticks(path: &str) -> Option<f64> {
    let stat = std::fs::read_to_string(path).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After the name: state is field 3; utime and stime are fields 14 and 15.
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / 100.0)
}

fn schedstat(path: &str) -> Option<f64> {
    let s = std::fs::read_to_string(path).ok()?;
    let ns: f64 = s.split_whitespace().next()?.parse().ok()?;
    Some(ns / 1e9)
}

fn switches(path: &str) -> u64 {
    std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .filter(|l| l.contains("ctxt_switches:"))
                .filter_map(|l| l.split(':').nth(1)?.trim().parse::<u64>().ok())
                .sum()
        })
        .unwrap_or(0)
}

fn cpu() -> Cpu {
    let pid = std::process::id();
    let mut cpu = Cpu {
        process_ticks: stat_ticks("/proc/self/stat").unwrap_or(f64::NAN),
        main: schedstat(&format!("/proc/self/task/{pid}/schedstat")).unwrap_or(f64::NAN),
        main_switches: switches(&format!("/proc/self/task/{pid}/status")),
        ..Cpu::default()
    };
    if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
        for task in tasks.flatten() {
            let dir = task.path();
            cpu.threads += schedstat(&format!("{}/schedstat", dir.display())).unwrap_or(0.0);
            cpu.all_switches += switches(&format!("{}/status", dir.display()));
        }
    }
    cpu
}

/// Percentile of sorted values.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn summarise(report: &mut Report, prefix: &str, intervals: &[f64], work: &[f64]) {
    let mut i = intervals.to_vec();
    i.sort_by(f64::total_cmp);
    let mut w = work.to_vec();
    w.sort_by(f64::total_cmp);
    report.number(&format!("{prefix}.frames"), i.len() as f64);
    report.number(
        &format!("{prefix}.interval_mean_ms"),
        i.iter().sum::<f64>() / i.len().max(1) as f64,
    );
    report.number(&format!("{prefix}.interval_p95_ms"), percentile(&i, 95.0));
    report.number(&format!("{prefix}.interval_p99_ms"), percentile(&i, 99.0));
    report.number(
        &format!("{prefix}.interval_max_ms"),
        i.last().copied().unwrap_or(f64::NAN),
    );
    report.number(
        &format!("{prefix}.over_50ms"),
        i.iter().filter(|&&v| v > 50.0).count() as f64,
    );
    report.number(
        &format!("{prefix}.over_1_5_refresh"),
        i.iter().filter(|&&v| v > 25.0).count() as f64,
    );
    report.number(&format!("{prefix}.work_p95_ms"), percentile(&w, 95.0));
    report.number(&format!("{prefix}.work_p99_ms"), percentile(&w, 99.0));
    report.number(
        &format!("{prefix}.work_max_ms"),
        w.last().copied().unwrap_or(f64::NAN),
    );
}

/// Every widget below `root`.
fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut all = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c.clone());
            all.push(c);
        }
    }
    all
}

async fn wait(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

/// Runs the main loop until `done` holds or `seconds` pass; whether it held.
async fn wait_for(seconds: f64, done: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    while !done() {
        if Instant::now() > end {
            return false;
        }
        wait(20).await;
    }
    true
}

/// Scrolls `adjustment` once per frame in three modes and adds the frame
/// statistics under `prefix`.
async fn scroll(
    window: &MainWindow,
    adjustment: &gtk::Adjustment,
    seconds: f64,
    report: &mut Report,
    prefix: &str,
) {
    let Some(clock) = window.frame_clock() else {
        report.text(&format!("{prefix}.error"), "no frame clock");
        return;
    };
    for (mode, step) in [("slow", 12.0), ("fast", 900.0), ("jump", -1.0)] {
        adjustment.set_value(0.0);
        wait(300).await;
        let work = Rc::new(RefCell::new(Vec::new()));
        let intervals = Rc::new(RefCell::new(Vec::new()));
        let started = Rc::new(Cell::new(None::<Instant>));
        let last = Rc::new(Cell::new(None::<i64>));
        let s = started.clone();
        let before = clock.connect_update(move |_| s.set(Some(Instant::now())));
        let (s, w) = (started.clone(), work.clone());
        let after = clock.connect_after_paint(move |_| {
            if let Some(start) = s.take() {
                w.borrow_mut().push(start.elapsed().as_secs_f64() * 1000.0);
            }
        });
        let (l, i, a) = (last.clone(), intervals.clone(), adjustment.clone());
        let seed = Cell::new(0x2545_F491_4F6C_DD1D_u64);
        let tick = window.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            if let Some(previous) = l.replace(Some(now)) {
                i.borrow_mut().push((now - previous) as f64 / 1000.0);
            }
            let range = a.upper() - a.page_size();
            let next = if step < 0.0 {
                let mut x = seed.get();
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                seed.set(x);
                (x % 10_000) as f64 / 10_000.0 * range
            } else {
                let v = a.value() + step;
                if v > range { 0.0 } else { v }
            };
            a.set_value(next);
            glib::ControlFlow::Continue
        });
        glib::timeout_future(Duration::from_secs_f64(seconds)).await;
        tick.remove();
        clock.disconnect(before);
        clock.disconnect(after);
        summarise(
            report,
            &format!("{prefix}.{mode}"),
            &intervals.borrow(),
            &work.borrow(),
        );
    }
    adjustment.set_value(0.0);
}

/// Measures CPU over `seconds` and the frames the window paints meanwhile.
async fn idle(window: &MainWindow, seconds: u64, report: &mut Report, prefix: &str) {
    let painted = Rc::new(Cell::new(0u64));
    let handler = window.frame_clock().map(|clock| {
        let p = painted.clone();
        (
            clock.connect_after_paint(move |_| p.set(p.get() + 1)),
            clock,
        )
    });
    let start = cpu();
    let t0 = Instant::now();
    glib::timeout_future_seconds(u32::try_from(seconds).unwrap_or(60)).await;
    let elapsed = t0.elapsed().as_secs_f64();
    let end = cpu();
    if let Some((id, clock)) = handler {
        clock.disconnect(id);
    }
    let pct = |a: f64, b: f64| (b - a) / elapsed * 100.0;
    report.number(&format!("{prefix}.seconds"), elapsed);
    report.number(
        &format!("{prefix}.cpu_process_pct"),
        pct(start.process_ticks, end.process_ticks),
    );
    report.number(
        &format!("{prefix}.cpu_threads_pct"),
        pct(start.threads, end.threads),
    );
    report.number(&format!("{prefix}.cpu_main_pct"), pct(start.main, end.main));
    report.number(
        &format!("{prefix}.main_wakeups_per_s"),
        end.main_switches.saturating_sub(start.main_switches) as f64 / elapsed,
    );
    report.number(
        &format!("{prefix}.all_switches_per_s"),
        end.all_switches.saturating_sub(start.all_switches) as f64 / elapsed,
    );
    report.number(&format!("{prefix}.frames_painted"), painted.get() as f64);
}

/// The measurements after start-up, then quits.
async fn measure(
    window: MainWindow,
    backend: Backend,
    options: Options,
    report: Rc<RefCell<Report>>,
) {
    let mut r = Report::default();
    wait(1000).await;
    r.number("rss_after_startup_mib", rss_mib());
    if !options.startup_only {
        // The album grid.
        let grid = descendants(window.upcast_ref())
            .into_iter()
            .find(|w| w.is::<gtk::GridView>() && w.is_mapped())
            .and_then(|w| w.downcast::<gtk::GridView>().ok());
        match grid.and_then(|g| g.vadjustment()) {
            Some(adjustment) => {
                r.number("albums.upper_px", adjustment.upper());
                scroll(&window, &adjustment, options.scroll_secs, &mut r, "albums").await;
            }
            None => r.text("albums.error", "no album grid shown"),
        }
        r.number("rss_after_albums_scroll_mib", rss_mib());

        // The track table.
        let total = backend
            .library()
            .counts()
            .map(|c| c.tracks as u32)
            .unwrap_or(0);
        r.number("library_tracks", f64::from(total));
        let t0 = Instant::now();
        window.open_page("tracks");
        let loaded = wait_for(60.0, || window.track_table_stats().0 >= total).await;
        let rows = wait_for(10.0, || window.track_table_stats().1 > 0).await;
        r.number("tracks.keys_loaded_ms", t0.elapsed().as_secs_f64() * 1000.0);
        if loaded && rows {
            wait(500).await;
            match window.track_table_adjustment() {
                Some(adjustment) => {
                    scroll(&window, &adjustment, options.scroll_secs, &mut r, "tracks").await
                }
                None => r.text("tracks.error", "no track table adjustment"),
            }
        } else {
            r.text("tracks.error", "the track table did not load in time");
        }
        r.number("rss_after_tracks_scroll_mib", rss_mib());
        window.open_page("albums");
        wait(1000).await;
        r.number("rss_peak_mib", {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find_map(|l| l.strip_prefix("VmHWM:"))
                        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
                })
                .map_or(f64::NAN, |kb| kb / 1024.0)
        });

        // Idle: stopped, paused, playing with the window hidden and minimized.
        backend.send(PlayerRequest::Stop);
        wait(3000).await;
        idle(&window, options.idle_secs, &mut r, "idle_stopped").await;

        let first = backend
            .library()
            .album_keys(&AlbumQuery::default())
            .ok()
            .and_then(|k| k.first().copied());
        if let Some(album) = first {
            backend.send(PlayerRequest::SetRepeat(Repeat::All));
            backend.send(PlayerRequest::PlayAlbum {
                album,
                start: 0,
                shuffle: false,
            });
            let playing = wait_for(10.0, || window.is_playing()).await;
            r.number("playback_started", if playing { 1.0 } else { 0.0 });
            if playing {
                wait(1000).await;
                backend.send(PlayerRequest::PlayPause);
                wait_for(5.0, || !window.is_playing()).await;
                wait(2000).await;
                idle(&window, options.idle_secs, &mut r, "idle_paused").await;

                backend.send(PlayerRequest::PlayPause);
                wait_for(5.0, || window.is_playing()).await;
                wait(2000).await;
                idle(&window, 10, &mut r, "playing_shown").await;
                window.set_visible(false);
                wait(2000).await;
                idle(&window, options.idle_secs, &mut r, "playing_hidden").await;
                r.number(
                    "playing_hidden.still_playing",
                    if window.is_playing() { 1.0 } else { 0.0 },
                );
                window.set_visible(true);
                window.present();
                wait(1000).await;
                window.minimize();
                wait(2000).await;
                idle(&window, options.idle_secs, &mut r, "playing_minimized").await;
                backend.send(PlayerRequest::Stop);
                wait(500).await;
            }
        } else {
            r.text("idle_playing.error", "the library has no albums");
        }
    }
    report.borrow_mut().fields.extend(r.fields);
    if let Some(app) = window.application() {
        app.quit();
    }
}

/// The service over the XDG folders of the environment, as the application
/// starts it (without the desktop's device reservation).
fn connect() -> Result<Backend, BackendError> {
    let fail = |e: &dyn std::fmt::Display| BackendError(e.to_string());
    let dirs = AppDirs::from_env().map_err(|e| fail(&e))?;
    let service = Service::start(ServiceConfig::new(dirs)).map_err(|e| fail(&e))?;
    service::connect(
        service,
        service::ServiceOptions {
            mpris: false,
            inhibit_suspend: false,
        },
    )
}

fn main() -> ExitCode {
    let main_us = now_us();
    let options = match options() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("perf: {e}");
            return ExitCode::FAILURE;
        }
    };
    let launched = options.launched_us.unwrap_or(main_us);
    let report = Rc::new(RefCell::new(Report::default()));
    report.borrow_mut().text("label", &options.label);
    report.borrow_mut().number(
        "startup.process_to_main_ms",
        (main_us - launched) as f64 / 1000.0,
    );

    let backend: Rc<RefCell<Option<Backend>>> = Rc::new(RefCell::new(None));
    let b = backend.clone();
    let rep = report.clone();
    let connect_once = move || {
        let t0 = now_us();
        let result = connect();
        rep.borrow_mut().number(
            "startup.service_connect_ms",
            (now_us() - t0) as f64 / 1000.0,
        );
        if let Ok(backend) = &result {
            b.replace(Some(backend.clone()));
        }
        result
    };

    // Watch for the main window, then for its frames.
    let rep = report.clone();
    let opts = options.clone();
    let watching = Rc::new(Cell::new(false));
    glib::timeout_add_local(Duration::from_millis(1), move || {
        if watching.get() {
            return glib::ControlFlow::Break;
        }
        let Some(window) = gtk::Window::list_toplevels()
            .into_iter()
            .find_map(|w| w.downcast::<MainWindow>().ok())
        else {
            return glib::ControlFlow::Continue;
        };
        watching.set(true);
        let ms = move |t: i64| (t - launched) as f64 / 1000.0;
        rep.borrow_mut()
            .number("startup.window_found_ms", ms(now_us()));
        let Some(clock) = window.frame_clock() else {
            // Not realised yet: look at the next frame of the display instead.
            watching.set(false);
            return glib::ControlFlow::Continue;
        };
        let first = Rc::new(Cell::new(false));
        let done = Rc::new(Cell::new(false));
        let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::new(RefCell::new(None));
        let (rep2, w, h, c) = (rep.clone(), window.clone(), handler.clone(), clock.clone());
        let (backend, opts) = (backend.clone(), opts.clone());
        let id = clock.connect_after_paint(move |_| {
            let t = now_us();
            if !first.replace(true) {
                rep2.borrow_mut().number("startup.first_frame_ms", ms(t));
            }
            if done.get() || w.album_grid_stats().1 == 0 {
                return;
            }
            done.set(true);
            rep2.borrow_mut()
                .number("startup.first_frame_with_albums_ms", ms(t));
            rep2.borrow_mut()
                .number("startup.albums_in_grid", f64::from(w.album_grid_stats().0));
            if let Some(id) = h.borrow_mut().take() {
                c.disconnect(id);
            }
            let (rep3, w3) = (rep2.clone(), w.clone());
            let (backend, opts) = (backend.clone(), opts.clone());
            glib::idle_add_local_full(glib::Priority::LOW, move || {
                rep3.borrow_mut()
                    .number("startup.interactive_ms", ms(now_us()));
                if let Some(backend) = backend.borrow().clone() {
                    glib::spawn_future_local(measure(
                        w3.clone(),
                        backend,
                        opts.clone(),
                        rep3.clone(),
                    ));
                }
                glib::ControlFlow::Break
            });
        });
        handler.replace(Some(id));
        glib::ControlFlow::Break
    });
    // Give up when something hangs.
    glib::timeout_add_local_once(Duration::from_secs(120 + 6 * options.idle_secs), || {
        eprintln!("perf: timed out");
        std::process::exit(2);
    });

    let code = helikodu_ui_gtk::run(
        RunOptions {
            style: Some(WindowStyle::Integrated),
            scheme: SchemeChoice::Dark,
            accent: AccentChoice::Amber,
            notifications: false,
        },
        &[],
        connect_once,
    );
    let mut report = report.borrow_mut();
    report.number("rss_at_exit_mib", rss_mib());
    let line = report.json();
    match &options.out {
        Some(path) => {
            use std::io::Write as _;
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| writeln!(f, "{line}"));
            if let Err(e) = written {
                eprintln!("perf: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
        None => println!("{line}"),
    }
    code
}
