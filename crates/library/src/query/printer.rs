// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing a query tree as text that reads back to the same tree.
//!
//! For every tree in canonical form (see [`Expr::normalized`]) whose comparisons pass
//! [`Comparison::check`](helikodu_model::query::Comparison::check),
//! `parse(&print(&tree)) == Ok(tree)`. Words are written bare where the parser reads
//! them back as they are, and in quotes otherwise.

use helikodu_model::query::{Comparison, Expr, FieldKind, Query, SortBy, Value};

use super::parser::{identifier_len, keyword_at, operator_at};
use super::values;

/// The text form of `query`: the filter, then the sort keys, then the limit.
pub fn print(query: &Query) -> String {
    let mut parts = Vec::new();
    if let Some(filter) = &query.filter {
        parts.push(expr(filter));
    }
    if !query.sort.is_empty() {
        let keys: Vec<String> = query
            .sort
            .iter()
            .map(|key| {
                let name = match &key.by {
                    SortBy::Field(field) => field.name().into_owned(),
                    SortBy::Random => "random".to_owned(),
                };
                if key.descending {
                    format!("{name} desc")
                } else {
                    name
                }
            })
            .collect();
        parts.push(format!("sort:{}", keys.join(",")));
    }
    if let Some(limit) = &query.limit {
        parts.push(format!("limit:{}{}", limit.amount, limit.unit.suffix()));
    }
    parts.join(" ")
}

fn expr(e: &Expr) -> String {
    match e {
        Expr::All(children) if children.is_empty() => "()".to_owned(),
        Expr::All(children) => children
            .iter()
            .map(|child| match child {
                Expr::Any(inner) if !inner.is_empty() => format!("({})", expr(child)),
                _ => expr(child),
            })
            .collect::<Vec<_>>()
            .join(" "),
        // No alternative: no track. It reads back from "not everything".
        Expr::Any(children) if children.is_empty() => "-()".to_owned(),
        Expr::Any(children) => children.iter().map(expr).collect::<Vec<_>>().join(" OR "),
        Expr::Not(inner) => format!("-{}", atom(inner)),
        Expr::Compare(comparison) => compare(comparison),
        Expr::Text(text) => bare_text(text),
        Expr::Has(presence) => format!("has:{}", presence.name()),
        Expr::Is(flag) => format!("is:{}", flag.name()),
        _ => String::new(),
    }
}

/// `e` as the operand of a negation.
fn atom(e: &Expr) -> String {
    match e {
        Expr::All(children) | Expr::Any(children) if !children.is_empty() => {
            format!("({})", expr(e))
        }
        _ => expr(e),
    }
}

fn compare(c: &Comparison) -> String {
    let value = match (&c.value, c.field.kind()) {
        (Value::Text(text), FieldKind::Text) => value_text(text),
        (value, _) => values::print_single(value),
    };
    format!("{}{}{value}", c.field.name(), c.op.symbol())
}

/// A text value after an operator: bare when the parser reads it back whole.
fn value_text(text: &str) -> String {
    let bare = !text.is_empty()
        && !text
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '(' | ')'))
        && !text.starts_with([':', '=', '!', '<', '>', '~']);
    if bare { text.to_owned() } else { quote(text) }
}

/// Plain text: bare when the parser reads it back as plain text.
fn bare_text(text: &str) -> String {
    let ident = identifier_len(text);
    let bare = !text.is_empty()
        && !text
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '(' | ')'))
        && !text.starts_with(['-', '!'])
        && keyword_at(text).is_none()
        && !(ident > 0 && operator_at(&text[ident..]).is_some());
    if bare { text.to_owned() } else { quote(text) }
}

/// `text` in quotes. A quote is escaped, and so is a backslash before a quote, before
/// another backslash or at the end; other backslashes are written as they are.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' if matches!(chars.peek(), None | Some('"' | '\\')) => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
