// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading query text into the query tree.
//!
//! ```text
//! query    := or_expr, with "sort:" keys and "limit:" anywhere at the top level
//! or_expr  := and_expr ( ("OR" | "|" | "||") and_expr )*
//! and_expr := unary ( ["AND" | "&" | "&&"] unary )*      -- juxtaposition is AND
//! unary    := ("NOT" | "-" | "!") unary | "(" or_expr ")" | term
//! term     := field op value | "has:" name | "is:" name | text
//! op       := ":" | "=" | "!=" | "<" | "<=" | ">" | ">=" | "~"
//! ```
//!
//! A value means what its field makes of it (see `values`). The parser reports the
//! first problem with the byte range of the text that fails; [`parse_lenient`] falls
//! back to a plain search of the words instead.

use std::ops::Range;

use helikodu_model::query::{
    Comparison, Expr, Field, FieldKind, Flag, Limit, LimitUnit, Operator, Presence, Query, SortBy,
    SortKey, TermProblem, Value,
};

use super::syntax_error::{SyntaxError, SyntaxErrorKind as Kind};
use super::values;

type Result<T> = std::result::Result<T, SyntaxError>;

/// How deeply parentheses and negations may nest.
const MAX_DEPTH: usize = 64;

/// The most memory a regular expression may compile to, as in the `regexp()` function.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Reads `text` into a query in canonical form (see [`Expr::normalized`]).
pub fn parse(text: &str) -> Result<Query> {
    let mut parser = Parser {
        src: text,
        pos: 0,
        depth: 0,
        sort: None,
        limit: None,
    };
    let filter = parser.or_expr()?;
    parser.skip_ws();
    if parser.pos < text.len() {
        // Only an unmatched `)` stops the top level early.
        return Err(parser.error(parser.pos..parser.pos + 1, Kind::UnexpectedClose));
    }
    Ok(Query {
        filter: filter.and_then(Expr::normalized),
        sort: parser.sort.unwrap_or_default(),
        limit: parser.limit,
    })
}

/// Query text read without failing: the query, and the problem if the text could
/// not be read as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    /// The query; a plain search of the words when the text could not be read.
    pub query: Query,
    /// Why the text could not be read as written, and where.
    pub error: Option<SyntaxError>,
}

/// Reads `text`, or, if it cannot be read, searches for its words as plain text and
/// reports why.
pub fn parse_lenient(text: &str) -> ParsedQuery {
    match parse(text) {
        Ok(query) => ParsedQuery { query, error: None },
        Err(error) => ParsedQuery {
            query: plain_text(text),
            error: Some(error),
        },
    }
}

/// A plain search of the words of `text`.
fn plain_text(text: &str) -> Query {
    let words: Vec<Expr> = text
        .split_whitespace()
        .map(|word| word.trim_matches('"'))
        .filter(|word| !word.is_empty())
        .map(Expr::text)
        .collect();
    Query {
        filter: Expr::All(words).normalized(),
        ..Query::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Keyword {
    And,
    Or,
    Not,
}

/// The keyword at the start of `rest`, with its length.
pub(super) fn keyword_at(rest: &str) -> Option<(Keyword, usize)> {
    const KEYWORDS: [(&str, Keyword); 7] = [
        ("AND", Keyword::And),
        ("OR", Keyword::Or),
        ("NOT", Keyword::Not),
        ("&&", Keyword::And),
        ("||", Keyword::Or),
        ("&", Keyword::And),
        ("|", Keyword::Or),
    ];
    KEYWORDS.iter().find_map(|(word, keyword)| {
        (rest.starts_with(word) && ends_token(&rest[word.len()..]))
            .then_some((*keyword, word.len()))
    })
}

/// Whether a keyword or name ends before `rest`.
fn ends_token(rest: &str) -> bool {
    rest.chars()
        .next()
        .is_none_or(|c| c.is_whitespace() || matches!(c, '(' | ')' | '"'))
}

/// The operator at the start of `rest`, with its length. `:` followed by another
/// operator (`rating:>=4`) is that operator.
pub(super) fn operator_at(rest: &str) -> Option<(Operator, usize)> {
    let (op, len) = strict_operator_at(rest)?;
    if op == Operator::Contains
        && let Some((next, next_len)) = strict_operator_at(&rest[1..])
        && next != Operator::Contains
    {
        return Some((next, len + next_len));
    }
    Some((op, len))
}

fn strict_operator_at(rest: &str) -> Option<(Operator, usize)> {
    const OPERATORS: [(&str, Operator); 9] = [
        ("!=", Operator::NotEquals),
        ("<=", Operator::LessOrEqual),
        (">=", Operator::GreaterOrEqual),
        ("==", Operator::Equals),
        (":", Operator::Contains),
        ("=", Operator::Equals),
        ("<", Operator::Less),
        (">", Operator::Greater),
        ("~", Operator::Matches),
    ];
    OPERATORS
        .iter()
        .find_map(|(symbol, op)| rest.starts_with(symbol).then_some((*op, symbol.len())))
}

/// The length of the leading ASCII letters and underscores of `rest`: a field name.
pub(super) fn identifier_len(rest: &str) -> usize {
    rest.find(|c: char| !(c.is_ascii_alphabetic() || c == '_'))
        .unwrap_or(rest.len())
}

/// Whether `op` can compare a field of `kind` at all.
fn operator_fits(kind: FieldKind, op: Operator) -> bool {
    match kind {
        FieldKind::Text => matches!(
            op,
            Operator::Contains | Operator::Equals | Operator::NotEquals | Operator::Matches
        ),
        FieldKind::Kind => matches!(
            op,
            Operator::Contains | Operator::Equals | Operator::NotEquals
        ),
        _ => op != Operator::Matches,
    }
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    /// Parentheses and negations around the current position.
    depth: usize,
    sort: Option<Vec<SortKey>>,
    limit: Option<Limit>,
}

impl Parser<'_> {
    fn error(&self, span: Range<usize>, kind: Kind) -> SyntaxError {
        SyntaxError::new(span, kind)
    }

    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn skip_ws(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    /// The span of the character at `at`, or of the last character at the end.
    fn char_span(&self, at: usize) -> Range<usize> {
        match self.src[at..].chars().next() {
            Some(c) => at..at + c.len_utf8(),
            None => {
                let last = self.src[..at].chars().next_back().map_or(0, char::len_utf8);
                at - last..at
            }
        }
    }

    fn enter(&mut self, span: Range<usize>) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error(span, Kind::TooDeep));
        }
        Ok(())
    }

    fn or_expr(&mut self) -> Result<Option<Expr>> {
        let mut alternatives = Vec::new();
        let mut pending = self.and_expr()?;
        loop {
            self.skip_ws();
            let Some((Keyword::Or, len)) = keyword_at(self.rest()) else {
                break;
            };
            let span = self.pos..self.pos + len;
            let Some(left) = pending.take() else {
                return Err(self.error(span, Kind::MissingOperand("OR")));
            };
            alternatives.push(left);
            self.pos += len;
            pending = self.and_expr()?;
            if pending.is_none() {
                return Err(self.error(span, Kind::MissingOperand("OR")));
            }
        }
        if alternatives.is_empty() {
            return Ok(pending);
        }
        alternatives.extend(pending);
        Ok(Some(Expr::Any(alternatives)))
    }

    fn and_expr(&mut self) -> Result<Option<Expr>> {
        let mut terms = Vec::new();
        let mut seen = false;
        loop {
            self.skip_ws();
            match self.peek() {
                None | Some(')') => break,
                Some(_) => {}
            }
            match keyword_at(self.rest()) {
                Some((Keyword::Or, _)) => break,
                Some((Keyword::And, len)) => {
                    let span = self.pos..self.pos + len;
                    self.pos += len;
                    self.skip_ws();
                    let follows = !matches!(self.peek(), None | Some(')'))
                        && !matches!(
                            keyword_at(self.rest()),
                            Some((Keyword::And | Keyword::Or, _))
                        );
                    if !seen || !follows {
                        return Err(self.error(span, Kind::MissingOperand("AND")));
                    }
                    continue;
                }
                _ => {}
            }
            seen = true;
            if let Some(expr) = self.unary()? {
                terms.push(expr);
            }
        }
        Ok(match terms.len() {
            0 => None,
            1 => terms.pop(),
            _ => Some(Expr::All(terms)),
        })
    }

    fn unary(&mut self) -> Result<Option<Expr>> {
        let start = self.pos;
        match keyword_at(self.rest()) {
            Some((Keyword::Not, len)) => return self.negation(start, len, "NOT"),
            Some((keyword, len)) => {
                let name = if keyword == Keyword::And { "AND" } else { "OR" };
                return Err(self.error(start..start + len, Kind::MissingOperand(name)));
            }
            None => {}
        }
        match self.peek() {
            Some('(') => self.group(),
            Some('-') => self.negation(start, 1, "-"),
            Some('!') => self.negation(start, 1, "!"),
            Some('"') => {
                let (text, span) = self.quoted()?;
                if text.is_empty() {
                    return Err(self.error(span, Kind::EmptyText));
                }
                Ok(Some(Expr::Text(text)))
            }
            _ => self.word(),
        }
    }

    fn negation(&mut self, start: usize, len: usize, name: &'static str) -> Result<Option<Expr>> {
        let span = start..start + len;
        self.pos += len;
        if name == "NOT" {
            self.skip_ws();
        }
        let missing = match self.peek() {
            None => true,
            Some(c) => c.is_whitespace() || c == ')',
        } || matches!(
            keyword_at(self.rest()),
            Some((Keyword::And | Keyword::Or, _))
        );
        if missing {
            return Err(self.error(span, Kind::MissingOperand(name)));
        }
        self.enter(span.clone())?;
        let inner = self.unary()?;
        self.depth -= 1;
        // `sort:` and `limit:` refuse to be negated themselves, so there is an operand.
        Ok(inner.map(Expr::negate))
    }

    fn group(&mut self) -> Result<Option<Expr>> {
        let open = self.pos..self.pos + 1;
        self.pos += 1;
        self.enter(open.clone())?;
        let inner = self.or_expr()?;
        self.skip_ws();
        if self.peek() != Some(')') {
            return Err(self.error(open, Kind::UnclosedGroup));
        }
        self.pos += 1;
        self.depth -= 1;
        Ok(Some(inner.unwrap_or(Expr::All(Vec::new()))))
    }

    /// The text in quotes at the position: `\"` is a quote and `\\` a backslash; any
    /// other backslash is kept, so that regular expressions need no doubling.
    fn quoted(&mut self) -> Result<(String, Range<usize>)> {
        let start = self.pos;
        let mut text = String::new();
        let mut chars = self.src[start + 1..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.pos = start + 1 + i + 1;
                    return Ok((text, start..self.pos));
                }
                '\\' => match chars.clone().next() {
                    Some((_, escaped @ ('"' | '\\'))) => {
                        chars.next();
                        text.push(escaped);
                    }
                    _ => text.push('\\'),
                },
                c => text.push(c),
            }
        }
        Err(self.error(start..self.src.len(), Kind::UnclosedQuote))
    }

    /// A field term, `has:`, `is:`, `sort:`, `limit:`, or a bare word of text.
    fn word(&mut self) -> Result<Option<Expr>> {
        let start = self.pos;
        let ident_len = identifier_len(self.rest());
        if ident_len > 0 {
            let ident = &self.src[start..start + ident_len];
            let after = start + ident_len;
            let colon = self.src[after..].starts_with(':');
            match ident.to_ascii_lowercase().as_str() {
                "sort" if colon => return self.sort(start, after + 1).map(|()| None),
                "limit" if colon => return self.limit(start, after + 1).map(|()| None),
                "has" if colon => return self.presence(start, after + 1).map(Some),
                "is" if colon => return self.flag(start, after + 1).map(Some),
                "tag" if colon => return self.tag_term(start, after + 1).map(Some),
                _ => {}
            }
            if let Some((op, op_len)) = operator_at(&self.src[after..]) {
                let Some(field) = Field::from_name(ident) else {
                    return Err(self.error(start..after, Kind::UnknownField(ident.to_owned())));
                };
                return self.term(start, field, after, op, op_len).map(Some);
            }
        }
        let rest = self.rest();
        let len = rest
            .find(|c: char| c.is_whitespace() || c == '(' || c == ')')
            .unwrap_or(rest.len());
        let word = rest[..len].to_owned();
        self.pos += len;
        Ok(Some(Expr::Text(word)))
    }

    /// The name after `prefix:` at `from`: letters, digits, `_`, `-` and `:`.
    fn name_after(&mut self, from: usize) -> Range<usize> {
        let rest = &self.src[from..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':')))
            .unwrap_or(rest.len());
        self.pos = from + len;
        from..from + len
    }

    fn presence(&mut self, start: usize, from: usize) -> Result<Expr> {
        let name = self.name_after(from);
        if name.is_empty() {
            return Err(self.error(start..from, Kind::MissingValue("has".into())));
        }
        let text = &self.src[name.clone()];
        Presence::from_name(text).map(Expr::Has).ok_or_else(|| {
            self.error(
                name.clone(),
                Kind::UnknownName {
                    prefix: "has",
                    name: text.to_owned(),
                },
            )
        })
    }

    fn flag(&mut self, start: usize, from: usize) -> Result<Expr> {
        let name = self.name_after(from);
        if name.is_empty() {
            return Err(self.error(start..from, Kind::MissingValue("is".into())));
        }
        let text = &self.src[name.clone()];
        Flag::from_name(text).map(Expr::Is).ok_or_else(|| {
            self.error(
                name.clone(),
                Kind::UnknownName {
                    prefix: "is",
                    name: text.to_owned(),
                },
            )
        })
    }

    /// `tag:NAME` followed by an operator and a value.
    fn tag_term(&mut self, start: usize, from: usize) -> Result<Expr> {
        let rest = &self.src[from..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(rest.len());
        let end = from + len;
        let Some(field) = Field::tag(&rest[..len]) else {
            return Err(self.error(start..from, Kind::UnknownField("tag:".into())));
        };
        let Some((op, op_len)) = operator_at(&self.src[end..]) else {
            return Err(self.error(start..end, Kind::MissingOperator(field.name().into())));
        };
        self.term(start, field, end, op, op_len)
    }

    /// `field op value`, with the field from `start` to `op_at`.
    fn term(
        &mut self,
        start: usize,
        field: Field,
        op_at: usize,
        mut op: Operator,
        op_len: usize,
    ) -> Result<Expr> {
        let name = self.src[start..op_at].to_owned();
        let op_span = op_at..op_at + op_len;
        if !operator_fits(field.kind(), op) {
            return Err(self.error(
                op_span.clone(),
                Kind::InvalidOperator {
                    field: name,
                    op: self.src[op_span].to_owned(),
                },
            ));
        }
        self.pos = op_span.end;
        let value_start = self.pos;
        let quoted = match self.peek() {
            None => {
                return Err(self.error(start..op_span.end, Kind::MissingValue(name)));
            }
            Some(c) if c.is_whitespace() || c == ')' => {
                return Err(self.error(start..op_span.end, Kind::MissingValue(name)));
            }
            Some('"') => Some(self.quoted()?.0),
            Some(_) => None,
        };
        let raw = match &quoted {
            Some(text) => text.clone(),
            None => self.bare_value(),
        };
        let value_span = value_start..self.pos;
        let expected = values::expected(&field);
        let invalid = |parser: &Self, span: Range<usize>| {
            parser.error(
                span,
                Kind::InvalidValue {
                    field: name.clone(),
                    expected: expected.what,
                    example: expected.example,
                },
            )
        };
        let kind = field.kind();
        let value = if kind == FieldKind::Text {
            if op == Operator::Matches {
                check_regex(&raw)
                    .map_err(|reason| self.error(value_span.clone(), Kind::InvalidRegex(reason)))?;
            }
            Value::Text(raw)
        } else if let (None, Some((low, high))) = (&quoted, raw.split_once("..")) {
            if !matches!(op, Operator::Contains | Operator::Equals) {
                return Err(self.error(op_span, Kind::RangeOperator));
            }
            op = Operator::Contains;
            let low_span = value_start..value_start + low.len();
            let high_span = low_span.end + 2..value_span.end;
            let low = values::parse_single(&field, low)
                .filter(|_| kind != FieldKind::Kind)
                .ok_or_else(|| invalid(self, low_span))?;
            let high = values::parse_single(&field, high)
                .ok_or_else(|| invalid(self, high_span.clone()))?;
            if std::mem::discriminant(&low) != std::mem::discriminant(&high) {
                return Err(invalid(self, high_span));
            }
            Value::range(low, high)
        } else {
            values::parse_single(&field, &raw).ok_or_else(|| invalid(self, value_span.clone()))?
        };
        let comparison = Comparison { field, op, value };
        match comparison.check() {
            Ok(()) => Ok(Expr::Compare(comparison)),
            Err(TermProblem::Operator | TermProblem::NoRange) => Err(self.error(
                op_span.clone(),
                Kind::InvalidOperator {
                    field: name.clone(),
                    op: self.src[op_span].to_owned(),
                },
            )),
            Err(_) => Err(invalid(self, value_span)),
        }
    }

    /// A value without quotes: up to white space, or a `)` that closes no `(` of the
    /// value itself (so `title~^(a|b)$` works without quotes).
    fn bare_value(&mut self) -> String {
        let rest = self.rest();
        let mut depth = 0_usize;
        let mut end = rest.len();
        for (i, c) in rest.char_indices() {
            match c {
                c if c.is_whitespace() => {
                    end = i;
                    break;
                }
                '(' => depth += 1,
                ')' if depth == 0 => {
                    end = i;
                    break;
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        let value = rest[..end].to_owned();
        self.pos += end;
        value
    }

    /// `sort:key [asc|desc], …` with the keys from `from`.
    fn sort(&mut self, start: usize, from: usize) -> Result<()> {
        self.pos = from;
        let mut keys = Vec::new();
        loop {
            let mut descending = false;
            if self.peek() == Some('-') {
                descending = true;
                self.pos += 1;
            }
            let name = self.name_after(self.pos);
            if name.is_empty() {
                let span = start..self.pos;
                let span = if keys.is_empty() {
                    span
                } else {
                    self.char_span(self.pos)
                };
                return Err(self.error(span, Kind::MissingValue("sort".into())));
            }
            let text = &self.src[name.clone()];
            let by = if text.eq_ignore_ascii_case("random") {
                SortBy::Random
            } else {
                Field::from_name(text).map(SortBy::Field).ok_or_else(|| {
                    self.error(name.clone(), Kind::UnknownSortKey(text.to_owned()))
                })?
            };
            let save = self.pos;
            self.skip_ws();
            let rest = self.rest();
            let direction = ["asc", "desc"].into_iter().find(|word| {
                rest.get(..word.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(word))
                    && rest[word.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| c.is_whitespace() || c == ',' || c == ')')
            });
            match direction {
                Some(word) => {
                    descending = word == "desc";
                    self.pos += word.len();
                }
                None => self.pos = save,
            }
            keys.push(SortKey { by, descending });
            let save = self.pos;
            self.skip_ws();
            if self.peek() == Some(',') {
                self.pos += 1;
                self.skip_ws();
            } else {
                self.pos = save;
                break;
            }
        }
        let span = start..self.pos;
        if self.depth > 0 {
            return Err(self.error(span, Kind::Misplaced("sort")));
        }
        if self.sort.is_some() {
            return Err(self.error(span, Kind::Duplicate("sort")));
        }
        self.sort = Some(keys);
        Ok(())
    }

    /// `limit:N[unit]` with the amount from `from`.
    fn limit(&mut self, start: usize, from: usize) -> Result<()> {
        let rest = &self.src[from..];
        let len = rest
            .find(|c: char| c.is_whitespace() || c == ')')
            .unwrap_or(rest.len());
        let token = &rest[..len];
        self.pos = from + len;
        let span = start..self.pos;
        if token.is_empty() {
            return Err(self.error(span, Kind::MissingValue("limit".into())));
        }
        let digits = token
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(token.len());
        let amount = token[..digits].parse::<u32>().ok();
        let unit = LimitUnit::from_suffix(&token[digits..]);
        let (Some(amount), Some(unit)) = (amount, unit) else {
            return Err(self.error(from..self.pos, Kind::InvalidLimit));
        };
        if self.depth > 0 {
            return Err(self.error(span, Kind::Misplaced("limit")));
        }
        if self.limit.is_some() {
            return Err(self.error(span, Kind::Duplicate("limit")));
        }
        self.limit = Some(Limit { amount, unit });
        Ok(())
    }
}

/// Whether `pattern` compiles as the `regexp()` function will compile it.
fn check_regex(pattern: &str) -> std::result::Result<(), String> {
    regex::RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map(drop)
        .map_err(|error| {
            let text = error.to_string();
            let last = text.lines().last().unwrap_or_default();
            last.trim_start_matches("error: ").to_owned()
        })
}
