// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The steps of a generator playlist in words: the chips of its page, the
//! kinds and choices of the editor's rows, and the values of the column the
//! steps sort by.

use std::time::SystemTime;

use gtk::glib;
use helikodu_service::library::parse_query;

use super::smart_rules::Rules;
use crate::backend::{
    GeneratorSort, GeneratorSource, GeneratorStep, GeneratorSteps, GeneratorTrack, StepProblem,
};
use crate::i18n::{fill, ntr, tr};

/// The kinds of step the editor offers after the source, in the order of
/// its kind dropdown and of the "Add step" menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepKind {
    Where,
    Exclude,
    Sort,
    AtMost,
    Limit,
}

impl StepKind {
    pub(crate) const ALL: [Self; 5] = [
        Self::Where,
        Self::Exclude,
        Self::Sort,
        Self::AtMost,
        Self::Limit,
    ];

    /// Its name in the kind dropdown and the "Add step" menu.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Where => tr("Where"),
            Self::Exclude => tr("Exclude"),
            Self::Sort => tr("Sort by"),
            Self::AtMost => tr("At most"),
            Self::Limit => tr("Limit"),
        }
    }

    /// The kind of `step`.
    pub(crate) fn of(step: &GeneratorStep) -> Self {
        match step {
            GeneratorStep::Where(_) => Self::Where,
            GeneratorStep::Exclude(_) => Self::Exclude,
            GeneratorStep::Sort { .. } => Self::Sort,
            GeneratorStep::PerArtist(_) | GeneratorStep::PerAlbum(_) => Self::AtMost,
            GeneratorStep::Limit(_) | GeneratorStep::LimitMinutes(_) => Self::Limit,
        }
    }

    /// A new step of this kind; `playlist` names the first playlist an
    /// Exclude step can choose.
    pub(crate) fn new_step(self, playlist: Option<&str>) -> GeneratorStep {
        match self {
            Self::Where => GeneratorStep::Where(String::new()),
            Self::Exclude => GeneratorStep::Exclude(playlist.unwrap_or_default().to_owned()),
            Self::Sort => GeneratorStep::Sort {
                by: GeneratorSort::Rating,
                descending: true,
            },
            Self::AtMost => GeneratorStep::PerArtist(2),
            Self::Limit => GeneratorStep::Limit(50),
        }
    }
}

/// The fields a sort step sorts by, in the order of its dropdown.
pub(crate) const SORTS: [GeneratorSort; 11] = [
    GeneratorSort::Rating,
    GeneratorSort::Plays,
    GeneratorSort::Skips,
    GeneratorSort::LastPlayed,
    GeneratorSort::Added,
    GeneratorSort::Year,
    GeneratorSort::Title,
    GeneratorSort::Artist,
    GeneratorSort::Album,
    GeneratorSort::Length,
    GeneratorSort::Random,
];

/// The name of a field to sort by, in its dropdown and as a column header.
pub(crate) fn sort_label(by: GeneratorSort) -> String {
    match by {
        GeneratorSort::Rating => tr("Rating"),
        GeneratorSort::Plays => tr("Plays"),
        GeneratorSort::Skips => tr("Skips"),
        GeneratorSort::LastPlayed => tr("Last played"),
        GeneratorSort::Added => tr("Date added"),
        GeneratorSort::Year => tr("Year"),
        GeneratorSort::Title => tr("Title"),
        GeneratorSort::Artist => tr("Artist"),
        GeneratorSort::Album => tr("Album"),
        GeneratorSort::Length => tr("Length"),
        GeneratorSort::Random => tr("Random"),
    }
}

/// The directions of a sort by `by`, in words that fit the field, and
/// whether each is descending; the first is the usual one. None for random.
pub(crate) fn directions(by: GeneratorSort) -> Vec<(String, bool)> {
    let (first, second, first_descending) = match by {
        GeneratorSort::Rating => (tr("Highest first"), tr("Lowest first"), true),
        GeneratorSort::Plays | GeneratorSort::Skips => (tr("Most first"), tr("Fewest first"), true),
        GeneratorSort::LastPlayed => (tr("Longest ago first"), tr("Latest first"), false),
        GeneratorSort::Added => (tr("Latest first"), tr("Earliest first"), true),
        GeneratorSort::Year => (tr("Newest first"), tr("Oldest first"), true),
        GeneratorSort::Title | GeneratorSort::Artist | GeneratorSort::Album => {
            (tr("A to Z"), tr("Z to A"), false)
        }
        GeneratorSort::Length => (tr("Longest first"), tr("Shortest first"), true),
        GeneratorSort::Random => return Vec::new(),
    };
    vec![(first, first_descending), (second, !first_descending)]
}

/// A sort step in words, for its chip.
fn sort_words(by: GeneratorSort, descending: bool) -> String {
    match (by, descending) {
        (GeneratorSort::Rating, true) => tr("Highest rated first"),
        (GeneratorSort::Rating, false) => tr("Lowest rated first"),
        (GeneratorSort::Plays, true) => tr("Most played first"),
        (GeneratorSort::Plays, false) => tr("Least played first"),
        (GeneratorSort::Skips, true) => tr("Most skipped first"),
        (GeneratorSort::Skips, false) => tr("Least skipped first"),
        (GeneratorSort::LastPlayed, false) => tr("Longest unplayed first"),
        (GeneratorSort::LastPlayed, true) => tr("Played most lately first"),
        (GeneratorSort::Added, true) => tr("Added most lately first"),
        (GeneratorSort::Added, false) => tr("Added earliest first"),
        (GeneratorSort::Year, true) => tr("Newest first"),
        (GeneratorSort::Year, false) => tr("Oldest first"),
        (GeneratorSort::Title, false) => tr("By title, A to Z"),
        (GeneratorSort::Title, true) => tr("By title, Z to A"),
        (GeneratorSort::Artist, false) => tr("By artist, A to Z"),
        (GeneratorSort::Artist, true) => tr("By artist, Z to A"),
        (GeneratorSort::Album, false) => tr("By album, A to Z"),
        (GeneratorSort::Album, true) => tr("By album, Z to A"),
        (GeneratorSort::Length, true) => tr("Longest first"),
        (GeneratorSort::Length, false) => tr("Shortest first"),
        (GeneratorSort::Random, _) => tr("In random order"),
    }
}

/// A query in words, where the rule builder can say it in one sentence.
fn query_words(text: &str) -> Option<String> {
    let query = parse_query(text).ok()?;
    let rules = Rules::from_query(&query)?;
    if rules.any || rules.limit.is_some() || !rules.sort.is_empty() {
        return None;
    }
    match rules.nodes.as_slice() {
        [one] => Some(one.sentence()),
        _ => None,
    }
}

/// One chip of the steps line: its words, whether they are a query in mono,
/// and whether it carries the warning of a step that cannot run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chip {
    pub(crate) text: String,
    pub(crate) mono: bool,
    pub(crate) problem: bool,
}

impl Chip {
    fn words(text: String) -> Self {
        Self {
            text,
            mono: false,
            problem: false,
        }
    }

    fn query(text: &str) -> Self {
        match query_words(text) {
            Some(words) => Self::words(words),
            None => Self {
                text: text.to_owned(),
                mono: true,
                problem: false,
            },
        }
    }
}

/// The chips of `steps`, the source first; `problem` is the step that cannot
/// run (the source counted as 0).
pub(crate) fn chips(steps: &GeneratorSteps, problem: Option<&(usize, StepProblem)>) -> Vec<Chip> {
    let mut chips = vec![match &steps.source {
        GeneratorSource::Library => Chip::words(tr("From the library")),
        GeneratorSource::Playlist(name) => Chip::words(fill(&tr("From “{}”"), &[name])),
        GeneratorSource::Query(text) => {
            let mut chip = Chip::query(text);
            if !chip.mono {
                chip.text = fill(&tr("From the tracks where {}"), &[&lowered(&chip.text)]);
            }
            chip
        }
    }];
    for step in &steps.steps {
        chips.push(match step {
            GeneratorStep::Where(text) => Chip::query(text),
            GeneratorStep::Exclude(name) => Chip::words(fill(&tr("Not in “{}”"), &[name])),
            GeneratorStep::Sort { by, descending } => Chip::words(sort_words(*by, *descending)),
            GeneratorStep::PerArtist(n) => Chip::words(fill(
                &ntr("At most {} per artist", "At most {} per artist", *n),
                &[&n.to_string()],
            )),
            GeneratorStep::PerAlbum(n) => Chip::words(fill(
                &ntr("At most {} per album", "At most {} per album", *n),
                &[&n.to_string()],
            )),
            GeneratorStep::Limit(n) => Chip::words(fill(
                &ntr("{} track", "{} tracks", *n),
                &[&crate::format::count(*n)],
            )),
            GeneratorStep::LimitMinutes(n) => Chip::words(fill(
                &ntr("{} minute", "{} minutes", *n as usize),
                &[&crate::format::count(*n)],
            )),
        });
    }
    if let Some((step, problem)) = problem
        && let Some(chip) = chips.get_mut(*step)
    {
        chip.problem = true;
        chip.mono = false;
        chip.text = match problem {
            StepProblem::MissingPlaylist(_) if *step == 0 => {
                tr("From a playlist that no longer exists")
            }
            StepProblem::MissingPlaylist(_) => tr("Not in a playlist that no longer exists"),
            StepProblem::Query(_) => tr("A search that cannot be read"),
            StepProblem::Other(_) => tr("A step that cannot run"),
        };
    }
    chips
}

/// `text` with its first letter in lower case, to follow other words.
fn lowered(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The field the steps sort by last, if one sorts (random does not count):
/// the column the table adds, and whether it is descending.
pub(crate) fn sorted_by(steps: &GeneratorSteps) -> Option<(GeneratorSort, bool)> {
    steps.steps.iter().rev().find_map(|s| match s {
        GeneratorStep::Sort { by, descending } if *by != GeneratorSort::Random => {
            Some((*by, *descending))
        }
        _ => None,
    })
}

/// "9 Nov 2026".
pub(crate) fn date(time: Option<SystemTime>) -> String {
    let Some(seconds) = time
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
    else {
        return String::new();
    };
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|d| d.format("%-d %b %Y").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// The value of the sort column in the row of `track`.
pub(crate) fn sort_value(by: GeneratorSort, track: &GeneratorTrack) -> String {
    let row = &track.row;
    match by {
        GeneratorSort::Rating => row
            .rating
            .map(|r| "★".repeat(usize::from(r)))
            .unwrap_or_default(),
        GeneratorSort::Plays => row.plays.to_string(),
        GeneratorSort::Skips => track.skips.to_string(),
        GeneratorSort::LastPlayed => match track.last_played {
            Some(_) => date(track.last_played),
            None => tr("Never"),
        },
        GeneratorSort::Added => date(row.added),
        GeneratorSort::Year => row.year.map(|y| y.to_string()).unwrap_or_default(),
        GeneratorSort::Title => row.title.clone(),
        GeneratorSort::Artist => row.artist.clone(),
        GeneratorSort::Album => row.album_title.clone(),
        GeneratorSort::Length => row
            .duration
            .map(crate::format::duration)
            .unwrap_or_default(),
        GeneratorSort::Random => String::new(),
    }
}

/// The order of two tracks by `by`.
pub(crate) fn compare(
    by: GeneratorSort,
    a: &GeneratorTrack,
    b: &GeneratorTrack,
) -> std::cmp::Ordering {
    let (x, y) = (&a.row, &b.row);
    match by {
        GeneratorSort::Rating => x.rating.cmp(&y.rating),
        GeneratorSort::Plays => x.plays.cmp(&y.plays),
        GeneratorSort::Skips => a.skips.cmp(&b.skips),
        GeneratorSort::LastPlayed => a.last_played.cmp(&b.last_played),
        GeneratorSort::Added => x.added.cmp(&y.added),
        GeneratorSort::Year => x.year.cmp(&y.year),
        GeneratorSort::Title => x.title.to_lowercase().cmp(&y.title.to_lowercase()),
        GeneratorSort::Artist => x.artist.to_lowercase().cmp(&y.artist.to_lowercase()),
        GeneratorSort::Album => x
            .album_title
            .to_lowercase()
            .cmp(&y.album_title.to_lowercase()),
        GeneratorSort::Length => x.duration.cmp(&y.duration),
        GeneratorSort::Random => std::cmp::Ordering::Equal,
    }
}

/// "made 2 minutes ago": when a result was made.
pub(crate) fn made_words(made: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(made)
        .map_or(0, |d| d.as_secs());
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    match () {
        () if minutes == 0 => tr("made just now"),
        () if hours == 0 => fill(
            &ntr(
                "made {} minute ago",
                "made {} minutes ago",
                minutes as usize,
            ),
            &[&minutes.to_string()],
        ),
        () if days == 0 => fill(
            &ntr("made {} hour ago", "made {} hours ago", hours as usize),
            &[&hours.to_string()],
        ),
        () if days < 7 => fill(
            &ntr("made {} day ago", "made {} days ago", days as usize),
            &[&days.to_string()],
        ),
        () => fill(&tr("made {}"), &[&date(Some(made))]),
    }
}

/// "As made 2 days ago": the results pane's note on the last result.
pub(crate) fn as_made_words(made: SystemTime) -> String {
    // Translators: {} is "made 2 days ago" or "made 9 Nov 2026".
    fill(&tr("As {}"), &[&made_words(made)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_drawn_steps_read_as_drawn() {
        let steps: GeneratorSteps = "from library\n\
                                     where rating>=4\n\
                                     exclude playlist \"Heard this week\"\n\
                                     sort last-played\n\
                                     per-artist 2\n\
                                     per-album 1\n\
                                     limit 50"
            .parse()
            .unwrap();
        let texts: Vec<String> = chips(&steps, None).into_iter().map(|c| c.text).collect();
        assert_eq!(
            texts,
            [
                "From the library",
                "Rating is at least 4 stars",
                "Not in “Heard this week”",
                "Longest unplayed first",
                "At most 2 per artist",
                "At most 1 per album",
                "50 tracks",
            ]
        );
        let broken = chips(
            &steps,
            Some(&(2, StepProblem::MissingPlaylist("Heard this week".into()))),
        );
        assert!(broken[2].problem);
        assert_eq!(broken[2].text, "Not in a playlist that no longer exists");
        assert_eq!(sorted_by(&steps), Some((GeneratorSort::LastPlayed, false)));
    }

    #[test]
    fn a_query_the_builder_cannot_say_stays_in_mono() {
        let steps: GeneratorSteps = "from library\nwhere genre:jazz OR year<1960 artist:ana"
            .parse()
            .unwrap();
        let chip = &chips(&steps, None)[1];
        assert!(chip.mono);
    }

    #[test]
    fn every_sort_has_its_directions() {
        for by in SORTS {
            let found = directions(by);
            if by == GeneratorSort::Random {
                assert!(found.is_empty());
            } else {
                assert_eq!(found.len(), 2);
                assert_ne!(found[0].1, found[1].1);
            }
        }
        assert_eq!(
            directions(GeneratorSort::LastPlayed)[0].0,
            "Longest ago first"
        );
    }

    #[test]
    fn made_reads_in_words() {
        let now = SystemTime::now();
        assert_eq!(made_words(now), "made just now");
        assert_eq!(
            made_words(now - Duration::from_secs(150)),
            "made 2 minutes ago"
        );
        assert_eq!(
            made_words(now - Duration::from_secs(2 * 86_400 + 60)),
            "made 2 days ago"
        );
    }
}
