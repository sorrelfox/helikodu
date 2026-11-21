// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › DSP and EQ against the sample backend: the chain in its order
//! (moved with the keys and from a stage's menu, back to the default), the
//! saved chains (save, apply, delete and undo), and the editors of the
//! compressor, convolution and pitch, the equalizer of a track that plays,
//! and the readout with the delay the stages add.
//!
//! Like `widgets.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test dsp
//! ```

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use helikodu_model::player::{SignalLamp, StageKind, StageState};
use helikodu_model::settings::{ChainStage, CompressorPreset};
use helikodu_ui_gtk::backend::{Backend, Update, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, chooser, install_actions, prepare_display};

fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A window over the sample backend with a track playing, and the channel
/// that stands in for the player's updates.
fn window(application: &gtk::Application) -> (MainWindow, Backend, async_channel::Sender<Update>) {
    let (backend, updates) = sample::backend_with_updates(&sample::SampleOptions {
        albums: 40,
        playing: Some((0, 2, 102)),
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(150);
    (window, backend, updates)
}

/// Every descendant of `root`.
fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut all = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c.clone());
            all.push(c);
        }
    }
    all
}

fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> T {
    descendants(root.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<T>().ok())
        .unwrap_or_else(|| panic!("no widget {name}"))
}

fn has_named(root: &impl IsA<gtk::Widget>, name: &str) -> bool {
    descendants(root.upcast_ref())
        .iter()
        .any(|w| w.widget_name() == name)
}

/// The button whose label reads `text`.
fn button(root: &impl IsA<gtk::Widget>, text: &str) -> gtk::Button {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| {
            b.label().as_deref() == Some(text)
                || descendants(b.upcast_ref()).iter().any(|w| {
                    w.downcast_ref::<gtk::Label>()
                        .is_some_and(|l| l.text() == text)
                })
        })
        .unwrap_or_else(|| panic!("no button {text}"))
}

/// The text of every visible label under `root`, joined.
fn texts(root: &impl IsA<gtk::Widget>) -> String {
    descendants(root.upcast_ref())
        .iter()
        .filter(|w| is_shown(w))
        .filter_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.text().to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `widget` and every ancestor are visible.
fn is_shown(widget: &gtk::Widget) -> bool {
    let mut at = Some(widget.clone());
    while let Some(w) = at {
        if !w.is_visible() {
            return false;
        }
        at = w.parent();
    }
    true
}

/// Presses `key` with `state` on the key controller of `widget`.
fn press(widget: &impl IsA<gtk::Widget>, key: gdk::Key, state: gdk::ModifierType) {
    let controllers = widget.observe_controllers();
    for i in 0..controllers.n_items() {
        if let Some(keys) = controllers
            .item(i)
            .and_downcast::<gtk::EventControllerKey>()
        {
            let _: bool = keys.emit_by_name("key-pressed", &[&key.into_glib(), &0u32, &state]);
            return;
        }
    }
    panic!("no key controller on {}", widget.widget_name());
}

fn label_text(root: &impl IsA<gtk::Widget>, name: &str) -> String {
    // The readout keeps a stage on one line with no-break spaces.
    named::<gtk::Label>(root, name)
        .text()
        .replace('\u{a0}', " ")
}

/// Saves `window` as `name`.png in `HELIKODU_DSP_SHOTS`, when it is set.
fn shot(window: &gtk::Window, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_DSP_SHOTS").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    spin(300);
    helikodu_ui_gtk::snapshot::save_png(window, &dir.join(format!("{name}.png"))).unwrap();
}

/// The order of the chain as the settings hold it.
fn order(backend: &Backend) -> Vec<ChainStage> {
    backend.services().settings().dsp.effective_order()
}

/// The chain in its order: Alt and the arrow keys move a stage, a stage's
/// menu moves it and turns it on, "Default order" puts the order back
/// without touching the switches. The places follow, and the readout lists
/// the stages that work in the order of the chain.
fn the_chain_moves_and_numbers_its_stages(app: &gtk::Application) {
    let (window, backend, _) = window(app);
    let settings = window.open_dsp_stage("equalizer");
    spin(100);
    assert_eq!(order(&backend), ChainStage::DEFAULT_ORDER);
    let reset: gtk::Button = named(&settings, "chain-default-order");
    assert!(!reset.is_sensitive());
    let pitch: gtk::ToggleButton = named(&settings, "stage-pitch");
    assert_eq!(label_text(&settings, "stage-place-pitch"), "1");
    // Alt+Right moves Pitch one place later.
    press(&pitch, gdk::Key::Right, gdk::ModifierType::ALT_MASK);
    spin(50);
    assert_eq!(
        order(&backend)[..2],
        [ChainStage::Equalizer, ChainStage::Pitch]
    );
    assert_eq!(label_text(&settings, "stage-place-pitch"), "2");
    assert!(reset.is_sensitive());
    // Alt+Left on the first stage does nothing.
    let equalizer: gtk::ToggleButton = named(&settings, "stage-equalizer");
    press(&equalizer, gdk::Key::Left, gdk::ModifierType::ALT_MASK);
    spin(50);
    assert_eq!(order(&backend)[0], ChainStage::Equalizer);
    // The Menu key opens the stage's menu; its items act on that stage.
    let limiter: gtk::ToggleButton = named(&settings, "stage-limiter");
    press(&limiter, gdk::Key::Menu, gdk::ModifierType::empty());
    spin(100);
    assert!(has_named(&settings, "chain-menu"));
    limiter.activate_action("chain.earlier", None).unwrap();
    spin(50);
    assert_eq!(order(&backend)[5], ChainStage::Limiter);
    press(&limiter, gdk::Key::Left, gdk::ModifierType::ALT_MASK);
    press(&limiter, gdk::Key::Left, gdk::ModifierType::ALT_MASK);
    spin(50);
    assert_eq!(order(&backend)[3], ChainStage::Limiter);
    assert_eq!(label_text(&settings, "stage-place-limiter"), "4");
    limiter.activate_action("chain.toggle", None).unwrap();
    spin(50);
    assert!(backend.services().settings().dsp.limiter.enabled);
    let switch: gtk::Switch = named(&settings, "stage-switch-limiter");
    assert!(switch.is_active());
    // The readout lists what works, in the order of the chain.
    named::<gtk::Switch>(&settings, "stage-switch-crossfeed").set_active(true);
    spin(50);
    let path = label_text(&settings, "dsp-path");
    let at = |what: &str| {
        path.find(what)
            .unwrap_or_else(|| panic!("{what} in {path}"))
    };
    assert!(at("limiter") < at("crossfeed"), "{path}");
    assert_eq!(label_text(&settings, "dsp-delay"), "Adds 2 ms");
    shot(&settings, "chain");
    // Default order: the order goes back, the switches stay.
    reset.emit_clicked();
    spin(50);
    assert_eq!(order(&backend), ChainStage::DEFAULT_ORDER);
    assert!(!reset.is_sensitive());
    assert!(backend.services().settings().dsp.limiter.enabled);
    let path = label_text(&settings, "dsp-path");
    assert!(path.find("crossfeed") < path.find("limiter"), "{path}");
    // Every stage off: bit-perfect again.
    named::<gtk::Switch>(&settings, "stage-switch-crossfeed").set_active(false);
    switch.set_active(false);
    spin(50);
    assert!(!named::<gtk::Label>(&settings, "dsp-delay").is_visible());
    assert!(label_text(&settings, "dsp-path").starts_with("Every stage is off"));
    assert_eq!(backend.services().settings().validate(), Ok(()));
    settings.close();
    window.destroy();
}

/// Saved chains: a name saves the chain as it is now, the row of the chain
/// in use is marked, activating a row applies it, the bin deletes it with an
/// Undo toast that brings it back.
fn saved_chains_save_apply_and_delete(app: &gtk::Application) {
    let (window, backend, _) = window(app);
    let services = backend.services();
    let settings = window.open_dsp_stage("equalizer");
    spin(100);
    let saved: gtk::MenuButton = named(&settings, "saved-chains");
    saved.popup();
    spin(100);
    assert!(texts(&settings).contains("No saved chains yet."));
    // Equalizer, Compressor and Limiter on, the compressor moved first.
    named::<gtk::Switch>(&settings, "stage-switch-equalizer").set_active(true);
    named::<gtk::Switch>(&settings, "stage-switch-compressor").set_active(true);
    named::<gtk::Switch>(&settings, "stage-switch-limiter").set_active(true);
    let compressor: gtk::ToggleButton = named(&settings, "stage-compressor");
    for _ in 0..5 {
        press(&compressor, gdk::Key::Left, gdk::ModifierType::ALT_MASK);
    }
    spin(50);
    assert_eq!(order(&backend)[0], ChainStage::Compressor);
    let entry: gtk::Entry = named(&settings, "chain-name");
    let save: gtk::Button = named(&settings, "chain-save");
    assert!(!save.is_sensitive());
    entry.set_text("Night");
    assert_eq!(save.label().as_deref(), Some("Save"));
    entry.emit_activate();
    spin(50);
    let chains = services.settings().dsp.chains.clone();
    assert_eq!(chains.len(), 1);
    assert_eq!(
        chains[0].stages,
        [
            ChainStage::Compressor,
            ChainStage::Equalizer,
            ChainStage::Limiter
        ]
    );
    assert!(entry.text().is_empty());
    let list: gtk::ListBox = named(&settings, "saved-chain-list");
    let row = list.row_at_index(0).unwrap();
    assert_eq!(row.widget_name(), "Night");
    let all = texts(&row);
    assert!(all.contains("Compressor → Equalizer → Limiter"), "{all}");
    // A second chain: Crossfeed alone.
    reset_switches(&settings);
    named::<gtk::Switch>(&settings, "stage-switch-crossfeed").set_active(true);
    entry.set_text("Headphones");
    named::<gtk::Button>(&settings, "chain-save").emit_clicked();
    spin(50);
    assert_eq!(services.settings().dsp.chains.len(), 2);
    // A name that exists replaces.
    entry.set_text("Night");
    assert_eq!(save.label().as_deref(), Some("Replace"));
    entry.set_text("");
    // Applying Night: its stages on in its order, the rest off.
    list.row_at_index(0).unwrap().activate();
    spin(50);
    let dsp = services.settings().dsp.clone();
    assert!(dsp.compressor.enabled && dsp.equalizer.enabled && dsp.limiter.enabled);
    assert!(!dsp.crossfeed.enabled);
    assert_eq!(dsp.effective_order()[0], ChainStage::Compressor);
    // The popover stayed open.
    assert!(saved.popover().unwrap().is_visible());
    // Deleting asks nothing; Undo brings the chain back in its place.
    named::<gtk::Button>(&settings, "delete-chain-Night").emit_clicked();
    spin(50);
    let names: Vec<String> = services
        .settings()
        .dsp
        .chains
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(names, ["Headphones"]);
    shot(&settings, "chains");
    assert_eq!(
        window.toast_text().as_deref(),
        Some("Deleted the chain “Night”")
    );
    button(&window, "Undo").emit_clicked();
    spin(50);
    let names: Vec<String> = services
        .settings()
        .dsp
        .chains
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(names, ["Night", "Headphones"]);
    assert_eq!(services.settings().validate(), Ok(()));
    saved.popdown();
    settings.close();
    window.destroy();
}

/// Turns every stage of the page off.
fn reset_switches(settings: &gtk::Window) {
    for id in [
        "pitch",
        "equalizer",
        "convolution",
        "crossfeed",
        "channel-mix",
        "compressor",
        "limiter",
    ] {
        named::<gtk::Switch>(settings, &format!("stage-switch-{id}")).set_active(false);
    }
    spin(50);
}

/// The compressor: Night shows its values without letting them change, the
/// limiter hint turns the limiter on, Custom starts from the preset's values
/// and takes changes, and the activity comes from the player.
fn the_compressor_page(app: &gtk::Application) {
    let (window, backend, _) = window(app);
    let services = backend.services();
    let settings = window.open_dsp_stage("compressor");
    spin(100);
    let stack: gtk::Stack = named(&settings, "dsp-editors");
    assert_eq!(stack.visible_child_name().as_deref(), Some("compressor"));
    named::<gtk::Switch>(&settings, "stage-switch-compressor").set_active(true);
    spin(50);
    assert_eq!(label_text(&settings, "stage-summary-compressor"), "Night");
    let threshold: gtk::Scale = named(&settings, "compressor-threshold");
    assert!(!threshold.is_sensitive());
    assert_eq!(threshold.value(), -30.0);
    // The limiter is off: the hint says so and turns it on.
    let hint: gtk::Box = named(&settings, "limiter-hint");
    assert!(hint.is_visible());
    assert!(texts(&hint).contains("Night adds up to 12 dB of makeup gain."));
    named::<gtk::Button>(&settings, "turn-limiter-on").emit_clicked();
    spin(50);
    assert!(services.settings().dsp.limiter.enabled);
    assert!(!hint.is_visible());
    // The activity readout.
    spin(300);
    assert_eq!(label_text(&settings, "compressor-reduction"), "−6.5 dB");
    // Custom: the values of Night, now editable.
    button(&settings, "Custom")
        .downcast::<gtk::ToggleButton>()
        .unwrap()
        .set_active(true);
    spin(50);
    let compressor = services.settings().dsp.compressor;
    assert_eq!(compressor.preset, CompressorPreset::Custom);
    assert_eq!(compressor.threshold_db, -30.0);
    assert_eq!(compressor.makeup_db, 12.0);
    assert!(threshold.is_sensitive());
    shot(&settings, "compressor");
    threshold.set_value(-18.0);
    spin(150);
    assert_eq!(services.settings().dsp.compressor.threshold_db, -18.0);
    assert_eq!(
        label_text(&settings, "stage-summary-compressor"),
        "4 : 1 at −18 dB"
    );
    // The lookahead is used by every preset, and adds its delay.
    named::<gtk::Scale>(&settings, "compressor-lookahead").set_value(3.5);
    spin(150);
    assert_eq!(services.settings().dsp.compressor.lookahead_ms, 3.5);
    assert_eq!(services.settings().validate(), Ok(()));
    settings.close();
    window.destroy();
}

/// Convolution: the table lists every rate with a file and the rate in use;
/// a file that cannot be read is marked, and while it is the one in use the
/// stage is left out (the chain box warns, the readout leaves it out); a file
/// chosen for a rate comes from the chooser.
fn the_convolution_page(app: &gtk::Application) {
    let (window, backend, updates) = window(app);
    let services = backend.services();
    services
        .update_settings(&mut |s| {
            s.dsp.convolution.enabled = true;
            s.dsp.limiter.enabled = true;
        })
        .unwrap();
    let settings = window.open_dsp_stage("convolution");
    spin(100);
    // Nothing chosen yet.
    assert!(named::<gtk::Label>(&settings, "ir-empty").is_visible());
    assert!(named::<gtk::Button>(&settings, "ir-choose-file").is_visible());
    assert!(!named::<gtk::Button>(&settings, "ir-clear").is_sensitive());
    assert_eq!(
        label_text(&settings, "stage-summary-convolution"),
        "No file"
    );
    assert!(!label_text(&settings, "dsp-path").contains("convolution"));
    // A folder from the chooser.
    let answers: Rc<RefCell<Vec<PathBuf>>> = Rc::new(RefCell::new(vec![
        PathBuf::from("/sample/Room correction"),
        PathBuf::from("/sample/room-96k-broken.wav"),
        PathBuf::from("/sample/room-96k-v2.wav"),
    ]));
    let queue = answers.clone();
    chooser::replace_folder_chooser(Some(Rc::new(move |_title, reply| {
        let next = queue.borrow_mut().remove(0);
        reply(Some(next));
    })));
    named::<gtk::Button>(&settings, "ir-choose-folder").emit_clicked();
    spin(100);
    assert_eq!(
        services.settings().dsp.convolution.folder,
        Some(PathBuf::from("/sample/Room correction"))
    );
    assert!(has_named(&settings, "ir-row-44100"));
    assert!(has_named(&settings, "ir-row-48000"));
    assert_eq!(
        label_text(&settings, "stage-summary-convolution"),
        "44.1 · 48 kHz"
    );
    // A file chosen by hand for 48 kHz that is not a WAV file.
    named::<gtk::Button>(&settings, "ir-choose-48000").emit_clicked();
    spin(100);
    // The chooser gave the broken file; it was chosen for 48 kHz.
    let files = services.settings().dsp.convolution.files.clone();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].rate_hz, 48_000);
    assert_eq!(
        label_text(&settings, "ir-problem-48000"),
        "cannot be read: it is not a WAV file"
    );
    // The engine leaves it out while it plays at 48 kHz.
    let mut path = sample::signal_path(SignalLamp::Resampled);
    for stage in &mut path.stages {
        if stage.kind == StageKind::Convolution {
            stage.state = StageState::Bypassed;
            stage.detail = "left out: the impulse response cannot be used".into();
        }
    }
    updates
        .try_send(Update::SignalPath(Some(Box::new(path.clone()))))
        .unwrap();
    spin(100);
    assert_eq!(
        label_text(&settings, "stage-summary-convolution"),
        "1 file cannot be read"
    );
    assert!(texts(&settings).contains("In use now. Convolution is left out until this is fixed."));
    assert!(!label_text(&settings, "dsp-path").contains("convolution"));
    shot(&settings, "convolution-problem");
    // A good file replaces it; at 88.2 kHz the nearest file is used.
    named::<gtk::Button>(&settings, "ir-choose-48000").emit_clicked();
    spin(100);
    chooser::replace_folder_chooser(None);
    assert!(!has_named(&settings, "ir-problem-48000"));
    for stage in &mut path.stages {
        if stage.kind == StageKind::Convolution {
            stage.state = StageState::Active;
            stage.detail = "convolution mono · 32768 taps · latency 10.7 ms".into();
            stage.delay_ms = 10.7;
        }
    }
    path.device.format.rate = helikodu_model::audio::SampleRate::new(88_200).unwrap();
    updates
        .try_send(Update::SignalPath(Some(Box::new(path))))
        .unwrap();
    spin(100);
    assert!(has_named(&settings, "ir-row-88200"));
    let all = texts(&settings);
    assert!(all.contains("Uses the 48 kHz file, resampled"), "{all}");
    assert!(label_text(&settings, "dsp-path").contains("convolution"));
    assert_eq!(label_text(&settings, "dsp-delay"), "Adds 13 ms");
    shot(&settings, "convolution");
    // The mix.
    named::<gtk::Switch>(&settings, "ir-normalize").set_active(false);
    named::<gtk::Scale>(&settings, "ir-wet").set_value(40.0);
    spin(150);
    let convolution = services.settings().dsp.convolution.clone();
    assert!(!convolution.normalize);
    assert_eq!(convolution.wet_percent, 40.0);
    // Back to the file of the folder.
    named::<gtk::Button>(&settings, "ir-forget-48000").emit_clicked();
    spin(50);
    assert!(services.settings().dsp.convolution.files.is_empty());
    named::<gtk::Button>(&settings, "ir-clear").emit_clicked();
    spin(50);
    assert_eq!(services.settings().dsp.convolution.folder, None);
    assert_eq!(services.settings().validate(), Ok(()));
    settings.close();
    window.destroy();
}

/// Pitch: whole semitones with the arrow keys, read in words in the chain
/// and the readout; at 0 the stage counts as off; Reset to 0.
fn the_pitch_page(app: &gtk::Application) {
    let (window, backend, _) = window(app);
    let services = backend.services();
    let settings = window.open_dsp_stage("pitch");
    spin(100);
    named::<gtk::Switch>(&settings, "stage-switch-pitch").set_active(true);
    spin(50);
    // On at 0: it does nothing.
    assert_eq!(label_text(&settings, "stage-summary-pitch"), "Bypassed");
    assert!(label_text(&settings, "dsp-path").starts_with("Every stage is off"));
    let reset: gtk::Button = named(&settings, "pitch-reset");
    assert!(!reset.is_sensitive());
    let scale: gtk::Widget = named(&settings, "pitch-shift");
    press(&scale, gdk::Key::Right, gdk::ModifierType::empty());
    press(&scale, gdk::Key::Right, gdk::ModifierType::empty());
    spin(150);
    assert_eq!(services.settings().dsp.pitch.semitones, 2.0);
    assert_eq!(label_text(&settings, "pitch-value"), "+2 semitones");
    assert_eq!(label_text(&settings, "stage-summary-pitch"), "+2 semitones");
    assert!(label_text(&settings, "dsp-path").contains("pitch +2 semitones"));
    shot(&settings, "pitch");
    // The sample player reports the path of what plays; with nothing on it
    // but the pitch, the delay is the estimate of the stage.
    assert!(reset.is_sensitive());
    reset.emit_clicked();
    spin(50);
    assert_eq!(services.settings().dsp.pitch.semitones, 0.0);
    assert_eq!(label_text(&settings, "pitch-value"), "0 semitones");
    assert_eq!(services.settings().validate(), Ok(()));
    settings.close();
    window.destroy();
}

/// A track that plays with its own equalizer: the chain box names it, a
/// notice says the editor is the main equalizer, the readout lists the
/// track's equalizer; the switch for all track equalizers is stored.
fn a_track_with_its_own_equalizer(app: &gtk::Application) {
    let (window, backend, updates) = window(app);
    let services = backend.services();
    services
        .update_settings(&mut |s| s.dsp.equalizer.enabled = true)
        .unwrap();
    let settings = window.open_dsp_stage("equalizer");
    spin(100);
    let notice: gtk::Box = named(&settings, "track-eq-notice");
    assert!(!notice.is_visible());
    assert_eq!(
        label_text(&settings, "track-equalizer-count"),
        "3 tracks have one."
    );
    let mut path = sample::signal_path(SignalLamp::LosslessProcessed);
    for stage in &mut path.stages {
        if stage.kind == StageKind::Equalizer {
            stage.detail = "track EQ 4 filters · preamp -3.0 dB".into();
        }
    }
    updates
        .try_send(Update::SignalPath(Some(Box::new(path))))
        .unwrap();
    spin(100);
    assert!(notice.is_visible());
    assert!(
        texts(&notice).contains("This track plays with its own equalizer, “Bright master fix”")
    );
    assert_eq!(
        label_text(&settings, "stage-summary-equalizer"),
        "Track: Bright master fix"
    );
    assert!(label_text(&settings, "dsp-path").contains("track EQ 4 filters"));
    shot(&settings, "track-equalizer");
    // The editor below stays the main equalizer.
    assert!(named::<gtk::DropDown>(&settings, "eq-preset").is_sensitive());
    let switch: gtk::Switch = named(&settings, "track-equalizers");
    assert!(switch.is_active());
    switch.set_active(false);
    spin(50);
    assert!(!services.settings().dsp.track_equalizer);
    settings.close();
    window.destroy();
}

/// The Signal path tab lists the DSP stages in the order the engine reports
/// them, between Resampler and Volume, with the delay they add.
fn the_signal_path_follows_the_chain(app: &gtk::Application) {
    let (window, _, updates) = window(app);
    window.show_signal_path();
    spin(100);
    let mut path = sample::signal_path(SignalLamp::Resampled);
    let at = |path: &helikodu_model::player::SignalPath, kind| {
        path.stages.iter().position(|s| s.kind == kind).unwrap()
    };
    let (pitch, convolution) = (
        at(&path, StageKind::Pitch),
        at(&path, StageKind::Convolution),
    );
    path.stages.swap(pitch, convolution);
    for stage in &mut path.stages {
        if stage.kind == StageKind::Compressor {
            stage.state = StageState::Active;
            stage.detail = "night mode · compressor -30.0 dB · 4:1 · makeup +12.0 dB".into();
            stage.delay_ms = 2.0;
        }
    }
    updates
        .try_send(Update::SignalPath(Some(Box::new(path))))
        .unwrap();
    spin(100);
    let texts = window.signal_path_texts();
    let names: Vec<&str> = texts.iter().filter_map(|t| t.split(':').next()).collect();
    let index = |name: &str| names.iter().position(|n| *n == name).unwrap();
    assert!(index("Resampler") < index("Convolution"), "{texts:?}");
    assert!(index("Convolution") < index("Pitch"), "{texts:?}");
    assert!(index("Limiter") < index("Volume"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t
            == "Compressor: night mode · compressor -30.0 dB · 4:1 · makeup +12.0 dB · 2 ms [Active]"),
        "{texts:?}"
    );
    assert_eq!(
        texts[1],
        "7 stages are active. Processing runs in 32-bit float and adds 4 ms."
    );
    assert!(texts.iter().any(|t| t == "Delay: 4 ms"), "{texts:?}");
    shot(window.upcast_ref(), "signal-path");
    window.destroy();
}

/// A change of the settings made elsewhere (the command line, the settings
/// file, another window) shows on the open page at once: the chain, the
/// editor of the chosen stage and the readout follow, the chosen stage stays,
/// and the page writes nothing back.
fn the_page_follows_changes_made_elsewhere(app: &gtk::Application) {
    let (window, backend, _) = window(app);
    let services = backend.services();
    let settings = window.open_dsp_stage("compressor");
    spin(100);
    let stack: gtk::Stack = named(&settings, "dsp-editors");
    let threshold: gtk::Scale = named(&settings, "compressor-threshold");
    assert!(!named::<gtk::Switch>(&settings, "stage-switch-limiter").is_active());
    assert!(label_text(&settings, "dsp-path").starts_with("Every stage is off"));
    // Changed outside the page, as the service announces a reloaded file.
    services
        .update_settings(&mut |s| {
            s.dsp.compressor.enabled = true;
            s.dsp.compressor.preset = CompressorPreset::Custom;
            s.dsp.compressor.threshold_db = -24.0;
            s.dsp.limiter.enabled = true;
            s.dsp.order = vec![
                ChainStage::Limiter,
                ChainStage::Pitch,
                ChainStage::Equalizer,
                ChainStage::Convolution,
                ChainStage::Crossfeed,
                ChainStage::ChannelMix,
                ChainStage::Compressor,
            ];
        })
        .unwrap();
    let stored = services.settings();
    spin(150);
    assert!(named::<gtk::Switch>(&settings, "stage-switch-limiter").is_active());
    assert!(named::<gtk::Switch>(&settings, "stage-switch-compressor").is_active());
    assert_eq!(label_text(&settings, "stage-place-limiter"), "1");
    assert_eq!(
        label_text(&settings, "stage-summary-compressor"),
        "4 : 1 at −24 dB"
    );
    assert_eq!(threshold.value(), -24.0);
    assert!(threshold.is_sensitive());
    let path = label_text(&settings, "dsp-path");
    assert!(path.find("limiter") < path.find("compressor"), "{path}");
    // The stage chosen stays.
    assert_eq!(stack.visible_child_name().as_deref(), Some("compressor"));
    assert!(named::<gtk::ToggleButton>(&settings, "stage-compressor").is_active());
    // Showing it wrote nothing back.
    assert_eq!(services.settings(), stored);
    // The page's own changes go on as before.
    threshold.set_value(-12.0);
    spin(150);
    assert_eq!(services.settings().dsp.compressor.threshold_db, -12.0);
    assert_eq!(threshold.value(), -12.0);
    // A change elsewhere while a control is dragged shows after the drag.
    threshold.set_value(-14.0);
    services
        .update_settings(&mut |s| s.dsp.crossfeed.enabled = true)
        .unwrap();
    threshold.set_value(-16.0);
    spin(250);
    let dsp = services.settings().dsp.clone();
    assert_eq!(dsp.compressor.threshold_db, -16.0);
    assert!(dsp.crossfeed.enabled);
    assert_eq!(threshold.value(), -16.0);
    assert!(named::<gtk::Switch>(&settings, "stage-switch-crossfeed").is_active());
    settings.close();
    window.destroy();
}

/// Convolution left out while music plays: one toast names the rate, it is
/// not shown again for the same file, and "Details…" opens the convolution
/// stage of Settings › DSP and EQ.
fn the_convolution_toast_shows_once(app: &gtk::Application) {
    let (window, backend, updates) = window(app);
    let services = backend.services();
    services
        .update_settings(&mut |s| {
            s.dsp.convolution.enabled = true;
            s.dsp.convolution.files = vec![helikodu_model::settings::ImpulseFile {
                rate_hz: 96_000,
                path: PathBuf::from("/sample/room-96k-broken.wav"),
            }];
        })
        .unwrap();
    spin(50);
    let mut path = sample::signal_path(SignalLamp::Resampled);
    path.device.format.rate = helikodu_model::audio::SampleRate::new(96_000).unwrap();
    for stage in &mut path.stages {
        if stage.kind == StageKind::Convolution {
            stage.state = StageState::Bypassed;
            stage.detail = "left out: the impulse response cannot be used".into();
        }
    }
    let text = "The impulse response for 96 kHz cannot be read";
    updates
        .try_send(Update::SignalPath(Some(Box::new(path.clone()))))
        .unwrap();
    spin(100);
    assert_eq!(window.toast_text().as_deref(), Some(text));
    // It stays: no clock.
    spin(5500);
    assert_eq!(window.toast_text().as_deref(), Some(text));
    shot(window.upcast_ref(), "convolution-toast");
    // Dismissed, it does not come back for the same file and rate.
    window.toast("Something else", false);
    updates
        .try_send(Update::SignalPath(Some(Box::new(path.clone()))))
        .unwrap();
    spin(100);
    assert_eq!(window.toast_text().as_deref(), Some("Something else"));
    // Another file for the rate that cannot be read either: told again.
    services
        .update_settings(&mut |s| {
            s.dsp.convolution.files[0].path = PathBuf::from("/sample/room-96k-broken-2.wav");
        })
        .unwrap();
    updates
        .try_send(Update::SignalPath(Some(Box::new(path.clone()))))
        .unwrap();
    spin(100);
    assert_eq!(window.toast_text().as_deref(), Some(text));
    // "Details…" opens the convolution stage.
    let details = button(&window, "Details…");
    details.emit_clicked();
    spin(200);
    assert_eq!(window.toast_text(), None);
    assert_eq!(window.settings_page().as_deref(), Some("dsp"));
    let settings = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| has_named(w, "dsp-editors"))
        .unwrap();
    let stack: gtk::Stack = named(&settings, "dsp-editors");
    assert_eq!(stack.visible_child_name().as_deref(), Some("convolution"));
    settings.close();
    // Fixed: a toast still up goes.
    services
        .update_settings(&mut |s| {
            s.dsp.convolution.files[0].path = PathBuf::from("/sample/room-96k-broken-3.wav");
        })
        .unwrap();
    updates
        .try_send(Update::SignalPath(Some(Box::new(path.clone()))))
        .unwrap();
    spin(100);
    assert_eq!(window.toast_text().as_deref(), Some(text));
    for stage in &mut path.stages {
        if stage.kind == StageKind::Convolution {
            stage.state = StageState::Active;
            stage.detail = "convolution mono · 32768 taps · latency 10.7 ms".into();
        }
    }
    updates
        .try_send(Update::SignalPath(Some(Box::new(path))))
        .unwrap();
    spin(100);
    assert_eq!(window.toast_text(), None);
    window.destroy();
}

type Test = fn(&gtk::Application);

const TESTS: &[(&str, Test)] = &[
    (
        "the_chain_moves_and_numbers_its_stages",
        the_chain_moves_and_numbers_its_stages,
    ),
    (
        "saved_chains_save_apply_and_delete",
        saved_chains_save_apply_and_delete,
    ),
    ("the_compressor_page", the_compressor_page),
    ("the_convolution_page", the_convolution_page),
    ("the_pitch_page", the_pitch_page),
    (
        "a_track_with_its_own_equalizer",
        a_track_with_its_own_equalizer,
    ),
    (
        "the_signal_path_follows_the_chain",
        the_signal_path_follows_the_chain,
    ),
    (
        "the_page_follows_changes_made_elsewhere",
        the_page_follows_changes_made_elsewhere,
    ),
    (
        "the_convolution_toast_shows_once",
        the_convolution_toast_shows_once,
    ),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("dsp: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("dsp: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.DspTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let f = failures.clone();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        // Arguments after `--` keep the tests whose names contain one of them.
        let filters: Vec<String> = std::env::args()
            .skip(1)
            .filter(|a| !a.starts_with('-'))
            .collect();
        for (name, test) in TESTS {
            if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
                continue;
            }
            let result = catch_unwind(AssertUnwindSafe(|| test(application)));
            match result {
                Ok(()) => println!("test {name} ... ok"),
                Err(_) => {
                    println!("test {name} ... FAILED");
                    f.borrow_mut().push(*name);
                }
            }
            for window in gtk::Window::list_toplevels() {
                if let Ok(window) = window.downcast::<gtk::Window>() {
                    window.destroy();
                }
            }
            spin(20);
        }
        application.quit();
    });
    application.run_with_args(&["dsp"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("dsp: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("dsp: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
