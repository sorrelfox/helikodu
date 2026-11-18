// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The query, its boolean expression, sort keys and limit.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::field::{Field, FieldKind};
use super::value::Value;

/// A whole query: which tracks, in what order, and how many.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Query {
    /// Which tracks; every track when `None`.
    pub filter: Option<Expr>,
    /// The sort keys, most significant first. Without any, tracks are in album order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortKey>,
    /// How many of the sorted tracks to keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Limit>,
}

impl Query {
    /// A query with `filter`, without sort keys or limit.
    pub fn new(filter: Expr) -> Self {
        Self {
            filter: Some(filter),
            ..Self::default()
        }
    }

    /// The same query sorted by `key` after its other sort keys.
    #[must_use]
    pub fn sorted_by(mut self, key: SortKey) -> Self {
        self.sort.push(key);
        self
    }

    /// The same query limited to `limit`.
    #[must_use]
    pub fn limited(mut self, limit: Limit) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Whether the query selects every track in the default order.
    pub fn is_empty(&self) -> bool {
        self.filter.is_none() && self.sort.is_empty() && self.limit.is_none()
    }

    /// Whether the query is only plain text, without fields, operators, sort keys or a
    /// limit: what a plain search does.
    pub fn is_plain_text(&self) -> bool {
        fn plain(expr: &Expr) -> bool {
            match expr {
                Expr::Text(_) => true,
                Expr::All(children) => children.iter().all(plain),
                _ => false,
            }
        }
        self.sort.is_empty() && self.limit.is_none() && self.filter.as_ref().is_some_and(plain)
    }

    /// The same query in canonical form (see [`Expr::normalized`]).
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            filter: self.filter.and_then(Expr::normalized),
            ..self
        }
    }

    /// The first comparison that does not fit together, with its problem.
    pub fn problem(&self) -> Option<(&Comparison, TermProblem)> {
        self.filter.as_ref().and_then(Expr::problem)
    }
}

/// A condition on a track.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Expr {
    /// Every one of these holds (`a b`, `a AND b`).
    All(Vec<Expr>),
    /// At least one of these holds (`a OR b`).
    Any(Vec<Expr>),
    /// This does not hold (`-a`, `NOT a`).
    Not(Box<Expr>),
    /// A field compared with a value (`year>=1970`).
    Compare(Comparison),
    /// Plain text searched in the title, artists, album and the other text fields.
    Text(String),
    /// The track has a value for this (`has:lyrics`, `has:rating`).
    Has(Presence),
    /// The track is of this sort (`is:lossless`).
    Is(Flag),
}

impl Expr {
    /// `field op value`.
    pub fn compare(field: Field, op: Operator, value: Value) -> Self {
        Self::Compare(Comparison { field, op, value })
    }

    /// Plain text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// This and `other`, flattened.
    #[must_use]
    pub fn and(self, other: Expr) -> Self {
        let mut children = match self {
            Self::All(children) => children,
            expr => vec![expr],
        };
        match other {
            Self::All(more) => children.extend(more),
            expr => children.push(expr),
        }
        Self::All(children)
    }

    /// This or `other`, flattened.
    #[must_use]
    pub fn or(self, other: Expr) -> Self {
        let mut children = match self {
            Self::Any(children) => children,
            expr => vec![expr],
        };
        match other {
            Self::Any(more) => children.extend(more),
            expr => children.push(expr),
        }
        Self::Any(children)
    }

    /// The negation of this.
    #[must_use]
    pub fn negate(self) -> Self {
        Self::Not(Box::new(self))
    }

    /// The canonical form, which the text form prints and parses back unchanged:
    /// groups nested in a group of the same kind are merged into it, a group of one
    /// is replaced by its member, and empty `All` groups are removed. `None` if
    /// nothing is left, which matches every track. An empty `Any` matches no track.
    pub fn normalized(self) -> Option<Self> {
        match self {
            Self::All(children) => {
                let mut flat = Vec::new();
                for child in children.into_iter().filter_map(Self::normalized) {
                    match child {
                        Self::All(inner) => flat.extend(inner),
                        other => flat.push(other),
                    }
                }
                match flat.len() {
                    0 => None,
                    1 => flat.pop(),
                    _ => Some(Self::All(flat)),
                }
            }
            Self::Any(children) => {
                let mut flat = Vec::new();
                for child in children {
                    match child.normalized() {
                        Some(Self::Any(inner)) => flat.extend(inner),
                        Some(other) => flat.push(other),
                        // An empty group matches every track, and so does the whole.
                        None => return None,
                    }
                }
                match flat.len() {
                    1 => flat.pop(),
                    _ => Some(Self::Any(flat)),
                }
            }
            // Not every track is no track: the empty `Any`.
            Self::Not(inner) => Some(match inner.normalized() {
                Some(inner) => Self::Not(Box::new(inner)),
                None => Self::Any(Vec::new()),
            }),
            other => Some(other),
        }
    }

    /// The first comparison in this expression that does not fit together.
    pub fn problem(&self) -> Option<(&Comparison, TermProblem)> {
        match self {
            Self::All(children) | Self::Any(children) => children.iter().find_map(Self::problem),
            Self::Not(inner) => inner.problem(),
            Self::Compare(comparison) => comparison.check().err().map(|p| (comparison, p)),
            Self::Text(_) | Self::Has(_) | Self::Is(_) => None,
        }
    }
}

/// A field compared with a value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Comparison {
    /// The field.
    pub field: Field,
    /// How it is compared.
    pub op: Operator,
    /// What with.
    pub value: Value,
}

impl Comparison {
    /// Whether the field, operator and value fit together.
    pub fn check(&self) -> Result<(), TermProblem> {
        let kind = self.field.kind();
        if let Value::Range(low, high) = &self.value {
            if matches!(kind, FieldKind::Text | FieldKind::Kind) {
                return Err(TermProblem::NoRange);
            }
            if self.op != Operator::Contains {
                return Err(TermProblem::Operator);
            }
            if matches!(**low, Value::Range(..))
                || std::mem::discriminant(&**low) != std::mem::discriminant(&**high)
            {
                return Err(TermProblem::Value);
            }
            for end in [low, high] {
                check_single(kind, Operator::Contains, end)?;
            }
            return Ok(());
        }
        check_single(kind, self.op, &self.value)
    }
}

fn check_single(kind: FieldKind, op: Operator, value: &Value) -> Result<(), TermProblem> {
    let op_fits = match kind {
        FieldKind::Text => matches!(
            op,
            Operator::Contains | Operator::Equals | Operator::NotEquals | Operator::Matches
        ),
        FieldKind::Kind => matches!(
            op,
            Operator::Contains | Operator::Equals | Operator::NotEquals
        ),
        _ => op != Operator::Matches,
    };
    if !op_fits {
        return Err(TermProblem::Operator);
    }
    let value_fits = match (kind, value) {
        (FieldKind::Text, Value::Text(_))
        | (FieldKind::Kind, Value::Kind(_))
        | (FieldKind::Number, Value::Number(_))
        | (FieldKind::Duration, Value::Duration(_))
        | (FieldKind::Size, Value::Size(_))
        | (FieldKind::Date, Value::Date(_) | Value::Age(_)) => true,
        (FieldKind::Rating, Value::Number(stars)) => {
            let t = stars.thousandths();
            if !(0..=5000).contains(&t) || t % 500 != 0 {
                return Err(TermProblem::Rating);
            }
            true
        }
        _ => false,
    };
    if value_fits {
        Ok(())
    } else {
        Err(TermProblem::Value)
    }
}

/// Why a comparison does not fit together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TermProblem {
    /// The field cannot be compared that way (`title<5`, `year~x`).
    Operator,
    /// The value is not of the field's kind (`year:soon`).
    Value,
    /// A range of a text field or the media kind.
    NoRange,
    /// A rating that is not 0 to 5 stars in half steps.
    Rating,
}

impl fmt::Display for TermProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Operator => "the field cannot be compared this way",
            Self::Value => "the value does not fit the field",
            Self::NoRange => "the field takes no range",
            Self::Rating => "a rating is 0 to 5 stars, in half stars",
        })
    }
}

/// How a field is compared with a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    /// `:` contains the text; for numbers and dates, equals or falls within.
    Contains,
    /// `=` equals.
    Equals,
    /// `!=` differs.
    NotEquals,
    /// `<` less than; for ages, more recent than.
    Less,
    /// `<=`.
    LessOrEqual,
    /// `>` greater than; for ages, longer ago than.
    Greater,
    /// `>=`.
    GreaterOrEqual,
    /// `~` matches a regular expression.
    Matches,
}

impl Operator {
    /// Every operator.
    pub const ALL: [Self; 8] = [
        Self::Contains,
        Self::Equals,
        Self::NotEquals,
        Self::Less,
        Self::LessOrEqual,
        Self::Greater,
        Self::GreaterOrEqual,
        Self::Matches,
    ];

    /// The symbol in the text form.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Contains => ":",
            Self::Equals => "=",
            Self::NotEquals => "!=",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
            Self::Matches => "~",
        }
    }
}

/// What `has:` tests for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Presence {
    /// Lyrics in the file (`has:lyrics`).
    Lyrics,
    /// A cover picture (`has:cover`).
    Cover,
    /// A value of the field (`has:rating`, `has:genre`).
    Field(Field),
}

impl Presence {
    /// The name after `has:`.
    pub fn name(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Lyrics => "lyrics".into(),
            Self::Cover => "cover".into(),
            Self::Field(field) => field.name(),
        }
    }

    /// The presence named `name`, in any case.
    pub fn from_name(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("lyrics") {
            Some(Self::Lyrics)
        } else if name.eq_ignore_ascii_case("cover") {
            Some(Self::Cover)
        } else {
            Field::from_name(name).map(Self::Field)
        }
    }
}

/// What `is:` tests for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Flag {
    /// In a lossless format (`is:lossless`).
    Lossless,
    /// Lossless above 48 kHz or 16 bits (`is:hires`).
    HiRes,
    /// On a compilation (`is:compilation`).
    Compilation,
    /// Marked as loved (`is:loved`).
    Loved,
}

impl Flag {
    /// Every flag.
    pub const ALL: [Self; 4] = [Self::Lossless, Self::HiRes, Self::Compilation, Self::Loved];

    /// The name after `is:`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lossless => "lossless",
            Self::HiRes => "hires",
            Self::Compilation => "compilation",
            Self::Loved => "loved",
        }
    }

    /// The flag named `name`, in any case.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|flag| flag.name().eq_ignore_ascii_case(name))
    }
}

/// One sort key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SortKey {
    /// What to sort by.
    pub by: SortBy,
    /// Largest, latest or last in the alphabet first.
    #[serde(default)]
    pub descending: bool,
}

impl SortKey {
    /// Ascending by `field`.
    pub fn ascending(field: Field) -> Self {
        Self {
            by: SortBy::Field(field),
            descending: false,
        }
    }

    /// Descending by `field`.
    pub fn descending(field: Field) -> Self {
        Self {
            by: SortBy::Field(field),
            descending: true,
        }
    }
}

/// What a sort key sorts by.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// A field; tracks without a value come last.
    Field(Field),
    /// A shuffled order, the same for the same seed.
    Random,
}

/// How many of the sorted tracks a query keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Limit {
    /// The amount, in `unit`.
    pub amount: u32,
    /// What is counted.
    pub unit: LimitUnit,
}

impl Limit {
    /// At most `amount` tracks.
    pub fn tracks(amount: u32) -> Self {
        Self {
            amount,
            unit: LimitUnit::Tracks,
        }
    }
}

/// The unit of a [`Limit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitUnit {
    /// Tracks (`limit:50`).
    Tracks,
    /// Minutes of playing time (`limit:90min`).
    Minutes,
    /// Hours of playing time (`limit:2h`).
    Hours,
    /// Megabytes of files (`limit:700MB`).
    Megabytes,
    /// Gigabytes of files (`limit:4GB`).
    Gigabytes,
}

impl LimitUnit {
    /// Every unit.
    pub const ALL: [Self; 5] = [
        Self::Tracks,
        Self::Minutes,
        Self::Hours,
        Self::Megabytes,
        Self::Gigabytes,
    ];

    /// The suffix in the text form; empty for tracks.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Tracks => "",
            Self::Minutes => "min",
            Self::Hours => "h",
            Self::Megabytes => "MB",
            Self::Gigabytes => "GB",
        }
    }

    /// The unit with `suffix` (any case; `tracks`, `m` and `minutes` too).
    pub fn from_suffix(suffix: &str) -> Option<Self> {
        let lower = suffix.to_ascii_lowercase();
        Some(match lower.as_str() {
            "" | "track" | "tracks" => Self::Tracks,
            "m" | "min" | "mins" | "minutes" => Self::Minutes,
            "h" | "hour" | "hours" => Self::Hours,
            "mb" => Self::Megabytes,
            "gb" | "g" => Self::Gigabytes,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{Age, AgeUnit, Decimal, MediaKind};

    fn year(op: Operator, value: Value) -> Comparison {
        Comparison {
            field: Field::Year,
            op,
            value,
        }
    }

    #[test]
    fn comparisons_are_checked() {
        assert_eq!(year(Operator::Less, Value::number(1970)).check(), Ok(()));
        assert_eq!(
            year(Operator::Matches, Value::number(1970)).check(),
            Err(TermProblem::Operator)
        );
        assert_eq!(
            year(Operator::Less, Value::Text("x".into())).check(),
            Err(TermProblem::Value)
        );
        let decade = Value::range(Value::number(1970), Value::number(1979));
        assert_eq!(year(Operator::Contains, decade.clone()).check(), Ok(()));
        assert_eq!(
            year(Operator::Less, decade).check(),
            Err(TermProblem::Operator)
        );
        let mixed = Value::range(Value::number(1970), Value::Duration(1));
        assert_eq!(
            year(Operator::Contains, mixed).check(),
            Err(TermProblem::Value)
        );
        let title = Comparison {
            field: Field::Title,
            op: Operator::Contains,
            value: Value::range(Value::Text("a".into()), Value::Text("b".into())),
        };
        assert_eq!(title.check(), Err(TermProblem::NoRange));
        let rating = |t| Comparison {
            field: Field::Rating,
            op: Operator::GreaterOrEqual,
            value: Value::Number(Decimal::from_thousandths(t)),
        };
        assert_eq!(rating(3500).check(), Ok(()));
        assert_eq!(rating(3200).check(), Err(TermProblem::Rating));
        assert_eq!(rating(5500).check(), Err(TermProblem::Rating));
        let added = Comparison {
            field: Field::Added,
            op: Operator::Less,
            value: Value::Age(Age {
                amount: 30,
                unit: AgeUnit::Days,
            }),
        };
        assert_eq!(added.check(), Ok(()));
        let kind = Comparison {
            field: Field::Kind,
            op: Operator::Less,
            value: Value::Kind(MediaKind::Music),
        };
        assert_eq!(kind.check(), Err(TermProblem::Operator));
    }

    #[test]
    fn normalizing_flattens_groups() {
        let a = Expr::text("a");
        let b = Expr::text("b");
        let c = Expr::text("c");
        let nested = Expr::All(vec![
            Expr::All(vec![a.clone(), b.clone()]),
            Expr::Any(vec![c.clone()]),
            Expr::All(vec![]),
        ]);
        assert_eq!(
            nested.normalized(),
            Some(Expr::All(vec![a.clone(), b.clone(), c.clone()]))
        );
        assert_eq!(Expr::All(vec![]).normalized(), None);
        assert_eq!(
            Expr::Any(vec![a.clone(), Expr::Any(vec![b.clone(), c.clone()])]).normalized(),
            Some(Expr::Any(vec![a.clone(), b.clone(), c.clone()]))
        );
        assert_eq!(
            a.clone().and(b.clone()).and(c.clone()),
            Expr::All(vec![a, b, c])
        );
    }

    #[test]
    fn plain_text_is_recognised() {
        assert!(Query::new(Expr::text("a").and(Expr::text("b"))).is_plain_text());
        assert!(!Query::new(Expr::text("a").negate()).is_plain_text());
        assert!(!Query::default().is_plain_text());
        assert!(
            !Query::new(Expr::text("a"))
                .sorted_by(SortKey::descending(Field::Added))
                .is_plain_text()
        );
    }

    #[test]
    fn queries_serialize() {
        let query = Query::new(
            Expr::compare(
                Field::Tag("MOOD".into()),
                Operator::Equals,
                Value::Text("calm".into()),
            )
            .and(Expr::Has(Presence::Field(Field::Rating)).negate()),
        )
        .sorted_by(SortKey::descending(Field::Added))
        .limited(Limit::tracks(50));
        let json = serde_json::to_string(&query).unwrap();
        assert_eq!(serde_json::from_str::<Query>(&json).unwrap(), query);
    }
}
