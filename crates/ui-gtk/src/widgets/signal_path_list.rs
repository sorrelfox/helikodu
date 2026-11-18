// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SignalPathList: the signal path as a vertical list of ten fixed stages,
//! from the file to the device, on a rail of nodes. Each row says what its
//! stage does with the samples now, in the engine's words; a stage that does
//! nothing says so, and its node stays hollow. On a bit-perfect path the rail
//! and the hollow nodes are drawn in the bit-perfect line colour, one
//! unbroken line from the file to the device.
//!
//! The rows come from the one `SignalPath` the engine reported; nothing is
//! computed here beyond choosing words. It is a list for assistive
//! technology, each row read as "stage: value"; activating a row opens the
//! setting of its stage.

use std::cell::Cell;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use crate::backend::{
    SignalLamp, SignalPath, SignalStage, SignalState, StageKind, StageState, output_bits,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::style::tokens::{glass, signal};
use crate::widgets::label;

/// What a row's node shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Node {
    /// An end of the path, Source or Device: filled in the text colour.
    #[default]
    End,
    /// A stage that changes the samples: filled in the processed lamp colour.
    Active,
    /// A stage that does nothing, or one not reported: a ring.
    Hollow,
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathRow {
    /// The stage's name.
    pub(crate) name: String,
    /// What it does, or why it does nothing; empty when not reported.
    pub(crate) value: String,
    /// The second line of Source and Device.
    pub(crate) detail: Option<String>,
    /// The node.
    pub(crate) node: Node,
    /// The actions that open its setting, the first one that exists.
    pub(crate) settings: &'static [&'static str],
}

/// A rate as `96 kHz`.
fn khz(hz: u32) -> String {
    fill(&tr("{} kHz"), &[&format::khz(hz)])
}

/// Bits as `24-bit`, or `32-bit float`.
fn bits(bits: u32, float: bool) -> String {
    if float {
        fill(&tr("{}-bit float"), &[&bits.to_string()])
    } else {
        fill(&tr("{}-bit"), &[&bits.to_string()])
    }
}

/// The words for a stage that does not change the samples.
fn idle_words(stage: &SignalStage) -> String {
    match stage.state {
        StageState::Off => tr("Off"),
        StageState::Bypassed => tr("Bypassed"),
        StageState::NotNeeded => tr("Not needed"),
        // The engine names what does it instead: the system mixer, a fixed
        // volume.
        StageState::External if !stage.detail.is_empty() => stage.detail.clone(),
        StageState::External => tr("Outside the player"),
        StageState::Active => stage.detail.clone(),
    }
}

/// The value and node of the stages of `kinds` taken together: active when
/// one of them is, with the words of each that is; otherwise the words of
/// the first one reported.
fn stages(path: &SignalPath, kinds: &[StageKind]) -> (String, Node) {
    let reported: Vec<&SignalStage> = kinds.iter().filter_map(|k| path.stage(*k)).collect();
    let active: Vec<&str> = reported
        .iter()
        .filter(|s| s.state == StageState::Active)
        .map(|s| s.detail.as_str())
        .collect();
    if !active.is_empty() {
        return (active.join(" · "), Node::Active);
    }
    match reported.first() {
        Some(stage) => (idle_words(stage), Node::Hollow),
        None => (String::new(), Node::Hollow),
    }
}

/// The source format as `FLAC · 96 kHz · 24-bit`.
pub(crate) fn source_text(path: &SignalPath) -> String {
    let source = &path.source;
    let mut parts = Vec::new();
    if let Some(codec) = &source.codec {
        parts.push(codec.to_uppercase());
    }
    parts.push(khz(source.spec.rate.hz()));
    parts.push(bits(source.bits, source.spec.format.is_float()));
    parts.join(" · ")
}

/// The device's backend name.
fn backend_name(path: &SignalPath) -> String {
    crate::popovers::output_switcher::backend_name(path.device.backend)
}

/// The format the device was opened with: `96 kHz / 24-bit`.
pub(crate) fn device_format(path: &SignalPath) -> String {
    let format = path.device.format;
    let mut text = fill(
        &tr("{} / {}"),
        &[
            &khz(format.rate.hz()),
            &bits(output_bits(path), format.format.is_float()),
        ],
    );
    if let Some(rate) = path.device.device_rate {
        text = fill(&tr("{}, device at {}"), &[&text, &khz(rate.hz())]);
    }
    text
}

/// The device's name as the path gives it, or else `fallback` (the output
/// button's name for it).
pub(crate) fn device_name(path: &SignalPath, fallback: &str) -> String {
    match &path.device.description {
        Some(description) => description.clone(),
        None if !fallback.is_empty() => fallback.to_owned(),
        None => path.device.device.to_string(),
    }
}

/// The ten rows for `path`; with nothing heard, only the Device row, named
/// `device`, has a value.
pub(crate) fn rows(path: Option<&SignalPath>, device: &str) -> Vec<PathRow> {
    const DSP: &[&str] = &["app.preferences-dsp", "app.preferences"];
    const OUTPUT: &[&str] = &["app.preferences-output", "app.preferences"];
    const LOUDNESS: &[&str] = &["app.preferences-loudness", "app.preferences"];
    let row = |name: String, (value, node): (String, Node), settings| PathRow {
        name,
        value,
        detail: None,
        node,
        settings,
    };
    let Some(path) = path else {
        let mut rows: Vec<PathRow> = NAMES
            .iter()
            .map(|(name, settings)| row(tr(name), (String::new(), Node::Hollow), *settings))
            .collect();
        rows[0].node = Node::End;
        let last = rows.len() - 1;
        rows[last].node = Node::End;
        rows[last].value = device.to_owned();
        return rows;
    };
    let mut source_detail = path.source.spec.layout.to_string();
    if path
        .stage(StageKind::Decoder)
        .is_some_and(|s| s.state == StageState::Active)
    {
        source_detail = fill(&tr("{} · lossy codec"), &[&source_detail]);
    }
    // The format stage: dither and the conversion to the device format; when
    // neither changes anything, the bits that pass untouched.
    let (mut output, output_node) = stages(path, &[StageKind::Dither, StageKind::Conversion]);
    if output_node == Node::Hollow {
        let format = path.device.format.format;
        output = fill(
            &tr("{} · untouched"),
            &[&bits(output_bits(path), format.is_float())],
        );
    }
    let device_detail = [
        backend_name(path),
        match path.device.mode {
            crate::backend::services::OutputMode::Shared => tr("shared"),
            crate::backend::services::OutputMode::Exclusive => tr("exclusive"),
        },
        device_format(path),
    ]
    .join(" · ");
    vec![
        PathRow {
            detail: Some(source_detail),
            ..row(tr("Source"), (source_text(path), Node::End), &[])
        },
        row(
            tr("Loudness"),
            stages(path, &[StageKind::Loudness]),
            LOUDNESS,
        ),
        row(
            tr("Resampler"),
            stages(path, &[StageKind::Resampler]),
            OUTPUT,
        ),
        row(tr("Equalizer"), stages(path, &[StageKind::Equalizer]), DSP),
        row(tr("Crossfeed"), stages(path, &[StageKind::Crossfeed]), DSP),
        row(
            tr("Channel mix"),
            stages(path, &[StageKind::ChannelMixer, StageKind::ChannelMix]),
            DSP,
        ),
        row(tr("Limiter"), stages(path, &[StageKind::Limiter]), DSP),
        row(
            tr("Volume"),
            stages(path, &[StageKind::Volume, StageKind::Crossfade]),
            OUTPUT,
        ),
        row(tr("Output format"), (output, output_node), OUTPUT),
        PathRow {
            detail: Some(device_detail),
            ..row(tr("Device"), (device_name(path, device), Node::End), OUTPUT)
        },
    ]
}

/// The stage names, untranslated, and the settings each row opens.
const NAMES: [(&str, &[&str]); 10] = [
    ("Source", &[]),
    ("Loudness", &["app.preferences-loudness", "app.preferences"]),
    ("Resampler", &["app.preferences-output", "app.preferences"]),
    ("Equalizer", &["app.preferences-dsp", "app.preferences"]),
    ("Crossfeed", &["app.preferences-dsp", "app.preferences"]),
    ("Channel mix", &["app.preferences-dsp", "app.preferences"]),
    ("Limiter", &["app.preferences-dsp", "app.preferences"]),
    ("Volume", &["app.preferences-output", "app.preferences"]),
    (
        "Output format",
        &["app.preferences-output", "app.preferences"],
    ),
    ("Device", &["app.preferences-output", "app.preferences"]),
];

/// The state label of the lamp, as the deck and the tab show it (upper case
/// comes from the stylesheet): `Bit-perfect`, `Bit-perfect, 24-bit` where
/// PipeWire carries only 24 bits exactly, `DSP active`, `Converted`.
pub(crate) fn state_label(state: SignalState, path: Option<&SignalPath>) -> Option<String> {
    match state {
        SignalState::Idle => Some(tr("Idle")),
        SignalState::BitPerfect => Some(match path.and_then(|p| p.device.exact_bits) {
            Some(bits) => fill(&tr("Bit-perfect, {}-bit"), &[&bits.to_string()]),
            None => tr("Bit-perfect"),
        }),
        SignalState::Processed { dsp: true } => Some(tr("DSP active")),
        SignalState::Processed { dsp: false } => Some(tr("Converted")),
        SignalState::NotConnected => Some(tr("Not connected")),
        SignalState::Unreported => None,
    }
}

/// One sentence on what happens to the samples.
pub(crate) fn sentence(path: Option<&SignalPath>) -> String {
    let Some(path) = path else {
        return tr("Nothing is playing.");
    };
    if path.lamp == SignalLamp::BitPerfect {
        return tr("The samples reach the device unchanged.");
    }
    let active = path.active_stages().count();
    let mut text = if active > 0 {
        let count = fill(
            &ntr("{} stage is active.", "{} stages are active.", active),
            &[&active.to_string()],
        );
        // The engine's processing stages work on 32-bit float samples.
        fill(&tr("{} Processing runs in 32-bit float."), &[&count])
    } else {
        tr("The sound server may change the samples.")
    };
    let engine_resamples = path
        .stage(StageKind::Resampler)
        .is_some_and(|s| s.state == StageState::Active);
    match path.lamp {
        SignalLamp::Resampled if !engine_resamples => {
            text = fill(&tr("{} The sound server converts the rate."), &[&text]);
        }
        SignalLamp::Lossy => {
            text = fill(
                &tr("{} The device gets fewer bits than the source has."),
                &[&text],
            );
        }
        _ => {}
    }
    text
}

/// The diagnostics line of a path, for the clipboard.
pub(crate) fn diagnostics(path: Option<&SignalPath>) -> String {
    path.map_or_else(|| tr("Nothing is playing."), ToString::to_string)
}

/// The node of a row and its part of the rail.
mod imp_node {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct PathNode {
        pub(super) node: Cell<Node>,
        pub(super) bit_perfect: Cell<bool>,
        /// Whether the rail goes up from the node, and down from it.
        pub(super) up: Cell<bool>,
        pub(super) down: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PathNode {
        const NAME: &'static str = "HkPathNode";
        type Type = super::PathNode;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for PathNode {}

    impl WidgetImpl for PathNode {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (NODE as i32, NODE as i32, -1, -1),
                _ => (ROW as i32, ROW as i32, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (width, height) = (obj.width() as f32, obj.height() as f32);
            let rail = if self.bit_perfect.get() {
                signal::bit_perfect::LINE
            } else {
                signal::processed::RAIL
            }
            .to_gdk();
            let cx = width / 2.0;
            let cy = ROW / 2.0;
            let x = (cx - RAIL / 2.0).round();
            if self.up.get() {
                snapshot.append_color(&rail, &graphene::Rect::new(x, 0.0, RAIL, cy));
            }
            if self.down.get() {
                snapshot.append_color(&rail, &graphene::Rect::new(x, cy, RAIL, height - cy));
            }
            let disc = gsk::RoundedRect::from_rect(
                graphene::Rect::new(cx - NODE / 2.0, cy - NODE / 2.0, NODE, NODE),
                NODE / 2.0,
            );
            let fill = match self.node.get() {
                Node::End => glass::T1,
                Node::Active => signal::processed::LAMP,
                // The card under the ring.
                Node::Hollow => glass::BG,
            }
            .to_gdk();
            snapshot.push_rounded_clip(&disc);
            snapshot.append_color(&fill, disc.bounds());
            snapshot.pop();
            if self.node.get() == Node::Hollow {
                snapshot.append_border(&disc, &[BORDER; 4], &[rail, rail, rail, rail]);
            }
        }
    }
}

const NODE: f32 = 9.0;
const BORDER: f32 = 1.5;
const RAIL: f32 = 1.0;
/// The height of a row's first line; the node sits at its middle.
const ROW: f32 = 30.0;

glib::wrapper! {
    /// A node on the rail of the signal path list.
    pub(crate) struct PathNode(ObjectSubclass<imp_node::PathNode>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PathNode {
    fn new(up: bool, down: bool) -> Self {
        let node: Self = glib::Object::new();
        node.imp().up.set(up);
        node.imp().down.set(down);
        node.set_valign(gtk::Align::Fill);
        node
    }

    fn show(&self, node: Node, bit_perfect: bool) {
        let imp = self.imp();
        if imp.node.replace(node) != node || imp.bit_perfect.replace(bit_perfect) != bit_perfect {
            self.queue_draw();
        }
    }

    /// What the node shows.
    pub(crate) fn node(&self) -> Node {
        self.imp().node.get()
    }
}

/// Lets `label` wrap at the right, between words where it can.
fn wrap(label: &gtk::Label) {
    label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    label.set_single_line_mode(false);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_justify(gtk::Justification::Right);
    label.set_max_width_chars(1);
}

/// The widgets of one row.
#[derive(Debug)]
struct RowParts {
    row: gtk::ListBoxRow,
    node: PathNode,
    line: gtk::Box,
    name: gtk::Label,
    value: gtk::Label,
    detail: gtk::Label,
}

/// The list: ten rows that stay in place and change their text.
#[derive(Debug)]
pub(crate) struct SignalPathList {
    list: gtk::ListBox,
    rows: Vec<RowParts>,
    settings: std::cell::RefCell<Vec<&'static [&'static str]>>,
}

impl SignalPathList {
    /// The list, empty until [`show`](Self::show).
    pub(crate) fn new() -> std::rc::Rc<Self> {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("hk-path-list");
        list.update_property(&[gtk::accessible::Property::Label(&tr("Signal path"))]);
        let count = NAMES.len();
        let rows = (0..count)
            .map(|i| {
                let row = gtk::ListBoxRow::new();
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                let node = PathNode::new(i > 0, i + 1 < count);
                line.append(&node);
                let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
                text.set_hexpand(true);
                let first = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                first.add_css_class("hk-path-line");
                let name = label("", &["hk-micro-label", "hk-path-name"]);
                name.set_ellipsize(gtk::pango::EllipsizeMode::None);
                name.set_valign(gtk::Align::Center);
                first.append(&name);
                let value = label("", &["hk-path-value"]);
                value.set_hexpand(true);
                value.set_xalign(1.0);
                value.set_valign(gtk::Align::Center);
                // The engine's words can be long: they wrap rather than lose
                // their end.
                wrap(&value);
                first.append(&value);
                text.append(&first);
                let detail = label("", &["hk-path-detail"]);
                detail.set_xalign(1.0);
                detail.set_visible(false);
                wrap(&detail);
                text.append(&detail);
                line.append(&text);
                row.set_child(Some(&line));
                list.append(&row);
                RowParts {
                    row,
                    node,
                    line: first,
                    name,
                    value,
                    detail,
                }
            })
            .collect();
        let this = std::rc::Rc::new(Self {
            list,
            rows,
            settings: std::cell::RefCell::new(Vec::new()),
        });
        let weak = std::rc::Rc::downgrade(&this);
        this.list.connect_row_activated(move |_, row| {
            let Some(this) = weak.upgrade() else { return };
            let index = usize::try_from(row.index()).unwrap_or(usize::MAX);
            let settings = this.settings.borrow().get(index).copied().unwrap_or(&[]);
            for action in settings {
                if WidgetExt::activate_action(row, action, None).is_ok() {
                    break;
                }
            }
        });
        this
    }

    /// The list widget.
    pub(crate) fn widget(&self) -> &gtk::ListBox {
        &self.list
    }

    /// Shows `path`; with none, only the device, named `device`.
    pub(crate) fn show(&self, path: Option<&SignalPath>, device: &str) {
        let rows = rows(path, device);
        let bit_perfect = path.is_some_and(|p| p.lamp == SignalLamp::BitPerfect);
        for (parts, row) in self.rows.iter().zip(&rows) {
            parts.name.set_text(&row.name);
            parts.value.set_text(&row.value);
            parts
                .value
                .set_tooltip_text((!row.value.is_empty()).then_some(&row.value));
            for class in ["active", "end"] {
                parts.value.remove_css_class(class);
            }
            match row.node {
                Node::Active => parts.value.add_css_class("active"),
                Node::End => parts.value.add_css_class("end"),
                Node::Hollow => {}
            }
            parts.detail.set_text(row.detail.as_deref().unwrap_or(""));
            parts.detail.set_visible(row.detail.is_some());
            if row.detail.is_some() {
                parts.line.add_css_class("has-detail");
            } else {
                parts.line.remove_css_class("has-detail");
            }
            parts.node.show(row.node, bit_perfect);
            parts.row.set_activatable(!row.settings.is_empty());
            let value = if row.value.is_empty() {
                tr("not reported")
            } else {
                row.value.clone()
            };
            let mut text = fill(&tr("{}: {}"), &[&row.name, &value]);
            if let Some(detail) = &row.detail {
                text = fill(&tr("{}, {}"), &[&text, detail]);
            }
            parts
                .row
                .update_property(&[gtk::accessible::Property::Label(&text)]);
        }
        self.settings
            .replace(rows.iter().map(|r| r.settings).collect());
    }

    /// The text of each row, as assistive technology reads it, and its node.
    pub(crate) fn texts(&self) -> Vec<(String, String, Node)> {
        self.rows
            .iter()
            .map(|r| {
                (
                    r.name.text().to_string(),
                    r.value.text().to_string(),
                    r.node.node(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::sample::signal_path;

    #[test]
    fn ten_rows_in_a_fixed_order() {
        for lamp in [
            SignalLamp::BitPerfect,
            SignalLamp::LosslessProcessed,
            SignalLamp::Resampled,
            SignalLamp::Lossy,
        ] {
            let path = signal_path(lamp);
            let rows = rows(Some(&path), "x");
            assert_eq!(rows.len(), 10);
            assert_eq!(rows[0].node, Node::End);
            assert_eq!(rows[9].node, Node::End);
        }
        let idle = rows(None, "Studio Interface");
        assert_eq!(idle.len(), 10);
        assert!(idle[..9].iter().all(|r| r.value.is_empty()));
        assert_eq!(idle[9].value, "Studio Interface");
    }

    #[test]
    fn values_come_from_the_path() {
        let path = signal_path(SignalLamp::BitPerfect);
        let rows = rows(Some(&path), "");
        assert_eq!(rows[0].value, "FLAC · 96 kHz · 24-bit");
        assert_eq!(rows[0].detail.as_deref(), Some("stereo"));
        assert!(rows[1..9].iter().all(|r| r.node == Node::Hollow));
        assert_eq!(rows[7].value, "fixed 100 %");
        assert_eq!(rows[8].value, "24-bit · untouched");
        assert_eq!(rows[9].value, "Studio Interface");
        assert_eq!(
            rows[9].detail.as_deref(),
            Some("ALSA · exclusive · 96 kHz / 24-bit")
        );

        let path = signal_path(SignalLamp::Resampled);
        let rows = super::rows(Some(&path), "");
        let active: Vec<&str> = rows
            .iter()
            .filter(|r| r.node == Node::Active)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(
            active,
            [
                "Loudness",
                "Resampler",
                "Equalizer",
                "Limiter",
                "Volume",
                "Output format"
            ]
        );
        assert_eq!(rows[2].value, "resample 96 → 48 kHz (high)");
        assert_eq!(
            rows[9].detail.as_deref(),
            Some("PipeWire · shared · 48 kHz / 32-bit float")
        );
        assert_eq!(
            sentence(Some(&path)),
            "6 stages are active. Processing runs in 32-bit float."
        );
    }

    #[test]
    fn labels_of_each_state() {
        let resampled = signal_path(SignalLamp::Resampled);
        let lossy = signal_path(SignalLamp::Lossy);
        assert_eq!(
            state_label(SignalState::of(&resampled), Some(&resampled)).as_deref(),
            Some("DSP active")
        );
        assert_eq!(
            state_label(SignalState::of(&lossy), Some(&lossy)).as_deref(),
            Some("Converted")
        );
        assert!(sentence(Some(&lossy)).ends_with("fewer bits than the source has."));
        let mut exclusive = signal_path(SignalLamp::BitPerfect);
        exclusive.device.exact_bits = Some(24);
        assert_eq!(
            state_label(SignalState::BitPerfect, Some(&exclusive)).as_deref(),
            Some("Bit-perfect, 24-bit")
        );
        assert_eq!(sentence(None), "Nothing is playing.");
    }
}
