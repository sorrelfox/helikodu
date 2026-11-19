// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A small element tree read with `quick-xml`, for the players' XML files.
//!
//! Element names are kept without their namespace prefix. Only the predefined
//! entities and character references are expanded; a document type's entities
//! are never expanded. Nesting deeper than [`MAX_DEPTH`] is refused.

use std::borrow::Cow;
use std::path::Path;

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::ImportError;

/// The deepest nesting read.
const MAX_DEPTH: usize = 256;

/// An element with its attributes, text and children.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Element {
    pub(super) name: String,
    pub(super) attributes: Vec<(String, String)>,
    pub(super) text: String,
    pub(super) children: Vec<Element>,
}

impl Element {
    /// The value of the attribute `name`.
    pub(super) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The first child named `name`.
    pub(super) fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The children named `name`.
    pub(super) fn children_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The trimmed text of the first child named `name`, if it is not empty.
    pub(super) fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name)
            .map(|c| c.text.trim())
            .filter(|t| !t.is_empty())
    }
}

fn start(element: &BytesStart<'_>) -> Element {
    let name = element.local_name().as_ref().to_owned();
    let attributes = element
        .attributes()
        .with_checks(false)
        .filter_map(Result::ok)
        .map(|a| {
            let key = a.key.local_name().as_ref().to_owned();
            let value = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map_or_else(|_| a.value.clone().into_owned(), Cow::into_owned);
            (key, value)
        })
        .collect();
    Element {
        name,
        attributes,
        ..Element::default()
    }
}

/// Reads the XML file at `path` into its root element.
pub(super) fn read(path: &Path) -> Result<Element, ImportError> {
    let bytes = std::fs::read(path).map_err(|source| ImportError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse(&bytes).map_err(|reason| ImportError::Format {
        path: path.to_path_buf(),
        reason,
    })
}

/// Reads an XML document into its root element.
pub(super) fn parse(bytes: &[u8]) -> Result<Element, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut reader = Reader::from_str(&text);
    reader.config_mut().expand_empty_elements = false;
    let mut stack: Vec<Element> = Vec::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("not well-formed XML: {e}"))?;
        match event {
            Event::Start(element) => {
                if stack.len() >= MAX_DEPTH {
                    return Err("the XML is nested too deeply".to_owned());
                }
                stack.push(start(&element));
            }
            Event::Empty(element) => {
                let element = start(&element);
                match stack.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => return Ok(element),
                }
            }
            Event::End(_) => {
                let Some(done) = stack.pop() else {
                    return Err("not well-formed XML: an end tag without a start".to_owned());
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => return Ok(done),
                }
            }
            Event::Text(t) => {
                if let Some(open) = stack.last_mut() {
                    open.text.push_str(&t.xml10_content());
                }
            }
            Event::CData(t) => {
                if let Some(open) = stack.last_mut() {
                    open.text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                let resolved = match reference.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    _ => resolve_predefined_entity(&reference.xml10_content()).map(str::to_owned),
                };
                if let (Some(text), Some(open)) = (resolved, stack.last_mut()) {
                    open.text.push_str(&text);
                }
            }
            Event::Eof => return Err("the XML ends before its root element does".to_owned()),
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_with_entities() {
        let root = parse(
            br#"<?xml version="1.0"?><a x="1 &amp; 2"><b>R&amp;B &#233;</b><c/><b>2</b></a>"#,
        )
        .unwrap();
        assert_eq!(root.name, "a");
        assert_eq!(root.attribute("x"), Some("1 & 2"));
        assert_eq!(root.child_text("b"), Some("R&B é"));
        assert_eq!(root.children_named("b").count(), 2);
        assert!(root.child("c").is_some());
        assert!(parse(b"<a>").is_err());
    }
}
