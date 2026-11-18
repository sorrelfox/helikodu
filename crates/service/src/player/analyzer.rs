// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The analyzer feed: the spectrum and the channel levels of what the device is
//! given, for a view that shows them.
//!
//! [`Player::watch_analyzer`](super::Player::watch_analyzer) starts a thread that,
//! as often as asked, takes what the engine's [`AudioTap`] copied and measures it
//! with a [`SoundAnalyzer`]. Frames are sent only while sound arrives: after it
//! stops (a pause, the end) one silent frame follows, then nothing until it comes
//! back. While no watch is open the device side copies nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helikodu_engine::analysis::SoundAnalyzer;
use helikodu_engine::output::AudioTap;
use helikodu_model::player::AnalyzerFrame;

/// How long no sound must arrive before the analyzer counts it as stopped: longer
/// than a device period, so a large period does not read as a pause.
const QUIET: Duration = Duration::from_millis(250);
/// How often the thread looks for sound while none arrives.
const IDLE_CHECK: Duration = Duration::from_millis(100);

/// The engine's tap and how many watches want it.
#[derive(Debug, Clone, Default)]
pub(super) struct TapUsers {
    pub(super) tap: AudioTap,
    count: Arc<AtomicUsize>,
}

/// An open analyzer feed; dropping it stops the thread and, with the last one,
/// the copying.
#[derive(Debug)]
pub struct AnalyzerWatch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    users: TapUsers,
}

impl AnalyzerWatch {
    pub(super) fn start(
        users: &TapUsers,
        interval: Duration,
        mut sink: impl FnMut(AnalyzerFrame) -> bool + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let tap = users.tap.clone();
        if users.count.fetch_add(1, Ordering::AcqRel) == 0 {
            tap.discard();
            tap.set_wanted(true);
        }
        let ended = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("helikodu-analyzer".into())
            .spawn(move || {
                let mut analyzer = SoundAnalyzer::new();
                let mut samples = Vec::new();
                let mut heard: Option<Instant> = None;
                while !ended.load(Ordering::Acquire) {
                    thread::park_timeout(if heard.is_some() {
                        interval
                    } else {
                        IDLE_CHECK
                    });
                    if ended.load(Ordering::Acquire) {
                        break;
                    }
                    samples.clear();
                    let format = tap.drain(&mut samples);
                    let frame = match format {
                        Some(format) if !samples.is_empty() => {
                            heard = Some(Instant::now());
                            analyzer.feed(format.rate, format.layout, &samples)
                        }
                        _ => match heard {
                            Some(at) if at.elapsed() >= QUIET => {
                                heard = None;
                                analyzer.reset();
                                analyzer.silent_frame()
                            }
                            _ => continue,
                        },
                    };
                    if !sink(frame) {
                        break;
                    }
                }
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                release(users);
                return Err(error);
            }
        };
        Ok(Self {
            stop,
            thread: Some(thread),
            users: users.clone(),
        })
    }
}

fn release(users: &TapUsers) {
    if users.count.fetch_sub(1, Ordering::AcqRel) == 1 {
        users.tap.set_wanted(false);
    }
}

impl Drop for AnalyzerWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
        release(&self.users);
    }
}
