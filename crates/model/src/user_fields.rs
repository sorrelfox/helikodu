// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fields the user defines, beside those every library has:
//!
//! - a **computed field** is named for an expression of the expression language
//!   ([`crate::expression`]): `decade` = `{left(year, 3)}0s`. Its value is the
//!   expression's text for the track. An expression may use other computed fields,
//!   as long as none of them comes back to itself;
//! - a **custom field** is a name for a tag: `mood` = the tag `MOOD`, which the
//!   library reads from a Vorbis comment `MOOD`, an ID3v2 frame `TXXX:MOOD` or an
//!   MP4 atom `----:com.apple.iTunes:MOOD` alike. It reads and writes as `tag:MOOD`
//!   does.
//!
//! The library keeps the definitions and puts them in use with [`install`]; from
//! then on [`Field::from_name`] and the templates of the expression language know
//! their names, in the query language, in naming patterns and wherever else a field
//! is named. A name may not be that of a field every library has, nor a word the
//! languages use; [`UserFields::build`] checks every definition and says what is
//! wrong where.

use std::fmt;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::expression::{self, ComputedField, Function, Template};
use crate::query::Field;

/// The longest name, in characters.
pub const MAX_NAME_CHARS: usize = 32;

/// How deeply computed fields may use one another: a field that uses one that uses
/// another is 3 deep.
pub const MAX_COMPUTED_DEPTH: usize = 8;

/// Words the query language or the expression language reads in their own way.
const RESERVED: &[&str] = &[
    "and", "or", "not", "sort", "limit", "has", "is", "tag", "random", "asc", "desc", "lyrics",
    "cover",
];

/// What a user's field stands for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldDefinition {
    /// The text an expression gives: `{left(year, 3)}0s`.
    Computed {
        /// The expression, a template of the expression language.
        expression: String,
    },
    /// A tag, by its key in upper case: `MOOD`.
    Tag {
        /// The key.
        key: String,
    },
}

/// A field the user defined: its name and what it stands for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserField {
    /// The name, in lower case.
    pub name: String,
    /// What it stands for.
    pub definition: FieldDefinition,
}

impl UserField {
    /// A computed field `name` with the expression `expression`.
    pub fn computed(name: impl Into<String>, expression: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            definition: FieldDefinition::Computed {
                expression: expression.into(),
            },
        }
    }

    /// A custom field `name` for the tag `key`.
    pub fn tag(name: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            definition: FieldDefinition::Tag { key: key.into() },
        }
    }
}

/// Which part of a definition a problem is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionPart {
    /// The name.
    Name,
    /// The expression of a computed field, or the tag of a custom field.
    Definition,
}

/// What is wrong with a definition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DefinitionProblem {
    /// The name is empty.
    EmptyName,
    /// The name has a character other than a letter from A to Z or `_`, or does not
    /// start with a letter.
    NameCharacter(char),
    /// The name is longer than [`MAX_NAME_CHARS`].
    NameTooLong,
    /// The name is that of a field every library has (or another spelling of one).
    BuiltIn(String),
    /// The name is a word the languages read in their own way, or a function.
    Reserved(String),
    /// Two definitions have the name.
    Duplicate(String),
    /// The expression cannot be read.
    Expression(expression::Problem),
    /// The tag is empty.
    EmptyTag,
    /// The tag has characters a tag key cannot have.
    InvalidTag(String),
    /// The expression comes back to the field itself, through the field named.
    Cycle(String),
    /// Computed fields use one another more than [`MAX_COMPUTED_DEPTH`] deep.
    TooDeep,
}

/// A definition that cannot be used: which field, what is wrong, and where, in
/// characters from the start of the part it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionError {
    /// The name of the field, as written.
    pub name: String,
    /// What is wrong.
    pub problem: DefinitionProblem,
    /// The part of the definition the problem is in.
    pub part: DefinitionPart,
    /// The first character of the problem, counted from 0.
    pub start: usize,
    /// The character after the problem.
    pub end: usize,
}

impl fmt::Display for DefinitionProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "the field needs a name"),
            Self::NameCharacter(c) => write!(
                f,
                "a name has only the letters a to z and _, and starts with a letter, not {c:?}"
            ),
            Self::NameTooLong => write!(f, "a name has at most {MAX_NAME_CHARS} characters"),
            Self::BuiltIn(name) => write!(f, "{name} is already a field"),
            Self::Reserved(name) => write!(f, "{name} is a word the languages use"),
            Self::Duplicate(name) => write!(f, "there is already a field {name}"),
            Self::Expression(problem) => write!(f, "{problem}"),
            Self::EmptyTag => write!(f, "the field needs a tag"),
            Self::InvalidTag(tag) => write!(
                f,
                "{tag} is no tag name: it may hold letters, digits, _ and - only"
            ),
            Self::Cycle(name) => write!(f, "the expression comes back to itself through {name}"),
            Self::TooDeep => write!(
                f,
                "computed fields use one another more than {MAX_COMPUTED_DEPTH} deep"
            ),
        }
    }
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let part = match self.part {
            DefinitionPart::Name => "name",
            DefinitionPart::Definition => "definition",
        };
        write!(
            f,
            "{}: {} (in the {part}, at character {})",
            self.name,
            self.problem,
            self.start + 1
        )
    }
}

impl std::error::Error for DefinitionError {}

/// The name `name` in lower case, if it may name a user's field.
pub fn check_name(name: &str) -> Result<String, DefinitionError> {
    let fail = |problem, start, end| DefinitionError {
        name: name.to_owned(),
        problem,
        part: DefinitionPart::Name,
        start,
        end,
    };
    let count = name.chars().count();
    if name.is_empty() {
        return Err(fail(DefinitionProblem::EmptyName, 0, 0));
    }
    for (i, c) in name.chars().enumerate() {
        let fits = c.is_ascii_alphabetic() || (c == '_' && i > 0);
        if !fits {
            return Err(fail(DefinitionProblem::NameCharacter(c), i, i + 1));
        }
    }
    if count > MAX_NAME_CHARS {
        return Err(fail(DefinitionProblem::NameTooLong, MAX_NAME_CHARS, count));
    }
    let lower = name.to_ascii_lowercase();
    if Field::built_in(&lower).is_some() {
        return Err(fail(DefinitionProblem::BuiltIn(lower), 0, count));
    }
    if RESERVED.contains(&lower.as_str()) || Function::from_name(&lower).is_some() {
        return Err(fail(DefinitionProblem::Reserved(lower), 0, count));
    }
    Ok(lower)
}

/// The tag key `key` stands for: in upper case, without a `TXXX:`,
/// `----:com.apple.iTunes:` or `tag:` in front. `None` if what is left is empty or
/// has characters other than ASCII letters, digits, `_` and `-`.
pub fn tag_key(key: &str) -> Option<String> {
    let mut key = key.trim();
    for prefix in ["txxx:", "----:com.apple.itunes:", "tag:"] {
        if key
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            key = &key[prefix.len()..];
        }
    }
    match Field::tag(key)? {
        Field::Tag(key) => Some(key),
        _ => None,
    }
}

/// What a name stands for in a template.
#[derive(Debug, Clone)]
pub(crate) enum Lookup {
    /// A field to read: one every library has, or the tag of a custom field.
    Field(Field),
    /// A computed field.
    Computed(Arc<ComputedField>),
}

/// A user's field, ready to use.
#[derive(Debug, Clone)]
enum Resolved {
    /// The tag of a custom field.
    Tag(String),
    /// A computed field.
    Computed(Arc<ComputedField>),
}

/// A set of user's fields whose definitions all fit together.
#[derive(Debug, Clone, Default)]
pub struct UserFields {
    /// The definitions in the order given, with their names in lower case.
    fields: Vec<UserField>,
    /// What each of `fields` stands for, by its index there.
    resolved: Vec<Resolved>,
}

impl UserFields {
    /// Checks `fields` and reads their expressions; the first definition that
    /// cannot be used fails the whole set.
    pub fn build(fields: &[UserField]) -> Result<Self, DefinitionError> {
        let (set, mut errors) = Self::build_lenient(fields);
        if errors.is_empty() {
            Ok(set)
        } else {
            Err(errors.remove(0))
        }
    }

    /// Checks `fields` and reads their expressions, leaving out those that cannot be
    /// used: the set of the others, and why each of those failed, in the order of
    /// `fields`.
    pub fn build_lenient(fields: &[UserField]) -> (Self, Vec<DefinitionError>) {
        let mut errors: Vec<(usize, DefinitionError)> = Vec::new();
        let mut set = Self::default();
        // Names and tags first: every computed field may use any custom field.
        let mut pending: Vec<(usize, String, &str)> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for (index, field) in fields.iter().enumerate() {
            let name = match check_name(&field.name) {
                Ok(name) => name,
                Err(error) => {
                    errors.push((index, error));
                    continue;
                }
            };
            if names.contains(&name) {
                let count = field.name.chars().count();
                errors.push((
                    index,
                    DefinitionError {
                        name: field.name.clone(),
                        problem: DefinitionProblem::Duplicate(name),
                        part: DefinitionPart::Name,
                        start: 0,
                        end: count,
                    },
                ));
                continue;
            }
            names.push(name.clone());
            match &field.definition {
                FieldDefinition::Tag { key } => match tag_key(key) {
                    Some(key) => set.push(UserField::tag(name, key.clone()), Resolved::Tag(key)),
                    None => {
                        let problem = if key.trim().is_empty() {
                            DefinitionProblem::EmptyTag
                        } else {
                            DefinitionProblem::InvalidTag(key.trim().to_owned())
                        };
                        errors.push((
                            index,
                            DefinitionError {
                                name: field.name.clone(),
                                problem,
                                part: DefinitionPart::Definition,
                                start: 0,
                                end: key.chars().count(),
                            },
                        ));
                    }
                },
                FieldDefinition::Computed { expression } => {
                    pending.push((index, name, expression));
                }
            }
        }
        // Computed fields in rounds: each round reads those whose fields are all
        // known, until no more can be read.
        loop {
            let mut progress = false;
            let mut waiting = Vec::new();
            // The computed fields not read before this round.
            let pending_names: Vec<String> = pending.iter().map(|(_, n, _)| n.clone()).collect();
            for (index, name, expression) in std::mem::take(&mut pending) {
                match set.read_computed(&name, expression) {
                    Ok(computed) => {
                        let definition = UserField::computed(name, expression);
                        set.push(definition, Resolved::Computed(computed));
                        progress = true;
                    }
                    Err(error) => waiting.push((index, name, expression, error)),
                }
            }
            let strip = |text: &str| {
                text.trim_matches(|c: char| c == '{' || c == '}' || c.is_whitespace())
                    .to_ascii_lowercase()
            };
            // What each waits for: a computed field not read yet, if any.
            let blocked: Vec<Option<String>> = waiting
                .iter()
                .map(|(_, _, expression, error)| {
                    error_text(expression, error)
                        .map(strip)
                        .filter(|text| pending_names.contains(text))
                })
                .collect();
            // Those that fail for a reason of their own cannot be waited for.
            let failed: Vec<String> = waiting
                .iter()
                .zip(&blocked)
                .filter(|(_, b)| b.is_none())
                .map(|((_, name, _, _), _)| name.clone())
                .collect();
            for ((index, name, expression, error), blocked_by) in waiting.into_iter().zip(blocked) {
                let blocked_by = blocked_by.filter(|other| !failed.contains(other));
                match blocked_by {
                    // Another computed field not read yet: try again next round.
                    Some(_) if progress => pending.push((index, name, expression)),
                    // Nothing more was read: the fields wait for each other.
                    Some(other) => errors.push((
                        index,
                        DefinitionError {
                            name: fields[index].name.clone(),
                            problem: DefinitionProblem::Cycle(other),
                            part: DefinitionPart::Definition,
                            start: error.start,
                            end: error.end,
                        },
                    )),
                    None => {
                        let problem = match error.problem {
                            expression::Problem::TooComplex
                                if error.start == 0 && error.end == 0 =>
                            {
                                DefinitionProblem::TooDeep
                            }
                            problem => DefinitionProblem::Expression(problem),
                        };
                        errors.push((
                            index,
                            DefinitionError {
                                name: fields[index].name.clone(),
                                problem,
                                part: DefinitionPart::Definition,
                                start: error.start,
                                end: error.end,
                            },
                        ));
                    }
                }
            }
            if pending.is_empty() {
                break;
            }
        }
        // The definitions in the order given.
        let order = |field: &UserField| {
            fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(&field.name))
                .unwrap_or(usize::MAX)
        };
        let mut both: Vec<(UserField, Resolved)> =
            set.fields.into_iter().zip(set.resolved).collect();
        both.sort_by_key(|(field, _)| order(field));
        let (fields_sorted, resolved) = both.into_iter().unzip();
        errors.sort_by_key(|(index, _)| *index);
        (
            Self {
                fields: fields_sorted,
                resolved,
            },
            errors.into_iter().map(|(_, error)| error).collect(),
        )
    }

    fn push(&mut self, field: UserField, resolved: Resolved) {
        self.fields.push(field);
        self.resolved.push(resolved);
    }

    /// Reads the expression of the computed field `name` with the fields read so
    /// far. A field nested too deeply fails with [`expression::Problem::TooComplex`]
    /// at 0..0.
    fn read_computed(
        &self,
        name: &str,
        expression: &str,
    ) -> Result<Arc<ComputedField>, expression::Error> {
        let template = Template::parse_with(expression, self)?;
        let depth = template.computed_depth() + 1;
        if depth > MAX_COMPUTED_DEPTH {
            return Err(expression::Error {
                problem: expression::Problem::TooComplex,
                start: 0,
                end: 0,
            });
        }
        Ok(Arc::new(ComputedField {
            name: name.to_owned(),
            template,
            depth,
        }))
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The definitions, in their order, with their names in lower case and the
    /// tags of custom fields as keys.
    pub fn definitions(&self) -> &[UserField] {
        &self.fields
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.fields
            .iter()
            .position(|field| field.name.eq_ignore_ascii_case(name))
    }

    /// The definition of the field `name`, in any case.
    pub fn get(&self, name: &str) -> Option<&UserField> {
        self.index(name).map(|i| &self.fields[i])
    }

    /// The field `name` stands for: [`Field::Computed`] for a computed field,
    /// [`Field::Tag`] for a custom field.
    pub fn field(&self, name: &str) -> Option<Field> {
        let index = self.index(name)?;
        Some(match &self.resolved[index] {
            Resolved::Tag(key) => Field::Tag(key.clone()),
            Resolved::Computed(computed) => Field::Computed(computed.name.clone()),
        })
    }

    /// The expression of the computed field `name`, read.
    pub fn expression(&self, name: &str) -> Option<&Template> {
        match &self.resolved[self.index(name)?] {
            Resolved::Computed(computed) => Some(&computed.template),
            Resolved::Tag(_) => None,
        }
    }

    /// What `name` stands for in a template: a field every library has, or one of
    /// these.
    pub(crate) fn lookup(&self, name: &str) -> Option<Lookup> {
        if let Some(field) = Field::built_in(name) {
            return Some(Lookup::Field(field));
        }
        Some(match &self.resolved[self.index(name)?] {
            Resolved::Tag(key) => Lookup::Field(Field::Tag(key.clone())),
            Resolved::Computed(computed) => Lookup::Computed(Arc::clone(computed)),
        })
    }
}

/// The text of `expression` that `error` is about.
fn error_text<'a>(expression: &'a str, error: &expression::Error) -> Option<&'a str> {
    let start = expression.char_indices().nth(error.start)?.0;
    let end = expression
        .char_indices()
        .nth(error.end)
        .map_or(expression.len(), |(i, _)| i);
    expression.get(start..end)
}

/// The fields in use.
static CURRENT: RwLock<Option<Arc<UserFields>>> = RwLock::new(None);

/// The user's fields in use: those the open library defines.
pub fn current() -> Arc<UserFields> {
    CURRENT
        .read()
        .ok()
        .and_then(|current| current.clone())
        .unwrap_or_default()
}

/// Puts `fields` in use, in place of those in use before. Templates read before
/// keep the computed fields as they were.
pub fn install(fields: UserFields) {
    let mut current = CURRENT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *current = Some(Arc::new(fields));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::FieldMap;

    fn values() -> FieldMap {
        FieldMap::default()
            .with(Field::Year, "1994")
            .with(Field::Title, "Blue")
            .with(Field::Tag("MOOD".into()), "calm")
    }

    #[test]
    fn computed_fields_evaluate_and_use_each_other() {
        let set = UserFields::build(&[
            UserField::computed("era", "{decade} ({mood})"),
            UserField::computed("Decade", "{left(year,3)}0s"),
            UserField::tag("mood", "TXXX:Mood"),
        ])
        .unwrap();
        assert_eq!(set.field("DECADE"), Some(Field::Computed("decade".into())));
        assert_eq!(set.field("mood"), Some(Field::Tag("MOOD".into())));
        let era = set.expression("era").unwrap();
        assert_eq!(era.evaluate(&values()), "1990s (calm)");
        assert_eq!(era.fields(), [Field::Year, Field::Tag("MOOD".into())]);
        let names: Vec<&str> = set.definitions().iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["era", "decade", "mood"]);
        assert_eq!(
            set.get("mood").unwrap().definition,
            FieldDefinition::Tag { key: "MOOD".into() }
        );
    }

    #[test]
    fn names_are_checked_with_their_place() {
        let error = |name: &str| {
            UserFields::build(&[UserField::computed(name, "{title}")])
                .unwrap_err()
                .problem
        };
        assert_eq!(error(""), DefinitionProblem::EmptyName);
        assert_eq!(error("year"), DefinitionProblem::BuiltIn("year".into()));
        assert_eq!(error("Length"), DefinitionProblem::BuiltIn("length".into()));
        assert_eq!(error("label"), DefinitionProblem::BuiltIn("label".into()));
        assert_eq!(error("sort"), DefinitionProblem::Reserved("sort".into()));
        assert_eq!(error("upper"), DefinitionProblem::Reserved("upper".into()));
        assert_eq!(error(&"a".repeat(40)), DefinitionProblem::NameTooLong);
        let bad = UserFields::build(&[UserField::computed("my2nd", "{title}")]).unwrap_err();
        assert_eq!(bad.problem, DefinitionProblem::NameCharacter('2'));
        assert_eq!((bad.part, bad.start, bad.end), (DefinitionPart::Name, 2, 3));
        let twice = UserFields::build(&[
            UserField::computed("x", "{title}"),
            UserField::tag("X", "MOOD"),
        ])
        .unwrap_err();
        assert_eq!(twice.problem, DefinitionProblem::Duplicate("x".into()));
    }

    #[test]
    fn expressions_and_tags_are_checked_with_their_place() {
        let bad = UserFields::build(&[UserField::computed("x", "{left(year)}")]).unwrap_err();
        assert!(matches!(
            bad.problem,
            DefinitionProblem::Expression(expression::Problem::Arguments { .. })
        ));
        assert_eq!(bad.part, DefinitionPart::Definition);
        let unknown = UserFields::build(&[UserField::computed("x", "a {nope}")]).unwrap_err();
        assert_eq!(
            unknown.problem,
            DefinitionProblem::Expression(expression::Problem::UnknownField("nope".into()))
        );
        assert_eq!((unknown.start, unknown.end), (2, 8));
        let tag = UserFields::build(&[UserField::tag("x", "my tag")]).unwrap_err();
        assert_eq!(tag.problem, DefinitionProblem::InvalidTag("my tag".into()));
        let empty = UserFields::build(&[UserField::tag("x", " ")]).unwrap_err();
        assert_eq!(empty.problem, DefinitionProblem::EmptyTag);
        assert_eq!(
            tag_key("----:com.apple.iTunes:MOOD").as_deref(),
            Some("MOOD")
        );
        assert_eq!(tag_key("tag:energy").as_deref(), Some("ENERGY"));
    }

    #[test]
    fn cycles_and_depth_are_refused() {
        let cycle = UserFields::build(&[
            UserField::computed("a", "{b}"),
            UserField::computed("b", "x{a}"),
        ])
        .unwrap_err();
        assert_eq!(cycle.problem, DefinitionProblem::Cycle("b".into()));
        assert_eq!((cycle.start, cycle.end), (0, 3));
        let broken = UserFields::build(&[
            UserField::computed("a", "{b}"),
            UserField::computed("b", "{nope}"),
        ])
        .unwrap_err();
        assert_eq!(
            broken.problem,
            DefinitionProblem::Expression(expression::Problem::UnknownField("b".into()))
        );
        let own = UserFields::build(&[UserField::computed("a", "{a}")]).unwrap_err();
        assert_eq!(own.problem, DefinitionProblem::Cycle("a".into()));
        let mut chain = vec![UserField::computed("fa", "{title}")];
        for (i, name) in ["fb", "fc", "fd", "fe", "ff", "fg", "fh", "fi"]
            .iter()
            .enumerate()
        {
            let previous = chain[i].name.clone();
            chain.push(UserField::computed(*name, format!("{{{previous}}}")));
        }
        let deep = UserFields::build(&chain).unwrap_err();
        assert_eq!(deep.name, "fi");
        assert_eq!(deep.problem, DefinitionProblem::TooDeep);
        let (set, errors) = UserFields::build_lenient(&chain);
        assert_eq!(errors.len(), 1);
        assert_eq!(set.definitions().len(), 8);
    }
}
