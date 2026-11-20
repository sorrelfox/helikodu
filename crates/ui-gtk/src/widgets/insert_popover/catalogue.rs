// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Insert popover offers and how a choice goes into the text: the
//! fields of the expression language with what each holds, the user's fields,
//! the functions with an example each, the search over them, and the text a
//! choice inserts at the cursor (in braces or not) with the part to select.

use helikodu_model::expression::Function;
use helikodu_model::query::Field;
use helikodu_model::user_fields::{FieldDefinition, UserField};

use crate::i18n::{fill, tr};

/// One row of the popover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    /// What the row shows in mono: a field's name or a function's call.
    pub(crate) name: String,
    /// What it is, in a sentence.
    pub(crate) description: String,
    /// An example, for a function.
    pub(crate) example: String,
    /// What goes into the text.
    pub(crate) insert: Insert,
}

/// What a choice puts into the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Insert {
    /// A field by its name.
    Field(String),
    /// A function call as written, such as `left(text, n)`.
    Function(String),
}

/// The fields every library has, in the popover's order: those a naming
/// pattern uses most first, then the others in the order of the query
/// language.
pub(crate) fn fields() -> Vec<Choice> {
    let first = [
        Field::AlbumArtist,
        Field::Album,
        Field::Artist,
        Field::Title,
        Field::Track,
        Field::Disc,
        Field::Year,
    ];
    let rest = Field::named().filter(|field| !first.contains(field));
    first
        .iter()
        .cloned()
        .chain(rest)
        .map(|field| {
            let name = field.name().into_owned();
            Choice {
                description: describe(&field),
                example: String::new(),
                insert: Insert::Field(name.clone()),
                name,
            }
        })
        .collect()
}

/// What a field holds, as the popover says it.
fn describe(field: &Field) -> String {
    match field {
        Field::AlbumArtist => tr("Album artist"),
        Field::Album => tr("Album title"),
        Field::Artist => tr("Track artist"),
        Field::Title => tr("Title"),
        Field::Track => tr("Track number, with two digits"),
        Field::Disc => tr("Disc number, on albums of several discs"),
        Field::Year => tr("Year of the release"),
        Field::Composer => tr("Composer"),
        Field::Performer => tr("Performer"),
        Field::Conductor => tr("Conductor"),
        Field::Lyricist => tr("Lyricist"),
        Field::Genre => tr("Genre"),
        Field::Work => tr("Classical work"),
        Field::Movement => tr("Movement name"),
        Field::DiscSubtitle => tr("Disc title"),
        Field::ReleaseType => tr("Release type"),
        Field::Codec => tr("Codec"),
        Field::Container => tr("Container"),
        Field::Path => tr("Whole path of the file"),
        Field::Folder => tr("Folder of the file"),
        Field::Filename => tr("Name of the file"),
        Field::Kind => tr("Music, audiobook, podcast or video"),
        Field::OriginalYear => tr("Year of the original release"),
        Field::MovementNumber => tr("Movement number"),
        Field::SampleRate => tr("Sample rate in Hz"),
        Field::BitDepth => tr("Bits per sample"),
        Field::Channels => tr("Number of channels"),
        Field::Bitrate => tr("Bit rate in kbit/s"),
        Field::TrackGain => tr("ReplayGain track gain in dB"),
        Field::AlbumGain => tr("ReplayGain album gain in dB"),
        Field::Plays => tr("How often it was played"),
        Field::Skips => tr("How often it was skipped"),
        Field::Rating => tr("Rating in stars"),
        Field::Duration => tr("Length in seconds"),
        Field::Resume => tr("Where listening stopped, in seconds"),
        Field::Size => tr("Size of the file in bytes"),
        Field::Added => tr("When the track came into the library"),
        Field::Modified => tr("When its tags last changed"),
        Field::LastPlayed => tr("When it was last played"),
        Field::FirstPlayed => tr("When it was first played"),
        Field::LastSkipped => tr("When it was last skipped"),
        Field::Label => tr("A label of the track, its album or an artist"),
        _ => tr("A tag"),
    }
}

/// The user's fields, in their order.
pub(crate) fn user_fields(fields: &[UserField]) -> Vec<Choice> {
    fields
        .iter()
        .map(|field| Choice {
            name: field.name.clone(),
            description: user_field_line(field),
            example: String::new(),
            insert: Insert::Field(field.name.clone()),
        })
        .collect()
}

/// What a user's field stands for, in a line: "Computed · {left(year,3)}0s" or
/// "The tag MOOD".
pub(crate) fn user_field_line(field: &UserField) -> String {
    match &field.definition {
        FieldDefinition::Computed { expression } => {
            // Translators: {} is the expression of a field the user defined.
            fill(&tr("Computed · {}"), &[expression])
        }
        FieldDefinition::Tag { key } => fill(&tr("The tag {}"), &[key]),
    }
}

/// A function's call as the popover shows it, an example, and what it gives.
fn function_help(function: Function) -> Option<(&'static str, &'static str, String)> {
    Some(match function {
        Function::If => (
            "if(test, then, else)",
            "{if(composer, composer, artist)}",
            tr("then when test is not empty, otherwise else"),
        ),
        Function::Concat => (
            "concat(a, b, …)",
            "{concat(artist, \" - \", title)}",
            tr("The values one after another"),
        ),
        Function::Upper => ("upper(text)", "{upper(album)}", tr("text in capitals")),
        Function::Lower => ("lower(text)", "{lower(genre)}", tr("text in small letters")),
        Function::Left => (
            "left(text, n)",
            "{left(year, 3)}0s",
            tr("The first n characters"),
        ),
        Function::Right => (
            "right(text, n)",
            "{right(year, 2)}",
            tr("The last n characters"),
        ),
        Function::Substr => (
            "substr(text, start, length)",
            "{substr(title, 1, 12)}",
            tr("The characters from start on (the first is 1), all or length of them"),
        ),
        Function::Len => ("len(text)", "{len(title)}", tr("The number of characters")),
        Function::Trim => (
            "trim(text)",
            "{trim(album)}",
            tr("text without spaces at either end"),
        ),
        Function::Replace => (
            "replace(text, from, to)",
            "{replace(title, \":\", \" -\")}",
            tr("text with every from replaced by to"),
        ),
        Function::Pad => (
            "pad(text, width, char)",
            "{pad(artist, 20)}",
            tr("text filled at the end with spaces or char to width characters"),
        ),
        Function::Num => (
            "num(number, width)",
            "{num(raw(track), 3)}",
            tr("A whole number with zeros in front"),
        ),
        Function::FirstWord => (
            "firstword(text)",
            "{firstword(artist)}",
            tr("The first word"),
        ),
        Function::SortName => (
            "sortname(text)",
            "{sortname(albumartist)}",
            tr("“The Linden Quartet” as “Linden Quartet, The”"),
        ),
        Function::NoArticle => (
            "noarticle(text)",
            "{noarticle(albumartist)}",
            tr("“The Linden Quartet” as “Linden Quartet”"),
        ),
        Function::Initial => (
            "initial(text)",
            "{initial(albumartist)}",
            tr("The first letter in capitals, or # for a digit"),
        ),
        Function::Year => ("year(date)", "{year(added)}", tr("The year of a date")),
        Function::Month => (
            "month(date)",
            "{month(added)}",
            tr("The month of a date, two digits"),
        ),
        Function::Day => (
            "day(date)",
            "{day(added)}",
            tr("The day of a date, two digits"),
        ),
        Function::Date => (
            "date(date)",
            "{date(lastplayed)}",
            tr("The date without its time"),
        ),
        Function::Time => (
            "time(seconds)",
            "{time(duration)}",
            tr("A number of seconds as 3:45"),
        ),
        Function::Join => (
            "join(field, separator)",
            "{join(composer, \" & \")}",
            tr("Every value of a field, with separator between them"),
        ),
        Function::First => (
            "first(field)",
            "{first(genre)}",
            tr("The first value of a field of several values"),
        ),
        Function::Last => (
            "last(field)",
            "{last(genre)}",
            tr("The last value of a field of several values"),
        ),
        Function::Count => (
            "count(field)",
            "{count(composer)}",
            tr("How many values a field has"),
        ),
        Function::Raw => (
            "raw(field)",
            "{raw(track)}",
            tr("The value as it is stored, without two digits"),
        ),
        Function::Let => (
            "let(name, value, body)",
            "{let(a, sortname(artist), left(a, 1))}",
            tr("body, where name stands for value"),
        ),
        _ => return None,
    })
}

/// The functions, in the order of the user guide.
pub(crate) fn functions() -> Vec<Choice> {
    Function::all()
        .filter_map(function_help)
        .map(|(call, example, description)| Choice {
            name: call.to_owned(),
            description,
            example: example.to_owned(),
            insert: Insert::Function(call.to_owned()),
        })
        .collect()
}

/// Whether `choice` matches what was typed in the search: its name or its
/// description holds it, without regard to case.
pub(crate) fn matches(choice: &Choice, typed: &str) -> bool {
    let typed = typed.trim().to_lowercase();
    typed.is_empty()
        || choice.name.to_lowercase().contains(&typed)
        || choice.description.to_lowercase().contains(&typed)
}

/// Whether the place `cursor` (in characters) of `text` is inside braces: after
/// a `{` that no `}` closes yet. Braces and quotes inside quotes do not count.
pub(crate) fn inside_braces(text: &str, cursor: usize) -> bool {
    let mut inside = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in text.chars().take(cursor) {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '{' => inside = true,
            '}' => inside = false,
            '"' | '\'' if inside => quote = Some(c),
            _ => {}
        }
    }
    inside
}

/// The text `insert` puts at `cursor` (in characters) of `text`, and the part
/// of it to select afterwards, in characters from its start: a field as
/// `{name}`, or `name` inside braces; a function as its call, in braces when
/// the cursor is not inside them, with its first argument selected.
pub(crate) fn insertion(
    text: &str,
    cursor: usize,
    insert: &Insert,
) -> (String, Option<(usize, usize)>) {
    let inside = inside_braces(text, cursor);
    let (body, select) = match insert {
        Insert::Field(name) => (name.clone(), None),
        Insert::Function(call) => {
            let select = call.find('(').map(|open| {
                let after = &call[open + 1..];
                let length = after.find([',', ')']).unwrap_or(after.len());
                let start = call[..=open].chars().count();
                (start, start + after[..length].chars().count())
            });
            (call.clone(), select.filter(|(start, end)| end > start))
        }
    };
    if inside {
        (body, select)
    } else {
        (
            format!("{{{body}}}"),
            select.map(|(start, end)| (start + 1, end + 1)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braces_are_found_outside_quotes() {
        assert!(!inside_braces("{album}/", 8));
        assert!(inside_braces("{album}/{", 9));
        assert!(inside_braces("{left(year, ", 12));
        assert!(!inside_braces("{album}", 0));
        assert!(inside_braces("{album}", 3));
        // A brace inside quotes closes nothing.
        assert!(inside_braces("{replace(title, \"}\", ", 21));
        // Outside braces a quote is text like any other.
        assert!(inside_braces("a\"{b", 4));
    }

    #[test]
    fn a_choice_goes_in_braces_unless_it_is_inside_them() {
        let field = Insert::Field("title".into());
        assert_eq!(insertion("{album}/", 8, &field), ("{title}".into(), None));
        assert_eq!(insertion("{if(", 4, &field), ("title".into(), None));
        let left = Insert::Function("left(text, n)".into());
        let (text, select) = insertion("", 0, &left);
        assert_eq!(text, "{left(text, n)}");
        assert_eq!(select, Some((6, 10)));
        assert_eq!(&text[6..10], "text");
        let (text, select) = insertion("{upper(", 7, &left);
        assert_eq!(text, "left(text, n)");
        assert_eq!(select, Some((5, 9)));
        let concat = Insert::Function("concat(a, b, …)".into());
        assert_eq!(insertion("{", 1, &concat).1, Some((7, 8)));
    }

    #[test]
    fn every_function_and_named_field_is_offered() {
        assert_eq!(functions().len(), Function::all().count());
        assert_eq!(fields().len(), Field::named().count());
        assert_eq!(fields()[0].name, "albumartist");
        // Each example can be read.
        for choice in functions() {
            assert!(
                helikodu_model::expression::Template::parse(&choice.example).is_ok(),
                "{}",
                choice.example
            );
        }
        let found: Vec<String> = functions()
            .into_iter()
            .filter(|c| matches(c, "LEFT"))
            .map(|c| c.name)
            .collect();
        assert_eq!(found, ["left(text, n)"]);
        let tag = UserField::tag("mood", "MOOD");
        assert_eq!(user_fields(&[tag])[0].description, "The tag MOOD");
    }
}
