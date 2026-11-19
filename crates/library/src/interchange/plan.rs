// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The plan of an import: what matched, what would change, and the work to do.

use std::collections::{HashMap, HashSet};
use std::os::unix::ffi::OsStrExt;

use rusqlite::{Connection, OptionalExtension};

use super::foreign::{
    ForeignItem, ForeignLibrary, ForeignListen, ForeignPlaylistKind, ForeignTrack,
};
use super::matching::{LibraryIndex, Match, describe};
use super::merge::{self, Baseline, CountRule, Incoming, Stats};
use super::uri::{PathMapping, local_path};
use super::{ImportRunKind, Player};
use crate::db::unix_time::{to_millis, to_unix_seconds};

/// The most entries listed by name as ambiguous or unmatched in a plan.
const LISTED: usize = 200;

/// How an import merges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeOptions {
    /// Path prefixes of the other library and the folders they are now.
    pub mappings: Vec<PathMapping>,
    /// Take the imported rating also for tracks that have one.
    pub replace_ratings: bool,
    /// Leave the playlists out.
    pub skip_playlists: bool,
}

/// How the entries matched the library's tracks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MatchCounts {
    /// The entries read.
    pub entries: usize,
    /// Matched by path.
    pub by_path: usize,
    /// Matched by tags.
    pub by_tags: usize,
    /// Fitting tracks of more than one identity, left out.
    pub ambiguous: usize,
    /// Matching no track.
    pub unmatched: usize,
}

impl MatchCounts {
    /// The entries matched, by path or by tags.
    pub fn matched(&self) -> usize {
        self.by_path + self.by_tags
    }
}

/// What an import would change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanTotals {
    /// The tracks (identities) whose statistics change.
    pub tracks: usize,
    /// The tracks whose play count rises, and by how many plays in all.
    pub play_counts: usize,
    /// The plays added.
    pub plays_added: u64,
    /// The tracks whose skip count rises.
    pub skip_counts: usize,
    /// The skips added.
    pub skips_added: u64,
    /// The ratings set.
    pub ratings: usize,
    /// The tracks rated differently here, whose rating is kept.
    pub rating_conflicts: usize,
    /// The tracks marked loved.
    pub loved: usize,
    /// The tracks whose first play moves earlier.
    pub first_played: usize,
    /// The tracks whose last play moves later.
    pub last_played: usize,
    /// The tracks whose date added moves earlier.
    pub added: usize,
    /// The matched tracks that would not change (for example because the same
    /// source was imported before).
    pub unchanged: usize,
    /// The history entries added.
    pub history: usize,
    /// The saved searches added.
    pub saved_searches: usize,
    /// The tracks added to Favourites.
    pub favourites: usize,
}

/// What a playlist of the source becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedPlaylistKind {
    /// A static playlist of `items` entries, `matched` of them library tracks;
    /// `from_rules` when the source fills it from rules that could not be read.
    Static {
        /// The entries.
        items: usize,
        /// The entries that are library tracks.
        matched: usize,
        /// Whether the source fills it from rules.
        from_rules: bool,
    },
    /// A smart playlist with this query.
    Smart(String),
    /// Not imported, and why.
    Unmapped(String),
    /// Made by an earlier import already (or, for user data, one of that name
    /// exists): not made again.
    Exists,
}

/// A playlist of the source in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPlaylist {
    /// Its name.
    pub name: String,
    /// The folders it goes in.
    pub folder: Vec<String>,
    /// What it becomes.
    pub kind: PlannedPlaylistKind,
}

/// An entry of a playlist to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ItemWork {
    pub(super) identity: Option<i64>,
    pub(super) path: Option<Vec<u8>>,
    pub(super) title: Option<String>,
    pub(super) artist: Option<String>,
    pub(super) album: Option<String>,
    pub(super) duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PlaylistWork {
    Static(Vec<ItemWork>),
    Smart(String),
}

/// The work of an import, done by `apply`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Work {
    pub(super) identities: Vec<(i64, Incoming, Vec<i64>)>,
    pub(super) playlists: Vec<(String, Vec<String>, PlaylistWork)>,
    pub(super) history: Vec<(i64, ForeignListen)>,
    pub(super) saved_searches: Vec<(String, String)>,
    pub(super) favourites: Vec<i64>,
}

/// What an import would do: [`Library::plan_import`](crate::Library::plan_import)
/// makes it, [`Library::apply_import`](crate::Library::apply_import) carries it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    /// What is imported.
    pub kind: ImportRunKind,
    /// The player, for an import from another player.
    pub player: Option<Player>,
    /// The source: the player and the path of its file, or the export file.
    pub source: String,
    /// How the entries matched.
    pub matching: MatchCounts,
    /// What would change.
    pub totals: PlanTotals,
    /// The playlists and what each becomes.
    pub playlists: Vec<PlannedPlaylist>,
    /// The first ambiguous entries, in words.
    pub ambiguous: Vec<String>,
    /// The first unmatched entries, in words.
    pub unmatched: Vec<String>,
    /// What the reader left out, in words.
    pub notes: Vec<String>,
    pub(super) rule: CountRule,
    pub(super) replace_ratings: bool,
    pub(super) work: Work,
}

impl ImportPlan {
    /// Whether applying the plan would change nothing.
    pub fn is_empty(&self) -> bool {
        self.totals.tracks == 0
            && self.totals.history == 0
            && self.totals.saved_searches == 0
            && self.totals.favourites == 0
            && self.work.playlists.is_empty()
    }
}

fn incoming(track: &ForeignTrack) -> Incoming {
    Incoming {
        play_count: i64::try_from(track.play_count).unwrap_or(i64::MAX),
        skip_count: i64::try_from(track.skip_count).unwrap_or(i64::MAX),
        first_played: track.first_played.map(to_unix_seconds),
        last_played: track.last_played.map(to_unix_seconds),
        last_skipped: track.last_skipped.map(to_unix_seconds),
        rating: track.rating.map(|r| i64::from(r.half_stars())),
        loved: track.loved,
        added: track.added.map(to_unix_seconds),
    }
}

/// The baseline of `source` for `identity`.
pub(super) fn baseline(
    c: &Connection,
    source: &str,
    identity: i64,
) -> rusqlite::Result<Option<Baseline>> {
    c.prepare_cached(
        "SELECT play_count, skip_count FROM import_baselines
         WHERE source = ?1 AND identity_id = ?2",
    )?
    .query_row(rusqlite::params![source, identity], |row| {
        Ok(Baseline {
            play_count: row.get(0)?,
            skip_count: row.get(1)?,
        })
    })
    .optional()
}

/// The source of an import and how it merges.
pub(super) struct PlanInput<'a> {
    pub(super) kind: ImportRunKind,
    pub(super) player: Option<Player>,
    pub(super) source: String,
    pub(super) library: &'a ForeignLibrary,
    pub(super) options: &'a MergeOptions,
}

/// The playlist names an earlier import of `source` made that still exist, or,
/// for user data (`any`), the names of every playlist.
fn existing_playlists(
    c: &Connection,
    source: &str,
    any: bool,
) -> rusqlite::Result<HashSet<String>> {
    let sql = if any {
        "SELECT name FROM playlists WHERE kind != 0 AND ?1 = ?1"
    } else {
        "SELECT p.name FROM import_run_rows r JOIN import_runs u ON u.id = r.run_id
         JOIN playlists p ON p.id = r.row_id
         WHERE r.kind = 0 AND u.undone = 0 AND u.source = ?1"
    };
    let mut statement = c.prepare(sql)?;
    let names = statement.query_map([source], |row| row.get::<_, String>(0))?;
    names.collect()
}

/// Makes the plan of `input` against the database `c`.
pub(super) fn make(c: &Connection, input: &PlanInput<'_>) -> rusqlite::Result<ImportPlan> {
    let index = LibraryIndex::load(c)?;
    let mappings = &input.options.mappings;
    let rule = match input.kind {
        ImportRunKind::UserData => CountRule::Max,
        _ => CountRule::AddNew,
    };
    let mut matching = MatchCounts::default();
    let mut ambiguous = Vec::new();
    let mut unmatched = Vec::new();
    // The identity of each entry, by index.
    let mut matched: Vec<Option<i64>> = Vec::with_capacity(input.library.tracks.len());
    let mut by_identity: HashMap<i64, (Incoming, Vec<i64>)> = HashMap::new();
    let mut order: Vec<i64> = Vec::new();
    let mut history = Vec::new();
    for entry in &input.library.tracks {
        matching.entries += 1;
        match index.find(entry, mappings) {
            Match::Track {
                track,
                identity,
                by_path,
            } => {
                if by_path {
                    matching.by_path += 1;
                } else {
                    matching.by_tags += 1;
                }
                matched.push(Some(identity));
                let values = incoming(entry);
                match by_identity.get_mut(&identity) {
                    Some((sum, tracks)) => {
                        sum.combine(&values);
                        tracks.push(track);
                    }
                    None => {
                        order.push(identity);
                        by_identity.insert(identity, (values, vec![track]));
                    }
                }
                history.extend(entry.history.iter().map(|l| (identity, l.clone())));
            }
            Match::Ambiguous(_) => {
                matching.ambiguous += 1;
                matched.push(None);
                if ambiguous.len() < LISTED {
                    ambiguous.push(describe(entry));
                }
            }
            Match::None => {
                matching.unmatched += 1;
                matched.push(None);
                if unmatched.len() < LISTED {
                    unmatched.push(describe(entry));
                }
            }
        }
    }

    let mut totals = PlanTotals::default();
    let mut identities = Vec::with_capacity(order.len());
    let mut added_query = c.prepare_cached("SELECT added_at FROM tracks WHERE id = ?1")?;
    for identity in order {
        let Some((values, tracks)) = by_identity.remove(&identity) else {
            continue;
        };
        let (current, _) = merge::read(c, identity)?;
        let base = match rule {
            CountRule::AddNew => baseline(c, &input.source, identity)?,
            CountRule::Max => None,
        };
        let after = merge::merge(&current, &values, rule, base, input.options.replace_ratings);
        let mut changed = after != current;
        count(&mut totals, &current, &after, &values);
        if let Some(added) = values.added {
            for &track in &tracks {
                let now: Option<i64> = added_query.query_row([track], |r| r.get(0)).optional()?;
                if now.is_some_and(|n| added < n) {
                    totals.added += 1;
                    changed = true;
                }
            }
        }
        if changed {
            totals.tracks += 1;
        } else {
            totals.unchanged += 1;
        }
        identities.push((identity, values, tracks));
    }

    // History entries that are not there yet.
    let mut known =
        c.prepare_cached("SELECT 1 FROM history WHERE identity_id = ?1 AND started_at = ?2")?;
    let mut new_history = Vec::new();
    for (identity, listen) in history {
        let at = to_unix_seconds(listen.started_at);
        if known
            .query_row(rusqlite::params![identity, at], |_| Ok(()))
            .optional()?
            .is_none()
        {
            new_history.push((identity, listen));
        }
    }
    totals.history = new_history.len();

    let item = |item: &ForeignItem| -> ItemWork {
        let entry = match item {
            ForeignItem::Track(i) => input.library.tracks.get(*i),
            ForeignItem::Other(track) => Some(&**track),
        };
        let identity = match item {
            ForeignItem::Track(i) => matched.get(*i).copied().flatten(),
            ForeignItem::Other(track) => match index.find(track, mappings) {
                Match::Track { identity, .. } => Some(identity),
                _ => None,
            },
        };
        let path = entry
            .and_then(|e| e.location.as_deref())
            .and_then(|l| local_path(l, mappings))
            .map(|p| p.as_os_str().as_bytes().to_vec());
        ItemWork {
            identity,
            path,
            title: entry.and_then(|e| e.title.clone()),
            artist: entry.and_then(|e| e.artist.clone()),
            album: entry.and_then(|e| e.album.clone()),
            duration_ms: entry.and_then(|e| e.duration).map(to_millis),
        }
    };

    let mut playlists = Vec::new();
    let mut playlist_work = Vec::new();
    if !input.options.skip_playlists {
        let exists = existing_playlists(c, &input.source, input.kind == ImportRunKind::UserData)?;
        for playlist in &input.library.playlists {
            let kind = if exists.contains(&playlist.name)
                && !matches!(playlist.kind, ForeignPlaylistKind::Unmapped(_))
            {
                PlannedPlaylistKind::Exists
            } else {
                match &playlist.kind {
                    ForeignPlaylistKind::Static { items, from_rules } => {
                        let items: Vec<ItemWork> = items.iter().map(item).collect();
                        let kind = PlannedPlaylistKind::Static {
                            items: items.len(),
                            matched: items.iter().filter(|i| i.identity.is_some()).count(),
                            from_rules: *from_rules,
                        };
                        playlist_work.push((
                            playlist.name.clone(),
                            playlist.folder.clone(),
                            PlaylistWork::Static(items),
                        ));
                        kind
                    }
                    ForeignPlaylistKind::Smart(text) => match smart_query(text) {
                        Ok(query) => {
                            playlist_work.push((
                                playlist.name.clone(),
                                playlist.folder.clone(),
                                PlaylistWork::Smart(query.clone()),
                            ));
                            PlannedPlaylistKind::Smart(query)
                        }
                        Err(reason) => PlannedPlaylistKind::Unmapped(reason),
                    },
                    ForeignPlaylistKind::Unmapped(reason) => {
                        PlannedPlaylistKind::Unmapped(reason.clone())
                    }
                }
            };
            playlists.push(PlannedPlaylist {
                name: playlist.name.clone(),
                folder: playlist.folder.clone(),
                kind,
            });
        }
    }

    // Saved searches not saved already under that name.
    let mut saved_searches = Vec::new();
    {
        let mut known = c.prepare_cached("SELECT 1 FROM saved_searches WHERE name = ?1")?;
        for (name, text) in &input.library.saved_searches {
            if known.query_row([name], |_| Ok(())).optional()?.is_some() {
                continue;
            }
            if let Ok(query) = smart_query(text) {
                saved_searches.push((name.clone(), query));
            }
        }
    }
    totals.saved_searches = saved_searches.len();

    // Favourites not favourites yet.
    let mut favourites = Vec::new();
    {
        let mut known = c.prepare_cached(
            "SELECT 1 FROM playlist_items i JOIN playlists p ON p.id = i.playlist_id
             WHERE p.builtin != 0 AND i.identity_id = ?1",
        )?;
        for entry in &input.library.favourites {
            let Some(identity) = item(entry).identity else {
                continue;
            };
            if favourites.contains(&identity)
                || known
                    .query_row([identity], |_| Ok(()))
                    .optional()?
                    .is_some()
            {
                continue;
            }
            favourites.push(identity);
        }
    }
    totals.favourites = favourites.len();

    Ok(ImportPlan {
        kind: input.kind,
        player: input.player,
        source: input.source.clone(),
        matching,
        totals,
        playlists,
        ambiguous,
        unmatched,
        notes: input.library.notes.clone(),
        rule,
        replace_ratings: input.options.replace_ratings,
        work: Work {
            identities,
            playlists: playlist_work,
            history: new_history,
            saved_searches,
            favourites,
        },
    })
}

/// Adds what merging changes for one identity to `totals`.
fn count(totals: &mut PlanTotals, current: &Stats, after: &Stats, incoming: &Incoming) {
    if after.play_count > current.play_count {
        totals.play_counts += 1;
        totals.plays_added += u64::try_from(after.play_count - current.play_count).unwrap_or(0);
    }
    if after.skip_count > current.skip_count {
        totals.skip_counts += 1;
        totals.skips_added += u64::try_from(after.skip_count - current.skip_count).unwrap_or(0);
    }
    if after.rating != current.rating {
        totals.ratings += 1;
    } else if incoming.rating.is_some() && incoming.rating != current.rating {
        totals.rating_conflicts += 1;
    }
    if after.loved != current.loved {
        totals.loved += 1;
    }
    if after.first_played != current.first_played {
        totals.first_played += 1;
    }
    if after.last_played != current.last_played {
        totals.last_played += 1;
    }
}

/// `text` read as a query and printed in its canonical form; why not, when it
/// cannot be read.
pub(super) fn smart_query(text: &str) -> Result<String, String> {
    let query = crate::query::parse_query(text)
        .map_err(|e| format!("its rules could not be read as a query: {e}"))?;
    if query.problem().is_some() {
        return Err("its rules could not be read as a query".to_owned());
    }
    Ok(crate::query::print_query(&query))
}
