// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › DSP and EQ: the chain of stages, the editor of the stage chosen
//! in it, and a readout of the processing pinned to the bottom of the page.
//!
//! The chain is also the selector: activating a stage's name shows its editor
//! below; the switch beside the name only turns the stage on or off, and a
//! stage can be edited while it is off. The stages run in an order the user
//! sets (by dragging, with Alt and the arrow keys, or from a stage's menu);
//! Dither is always last and has no editor. Every change goes to the settings
//! store at once, which the player applies live; while a control is dragged
//! the changes are passed on at most every 80 ms, the last one always.

mod chain;
mod compressor;
mod convolution;
pub(crate) mod equalizer;
mod pitch;
mod stages;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use helikodu_model::settings::{
    ChainStage, ChannelMode, CompressorPreset, DspSettings, EqMode, ImpulseFile,
};

use super::{Context, page_body};
use crate::backend::dsp::ImpulseEntry;
use crate::backend::services::Settings;
use crate::backend::{SignalPath, SignalState, StageKind, StageState, TrackKey};
use crate::dialogs::common::{compact_button, divider, switch_row};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::{SignalLamp, label};

/// A stage of the chain; each has an editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Stage {
    /// Pitch shift.
    Pitch,
    /// The equalizer.
    Equalizer,
    /// Convolution with an impulse response.
    Convolution,
    /// Crossfeed.
    Crossfeed,
    /// The channel mixer.
    ChannelMix,
    /// The compressor.
    Compressor,
    /// The limiter.
    Limiter,
}

impl Stage {
    /// Every stage, in the default order.
    pub(crate) const ALL: [Self; 7] = [
        Self::Pitch,
        Self::Equalizer,
        Self::Convolution,
        Self::Crossfeed,
        Self::ChannelMix,
        Self::Compressor,
        Self::Limiter,
    ];

    /// The name of its editor in the page's stack.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Pitch => "pitch",
            Self::Equalizer => "equalizer",
            Self::Convolution => "convolution",
            Self::Crossfeed => "crossfeed",
            Self::ChannelMix => "channel-mix",
            Self::Compressor => "compressor",
            Self::Limiter => "limiter",
        }
    }

    /// The stage whose editor is called `id`.
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    pub(crate) fn title(self) -> String {
        match self {
            Self::Pitch => tr("Pitch"),
            Self::Equalizer => tr("Equalizer"),
            Self::Convolution => tr("Convolution"),
            Self::Crossfeed => tr("Crossfeed"),
            Self::ChannelMix => tr("Channel mix"),
            Self::Compressor => tr("Compressor"),
            Self::Limiter => tr("Limiter"),
        }
    }

    /// The stage in the settings' terms.
    pub(crate) fn chain(self) -> ChainStage {
        match self {
            Self::Pitch => ChainStage::Pitch,
            Self::Equalizer => ChainStage::Equalizer,
            Self::Convolution => ChainStage::Convolution,
            Self::Crossfeed => ChainStage::Crossfeed,
            Self::ChannelMix => ChainStage::ChannelMix,
            Self::Compressor => ChainStage::Compressor,
            Self::Limiter => ChainStage::Limiter,
        }
    }

    /// The stage of `stage` in the settings' terms.
    pub(crate) fn of(stage: ChainStage) -> Self {
        match stage {
            ChainStage::Pitch => Self::Pitch,
            ChainStage::Equalizer => Self::Equalizer,
            ChainStage::Convolution => Self::Convolution,
            ChainStage::Crossfeed => Self::Crossfeed,
            ChainStage::ChannelMix => Self::ChannelMix,
            ChainStage::Compressor => Self::Compressor,
            ChainStage::Limiter => Self::Limiter,
        }
    }

    /// Whether its switch is on.
    pub(crate) fn enabled(self, dsp: &DspSettings) -> bool {
        dsp.is_enabled(self.chain())
    }

    fn set_enabled(self, settings: &mut Settings, on: bool) {
        settings.dsp.set_enabled(self.chain(), on);
    }

    /// Whether it is on and does something: pitch at 0 and convolution
    /// without a file or with a file that cannot be used count as off.
    fn working(self, dsp: &DspSettings, facts: &Facts) -> bool {
        self.enabled(dsp)
            && match self {
                Self::Pitch => dsp.pitch.semitones != 0.0,
                Self::Convolution => !facts.impulses.is_empty() && !facts.left_out,
                _ => true,
            }
    }

    /// The line under the name in the chain, and whether it tells of a
    /// problem (with a warning icon).
    fn summary(self, dsp: &DspSettings, facts: &Facts) -> (String, bool) {
        if self == Self::Convolution && self.enabled(dsp) && facts.left_out {
            let problems = facts
                .impulses
                .iter()
                .filter(|e| e.problem.is_some())
                .count()
                .max(1);
            return (
                fill(
                    &ntr(
                        "{} file cannot be read",
                        "{} files cannot be read",
                        problems,
                    ),
                    &[&problems.to_string()],
                ),
                true,
            );
        }
        if !self.enabled(dsp) {
            return (tr("Bypassed"), false);
        }
        let text = match self {
            Self::Pitch if dsp.pitch.semitones == 0.0 => tr("Bypassed"),
            Self::Pitch => pitch::semitones(dsp.pitch.semitones),
            Self::Equalizer => match &facts.track_eq {
                Some(track) => fill(&tr("Track: {}"), &[&track.name]),
                None => {
                    let eq = &dsp.equalizer;
                    match eq.mode {
                        EqMode::Parametric => {
                            let n = eq.parametric.iter().filter(|f| f.enabled).count();
                            fill(&ntr("{} filter", "{} filters", n), &[&n.to_string()])
                        }
                        EqMode::Graphic10 => tr("10 bands"),
                        EqMode::Graphic31 => tr("31 bands"),
                    }
                }
            },
            Self::Convolution => convolution::rates_summary(&facts.impulses),
            Self::Crossfeed => {
                let (hz, db) = dsp.crossfeed.effective();
                fill(
                    &tr("{} Hz / {} dB"),
                    &[&hz.to_string(), &crate::format::decimal(db, 1)],
                )
            }
            Self::ChannelMix => {
                let mix = &dsp.channel_mix;
                let channels = match mix.channels {
                    ChannelMode::Stereo => tr("Stereo"),
                    ChannelMode::Mono => tr("Mono"),
                    ChannelMode::Swap => tr("Swapped"),
                };
                fill(
                    &tr("{}, balance {}"),
                    &[&channels, &stages::balance_short(mix.balance_db)],
                )
            }
            Self::Compressor => compressor::summary(&dsp.compressor),
            Self::Limiter => fill(
                &tr("Lookahead {} ms"),
                &[&trim_number(dsp.limiter.lookahead_ms)],
            ),
        };
        (text, false)
    }

    /// The stage in the path of the readout, when it works.
    fn in_path(self, dsp: &DspSettings, facts: &Facts) -> Option<String> {
        if !self.working(dsp, facts) {
            return None;
        }
        Some(match self {
            Self::Pitch => fill(&tr("pitch {}"), &[&pitch::semitones(dsp.pitch.semitones)]),
            Self::Equalizer => match &facts.track_eq {
                Some(track) => track.path.clone(),
                None => fill(&tr("EQ {}"), &[&self.summary(dsp, facts).0]),
            },
            Self::Convolution => tr("convolution"),
            Self::Crossfeed => tr("crossfeed"),
            Self::ChannelMix => tr("channel mixer"),
            Self::Compressor => match dsp.compressor.preset {
                CompressorPreset::Custom => tr("compressor"),
                _ => fill(
                    &tr("compressor {}"),
                    &[&compressor::summary(&dsp.compressor)],
                ),
            },
            Self::Limiter => tr("limiter"),
        })
    }

    /// The delay it adds while it works, as the design gives it when nothing
    /// plays: the lookahead of the limiter and the compressor, about 11 ms
    /// for convolution and about 33 ms for pitch shift.
    fn estimated_delay_ms(self, dsp: &DspSettings) -> f64 {
        match self {
            Self::Pitch => 33.0,
            Self::Convolution => 11.0,
            Self::Compressor => dsp.compressor.lookahead_ms,
            Self::Limiter => dsp.limiter.lookahead_ms,
            _ => 0.0,
        }
    }
}

/// `2`, `2.5`: a number with its decimals only when it has them.
pub(crate) fn trim_number(value: f64) -> String {
    let text = format!("{value:.1}");
    crate::format::localized(text.trim_end_matches('0').trim_end_matches('.').to_owned())
}

/// The equalizer of the track that plays, when it replaces the main one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrackEq {
    /// Its name.
    pub(crate) name: String,
    /// The equalizer step of the readout: `track EQ 4 filters`.
    pub(crate) path: String,
    /// The track.
    pub(crate) track: Option<TrackKey>,
}

/// What the page knows beyond the settings: the impulse responses, and what
/// the engine reports about the track that plays.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    /// The impulse responses the convolution settings name.
    pub(crate) impulses: Vec<ImpulseEntry>,
    /// Convolution is on but left out: its file for the rate in use cannot be
    /// used.
    pub(crate) left_out: bool,
    /// The rate the chain runs at now, while something plays.
    pub(crate) rate: Option<u32>,
    /// The equalizer of the track that plays, when it replaces the main one.
    pub(crate) track_eq: Option<TrackEq>,
    /// The delay the active stages add, as the engine reports it.
    pub(crate) delay_ms: Option<f64>,
}

impl Facts {
    /// Reads what the engine reported in `path`.
    fn follow(&mut self, path: Option<&SignalPath>, live: &Live) {
        self.rate = path.map(|p| p.device.format.rate.hz());
        self.left_out = path
            .and_then(|p| p.stage(StageKind::Convolution))
            .is_some_and(|s| s.state == StageState::Bypassed && s.detail.starts_with("left out"));
        // The delay the engine reports for the stages that work; with none
        // reported at work, the page estimates it from the settings.
        self.delay_ms = path
            .map(|p| {
                p.stages
                    .iter()
                    .filter(|s| s.kind.is_dsp() && s.state == StageState::Active)
                    .map(|s| s.delay_ms)
                    .collect::<Vec<f64>>()
            })
            .filter(|delays| !delays.is_empty())
            .map(|delays| delays.iter().fold(0.0, |sum, d| sum + d));
        let track = path
            .and_then(|p| p.stage(StageKind::Equalizer))
            .filter(|s| s.state == StageState::Active && s.detail.starts_with("track "));
        self.track_eq = track.map(|stage| {
            let context = live.context();
            let playing = context
                .as_ref()
                .and_then(|c| c.playback.clone())
                .and_then(|p| p.view().now.map(|n| n.track.key));
            // The name the library keeps for the playing track's equalizer.
            let name = context
                .as_ref()
                .and_then(|c| {
                    playing
                        .and_then(|key| c.services.track_equalizer(key).ok().flatten())
                        .map(|eq| eq.name)
                        .filter(|name| !name.is_empty())
                        .or_else(|| c.services.dsp().playing_equalizer_name())
                })
                .unwrap_or_else(|| tr("Own settings"));
            // The engine's words up to the preamp: `track EQ 4 filters`.
            let path = stage
                .detail
                .split(" · ")
                .next()
                .unwrap_or_default()
                .to_owned();
            TrackEq {
                name,
                path,
                track: playing,
            }
        });
    }
}

/// A change of the settings, waiting to be applied.
type Change = Box<dyn Fn(&mut Settings)>;

/// Passes changes on to the store at once, then at most every 80 ms while
/// they keep coming, so that a drag neither floods the store nor lags.
pub(crate) struct Live {
    context: Weak<Context>,
    timer: Cell<Option<glib::SourceId>>,
    pending: RefCell<Option<Change>>,
    after: RefCell<Vec<Box<dyn Fn()>>>,
    /// What the page knows beyond the settings.
    facts: RefCell<Facts>,
    /// The convolution files the impulse responses were read for.
    read_for: RefCell<Option<(Option<PathBuf>, Vec<ImpulseFile>)>>,
}

impl Live {
    const PERIOD: Duration = Duration::from_millis(80);

    fn new(context: &Rc<Context>) -> Rc<Self> {
        Rc::new(Self {
            context: Rc::downgrade(context),
            timer: Cell::new(None),
            pending: RefCell::new(None),
            after: RefCell::new(Vec::new()),
            facts: RefCell::new(Facts::default()),
            read_for: RefCell::new(None),
        })
    }

    /// The settings as they are now.
    pub(crate) fn settings(&self) -> Option<std::sync::Arc<Settings>> {
        Some(self.context.upgrade()?.settings())
    }

    /// The page's context.
    pub(crate) fn context(&self) -> Option<Rc<Context>> {
        self.context.upgrade()
    }

    /// What the page knows beyond the settings.
    pub(crate) fn facts(&self) -> Facts {
        self.facts.borrow().clone()
    }

    /// Reads the impulse responses again when the convolution settings name
    /// other files, and what the engine reports about the track that plays.
    fn update_facts(&self) {
        let Some(context) = self.context() else {
            return;
        };
        let settings = context.settings();
        let convolution = &settings.dsp.convolution;
        let key = (convolution.folder.clone(), convolution.files.clone());
        if self.read_for.borrow().as_ref() != Some(&key) {
            let entries = context.services.dsp().impulse_files(convolution);
            self.facts.borrow_mut().impulses = entries;
            self.read_for.replace(Some(key));
        }
        let path = context.playback.as_ref().and_then(|p| p.view().signal);
        let mut facts = self.facts.borrow().clone();
        facts.follow(path.as_deref(), self);
        self.facts.replace(facts);
    }

    /// Reads the impulse responses again even when the same files are named:
    /// a file was replaced.
    pub(crate) fn reread_impulses(&self) {
        self.read_for.replace(None);
        self.update_facts();
    }

    /// Calls `listener` after every change that reached the store.
    fn connect_applied(&self, listener: impl Fn() + 'static) {
        self.after.borrow_mut().push(Box::new(listener));
    }

    /// Tells the listeners, as after a change.
    fn notify(&self) {
        for listener in self.after.borrow().iter() {
            listener();
        }
    }

    fn commit(&self, change: &dyn Fn(&mut Settings)) -> bool {
        let Some(context) = self.context.upgrade() else {
            return false;
        };
        let done = context.update(change);
        self.update_facts();
        self.notify();
        done
    }

    /// Applies `change` now; the result says whether the store took it.
    pub(crate) fn apply(&self, change: impl Fn(&mut Settings)) -> bool {
        self.pending.borrow_mut().take();
        self.commit(&change)
    }

    /// Applies `change` now or, while changes keep coming, with the next tick.
    pub(crate) fn apply_soon(self: &Rc<Self>, change: impl Fn(&mut Settings) + 'static) {
        let timer = self.timer.take();
        let ticking = timer.is_some();
        self.timer.set(timer);
        if ticking {
            self.pending.replace(Some(Box::new(change)));
            return;
        }
        self.commit(&change);
        self.start_timer();
    }

    fn start_timer(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.timer.set(Some(glib::timeout_add_local_once(
            Self::PERIOD,
            move || {
                let Some(live) = weak.upgrade() else {
                    return;
                };
                live.timer.set(None);
                let pending = live.pending.borrow_mut().take();
                if let Some(change) = pending {
                    live.commit(&*change);
                    live.start_timer();
                }
            },
        )));
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        if let Some(id) = self.timer.take() {
            id.remove();
        }
    }
}

/// The readout box pinned to the bottom of the page.
struct Readout {
    root: gtk::Box,
    lamp: SignalLamp,
    state: gtk::Label,
    delay: gtk::Label,
    path: gtk::Label,
}

/// The parts of the page that follow the settings.
struct Page {
    live: Rc<Live>,
    chain: Rc<chain::Chain>,
    stack: gtk::Stack,
    readout: Readout,
    track_notice: gtk::Box,
    track_notice_text: gtk::Label,
    editors: RefCell<Vec<Rc<dyn Fn()>>>,
}

impl Page {
    /// Shows the stored settings in the chain, the readout and the editors.
    fn refresh(&self) {
        let Some(settings) = self.live.settings() else {
            return;
        };
        let dsp = &settings.dsp;
        let facts = self.live.facts();
        self.chain.refresh(dsp, &facts);
        self.refresh_readout(dsp, &facts);
        match &facts.track_eq {
            Some(track) => {
                self.track_notice_text.set_text(&fill(
                    &tr("This track plays with its own equalizer, “{}”"),
                    &[&track.name],
                ));
                self.track_notice.set_visible(true);
            }
            None => self.track_notice.set_visible(false),
        }
        for editor in self.editors.borrow().iter() {
            editor();
        }
    }

    fn refresh_readout(&self, dsp: &DspSettings, facts: &Facts) {
        let readout = &self.readout;
        let order: Vec<Stage> = dsp.effective_order().into_iter().map(Stage::of).collect();
        let stages: Vec<String> = order
            .iter()
            .filter_map(|stage| stage.in_path(dsp, facts))
            .collect();
        let device = self
            .live
            .context()
            .and_then(|c| c.playback.as_ref().map(|p| p.view().output.device))
            .filter(|d| !d.is_empty());
        if stages.is_empty() {
            readout.lamp.set_state(SignalState::BitPerfect);
            readout.state.set_text(&tr("BIT-PERFECT"));
            readout.state.remove_css_class("processed");
            readout.state.add_css_class("bit-perfect");
            readout.delay.set_visible(false);
            readout.path.set_text(&match device {
                Some(device) => fill(&tr("Every stage is off → {}"), &[&device]),
                None => tr("Every stage is off. The samples pass unchanged."),
            });
            return;
        }
        readout.lamp.set_state(SignalState::Processed { dsp: true });
        readout.state.set_text(&tr("DSP ACTIVE · 32-BIT FLOAT"));
        readout.state.remove_css_class("bit-perfect");
        readout.state.add_css_class("processed");
        // The delay the engine reports; with nothing playing, what the
        // stages would add.
        let delay = facts.delay_ms.unwrap_or_else(|| {
            order
                .iter()
                .filter(|s| s.working(dsp, facts))
                .map(|s| s.estimated_delay_ms(dsp))
                .fold(0.0, |sum, d| sum + d)
        });
        let delay = delay.round();
        readout.delay.set_visible(delay >= 1.0);
        readout
            .delay
            .set_text(&fill(&tr("Adds {} ms"), &[&format!("{delay:.0}")]));
        let mut parts = stages;
        parts.push(tr("TPDF dither"));
        if let Some(device) = device {
            parts.push(device);
        }
        // Wrap only between stages: the arrow starts the new line.
        let parts: Vec<String> = parts.iter().map(|p| p.replace(' ', "\u{00A0}")).collect();
        readout.path.set_text(&parts.join(" →\u{00A0}"));
    }

    fn select(&self, stage: Stage) {
        self.stack.set_visible_child_name(stage.id());
        self.chain.select(stage);
    }
}

/// One or two sentences that say what a stage is for.
pub(crate) fn lead(text: &str) -> gtk::Label {
    let lead = gtk::Label::new(Some(text));
    lead.set_xalign(0.0);
    lead.set_wrap(true);
    lead.set_max_width_chars(110);
    lead.set_halign(gtk::Align::Start);
    lead.add_css_class("hk-t2");
    lead
}

/// A note row: an info icon and a sentence, for facts the user cannot change.
pub(crate) fn info_note(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let icon = gtk::Image::from_icon_name("helikodu-info-symbolic");
    icon.add_css_class("hk-t2");
    icon.set_valign(gtk::Align::Start);
    row.append(&icon);
    let sentence = gtk::Label::new(Some(text));
    sentence.set_xalign(0.0);
    sentence.set_wrap(true);
    sentence.add_css_class("hk-t2");
    row.append(&sentence);
    row
}

/// A notice box: an icon, a message in 500 with a 12 px line under it, and
/// a compact button at the right.
pub(crate) fn notice(icon: &str, button: &str) -> (gtk::Box, gtk::Label, gtk::Label, gtk::Button) {
    let notice = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    notice.add_css_class("hk-notice");
    notice.add_css_class("hk-dsp-notice");
    let image = gtk::Image::from_icon_name(icon);
    image.set_valign(gtk::Align::Center);
    notice.append(&image);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let message = label("", &["message"]);
    message.set_wrap(true);
    message.set_xalign(0.0);
    text.append(&message);
    let detail = gtk::Label::new(None);
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.add_css_class("detail");
    text.append(&detail);
    notice.append(&text);
    let button = compact_button(button);
    notice.append(&button);
    (notice, message, detail, button)
}

fn readout() -> Readout {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    root.add_css_class("hk-readout");
    root.add_css_class("hk-dsp-readout");
    root.set_widget_name("dsp-readout");
    let lamp = SignalLamp::default();
    lamp.set_valign(gtk::Align::Center);
    root.append(&lamp);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let state = label("", &["hk-signal"]);
    state.set_hexpand(true);
    head.append(&state);
    let delay = label("", &["hk-mono", "hk-dsp-delay"]);
    delay.set_widget_name("dsp-delay");
    delay.set_halign(gtk::Align::End);
    head.append(&delay);
    text.append(&head);
    let path = gtk::Label::new(None);
    path.set_xalign(0.0);
    path.set_wrap(true);
    path.add_css_class("hk-mono");
    path.add_css_class("hk-dsp-path");
    path.set_widget_name("dsp-path");
    text.append(&path);
    root.append(&text);
    root.update_property(&[gtk::accessible::Property::Label(&tr(
        "Processing right now",
    ))]);
    Readout {
        root,
        lamp,
        state,
        delay,
        path,
    }
}

// The stage whose editor the page opens with, as asked from outside (the
// Signal path tab) for the next page built.
thread_local! {
    static OPEN_AT: Cell<Option<Stage>> = const { Cell::new(None) };
}

/// Asks the DSP and EQ page to show `stage` when it is built or shown next.
pub(crate) fn show_stage(stage: Stage) {
    OPEN_AT.with(|cell| cell.set(Some(stage)));
}

/// The equalizer's page in the stack: the notice of a track's own
/// equalizer, the preset row, the editor and the switch for the equalizers
/// of single tracks.
fn equalizer_page(live: &Rc<Live>) -> EqualizerParts {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let (notice, message, detail, button) =
        notice("helikodu-info-symbolic", &tr("Track settings…"));
    notice.set_widget_name("track-eq-notice");
    detail.set_text(&tr(
        "The settings below are the main equalizer. They are heard again when another track plays.",
    ));
    button.set_widget_name("track-eq-settings");
    notice.set_visible(false);
    notice.set_margin_bottom(6);
    column.append(&notice);
    let presets = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    presets.set_widget_name("dsp-presets");
    column.append(&presets);
    let (editor, refresh) = equalizer::editor(live, &presets);
    column.append(&editor);

    // The equalizers of single tracks.
    let (row, switch) = switch_row(
        &tr("Use the equalizers of single tracks"),
        Some(&tr(
            "A track that has its own equalizer plays with that one in place of this one.",
        )),
    );
    row.set_margin_top(8);
    switch.set_widget_name("track-equalizers");
    let count = gtk::Label::new(None);
    count.set_xalign(0.0);
    count.add_css_class("hk-note");
    count.set_widget_name("track-equalizer-count");
    if let Some(text) = row.first_child().and_downcast::<gtk::Box>() {
        text.append(&count);
    }
    column.append(&divider());
    column.append(&row);
    {
        let live = live.clone();
        switch.connect_active_notify(move |switch| {
            let on = switch.is_active();
            let stored = live.settings().is_some_and(|s| s.dsp.track_equalizer);
            if stored != on && !live.apply(|s| s.dsp.track_equalizer = on) {
                switch.set_active(stored);
            }
        });
    }
    {
        let weak_live = Rc::downgrade(live);
        button.connect_clicked(move |_| {
            let Some(live) = weak_live.upgrade() else {
                return;
            };
            let (Some(context), Some(track)) =
                (live.context(), live.facts().track_eq.and_then(|t| t.track))
            else {
                return;
            };
            if let Some(main) = context.main.upgrade() {
                crate::dialogs::tag_editor::open_on_equalizer(&main, &[track]);
            }
        });
    }
    let weak_live = Rc::downgrade(live);
    let tracks = Cell::new(None::<usize>);
    let refresh_all: Rc<dyn Fn()> = Rc::new(move || {
        refresh();
        let Some(live) = weak_live.upgrade() else {
            return;
        };
        let on = live.settings().is_some_and(|s| s.dsp.track_equalizer);
        if switch.is_active() != on {
            switch.set_active(on);
        }
        // Counted once: the library changes it only from the tag editor.
        let n = match tracks.get() {
            Some(n) => n,
            None => {
                let n = live
                    .context()
                    .map_or(0, |c| c.services.dsp().tracks_with_equalizer());
                tracks.set(Some(n));
                n
            }
        };
        count.set_visible(n > 0);
        count.set_text(&fill(
            &ntr("{} track has one.", "{} tracks have one.", n),
            &[&n.to_string()],
        ));
    });
    EqualizerParts {
        column,
        notice,
        message,
        refresh: refresh_all,
    }
}

struct EqualizerParts {
    column: gtk::Box,
    notice: gtk::Box,
    message: gtk::Label,
    refresh: Rc<dyn Fn()>,
}

/// The DSP and EQ page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let (scroller, body) = page_body();
    body.set_widget_name("dsp-page");
    body.set_spacing(16);
    body.add_css_class("hk-dsp-page");
    outer.append(&scroller);
    let live = Live::new(context);
    live.update_facts();
    let settings = context.settings();

    let chain = chain::Chain::new(&live, &settings.dsp);
    body.append(chain.widget());

    let stack = gtk::Stack::new();
    stack.set_widget_name("dsp-editors");
    stack.set_vhomogeneous(false);
    stack.set_hhomogeneous(false);
    body.append(&stack);

    let readout = readout();
    outer.append(&readout.root);

    let eq = equalizer_page(&live);
    let page = Rc::new(Page {
        live: live.clone(),
        chain: chain.clone(),
        stack: stack.clone(),
        readout,
        track_notice: eq.notice.clone(),
        track_notice_text: eq.message.clone(),
        editors: RefCell::new(Vec::new()),
    });

    let (pitch, pitch_refresh) = pitch::editor(&live);
    stack.add_named(&pitch, Some(Stage::Pitch.id()));
    stack.add_named(&eq.column, Some(Stage::Equalizer.id()));
    let (convolution, convolution_refresh) = convolution::editor(&live);
    stack.add_named(&convolution, Some(Stage::Convolution.id()));
    let (crossfeed, crossfeed_refresh) = stages::crossfeed(&live);
    stack.add_named(&crossfeed, Some(Stage::Crossfeed.id()));
    let (mix, mix_refresh) = stages::channel_mix(&live);
    stack.add_named(&mix, Some(Stage::ChannelMix.id()));
    let (compressor, compressor_refresh) = compressor::editor(&live);
    stack.add_named(&compressor, Some(Stage::Compressor.id()));
    let limiter = stages::limiter(&live);
    stack.add_named(&limiter, Some(Stage::Limiter.id()));
    page.editors.borrow_mut().extend([
        pitch_refresh,
        eq.refresh.clone(),
        convolution_refresh,
        crossfeed_refresh,
        mix_refresh,
        compressor_refresh,
    ]);

    let weak = Rc::downgrade(&page);
    chain.connect_select(move |stage| {
        if let Some(page) = weak.upgrade() {
            page.select(stage);
        }
    });
    let weak = Rc::downgrade(&page);
    live.connect_applied(move || {
        if let Some(page) = weak.upgrade() {
            page.refresh();
        }
    });
    // What the engine reports about the track that plays.
    if let Some(playback) = &context.playback {
        let weak = Rc::downgrade(&page);
        playback.connect(move |_, change| {
            if !matches!(change, crate::playback::Change::Signal) {
                return;
            }
            if let Some(page) = weak.upgrade() {
                page.live.update_facts();
                page.refresh();
            }
        });
    }
    page.select(OPEN_AT.with(Cell::take).unwrap_or(Stage::Equalizer));
    page.refresh();
    // A request to show a stage while the page is already built.
    let weak = Rc::downgrade(&page);
    outer.connect_map(move |_| {
        if let (Some(page), Some(stage)) = (weak.upgrade(), OPEN_AT.with(Cell::take)) {
            page.select(stage);
        }
    });
    // The page lives as long as its widgets.
    let keep = RefCell::new(Some(page));
    outer.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    outer.upcast()
}

/// Opens Settings › DSP and EQ over `main` with the editor of `stage`, as the
/// rows of the Signal path tab do.
pub(crate) fn open_stage(main: &crate::window::MainWindow, stage: Stage) -> gtk::Window {
    show_stage(stage);
    let window = super::open(main, Some(super::Page::Dsp));
    // A page built before: choose the stage in its chain.
    let name = format!("stage-{}", stage.id());
    let mut pending: Vec<gtk::Widget> = window.child().into_iter().collect();
    while let Some(widget) = pending.pop() {
        if widget.widget_name() == name.as_str()
            && let Some(button) = widget.downcast_ref::<gtk::ToggleButton>()
        {
            OPEN_AT.with(Cell::take);
            button.emit_clicked();
            break;
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c);
        }
    }
    window
}
