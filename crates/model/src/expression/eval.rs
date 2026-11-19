// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Evaluating templates and expressions for one track.

use std::cmp::Ordering;

use super::{Comparison, FieldValues, Function, MAX_PAD, MAX_TEXT_BYTES, Node, Part, Piece};
use crate::query::Field;

/// Articles that `sortname` moves to the end and `noarticle` drops.
const ARTICLES: &[&str] = &["The", "A", "An"];

/// `text` cut to at most [`MAX_TEXT_BYTES`] at a character boundary.
pub(super) fn cut(text: &mut String) {
    if text.len() > MAX_TEXT_BYTES {
        let mut end = MAX_TEXT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

/// The number of digits of `n`.
fn digits(n: u32) -> usize {
    n.max(1).to_string().len()
}

/// The text of `field` as a naming pattern writes it: empty as `None`, the track
/// number padded, the disc only on albums of several discs.
fn field_text(field: &Field, values: &dyn FieldValues) -> Option<String> {
    let value = values.value(field)?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    match field {
        Field::Track => Some(match value.parse::<u32>() {
            Ok(n) => {
                let width = values.track_count().map_or(2, |t| digits(t).max(2));
                format!("{n:0width$}")
            }
            Err(_) => value.to_owned(),
        }),
        Field::Disc => {
            let n = value.parse::<u32>().ok();
            let most = values.disc_count().unwrap_or(1).max(n.unwrap_or(1));
            if most <= 1 {
                return None;
            }
            Some(match n {
                Some(n) => {
                    let width = digits(most);
                    format!("{n:0width$}")
                }
                None => value.to_owned(),
            })
        }
        _ => Some(value.to_owned()),
    }
}

pub(super) fn collect_fields(parts: &[Part], out: &mut Vec<Field>) {
    for part in parts {
        match part {
            Part::Value(node) => node_fields(node, out),
            Part::Optional(inner) => collect_fields(inner, out),
            Part::Text(_) | Part::Folder(_) => {}
        }
    }
}

fn node_fields(node: &Node, out: &mut Vec<Field>) {
    match node {
        Node::Field(field) | Node::Raw(field) => {
            if !out.contains(field) {
                out.push(field.clone());
            }
        }
        Node::Literal(_) | Node::Variable(_) => {}
        Node::Let(value, body) => {
            node_fields(value, out);
            node_fields(body, out);
        }
        Node::Not(inner) => node_fields(inner, out),
        Node::Compare(_, left, right) => {
            node_fields(left, out);
            node_fields(right, out);
        }
        Node::Call(_, nodes) | Node::And(nodes) | Node::Or(nodes) => {
            for node in nodes {
                node_fields(node, out);
            }
        }
    }
}

fn truth(yes: bool) -> String {
    if yes { "1".to_owned() } else { String::new() }
}

/// A number, when `text` is one.
fn number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty()
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return None;
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// A count argument: a whole number from 0, at most `most`.
fn count(text: &str, most: usize) -> usize {
    number(text).map_or(0, |n| {
        if n <= 0.0 {
            0
        } else {
            // In range: clamped to `most` first.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = n.min(most as f64) as usize;
            n
        }
    })
}

/// The leading `YYYY[-MM[-DD]]` of a date, as its parts.
fn date_parts(text: &str) -> (Option<&str>, Option<&str>, Option<&str>) {
    let text = text.trim();
    fn digits(s: &str, at: usize, n: usize) -> Option<&str> {
        s.get(at..at + n)
            .filter(|d| d.bytes().all(|b| b.is_ascii_digit()))
    }
    let year = digits(text, 0, 4);
    if year.is_none() || text.as_bytes().get(4).is_some_and(u8::is_ascii_digit) {
        return (None, None, None);
    }
    let month = (text.as_bytes().get(4) == Some(&b'-'))
        .then(|| digits(text, 5, 2))
        .flatten();
    let day = (month.is_some() && text.as_bytes().get(7) == Some(&b'-'))
        .then(|| digits(text, 8, 2))
        .flatten();
    (year, month, day)
}

/// A number of seconds as `m:ss` or `h:mm:ss`.
fn time(text: &str) -> String {
    let Some(seconds) = number(text).filter(|n| *n >= 0.0) else {
        return text.trim().to_owned();
    };
    // Whole seconds, clamped to a range that fits.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = seconds.min(1e15) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn article(text: &str) -> Option<(&'static str, &str)> {
    let text = text.trim();
    ARTICLES.iter().find_map(|article| {
        text.strip_prefix(article)
            .and_then(|rest| rest.strip_prefix(' '))
            .map(str::trim)
            .filter(|rest| !rest.is_empty())
            .map(|rest| (*article, rest))
    })
}

fn compare(op: Comparison, left: &str, right: &str) -> bool {
    if op == Comparison::Contains {
        return left.to_lowercase().contains(&right.to_lowercase());
    }
    let order = match (number(left), number(right)) {
        (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        _ => left.trim().to_lowercase().cmp(&right.trim().to_lowercase()),
    };
    match op {
        Comparison::Equal => order == Ordering::Equal,
        Comparison::NotEqual => order != Ordering::Equal,
        Comparison::Less => order == Ordering::Less,
        Comparison::LessOrEqual => order != Ordering::Greater,
        Comparison::Greater => order == Ordering::Greater,
        Comparison::GreaterOrEqual => order != Ordering::Less,
        Comparison::Contains => false,
    }
}

/// Evaluates expressions for one track.
pub(super) struct Evaluator<'a> {
    values: &'a dyn FieldValues,
    /// The values `let` binds, outermost first.
    bound: Vec<String>,
}

impl<'a> Evaluator<'a> {
    pub(super) fn new(values: &'a dyn FieldValues) -> Self {
        Self {
            values,
            bound: Vec::new(),
        }
    }

    /// Gives the pieces of `parts` to `out`.
    pub(super) fn pieces<'p>(&mut self, parts: &'p [Part], out: &mut dyn FnMut(Piece<'p>)) {
        self.walk(parts, None, out);
    }

    /// `known` holds the values of the expressions directly in `parts`, when a
    /// bracket has worked them out already.
    fn walk<'p>(
        &mut self,
        parts: &'p [Part],
        known: Option<Vec<String>>,
        out: &mut dyn FnMut(Piece<'p>),
    ) {
        let mut known = known.map(Vec::into_iter);
        for part in parts {
            match part {
                Part::Text(text) => out(Piece::Text(text)),
                Part::Folder(_) => out(Piece::Folder),
                Part::Value(node) => {
                    let value = match known.as_mut().and_then(Iterator::next) {
                        Some(value) => value,
                        None => self.text(node),
                    };
                    out(Piece::Value((!value.is_empty()).then_some(value)));
                }
                Part::Optional(inner) => {
                    let mut values = Vec::new();
                    let complete = inner.iter().all(|part| match part {
                        Part::Value(node) => {
                            let value = self.text(node);
                            let present = !value.is_empty();
                            values.push(value);
                            present
                        }
                        _ => true,
                    });
                    if complete {
                        self.walk(inner, Some(values), out);
                    }
                }
            }
        }
    }

    /// The values of a field of several values, or the one value of anything else.
    fn list(&mut self, node: &Node) -> Vec<String> {
        match node {
            Node::Field(field) | Node::Raw(field) => self
                .values
                .values(field)
                .into_iter()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .collect(),
            other => {
                let text = self.text(other);
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![text]
                }
            }
        }
    }

    /// The value of `node`.
    pub(super) fn text(&mut self, node: &Node) -> String {
        let mut text = match node {
            Node::Literal(text) => text.clone(),
            Node::Field(field) => field_text(field, self.values).unwrap_or_default(),
            Node::Raw(field) => self
                .values
                .value(field)
                .map(|v| v.trim().to_owned())
                .unwrap_or_default(),
            Node::Variable(index) => self.bound.get(*index).cloned().unwrap_or_default(),
            Node::Let(value, body) => {
                let value = self.text(value);
                self.bound.push(value);
                let text = self.text(body);
                self.bound.pop();
                text
            }
            Node::Not(inner) => truth(self.text(inner).is_empty()),
            Node::And(nodes) => {
                let mut last = String::new();
                for node in nodes {
                    last = self.text(node);
                    if last.is_empty() {
                        break;
                    }
                }
                last
            }
            Node::Or(nodes) => {
                let mut last = String::new();
                for node in nodes {
                    last = self.text(node);
                    if !last.is_empty() {
                        break;
                    }
                }
                last
            }
            Node::Compare(op, left, right) => {
                let left = self.text(left);
                let right = self.text(right);
                truth(compare(*op, &left, &right))
            }
            Node::Call(function, arguments) => self.call(*function, arguments),
        };
        cut(&mut text);
        text
    }

    fn argument(&mut self, arguments: &[Node], index: usize) -> String {
        arguments
            .get(index)
            .map(|node| self.text(node))
            .unwrap_or_default()
    }

    fn call(&mut self, function: Function, arguments: &[Node]) -> String {
        let mut arg = |i: usize| self.argument(arguments, i);
        match function {
            Function::If => {
                if arg(0).is_empty() {
                    arg(2)
                } else {
                    arg(1)
                }
            }
            Function::Concat => {
                let mut text = String::new();
                for node in arguments {
                    text.push_str(&self.text(node));
                    if text.len() > MAX_TEXT_BYTES {
                        break;
                    }
                }
                text
            }
            Function::Upper => arg(0).to_uppercase(),
            Function::Lower => arg(0).to_lowercase(),
            Function::Left => {
                let text = arg(0);
                let n = count(&arg(1), MAX_TEXT_BYTES);
                text.chars().take(n).collect()
            }
            Function::Right => {
                let text = arg(0);
                let n = count(&arg(1), MAX_TEXT_BYTES);
                let len = text.chars().count();
                text.chars().skip(len.saturating_sub(n)).collect()
            }
            Function::Substr => {
                let text = arg(0);
                let start = count(&arg(1), MAX_TEXT_BYTES).max(1);
                let length = if arguments.len() > 2 {
                    count(&arg(2), MAX_TEXT_BYTES)
                } else {
                    usize::MAX
                };
                text.chars().skip(start - 1).take(length).collect()
            }
            Function::Len => arg(0).chars().count().to_string(),
            Function::Trim => arg(0).trim().to_owned(),
            Function::Replace => {
                let (text, from, to) = (arg(0), arg(1), arg(2));
                if from.is_empty() {
                    return text;
                }
                // The result can grow by `to` for each `from`: stop at the limit.
                let mut out = String::new();
                let mut rest = text.as_str();
                while let Some(at) = rest.find(&from) {
                    out.push_str(&rest[..at]);
                    out.push_str(&to);
                    rest = &rest[at + from.len()..];
                    if out.len() > MAX_TEXT_BYTES {
                        return out;
                    }
                }
                out.push_str(rest);
                out
            }
            Function::Pad => {
                let mut text = arg(0);
                let width = count(&arg(1), MAX_PAD);
                let fill = if arguments.len() > 2 {
                    arg(2).chars().next().unwrap_or(' ')
                } else {
                    ' '
                };
                let len = text.chars().count();
                text.extend(std::iter::repeat_n(fill, width.saturating_sub(len)));
                text
            }
            Function::Num => {
                let text = arg(0);
                let width = count(&arg(1), MAX_PAD);
                match text.trim().parse::<i64>() {
                    Ok(n) => format!("{n:0width$}"),
                    Err(_) => text,
                }
            }
            Function::FirstWord => arg(0)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned(),
            Function::SortName => {
                let text = arg(0);
                match article(&text) {
                    Some((article, rest)) => format!("{rest}, {article}"),
                    None => text,
                }
            }
            Function::NoArticle => {
                let text = arg(0);
                match article(&text) {
                    Some((_, rest)) => rest.to_owned(),
                    None => text,
                }
            }
            Function::Initial => match arg(0).chars().find(|c| c.is_alphanumeric()) {
                Some(c) if c.is_alphabetic() => c.to_uppercase().collect(),
                Some(_) => "#".to_owned(),
                None => String::new(),
            },
            Function::Year => date_parts(&arg(0)).0.unwrap_or_default().to_owned(),
            Function::Month => date_parts(&arg(0)).1.unwrap_or_default().to_owned(),
            Function::Day => date_parts(&arg(0)).2.unwrap_or_default().to_owned(),
            Function::Date => {
                let text = arg(0);
                match date_parts(&text) {
                    (Some(y), Some(m), Some(d)) => format!("{y}-{m}-{d}"),
                    (Some(y), Some(m), None) => format!("{y}-{m}"),
                    (Some(y), None, _) => y.to_owned(),
                    _ => String::new(),
                }
            }
            Function::Time => time(&arg(0)),
            Function::Join => {
                let separator = if arguments.len() > 1 {
                    arg(1)
                } else {
                    ", ".to_owned()
                };
                let list = arguments
                    .first()
                    .map(|node| self.list(node))
                    .unwrap_or_default();
                let mut text = String::new();
                for (i, value) in list.iter().enumerate() {
                    if i > 0 {
                        text.push_str(&separator);
                    }
                    text.push_str(value);
                    if text.len() > MAX_TEXT_BYTES {
                        break;
                    }
                }
                text
            }
            Function::First | Function::Last | Function::Count => {
                let list = arguments
                    .first()
                    .map(|node| self.list(node))
                    .unwrap_or_default();
                match function {
                    Function::First => list.into_iter().next().unwrap_or_default(),
                    Function::Last => list.into_iter().last().unwrap_or_default(),
                    _ => list.len().to_string(),
                }
            }
            // Read into their own nodes; never called.
            Function::Raw | Function::Let => String::new(),
        }
    }
}
