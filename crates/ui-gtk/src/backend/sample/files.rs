// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Invented files for the file tools: the organizer's files with their tags,
//! moved and moved back in memory only; missing files and where they turn up;
//! and the results of a check of files. Nothing touches the disk.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use helikodu_model::library::TrackId;
use helikodu_model::query::Field;

use super::super::services::{
    Change, Destination, FileId, FileRunId, FileRunSummary, FoundFile, MissingFile, MoveOutcome,
    MoveProblem, MoveRequest, NamingPattern, OrganizeOptions, PlannedMove, RelinkOutcome, RunKind,
    RunReport, SavedPattern, VerifiedFormat, VerifyProblem, VerifyReport,
};
use super::super::{BackendError, TrackKey};

/// One invented file of the organizer.
#[derive(Debug, Clone)]
struct SampleFile {
    id: i64,
    path: PathBuf,
    albumartist: &'static str,
    artist: &'static str,
    album: &'static str,
    year: u32,
    track: u32,
    disc: Option<u32>,
    title: &'static str,
    composer: Option<&'static str>,
}

/// The newest run, kept in memory.
#[derive(Debug, Clone)]
struct SampleRun {
    id: i64,
    pattern: Option<String>,
    moves: Vec<(i64, PathBuf, PathBuf)>,
    undone: bool,
}

#[derive(Debug)]
struct State {
    files: Vec<SampleFile>,
    taken: HashSet<PathBuf>,
    patterns: Vec<SavedPattern>,
    run: Option<SampleRun>,
    next_run: i64,
    missing: Vec<MissingFile>,
}

/// The invented files. See the module documentation.
#[derive(Debug)]
pub(super) struct SampleFiles {
    music: PathBuf,
    state: Mutex<State>,
}

fn file(
    id: i64,
    path: PathBuf,
    (albumartist, album, year): (&'static str, &'static str, u32),
    (track, disc, title): (u32, Option<u32>, &'static str),
) -> SampleFile {
    SampleFile {
        id,
        path,
        albumartist,
        artist: albumartist,
        album,
        year,
        track,
        disc,
        title,
        composer: None,
    }
}

fn missing(
    id: i64,
    path: PathBuf,
    root: &Path,
    (title, artist): (&str, &str),
    days: u64,
) -> MissingFile {
    let since = SystemTime::now()
        .checked_sub(Duration::from_secs(days * 86_400))
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    MissingFile {
        file: FileId::from_raw(id),
        track: Some(TrackId::new(u64::try_from(id).unwrap_or(1))),
        title: Some(title.into()),
        artist: Some(artist.into()),
        path,
        root: root.to_path_buf(),
        missing_since: since,
        tracks: 1,
    }
}

/// The folder the sample's missing files turn up in.
pub(super) fn sorted_folder(music: &Path) -> PathBuf {
    music.join("Sorted")
}

/// The second library folder of the sample, on a drive.
fn archive() -> PathBuf {
    PathBuf::from("/mnt/archive/Lossless")
}

impl SampleFiles {
    pub(super) fn new(music: PathBuf) -> Self {
        let tide = ("Mari Kask", "Tidewater Rooms", 2021);
        let incoming = music.join("Incoming/Mari Kask - Tidewater Rooms (2021) [24-96]");
        let ferry = ("Okto", "Night Ferry", 2020);
        let letters = ("Mari Kask", "Letters from Hiiumaa", 2018);
        let files = vec![
            file(
                901,
                incoming.join("01 Tidewater.flac"),
                tide,
                (1, None, "Tidewater"),
            ),
            file(
                902,
                incoming.join("02 Glass Harbour.flac"),
                tide,
                (2, None, "Glass Harbour"),
            ),
            file(
                903,
                incoming.join("03 Shoreline 6am.flac"),
                tide,
                (3, None, "Shoreline, 6 a.m."),
            ),
            file(
                904,
                music.join("Okto/Night Ferry/02 - night ferry.flac"),
                ferry,
                (2, None, "Night Ferry"),
            ),
            file(
                905,
                music.join("Okto/Night Ferry/03 - car deck.flac"),
                ferry,
                (3, None, "Car Deck"),
            ),
            SampleFile {
                composer: Some("Hildegard von Bingen"),
                ..file(
                    906,
                    music.join("Nordlys Ensemble/Northern Hours/CD2/01 Compline.dsf"),
                    ("Nordlys Ensemble", "Northern Hours", 2020),
                    (1, Some(2), "Compline"),
                )
            },
            file(
                907,
                music.join("Incoming/unsorted/track07.mp3"),
                ("Halden", "Static Bloom", 2021),
                (7, None, "Kiln"),
            ),
            file(
                908,
                music.join("Mari Kask/2018 Letters from Hiiumaa/01 First Letter.flac"),
                letters,
                (1, None, "First Letter"),
            ),
            file(
                909,
                music.join("Mari Kask/2018 Letters from Hiiumaa/02 Kõpu Light.flac"),
                letters,
                (2, None, "Kõpu Light"),
            ),
        ];
        let taken = [music.join("Halden/2021 Static Bloom/07 Kiln.mp3")]
            .into_iter()
            .collect();
        let patterns = [
            (
                "Artist, album, track",
                "{albumartist}/{album}/[{disc}-]{track} {title}",
            ),
            (
                "With year and disc",
                "{albumartist}/{year} {album}/[{disc}-]{track} {title}",
            ),
            ("Classical", "{composer}/{album}/[{disc}-]{track} {title}"),
            (
                "One folder for everything",
                "{artist} - {album} - {track} {title}",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (name, pattern))| SavedPattern {
            id: i64::try_from(i).unwrap_or(0) + 1,
            name: name.into(),
            pattern: pattern.into(),
        })
        .collect();
        let lossless = archive();
        let mut lost = Vec::new();
        for (i, (folder, name, title, artist)) in [
            ("Okto/Night Ferry", "06.flac", "Open Water", "Okto"),
            ("Okto/Night Ferry", "07.flac", "Wake", "Okto"),
            ("Okto/Night Ferry", "08.flac", "Harbour Wall", "Okto"),
            ("Okto/Night Ferry", "09.flac", "Arrival", "Okto"),
            (
                "Nordlys/Winter Light",
                "01.dsf",
                "Frost Fair",
                "Nordlys Ensemble",
            ),
            (
                "Nordlys/Winter Light",
                "02.dsf",
                "Candlemas",
                "Nordlys Ensemble",
            ),
            (
                "Nordlys/Winter Light",
                "03.dsf",
                "First Snow",
                "Nordlys Ensemble",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            lost.push(missing(
                950 + i64::try_from(i).unwrap_or(0),
                lossless.join(folder).join(name),
                &lossless,
                (title, artist),
                3,
            ));
        }
        for (id, path, title, artist, days) in [
            (
                960,
                "Mari Kask/2018 Letters from Hiiumaa/07.flac",
                "Ferry at Dusk",
                "Mari Kask",
                9,
            ),
            (
                961,
                "Incoming/Juno Park - Second Light/02.flac",
                "Second Light",
                "Juno Park",
                12,
            ),
            (
                962,
                "Incoming/Juno Park - Second Light/03.flac",
                "Paper Boat",
                "Juno Park",
                12,
            ),
            (963, "Halden/demos/march.wav", "Demo, March", "Halden", 27),
        ] {
            lost.push(missing(id, music.join(path), &music, (title, artist), days));
        }
        Self {
            music,
            state: Mutex::new(State {
                files,
                taken,
                patterns,
                run: None,
                next_run: 1,
                missing: lost,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn plan(&self, options: &OrganizeOptions) -> Result<Vec<PlannedMove>, BackendError> {
        let pattern =
            NamingPattern::parse(&options.pattern).map_err(|e| BackendError(e.to_string()))?;
        let state = self.lock();
        let base = match &options.into {
            Destination::Folder(folder) => folder.clone(),
            _ => self.music.clone(),
        };
        let mut claimed = HashSet::new();
        let mut planned = Vec::new();
        for f in &state.files {
            let value = |field: &Field| -> Option<String> {
                Some(match field {
                    Field::AlbumArtist => f.albumartist.into(),
                    Field::Artist => f.artist.into(),
                    Field::Album => f.album.into(),
                    Field::Year => f.year.to_string(),
                    Field::Track => format!("{:02}", f.track),
                    Field::Disc => f.disc?.to_string(),
                    Field::Title => f.title.into(),
                    Field::Composer => f.composer?.into(),
                    _ => return None,
                })
            };
            let ext = f
                .path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let names = pattern.render(&value, options.characters, &options.unknown, &ext);
            let mut to = base.clone();
            let count = names.len();
            for (i, name) in names.into_iter().enumerate() {
                to.push(if i + 1 == count {
                    format!("{name}{ext}")
                } else {
                    name
                });
            }
            let change = if to == f.path {
                Change::InPlace
            } else if state.taken.contains(&to) || !claimed.insert(to.clone()) {
                Change::NameTaken
            } else if to.parent() == f.path.parent() {
                Change::Rename
            } else if to.file_name() == f.path.file_name() {
                Change::Move
            } else {
                Change::MoveAndRename
            };
            planned.push(PlannedMove {
                file: FileId::from_raw(f.id),
                track: TrackId::new(u64::try_from(f.id).unwrap_or(1)),
                from: f.path.clone(),
                to,
                change,
            });
        }
        Ok(planned)
    }

    pub(super) fn organize(&self, moves: &[MoveRequest], pattern: Option<&str>) -> RunReport {
        let mut state = self.lock();
        let mut outcomes = Vec::new();
        let mut done = Vec::new();
        for request in moves {
            let found = state
                .files
                .iter_mut()
                .find(|f| f.id == request.file.get() && f.path == request.from);
            let problem = match found {
                Some(f) => {
                    f.path.clone_from(&request.to);
                    done.push((f.id, request.from.clone(), request.to.clone()));
                    None
                }
                None => Some(MoveProblem::Changed),
            };
            outcomes.push(MoveOutcome {
                file: Some(request.file),
                from: request.from.clone(),
                to: request.to.clone(),
                problem,
            });
        }
        let run = (!done.is_empty()).then(|| {
            let id = state.next_run;
            state.next_run += 1;
            state.run = Some(SampleRun {
                id,
                pattern: pattern.map(str::to_owned),
                moves: done,
                undone: false,
            });
            FileRunId::from_raw(id)
        });
        RunReport { run, outcomes }
    }

    pub(super) fn last_run(&self) -> Option<FileRunSummary> {
        let state = self.lock();
        let run = state.run.as_ref()?;
        let n = run.moves.len();
        Some(FileRunSummary {
            run: FileRunId::from_raw(run.id),
            kind: if run.pattern.is_some() {
                RunKind::Organize
            } else {
                RunKind::Rename
            },
            started_at: 0,
            pattern: run.pattern.clone(),
            moved: if run.undone { 0 } else { n },
            undone: if run.undone { n } else { 0 },
            failed: 0,
        })
    }

    pub(super) fn step(&self, run: FileRunId, redo: bool) -> Result<RunReport, BackendError> {
        let mut state = self.lock();
        let Some(mut current) = state.run.clone().filter(|r| r.id == run.get()) else {
            return Err(BackendError(
                "there is nothing to undo or redo in that run".into(),
            ));
        };
        if current.undone != redo {
            return Err(BackendError(
                "there is nothing to undo or redo in that run".into(),
            ));
        }
        let mut outcomes = Vec::new();
        let mut moves = current.moves.clone();
        if !redo {
            moves.reverse();
        }
        for (id, from, to) in moves {
            let (from, to) = if redo { (from, to) } else { (to, from) };
            if let Some(f) = state.files.iter_mut().find(|f| f.id == id) {
                f.path.clone_from(&to);
            }
            outcomes.push(MoveOutcome {
                file: Some(FileId::from_raw(id)),
                from,
                to,
                problem: None,
            });
        }
        current.undone = !redo;
        state.run = Some(current);
        Ok(RunReport {
            run: Some(run),
            outcomes,
        })
    }

    pub(super) fn rename(&self, track: TrackKey, name: &str) -> Result<RunReport, BackendError> {
        let request = {
            let state = self.lock();
            let f = state
                .files
                .iter()
                .find(|f| u64::try_from(f.id).ok() == Some(track.0))
                .ok_or_else(|| BackendError("the track is not in the library".into()))?;
            MoveRequest {
                file: FileId::from_raw(f.id),
                from: f.path.clone(),
                to: f.path.with_file_name(name),
            }
        };
        Ok(self.organize(&[request], None))
    }

    pub(super) fn patterns(&self) -> Vec<SavedPattern> {
        self.lock().patterns.clone()
    }

    pub(super) fn save_pattern(&self, id: Option<i64>, name: &str, pattern: &str) -> i64 {
        let mut state = self.lock();
        match id.and_then(|id| state.patterns.iter_mut().find(|p| p.id == id)) {
            Some(saved) => {
                saved.name = name.into();
                saved.pattern = pattern.into();
                saved.id
            }
            None => {
                let id = state.patterns.iter().map(|p| p.id).max().unwrap_or(0) + 1;
                state.patterns.push(SavedPattern {
                    id,
                    name: name.into(),
                    pattern: pattern.into(),
                });
                id
            }
        }
    }

    pub(super) fn delete_pattern(&self, id: i64) {
        self.lock().patterns.retain(|p| p.id != id);
    }

    pub(super) fn missing(&self) -> Vec<MissingFile> {
        self.lock().missing.clone()
    }

    /// The archive's missing files turn up in the sorted folder, renamed; the
    /// others are not found.
    pub(super) fn find(&self, files: &[FileId], folder: Option<&Path>) -> Vec<FoundFile> {
        let sorted = sorted_folder(&self.music);
        if folder.is_some_and(|f| !sorted.starts_with(f) && !f.starts_with(&sorted)) {
            return Vec::new();
        }
        let state = self.lock();
        state
            .missing
            .iter()
            .filter(|m| files.contains(&m.file) && m.root == archive())
            .map(|m| {
                let title = m.title.clone().unwrap_or_default();
                let number = m
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let ext = m
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let album = if m.artist.as_deref() == Some("Okto") {
                    "Okto/2020 Night Ferry"
                } else {
                    "Nordlys Ensemble/2017 Winter Light"
                };
                let name = if ext == "dsf" {
                    format!("{number}.{ext}")
                } else {
                    format!("{number} {title}.{ext}")
                };
                FoundFile {
                    file: m.file,
                    at: sorted.join(album).join(name),
                }
            })
            .collect()
    }

    pub(super) fn relink(&self, found: &[FoundFile]) -> Vec<RelinkOutcome> {
        let mut state = self.lock();
        found
            .iter()
            .map(|f| {
                let before = state.missing.len();
                state.missing.retain(|m| m.file != f.file);
                RelinkOutcome {
                    file: f.file,
                    problem: (state.missing.len() == before)
                        .then_some(super::super::services::RelinkProblem::NotMissing),
                }
            })
            .collect()
    }

    pub(super) fn remove(&self, files: &[FileId]) -> usize {
        let mut state = self.lock();
        let before = state.missing.len();
        state.missing.retain(|m| !files.contains(&m.file));
        before - state.missing.len()
    }

    /// The paths of a check: the archive's files.
    pub(super) fn verify_targets(&self) -> Vec<PathBuf> {
        let lossless = archive();
        let mut paths: Vec<PathBuf> = PROBLEMS
            .iter()
            .map(|(path, _, _)| lossless.join(path))
            .collect();
        paths.extend(UNREADABLE.iter().map(|path| lossless.join(path)));
        for i in 0..1_112 {
            paths.push(lossless.join(format!(
                "Library/{:03}/{:02} Track.flac",
                i / 12,
                i % 12 + 1
            )));
        }
        paths
    }

    /// The invented report of `path`.
    pub(super) fn verify(path: &Path) -> VerifyReport {
        let lossless = archive();
        let relative = path.strip_prefix(&lossless).unwrap_or(path);
        let mut report = VerifyReport {
            path: path.to_path_buf(),
            format: Some(VerifiedFormat {
                codec: "flac".into(),
                bits: Some(16),
                rate: Some(44_100),
            }),
            unreadable: None,
            problems: Vec::new(),
        };
        if let Some((_, format, problem)) =
            PROBLEMS.iter().find(|(p, _, _)| Path::new(p) == relative)
        {
            report.format = Some(format());
            report.problems.push(problem());
        } else if UNREADABLE.iter().any(|p| Path::new(p) == relative) {
            report.format = None;
            report.unreadable = Some("no decoder can open the file".into());
        }
        report
    }
}

type Problem = (&'static str, fn() -> VerifiedFormat, fn() -> VerifyProblem);

fn format(codec: &str, bits: Option<u32>, rate: u32) -> VerifiedFormat {
    VerifiedFormat {
        codec: codec.into(),
        bits,
        rate: Some(rate),
    }
}

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s)
}

const PROBLEMS: &[Problem] = &[
    (
        "Okto/Paper Lanterns/04 Lantern Maker.m4a",
        || format("alac", Some(16), 44_100),
        || VerifyProblem::Shorter {
            stated: secs(254.4),
            decoded: secs(252.0),
        },
    ),
    (
        "Eeva Saar/Goldberg Variations/17 Variatio 16.flac",
        || format("flac", Some(24), 192_000),
        || VerifyProblem::ChecksumMismatch,
    ),
    (
        "Halden/Concrete Garden/02 Rebar.flac",
        || format("flac", Some(16), 44_100),
        || VerifyProblem::Skipped {
            count: 12,
            at: Some(secs(221.0)),
        },
    ),
    (
        "Harbour Choir/Saltwater Hymns/09 Evening Tide.wv",
        || format("wavpack", Some(16), 44_100),
        || VerifyProblem::Stopped {
            at: secs(302.0),
            reason: "wavpack failed while decoding".into(),
        },
    ),
    (
        "Anton Verde Trio/Blue Hour Sessions/05 Lamplight.wv",
        || format("wavpack", Some(24), 96_000),
        || VerifyProblem::ChecksumMismatch,
    ),
    (
        "Nordlys Ensemble/Aurora Tapes/03 Green Arc.dff",
        || format("dsd", Some(1), 2_822_400),
        || VerifyProblem::Shorter {
            stated: secs(380.0),
            decoded: secs(362.0),
        },
    ),
];

const UNREADABLE: &[&str] = &[
    "Incoming/rip-errors/track01.flac",
    "Incoming/rip-errors/track02.flac",
];
