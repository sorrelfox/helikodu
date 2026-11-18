// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What each tag tool would do to the files of the tag editor: the table the
//! Tag tools window shows and the values "Fill" hands back to the editor.
//! Nothing here touches a file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use helikodu_service::library::tag_tools::{
    self, Case, NamePattern, Numbering, Replace, SortForms,
};

use crate::backend::TrackKey;
use crate::backend::services::TagField;
use crate::i18n::{fill, ntr, tr, trc};

/// New values for each track, field by field.
pub(crate) type ToolValues = BTreeMap<TrackKey, BTreeMap<TagField, Vec<String>>>;

/// One file as the tools see it: its values as the editor holds them now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolTrack {
    /// The track.
    pub(crate) key: TrackKey,
    /// Its file.
    pub(crate) path: PathBuf,
    /// The values, the sort fields among them by their tag names.
    pub(crate) values: BTreeMap<TagField, Vec<String>>,
    /// Values as the file stores them, where garbled text was repaired on
    /// reading.
    pub(crate) raw: BTreeMap<TagField, Vec<String>>,
}

impl ToolTrack {
    fn get(&self, field: &TagField) -> &[String] {
        self.values.get(field).map_or(&[], Vec::as_slice)
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// The tools, in the order of the left pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tool {
    FromNames,
    Number,
    Case,
    Replace,
    Repair,
    SortNames,
}

impl Tool {
    pub(crate) const ALL: [Tool; 6] = [
        Tool::FromNames,
        Tool::Number,
        Tool::Case,
        Tool::Replace,
        Tool::Repair,
        Tool::SortNames,
    ];

    /// The tool's name in the left pane and the header.
    pub(crate) fn title(self) -> String {
        match self {
            Tool::FromNames => tr("Fill tags from file names"),
            Tool::Number => tr("Number the tracks"),
            Tool::Case => tr("Change capital letters"),
            Tool::Replace => tr("Find and replace"),
            Tool::Repair => tr("Repair garbled text"),
            Tool::SortNames => tr("Create sort names"),
        }
    }

    /// A short name for actions.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Tool::FromNames => "from-names",
            Tool::Number => "number",
            Tool::Case => "case",
            Tool::Replace => "replace",
            Tool::Repair => "repair",
            Tool::SortNames => "sort-names",
        }
    }

    /// The tool named `id`.
    pub(crate) fn from_id(id: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.id() == id)
    }

    /// The primary button's label for `n` files.
    pub(crate) fn action(self, n: usize) -> String {
        let text = match self {
            Tool::FromNames | Tool::SortNames => ntr("Fill {} file", "Fill {} files", n),
            Tool::Number => ntr("Number {} file", "Number {} files", n),
            Tool::Case => ntr("Change {} file", "Change {} files", n),
            Tool::Replace => ntr("Replace in {} file", "Replace in {} files", n),
            Tool::Repair => ntr("Repair {} file", "Repair {} files", n),
        };
        fill(&text, &[&n.to_string()])
    }
}

/// The text fields Change capital letters, Find and replace and Repair
/// garbled text work on.
pub(crate) const TEXT_FIELDS: [TagField; 7] = [
    TagField::Title,
    TagField::Artist,
    TagField::AlbumArtist,
    TagField::Album,
    TagField::Composer,
    TagField::Genre,
    TagField::Comment,
];

/// The fields that get sort names, with the sort field of each.
pub(crate) fn sort_fields() -> [(TagField, TagField); 3] {
    [
        (TagField::Artist, TagField::Custom("ARTISTSORT".into())),
        (
            TagField::AlbumArtist,
            TagField::Custom("ALBUMARTISTSORT".into()),
        ),
        (TagField::Composer, TagField::Custom("COMPOSERSORT".into())),
    ]
}

/// The form field of a pattern field.
pub(crate) fn pattern_field(name: &str) -> Option<TagField> {
    Some(match name {
        "track" => TagField::Track,
        "disc" => TagField::Disc,
        "title" => TagField::Title,
        "artist" => TagField::Artist,
        "albumartist" => TagField::AlbumArtist,
        "album" => TagField::Album,
        "year" => TagField::Year,
        "genre" => TagField::Genre,
        "composer" => TagField::Composer,
        "comment" => TagField::Comment,
        _ => return None,
    })
}

/// The name of a field as a column shows it.
pub(crate) fn field_name(field: &TagField) -> String {
    match field {
        TagField::Title => tr("Title"),
        TagField::Artist => tr("Artist"),
        TagField::AlbumArtist => tr("Album artist"),
        TagField::Album => tr("Album"),
        TagField::Track => tr("Track number"),
        TagField::TrackTotal => tr("Total"),
        TagField::Disc => tr("Disc"),
        TagField::DiscTotal => tr("Discs"),
        TagField::Year => tr("Year"),
        TagField::Genre => tr("Genre"),
        TagField::Composer => tr("Composer"),
        TagField::Comment => tr("Comment"),
        TagField::Custom(name) => match name.as_str() {
            "ARTISTSORT" => tr("Artist sort name"),
            "ALBUMARTISTSORT" => tr("Album artist sort name"),
            "COMPOSERSORT" => tr("Composer sort name"),
            other => other.to_owned(),
        },
    }
}

/// The choices of the tools; they last while the window is open.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Options {
    pub(crate) pattern: String,
    /// Pattern fields not filled.
    pub(crate) unfilled: BTreeSet<String>,
    pub(crate) only_empty: bool,
    pub(crate) first: u32,
    pub(crate) per_disc: bool,
    pub(crate) totals: bool,
    pub(crate) case: Case,
    pub(crate) case_fields: BTreeSet<TagField>,
    pub(crate) find: String,
    pub(crate) replace: String,
    pub(crate) regular: bool,
    pub(crate) match_case: bool,
    pub(crate) replace_fields: BTreeSet<TagField>,
    /// The charset chosen for garbled text; `None` for the detected one.
    pub(crate) charset: Option<String>,
    pub(crate) forms: SortForms,
    pub(crate) sort_fields: BTreeSet<TagField>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pattern: "{track} - {artist} - {title}".into(),
            unfilled: BTreeSet::new(),
            only_empty: false,
            first: 1,
            per_disc: true,
            totals: true,
            case: Case::Title,
            case_fields: [TagField::Title].into_iter().collect(),
            find: String::new(),
            replace: String::new(),
            regular: false,
            match_case: false,
            replace_fields: [TagField::Title].into_iter().collect(),
            charset: None,
            forms: SortForms {
                article_last: true,
                surname_first: false,
            },
            sort_fields: [TagField::Artist, TagField::AlbumArtist, TagField::Composer]
                .into_iter()
                .collect(),
        }
    }
}

/// One cell of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell {
    /// A value that changes the tag; `old` is shown beside it in `t3`.
    New { new: String, old: Option<String> },
    /// A value the tag already holds, or one not written.
    Kept(String),
    /// Nothing.
    Blank,
}

/// One row: the file name, then a cell per column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) file: String,
    pub(crate) cells: Vec<Cell>,
    /// A note in place of the cells ("Does not fit the pattern").
    pub(crate) note: Option<String>,
}

/// A column after the file name: its title, its width (-1 to share the
/// rest) and whether it is right-aligned.
pub(crate) type Column = (String, i32, bool);

/// What a tool would do.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Preview {
    pub(crate) columns: Vec<Column>,
    pub(crate) rows: Vec<Row>,
    /// The values "Fill" hands to the editor.
    pub(crate) values: ToolValues,
    /// The footer's summary.
    pub(crate) summary: String,
    /// Why the tool cannot run with these options.
    pub(crate) problem: Option<String>,
    /// The charset detected for garbled text.
    pub(crate) detected: Option<&'static str>,
}

impl Preview {
    /// The files that would change.
    pub(crate) fn files(&self) -> usize {
        self.values.values().filter(|f| !f.is_empty()).count()
    }
}

const NUMBER_WIDTH: i32 = 64;

fn text_column(field: &TagField) -> Column {
    let number = matches!(
        field,
        TagField::Track | TagField::TrackTotal | TagField::Disc | TagField::DiscTotal
    );
    (
        if *field == TagField::Track {
            trc("track number", "Track")
        } else {
            field_name(field)
        },
        if number { NUMBER_WIDTH } else { -1 },
        number,
    )
}

/// What `tool` with `options` would do to `tracks`.
pub(crate) fn preview(tool: Tool, tracks: &[ToolTrack], options: &Options) -> Preview {
    let mut preview = match tool {
        Tool::FromNames => from_names(tracks, options),
        Tool::Number => numbering(tracks, options),
        Tool::Case => per_field(tracks, &options.case_fields, |text| {
            Some(tag_tools::change_case(text, options.case))
        }),
        Tool::Replace => replace(tracks, options),
        Tool::Repair => repair(tracks, options),
        Tool::SortNames => sort_names(tracks, options),
    };
    if preview.summary.is_empty() && preview.problem.is_none() {
        let n = preview.files();
        preview.summary = fill(
            &ntr(
                "{} file changes · the old values are in grey",
                "{} files change · the old values are in grey",
                n,
            ),
            &[&n.to_string()],
        );
    }
    preview
}

fn from_names(tracks: &[ToolTrack], options: &Options) -> Preview {
    let pattern = match NamePattern::parse(&options.pattern) {
        Ok(pattern) => pattern,
        Err(e) => {
            return Preview {
                problem: Some(e.to_string()),
                summary: e.to_string(),
                ..Preview::default()
            };
        }
    };
    let fields: Vec<(String, TagField)> = pattern
        .fields()
        .into_iter()
        .filter_map(|name| Some((name.to_owned(), pattern_field(name)?)))
        .collect();
    let mut preview = Preview {
        columns: fields.iter().map(|(_, f)| text_column(f)).collect(),
        ..Preview::default()
    };
    let mut fit = 0;
    for track in tracks {
        let Some(found) = pattern.values(&track.path) else {
            preview.rows.push(Row {
                file: track.file_name(),
                cells: vec![Cell::Blank; fields.len()],
                note: Some(tr("Does not fit the pattern")),
            });
            continue;
        };
        fit += 1;
        let mut cells = Vec::new();
        let mut writes = BTreeMap::new();
        for (name, field) in &fields {
            let value = found
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let now = track.get(field);
            let held = now.len() == 1 && now[0] == value || now.is_empty() && value.is_empty();
            let wanted = !options.unfilled.contains(name)
                && !(options.only_empty && !now.is_empty())
                && !held;
            if wanted {
                writes.insert(field.clone(), split_people(field, &value));
                cells.push(Cell::New {
                    new: value,
                    old: None,
                });
            } else {
                cells.push(Cell::Kept(value));
            }
        }
        if !writes.is_empty() {
            preview.values.insert(track.key, writes);
        }
        preview.rows.push(Row {
            file: track.file_name(),
            cells,
            note: None,
        });
    }
    preview.summary = fill(
        &ntr(
            // Translators: {0} is the number of file names that fit, which
            // chooses the form, {1} the number of all files.
            "{} of {} file names fits the pattern · values in grey are already in the tags",
            "{} of {} file names fit the pattern · values in grey are already in the tags",
            fit,
        ),
        &[&fit.to_string(), &tracks.len().to_string()],
    );
    preview
}

/// A value typed for a field of people, split as the editor splits it.
fn split_people(field: &TagField, value: &str) -> Vec<String> {
    match field {
        TagField::Artist | TagField::AlbumArtist | TagField::Composer => value
            .split(';')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect(),
        _ if value.trim().is_empty() => Vec::new(),
        _ => vec![value.trim().to_owned()],
    }
}

fn numbering(tracks: &[ToolTrack], options: &Options) -> Preview {
    let discs: Vec<Option<u32>> = tracks
        .iter()
        .map(|t| t.get(&TagField::Disc).first().and_then(|d| d.parse().ok()))
        .collect();
    let numbers = tag_tools::number_tracks(
        &discs,
        Numbering {
            first: options.first,
            per_disc: options.per_disc,
        },
    );
    let mut fields = vec![TagField::Track];
    if options.totals {
        fields.push(TagField::TrackTotal);
    }
    let mut preview = Preview {
        columns: fields.iter().map(text_column).collect(),
        ..Preview::default()
    };
    for (track, (number, total)) in tracks.iter().zip(numbers) {
        let mut cells = Vec::new();
        let mut writes = BTreeMap::new();
        for field in &fields {
            let new = if *field == TagField::Track {
                number
            } else {
                total
            }
            .to_string();
            let old = track.get(field).join("; ");
            if old == new {
                cells.push(Cell::Kept(new));
            } else {
                writes.insert(field.clone(), vec![new.clone()]);
                cells.push(Cell::New {
                    new,
                    old: Some(old),
                });
            }
        }
        if !writes.is_empty() {
            preview.values.insert(track.key, writes);
        }
        preview.rows.push(Row {
            file: track.file_name(),
            cells,
            note: None,
        });
    }
    preview
}

/// A tool that changes each value of `fields` with `change`.
fn per_field(
    tracks: &[ToolTrack],
    fields: &BTreeSet<TagField>,
    change: impl Fn(&str) -> Option<String>,
) -> Preview {
    let fields: Vec<TagField> = TEXT_FIELDS
        .iter()
        .filter(|f| fields.contains(f))
        .cloned()
        .collect();
    let mut preview = Preview {
        columns: fields.iter().map(text_column).collect(),
        ..Preview::default()
    };
    for track in tracks {
        let mut cells = Vec::new();
        let mut writes = BTreeMap::new();
        for field in &fields {
            let old = track.get(field);
            let new: Vec<String> = old
                .iter()
                .map(|v| change(v).unwrap_or_else(|| v.clone()))
                .collect();
            if new == old {
                cells.push(Cell::Kept(old.join("; ")));
            } else {
                cells.push(Cell::New {
                    new: new.join("; "),
                    old: Some(old.join("; ")),
                });
                writes.insert(field.clone(), new);
            }
        }
        if !writes.is_empty() {
            preview.values.insert(track.key, writes);
        }
        preview.rows.push(Row {
            file: track.file_name(),
            cells,
            note: None,
        });
    }
    preview
}

fn replace(tracks: &[ToolTrack], options: &Options) -> Preview {
    if options.find.is_empty() {
        let mut preview = per_field(tracks, &options.replace_fields, |_| None);
        preview.summary = tr("Type the text to find.");
        return preview;
    }
    match Replace::new(
        &options.find,
        &options.replace,
        options.regular,
        options.match_case,
    ) {
        Ok(replace) => per_field(tracks, &options.replace_fields, |text| {
            Some(replace.apply(text))
        }),
        Err(_) => Preview {
            problem: Some(tr("This is not a regular expression the search can use.")),
            summary: tr("This is not a regular expression the search can use."),
            ..Preview::default()
        },
    }
}

fn repair(tracks: &[ToolTrack], options: &Options) -> Preview {
    // Every text value as the file stores it, so that the charset is guessed
    // from all of them together.
    let mut places: Vec<(usize, TagField, usize)> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for (t, track) in tracks.iter().enumerate() {
        for field in TEXT_FIELDS {
            let values = track
                .raw
                .get(&field)
                .map_or_else(|| track.get(&field), Vec::as_slice);
            for (i, value) in values.iter().enumerate() {
                places.push((t, field.clone(), i));
                texts.push(value.clone());
            }
        }
    }
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let found = tag_tools::repair_garbled(&refs, options.charset.as_deref());
    let mut preview = Preview {
        columns: vec![(tr("Field"), 150, false), (tr("Value"), -1, false)],
        detected: found.charset,
        ..Preview::default()
    };
    let mut changed: BTreeMap<(usize, TagField), Vec<String>> = BTreeMap::new();
    for ((t, field, i), repaired) in places.into_iter().zip(found.repaired) {
        let Some(repaired) = repaired else { continue };
        let track = &tracks[t];
        let entry = changed.entry((t, field.clone())).or_insert_with(|| {
            track
                .raw
                .get(&field)
                .map_or_else(|| track.get(&field), Vec::as_slice)
                .to_vec()
        });
        if let Some(slot) = entry.get_mut(i) {
            *slot = repaired;
        }
    }
    for ((t, field), new) in changed {
        let track = &tracks[t];
        let old = track
            .raw
            .get(&field)
            .map_or_else(|| track.get(&field), Vec::as_slice);
        if new.as_slice() == old {
            continue;
        }
        preview.rows.push(Row {
            file: track.file_name(),
            cells: vec![
                Cell::Kept(field_name(&field)),
                Cell::New {
                    new: new.join("; "),
                    old: Some(old.join("; ")),
                },
            ],
            note: None,
        });
        preview
            .values
            .entry(track.key)
            .or_default()
            .insert(field, new);
    }
    if preview.rows.is_empty() {
        preview.summary = tr("No garbled text was found in these files.");
    }
    preview
}

fn sort_names(tracks: &[ToolTrack], options: &Options) -> Preview {
    let pairs: Vec<(TagField, TagField)> = sort_fields()
        .into_iter()
        .filter(|(f, _)| options.sort_fields.contains(f))
        .collect();
    let mut preview = Preview {
        columns: pairs.iter().map(|(_, sort)| text_column(sort)).collect(),
        ..Preview::default()
    };
    for track in tracks {
        let mut cells = Vec::new();
        let mut writes = BTreeMap::new();
        for (field, sort) in &pairs {
            let existing = track.get(sort);
            if !existing.is_empty() {
                cells.push(Cell::Kept(existing.join("; ")));
                continue;
            }
            let names = track.get(field);
            let made: Vec<String> = names
                .iter()
                .map(|n| tag_tools::sort_name(n, options.forms).unwrap_or_else(|| n.clone()))
                .collect();
            if made.is_empty() || made.as_slice() == names {
                cells.push(Cell::Blank);
            } else {
                cells.push(Cell::New {
                    new: made.join("; "),
                    old: None,
                });
                writes.insert(sort.clone(), made);
            }
        }
        if !writes.is_empty() {
            preview.values.insert(track.key, writes);
        }
        preview.rows.push(Row {
            file: track.file_name(),
            cells,
            note: None,
        });
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(key: u64, name: &str, values: &[(TagField, &str)]) -> ToolTrack {
        ToolTrack {
            key: TrackKey(key),
            path: PathBuf::from(format!("/music/Okto/{name}")),
            values: values
                .iter()
                .map(|(f, v)| (f.clone(), vec![(*v).to_owned()]))
                .collect(),
            raw: BTreeMap::new(),
        }
    }

    #[test]
    fn file_names_fill_only_what_changes() {
        let tracks = [
            track(1, "01 - Okto - Departure.flac", &[]),
            track(
                2,
                "02 - Okto - Night Ferry.flac",
                &[
                    (TagField::Track, "2"),
                    (TagField::Artist, "Okto"),
                    (TagField::Title, "Night Ferry"),
                ],
            ),
            track(3, "bonus_open_water_live.flac", &[]),
        ];
        let p = preview(Tool::FromNames, &tracks, &Options::default());
        assert_eq!(p.columns.len(), 3);
        assert_eq!(p.files(), 1, "the second already holds its values");
        assert_eq!(
            p.values[&TrackKey(1)][&TagField::Title],
            vec!["Departure".to_owned()]
        );
        assert!(matches!(p.rows[1].cells[0], Cell::Kept(_)));
        assert!(p.rows[2].note.is_some());
        assert!(p.summary.starts_with("2 of 3"));

        let only_artist = Options {
            unfilled: ["track".into(), "title".into()].into_iter().collect(),
            ..Options::default()
        };
        let p = preview(Tool::FromNames, &tracks, &only_artist);
        assert_eq!(p.values[&TrackKey(1)].len(), 1);

        let bad = Options {
            pattern: "{title".into(),
            ..Options::default()
        };
        assert!(preview(Tool::FromNames, &tracks, &bad).problem.is_some());
    }

    #[test]
    fn numbering_restarts_per_disc_and_writes_totals() {
        let tracks = [
            track(1, "a.flac", &[(TagField::Disc, "1")]),
            track(
                2,
                "b.flac",
                &[(TagField::Disc, "1"), (TagField::Track, "2")],
            ),
            track(3, "c.flac", &[(TagField::Disc, "2")]),
        ];
        let p = preview(Tool::Number, &tracks, &Options::default());
        assert_eq!(p.values[&TrackKey(1)][&TagField::Track], vec!["1"]);
        assert_eq!(p.values[&TrackKey(2)].get(&TagField::Track), None);
        assert_eq!(p.values[&TrackKey(2)][&TagField::TrackTotal], vec!["2"]);
        assert_eq!(p.values[&TrackKey(3)][&TagField::TrackTotal], vec!["1"]);
    }

    #[test]
    fn case_replace_and_sort_names() {
        let tracks = [track(
            1,
            "a.flac",
            &[
                (TagField::Title, "song of the sea"),
                (TagField::Artist, "The Beatles"),
            ],
        )];
        let p = preview(Tool::Case, &tracks, &Options::default());
        assert_eq!(
            p.values[&TrackKey(1)][&TagField::Title],
            vec!["Song of the Sea"]
        );

        let mut options = Options {
            find: "sea".into(),
            replace: "Ocean".into(),
            ..Options::default()
        };
        let p = preview(Tool::Replace, &tracks, &options);
        assert_eq!(
            p.values[&TrackKey(1)][&TagField::Title],
            vec!["song of the Ocean"]
        );
        options.regular = true;
        options.find = "(".into();
        assert!(preview(Tool::Replace, &tracks, &options).problem.is_some());

        let p = preview(Tool::SortNames, &tracks, &Options::default());
        assert_eq!(
            p.values[&TrackKey(1)][&TagField::Custom("ARTISTSORT".into())],
            vec!["Beatles, The"]
        );
    }

    #[test]
    fn garbled_text_uses_the_stored_form() {
        let mut broken = track(1, "a.flac", &[(TagField::Title, "Кино")]);
        let latin1: String = [0xCA_u8, 0xE8, 0xED, 0xEE]
            .iter()
            .map(|&b| char::from(b))
            .collect();
        broken.raw.insert(TagField::Title, vec![latin1]);
        let p = preview(Tool::Repair, &[broken], &Options::default());
        assert_eq!(p.detected, Some("windows-1251"));
        assert_eq!(p.values[&TrackKey(1)][&TagField::Title], vec!["Кино"]);
        let clean = track(2, "b.flac", &[(TagField::Title, "Plain")]);
        assert!(
            preview(Tool::Repair, &[clean], &Options::default())
                .rows
                .is_empty()
        );
    }
}
