// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The conversion or sync that runs, as the tool windows show it.
//!
//! One job runs at a time. Its events arrive on the main loop through a channel
//! and are kept here, so a window that is closed and opened again shows where the
//! job stands; the job itself goes on without a window. Nothing runs and nothing
//! is drawn while no job runs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gtk::glib;

use crate::backend::BackendError;
use crate::backend::export::{ConvertEvent, ConvertSummary, ItemOutcome, JobEvents};

/// What kind of job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobKind {
    /// The converter.
    Convert,
    /// A sync, to the destination with this index in the store.
    Sync,
}

/// Where one row of a job stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RowState {
    /// Not started.
    Waiting,
    /// Under way, with the share done.
    Working(f32),
    /// Written, with its size.
    Written(u64),
    /// Left alone: it was there already.
    Skipped,
    /// It failed, for this reason.
    Failed(String),
}

/// A job and where it stands.
#[derive(Debug, Clone)]
pub(crate) struct JobView {
    /// The kind.
    pub(crate) kind: JobKind,
    /// What it works on: the destination's identifier for a sync.
    pub(crate) target: String,
    /// Each row.
    pub(crate) rows: Vec<RowState>,
    /// The bytes written so far.
    pub(crate) bytes: u64,
    /// When it started.
    pub(crate) started: Instant,
    /// How it ended, once it has.
    pub(crate) finished: Option<ConvertSummary>,
}

impl JobView {
    /// The rows that ended.
    pub(crate) fn done(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| !matches!(r, RowState::Waiting | RowState::Working(_)))
            .count()
    }

    /// The share of the whole job done, from 0 to 1.
    pub(crate) fn fraction(&self) -> f64 {
        if self.rows.is_empty() {
            return 1.0;
        }
        let sum: f64 = self
            .rows
            .iter()
            .map(|r| match r {
                RowState::Waiting => 0.0,
                RowState::Working(f) => f64::from(*f),
                _ => 1.0,
            })
            .sum();
        (sum / self.rows.len() as f64).clamp(0.0, 1.0)
    }

    /// The seconds still to go, once enough is done to tell.
    pub(crate) fn seconds_left(&self) -> Option<f64> {
        let fraction = self.fraction();
        let elapsed = self.started.elapsed().as_secs_f64();
        (fraction > 0.02 && elapsed > 2.0).then(|| elapsed * (1.0 - fraction) / fraction)
    }

    /// Whether it is still running.
    pub(crate) fn is_running(&self) -> bool {
        self.finished.is_none()
    }
}

type Listener = Box<dyn Fn(&JobView, Option<usize>)>;

#[derive(Default)]
struct Hub {
    view: RefCell<Option<JobView>>,
    listeners: RefCell<Vec<(u64, Listener)>>,
    next: Cell<u64>,
}

thread_local! {
    static HUB: Rc<Hub> = Rc::new(Hub::default());
}

fn hub() -> Rc<Hub> {
    HUB.with(Rc::clone)
}

/// The job that runs or ran last, if any.
pub(crate) fn current() -> Option<JobView> {
    hub().view.borrow().clone()
}

/// Calls `listener` on the main loop after each change, with the row that
/// changed (`None` when the job started or ended). Returns its handle.
pub(crate) fn listen(listener: impl Fn(&JobView, Option<usize>) + 'static) -> u64 {
    let hub = hub();
    let id = hub.next.get() + 1;
    hub.next.set(id);
    hub.listeners.borrow_mut().push((id, Box::new(listener)));
    id
}

/// Stops calling the listener `id`.
pub(crate) fn unlisten(id: u64) {
    hub().listeners.borrow_mut().retain(|(i, _)| *i != id);
}

fn notify(row: Option<usize>) {
    let hub = hub();
    let Some(view) = hub.view.borrow().clone() else {
        return;
    };
    // A listener may add or remove listeners; it sees the list as it was.
    let listeners = std::mem::take(&mut *hub.listeners.borrow_mut());
    for (_, listener) in &listeners {
        listener(&view, row);
    }
    let mut now = hub.listeners.borrow_mut();
    let added = std::mem::take(&mut *now);
    *now = listeners;
    now.extend(added);
}

fn apply(event: ConvertEvent) -> Option<Option<usize>> {
    let hub = hub();
    let mut view = hub.view.borrow_mut();
    let view = view.as_mut()?;
    Some(match event {
        ConvertEvent::Started { .. } => None,
        ConvertEvent::Progress { index, fraction } => {
            let row = view.rows.get_mut(index)?;
            if matches!(row, RowState::Waiting | RowState::Working(_)) {
                *row = RowState::Working(fraction);
            }
            Some(index)
        }
        ConvertEvent::Done { index, outcome } => {
            let row = view.rows.get_mut(index)?;
            *row = match outcome {
                ItemOutcome::Written(bytes) => {
                    view.bytes += bytes;
                    RowState::Written(bytes)
                }
                ItemOutcome::Skipped => RowState::Skipped,
                ItemOutcome::Failed(why) => RowState::Failed(why),
            };
            Some(index)
        }
        ConvertEvent::Finished(summary) => {
            for row in &mut view.rows {
                if matches!(row, RowState::Working(_)) {
                    *row = RowState::Waiting;
                }
            }
            view.finished = Some(summary);
            None
        }
    })
}

/// Starts a job of `kind` over `rows` rows: `start` hands the events to the
/// backend, which runs the job on its own threads.
pub(crate) fn start(
    kind: JobKind,
    target: String,
    rows: usize,
    start: impl FnOnce(JobEvents) -> Result<(), BackendError>,
) -> Result<(), BackendError> {
    let (sender, receiver) = async_channel::unbounded::<ConvertEvent>();
    let events: JobEvents = Box::new(move |event| {
        let _ = sender.send_blocking(event);
    });
    start(events)?;
    hub().view.replace(Some(JobView {
        kind,
        target,
        rows: vec![RowState::Waiting; rows],
        bytes: 0,
        started: Instant::now(),
        finished: None,
    }));
    notify(None);
    glib::spawn_future_local(async move {
        while let Ok(event) = receiver.recv().await {
            let finished = matches!(event, ConvertEvent::Finished(_));
            if let Some(row) = apply(event) {
                notify(row);
            }
            if finished {
                break;
            }
        }
    });
    Ok(())
}

/// Forgets a finished job, so that the windows show their plans again.
pub(crate) fn clear() {
    let hub = hub();
    let running = hub.view.borrow().as_ref().is_some_and(JobView::is_running);
    if !running {
        hub.view.replace(None);
    }
}
