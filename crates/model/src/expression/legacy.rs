// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The reader of naming patterns from before the expression language, kept
//! unchanged as the reference that the compatibility tests and the fuzz target
//! compare the expression language with: every pattern it reads must read the
//! same and give the same pieces.
//!
//! It is self-contained apart from `Field` and `FieldValues`, which the module
//! that includes it provides.

use super::{Field, FieldValues};

/// The deepest square brackets may nest.
const MAX_BRACKET_DEPTH: usize = 16;

/// One piece of output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Piece {
    Text(String),
    Value(Option<String>),
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Field(Field),
    Folder,
    Optional(Vec<Part>),
}

/// A pattern as the old reader read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pattern {
    parts: Vec<Part>,
}

fn digits(n: u32) -> usize {
    n.max(1).to_string().len()
}

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

impl Pattern {
    /// Reads `text`; `None` when the old reader refused it.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let chars: Vec<char> = text.chars().collect();
        if text.trim().is_empty() {
            return None;
        }
        let mut position = 0;
        let parts = parse_parts(&chars, &mut position, None, 0)?;
        check_folders(&chars)?;
        Some(Self { parts })
    }

    /// The fields, in order, each once.
    pub(crate) fn fields(&self) -> Vec<Field> {
        let mut out = Vec::new();
        collect(&self.parts, &mut out);
        out
    }

    /// The pieces of the output for `values`.
    pub(crate) fn pieces(&self, values: &dyn FieldValues) -> Vec<Piece> {
        let mut out = Vec::new();
        render(&self.parts, values, &mut out);
        out
    }
}

fn collect(parts: &[Part], out: &mut Vec<Field>) {
    for part in parts {
        match part {
            Part::Field(field) if !out.contains(field) => out.push(field.clone()),
            Part::Optional(inner) => collect(inner, out),
            _ => {}
        }
    }
}

fn parse_parts(
    chars: &[char],
    position: &mut usize,
    open: Option<usize>,
    depth: usize,
) -> Option<Vec<Part>> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, parts: &mut Vec<Part>| {
        if !text.is_empty() {
            parts.push(Part::Text(std::mem::take(text)));
        }
    };
    while let Some(&c) = chars.get(*position) {
        match c {
            '{' => {
                flush(&mut text, &mut parts);
                let mut name = String::new();
                *position += 1;
                loop {
                    match chars.get(*position) {
                        None | Some('{') => return None,
                        Some('}') => break,
                        Some(&c) => name.push(c),
                    }
                    *position += 1;
                }
                *position += 1;
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    return None;
                }
                parts.push(Part::Field(Field::from_name(trimmed)?));
            }
            '[' => {
                flush(&mut text, &mut parts);
                if depth >= MAX_BRACKET_DEPTH {
                    return None;
                }
                let start = *position;
                *position += 1;
                let inner = parse_parts(chars, position, Some(start), depth + 1)?;
                parts.push(Part::Optional(inner));
            }
            ']' => {
                if open.is_some() {
                    flush(&mut text, &mut parts);
                    *position += 1;
                    return Some(parts);
                }
                return None;
            }
            '}' => return None,
            '/' => {
                flush(&mut text, &mut parts);
                parts.push(Part::Folder);
                *position += 1;
            }
            c => {
                text.push(c);
                *position += 1;
            }
        }
    }
    if open.is_some() {
        return None;
    }
    flush(&mut text, &mut parts);
    Some(parts)
}

fn check_folders(chars: &[char]) -> Option<()> {
    let mut seen_content = false;
    let mut in_field = false;
    for &c in chars {
        match c {
            '{' => {
                in_field = true;
                seen_content = true;
            }
            '}' => in_field = false,
            '/' if !in_field => {
                if !seen_content {
                    return None;
                }
                seen_content = false;
            }
            '[' | ']' => {}
            c if c.is_whitespace() => {}
            _ => seen_content = true,
        }
    }
    seen_content.then_some(())
}

fn all_present(parts: &[Part], values: &dyn FieldValues) -> bool {
    parts.iter().all(|part| match part {
        Part::Field(field) => field_text(field, values).is_some(),
        _ => true,
    })
}

fn render(parts: &[Part], values: &dyn FieldValues, out: &mut Vec<Piece>) {
    for part in parts {
        match part {
            Part::Text(text) => out.push(Piece::Text(text.clone())),
            Part::Field(field) => out.push(Piece::Value(field_text(field, values))),
            Part::Folder => out.push(Piece::Folder),
            Part::Optional(inner) => {
                if all_present(inner, values) {
                    render(inner, values, out);
                }
            }
        }
    }
}
