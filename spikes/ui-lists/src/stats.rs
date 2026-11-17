// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Frame-time recording from the GdkFrameClock, percentiles and memory.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::Instant;

use gtk::gdk;

/// Samples of one phase of the scripted run.
#[derive(Debug, Default, Clone)]
pub struct Phase {
    pub name: String,
    /// Interval between consecutive frame-clock frame times, in ms.
    pub intervals: Vec<f64>,
    /// Main-thread time from before-paint to after-paint (layout, snapshot, render), in ms.
    pub work: Vec<f64>,
    /// Intervals between presentation times reported by the compositor, in ms.
    pub presented: Vec<f64>,
    pub rss_end_kib: u64,
    pub notes: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Recorder {
    pub phases: Vec<Phase>,
    last_frame_time: Option<i64>,
    last_presented: Option<i64>,
    paint_start: Option<Instant>,
    pub refresh_interval_us: i64,
    pub frames_total: u64,
    /// Set when the stylesheet is switched; the next after-paint stores the
    /// time from the switch to the end of that frame.
    pub switch_started: Option<Instant>,
    pub switch_costs: Vec<f64>,
    pub presented_at: Option<Instant>,
    pub first_paint_ms: Option<f64>,
}

impl Recorder {
    pub fn start_phase(&mut self, name: &str) {
        if let Some(p) = self.phases.last_mut() {
            p.rss_end_kib = rss_kib().0;
        }
        self.phases.push(Phase {
            name: name.to_owned(),
            ..Default::default()
        });
        self.last_frame_time = None;
        self.last_presented = None;
    }

    pub fn finish(&mut self) {
        if let Some(p) = self.phases.last_mut() {
            p.rss_end_kib = rss_kib().0;
        }
    }

    pub fn note(&mut self, s: String) {
        if let Some(p) = self.phases.last_mut() {
            p.notes.push(s);
        }
    }
}

pub type SharedRecorder = Rc<RefCell<Recorder>>;

/// Hooks the recorder into the frame clock of a realized widget.
pub fn attach(clock: &gdk::FrameClock, rec: &SharedRecorder) {
    let r = rec.clone();
    clock.connect_before_paint(move |_| {
        r.borrow_mut().paint_start = Some(Instant::now());
    });
    let r = rec.clone();
    clock.connect_after_paint(move |clock| {
        let mut rec = r.borrow_mut();
        rec.frames_total += 1;
        let now = Instant::now();
        let work = rec
            .paint_start
            .take()
            .map(|t| (now - t).as_secs_f64() * 1000.0);
        let ft = clock.frame_time();
        let interval = rec.last_frame_time.map(|l| (ft - l) as f64 / 1000.0);
        rec.last_frame_time = Some(ft);
        // Timings become complete a few frames later; look back a little.
        let counter = clock.frame_counter();
        let mut presented = None;
        if let Some(t) = clock.timings(counter - 4)
            && t.is_complete()
            && t.presentation_time() != 0
        {
            if t.refresh_interval() > 0 {
                rec.refresh_interval_us = t.refresh_interval();
            }
            let p = t.presentation_time();
            presented = rec.last_presented.map(|l| (p - l) as f64 / 1000.0);
            rec.last_presented = Some(p);
        }
        if rec.first_paint_ms.is_none() {
            rec.first_paint_ms = rec.presented_at.map(|t| (now - t).as_secs_f64() * 1000.0);
        }
        if let Some(t) = rec.switch_started.take() {
            rec.switch_costs.push((now - t).as_secs_f64() * 1000.0);
        }
        if let Some(phase) = rec.phases.last_mut() {
            if let Some(i) = interval {
                phase.intervals.push(i);
            }
            if let Some(w) = work {
                phase.work.push(w);
            }
            if let Some(p) = presented.filter(|p| *p > 0.0) {
                phase.presented.push(p);
            }
        }
    });
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Summary {
    pub n: usize,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

pub fn summarize(v: &[f64]) -> Summary {
    if v.is_empty() {
        return Summary::default();
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize];
    Summary {
        n: s.len(),
        mean: s.iter().sum::<f64>() / s.len() as f64,
        p50: pct(0.50),
        p95: pct(0.95),
        p99: pct(0.99),
        max: *s.last().unwrap_or(&0.0),
    }
}

/// (VmRSS, VmHWM) in KiB from /proc/self/status.
pub fn rss_kib() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// Markdown table of all phases.
pub fn report(rec: &Recorder) -> String {
    let mut out = String::new();
    let refresh = rec.refresh_interval_us as f64 / 1000.0;
    let _ = writeln!(
        out,
        "refresh interval reported by compositor: {refresh:.2} ms"
    );
    let _ = writeln!(
        out,
        "| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for p in &rec.phases {
        let i = summarize(&p.intervals);
        let w = summarize(&p.work);
        let late = if refresh > 0.0 {
            p.intervals.iter().filter(|x| **x > refresh * 1.5).count()
        } else {
            0
        };
        let over50 = p.intervals.iter().filter(|x| **x > 50.0).count();
        let _ = writeln!(
            out,
            "| {} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.1} |",
            p.name,
            i.n,
            i.mean,
            i.p95,
            i.p99,
            i.max,
            late,
            over50,
            w.mean,
            w.p95,
            w.p99,
            w.max,
            p.rss_end_kib as f64 / 1024.0
        );
    }
    for p in &rec.phases {
        for n in &p.notes {
            let _ = writeln!(out, "- {}: {}", p.name, n);
        }
    }
    out
}
