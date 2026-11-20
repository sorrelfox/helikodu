// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text typed into the search entry or a filter field, read with the query
//! language: whether it is a query (it has a field operator) or plain words,
//! what fails to read and where, and how the entry shows it: a query is set in
//! the query font, and the part that fails gets a wavy underline (no colour).

use gtk::pango;
use gtk::prelude::*;
use helikodu_model::query::Query;
use helikodu_service::library::{SyntaxError, SyntaxErrorKind, parse_query_lenient};

use crate::i18n::{fill, tr};

/// The class of an entry that holds a query.
const QUERY_CLASS: &str = "hk-query";

/// Text read with the query language.
#[derive(Debug, Clone)]
pub(crate) struct Reading {
    /// The query; plain words when the text cannot be read.
    pub(crate) query: Query,
    /// Why the text cannot be read, if it cannot.
    pub(crate) error: Option<SyntaxError>,
    /// Whether the text is a query rather than plain words.
    pub(crate) is_query: bool,
}

/// Whether `text` has a field operator, which makes it a query even while it
/// cannot be read.
fn has_operator(text: &str) -> bool {
    text.contains([':', '=', '<', '>', '~'])
}

/// Reads `text`.
pub(crate) fn read(text: &str) -> Reading {
    let text = text.trim();
    let parsed = parse_query_lenient(text);
    let is_query = if parsed.error.is_some() {
        has_operator(text)
    } else {
        !parsed.query.is_empty() && !parsed.query.is_plain_text()
    };
    Reading {
        query: parsed.query,
        error: parsed.error,
        is_query,
    }
}

/// What is wrong, in a sentence for the notice.
pub(crate) fn message(error: &SyntaxError) -> String {
    match error.kind() {
        SyntaxErrorKind::UnclosedQuote => tr("The quotes are not closed."),
        SyntaxErrorKind::UnclosedGroup => tr("The parenthesis is not closed."),
        SyntaxErrorKind::UnexpectedClose => tr("This parenthesis closes nothing."),
        SyntaxErrorKind::MissingOperand(op) => fill(&tr("{} needs something after it."), &[op]),
        SyntaxErrorKind::UnknownField(name) => fill(&tr("There is no field called “{}”."), &[name]),
        SyntaxErrorKind::UnknownName { prefix, name } => {
            fill(&tr("{}: does not know “{}”."), &[prefix, name])
        }
        SyntaxErrorKind::MissingOperator(field) => {
            fill(&tr("{} needs an operator such as : or =."), &[field])
        }
        SyntaxErrorKind::MissingValue(field) => fill(&tr("{} needs a value."), &[field]),
        SyntaxErrorKind::InvalidOperator { field, op } => {
            fill(&tr("{} cannot be compared with {}."), &[field, op])
        }
        SyntaxErrorKind::InvalidValue { field, .. } => {
            fill(&tr("This value does not fit {}."), &[field])
        }
        SyntaxErrorKind::InvalidRegex(_) => tr("The regular expression does not work."),
        SyntaxErrorKind::RangeOperator => tr("A range goes after : or =."),
        SyntaxErrorKind::UnknownSortKey(name)
            if helikodu_model::user_fields::current()
                .expression(name)
                .is_some() =>
        {
            fill(&tr("A computed field cannot sort the list: {}."), &[name])
        }
        SyntaxErrorKind::UnknownSortKey(name) => fill(&tr("Cannot sort by “{}”."), &[name]),
        SyntaxErrorKind::InvalidLimit => {
            tr("A limit is a number of tracks, or of min, h, MB or GB.")
        }
        SyntaxErrorKind::Duplicate(what) => fill(&tr("{}: is given twice."), &[what]),
        SyntaxErrorKind::Misplaced(what) => fill(
            &tr("{}: belongs at the top level, not in parentheses or negated."),
            &[what],
        ),
        SyntaxErrorKind::EmptyText => tr("The quotes are empty."),
        SyntaxErrorKind::TooDeep => tr("The parentheses are nested too deeply."),
        _ => error.to_string(),
    }
}

/// Shows `reading` of the text of `entry`: the query font for a query, and a
/// wavy underline under the part that fails.
pub(crate) fn mark(entry: &impl IsA<gtk::Editable>, reading: &Reading) {
    let widget = entry.as_ref().upcast_ref::<gtk::Widget>();
    if reading.is_query {
        widget.add_css_class(QUERY_CLASS);
    } else {
        widget.remove_css_class(QUERY_CLASS);
    }
    let Some(text) = entry
        .as_ref()
        .delegate()
        .and_downcast::<gtk::Text>()
        .or_else(|| entry.as_ref().clone().downcast::<gtk::Text>().ok())
    else {
        return;
    };
    let attributes = pango::AttrList::new();
    if reading.is_query
        && let Some(error) = &reading.error
    {
        // The entry's text is the trimmed text read, after any leading space.
        let content = text.text();
        let lead = content.len() - content.trim_start().len();
        let span = error.span();
        let start = lead + span.start;
        let end = (lead + span.end.max(span.start + 1)).min(content.len());
        let mut underline = pango::AttrInt::new_underline(pango::Underline::Error);
        underline.set_start_index(u32::try_from(start).unwrap_or(0));
        underline.set_end_index(u32::try_from(end).unwrap_or(u32::MAX));
        attributes.insert(underline);
    }
    text.set_attributes(Some(&attributes));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_make_a_query_and_words_do_not() {
        assert!(!read("harbour").is_query);
        assert!(!read("glass harbour").is_query);
        let good = |text: &str| {
            let reading = read(text);
            reading.is_query && reading.error.is_none()
        };
        assert!(good("genre:jazz -genre:fusion"));
        assert!(good("rating>=4"));
        let broken = read("genre:jazz year>");
        assert!(broken.is_query);
        assert!(broken.error.is_some());
        // A stray quote in plain words searches the words.
        assert!(!read("don\"t").is_query);
    }
}
