// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Imports over invented content, with the figures of the design: Rhythmbox
//! and Lollypop found on this computer, an iTunes `Library.xml` from another
//! computer that matches by path only once a folder that moved is added, a
//! Helikodu data file, three earlier imports, and twelve missing tracks
//! replaced by another copy. Reading takes a moment, so that the window's
//! working state can be seen. Nothing touches the disk: an export writes
//! nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_model::user_fields::FieldDefinition;

use super::super::interchange::{
    FieldConflict, ImportFrom, ImportPreview, ImportProblem, ImportRunKind, ImportRunRow,
    ImportSummary, InterchangeServices, MatchCounts, MergeOptions, PlanTotals, PlannedPlaylist,
    PlannedPlaylistKind, Player, PlayerSource, ReplacedTrack, SavedBy,
};
use super::super::{BackendError, TrackKey};

/// How long reading a sample source takes.
const READING: Duration = Duration::from_millis(250);

fn day(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// 2 November 2026, 10:00 UTC.
const NOV_2: u64 = 1_793_613_600;
/// 28 October 2026, 10:00 UTC.
const OCT_28: u64 = 1_793_181_600;
/// 11 October 2026, 10:00 UTC.
const OCT_11: u64 = 1_791_712_800;
/// 18 November 2026, 10:00 UTC.
const NOV_18: u64 = 1_794_996_000;

fn home() -> PathBuf {
    gtk::glib::home_dir()
}

fn rhythmbox_file() -> PathBuf {
    home().join(".local/share/rhythmbox/rhythmdb.xml")
}

fn lollypop_file() -> PathBuf {
    home().join(".var/app/org.gnome.Lollypop/data/lollypop/lollypop.db")
}

fn listed(count: usize, seed: usize) -> Vec<String> {
    const ARTISTS: [&str; 6] = [
        "Mari Kask",
        "Okto",
        "Nordlys Ensemble",
        "Eeva Saar",
        "Juno Park",
        "Tuuli",
    ];
    const TITLES: [&str; 8] = [
        "Departure",
        "Low Tide",
        "Glass Harbour",
        "Northern Hours",
        "Paper Kites",
        "Undertow",
        "First Light",
        "Long Way Home",
    ];
    (0..count.min(200))
        .map(|i| {
            let n = i + seed;
            let title = TITLES[n % TITLES.len()];
            let artist = ARTISTS[(n / TITLES.len()) % ARTISTS.len()];
            if n < TITLES.len() * ARTISTS.len() {
                format!("{artist}, {title}")
            } else {
                format!(
                    "{artist}, {title} ({})",
                    n / (TITLES.len() * ARTISTS.len()) + 1
                )
            }
        })
        .collect()
}

fn playlist(name: &str, folder: &[&str], kind: PlannedPlaylistKind) -> PlannedPlaylist {
    PlannedPlaylist {
        name: name.to_owned(),
        folder: folder.iter().map(|f| (*f).to_owned()).collect(),
        kind,
    }
}

fn list(items: usize, matched: usize) -> PlannedPlaylistKind {
    PlannedPlaylistKind::Static {
        items,
        matched,
        from_rules: false,
    }
}

fn rules(items: usize) -> PlannedPlaylistKind {
    PlannedPlaylistKind::Static {
        items,
        matched: items,
        from_rules: true,
    }
}

fn notes() -> Vec<String> {
    vec![
        "2 entries are streams, not files".to_owned(),
        "1 entry has no location".to_owned(),
        "the play queue was left out".to_owned(),
    ]
}

fn preview(from: &ImportFrom, files: &[&str]) -> ImportPreview {
    ImportPreview {
        from: from.clone(),
        files: files.iter().map(|f| (*f).to_owned()).collect(),
        saved: None,
        matching: MatchCounts::default(),
        totals: PlanTotals::default(),
        playlists: Vec::new(),
        ambiguous: Vec::new(),
        unmatched: Vec::new(),
        field_conflicts: Vec::new(),
        notes: Vec::new(),
        earlier: None,
        empty: false,
        plan: 0,
    }
}

fn rhythmbox(from: &ImportFrom) -> ImportPreview {
    let mut p = preview(from, &["rhythmdb.xml", "playlists.xml"]);
    p.matching = MatchCounts {
        entries: 9_120,
        by_path: 7_900,
        by_tags: 314,
        ambiguous: 26,
        unmatched: 880,
    };
    p.totals = PlanTotals {
        tracks: 7_904,
        play_counts: 6_102,
        plays_added: 48_330,
        ratings: 1_240,
        rating_conflicts: 38,
        last_played: 5_870,
        added: 2_304,
        unchanged: 310,
        ..PlanTotals::default()
    };
    p.playlists = vec![
        playlist("Late evening", &[], list(45, 42)),
        playlist("Morning coffee", &[], list(62, 62)),
        playlist(
            "Recently added",
            &[],
            PlannedPlaylistKind::Smart("added<30d sort:added desc".to_owned()),
        ),
        playlist(
            "My top rated",
            &[],
            PlannedPlaylistKind::Smart("rating>=4 sort:rating desc".to_owned()),
        ),
        playlist(
            "Unheard this year",
            &[],
            PlannedPlaylistKind::Unmapped("One of its rules has no counterpart here".to_owned()),
        ),
        playlist("Road songs", &[], PlannedPlaylistKind::Exists),
    ];
    p.ambiguous = listed(26, 3);
    p.unmatched = listed(880, 0);
    p.notes = notes();
    p.earlier = Some(day(NOV_2));
    p
}

fn itunes(from: &ImportFrom, moved: bool) -> ImportPreview {
    let mut p = preview(from, &["Library.xml"]);
    if moved {
        p.matching = MatchCounts {
            entries: 9_120,
            by_path: 2_140,
            by_tags: 4_770,
            ambiguous: 40,
            unmatched: 2_170,
        };
        p.totals = PlanTotals {
            tracks: 6_700,
            play_counts: 5_960,
            plays_added: 51_240,
            skip_counts: 1_712,
            skips_added: 2_940,
            ratings: 2_204,
            rating_conflicts: 52,
            loved: 318,
            last_played: 5_420,
            added: 6_104,
            unchanged: 210,
            ..PlanTotals::default()
        };
    } else {
        p.matching = MatchCounts {
            entries: 9_120,
            by_path: 0,
            by_tags: 5_480,
            ambiguous: 46,
            unmatched: 3_594,
        };
        p.totals = PlanTotals {
            tracks: 5_320,
            play_counts: 4_730,
            plays_added: 40_118,
            skip_counts: 1_380,
            skips_added: 2_310,
            ratings: 1_760,
            rating_conflicts: 52,
            loved: 260,
            last_played: 4_310,
            added: 4_890,
            unchanged: 160,
            ..PlanTotals::default()
        };
    }
    p.playlists = vec![
        playlist("90s road trip", &["Trips"], list(64, 55)),
        playlist("Sunday", &[], list(31, 31)),
        playlist("Recently Played", &[], rules(25)),
        playlist("Top 25 Most Played", &[], rules(25)),
        playlist("Purchased", &[], list(212, 172)),
    ];
    p.ambiguous = listed(p.matching.ambiguous, 5);
    p.unmatched = listed(p.matching.unmatched, 1);
    p.notes = notes();
    p
}

fn data_file(from: &ImportFrom) -> ImportPreview {
    let name = from
        .path()
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut p = preview(from, &[&name]);
    p.saved = Some(SavedBy {
        program: Some("Helikodu 1.0".to_owned()),
        at: Some(day(NOV_18)),
        tracks: 18_240,
    });
    p.matching = MatchCounts {
        entries: 18_240,
        by_path: 17_100,
        by_tags: 850,
        ambiguous: 28,
        unmatched: 262,
    };
    p.totals = PlanTotals {
        tracks: 14_830,
        play_counts: 2_410,
        plays_added: 9_870,
        skip_counts: 880,
        skips_added: 1_204,
        ratings: 3_120,
        rating_conflicts: 14,
        loved: 96,
        first_played: 1_204,
        last_played: 1_950,
        history: 31_200,
        saved_searches: 4,
        favourites: 96,
        labels: 7,
        label_links: 229,
        label_tracks: 212,
        label_albums: 14,
        label_artists: 3,
        label_unmatched: 5,
        fields: 2,
        field_conflicts: 1,
        unchanged: 3_120,
        ..PlanTotals::default()
    };
    p.playlists = vec![
        playlist("Late evening", &[], list(48, 48)),
        playlist("Intervals", &["Workouts"], list(32, 30)),
        playlist(
            "Unrated hi-res",
            &[],
            PlannedPlaylistKind::Smart("rating=0 samplerate>=88200 added<12m".to_owned()),
        ),
    ];
    p.field_conflicts = vec![FieldConflict {
        name: "era".to_owned(),
        ours: FieldDefinition::Computed {
            expression: "{decade or \"unknown\"}".to_owned(),
        },
        theirs: FieldDefinition::Computed {
            expression: "{decade or \"?\"}".to_owned(),
        },
    }];
    p.ambiguous = listed(28, 7);
    p.unmatched = listed(262, 2);
    p
}

/// Lollypop, imported before with nothing new since.
fn lollypop(from: &ImportFrom) -> ImportPreview {
    let mut p = preview(from, &["lollypop.db", "playlists.db"]);
    p.matching = MatchCounts {
        entries: 3_520,
        by_path: 3_410,
        by_tags: 0,
        ambiguous: 0,
        unmatched: 110,
    };
    p.totals = PlanTotals {
        unchanged: 3_410,
        ..PlanTotals::default()
    };
    p.playlists = vec![playlist("Evening", &[], PlannedPlaylistKind::Exists)];
    p.unmatched = listed(110, 4);
    p.earlier = Some(day(NOV_2));
    p.empty = true;
    p
}

fn summary_of(preview: &ImportPreview) -> ImportSummary {
    ImportSummary {
        files: preview.files.clone(),
        matching: preview.matching,
        totals: preview.totals,
        playlists: preview.playlists.clone(),
    }
}

fn replaced() -> Vec<ReplacedTrack> {
    let music = home().join("Music");
    let row = |old: u64,
               title: &str,
               artist: &str,
               from: &str,
               to: &[&str],
               (plays, rating, history, playlists): (u64, Option<u8>, usize, usize)| {
        ReplacedTrack {
            old: TrackKey(900_000 + old),
            old_path: if from.starts_with('/') {
                PathBuf::from(from)
            } else {
                music.join(from)
            },
            title: Some(title.to_owned()),
            artist: Some(artist.to_owned()),
            plays,
            rating,
            history,
            playlists,
            new: to
                .iter()
                .enumerate()
                .map(|(i, p)| (TrackKey(910_000 + old * 10 + i as u64), music.join(p)))
                .collect(),
        }
    };
    vec![
        row(
            1,
            "Shoreline, 6 a.m.",
            "Mari Kask",
            "Incoming/old rips/Mari Kask - Shoreline.mp3",
            &["Mari Kask/2021 Tidewater Rooms/03 Shoreline, 6 a.m..flac"],
            (142, Some(8), 96, 3),
        ),
        row(
            2,
            "Night Ferry",
            "Okto",
            "/mnt/archive/Lossless/Okto/Night Ferry/02.mp3",
            &[
                "Okto/2020 Night Ferry/02 Night Ferry.flac",
                "Okto/Live 2022/07 Night Ferry.flac",
            ],
            (61, Some(10), 40, 2),
        ),
        row(
            3,
            "Kõpu Light",
            "Mari Kask",
            "Incoming/old rips/Kopu Light.mp3",
            &["Mari Kask/2018 Letters from Hiiumaa/03 Kõpu Light.flac"],
            (38, Some(8), 31, 1),
        ),
        row(
            4,
            "Vespers",
            "Nordlys Ensemble",
            "/mnt/archive/Lossless/Nordlys/Northern Hours (PCM)/01 Vespers.flac",
            &["Nordlys Ensemble/2020 Northern Hours/1-01 Vespers.dsf"],
            (19, None, 12, 0),
        ),
        row(
            5,
            "Paper Lanterns",
            "Okto",
            "Okto/Paper Lanterns (128k)/01 Paper Lanterns.mp3",
            &["Okto/2017 Paper Lanterns/01 Paper Lanterns.m4a"],
            (27, Some(6), 22, 4),
        ),
        row(
            6,
            "Aria",
            "Eeva Saar",
            "Incoming/old rips/Goldberg - Aria.ogg",
            &["Eeva Saar/2018 Goldberg Variations/01 Aria.flac"],
            (31, Some(10), 25, 1),
        ),
        row(
            7,
            "Greenhouse",
            "Juno Park",
            "Incoming/Juno Park - Greenhouse (web)/01.opus",
            &["Juno Park/2024 Greenhouse/01 Greenhouse.flac"],
            (9, None, 9, 0),
        ),
        row(
            8,
            "Low Tide",
            "Mari Kask",
            "Incoming/old rips/Mari Kask - Low Tide.mp3",
            &["Mari Kask/2021 Tidewater Rooms/05 Low Tide.flac"],
            (54, Some(8), 41, 2),
        ),
        row(
            9,
            "Undertow",
            "Okto",
            "Okto/Paper Lanterns (128k)/04 Undertow.mp3",
            &["Okto/2017 Paper Lanterns/04 Undertow.m4a"],
            (12, Some(4), 10, 0),
        ),
        row(
            10,
            "Glass Harbour",
            "Tuuli",
            "Incoming/Tuuli - Glass Harbour.ogg",
            &["Tuuli/2023 Glass Harbour/01 Glass Harbour.flac"],
            (23, None, 18, 1),
        ),
        row(
            11,
            "Matins",
            "Nordlys Ensemble",
            "/mnt/archive/Lossless/Nordlys/Northern Hours (PCM)/02 Matins.flac",
            &["Nordlys Ensemble/2020 Northern Hours/1-02 Matins.dsf"],
            (8, Some(8), 6, 0),
        ),
        row(
            12,
            "First Light",
            "Juno Park",
            "Incoming/Juno Park - Greenhouse (web)/02.opus",
            &["Juno Park/2024 Greenhouse/02 First Light.flac"],
            (6, None, 6, 0),
        ),
    ]
}

#[derive(Debug)]
struct State {
    runs: Vec<ImportRunRow>,
    replaced: Vec<ReplacedTrack>,
    /// The tracks each carry run took out of `replaced`.
    carried: HashMap<i64, Vec<ReplacedTrack>>,
    plan: Option<ImportPreview>,
    next_plan: u64,
}

/// [`InterchangeServices`] over invented content.
#[derive(Debug)]
pub(crate) struct SampleInterchange {
    state: Mutex<State>,
}

impl Default for SampleInterchange {
    fn default() -> Self {
        let lollypop = lollypop(&ImportFrom::Player {
            player: Player::Lollypop,
            path: lollypop_file(),
        });
        let mut lollypop_summary = summary_of(&lollypop);
        lollypop_summary.totals.tracks = 3_410;
        lollypop_summary.totals.play_counts = 3_102;
        lollypop_summary.totals.plays_added = 21_406;
        lollypop_summary.totals.unchanged = 0;
        let strawberry = home().join(".local/share/strawberry/strawberry/strawberry.db");
        let runs = vec![
            ImportRunRow {
                id: 3,
                kind: ImportRunKind::Player,
                player: Some(Player::Lollypop),
                path: Some(lollypop_file()),
                at: day(NOV_2),
                undone: false,
                undone_at: None,
                tracks: 3_410,
                summary: Some(lollypop_summary),
            },
            ImportRunRow {
                id: 2,
                kind: ImportRunKind::Carry,
                player: None,
                path: None,
                at: day(OCT_28),
                undone: false,
                undone_at: None,
                tracks: 12,
                summary: Some(ImportSummary {
                    totals: PlanTotals {
                        tracks: 12,
                        ..PlanTotals::default()
                    },
                    ..ImportSummary::default()
                }),
            },
            ImportRunRow {
                id: 1,
                kind: ImportRunKind::Player,
                player: Some(Player::Strawberry),
                path: Some(strawberry),
                at: day(OCT_11),
                undone: true,
                undone_at: Some(day(OCT_11 + 86_400)),
                tracks: 1_288,
                summary: Some(ImportSummary {
                    files: vec!["strawberry.db".to_owned()],
                    matching: MatchCounts {
                        entries: 1_402,
                        by_path: 1_288,
                        by_tags: 0,
                        ambiguous: 0,
                        unmatched: 114,
                    },
                    totals: PlanTotals {
                        tracks: 1_288,
                        play_counts: 1_288,
                        plays_added: 9_120,
                        ..PlanTotals::default()
                    },
                    playlists: Vec::new(),
                }),
            },
        ];
        Self {
            state: Mutex::new(State {
                runs,
                replaced: replaced(),
                carried: HashMap::new(),
                plan: None,
                next_plan: 0,
            }),
        }
    }
}

impl SampleInterchange {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn newest_open(state: &State) -> Option<i64> {
        state.runs.iter().filter(|r| !r.undone).map(|r| r.id).max()
    }

    fn add_run(state: &mut State, mut run: ImportRunRow) -> i64 {
        let id = state.runs.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        run.id = id;
        state.runs.insert(0, run);
        id
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl InterchangeServices for SampleInterchange {
    fn players(&self) -> Vec<PlayerSource> {
        Player::ALL
            .into_iter()
            .map(|player| PlayerSource {
                player,
                found: match player {
                    Player::Rhythmbox => Some(rhythmbox_file()),
                    Player::Lollypop => Some(lollypop_file()),
                    _ => None,
                },
            })
            .collect()
    }

    fn plan_import(
        &self,
        from: &ImportFrom,
        options: &MergeOptions,
    ) -> Result<ImportPreview, ImportProblem> {
        std::thread::sleep(READING);
        let name = file_name(from.path());
        let mut preview = match from {
            ImportFrom::DataFile(_) if name.contains("newer") => {
                return Err(ImportProblem::Newer);
            }
            ImportFrom::DataFile(_) if !name.ends_with(".json") => {
                return Err(ImportProblem::Unreadable(
                    "not a user data file: expected value at line 1 column 1".to_owned(),
                ));
            }
            ImportFrom::DataFile(_) => data_file(from),
            ImportFrom::Player { player, .. } => match player {
                Player::Rhythmbox => rhythmbox(from),
                Player::Lollypop => lollypop(from),
                Player::ITunes if name.ends_with(".xml") => {
                    itunes(from, !options.mappings.is_empty())
                }
                Player::Strawberry | Player::Clementine if name.ends_with(".db") => {
                    let mut p = lollypop(from);
                    p.files = vec![name];
                    p.earlier = None;
                    p.playlists.clear();
                    p
                }
                _ => {
                    return Err(ImportProblem::Unreadable(format!(
                        "{name}: the file is not a database"
                    )));
                }
            },
        };
        if options.skip_playlists {
            preview.playlists.clear();
        }
        if options.replace_ratings {
            preview.totals.ratings += preview.totals.rating_conflicts;
            preview.totals.rating_conflicts = 0;
        }
        let mut state = self.lock();
        state.next_plan += 1;
        preview.plan = state.next_plan;
        state.plan = Some(preview.clone());
        Ok(preview)
    }

    fn apply_import(&self, plan: u64) -> Result<i64, BackendError> {
        let mut state = self.lock();
        let Some(preview) = state.plan.take().filter(|p| p.plan == plan && !p.empty) else {
            return Err(BackendError("Nothing to import.".to_owned()));
        };
        let run = ImportRunRow {
            id: 0,
            kind: if preview.from.player().is_some() {
                ImportRunKind::Player
            } else {
                ImportRunKind::UserData
            },
            player: preview.from.player(),
            path: Some(preview.from.path().to_path_buf()),
            at: SystemTime::now(),
            undone: false,
            undone_at: None,
            tracks: preview.totals.tracks,
            summary: Some(summary_of(&preview)),
        };
        Ok(Self::add_run(&mut state, run))
    }

    fn import_runs(&self) -> Result<Vec<ImportRunRow>, BackendError> {
        Ok(self.lock().runs.clone())
    }

    fn undo_import(&self, run: i64) -> Result<(), BackendError> {
        let mut state = self.lock();
        if Self::newest_open(&state) != Some(run) {
            return Err(BackendError("Undo the newer imports first.".to_owned()));
        }
        if let Some(row) = state.runs.iter_mut().find(|r| r.id == run) {
            row.undone = true;
            row.undone_at = Some(SystemTime::now());
        }
        if let Some(back) = state.carried.remove(&run) {
            state.replaced.extend(back);
            state.replaced.sort_by_key(|t| t.old.0);
        }
        Ok(())
    }

    fn export_user_data(&self, path: &Path) -> Result<(), BackendError> {
        if path.as_os_str().is_empty() {
            return Err(BackendError("No file was named.".to_owned()));
        }
        Ok(())
    }

    fn replaced_tracks(&self) -> Result<Vec<ReplacedTrack>, BackendError> {
        Ok(self.lock().replaced.clone())
    }

    fn carry_stats(&self, pairs: &[(TrackKey, TrackKey)]) -> Result<i64, BackendError> {
        let mut state = self.lock();
        let olds: Vec<TrackKey> = pairs.iter().map(|(old, _)| *old).collect();
        let (taken, kept): (Vec<ReplacedTrack>, Vec<ReplacedTrack>) = state
            .replaced
            .drain(..)
            .partition(|t| olds.contains(&t.old));
        state.replaced = kept;
        if taken.is_empty() {
            return Err(BackendError(
                "There is no such track in the library.".to_owned(),
            ));
        }
        let run = ImportRunRow {
            id: 0,
            kind: ImportRunKind::Carry,
            player: None,
            path: None,
            at: SystemTime::now(),
            undone: false,
            undone_at: None,
            tracks: taken.len(),
            summary: Some(ImportSummary {
                totals: PlanTotals {
                    tracks: taken.len(),
                    ..PlanTotals::default()
                },
                ..ImportSummary::default()
            }),
        };
        let id = Self::add_run(&mut state, run);
        state.carried.insert(id, taken);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_import_is_undone() {
        let sample = SampleInterchange::default();
        assert!(sample.undo_import(1).is_err(), "undone already");
        assert!(sample.undo_import(2).is_err(), "not the newest");
        let from = ImportFrom::Player {
            player: Player::Rhythmbox,
            path: rhythmbox_file(),
        };
        let plan = sample.plan_import(&from, &MergeOptions::default()).unwrap();
        assert_eq!(plan.totals.tracks, 7_904);
        let run = sample.apply_import(plan.plan).unwrap();
        assert_eq!(sample.import_runs().unwrap()[0].id, run);
        assert!(sample.undo_import(3).is_err());
        sample.undo_import(run).unwrap();
        sample.undo_import(3).unwrap();
    }

    #[test]
    fn carrying_takes_rows_out_until_undone() {
        let sample = SampleInterchange::default();
        let rows = sample.replaced_tracks().unwrap();
        assert_eq!(rows.len(), 12);
        assert_eq!(rows[1].new.len(), 2);
        let pairs: Vec<_> = rows.iter().take(3).map(|r| (r.old, r.new[0].0)).collect();
        let run = sample.carry_stats(&pairs).unwrap();
        assert_eq!(sample.replaced_tracks().unwrap().len(), 9);
        assert_eq!(sample.import_runs().unwrap()[0].tracks, 3);
        sample.undo_import(run).unwrap();
        assert_eq!(sample.replaced_tracks().unwrap(), rows);
    }

    #[test]
    fn a_folder_that_moved_matches_by_path() {
        let sample = SampleInterchange::default();
        let from = ImportFrom::Player {
            player: Player::ITunes,
            path: PathBuf::from("/backup/Library.xml"),
        };
        let plain = sample.plan_import(&from, &MergeOptions::default()).unwrap();
        assert_eq!(plain.matching.by_path, 0);
        let moved = sample
            .plan_import(
                &from,
                &MergeOptions {
                    mappings: vec![super::super::super::interchange::PathMapping::new(
                        "D:\\Musik",
                        "/mnt/archive/Lossless",
                    )],
                    ..MergeOptions::default()
                },
            )
            .unwrap();
        assert_eq!(moved.matching.by_path, 2_140);
        assert_eq!(moved.totals.tracks, 6_700);
        assert!(
            sample.apply_import(plain.plan).is_err(),
            "only the newest plan"
        );
    }
}
