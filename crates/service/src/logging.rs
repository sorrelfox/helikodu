// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The log: what the program records about itself, for a bug report.
//!
//! Everything logs through the `log` facade. [`install`] makes [`Logger`] the
//! program's logger; the settings choose how much it keeps ([`LogDetail`]):
//!
//! - **No logs** (the default of release builds): nothing is written to disk, and
//!   the log folder is not even created. Warnings and errors are still kept in a
//!   small ring in memory for a crash report, and shown on the terminal when the
//!   program runs in one.
//! - **Normal detail:** warnings and the main events go to
//!   `$XDG_STATE_HOME/helikodu/logs/helikodu.log`.
//! - **Full detail:** everything Helikodu says, and the debug messages of the
//!   libraries it uses.
//!
//! The files stay within the size limit of Settings › Library › Storage: the
//! current file is renamed to `helikodu.log.1` (and the older ones moved up) when
//! it reaches a quarter of the limit, and only three old files are kept. Nothing
//! is ever sent anywhere.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

pub use helikodu_model::settings::LogDetail;
use log::{Level, LevelFilter, Log, Metadata, Record};

/// The log folder inside the state folder.
pub const LOG_FOLDER: &str = "logs";
/// The current log file.
pub const LOG_FILE: &str = "helikodu.log";
/// How many older files are kept beside the current one.
const OLD_FILES: usize = 3;
/// How many lines the ring in memory keeps.
const RING_LINES: usize = 200;

/// How the log is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogConfig {
    /// The folder of the log files.
    pub folder: PathBuf,
    /// How much is recorded.
    pub detail: LogDetail,
    /// The most bytes all log files may take together.
    pub limit: u64,
    /// Whether warnings also go to the standard error stream (when it is a
    /// terminal).
    pub terminal: bool,
}

impl LogConfig {
    /// The log in `<state>/logs` with `detail`, limited to `limit_mib` MiB.
    pub fn new(state: &Path, detail: LogDetail, limit_mib: u32) -> Self {
        Self {
            folder: state.join(LOG_FOLDER),
            detail,
            limit: u64::from(limit_mib.max(1)) << 20,
            terminal: io::stderr().is_terminal(),
        }
    }
}

/// The logger: a file with rotation, a ring in memory, and the terminal.
#[derive(Debug)]
pub struct Logger {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    config: LogConfig,
    file: Option<File>,
    written: u64,
    ring: VecDeque<String>,
}

impl Logger {
    /// A logger kept as `config` says. Nothing is written until a message comes.
    pub fn new(config: LogConfig) -> Self {
        Self {
            state: Mutex::new(State {
                config,
                file: None,
                written: 0,
                ring: VecDeque::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes how much is recorded and the size limit. With
    /// [`LogDetail::Off`] the file is closed and nothing more is written.
    pub fn configure(&self, detail: LogDetail, limit: u64) {
        let mut state = self.lock();
        if state.config.detail != detail {
            state.file = None;
        }
        state.config.detail = detail;
        state.config.limit = limit.max(1 << 20);
        log::set_max_level(max_level(detail));
    }

    /// The current detail.
    pub fn detail(&self) -> LogDetail {
        self.lock().config.detail
    }

    /// The log folder.
    pub fn folder(&self) -> PathBuf {
        self.lock().config.folder.clone()
    }

    /// The newest `count` lines, oldest first, from memory.
    pub fn recent(&self, count: usize) -> Vec<String> {
        let state = self.lock();
        let skip = state.ring.len().saturating_sub(count);
        state.ring.iter().skip(skip).cloned().collect()
    }

    /// Whether a message at `level` from `target` is recorded at `detail`.
    fn wanted(detail: LogDetail, level: Level, target: &str) -> bool {
        let ours = target.starts_with("helikodu");
        match detail {
            LogDetail::Off => level <= Level::Warn,
            LogDetail::Normal => level <= Level::Info && (ours || level <= Level::Warn),
            LogDetail::Full => ours || level <= Level::Debug,
        }
    }

    fn write_line(&self, level: Level, line: &str) {
        let mut state = self.lock();
        if level <= Level::Warn {
            if state.ring.len() == RING_LINES {
                state.ring.pop_front();
            }
            state.ring.push_back(line.to_owned());
            if state.config.terminal {
                eprintln!("{line}");
            }
        }
        if state.config.detail == LogDetail::Off {
            return;
        }
        // A log that cannot be written is given up quietly: logging must
        // never stop the program.
        let _ = state.append(line);
    }
}

impl State {
    fn append(&mut self, line: &str) -> io::Result<()> {
        let bytes = line.len() as u64 + 1;
        if self.file.is_some() && self.written + bytes > self.config.limit / (OLD_FILES as u64 + 1)
        {
            self.file = None;
            self.rotate()?;
        }
        if self.file.is_none() {
            fs::create_dir_all(&self.config.folder)?;
            let path = self.config.folder.join(LOG_FILE);
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            self.written = file.metadata()?.len();
            self.file = Some(file);
            if self.written + bytes > self.config.limit / (OLD_FILES as u64 + 1) {
                self.file = None;
                self.rotate()?;
                return self.append(line);
            }
        }
        if let Some(file) = &mut self.file {
            writeln!(file, "{line}")?;
            self.written += bytes;
        }
        Ok(())
    }

    /// `helikodu.log` becomes `helikodu.log.1`, and each older file moves one
    /// up; the oldest goes.
    fn rotate(&mut self) -> io::Result<()> {
        let folder = &self.config.folder;
        let name = |n: usize| {
            if n == 0 {
                folder.join(LOG_FILE)
            } else {
                folder.join(format!("{LOG_FILE}.{n}"))
            }
        };
        let _ = fs::remove_file(name(OLD_FILES));
        for n in (0..OLD_FILES).rev() {
            match fs::rename(name(n), name(n + 1)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        self.written = 0;
        Ok(())
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        Self::wanted(self.detail(), metadata.level(), metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut line = format!(
            "{} {:<5} {}: ",
            utc_time(SystemTime::now()),
            record.level(),
            record.target()
        );
        let _ = write!(line, "{}", record.args());
        self.write_line(record.level(), &line);
    }

    fn flush(&self) {
        if let Some(file) = &mut self.lock().file {
            let _ = file.flush();
        }
    }
}

fn max_level(detail: LogDetail) -> LevelFilter {
    match detail {
        LogDetail::Off => LevelFilter::Warn,
        LogDetail::Normal => LevelFilter::Info,
        LogDetail::Full => LevelFilter::Trace,
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Makes a [`Logger`] kept as `config` says the program's logger. Only the
/// first call has an effect.
pub fn install(config: LogConfig) {
    let detail = config.detail;
    let mut fresh = false;
    let logger = LOGGER.get_or_init(|| {
        fresh = true;
        Logger::new(config)
    });
    if fresh && log::set_logger(logger).is_ok() {
        log::set_max_level(max_level(detail));
        log::info!(
            target: "helikodu",
            "Helikodu {} started; log detail {detail:?}",
            env!("CARGO_PKG_VERSION")
        );
    }
}

/// The installed logger, if any.
pub fn logger() -> Option<&'static Logger> {
    LOGGER.get()
}

/// Changes the installed logger, if there is one.
pub fn configure(detail: LogDetail, limit_mib: u32) {
    if let Some(logger) = logger() {
        logger.configure(detail, u64::from(limit_mib.max(1)) << 20);
    }
}

/// The bytes the files in the log folder take.
pub fn usage(folder: &Path) -> Option<u64> {
    let entries = fs::read_dir(folder).ok()?;
    Some(
        entries
            .filter_map(Result::ok)
            .filter_map(|e| e.metadata().ok())
            .filter(fs::Metadata::is_file)
            .map(|m| m.len())
            .sum(),
    )
}

/// Removes the log files. The current file starts again with the next message.
pub fn clear(folder: &Path) -> io::Result<()> {
    if let Some(logger) = logger()
        && logger.folder() == folder
    {
        let mut state = logger.lock();
        state.file = None;
        state.written = 0;
    }
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.filter_map(Result::ok) {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(LOG_FILE))
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// `at` as `YYYY-MM-DDThh:mm:ss.mmmZ`.
pub(crate) fn utc_time(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = i64::try_from(since.as_secs()).unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let day = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        day / 3600,
        day % 3600 / 60,
        day % 60,
        since.subsec_millis()
    )
}

/// The date of a day since 1970-01-01 (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helikodu-logging-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn logger(folder: &Path, detail: LogDetail) -> Logger {
        Logger::new(LogConfig {
            folder: folder.to_path_buf(),
            detail,
            limit: 1 << 20,
            terminal: false,
        })
    }

    fn say(logger: &Logger, level: Level, target: &str, text: &str) {
        logger.log(
            &Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{text}"))
                .build(),
        );
    }

    #[test]
    fn no_logs_writes_nothing_at_all() {
        let folder = scratch("off");
        let logger = logger(&folder, LogDetail::Off);
        for level in [Level::Error, Level::Warn, Level::Info, Level::Debug] {
            say(&logger, level, "helikodu_service", "something");
        }
        assert!(!folder.exists(), "the log folder was created");
        // The warnings are kept in memory for a crash report.
        assert_eq!(logger.recent(10).len(), 2);
    }

    #[test]
    fn the_detail_decides_what_is_kept() {
        let folder = scratch("detail");
        let logger = logger(&folder, LogDetail::Normal);
        say(&logger, Level::Info, "helikodu_service", "kept");
        say(&logger, Level::Debug, "helikodu_service", "too detailed");
        say(&logger, Level::Info, "zbus", "not ours");
        logger.configure(LogDetail::Full, 1 << 20);
        say(&logger, Level::Trace, "helikodu_engine", "full");
        say(&logger, Level::Trace, "zbus", "library trace");
        say(&logger, Level::Debug, "zbus", "library debug");
        logger.configure(LogDetail::Off, 1 << 20);
        say(&logger, Level::Error, "helikodu_service", "after off");
        let text = fs::read_to_string(folder.join(LOG_FILE)).unwrap();
        assert!(text.contains("kept"));
        assert!(!text.contains("too detailed"));
        assert!(!text.contains("not ours"));
        assert!(text.contains("full"));
        assert!(!text.contains("library trace"));
        assert!(text.contains("library debug"));
        assert!(!text.contains("after off"));
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn the_files_stay_within_the_limit() {
        let folder = scratch("rotate");
        let logger = logger(&folder, LogDetail::Normal);
        let line = "x".repeat(1000);
        for _ in 0..5000 {
            say(&logger, Level::Warn, "helikodu_service", &line);
        }
        let total = usage(&folder).unwrap();
        assert!(total <= 1 << 20, "{total} bytes");
        assert!(total > 700 << 10, "{total} bytes");
        let names: Vec<String> = fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), OLD_FILES + 1, "{names:?}");
        clear(&folder).unwrap();
        assert_eq!(usage(&folder), Some(0));
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn times_are_utc() {
        let at = UNIX_EPOCH + Duration::from_millis(1_795_000_000_123);
        assert_eq!(utc_time(at), "2026-11-18T11:06:40.123Z");
    }
}
