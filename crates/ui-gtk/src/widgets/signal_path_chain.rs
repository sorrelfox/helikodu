// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SignalPathChain: the signal path as boxes joined by lines, left to right:
//! Source, ReplayGain, DSP, Volume, Output. Each box holds a micro label over
//! a mono value. A stage that does something, and the two ends, have the
//! active border; a stage that does nothing says so in words in the weaker
//! colour. The lines take the bit-perfect colour on a bit-perfect path.
//!
//! It is the horizontal form of `SignalPathList` and shows only what the
//! engine reported. For assistive technology it is a list whose items read
//! "stage: value".

use gtk::prelude::*;

use crate::backend::{SignalLamp, SignalPath, StageKind};
use crate::i18n::{fill, tr};
use crate::widgets::label;
use crate::widgets::signal_path_list::{Node, device_format, device_name, stages};

/// One box of the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChainStage {
    /// The micro label.
    pub(crate) name: String,
    /// The value.
    pub(crate) value: String,
    /// Whether it has the active border and the strong value.
    pub(crate) active: bool,
}

/// The five boxes for `path`; with nothing heard, only the output names the
/// device `device`.
pub(crate) fn chain(path: Option<&SignalPath>, device: &str) -> Vec<ChainStage> {
    let stage = |name: String, (value, node): (String, Node)| ChainStage {
        name,
        value,
        active: node != Node::Hollow,
    };
    let Some(path) = path else {
        return vec![
            stage(tr("Source"), (String::new(), Node::Hollow)),
            stage(tr("ReplayGain"), (String::new(), Node::Hollow)),
            stage(tr("DSP"), (String::new(), Node::Hollow)),
            stage(tr("Volume"), (String::new(), Node::Hollow)),
            stage(tr("Output"), (device.to_owned(), Node::Hollow)),
        ];
    };
    let source = &path.source;
    let mut format = fill(
        &tr("{} / {}"),
        &[
            &fill(&tr("{} kHz"), &[&crate::format::khz(source.spec.rate.hz())]),
            &if source.spec.format.is_float() {
                fill(&tr("{}-bit float"), &[&source.bits.to_string()])
            } else {
                fill(&tr("{}-bit"), &[&source.bits.to_string()])
            },
        ],
    );
    if let Some(codec) = &source.codec {
        format = format!("{} {format}", codec.to_uppercase());
    }
    let output_name = fill(
        &tr("Output · {} {}"),
        &[
            &crate::popovers::output_switcher::backend_name(path.device.backend),
            &match path.device.mode {
                crate::backend::services::OutputMode::Shared => tr("shared"),
                crate::backend::services::OutputMode::Exclusive => tr("exclusive"),
            },
        ],
    );
    vec![
        stage(tr("Source"), (format, Node::End)),
        stage(tr("ReplayGain"), stages(path, &[StageKind::Loudness])),
        stage(
            tr("DSP"),
            stages(
                path,
                &[
                    StageKind::Equalizer,
                    StageKind::Crossfeed,
                    StageKind::ChannelMixer,
                    StageKind::Limiter,
                ],
            ),
        ),
        stage(
            tr("Volume"),
            stages(path, &[StageKind::Volume, StageKind::Crossfade]),
        ),
        stage(
            output_name,
            (
                fill(
                    &tr("{} {}"),
                    &[&device_name(path, device), &device_format(path)],
                ),
                Node::End,
            ),
        ),
    ]
}

/// The chain's widgets.
#[derive(Debug)]
pub(crate) struct SignalPathChain {
    root: gtk::Box,
    boxes: Vec<(gtk::Box, gtk::Label, gtk::Label)>,
    lines: Vec<gtk::Box>,
}

impl SignalPathChain {
    /// An empty chain.
    pub(crate) fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("hk-chain");
        root.set_accessible_role(gtk::AccessibleRole::List);
        root.update_property(&[gtk::accessible::Property::Label(&tr("Signal path"))]);
        let mut boxes = Vec::new();
        let mut lines = Vec::new();
        for i in 0..5 {
            if i > 0 {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                line.add_css_class("hk-chain-line");
                line.set_hexpand(true);
                line.set_valign(gtk::Align::Center);
                line.set_accessible_role(gtk::AccessibleRole::Presentation);
                root.append(&line);
                lines.push(line);
            }
            let stage = gtk::Box::new(gtk::Orientation::Vertical, 2);
            stage.add_css_class("hk-chain-stage");
            stage.set_valign(gtk::Align::Center);
            stage.set_accessible_role(gtk::AccessibleRole::ListItem);
            let name = label("", &["hk-micro-label", "hk-chain-name"]);
            name.set_ellipsize(gtk::pango::EllipsizeMode::None);
            let value = label("", &["hk-mono", "hk-chain-value"]);
            value.set_max_width_chars(44);
            stage.append(&name);
            stage.append(&value);
            root.append(&stage);
            boxes.push((stage, name, value));
        }
        Self { root, boxes, lines }
    }

    /// The chain.
    pub(crate) fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows `path`; `device` names the output when nothing is heard.
    pub(crate) fn show(&self, path: Option<&SignalPath>, device: &str) {
        let bit_perfect = path.is_some_and(|p| p.lamp == SignalLamp::BitPerfect);
        for line in &self.lines {
            if bit_perfect {
                line.add_css_class("bit-perfect");
            } else {
                line.remove_css_class("bit-perfect");
            }
        }
        for ((stage, name, value), shown) in self.boxes.iter().zip(chain(path, device)) {
            if name.text() != shown.name {
                name.set_text(&shown.name);
            }
            if value.text() != shown.value {
                value.set_text(&shown.value);
                value.set_tooltip_text(Some(shown.value.as_str()).filter(|v| !v.is_empty()));
            }
            if shown.active {
                stage.add_css_class("active");
            } else {
                stage.remove_css_class("active");
            }
            stage.update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("{}: {}"),
                &[&shown.name, &shown.value],
            ))]);
        }
    }

    /// Each box as "name: value [active]", for tests.
    pub(crate) fn texts(&self) -> Vec<String> {
        self.boxes
            .iter()
            .map(|(stage, name, value)| {
                format!(
                    "{}: {}{}",
                    name.text(),
                    value.text(),
                    if stage.has_css_class("active") {
                        " [active]"
                    } else {
                        ""
                    }
                )
            })
            .collect()
    }
}
