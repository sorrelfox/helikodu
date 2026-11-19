// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The expression language: text with the values of a track's fields, as naming
//! patterns, display rows, sort keys and groups write it. One parser and one
//! evaluator serve every use, so that an expression means the same everywhere.
//!
//! A [`Template`] is text with three kinds of special parts:
//!
//! - `{…}` holds an expression; its value is written in its place. The simplest
//!   expression is a field, named as the query language names it (`{albumartist}`,
//!   `{year}`, `{track}`, `{tag:MOOD}`);
//! - text in square brackets is left out when an expression directly inside it
//!   gives nothing (`[{disc}-]{track}`); brackets nest, and an inner bracket is
//!   left out on its own;
//! - in a path template ([`Template::parse_path`]), `/` starts a folder.
//!
//! Everything else is written as it is. Inside the braces:
//!
//! ```text
//! expression := or
//! or         := and ( ("or" | "||") and )*
//! and        := not ( ("and" | "&&") not )*
//! not        := ("not" | "!") not | compare
//! compare    := value ( ("=" | "==" | "!=" | "<" | "<=" | ">" | ">=" | ":") value )?
//! value      := "text" | 'text' | number | name | name "(" [ expression ("," expression)* ] ")"
//!             | "(" expression ")"
//! ```
//!
//! Every value is text; an empty text is false and any other is true. `a or b`
//! gives `a` when it is not empty and `b` otherwise; `a and b` gives `b` when `a`
//! is not empty. A comparison and `not` give `1` for true and nothing for false.
//! Comparisons compare numbers as numbers when both sides are numbers, and text
//! otherwise, without regard to case; `:` tests whether the left side contains
//! the right. A name is a field, or a name that `let` binds. The functions are
//! listed in [`Function`]; their names, like field names and the words `and`,
//! `or` and `not`, may be written in any case.
//!
//! A field's value is the one a naming pattern writes: `{track}` has two digits
//! (three on an album of a hundred tracks or more), `{disc}` has a value only on
//! an album of several discs. `raw(disc)` gives the value as it is stored.
//!
//! Every pattern of the older naming syntax (fields, folders and brackets only)
//! is a template and gives the same text as before.
//!
//! Limits keep any input safe: brackets nest at most [`MAX_BRACKET_DEPTH`] deep,
//! expressions at most [`MAX_NESTING`] deep, and no value grows beyond
//! [`MAX_TEXT_BYTES`].

mod eval;
mod parser;
mod values;

#[cfg(test)]
mod legacy;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::fmt;

use crate::query::Field;

pub use values::{FieldMap, FieldValues};

/// The deepest square brackets may nest.
pub const MAX_BRACKET_DEPTH: usize = 16;

/// The deepest expressions may nest: parentheses, function calls, `not`.
pub const MAX_NESTING: usize = 32;

/// The longest text any value or a whole template gives, in bytes; longer text is
/// cut at a character boundary.
pub const MAX_TEXT_BYTES: usize = 65_536;

/// The widest padding `pad` and `num` add, in characters.
pub const MAX_PAD: usize = 1024;

/// A template that has been read. See the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    text: String,
    parts: Vec<Part>,
}

/// One piece of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// Text written as it is.
    Text(String),
    /// A folder starts here; the character where the `/` is.
    Folder(usize),
    /// An expression's value.
    Value(Node),
    /// Parts left out when an expression directly among them gives nothing.
    Optional(Vec<Part>),
}

/// An expression.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    /// Text or a number.
    Literal(String),
    /// A field's value, as a naming pattern writes it.
    Field(Field),
    /// A field's value as it is stored.
    Raw(Field),
    /// The value bound by the `let` this many bindings from the outermost.
    Variable(usize),
    /// `let(name, value, body)`: the body with the value bound.
    Let(Box<Node>, Box<Node>),
    /// A function.
    Call(Function, Vec<Node>),
    /// `not`.
    Not(Box<Node>),
    /// `and` between two or more values.
    And(Vec<Node>),
    /// `or` between two or more values.
    Or(Vec<Node>),
    /// A comparison.
    Compare(Comparison, Box<Node>, Box<Node>),
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Contains,
}

/// The functions of the language, by the name they are called by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Function {
    /// `if(test, then[, else])`: `then` when `test` is not empty, otherwise `else`
    /// (or nothing).
    If,
    /// `concat(a, b, …)`: the values one after another.
    Concat,
    /// `upper(x)`: in upper case.
    Upper,
    /// `lower(x)`: in lower case.
    Lower,
    /// `left(x, n)`: the first `n` characters.
    Left,
    /// `right(x, n)`: the last `n` characters.
    Right,
    /// `substr(x, start[, length])`: the characters from `start` (the first is 1),
    /// all or `length` of them.
    Substr,
    /// `len(x)`: the number of characters.
    Len,
    /// `trim(x)`: without spaces at either end.
    Trim,
    /// `replace(x, from, to)`: every `from` replaced by `to`.
    Replace,
    /// `pad(x, width[, char])`: filled at the end with spaces (or `char`) to
    /// `width` characters.
    Pad,
    /// `num(x, width)`: a whole number with zeros in front to `width` digits;
    /// anything else as it is.
    Num,
    /// `firstword(x)`: the first word.
    FirstWord,
    /// `sortname(x)`: "The Beatles" as "Beatles, The" (also "A" and "An").
    SortName,
    /// `noarticle(x)`: "The Beatles" as "Beatles".
    NoArticle,
    /// `initial(x)`: the first letter or digit in upper case, `#` for a digit.
    Initial,
    /// `year(x)`: the year of a date (`2021` of `2021-03-04`).
    Year,
    /// `month(x)`: the month of a date, two digits.
    Month,
    /// `day(x)`: the day of a date, two digits.
    Day,
    /// `date(x)`: the date part of a date and time (`2021-03-04`).
    Date,
    /// `time(x)`: a number of seconds as `m:ss`, or `h:mm:ss` from an hour on.
    Time,
    /// `join(field[, separator])`: every value of a field of several values,
    /// with `, ` or the separator between them.
    Join,
    /// `first(field)`: the first value of a field of several values.
    First,
    /// `last(field)`: the last value.
    Last,
    /// `count(field)`: how many values the field has.
    Count,
    /// `raw(field)`: the value as stored, without the padding of `{track}` or the
    /// rule that hides `{disc}` on an album of one disc.
    Raw,
    /// `let(name, value, body)`: `body`, where `name` stands for `value`.
    Let,
}

/// The functions with their names and the numbers of arguments they take.
const FUNCTIONS: &[(Function, &str, usize, Option<usize>)] = &[
    (Function::If, "if", 2, Some(3)),
    (Function::Concat, "concat", 0, None),
    (Function::Upper, "upper", 1, Some(1)),
    (Function::Lower, "lower", 1, Some(1)),
    (Function::Left, "left", 2, Some(2)),
    (Function::Right, "right", 2, Some(2)),
    (Function::Substr, "substr", 2, Some(3)),
    (Function::Len, "len", 1, Some(1)),
    (Function::Trim, "trim", 1, Some(1)),
    (Function::Replace, "replace", 3, Some(3)),
    (Function::Pad, "pad", 2, Some(3)),
    (Function::Num, "num", 2, Some(2)),
    (Function::FirstWord, "firstword", 1, Some(1)),
    (Function::SortName, "sortname", 1, Some(1)),
    (Function::NoArticle, "noarticle", 1, Some(1)),
    (Function::Initial, "initial", 1, Some(1)),
    (Function::Year, "year", 1, Some(1)),
    (Function::Month, "month", 1, Some(1)),
    (Function::Day, "day", 1, Some(1)),
    (Function::Date, "date", 1, Some(1)),
    (Function::Time, "time", 1, Some(1)),
    (Function::Join, "join", 1, Some(2)),
    (Function::First, "first", 1, Some(1)),
    (Function::Last, "last", 1, Some(1)),
    (Function::Count, "count", 1, Some(1)),
    (Function::Raw, "raw", 1, Some(1)),
    (Function::Let, "let", 3, Some(3)),
];

impl Function {
    /// Every function, in the order of the documentation.
    pub fn all() -> impl Iterator<Item = Function> {
        FUNCTIONS.iter().map(|(function, ..)| *function)
    }

    /// The function called `name`, in any case.
    pub fn from_name(name: &str) -> Option<Self> {
        FUNCTIONS
            .iter()
            .find(|(_, known, ..)| known.eq_ignore_ascii_case(name))
            .map(|(function, ..)| *function)
    }

    /// The name it is called by.
    pub fn name(self) -> &'static str {
        FUNCTIONS
            .iter()
            .find(|(function, ..)| *function == self)
            .map_or("", |(_, name, ..)| name)
    }

    /// The fewest and the most arguments it takes (`None`: any number).
    pub fn arguments(self) -> (usize, Option<usize>) {
        FUNCTIONS
            .iter()
            .find(|(function, ..)| *function == self)
            .map_or((0, None), |(_, _, min, max)| (*min, *max))
    }
}

/// Why a template cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The template is empty.
    Empty,
    /// A `{` without its `}`.
    UnclosedField,
    /// `{}` without anything in it.
    EmptyField,
    /// A name that is no field (nor a name `let` binds).
    UnknownField(String),
    /// A `[` without its `]`.
    UnclosedBracket,
    /// A `]`, `}` or `)` without its opening half.
    UnexpectedClose(char),
    /// A `{` inside braces.
    NestedField,
    /// A path template starts or ends with `/`, or has two in a row: a folder
    /// name would be empty.
    EmptyFolder,
    /// Square brackets nested deeper than [`MAX_BRACKET_DEPTH`].
    TooDeep,
    /// Expressions nested deeper than [`MAX_NESTING`].
    TooComplex,
    /// A quote without its closing quote.
    UnclosedQuote,
    /// A `(` without its `)`.
    UnclosedParenthesis,
    /// A name followed by `(` that is no function.
    UnknownFunction(String),
    /// A function called with too few or too many arguments.
    Arguments {
        /// The function's name.
        function: &'static str,
        /// The fewest it takes.
        min: usize,
        /// The most it takes; `None` for any number.
        max: Option<usize>,
    },
    /// An operator or a `,` with no value after it, or `()` with nothing in it.
    MissingValue,
    /// Something that does not belong where it is: the text of it.
    Unexpected(String),
    /// The first argument of `let` is not a plain name.
    NameExpected,
    /// The argument of `raw` is not a field.
    FieldExpected,
}

/// A template that cannot be read: what is wrong and where, in characters from
/// the start of the template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// What is wrong.
    pub problem: Problem,
    /// The first character of the problem, counted from 0.
    pub start: usize,
    /// The character after the problem.
    pub end: usize,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the pattern is empty"),
            Self::UnclosedField => write!(f, "a field is not closed with }}"),
            Self::EmptyField => write!(f, "{{}} names no field"),
            Self::UnknownField(name) => write!(f, "there is no field {name}"),
            Self::UnclosedBracket => write!(f, "a [ is not closed with ]"),
            Self::UnexpectedClose(c) => write!(f, "{c} closes nothing"),
            Self::NestedField => write!(f, "a field name cannot hold {{"),
            Self::EmptyFolder => write!(f, "a folder name would be empty"),
            Self::TooDeep => write!(f, "square brackets are nested too deeply"),
            Self::TooComplex => write!(f, "the expression is nested too deeply"),
            Self::UnclosedQuote => write!(f, "a quote is not closed"),
            Self::UnclosedParenthesis => write!(f, "a ( is not closed with )"),
            Self::UnknownFunction(name) => write!(f, "there is no function {name}"),
            Self::Arguments { function, min, max } => match max {
                Some(max) if max == min => write!(f, "{function} takes {min} values"),
                Some(max) => write!(f, "{function} takes {min} to {max} values"),
                None => write!(f, "{function} takes at least {min} values"),
            },
            Self::MissingValue => write!(f, "a value is missing"),
            Self::Unexpected(text) => write!(f, "{text} does not belong here"),
            Self::NameExpected => write!(f, "let needs a name first"),
            Self::FieldExpected => write!(f, "raw needs a field"),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at character {})", self.problem, self.start + 1)
    }
}

impl std::error::Error for Error {}

/// One piece of a template's output, as [`Template::pieces`] gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Text of the template itself.
    Text(&'a str),
    /// The value of an expression; `None` when it gave nothing (which happens
    /// only outside square brackets).
    Value(Option<String>),
    /// A folder starts here (only in path templates).
    Folder,
}

impl Template {
    /// Reads `text` as a template in which `/` is text like any other.
    pub fn parse(text: &str) -> Result<Self, Error> {
        Self::read(text, false)
    }

    /// Reads `text` as a path template: `/` starts a folder, and no folder name
    /// may be always empty.
    pub fn parse_path(text: &str) -> Result<Self, Error> {
        Self::read(text, true)
    }

    fn read(text: &str, path: bool) -> Result<Self, Error> {
        let parts = parser::parse(text, path)?;
        Ok(Self {
            text: text.to_owned(),
            parts,
        })
    }

    /// The template as written.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The fields the template reads, in order, each once.
    pub fn fields(&self) -> Vec<Field> {
        let mut fields = Vec::new();
        eval::collect_fields(&self.parts, &mut fields);
        fields
    }

    /// The pieces of the output for a track whose values `values` gives, in
    /// order, to `out`: the template's own text, the values of its expressions
    /// and the starts of folders. Brackets whose expressions give nothing are
    /// left out already.
    pub fn pieces(&self, values: &dyn FieldValues, out: &mut dyn FnMut(Piece<'_>)) {
        eval::Evaluator::new(values).pieces(&self.parts, out);
    }

    /// The text of the template for a track whose values `values` gives; an
    /// expression that gives nothing outside brackets writes nothing, and a
    /// folder is written as `/`. At most [`MAX_TEXT_BYTES`] long.
    pub fn evaluate(&self, values: &dyn FieldValues) -> String {
        let mut text = String::new();
        let mut push = |piece: &str| {
            if text.len() < MAX_TEXT_BYTES {
                text.push_str(piece);
            }
        };
        self.pieces(values, &mut |piece| match piece {
            Piece::Text(t) => push(t),
            Piece::Value(Some(v)) => push(&v),
            Piece::Value(None) => {}
            Piece::Folder => push("/"),
        });
        eval::cut(&mut text);
        text
    }

    /// A template that writes `text` and nothing else; a fallback for one that
    /// cannot be read.
    pub fn literal(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            parts: vec![Part::Text(text.to_owned())],
        }
    }
}

/// The order of sort keys and group names: without regard to case, and runs of
/// digits by their value, so that "Disc 2" comes before "Disc 10".
pub fn compare_keys(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (x, y) = (take(&mut a), take(&mut b));
                let (xt, yt) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = xt.len().cmp(&yt.len()).then_with(|| xt.cmp(yt));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}
