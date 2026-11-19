// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tray menu as `com.canonical.dbusmenu`: one level under the root, whose
//! id is 0. Every change of the menu is announced with `LayoutUpdated` and a
//! new revision, after which hosts fetch the whole layout again.
//!
//! Each revision numbers its items afresh, in a range of its own: hosts keep
//! the items they know by id and update only the properties they are sent
//! again (libdbusmenu-qt in KDE Plasma does), so an id that meant another item
//! before would keep that item's separator type or check box. An event for an
//! id of an older revision is reported as an unknown item.

use std::collections::HashMap;
use std::sync::Arc;

use zbus::object_server::SignalEmitter;
use zbus::{OwnedVariant, Variant, fdo, interface};

use super::{Shared, TrayMenuItem, no_item};

/// The number of a menu item: its revision's range plus its position in the
/// menu, counted from 1; 0 is the root.
pub type MenuItemId = i32;

/// The ids of one revision: `SPAN` of them, of which the first is not used.
const SPAN: i32 = 1000;
/// How many revisions have ranges of their own before the ranges are used
/// again.
const RANGES: u32 = (i32::MAX / SPAN - 1) as u32;

/// The properties of one item, as the protocol names them.
type Properties = HashMap<String, OwnedVariant>;
/// One node of the layout: id, properties, children (each a variant holding a
/// node).
type Node = (i32, Properties, Vec<OwnedVariant>);

fn variant(value: Variant<'_>) -> OwnedVariant {
    OwnedVariant::try_from(value).expect("plain values hold no file descriptors")
}

/// A label with its underscores kept: dbusmenu reads one underscore as the
/// mark of an access key, two as an underscore.
fn label(text: &str) -> OwnedVariant {
    variant(Variant::from(text.replace('_', "__")))
}

/// The properties of `item`, limited to `names` when that is not empty.
fn properties(item: Option<&TrayMenuItem>, names: &[String]) -> Properties {
    let mut all = Properties::new();
    match item {
        None => {
            all.insert("children-display".into(), variant(Variant::from("submenu")));
        }
        Some(TrayMenuItem::Separator) => {
            all.insert("type".into(), variant(Variant::from("separator")));
        }
        Some(TrayMenuItem::Text(text)) => {
            all.insert("label".into(), label(text));
            all.insert("enabled".into(), variant(Variant::from(false)));
        }
        Some(TrayMenuItem::Action {
            label: text,
            enabled,
            checked,
            ..
        }) => {
            all.insert("label".into(), label(text));
            if !enabled {
                all.insert("enabled".into(), variant(Variant::from(false)));
            }
            if let Some(checked) = checked {
                all.insert("toggle-type".into(), variant(Variant::from("checkmark")));
                all.insert(
                    "toggle-state".into(),
                    variant(Variant::from(i32::from(*checked))),
                );
            }
        }
    }
    if !names.is_empty() {
        all.retain(|name, _| names.iter().any(|n| n == name));
    }
    all
}

/// The first id of `revision`'s range.
fn base(revision: u32) -> i32 {
    // RANGES is below i32::MAX / SPAN, so the product fits.
    i32::try_from(revision % RANGES).unwrap_or(0) * SPAN
}

/// The position of the item `id` of `revision`, if it is one of its ids.
fn index(revision: u32, id: i32) -> Option<usize> {
    let offset = id.checked_sub(base(revision))?;
    if !(1..SPAN).contains(&offset) {
        return None;
    }
    usize::try_from(offset - 1).ok()
}

/// The id of the item at `index` in `revision`.
fn id_of(revision: u32, index: usize) -> i32 {
    base(revision) + i32::try_from(index + 1).unwrap_or(SPAN - 1).min(SPAN - 1)
}

/// The menu object.
pub(super) struct Menu {
    pub(super) shared: Arc<Shared>,
}

impl Menu {
    fn item(menu: &[TrayMenuItem], revision: u32, id: i32) -> fdo::Result<Option<&TrayMenuItem>> {
        if id == 0 {
            return Ok(None);
        }
        index(revision, id)
            .and_then(|i| menu.get(i))
            .map(Some)
            .ok_or_else(|| no_item(id))
    }

    /// Acts on one event; whether the item exists.
    fn event_on(&self, id: i32, event: &str) -> bool {
        let (content, revision) = self.shared.content();
        let Ok(item) = Self::item(&content.menu, revision, id) else {
            return false;
        };
        if event == "clicked"
            && let Some(TrayMenuItem::Action {
                action,
                enabled: true,
                ..
            }) = item
        {
            (self.shared.hooks.action)(*action);
        }
        true
    }
}

#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> fdo::Result<(u32, Node)> {
        let (content, revision) = self.shared.content();
        let parent = Self::item(&content.menu, revision, parent_id)?;
        let mut children = Vec::new();
        if parent.is_none() && recursion_depth != 0 {
            for (i, item) in content.menu.iter().enumerate() {
                let node: Node = (
                    id_of(revision, i),
                    properties(Some(item), &property_names),
                    Vec::new(),
                );
                children.push(variant(Variant::from(node)));
            }
        }
        Ok((
            revision,
            (parent_id, properties(parent, &property_names), children),
        ))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, Properties)> {
        let (content, revision) = self.shared.content();
        let ids = if ids.is_empty() {
            (0..=content.menu.len())
                .map(|n| id_of_or_root(revision, n))
                .collect()
        } else {
            ids
        };
        ids.into_iter()
            .filter_map(|id| {
                let item = Self::item(&content.menu, revision, id).ok()?;
                Some((id, properties(item, &property_names)))
            })
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> fdo::Result<OwnedVariant> {
        let (content, revision) = self.shared.content();
        let item = Self::item(&content.menu, revision, id)?;
        properties(item, std::slice::from_ref(&name))
            .remove(&name)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no property {name}")))
    }

    fn event(
        &self,
        id: i32,
        event_id: String,
        _data: OwnedVariant,
        _timestamp: u32,
    ) -> fdo::Result<()> {
        if self.event_on(id, &event_id) {
            Ok(())
        } else {
            Err(no_item(id))
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedVariant, u32)>) -> Vec<i32> {
        events
            .into_iter()
            .filter(|(id, event, _, _)| !self.event_on(*id, event))
            .map(|(id, ..)| id)
            .collect()
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn items_properties_updated(
        emitter: &SignalEmitter<'_>,
        updated: Vec<(i32, Properties)>,
        removed: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;
}

/// The root for 0, otherwise the item at `n - 1` of `revision`.
fn id_of_or_root(revision: u32, n: usize) -> i32 {
    if n == 0 { 0 } else { id_of(revision, n - 1) }
}

#[cfg(test)]
mod tests {
    use super::super::TrayAction;
    use super::*;

    fn text(properties: &Properties, name: &str) -> Option<String> {
        properties
            .get(name)
            .and_then(|v| v.downcast_ref::<&str>().ok().map(str::to_owned))
    }

    #[test]
    fn items_map_to_their_properties() {
        let root = properties(None, &[]);
        assert_eq!(text(&root, "children-display").as_deref(), Some("submenu"));
        let separator = properties(Some(&TrayMenuItem::Separator), &[]);
        assert_eq!(text(&separator, "type").as_deref(), Some("separator"));
        let line = properties(Some(&TrayMenuItem::Text("Now_here".into())), &[]);
        assert_eq!(text(&line, "label").as_deref(), Some("Now__here"));
        assert_eq!(
            line.get("enabled")
                .and_then(|v| v.downcast_ref::<bool>().ok()),
            Some(false)
        );
        let check = properties(
            Some(&TrayMenuItem::check(TrayAction::StopAfter, "Stop", true)),
            &[],
        );
        assert_eq!(text(&check, "toggle-type").as_deref(), Some("checkmark"));
        assert_eq!(
            check
                .get("toggle-state")
                .and_then(|v| v.downcast_ref::<i32>().ok()),
            Some(1)
        );
        assert!(!check.contains_key("enabled"));
        let only = properties(
            Some(&TrayMenuItem::check(TrayAction::StopAfter, "Stop", false)),
            &["label".to_owned()],
        );
        assert_eq!(only.len(), 1);
    }

    #[test]
    fn each_revision_numbers_its_items_afresh() {
        assert_eq!(id_of(0, 0), 1);
        assert_eq!(index(0, 1), Some(0));
        assert_eq!(index(0, 0), None);
        assert_eq!(index(0, -3), None);
        assert_eq!(id_of_or_root(0, 0), 0);
        assert_eq!(id_of_or_root(0, 2), 2);
        // The same position in the next revision is another id, and the ids
        // of the old revision are no longer items.
        assert_eq!(id_of(1, 0), 1001);
        assert_eq!(index(1, 1001), Some(0));
        assert_eq!(index(1, 1), None);
        assert_eq!(index(2, 1001), None);
        assert_eq!(index(1, 2000), None);
        // The ranges stay within i32 and start over after RANGES revisions.
        assert!(id_of(RANGES - 1, 998) > 0);
        assert_eq!(id_of(RANGES, 0), 1);
        assert_eq!(id_of(u32::MAX, 0), base(u32::MAX) + 1);
        assert!(base(u32::MAX) >= 0);
    }
}
