// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › DSP and EQ: the preset row, the chain of stages and the editor of
//! the stage chosen in it, and a readout of the processing at the bottom.
//!
//! The chain is also the selector: activating a stage's name shows its editor
//! below; the switch beside the name only turns the stage on or off, and a
//! stage can be edited while it is off. Dither has no editor. Every change goes
//! to the settings store at once, which the player applies live; while a
//! control is dragged the changes are passed on at most every 80 ms, the last
//! one always.

pub(crate) mod equalizer;
mod stages;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use helikodu_model::settings::{ChannelMode, DspSettings, EqMode};

use super::{Context, page_body};
use crate::backend::SignalState;
use crate::backend::services::Settings;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::{SignalLamp, label};

/// A stage of the chain that has an editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The equalizer.
    Equalizer,
    /// Crossfeed.
    Crossfeed,
    /// The channel mixer.
    ChannelMix,
    /// The limiter.
    Limiter,
}

impl Stage {
    const ALL: [Self; 4] = [
        Self::Equalizer,
        Self::Crossfeed,
        Self::ChannelMix,
        Self::Limiter,
    ];

    /// The name of its editor in the page's stack.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Equalizer => "equalizer",
            Self::Crossfeed => "crossfeed",
            Self::ChannelMix => "channel-mix",
            Self::Limiter => "limiter",
        }
    }

    fn title(self) -> String {
        match self {
            Self::Equalizer => tr("Equalizer"),
            Self::Crossfeed => tr("Crossfeed"),
            Self::ChannelMix => tr("Channel mix"),
            Self::Limiter => tr("Limiter"),
        }
    }

    fn enabled(self, dsp: &DspSettings) -> bool {
        match self {
            Self::Equalizer => dsp.equalizer.enabled,
            Self::Crossfeed => dsp.crossfeed.enabled,
            Self::ChannelMix => dsp.channel_mix.enabled,
            Self::Limiter => dsp.limiter.enabled,
        }
    }

    fn set_enabled(self, settings: &mut Settings, on: bool) {
        let dsp = &mut settings.dsp;
        match self {
            Self::Equalizer => dsp.equalizer.enabled = on,
            Self::Crossfeed => dsp.crossfeed.enabled = on,
            Self::ChannelMix => dsp.channel_mix.enabled = on,
            Self::Limiter => dsp.limiter.enabled = on,
        }
    }

    /// The line under the name in the chain.
    fn summary(self, dsp: &DspSettings) -> String {
        match self {
            Self::Equalizer => {
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
            Self::Crossfeed if dsp.crossfeed.enabled => {
                let (hz, db) = dsp.crossfeed.effective();
                fill(
                    &tr("{} Hz / {} dB"),
                    &[&hz.to_string(), &crate::format::decimal(db, 1)],
                )
            }
            Self::ChannelMix if dsp.channel_mix.enabled => {
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
            Self::Limiter => fill(
                &tr("Lookahead {} ms"),
                &[&trim_number(dsp.limiter.lookahead_ms)],
            ),
            _ => tr("Bypassed"),
        }
    }

    /// The stage in the path of the readout, when it is on.
    fn in_path(self, dsp: &DspSettings) -> Option<String> {
        if !self.enabled(dsp) {
            return None;
        }
        Some(match self {
            Self::Equalizer => fill(&tr("EQ {}"), &[&self.summary(dsp)]),
            Self::Crossfeed => tr("crossfeed"),
            Self::ChannelMix => tr("channel mixer"),
            Self::Limiter => tr("limiter"),
        })
    }
}

/// `2`, `2.5`: a number with its decimals only when it has them.
pub(crate) fn trim_number(value: f64) -> String {
    let text = format!("{value:.1}");
    crate::format::localized(text.trim_end_matches('0').trim_end_matches('.').to_owned())
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
}

impl Live {
    const PERIOD: Duration = Duration::from_millis(80);

    fn new(context: &Rc<Context>) -> Rc<Self> {
        Rc::new(Self {
            context: Rc::downgrade(context),
            timer: Cell::new(None),
            pending: RefCell::new(None),
            after: RefCell::new(Vec::new()),
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

    /// Calls `listener` after every change that reached the store.
    fn connect_applied(&self, listener: impl Fn() + 'static) {
        self.after.borrow_mut().push(Box::new(listener));
    }

    fn commit(&self, change: &dyn Fn(&mut Settings)) -> bool {
        let Some(context) = self.context.upgrade() else {
            return false;
        };
        let done = context.update(change);
        for listener in self.after.borrow().iter() {
            listener();
        }
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

/// One stage box of the chain.
struct StageBox {
    stage: Stage,
    root: gtk::Box,
    button: gtk::ToggleButton,
    name: gtk::Label,
    summary: gtk::Label,
    switch: gtk::Switch,
}

/// The parts of the page that follow the settings.
struct Page {
    live: Rc<Live>,
    boxes: Vec<StageBox>,
    stack: gtk::Stack,
    lamp: SignalLamp,
    state: gtk::Label,
    path: gtk::Label,
    presets: gtk::Box,
    editors: RefCell<Vec<Rc<dyn Fn()>>>,
}

impl Page {
    /// Shows the stored settings in the chain, the readout and the preset row.
    fn refresh(&self) {
        let Some(settings) = self.live.settings() else {
            return;
        };
        let dsp = &settings.dsp;
        for b in &self.boxes {
            let on = b.stage.enabled(dsp);
            if b.switch.is_active() != on {
                b.switch.set_active(on);
            }
            b.summary.set_text(&b.stage.summary(dsp));
            if on {
                b.root.add_css_class("on");
                b.name.add_css_class("hk-strong");
            } else {
                b.root.remove_css_class("on");
                b.name.remove_css_class("hk-strong");
            }
        }
        let stages: Vec<String> = Stage::ALL
            .iter()
            .filter_map(|stage| stage.in_path(dsp))
            .collect();
        let device = self
            .live
            .context()
            .and_then(|c| c.playback.as_ref().map(|p| p.view().output.device))
            .filter(|d| !d.is_empty());
        if stages.is_empty() {
            self.lamp.set_state(SignalState::BitPerfect);
            self.state.set_text(&tr("BIT-PERFECT"));
            self.state.remove_css_class("processed");
            self.state.add_css_class("bit-perfect");
            self.path.set_text(&match device {
                Some(device) => fill(&tr("Every stage is off → {}"), &[&device]),
                None => tr("Every stage is off. The samples pass unchanged."),
            });
        } else {
            self.lamp.set_state(SignalState::Processed { dsp: true });
            self.state.set_text(&tr("DSP ACTIVE · 32-BIT FLOAT"));
            self.state.remove_css_class("bit-perfect");
            self.state.add_css_class("processed");
            let mut parts = stages;
            parts.push(tr("TPDF dither"));
            if let Some(device) = device {
                parts.push(device);
            }
            // Wrap only between stages: the arrow starts the new line.
            let parts: Vec<String> = parts.iter().map(|p| p.replace(' ', "\u{00A0}")).collect();
            self.path.set_text(&parts.join(" →\u{00A0}"));
        }
        for editor in self.editors.borrow().iter() {
            editor();
        }
    }

    fn select(&self, stage: Stage) {
        self.stack.set_visible_child_name(stage.id());
        for b in &self.boxes {
            let chosen = b.stage == stage;
            if b.button.is_active() != chosen {
                b.button.set_active(chosen);
            }
            if chosen {
                b.root.add_css_class("selected");
            } else {
                b.root.remove_css_class("selected");
            }
        }
        // The preset row belongs to the equalizer.
        self.presets.set_visible(stage == Stage::Equalizer);
    }
}

fn stage_box(stage: Stage, dsp: &DspSettings) -> StageBox {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("hk-stage-box");
    root.set_hexpand(true);
    let overlay = gtk::Overlay::new();
    let button = gtk::ToggleButton::new();
    button.add_css_class("hk-stage-select");
    button.set_widget_name(&format!("stage-{}", stage.id()));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let name = label(&stage.title(), &["hk-stage-name"]);
    // Room for the switch at the right.
    name.set_margin_end(40);
    text.append(&name);
    let summary = label(&stage.summary(dsp), &["hk-mono-meta", "hk-t3"]);
    text.append(&summary);
    button.set_child(Some(&text));
    button.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("Edit {}"),
        &[&stage.title()],
    ))]);
    overlay.set_child(Some(&button));
    let switch = gtk::Switch::new();
    switch.add_css_class("hk-switch");
    switch.set_halign(gtk::Align::End);
    switch.set_valign(gtk::Align::Start);
    switch.set_margin_top(9);
    switch.set_margin_end(12);
    switch.set_active(stage.enabled(dsp));
    switch.set_widget_name(&format!("stage-switch-{}", stage.id()));
    switch.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("{} on"),
        &[&stage.title()],
    ))]);
    overlay.add_overlay(&switch);
    root.append(&overlay);
    StageBox {
        stage,
        root,
        button,
        name,
        summary,
        switch,
    }
}

/// The Dither box: always automatic, no editor.
fn dither_box() -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 3);
    root.add_css_class("hk-stage-box");
    root.add_css_class("on");
    root.add_css_class("fixed");
    root.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = label(&tr("Dither"), &["hk-stage-name", "hk-strong"]);
    name.set_hexpand(true);
    head.append(&name);
    head.append(&label(&tr("Auto"), &["hk-t3", "hk-secondary"]));
    root.append(&head);
    root.append(&label("TPDF", &["hk-mono-meta", "hk-t3"]));
    root.update_property(&[gtk::accessible::Property::Label(&tr(
        "Dither: automatic, TPDF",
    ))]);
    root
}

/// A joint between two stage boxes.
fn joint() -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.add_css_class("hk-stage-joint");
    line.set_size_request(10, 1);
    line.set_valign(gtk::Align::Center);
    line
}

/// One or two sentences that say what a stage is for.
pub(crate) fn lead(text: &str) -> gtk::Label {
    let lead = gtk::Label::new(Some(text));
    lead.set_xalign(0.0);
    lead.set_wrap(true);
    lead.set_max_width_chars(120);
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

fn readout() -> (gtk::Box, SignalLamp, gtk::Label, gtk::Label) {
    let readout = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    readout.add_css_class("hk-readout");
    readout.add_css_class("hk-dsp-readout");
    readout.set_valign(gtk::Align::End);
    let lamp = SignalLamp::default();
    lamp.set_valign(gtk::Align::Center);
    readout.append(&lamp);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let state = label("", &["hk-signal"]);
    text.append(&state);
    let path = gtk::Label::new(None);
    path.set_xalign(0.0);
    path.set_wrap(true);
    path.add_css_class("hk-mono");
    path.add_css_class("hk-dsp-path");
    path.set_widget_name("dsp-path");
    text.append(&path);
    readout.append(&text);
    readout.update_property(&[gtk::accessible::Property::Label(&tr(
        "Processing right now",
    ))]);
    (readout, lamp, state, path)
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

/// The DSP and EQ page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let (scroller, body) = page_body();
    body.set_widget_name("dsp-page");
    let settings = context.settings();
    let live = Live::new(context);

    let presets = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    presets.set_widget_name("dsp-presets");
    body.append(&presets);

    let chain = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    chain.add_css_class("hk-stage-chain");
    chain.update_property(&[gtk::accessible::Property::Label(&tr("Processing stages"))]);
    let boxes: Vec<StageBox> = Stage::ALL
        .iter()
        .map(|&stage| stage_box(stage, &settings.dsp))
        .collect();
    for b in &boxes {
        chain.append(&b.root);
        chain.append(&joint());
    }
    chain.append(&dither_box());
    body.append(&chain);

    let stack = gtk::Stack::new();
    stack.set_widget_name("dsp-editors");
    stack.set_vhomogeneous(false);
    stack.set_hhomogeneous(false);
    body.append(&stack);
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    body.append(&spacer);
    let (readout_box, lamp, state, path) = readout();
    body.append(&readout_box);

    let page = Rc::new(Page {
        live: live.clone(),
        boxes,
        stack: stack.clone(),
        lamp,
        state,
        path,
        presets: presets.clone(),
        editors: RefCell::new(Vec::new()),
    });

    let (eq_editor, eq_refresh) = equalizer::editor(&live, &presets);
    stack.add_named(&eq_editor, Some(Stage::Equalizer.id()));
    let (crossfeed, crossfeed_refresh) = stages::crossfeed(&live);
    stack.add_named(&crossfeed, Some(Stage::Crossfeed.id()));
    let (mix, mix_refresh) = stages::channel_mix(&live);
    stack.add_named(&mix, Some(Stage::ChannelMix.id()));
    let limiter = stages::limiter(&live);
    stack.add_named(&limiter, Some(Stage::Limiter.id()));
    page.editors
        .borrow_mut()
        .extend([eq_refresh, crossfeed_refresh, mix_refresh]);

    for b in &page.boxes {
        let weak = Rc::downgrade(&page);
        let stage = b.stage;
        b.button.connect_clicked(move |button| {
            if let Some(page) = weak.upgrade() {
                page.select(stage);
                if !button.is_active() {
                    button.set_active(true);
                }
            }
        });
        let weak = Rc::downgrade(&page);
        b.switch.connect_active_notify(move |switch| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            let on = switch.is_active();
            let stored = page.live.settings().is_some_and(|s| stage.enabled(&s.dsp));
            if stored != on && !page.live.apply(|s| stage.set_enabled(s, on)) {
                switch.set_active(stored);
            }
        });
    }
    let weak = Rc::downgrade(&page);
    live.connect_applied(move || {
        if let Some(page) = weak.upgrade() {
            page.refresh();
        }
    });
    page.select(OPEN_AT.with(Cell::take).unwrap_or(Stage::Equalizer));
    page.refresh();
    // A request to show a stage while the page is already built.
    let weak = Rc::downgrade(&page);
    scroller.connect_map(move |_| {
        if let (Some(page), Some(stage)) = (weak.upgrade(), OPEN_AT.with(Cell::take)) {
            page.select(stage);
        }
    });
    // The page lives as long as its widgets.
    let keep = RefCell::new(Some(page));
    scroller.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    scroller.upcast()
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
