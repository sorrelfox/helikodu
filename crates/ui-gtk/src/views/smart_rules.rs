// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The rules of a smart playlist as the rule builder shows them, and their
//! way to and from the query tree.
//!
//! The builder shows "Match all (or any) of these rules": rules and groups of
//! rules, each group "any (or all) of these", then a limit and an order. A
//! query whose tree has another shape (plain words, a negated group, groups
//! inside groups, a range) cannot be shown as rules and is edited as text.
//!
//! Rules made from a query give the same query back (in its canonical form),
//! and a rule's value is read with the query language's own parser, so the
//! text form and the rules never disagree.

use helikodu_model::query::{
    AgeUnit, Comparison, Expr, Field, FieldKind, Limit, LimitUnit, Operator, Presence, Query,
    SortBy, SortKey, Value,
};
use helikodu_service::library::{parse_query, print_query};

use crate::i18n::{fill, ntr, tr};

/// How a rule compares its field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Cond {
    Contains,
    Is,
    IsNot,
    NotContains,
    Matches,
    AtLeast,
    AtMost,
    MoreThan,
    LessThan,
    InLast,
    NotInLast,
    Before,
    After,
    Present,
    Absent,
}

impl Cond {
    /// The conditions a field of `kind` offers, in the dropdown's order.
    pub(crate) fn for_kind(kind: FieldKind) -> &'static [Cond] {
        use Cond::*;
        match kind {
            FieldKind::Text => &[Contains, Is, IsNot, NotContains, Matches, Present, Absent],
            FieldKind::Kind => &[Is, IsNot],
            FieldKind::Date => &[InLast, NotInLast, Before, After, Is, Present, Absent],
            _ => &[
                Is, IsNot, AtLeast, AtMost, MoreThan, LessThan, Present, Absent,
            ],
        }
    }

    /// Its words, as the dropdown and the rule line show them.
    pub(crate) fn label(self) -> String {
        tr(match self {
            Self::Contains => "contains",
            Self::Is => "is",
            Self::IsNot => "is not",
            Self::NotContains => "does not contain",
            Self::Matches => "matches",
            Self::AtLeast => "is at least",
            Self::AtMost => "is at most",
            Self::MoreThan => "is more than",
            Self::LessThan => "is less than",
            Self::InLast => "in the last",
            Self::NotInLast => "not in the last",
            Self::Before => "is before",
            Self::After => "is after",
            Self::Present => "is set",
            Self::Absent => "is not set",
        })
    }

    /// Its words for `field`: a codec or container "is" what it contains,
    /// as `codec:flac` finds every FLAC file.
    pub(crate) fn label_for(self, field: &Field) -> String {
        match (field, self) {
            (Field::Codec | Field::Container, Self::Contains) => tr("is"),
            (Field::Codec | Field::Container, Self::Is) => tr("is exactly"),
            _ => self.label(),
        }
    }

    /// Whether the rule needs a value.
    pub(crate) fn takes_value(self) -> bool {
        !matches!(self, Self::Present | Self::Absent)
    }

    /// The operator of the comparison, and whether it is negated.
    fn operator(self, kind: FieldKind) -> Option<(Operator, bool)> {
        Some(match (self, kind) {
            (Self::Contains, _) => (Operator::Contains, false),
            (Self::NotContains, _) => (Operator::Contains, true),
            (Self::Is, FieldKind::Date) => (Operator::Contains, false),
            (Self::Is, _) => (Operator::Equals, false),
            (Self::IsNot, _) => (Operator::NotEquals, false),
            (Self::Matches, _) => (Operator::Matches, false),
            (Self::AtLeast, _) => (Operator::GreaterOrEqual, false),
            (Self::AtMost, _) => (Operator::LessOrEqual, false),
            (Self::MoreThan, _) => (Operator::Greater, false),
            (Self::LessThan | Self::InLast | Self::Before, _) => (Operator::Less, false),
            (Self::NotInLast | Self::After, _) => (Operator::Greater, false),
            (Self::Present | Self::Absent, _) => return None,
        })
    }
}

/// The fields the field dropdown offers, in its order.
pub(crate) const FIELDS: &[Field] = &[
    Field::Title,
    Field::Artist,
    Field::AlbumArtist,
    Field::Album,
    Field::Genre,
    Field::Composer,
    Field::Performer,
    Field::Conductor,
    Field::Year,
    Field::Rating,
    Field::Plays,
    Field::Skips,
    Field::Added,
    Field::LastPlayed,
    Field::FirstPlayed,
    Field::Modified,
    Field::Duration,
    Field::Codec,
    Field::SampleRate,
    Field::BitDepth,
    Field::Bitrate,
    Field::Channels,
    Field::Size,
    Field::Kind,
    Field::ReleaseType,
    Field::Work,
    Field::Lyricist,
    Field::OriginalYear,
    Field::Track,
    Field::Disc,
    Field::Path,
    Field::Folder,
    Field::Filename,
    Field::Container,
];

/// The name of `field` in the field dropdown.
pub(crate) fn field_label(field: &Field) -> String {
    match field {
        Field::Title => tr("Title"),
        Field::Artist => tr("Artist"),
        Field::AlbumArtist => tr("Album artist"),
        Field::Album => tr("Album"),
        Field::Composer => tr("Composer"),
        Field::Performer => tr("Performer"),
        Field::Conductor => tr("Conductor"),
        Field::Lyricist => tr("Lyricist"),
        Field::Genre => tr("Genre"),
        Field::Work => tr("Work"),
        Field::Movement => tr("Movement"),
        Field::DiscSubtitle => tr("Disc subtitle"),
        Field::ReleaseType => tr("Release type"),
        Field::Codec => tr("Codec"),
        Field::Container => tr("Container"),
        Field::Path => tr("Path"),
        Field::Folder => tr("Folder"),
        Field::Filename => tr("File name"),
        Field::Kind => tr("Kind"),
        Field::Year => tr("Year"),
        Field::OriginalYear => tr("Original year"),
        Field::Track => tr("Track number"),
        Field::Disc => tr("Disc number"),
        Field::MovementNumber => tr("Movement number"),
        Field::SampleRate => tr("Sample rate"),
        Field::BitDepth => tr("Bit depth"),
        Field::Channels => tr("Channels"),
        Field::Bitrate => tr("Bit rate"),
        Field::TrackGain => tr("Track gain"),
        Field::AlbumGain => tr("Album gain"),
        Field::Plays => tr("Plays"),
        Field::Skips => tr("Skips"),
        Field::Rating => tr("Rating"),
        Field::Duration => tr("Length"),
        Field::Resume => tr("Resume position"),
        Field::Size => tr("File size"),
        Field::Added => tr("Date added"),
        Field::Modified => tr("Date modified"),
        Field::LastPlayed => tr("Last played"),
        Field::FirstPlayed => tr("First played"),
        Field::LastSkipped => tr("Last skipped"),
        Field::Tag(name) => name.clone(),
        _ => field.name().into_owned(),
    }
}

/// The name of `field` at the start of a sentence of the rule line.
fn field_words(field: &Field) -> String {
    match field {
        Field::Added => tr("Added"),
        Field::Modified => tr("Modified"),
        _ => field_label(field),
    }
}

/// One rule: a field, a condition and the value as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) field: Field,
    pub(crate) cond: Cond,
    pub(crate) value: String,
}

impl Rule {
    /// A new rule on `field` with its first condition and no value.
    pub(crate) fn new(field: Field) -> Self {
        let cond = Cond::for_kind(field.kind())[0];
        Self {
            field,
            cond,
            value: String::new(),
        }
    }

    /// The rule as an expression; `None` while its value cannot be read.
    pub(crate) fn expr(&self) -> Option<Expr> {
        let kind = self.field.kind();
        match self.cond {
            Cond::Present => return Some(Expr::Has(Presence::Field(self.field.clone()))),
            Cond::Absent => {
                return Some(Expr::Has(Presence::Field(self.field.clone())).negate());
            }
            _ => {}
        }
        let (op, negated) = self.cond.operator(kind)?;
        let value = read_value(&self.field, op, &self.value)?;
        let comparison = Comparison {
            field: self.field.clone(),
            op,
            value,
        };
        comparison.check().ok()?;
        let expr = Expr::Compare(comparison);
        Some(if negated { expr.negate() } else { expr })
    }

    /// The rule an expression stands for, if it is one the builder shows.
    fn from_expr(expr: &Expr) -> Option<Self> {
        match expr {
            Expr::Has(Presence::Field(field)) => Some(Self {
                field: field.clone(),
                cond: Cond::Present,
                value: String::new(),
            }),
            Expr::Not(inner) => match &**inner {
                Expr::Has(Presence::Field(field)) => Some(Self {
                    field: field.clone(),
                    cond: Cond::Absent,
                    value: String::new(),
                }),
                Expr::Compare(c)
                    if c.op == Operator::Contains && c.field.kind() == FieldKind::Text =>
                {
                    Some(Self {
                        field: c.field.clone(),
                        cond: Cond::NotContains,
                        value: value_text(&c.field, &c.value)?,
                    })
                }
                _ => None,
            },
            Expr::Compare(c) => {
                let kind = c.field.kind();
                let date = |value: &Value| matches!(value, Value::Age(_));
                let cond = match (c.op, kind) {
                    (Operator::Contains, FieldKind::Text) => Cond::Contains,
                    (Operator::Contains, FieldKind::Date) if !date(&c.value) => Cond::Is,
                    (Operator::Equals, FieldKind::Date) => return None,
                    (Operator::Equals, _) => Cond::Is,
                    (Operator::NotEquals, FieldKind::Date) => return None,
                    (Operator::NotEquals, _) => Cond::IsNot,
                    (Operator::Matches, FieldKind::Text) => Cond::Matches,
                    (Operator::Less, FieldKind::Date) if date(&c.value) => Cond::InLast,
                    (Operator::Greater, FieldKind::Date) if date(&c.value) => Cond::NotInLast,
                    (Operator::Less, FieldKind::Date) => Cond::Before,
                    (Operator::Greater, FieldKind::Date) => Cond::After,
                    (Operator::GreaterOrEqual, FieldKind::Date)
                    | (Operator::LessOrEqual, FieldKind::Date) => return None,
                    (Operator::GreaterOrEqual, _) => Cond::AtLeast,
                    (Operator::LessOrEqual, _) => Cond::AtMost,
                    (Operator::Greater, _) => Cond::MoreThan,
                    (Operator::Less, _) => Cond::LessThan,
                    _ => return None,
                };
                if !Cond::for_kind(kind).contains(&cond) {
                    return None;
                }
                Some(Self {
                    field: c.field.clone(),
                    cond,
                    value: value_text(&c.field, &c.value)?,
                })
            }
            _ => None,
        }
    }

    /// The rule in words, for the rule line: "Rating is Unrated".
    pub(crate) fn sentence(&self) -> String {
        if !self.cond.takes_value() {
            return format!(
                "{} {}",
                field_words(&self.field),
                self.cond.label_for(&self.field)
            );
        }
        format!(
            "{} {} {}",
            field_words(&self.field),
            self.cond.label_for(&self.field),
            self.value_words()
        )
    }

    /// The value in words: "Unrated", "88.2 kHz", "12 months", "FLAC".
    pub(crate) fn value_words(&self) -> String {
        let parsed = self
            .cond
            .operator(self.field.kind())
            .and_then(|(op, _)| read_value(&self.field, op, &self.value));
        match (&self.field, parsed) {
            (Field::Rating, Some(Value::Number(n))) if n.thousandths() == 0 => tr("Unrated"),
            (Field::Rating, Some(Value::Number(n))) => {
                let stars = n.as_f64();
                let text = if stars.fract() == 0.0 {
                    format!("{stars:.0}")
                } else {
                    crate::format::decimal(stars, 1)
                };
                fill(&ntr("{} star", "{} stars", stars.ceil() as usize), &[&text])
            }
            (Field::SampleRate, Some(Value::Number(n))) => {
                let hz = u32::try_from(n.thousandths() / 1000).unwrap_or(0);
                fill(&tr("{} kHz"), &[&crate::format::khz(hz)])
            }
            (Field::Codec | Field::Container, Some(Value::Text(text))) => text.to_uppercase(),
            (_, Some(Value::Age(age))) => {
                let n = age.amount as usize;
                let unit = match age.unit {
                    AgeUnit::Hours => ntr("{} hour", "{} hours", n),
                    AgeUnit::Days => ntr("{} day", "{} days", n),
                    AgeUnit::Weeks => ntr("{} week", "{} weeks", n),
                    AgeUnit::Months => ntr("{} month", "{} months", n),
                    AgeUnit::Years => ntr("{} year", "{} years", n),
                };
                fill(&unit, &[&age.amount.to_string()])
            }
            _ => self.value.clone(),
        }
    }
}

/// The value text typed for `field` read as a value for `op`.
fn read_value(field: &Field, op: Operator, text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if field.kind() == FieldKind::Text {
        return Some(Value::Text(text.to_owned()));
    }
    let typed = match (field, text) {
        (Field::Rating, t) if t.eq_ignore_ascii_case(&tr("Unrated")) => "0".to_owned(),
        _ => text.chars().filter(|c| !c.is_whitespace()).collect(),
    };
    let query = parse_query(&format!("{}{}{typed}", field.name(), op.symbol())).ok()?;
    match query.filter {
        Some(Expr::Compare(c)) if c.field == *field && c.op == op && query.sort.is_empty() => {
            Some(c.value)
        }
        _ => None,
    }
}

/// The text the value entry shows for `value`.
fn value_text(field: &Field, value: &Value) -> Option<String> {
    match value {
        Value::Range(..) => None,
        Value::Text(text) => Some(text.clone()),
        Value::Number(n) if *field == Field::Rating && n.thousandths() == 0 => Some(tr("Unrated")),
        // "88.2 kHz", which reads back as 88200 Hz.
        Value::Number(n) if *field == Field::SampleRate && n.thousandths() % 100_000 == 0 => {
            let hz = u32::try_from(n.thousandths() / 1000).ok()?;
            // A decimal point, as the query language reads it.
            let khz = if hz.is_multiple_of(1000) {
                (hz / 1000).to_string()
            } else {
                format!("{:.1}", f64::from(hz) / 1000.0)
            };
            Some(format!("{khz} kHz"))
        }
        other => {
            let printed = print_query(&Query::new(Expr::compare(
                field.clone(),
                Operator::Equals,
                other.clone(),
            )));
            let prefix = format!("{}=", field.name());
            Some(printed.strip_prefix(&prefix).unwrap_or(&printed).to_owned())
        }
    }
}

/// A rule, or a group of rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Node {
    Rule(Rule),
    Group { any: bool, rules: Vec<Rule> },
}

impl Node {
    fn expr(&self) -> Option<Expr> {
        match self {
            Self::Rule(rule) => rule.expr(),
            Self::Group { any, rules } => {
                let exprs = rules.iter().map(Rule::expr).collect::<Option<Vec<_>>>()?;
                Some(if *any {
                    Expr::Any(exprs)
                } else {
                    Expr::All(exprs)
                })
            }
        }
    }

    fn from_expr(expr: &Expr) -> Option<Self> {
        match expr {
            Expr::Any(children) | Expr::All(children) if !children.is_empty() => {
                let rules = children
                    .iter()
                    .map(Rule::from_expr)
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::Group {
                    any: matches!(expr, Expr::Any(_)),
                    rules,
                })
            }
            other => Rule::from_expr(other).map(Self::Rule),
        }
    }

    /// The node in words, for one chip: "Codec is FLAC or DSD".
    pub(crate) fn sentence(&self) -> String {
        match self {
            Self::Rule(rule) => rule.sentence(),
            Self::Group { any, rules } => {
                let joiner = if *any { tr("or") } else { tr("and") };
                let same = rules
                    .windows(2)
                    .all(|w| w[0].field == w[1].field && w[0].cond == w[1].cond)
                    && rules.first().is_some_and(|r| r.cond.takes_value());
                match rules.first() {
                    Some(first) if same => {
                        let values: Vec<String> = rules.iter().map(Rule::value_words).collect();
                        format!(
                            "{} {} {}",
                            field_words(&first.field),
                            first.cond.label_for(&first.field),
                            values.join(&format!(" {joiner} "))
                        )
                    }
                    _ => rules
                        .iter()
                        .map(Rule::sentence)
                        .collect::<Vec<_>>()
                        .join(&format!(" {joiner} ")),
                }
            }
        }
    }
}

/// The orders the order dropdown offers: its words, the words of the rule
/// line, and the sort keys.
pub(crate) fn orders() -> Vec<(String, String, Vec<SortKey>)> {
    let key = |field: Field, descending: bool| SortKey {
        by: SortBy::Field(field),
        descending,
    };
    vec![
        (tr("Album order"), tr("Album order"), Vec::new()),
        (
            tr("Date added, newest first"),
            tr("Newest first"),
            vec![key(Field::Added, true)],
        ),
        (
            tr("Date added, oldest first"),
            tr("Oldest first"),
            vec![key(Field::Added, false)],
        ),
        (tr("Title"), tr("By title"), vec![key(Field::Title, false)]),
        (
            tr("Artist"),
            tr("By artist"),
            vec![key(Field::Artist, false)],
        ),
        (tr("Album"), tr("By album"), vec![key(Field::Album, false)]),
        (
            tr("Year, newest first"),
            tr("Latest year first"),
            vec![key(Field::Year, true)],
        ),
        (
            tr("Year, oldest first"),
            tr("Earliest year first"),
            vec![key(Field::Year, false)],
        ),
        (
            tr("Rating, highest first"),
            tr("Highest rated first"),
            vec![key(Field::Rating, true)],
        ),
        (
            tr("Most played first"),
            tr("Most played first"),
            vec![key(Field::Plays, true)],
        ),
        (
            tr("Last played, most recent first"),
            tr("Recently played first"),
            vec![key(Field::LastPlayed, true)],
        ),
        (
            tr("Random"),
            tr("In random order"),
            vec![SortKey {
                by: SortBy::Random,
                descending: false,
            }],
        ),
    ]
}

/// The limit units the unit dropdown offers.
pub(crate) const UNITS: &[LimitUnit] = &[
    LimitUnit::Tracks,
    LimitUnit::Minutes,
    LimitUnit::Hours,
    LimitUnit::Gigabytes,
    LimitUnit::Megabytes,
];

/// The name of a limit unit in the unit dropdown.
pub(crate) fn unit_label(unit: LimitUnit) -> String {
    tr(match unit {
        LimitUnit::Tracks => "tracks",
        LimitUnit::Minutes => "minutes",
        LimitUnit::Hours => "hours",
        LimitUnit::Megabytes => "MB",
        LimitUnit::Gigabytes => "GB",
    })
}

/// The rules of a smart playlist as the builder shows them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Rules {
    /// Whether any rule is enough, rather than all.
    pub(crate) any: bool,
    pub(crate) nodes: Vec<Node>,
    pub(crate) sort: Vec<SortKey>,
    pub(crate) limit: Option<Limit>,
}

impl Rules {
    /// The rules `query` stands for, if the builder can show them.
    pub(crate) fn from_query(query: &Query) -> Option<Self> {
        let query = query.clone().normalized();
        let (any, nodes) = match &query.filter {
            None => (false, Vec::new()),
            Some(Expr::All(children)) => (
                false,
                children
                    .iter()
                    .map(Node::from_expr)
                    .collect::<Option<Vec<_>>>()?,
            ),
            Some(Expr::Any(children)) if !children.is_empty() => (
                true,
                children
                    .iter()
                    .map(|c| match c {
                        // A group inside "any" is an "all" group.
                        Expr::All(_) => Node::from_expr(c),
                        other => Rule::from_expr(other).map(Node::Rule),
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            Some(other) => (false, vec![Node::from_expr(other)?]),
        };
        Some(Self {
            any,
            nodes,
            sort: query.sort,
            limit: query.limit,
        })
    }

    /// The query of the rules; `None` while a rule's value cannot be read.
    pub(crate) fn query(&self) -> Option<Query> {
        let exprs = self
            .nodes
            .iter()
            .map(Node::expr)
            .collect::<Option<Vec<_>>>()?;
        let filter = if self.any {
            Expr::Any(exprs)
        } else {
            Expr::All(exprs)
        };
        let filter = if self.any && self.nodes.is_empty() {
            None
        } else {
            filter.normalized()
        };
        Some(
            Query {
                filter,
                sort: self.sort.clone(),
                limit: self.limit,
            }
            .normalized(),
        )
    }

    /// The order and limit in words: "Newest first · at most 100 tracks".
    pub(crate) fn order_words(&self) -> String {
        let mut parts = Vec::new();
        if let Some((_, words, _)) = orders().into_iter().find(|(_, _, keys)| *keys == self.sort) {
            if !self.sort.is_empty() {
                parts.push(words);
            }
        } else {
            parts.push(tr("In the order of the text"));
        }
        if let Some(limit) = self.limit {
            let n = limit.amount as usize;
            let amount = limit.amount.to_string();
            parts.push(match limit.unit {
                LimitUnit::Tracks => {
                    fill(&ntr("at most {} track", "at most {} tracks", n), &[&amount])
                }
                LimitUnit::Minutes => fill(
                    &ntr("at most {} minute", "at most {} minutes", n),
                    &[&amount],
                ),
                LimitUnit::Hours => {
                    fill(&ntr("at most {} hour", "at most {} hours", n), &[&amount])
                }
                LimitUnit::Megabytes => fill(&tr("at most {} MB"), &[&amount]),
                LimitUnit::Gigabytes => fill(&tr("at most {} GB"), &[&amount]),
            });
        }
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> Option<Rules> {
        Rules::from_query(&parse_query(text).unwrap())
    }

    /// Text → rules → query → text gives the text back, in canonical form.
    #[test]
    fn rules_round_trip_with_the_text() {
        for text in [
            "rating=0 samplerate>=88200 (codec:flac OR codec:dsd) added<12m sort:added desc limit:100",
            "artist:Kask OR genre=Jazz",
            "title~^Intro -album:live has:genre -has:composer",
            "year>=1970 year<=1979 kind=audiobook sort:random limit:2h",
            "added>2w lastplayed<2024 plays>3 duration<=3:00 size>10MB",
            "(year<1960 album:blue) OR rating>=4",
            "added:2024-05 limit:4GB",
        ] {
            let parsed = parse_query(text).unwrap().normalized();
            let built = rules(text).unwrap_or_else(|| panic!("{text} has no rules"));
            let back = built.query().unwrap();
            assert_eq!(back, parsed, "{text}");
            assert_eq!(print_query(&back), print_query(&parsed));
        }
    }

    #[test]
    fn queries_the_builder_cannot_show_stay_text() {
        for text in [
            "loose words",
            "-(album:b OR genre:d)",
            "year:1990..1999",
            "is:lossless",
        ] {
            assert!(rules(text).is_none(), "{text}");
        }
    }

    #[test]
    fn rules_read_their_values_with_the_parser() {
        let mut rule = Rule::new(Field::SampleRate);
        rule.cond = Cond::AtLeast;
        rule.value = "88.2 kHz".into();
        assert_eq!(
            print_query(&Query::new(rule.expr().unwrap())),
            "samplerate>=88200"
        );
        rule.value = "fast".into();
        assert!(rule.expr().is_none());
        let mut rating = Rule::new(Field::Rating);
        rating.value = "Unrated".into();
        assert_eq!(print_query(&Query::new(rating.expr().unwrap())), "rating=0");
    }

    #[test]
    fn the_rule_line_reads_as_sentences() {
        let built = rules(
            "rating=0 samplerate>=88200 (codec:flac OR codec:dsd) added<12m sort:added desc limit:100",
        )
        .unwrap();
        let chips: Vec<String> = built.nodes.iter().map(Node::sentence).collect();
        assert_eq!(
            chips,
            [
                "Rating is Unrated",
                "Sample rate is at least 88.2 kHz",
                "Codec is FLAC or DSD",
                "Added in the last 12 months",
            ]
        );
        assert_eq!(built.order_words(), "Newest first · at most 100 tracks");
    }
}
