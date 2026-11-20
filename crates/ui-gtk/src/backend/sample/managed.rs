// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! An invented managed folder: plans with the files of the design's sample (an
//! album with its cover and booklet, a name that is taken, lyrics, a CUE sheet,
//! a rip log, an identical file, a file of the library and one that cannot be
//! read) and enough made-up tracks to fill them, runs that take a moment and can
//! be stopped, and a run cut short that goes on or rolls back. Nothing touches
//! the disk.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use super::super::BackendError;
use super::super::services::{
    CancelToken, CharacterSet, FileProgress, ItemAction, ItemKind, ItemResult, ManagedItem,
    ManagedOperation, ManagedOptions, ManagedOutcome, ManagedPlan, ManagedReport, ManagedRunId,
    ManagedRunSummary, ManagedSource, Settings, SkipReason, Transfer,
};

/// How long the invented run takes for each file.
const PER_FILE: Duration = Duration::from_millis(4);
/// The bytes free on the drive of the managed folder: a big disk, or a small
/// removable drive.
const FREE: u64 = 180_000_000_000;
const FREE_SMALL: u64 = 1_100_000_000;

/// A run of the invented managed folder.
#[derive(Debug, Clone)]
struct SampleRun {
    id: i64,
    plan: ManagedPlan,
    /// How many of the files that move are done.
    done: usize,
    started_at: i64,
}

/// The managed folder's runs, in memory.
#[derive(Debug, Default)]
pub(super) struct SampleManaged {
    next: Mutex<i64>,
    interrupted: Mutex<Option<SampleRun>>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn options(settings: &Settings, transfer: Transfer) -> Result<ManagedOptions, BackendError> {
    let managed = &settings.library.managed;
    let folder = managed
        .folder
        .clone()
        .ok_or_else(|| BackendError("the managed folder is off".into()))?;
    let library_folder = settings
        .library
        .folders
        .iter()
        .filter(|f| folder.starts_with(f))
        .max_by_key(|f| f.as_os_str().len())
        .cloned()
        .unwrap_or_else(|| folder.clone());
    Ok(ManagedOptions {
        folder,
        library_folder,
        pattern: managed.pattern.clone(),
        characters: CharacterSet::from_id(&managed.characters).unwrap_or_default(),
        unknown: "Unknown".into(),
        transfer,
    })
}

fn item(kind: ItemKind, from: PathBuf, to: PathBuf, size: u64, action: ItemAction) -> ManagedItem {
    ManagedItem {
        kind,
        file: None,
        from,
        to,
        size,
        action,
        renamed: false,
        rewritten: false,
    }
}

fn moving(transfer: Transfer) -> ItemAction {
    match transfer {
        Transfer::Copy => ItemAction::Copy,
        Transfer::Move => ItemAction::Move,
    }
}

const ARTISTS: [&str; 6] = [
    "Okto",
    "Sola",
    "Halden",
    "Mari Kask",
    "Nordlys",
    "Eeva Saar",
];
const WORDS: [&str; 12] = [
    "Harbour",
    "Low Tide",
    "Lanterns",
    "Ferry",
    "North",
    "Glass",
    "Shoreline",
    "Kiln",
    "Amber",
    "Winter",
    "Departure",
    "Drift",
];

/// The plan of adding `paths`: the design's rows, then made-up tracks, spread
/// over the sources.
fn import_plan(paths: &[PathBuf], options: &ManagedOptions) -> Vec<ManagedItem> {
    let first = paths.first().cloned().unwrap_or_default();
    let last = paths.last().cloned().unwrap_or_default();
    let middle = paths.get(1).cloned().unwrap_or_else(|| first.clone());
    let into = |rel: &str| options.folder.join(rel);
    let transfer = moving(options.transfer);
    let mut items = vec![
        item(
            ItemKind::Audio,
            first.join("Okto - Night Ferry (Deluxe)/Okto - Night Ferry - 01 Departure.flac"),
            into("Okto/Night Ferry (Deluxe)/01 Departure.flac"),
            21_000_000,
            transfer.clone(),
        ),
        item(
            ItemKind::Cover,
            first.join("Okto - Night Ferry (Deluxe)/cover.jpg"),
            into("Okto/Night Ferry (Deluxe)/cover.jpg"),
            1_400_000,
            transfer.clone(),
        ),
        item(
            ItemKind::Booklet,
            first.join("Okto - Night Ferry (Deluxe)/Night Ferry booklet.pdf"),
            into("Okto/Night Ferry (Deluxe)/Night Ferry booklet.pdf"),
            8_200_000,
            transfer.clone(),
        ),
        ManagedItem {
            renamed: true,
            ..item(
                ItemKind::Audio,
                first.join("Sola - Amber/01 Amber.flac"),
                into("Sola/Amber/01 Amber (2).flac"),
                31_000_000,
                transfer.clone(),
            )
        },
        ManagedItem {
            renamed: true,
            ..item(
                ItemKind::Lyrics,
                first.join("Sola - Amber/01 Amber.lrc"),
                into("Sola/Amber/01 Amber (2).lrc"),
                2_000,
                transfer.clone(),
            )
        },
        ManagedItem {
            rewritten: true,
            ..item(
                ItemKind::CueSheet,
                first.join("Halden - Kiln (live)/Kiln.cue"),
                into("Halden/Kiln (Live)/Kiln.cue"),
                1_000,
                transfer.clone(),
            )
        },
        item(
            ItemKind::Log,
            first.join("Halden - Kiln (live)/Kiln.log"),
            into("Halden/Kiln (Live)/Kiln.log"),
            6_000,
            transfer.clone(),
        ),
        item(
            ItemKind::Audio,
            first.join("Mari Kask - Tidewater Rooms/03 Shoreline 6am.flac"),
            into("Mari Kask/Tidewater Rooms/03 Shoreline 6am.flac"),
            101_000_000,
            ItemAction::Skip(SkipReason::Identical),
        ),
        item(
            ItemKind::Audio,
            last.join("Juno Park - Low Sun.flac"),
            PathBuf::new(),
            34_000_000,
            ItemAction::Skip(SkipReason::InLibrary),
        ),
        item(
            ItemKind::Audio,
            middle.join("Okto/demos/untitled 4.wav"),
            PathBuf::new(),
            0,
            ItemAction::Skip(SkipReason::Unreadable("the file is empty".into())),
        ),
    ];
    // Made-up albums to reach the design's figures: 214 to move, 3 with a
    // number, 9 already there and 4 left where they are.
    let mut n = 0;
    while items.iter().filter(|i| i.transfers()).count() < 214 {
        let artist = ARTISTS[n % ARTISTS.len()];
        let album = WORDS[(n / 12) % WORDS.len()];
        let title = WORDS[n % WORDS.len()];
        let number = n % 12 + 1;
        let source = &paths[n % paths.len().max(1)];
        let from = source.join(format!("{artist} - {album}/{number:02} {title}.flac"));
        let to = into(&format!("{artist}/{album}/{number:02} {title}.flac"));
        let mut planned = item(ItemKind::Audio, from, to, 11_300_000, transfer.clone());
        if n == 40 {
            planned.renamed = true;
            planned.to = into(&format!("{artist}/{album}/{number:02} {title} (2).flac"));
        }
        if n % 25 == 7 && items.iter().filter(|i| is_identical(i)).count() < 9 {
            planned.action = ItemAction::Skip(SkipReason::Identical);
        } else if n % 90 == 89 {
            planned.action = ItemAction::Skip(SkipReason::InManagedFolder);
            planned.to = PathBuf::new();
        }
        items.push(planned);
        n += 1;
    }
    items
}

fn is_identical(item: &ManagedItem) -> bool {
    item.action == ItemAction::Skip(SkipReason::Identical)
}

/// The plan of gathering the library: the files of `~/Music/Lossless` and a
/// few from elsewhere in the music folder.
fn gather_plan(options: &ManagedOptions, music: &Path) -> Vec<ManagedItem> {
    let transfer = moving(options.transfer);
    let into = |rel: &str| options.folder.join(rel);
    let lossless = music.join("Lossless");
    let mut items = Vec::new();
    let named = [
        (
            "Nordlys/Winter Light/01.dsf",
            "Nordlys Ensemble/Winter Light/01 Frost Fair.dsf",
            412_000_000_u64,
        ),
        (
            "Nordlys/Winter Light/02.dsf",
            "Nordlys Ensemble/Winter Light/02 Candlemas.dsf",
            388_000_000,
        ),
        (
            "Nordlys/Winter Light/folder.jpg",
            "Nordlys Ensemble/Winter Light/folder.jpg",
            2_100_000,
        ),
        (
            "Nordlys/Winter Light/03.dsf",
            "Nordlys Ensemble/Winter Light/03 First Snow.dsf",
            503_000_000,
        ),
        (
            "Nordlys/Winter Light/04.dsf",
            "Nordlys Ensemble/Winter Light/04 Thaw.dsf",
            329_000_000,
        ),
        (
            "Okto/Night Ferry/01.flac",
            "Okto/Night Ferry/01 Departure.flac",
            21_000_000,
        ),
        (
            "Okto/Night Ferry/02.flac",
            "Okto/Night Ferry/02 Night Ferry (2).flac",
            41_000_000,
        ),
        (
            "Okto/Night Ferry/Night Ferry.cue",
            "Okto/Night Ferry/Night Ferry.cue",
            1_000,
        ),
    ];
    for (i, (from, to, size)) in named.into_iter().enumerate() {
        let kind = if from.ends_with(".jpg") {
            ItemKind::Cover
        } else if from.ends_with(".cue") {
            ItemKind::CueSheet
        } else {
            ItemKind::Audio
        };
        let mut planned = item(kind, lossless.join(from), into(to), size, transfer.clone());
        planned.renamed = i == 6;
        planned.rewritten = kind == ItemKind::CueSheet;
        items.push(planned);
    }
    items.push(item(
        ItemKind::Audio,
        lossless.join("Eeva Saar/Goldberg Variations/01 Aria.flac"),
        into("Eeva Saar/Goldberg Variations/01 Aria.flac"),
        96_000_000,
        ItemAction::Skip(SkipReason::Identical),
    ));
    let mut n = 0;
    while items.iter().filter(|i| i.transfers()).count() < 1098 {
        let artist = ARTISTS[n % ARTISTS.len()];
        let album = WORDS[(n / 12) % WORDS.len()];
        let number = n % 12 + 1;
        let title = WORDS[n % WORDS.len()];
        let mut planned = item(
            ItemKind::Audio,
            lossless.join(format!("{artist}/{album}/{number:02}.flac")),
            into(&format!("{artist}/{album}/{number:02} {title}.flac")),
            33_000_000,
            transfer.clone(),
        );
        if n == 300 {
            planned.renamed = true;
        }
        if n % 50 == 11 && items.iter().filter(|i| is_identical(i)).count() < 22 {
            planned.action = ItemAction::Skip(SkipReason::Identical);
        }
        planned.file = Some(super::super::services::FileId::from_raw(
            i64::try_from(n).unwrap_or(0) + 1,
        ));
        items.push(planned);
        n += 1;
    }
    items
}

/// Whether the managed folder is on a small removable drive in the sample.
fn free_bytes(folder: &Path) -> u64 {
    if folder.starts_with("/run/media") {
        FREE_SMALL
    } else {
        FREE
    }
}

fn result_of(item: &ManagedItem, done: bool) -> ItemResult {
    match &item.action {
        ItemAction::Skip(reason) => ItemResult::Skipped(reason.clone()),
        _ if !done => ItemResult::Waiting,
        ItemAction::Move => ItemResult::Moved,
        _ => ItemResult::Copied,
    }
}

impl SampleManaged {
    /// The library files outside the managed folder of `settings`.
    pub(super) fn outside(settings: &Settings, music: &Path) -> usize {
        match settings.library.managed.folder.as_deref() {
            Some(folder) if music.starts_with(folder) => 0,
            Some(_) => 1120,
            None => 0,
        }
    }

    pub(super) fn plan(
        settings: &Settings,
        source: &ManagedSource,
        transfer: Transfer,
        music: &Path,
        cancel: &CancelToken,
    ) -> Result<ManagedPlan, BackendError> {
        let options = options(settings, transfer)?;
        // Reading the files takes a moment, as the scan workers do.
        std::thread::sleep(Duration::from_millis(60));
        if cancel.is_cancelled() {
            return Err(BackendError("stopped".into()));
        }
        let (operation, items) = match source {
            ManagedSource::Add(paths) if paths.is_empty() => (ManagedOperation::Import, Vec::new()),
            ManagedSource::Add(paths) => (ManagedOperation::Import, import_plan(paths, &options)),
            ManagedSource::Gather => (ManagedOperation::Consolidate, gather_plan(&options, music)),
        };
        let needed = items
            .iter()
            .filter(|i| match i.action {
                ItemAction::Copy | ItemAction::CopyKeepingSource => true,
                ItemAction::Move => i.rewritten,
                ItemAction::Skip(_) => false,
            })
            .map(|i| i.size)
            .sum();
        let free = free_bytes(&options.folder);
        Ok(ManagedPlan::from_parts(
            operation,
            options,
            items,
            needed,
            Some(free),
        ))
    }

    fn interrupted_run(&self) -> std::sync::MutexGuard<'_, Option<SampleRun>> {
        self.interrupted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Places the files of `run` that wait, one after the other.
    fn drive(
        &self,
        mut run: SampleRun,
        progress: FileProgress<'_>,
        cancel: &CancelToken,
    ) -> ManagedReport {
        let total = run.plan.to_transfer();
        let mut stopped = false;
        let mut done_now = 0;
        while run.done < total {
            if cancel.is_cancelled() {
                stopped = true;
                break;
            }
            std::thread::sleep(PER_FILE);
            run.done += 1;
            done_now += 1;
            progress(done_now, total - (run.done - done_now));
        }
        let mut transferring = 0;
        let outcomes = run
            .plan
            .items
            .iter()
            .map(|item| {
                let done = item.transfers() && {
                    transferring += 1;
                    transferring <= run.done
                };
                ManagedOutcome {
                    kind: item.kind,
                    file: item.file,
                    from: item.from.clone(),
                    to: item.to.clone(),
                    result: result_of(item, done),
                }
            })
            .collect();
        let id = run.id;
        *self.interrupted_run() = stopped.then_some(run);
        ManagedReport {
            run: Some(ManagedRunId::from_raw(id)),
            outcomes,
            finished: !stopped,
            library_error: None,
        }
    }

    pub(super) fn apply(
        &self,
        plan: &ManagedPlan,
        progress: FileProgress<'_>,
        cancel: &CancelToken,
    ) -> Result<ManagedReport, BackendError> {
        if self.interrupted_run().is_some() {
            return Err(BackendError(
                "an earlier run of the managed folder was cut short; continue it or roll it back first"
                    .into(),
            ));
        }
        if !plan.fits() {
            return Err(BackendError("the files do not fit".into()));
        }
        if plan.to_transfer() == 0 {
            return Ok(ManagedReport {
                finished: true,
                ..ManagedReport::default()
            });
        }
        let id = {
            let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
            *next += 1;
            *next
        };
        let run = SampleRun {
            id,
            plan: plan.clone(),
            done: 0,
            started_at: now(),
        };
        Ok(self.drive(run, progress, cancel))
    }

    pub(super) fn interrupted(&self) -> Option<ManagedRunSummary> {
        let run = self.interrupted_run().clone()?;
        let moving: Vec<&ManagedItem> = run.plan.items.iter().filter(|i| i.transfers()).collect();
        let (done, waiting) = moving.split_at(run.done.min(moving.len()));
        Some(ManagedRunSummary {
            run: ManagedRunId::from_raw(run.id),
            operation: run.plan.operation,
            transfer: run.plan.options.transfer,
            started_at: run.started_at,
            folder: run.plan.options.folder.clone(),
            done: done.len(),
            waiting: waiting.len(),
            skipped: run.plan.skipped(),
            done_bytes: done.iter().map(|i| i.size).sum(),
            waiting_bytes: waiting.iter().map(|i| i.size).sum(),
        })
    }

    fn take(&self, id: ManagedRunId) -> Result<SampleRun, BackendError> {
        let mut interrupted = self.interrupted_run();
        match interrupted.take() {
            Some(run) if run.id == id.get() => Ok(run),
            other => {
                *interrupted = other;
                Err(BackendError("that run is not waiting to go on".into()))
            }
        }
    }

    pub(super) fn resume(
        &self,
        id: ManagedRunId,
        progress: FileProgress<'_>,
        cancel: &CancelToken,
    ) -> Result<ManagedReport, BackendError> {
        let run = self.take(id)?;
        Ok(self.drive(run, progress, cancel))
    }

    pub(super) fn roll_back(
        &self,
        id: ManagedRunId,
        progress: FileProgress<'_>,
    ) -> Result<ManagedReport, BackendError> {
        let run = self.take(id)?;
        let mut transferring = 0;
        let outcomes = run
            .plan
            .items
            .iter()
            .map(|item| {
                let done = item.transfers() && {
                    transferring += 1;
                    transferring <= run.done
                };
                ManagedOutcome {
                    kind: item.kind,
                    file: item.file,
                    from: item.from.clone(),
                    to: item.to.clone(),
                    result: if done || item.transfers() {
                        ItemResult::RolledBack
                    } else {
                        result_of(item, false)
                    },
                }
            })
            .collect();
        progress(run.done, run.done);
        Ok(ManagedReport {
            run: Some(id),
            outcomes,
            finished: true,
            library_error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        let mut settings = Settings::default();
        let music = PathBuf::from("/home/someone/Music");
        settings.library.folders = vec![music.clone()];
        settings.library.managed.folder = Some(music.join("Library"));
        settings
    }

    #[test]
    fn the_plans_have_the_figures_of_the_design() {
        let music = PathBuf::from("/home/someone/Music");
        let sources = vec![
            PathBuf::from("/home/someone/Downloads/bandcamp"),
            PathBuf::from("/run/media/user/USB-STICK"),
            PathBuf::from("/home/someone/Music/Incoming/new"),
        ];
        let plan = SampleManaged::plan(
            &settings(),
            &ManagedSource::Add(sources),
            Transfer::Copy,
            &music,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(plan.to_transfer(), 214);
        assert_eq!(plan.renamed(), 3);
        assert_eq!(plan.items.iter().filter(|i| is_identical(i)).count(), 9);
        assert_eq!(plan.skipped() - 9, 4);
        assert!(plan.fits());
        let gather = SampleManaged::plan(
            &settings(),
            &ManagedSource::Gather,
            Transfer::Copy,
            &music,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(gather.to_transfer(), 1098);
        assert_eq!(gather.items.iter().filter(|i| is_identical(i)).count(), 22);
        assert_eq!(SampleManaged::outside(&settings(), &music), 1120);
    }

    #[test]
    fn a_run_stopped_goes_on_or_rolls_back() {
        let music = PathBuf::from("/home/someone/Music");
        let managed = SampleManaged::default();
        let plan = SampleManaged::plan(
            &settings(),
            &ManagedSource::Add(vec![PathBuf::from("/tmp/in")]),
            Transfer::Move,
            &music,
            &CancelToken::new(),
        )
        .unwrap();
        let cancel = CancelToken::new();
        let report = managed
            .apply(
                &plan,
                &|done, _| {
                    if done == 5 {
                        cancel.cancel();
                    }
                },
                &cancel,
            )
            .unwrap();
        assert!(!report.finished);
        let waiting = managed.interrupted().unwrap();
        assert_eq!(waiting.done, 5);
        assert_eq!(waiting.waiting, 209);
        assert!(
            managed
                .apply(&plan, &|_, _| {}, &CancelToken::new())
                .is_err()
        );
        let report = managed
            .resume(waiting.run, &|_, _| {}, &CancelToken::new())
            .unwrap();
        assert!(report.finished);
        assert_eq!(report.transferred(), 214);
        assert!(managed.interrupted().is_none());
    }
}
