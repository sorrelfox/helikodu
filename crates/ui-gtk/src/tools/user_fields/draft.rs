// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A field as the editor of Your fields holds it before it is saved: whether
//! it can be saved together with the other fields, what is wrong where (in
//! words for the notice under the entry, with the characters to underline),
//! and the expression that gives its example values. Also which other fields
//! use a field, which keeps it from being removed.

use helikodu_model::expression::{self, Template};
use helikodu_model::user_fields::{
    DefinitionError, DefinitionPart, DefinitionProblem, FieldDefinition, MAX_COMPUTED_DEPTH,
    MAX_NAME_CHARS, UserField, UserFields, check_name,
};

use crate::i18n::{fill, tr};

/// What a field stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Kind {
    /// An expression of other fields.
    #[default]
    Computed,
    /// A tag of the files.
    Tag,
}

/// A field being edited.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Draft {
    /// The name the field is saved under, for one that is saved.
    pub(crate) saved: Option<String>,
    /// The name as typed.
    pub(crate) name: String,
    /// What it stands for.
    pub(crate) kind: Kind,
    /// The expression, kept while the kind is a tag.
    pub(crate) expression: String,
    /// The tag, kept while the kind is computed.
    pub(crate) tag: String,
}

impl Draft {
    /// The draft of the saved field `field`.
    pub(crate) fn of(field: &UserField) -> Self {
        let mut draft = Self {
            saved: Some(field.name.clone()),
            name: field.name.clone(),
            ..Self::default()
        };
        match &field.definition {
            FieldDefinition::Computed { expression } => {
                draft.expression.clone_from(expression);
            }
            FieldDefinition::Tag { key } => {
                draft.kind = Kind::Tag;
                draft.tag.clone_from(key);
            }
        }
        draft
    }

    /// The field it defines, under `name`.
    fn field_named(&self, name: &str) -> UserField {
        match self.kind {
            Kind::Computed => UserField::computed(name, self.expression.trim()),
            Kind::Tag => UserField::tag(name, self.tag.trim()),
        }
    }

    /// The field it defines.
    pub(crate) fn field(&self) -> UserField {
        self.field_named(self.name.trim())
    }

    /// The text of the part it is defined by.
    fn definition(&self) -> &str {
        match self.kind {
            Kind::Computed => &self.expression,
            Kind::Tag => &self.tag,
        }
    }

    /// Whether it differs from the saved field `saved` (or, for a new field,
    /// whether anything was typed).
    pub(crate) fn changed_from(&self, saved: Option<&UserField>) -> bool {
        match saved {
            Some(field) => {
                let original = Self::of(field);
                self.name.trim() != original.name
                    || self.kind != original.kind
                    || self.definition().trim() != original.definition().trim()
            }
            None => !self.name.trim().is_empty() || !self.definition().trim().is_empty(),
        }
    }
}

/// A problem for the notice under an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    /// What is wrong, in a sentence (in 500).
    pub(crate) message: String,
    /// A line under it in `t3`: the rule for names.
    pub(crate) detail: Option<String>,
    /// An expression that can be read, for "For example …".
    pub(crate) example: Option<String>,
    /// The characters to underline, from 0.
    pub(crate) span: (usize, usize),
}

/// What the editor shows for a draft.
#[derive(Debug, Clone, Default)]
pub(crate) struct Check {
    /// The problem with the name.
    pub(crate) name: Option<Notice>,
    /// The problem with the expression or the tag.
    pub(crate) definition: Option<Notice>,
    /// Whether the name or the definition is still empty.
    pub(crate) incomplete: bool,
    /// The expression of the example values, when they can be worked out.
    pub(crate) values: Option<Template>,
}

impl Check {
    /// Whether the draft can be saved.
    pub(crate) fn can_save(&self) -> bool {
        self.name.is_none() && self.definition.is_none() && !self.incomplete
    }
}

/// The rule for names, under a name that cannot be used.
fn name_rule() -> String {
    tr(
        "A name has the letters a to z and _, starts with a letter, and is not a field, a function or a word of searches.",
    )
}

/// A name nobody uses, for a draft without a name yet.
fn placeholder(stored: &[UserField]) -> String {
    let mut name = String::from("unnamed");
    while stored.iter().any(|f| f.name.eq_ignore_ascii_case(&name)) {
        name.push('x');
    }
    name
}

/// Checks `draft` with the saved fields `stored`.
pub(crate) fn check(stored: &[UserField], draft: &Draft) -> Check {
    let typed = draft.name.trim();
    let mut result = Check {
        incomplete: typed.is_empty() || draft.definition().trim().is_empty(),
        ..Check::default()
    };
    let lead = |text: &str| {
        let bytes = text.len() - text.trim_start().len();
        text[..bytes].chars().count()
    };
    let at = draft.saved.as_ref().and_then(|saved| {
        stored
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(saved))
    });
    // The name on its own, and against the other fields.
    if !typed.is_empty() {
        let taken = stored
            .iter()
            .enumerate()
            .any(|(i, f)| Some(i) != at && f.name.eq_ignore_ascii_case(typed));
        let problem = match check_name(typed) {
            Err(error) => Some(error),
            Ok(lower) if taken => Some(DefinitionError {
                name: typed.to_owned(),
                problem: DefinitionProblem::Duplicate(lower),
                part: DefinitionPart::Name,
                start: 0,
                end: typed.chars().count(),
            }),
            Ok(_) => None,
        };
        result.name = problem.map(|error| name_notice(&error, lead(&draft.name)));
    }
    // The definition, with the other fields: under the name when it can be
    // used, otherwise under a name nobody uses.
    let name = if typed.is_empty() || result.name.is_some() {
        placeholder(stored)
    } else {
        typed.to_owned()
    };
    let mut fields: Vec<UserField> = stored.to_vec();
    let index = match at {
        Some(at) => {
            fields[at] = draft.field_named(&name);
            at
        }
        None => {
            fields.push(draft.field_named(&name));
            fields.len() - 1
        }
    };
    let (_, before) = UserFields::build_lenient(stored);
    let (set, errors) = UserFields::build_lenient(&fields);
    let definition = draft.definition();
    for error in &errors {
        if fields[index].name.eq_ignore_ascii_case(&error.name) {
            if error.part == DefinitionPart::Definition
                && !definition.trim().is_empty()
                && result.definition.is_none()
            {
                result.definition = Some(definition_notice(
                    error,
                    typed_or(&name, typed),
                    draft,
                    lead(definition),
                ));
            }
            continue;
        }
        if before.iter().any(|b| b == error) {
            continue;
        }
        // Another field cannot be read with this change.
        let old = draft.saved.as_deref().unwrap_or_default();
        let renamed = matches!(
            &error.problem,
            DefinitionProblem::Expression(expression::Problem::UnknownField(n))
                if !old.is_empty() && n.eq_ignore_ascii_case(old)
        );
        if renamed {
            if result.name.is_none() {
                result.name = Some(Notice {
                    message: fill(
                        &tr("The field {} uses {}, so the name cannot change."),
                        &[&error.name, old],
                    ),
                    detail: None,
                    example: None,
                    span: (lead(&draft.name), lead(&draft.name) + typed.chars().count()),
                });
            }
        } else if result.definition.is_none() {
            result.definition = Some(Notice {
                message: fill(
                    &tr("The field {} cannot be read with this change: {}"),
                    &[&error.name, &problem_text(&error.problem, &error.name)],
                ),
                detail: None,
                example: None,
                span: (0, definition.chars().count()),
            });
        }
    }
    if result.definition.is_none() && !definition.trim().is_empty() {
        result.values = match draft.kind {
            Kind::Computed => set.expression(&name).cloned(),
            Kind::Tag => set
                .get(&name)
                .and_then(|field| match &field.definition {
                    FieldDefinition::Tag { key } => Some(key.clone()),
                    FieldDefinition::Computed { .. } => None,
                })
                .and_then(|key| Template::parse(&format!("{{tag:{key}}}")).ok()),
        };
    }
    result
}

/// The name a problem names the field by: the one typed, or the stand-in.
fn typed_or<'a>(stand_in: &'a str, typed: &'a str) -> &'a str {
    if typed.is_empty() { stand_in } else { typed }
}

/// The notice for a name that cannot be used.
fn name_notice(error: &DefinitionError, shift: usize) -> Notice {
    let message = match &error.problem {
        DefinitionProblem::BuiltIn(name) => {
            fill(&tr("{} is a field Helikodu already has."), &[name])
        }
        DefinitionProblem::Reserved(name) if expression::Function::from_name(name).is_some() => {
            fill(&tr("{} is a function of expressions."), &[name])
        }
        DefinitionProblem::Reserved(name) => fill(&tr("{} is a word of searches."), &[name]),
        DefinitionProblem::Duplicate(name) => fill(&tr("There is already a field {}."), &[name]),
        DefinitionProblem::NameTooLong => fill(
            &tr("A name has at most {} characters."),
            &[&MAX_NAME_CHARS.to_string()],
        ),
        DefinitionProblem::NameCharacter(c) if error.start == 0 && c.is_ascii_alphabetic() => {
            tr("A name starts with a letter.")
        }
        DefinitionProblem::NameCharacter(c) if error.start == 0 && *c == '_' => {
            tr("A name starts with a letter.")
        }
        DefinitionProblem::NameCharacter(c) => {
            fill(&tr("A name cannot hold “{}”."), &[&c.to_string()])
        }
        other => problem_text(other, &error.name),
    };
    Notice {
        message,
        detail: Some(name_rule()),
        example: None,
        span: (error.start + shift, error.end + shift),
    }
}

/// What is wrong with an expression or a tag, in a sentence.
fn problem_text(problem: &DefinitionProblem, name: &str) -> String {
    match problem {
        DefinitionProblem::Expression(problem) => crate::tools::problem_sentence(problem),
        DefinitionProblem::InvalidTag(tag) => fill(
            &tr("{} is no tag name: it may hold letters, digits, _ and - only."),
            &[tag],
        ),
        DefinitionProblem::EmptyTag => tr("The field needs a tag."),
        DefinitionProblem::Cycle(other) if other.eq_ignore_ascii_case(name) => {
            fill(&tr("{} uses itself."), &[name])
        }
        DefinitionProblem::Cycle(other) => {
            fill(&tr("{} uses {}, which uses {}."), &[name, other, name])
        }
        DefinitionProblem::TooDeep => fill(
            &tr("Fields use one another more than {} deep."),
            &[&MAX_COMPUTED_DEPTH.to_string()],
        ),
        DefinitionProblem::EmptyName => tr("The field needs a name."),
        other => other.to_string(),
    }
}

/// The notice for an expression or a tag that cannot be used.
fn definition_notice(error: &DefinitionError, name: &str, draft: &Draft, shift: usize) -> Notice {
    let place = |sentence: String| {
        let sentence = sentence.trim_end_matches(['.', '。']).to_owned();
        fill(
            // Translators: {} are what is wrong and a number.
            &tr("{}: at character {}."),
            &[&sentence, &(error.start + shift + 1).to_string()],
        )
    };
    let (message, example) = match &error.problem {
        DefinitionProblem::Expression(problem) => (
            place(problem_text(&error.problem, name)),
            repaired(draft.expression.trim(), problem),
        ),
        DefinitionProblem::Cycle(_) | DefinitionProblem::TooDeep => {
            (problem_text(&error.problem, name), None)
        }
        other => (problem_text(other, name), None),
    };
    // What is not closed is underlined up to the end of its braces.
    let mut end = error.end;
    if let DefinitionProblem::Expression(
        expression::Problem::UnclosedParenthesis | expression::Problem::UnclosedQuote,
    ) = &error.problem
    {
        let text = draft.expression.trim();
        end = text
            .chars()
            .enumerate()
            .skip(error.start)
            .find(|(_, c)| *c == '}')
            .map_or(text.chars().count(), |(i, _)| i)
            .max(error.end);
    }
    Notice {
        message,
        detail: None,
        example,
        span: (error.start + shift, end + shift),
    }
}

/// `text` with what `problem` says is missing put in, when that makes an
/// expression that can be read: the closing parenthesis, quote or brace.
fn repaired(text: &str, problem: &expression::Problem) -> Option<String> {
    let close = match problem {
        expression::Problem::UnclosedParenthesis => ")",
        expression::Problem::UnclosedQuote => "\"",
        expression::Problem::UnclosedField => "}",
        expression::Problem::UnclosedBracket => "]",
        _ => return None,
    };
    let mut candidates = vec![format!("{text}{close}")];
    // Before a closing brace at the end, as in `{if(a, b}`.
    if let Some(head) = text.strip_suffix('}') {
        candidates.insert(0, format!("{head}{close}}}"));
    }
    if close == ")" {
        candidates.push(format!("{text})}}"));
    }
    candidates
        .into_iter()
        .find(|candidate| Template::parse(candidate).is_ok())
}

/// The computed fields among `stored` whose expressions name the field `name`.
pub(crate) fn users_of(stored: &[UserField], name: &str) -> Vec<String> {
    stored
        .iter()
        .filter(|field| !field.name.eq_ignore_ascii_case(name))
        .filter(|field| match &field.definition {
            FieldDefinition::Computed { expression } => names_in(expression)
                .iter()
                .any(|n| n.eq_ignore_ascii_case(name)),
            FieldDefinition::Tag { .. } => false,
        })
        .map(|field| field.name.clone())
        .collect()
}

/// The names an expression reads inside its braces, outside quotes.
fn names_in(expression: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut word = String::new();
    let flush = |word: &mut String, names: &mut Vec<String>| {
        if !word.is_empty() {
            names.push(std::mem::take(word));
        }
    };
    for c in expression.chars() {
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
        if inside && (c.is_ascii_alphanumeric() || c == '_' || (c == ':' && !word.is_empty())) {
            word.push(c);
            continue;
        }
        flush(&mut word, &mut names);
        match c {
            '{' => inside = true,
            '}' => inside = false,
            '"' | '\'' if inside => quote = Some(c),
            _ => {}
        }
    }
    flush(&mut word, &mut names);
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored() -> Vec<UserField> {
        vec![
            UserField::computed("decade", "{left(year,3)}0s"),
            UserField::computed("era", "{decade or \"unknown\"}"),
            UserField::tag("mood", "MOOD"),
        ]
    }

    fn new(name: &str, expression: &str) -> Draft {
        Draft {
            name: name.into(),
            expression: expression.into(),
            ..Draft::default()
        }
    }

    #[test]
    fn a_good_field_can_be_saved_and_gives_values() {
        let check = check(&stored(), &new("decade_two", "{decade} ({mood})"));
        assert!(check.can_save(), "{check:?}");
        assert!(check.values.is_some());
        let mood = Draft::of(&stored()[2]);
        let check = super::check(&stored(), &mood);
        assert!(check.can_save());
        assert_eq!(check.values.unwrap().text(), "{tag:MOOD}");
    }

    #[test]
    fn names_that_cannot_be_used_say_why() {
        let check = check(&stored(), &new("year", "{title}"));
        let notice = check.name.unwrap();
        assert_eq!(notice.message, "year is a field Helikodu already has.");
        assert_eq!(notice.span, (0, 4));
        assert!(notice.detail.is_some());
        let taken = super::check(&stored(), &new("era", "{title}"))
            .name
            .unwrap();
        assert_eq!(taken.message, "There is already a field era.");
        let long = super::check(&stored(), &new(&"a".repeat(40), "{title}"));
        assert_eq!(
            long.name.unwrap().message,
            "A name has at most 32 characters."
        );
        let function = super::check(&stored(), &new("upper", "{title}"));
        assert_eq!(
            function.name.unwrap().message,
            "upper is a function of expressions."
        );
        assert!(!super::check(&stored(), &new("my2", "{title}")).can_save());
    }

    #[test]
    fn expressions_that_cannot_be_read_say_where_with_an_example() {
        let check = check(
            &stored(),
            &new("year", "{if(year > 1999, \"new\", \"old\"}"),
        );
        let notice = check.definition.unwrap();
        assert_eq!(
            notice.message,
            "A parenthesis is not closed: at character 4."
        );
        assert_eq!(
            notice.example.as_deref(),
            Some("{if(year > 1999, \"new\", \"old\")}")
        );
        assert_eq!(notice.span, (3, 29));
        assert!(check.values.is_none());
        let unknown = super::check(&stored(), &new("x", "a {nope}"))
            .definition
            .unwrap();
        assert_eq!(unknown.span, (2, 8));
    }

    #[test]
    fn a_field_that_comes_back_to_itself_is_reported() {
        let mut decade = Draft::of(&stored()[0]);
        decade.expression = "{era}".into();
        let notice = check(&stored(), &decade).definition.unwrap();
        assert_eq!(notice.message, "decade uses era, which uses decade.");
        let mut own = Draft::of(&stored()[0]);
        own.expression = "{decade}".into();
        assert_eq!(
            check(&stored(), &own).definition.unwrap().message,
            "decade uses itself."
        );
    }

    #[test]
    fn a_name_another_field_needs_cannot_change() {
        let mut decade = Draft::of(&stored()[0]);
        decade.name = "years".into();
        let notice = check(&stored(), &decade).name.unwrap();
        assert_eq!(
            notice.message,
            "The field era uses decade, so the name cannot change."
        );
        let mut mood = Draft::of(&stored()[2]);
        mood.name = "feel".into();
        assert!(check(&stored(), &mood).can_save());
    }

    #[test]
    fn empty_parts_wait_without_a_notice() {
        let check = check(&stored(), &Draft::default());
        assert!(check.name.is_none() && check.definition.is_none());
        assert!(!check.can_save());
        let typed = super::check(&stored(), &new("", "{left(year,3)}"));
        assert!(!typed.can_save());
        assert!(typed.values.is_some());
    }

    #[test]
    fn the_users_of_a_field_are_found() {
        assert_eq!(users_of(&stored(), "decade"), ["era"]);
        assert!(users_of(&stored(), "era").is_empty());
        let quoted = UserField::computed("x", "{concat(\"decade\", title)}");
        assert!(users_of(&[quoted], "decade").is_empty());
        assert_eq!(
            names_in("{if(tag:MOOD, mood, \"x\")} y"),
            ["if", "tag:MOOD", "mood"]
        );
    }

    #[test]
    fn changes_are_noticed() {
        let mut draft = Draft::of(&stored()[0]);
        assert!(!draft.changed_from(Some(&stored()[0])));
        draft.expression.push('x');
        assert!(draft.changed_from(Some(&stored()[0])));
        assert!(!Draft::default().changed_from(None));
    }
}
