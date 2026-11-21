// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The smarter playback groups of Settings › Playback: Shuffle (what it
//! favours and what it skips), Intro scan, and When the queue ends with its two
//! cards, Continue with related tracks (with "How alike" and the user's own
//! weights) and Auto-DJ. They write `[playback.shuffle]`,
//! `playback.intro_scan_s`, `[playback.endless]` and `[playback.auto_dj]`,
//! which the player follows at once.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::settings::{PlayWeight, RelatedWeights, SeedKind};

use super::Context;
use super::playback::{seconds, value_dropdown};
use crate::backend::services::{AnalysisProgress, Settings};
use crate::dialogs::common::{bind_segmented, dropdown, note, on_selected, segmented};
use crate::i18n::{fill, ntr, tr};
use crate::popovers::query_completion::QueryCompletion;
use crate::related::{Alike, all_zero};
use crate::widgets::label;

/// The widget name of the "When the queue ends" row, which the queue panel's
/// "Change…" opens.
pub(crate) use crate::window::panel::queue_end::SETTINGS_ROW;

/// How long typing pauses before a query is read and stored, in ms.
const QUERY_DELAY_MS: u64 = 400;

/// The lengths intro scan offers, in seconds.
const INTRO_SCAN_S: [u32; 8] = [3, 5, 10, 15, 20, 30, 60, 120];

/// The rating floors offered, in half stars: none, then 1 to 5 stars.
const MIN_RATINGS: [u8; 10] = [0, 2, 3, 4, 5, 6, 7, 8, 9, 10];

/// What a weighted pick can favour, in the order of the check boxes.
const FAVOURS: [PlayWeight; 3] = [
    PlayWeight::Rating,
    PlayWeight::NotRecentlyPlayed,
    PlayWeight::LessPlayed,
];

/// Changes the settings and shows the queue panel again, whose block words
/// them; the result says whether the change was made.
fn apply(context: &Rc<Context>, change: impl FnMut(&mut Settings)) -> bool {
    let done = context.update(change);
    if done && let Some(main) = context.main.upgrade() {
        main.refresh_queue_panel();
    }
    done
}

/// A rating floor in words: "No limit", "1 star", "1½ stars".
fn rating_floor(half_stars: u8) -> String {
    let whole = half_stars / 2;
    match (half_stars, half_stars % 2) {
        (0, _) => tr("No limit"),
        (1, _) => tr("½ star"),
        (_, 0) => fill(
            &ntr("{} star", "{} stars", usize::from(whole)),
            &[&whole.to_string()],
        ),
        _ => fill(&tr("{}½ stars"), &[&whole.to_string()]),
    }
}

/// The name of a favour check box.
fn favour_name(weight: PlayWeight) -> String {
    match weight {
        PlayWeight::Rating => tr("Higher rated tracks"),
        PlayWeight::NotRecentlyPlayed => tr("Tracks not played for a while"),
        PlayWeight::LessPlayed => tr("Tracks played less often"),
    }
}

/// The three favour check boxes, ticked as `current` says; `set` stores the
/// ticked ones, in their order. In a row that wraps where there is room for
/// all three (`wrapping`), otherwise two to a line, as drawn in the card.
fn favour_boxes(
    context: &Rc<Context>,
    current: &[PlayWeight],
    name: &str,
    wrapping: bool,
    set: fn(&mut Settings, Vec<PlayWeight>),
) -> gtk::Widget {
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(false);
    flow.set_column_spacing(22);
    flow.set_row_spacing(8);
    flow.set_max_children_per_line(3);
    flow.add_css_class("hk-favour");
    let grid = gtk::Grid::new();
    grid.set_column_spacing(22);
    grid.set_row_spacing(6);
    grid.set_valign(gtk::Align::Center);
    let root: gtk::Widget = if wrapping {
        flow.clone().upcast()
    } else {
        grid.clone().upcast()
    };
    root.set_hexpand(true);
    // The boxes keep their own widths, rather than spreading over the line.
    flow.set_halign(gtk::Align::Start);
    root.set_widget_name(name);
    let checks: Vec<(PlayWeight, gtk::CheckButton)> = FAVOURS
        .iter()
        .map(|&w| {
            let check = gtk::CheckButton::with_label(&favour_name(w));
            check.set_active(current.contains(&w));
            (w, check)
        })
        .collect();
    for (i, (_, check)) in checks.iter().enumerate() {
        if wrapping {
            flow.insert(check, -1);
            if let Some(child) = check.parent() {
                child.set_focusable(false);
            }
        } else {
            grid.attach(check, (i % 2) as i32, (i / 2) as i32, 1, 1);
        }
    }
    let all: Rc<Vec<(PlayWeight, glib::WeakRef<gtk::CheckButton>)>> =
        Rc::new(checks.iter().map(|(w, c)| (*w, c.downgrade())).collect());
    for (_, check) in &checks {
        let weak = Rc::downgrade(context);
        let all = all.clone();
        check.connect_toggled(move |_| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let ticked: Vec<PlayWeight> = all
                .iter()
                .filter(|(_, c)| c.upgrade().is_some_and(|c| c.is_active()))
                .map(|(w, _)| *w)
                .collect();
            apply(&context, |s| set(s, ticked.clone()));
        });
    }
    root
}

/// A number entry from `min` to `max`: arrow keys and the scroll wheel step
/// it; a value out of range goes to the nearest allowed one.
fn number(
    context: &Rc<Context>,
    range: (u32, u32),
    current: u32,
    width: i32,
    name: &str,
    set: fn(&mut Settings, u32),
) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(f64::from(range.0), f64::from(range.1), 1.0);
    spin.set_digits(0);
    spin.set_numeric(true);
    spin.set_update_policy(gtk::SpinButtonUpdatePolicy::Always);
    spin.set_value(f64::from(current));
    spin.set_size_request(width, -1);
    spin.set_width_chars(1);
    spin.set_valign(gtk::Align::Center);
    EditableExt::set_alignment(&spin, 1.0);
    spin.add_css_class("hk-number");
    spin.set_widget_name(name);
    // A plain entry, as drawn: the arrow keys and the wheel step it.
    let mut child = spin.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if widget.is::<gtk::Button>() {
            widget.set_visible(false);
        }
    }
    let weak = Rc::downgrade(context);
    spin.connect_value_changed(move |spin| {
        if let Some(context) = weak.upgrade() {
            let value = spin.value().round().max(0.0) as u32;
            apply(&context, |s| set(s, value));
        }
    });
    spin
}

/// A query entry in small: the search entry's query font and completion, and
/// under it the reason a query cannot be read, which keeps the last one that
/// could.
fn query_entry(
    context: &Rc<Context>,
    current: &str,
    accessible: &str,
    name: &str,
    set: fn(&mut Settings, String),
) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.set_hexpand(true);
    let entry = gtk::SearchEntry::new();
    entry.add_css_class("hk-input");
    entry.add_css_class("hk-small-query");
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some(&tr("A search query, for example genre:Christmas")));
    entry.update_property(&[gtk::accessible::Property::Label(accessible)]);
    entry.set_widget_name(name);
    entry.set_text(current);
    crate::query_text::mark(&entry, &crate::query_text::read(current));
    column.append(&entry);
    let problem = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    problem.add_css_class("hk-query-problem");
    problem.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
    let message = label("", &[]);
    message.set_ellipsize(gtk::pango::EllipsizeMode::None);
    message.set_wrap(true);
    message.set_single_line_mode(false);
    message.set_hexpand(true);
    problem.append(&message);
    problem.set_visible(false);
    column.append(&problem);
    let completion = QueryCompletion::new(&entry, &context.backend);
    let weak = Rc::downgrade(&completion);
    entry.connect_changed(move |entry| {
        crate::query_text::mark(entry, &crate::query_text::read(&entry.text()));
        if let Some(completion) = weak.upgrade() {
            completion.update();
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&completion);
    keys.connect_key_pressed(move |_, key, _, _| match weak.upgrade() {
        Some(completion) if completion.key(key) => glib::Propagation::Stop,
        _ => glib::Propagation::Proceed,
    });
    entry.add_controller(keys);
    // The completion lives as long as its entry.
    let keep = std::cell::RefCell::new(Some(completion));
    entry.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    // Stored once typing pauses; a query that cannot be read is not.
    let weak = Rc::downgrade(context);
    let (weak_problem, weak_message) = (problem.downgrade(), message.downgrade());
    let typed = Rc::new(Cell::new(0u64));
    entry.connect_changed(move |entry| {
        let generation = typed.get() + 1;
        typed.set(generation);
        let (weak, weak_problem, weak_message) =
            (weak.clone(), weak_problem.clone(), weak_message.clone());
        let (typed, weak_entry) = (typed.clone(), entry.downgrade());
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(QUERY_DELAY_MS),
            move || {
                let (Some(context), Some(entry)) = (weak.upgrade(), weak_entry.upgrade()) else {
                    return;
                };
                if typed.get() != generation {
                    return;
                }
                let text = entry.text().trim().to_owned();
                let reading = crate::query_text::read(&text);
                let error = reading.error.as_ref().map(crate::query_text::message);
                if let (Some(problem), Some(message)) =
                    (weak_problem.upgrade(), weak_message.upgrade())
                {
                    problem.set_visible(error.is_some());
                    message.set_text(error.as_deref().unwrap_or_default());
                }
                if error.is_none() {
                    apply(&context, |s| set(s, text.clone()));
                }
            },
        );
    });
    // Escape in the entry clears nothing: it closes the completion.
    entry.connect_stop_search(|_| {});
    column
}

/// The Shuffle group: Favour, Skip, tracks that match, and its note.
pub(super) fn shuffle(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let shuffle = &settings.playback.shuffle;
    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let lead = |text: &str| {
        let words = label(text, &["hk-t2"]);
        words.set_size_request(52, -1);
        words.set_valign(gtk::Align::Start);
        words
    };
    let favour = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let favour_words = lead(&tr("Favour"));
    favour.append(&favour_words);
    let boxes = favour_boxes(
        context,
        &shuffle.weight_by,
        "shuffle-favour",
        true,
        |s, w| {
            s.playback.shuffle.weight_by = w;
        },
    );
    favour.append(&boxes);
    column.append(&favour);

    let skip = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    skip.append(&lead(&tr("Skip")));
    let below = label(&tr("tracks rated below"), &[]);
    skip.append(&below);
    let floor = value_dropdown(
        context,
        &MIN_RATINGS,
        shuffle.min_rating,
        rating_floor,
        132,
        |s, v| s.playback.shuffle.min_rating = v,
    );
    floor.set_widget_name("shuffle-min-rating");
    floor.add_css_class(crate::dialogs::common::OWN_LABEL);
    floor.update_property(&[gtk::accessible::Property::Label(&tr(
        "Skip tracks rated below",
    ))]);
    skip.append(&floor);
    column.append(&skip);

    let matching = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    matching.append(&lead(""));
    let words = label(&tr("tracks that match"), &[]);
    words.set_valign(gtk::Align::Start);
    words.set_margin_top(6);
    matching.append(&words);
    matching.append(&query_entry(
        context,
        &shuffle.skip_query,
        &tr("Skip tracks that match this search query"),
        "shuffle-skip-query",
        |s, q| s.playback.shuffle.skip_query = q,
    ));
    column.append(&matching);
    column.append(&note(&tr(
        "Every track still plays once in each round; the favoured ones come earlier. Tracks without a rating are never skipped for their rating.",
    )));
    column
}

/// Intro scan: "play the first [10 seconds] of each track".
pub(super) fn intro_scan(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let length = value_dropdown(
        context,
        &INTRO_SCAN_S,
        context.settings().playback.intro_scan_s,
        seconds,
        124,
        |s, v| s.playback.intro_scan_s = v,
    );
    length.set_widget_name("intro-scan");
    length.add_css_class(crate::dialogs::common::OWN_LABEL);
    length.update_property(&[gtk::accessible::Property::Label(&tr(
        "Seconds of each track that intro scan plays",
    ))]);
    crate::widgets::sentence(
        &line,
        // Translators: intro scan. {} is a dropdown with a time ("10 seconds").
        &tr("play the first {} of each track"),
        &[length.upcast_ref()],
        &["hk-t2"],
    );
    crate::dialogs::common::with_note(
        &line,
        &tr("Used while Queue › Intro Scan is ticked in the Playback menu."),
    )
}

/// A card: a head with its title over a line, then its rows.
fn card(title: &str, line: &str, name: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("hk-card");
    card.set_widget_name(name);
    card.set_overflow(gtk::Overflow::Hidden);
    let head = gtk::Box::new(gtk::Orientation::Vertical, 1);
    head.add_css_class("head");
    head.set_valign(gtk::Align::Center);
    let heading = label(title, &["hk-card-title"]);
    heading.set_ellipsize(gtk::pango::EllipsizeMode::None);
    heading.set_wrap(true);
    heading.set_single_line_mode(false);
    head.append(&heading);
    head.append(&note(line));
    card.append(&head);
    card.update_property(&[gtk::accessible::Property::Label(title)]);
    card
}

/// A row of a card: its 150 px label in `t2`, then `controls`.
fn card_row(title: &str, controls: &[&gtk::Widget]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("row");
    let name = label(title, &["label"]);
    name.set_ellipsize(gtk::pango::EllipsizeMode::None);
    name.set_wrap(true);
    name.set_single_line_mode(false);
    name.set_max_width_chars(18);
    name.set_size_request(150, -1);
    name.set_valign(gtk::Align::Center);
    row.append(&name);
    for control in controls {
        if let Some(dropdown) = control.downcast_ref::<gtk::DropDown>() {
            dropdown.add_css_class(crate::dialogs::common::OWN_LABEL);
            dropdown
                .update_relation(&[gtk::accessible::Relation::LabelledBy(&[name.upcast_ref()])]);
        }
        row.append(*control);
    }
    row
}

/// "Don’t repeat the last [N] tracks", after the first number of a row.
fn window_words(
    context: &Rc<Context>,
    current: u32,
    name: &str,
    set: fn(&mut Settings, u32),
) -> (gtk::Label, gtk::SpinButton, gtk::Label) {
    let before = label(&tr("Don’t repeat the last"), &["hk-t2"]);
    before.set_ellipsize(gtk::pango::EllipsizeMode::None);
    before.set_margin_start(4);
    let window = number(context, (0, 10_000), current, 64, name, set);
    window.update_property(&[gtk::accessible::Property::Label(&tr(
        "Recently played tracks that are not added again",
    ))]);
    let after = label(&tr("tracks"), &["hk-t2"]);
    (before, window, after)
}

/// The "When the queue ends" group: the two cards.
pub(super) fn queue_ends(context: &Rc<Context>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.append(&related_card(context));
    column.append(&auto_dj_card(context));
    column
}

/// The explanation of "When the queue ends", under its label.
pub(super) fn queue_ends_note() -> String {
    tr(
        "How the two Repeat modes that keep the music going choose their tracks. The mode itself is chosen in the Repeat menu.",
    )
}

/// The card "Continue with related tracks".
fn related_card(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let endless = &settings.playback.endless;
    let card = card(
        &tr("Continue with related tracks"),
        &tr("When the queue runs out, tracks like the last ones are added to it."),
        "related-card",
    );

    let (seed, buttons) = segmented(&[tr("The last track"), tr("Its album"), tr("Its artist")]);
    seed.set_widget_name("related-to");
    seed.update_property(&[gtk::accessible::Property::Label(&tr(
        "What the added tracks are related to",
    ))]);
    let kinds = [SeedKind::Track, SeedKind::Album, SeedKind::Artist];
    let weak = Rc::downgrade(context);
    bind_segmented(
        &buttons,
        kinds.iter().position(|k| *k == endless.seed).unwrap_or(0),
        move |i| {
            if let Some(context) = weak.upgrade() {
                apply(&context, |s| s.playback.endless.seed = kinds[i]);
            }
        },
    );
    card.append(&card_row(&tr("Related to"), &[seed.upcast_ref()]));

    // How alike, with the line beside it and the weights under it.
    let current = Alike::of(&endless.weights);
    let names: Vec<String> = Alike::ALL.iter().map(|a| a.name()).collect();
    let index = Alike::ALL.iter().position(|a| *a == current).unwrap_or(0);
    let alike = dropdown(&names, index, 176);
    alike.set_widget_name("how-alike");
    let beside = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    beside.set_hexpand(true);
    let warning = gtk::Image::from_icon_name("dialog-warning-symbolic");
    warning.set_pixel_size(12);
    beside.append(&warning);
    let explanation = note("");
    explanation.set_hexpand(true);
    explanation.set_widget_name("how-alike-note");
    beside.append(&explanation);
    let show_explanation = {
        let (warning, explanation) = (warning.downgrade(), explanation.downgrade());
        move |alike: Alike, weights: &RelatedWeights| {
            let (Some(warning), Some(explanation)) = (warning.upgrade(), explanation.upgrade())
            else {
                return;
            };
            let empty = alike == Alike::Own && all_zero(weights);
            warning.set_visible(empty);
            explanation.set_text(&if empty {
                tr("Set at least one likeness above 0.")
            } else {
                alike.explanation()
            });
        }
    };
    show_explanation(current, &endless.weights);
    card.append(&card_row(
        &tr("How alike"),
        &[alike.upcast_ref(), beside.upcast_ref()],
    ));
    let weights = weights_block(context, &endless.weights, endless.year_range, {
        let show = show_explanation.clone();
        move |w: &RelatedWeights| show(Alike::Own, w)
    });
    weights.root.set_visible(current == Alike::Own);
    card.append(&weights.root);
    let weak = Rc::downgrade(context);
    let weak_block = weights.root.downgrade();
    let sliders = weights.sliders.clone();
    let quiet = weights.quiet.clone();
    on_selected(&alike, move |i| {
        let (Some(context), Some(&choice)) = (weak.upgrade(), Alike::ALL.get(i)) else {
            return;
        };
        let before = context.settings().playback.endless.weights;
        let now = match choice.weights() {
            Some(named) => {
                if Alike::of(&before) == Alike::Own {
                    Alike::keep_own(before);
                }
                named
            }
            None => Alike::own_or(before),
        };
        apply(&context, |s| s.playback.endless.weights = now);
        if let Some(block) = weak_block.upgrade() {
            block.set_visible(choice == Alike::Own);
        }
        set_sliders(&sliders, &quiet, &now);
        show_explanation(choice, &now);
    });

    let batch = number(
        context,
        (1, 100),
        endless.batch,
        56,
        "related-batch",
        |s, v| s.playback.endless.batch = v,
    );
    batch.update_property(&[gtk::accessible::Property::Label(&tr(
        "Tracks added at a time",
    ))]);
    let (before, window, after) =
        window_words(context, endless.window, "related-window", |s, v| {
            s.playback.endless.window = v;
        });
    card.append(&card_row(
        &tr("Tracks added at a time"),
        &[
            batch.upcast_ref(),
            before.upcast_ref(),
            window.upcast_ref(),
            after.upcast_ref(),
        ],
    ));
    let filter = query_entry(
        context,
        &endless.filter,
        &tr("Only add tracks that match this search query"),
        "related-filter",
        |s, q| s.playback.endless.filter = q,
    );
    card.append(&card_row(
        &tr("Only tracks that match"),
        &[filter.upcast_ref()],
    ));
    card
}

/// The sliders of the weights, with how each is read and set.
type WeightSlider = (gtk::Scale, gtk::Label, fn(&RelatedWeights) -> u8);

/// The block of the user's own weights.
struct Weights {
    root: gtk::Box,
    sliders: Rc<Vec<WeightSlider>>,
    /// Set while the sliders show weights that are not the user's moves.
    quiet: Rc<Cell<bool>>,
}

/// Shows `weights` on `sliders` without storing them again.
fn set_sliders(sliders: &[WeightSlider], quiet: &Cell<bool>, weights: &RelatedWeights) {
    quiet.set(true);
    for (scale, value, get) in sliders {
        let v = get(weights);
        value.set_text(&v.to_string());
        scale.set_value(f64::from(v));
    }
    quiet.set(false);
}

/// The seven sliders of "Your own weights", in two columns, and the note on
/// how many tracks are analysed; `changed` follows each change.
fn weights_block(
    context: &Rc<Context>,
    current: &RelatedWeights,
    year_range: u32,
    changed: impl Fn(&RelatedWeights) + 'static,
) -> Weights {
    type Weight = (
        String,
        fn(&RelatedWeights) -> u8,
        fn(&mut RelatedWeights, u8),
    );
    let rows: [Weight; 7] = [
        (tr("Same artist"), |w| w.artist, |w, v| w.artist = v),
        (
            tr("Same album artist"),
            |w| w.album_artist,
            |w, v| w.album_artist = v,
        ),
        (tr("Same genre"), |w| w.genre, |w, v| w.genre = v),
        (tr("Years close together"), |w| w.year, |w, v| w.year = v),
        (tr("Shared labels"), |w| w.labels, |w, v| w.labels = v),
        (
            tr("Similar play counts and ratings"),
            |w| w.statistics,
            |w, v| w.statistics = v,
        ),
        (tr("Sounds alike"), |w| w.sound, |w, v| w.sound = v),
    ];
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("weights");
    root.set_widget_name("related-weights");
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(32);
    grid.set_column_homogeneous(true);
    root.append(&grid);
    let changed = Rc::new(changed);
    let quiet = Rc::new(Cell::new(false));
    let mut sliders: Vec<WeightSlider> = Vec::new();
    for (i, (title, get, set)) in rows.into_iter().enumerate() {
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell.add_css_class("hk-weight");
        cell.set_hexpand(true);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        head.set_size_request(-1, 26);
        let name = label(&title, &[]);
        name.set_hexpand(true);
        head.append(&name);
        if i == 3 {
            let within = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            within.add_css_class("hk-note");
            within.append(&label(&tr("within"), &["hk-t3"]));
            let years = number(context, (0, 50), year_range, 44, "related-years", |s, v| {
                s.playback.endless.year_range = v;
            });
            years.update_property(&[gtk::accessible::Property::Label(&tr(
                "Years either side that count as close",
            ))]);
            within.append(&years);
            within.append(&label(&tr("years"), &["hk-t3"]));
            head.append(&within);
        }
        let value = label(&get(current).to_string(), &["value"]);
        head.append(&value);
        cell.append(&head);
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 10.0, 1.0);
        scale.add_css_class("hk-slider");
        scale.set_draw_value(false);
        scale.set_round_digits(0);
        scale.set_value(f64::from(get(current)));
        scale.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}, 0 to 10"),
            &[&title],
        ))]);
        let weak = Rc::downgrade(context);
        let weak_value = value.downgrade();
        let changed = changed.clone();
        let still = quiet.clone();
        scale.connect_value_changed(move |scale| {
            let v = scale.value().round().clamp(0.0, 10.0) as u8;
            if let Some(value) = weak_value.upgrade() {
                value.set_text(&v.to_string());
            }
            if still.get() {
                return;
            }
            let Some(context) = weak.upgrade() else {
                return;
            };
            let mut weights = context.settings().playback.endless.weights;
            if get(&weights) == v {
                return;
            }
            set(&mut weights, v);
            if apply(&context, |s| s.playback.endless.weights = weights) {
                changed(&weights);
            }
        });
        cell.append(&scale);
        grid.attach(&cell, (i % 2) as i32, (i / 2) as i32, 1, 1);
        sliders.push((scale, value, get));
    }
    let analysed = note(&sound_note(None));
    analysed.set_widget_name("sound-note");
    analysed.set_valign(gtk::Align::Center);
    grid.attach(&analysed, 1, 3, 1, 1);
    let services = context.services.clone();
    let weak_note = analysed.downgrade();
    glib::spawn_future_local(async move {
        let progress = gio::spawn_blocking(move || services.analysis_progress())
            .await
            .ok()
            .flatten();
        if let (Some(note), Some(progress)) = (weak_note.upgrade(), progress) {
            note.set_text(&sound_note(Some(&progress)));
        }
    });
    Weights {
        root,
        sliders: Rc::new(sliders),
        quiet,
    }
}

/// The note on the sound likeness: how many tracks are analysed, left out
/// once all are.
fn sound_note(progress: Option<&AnalysisProgress>) -> String {
    match progress {
        Some(p) if p.total > 0 && p.analyzed < p.total => fill(
            &ntr(
                // Translators: {0} is the number of tracks analysed, {1} the
                // number of all tracks, which chooses the form.
                "Sounds alike uses the track analysis, so it works for tracks that are analysed: {} of {} so far. At 0 a likeness is not used.",
                "Sounds alike uses the track analysis, so it works for tracks that are analysed: {} of {} so far. At 0 a likeness is not used.",
                p.total,
            ),
            &[
                &crate::format::count(p.analyzed),
                &crate::format::count(p.total),
            ],
        ),
        _ => tr(
            "Sounds alike uses the track analysis, so it works for tracks that are analysed. At 0 a likeness is not used.",
        ),
    }
}

/// The card "Auto-DJ".
fn auto_dj_card(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let auto_dj = &settings.playback.auto_dj;
    let card = card(
        &tr("Auto-DJ"),
        &tr(
            "Keeps tracks from the library queued after the playing one, so the music never stops.",
        ),
        "auto-dj-card",
    );
    let ahead = number(
        context,
        (1, 50),
        auto_dj.ahead,
        56,
        "auto-dj-ahead",
        |s, v| {
            s.playback.auto_dj.ahead = v;
        },
    );
    ahead.update_property(&[gtk::accessible::Property::Label(&tr(
        "Tracks kept queued ahead",
    ))]);
    let (before, window, after) =
        window_words(context, auto_dj.window, "auto-dj-window", |s, v| {
            s.playback.auto_dj.window = v;
        });
    card.append(&card_row(
        &tr("Tracks kept ahead"),
        &[
            ahead.upcast_ref(),
            before.upcast_ref(),
            window.upcast_ref(),
            after.upcast_ref(),
        ],
    ));
    let favour = favour_boxes(
        context,
        &auto_dj.weight_by,
        "auto-dj-favour",
        false,
        |s, w| {
            s.playback.auto_dj.weight_by = w;
        },
    );
    card.append(&card_row(&tr("Favour"), &[&favour]));
    let floor = value_dropdown(
        context,
        &MIN_RATINGS,
        auto_dj.min_rating,
        rating_floor,
        132,
        |s, v| s.playback.auto_dj.min_rating = v,
    );
    floor.set_widget_name("auto-dj-min-rating");
    floor.add_css_class(crate::dialogs::common::OWN_LABEL);
    floor.set_halign(gtk::Align::Start);
    card.append(&card_row(
        &tr("Skip tracks rated below"),
        &[floor.upcast_ref()],
    ));
    let filter = query_entry(
        context,
        &auto_dj.filter,
        &tr("Only queue tracks that match this search query"),
        "auto-dj-filter",
        |s, q| s.playback.auto_dj.filter = q,
    );
    card.append(&card_row(
        &tr("Only tracks that match"),
        &[filter.upcast_ref()],
    ));
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_floors_read_as_drawn() {
        let texts: Vec<String> = MIN_RATINGS.iter().map(|&r| rating_floor(r)).collect();
        assert_eq!(
            texts,
            [
                "No limit",
                "1 star",
                "1½ stars",
                "2 stars",
                "2½ stars",
                "3 stars",
                "3½ stars",
                "4 stars",
                "4½ stars",
                "5 stars"
            ]
        );
    }

    #[test]
    fn the_sound_note_counts_until_every_track_is_analysed() {
        let mut progress = AnalysisProgress {
            total: 18_402,
            analyzed: 14_950,
            failed: 0,
            state: crate::backend::services::AnalysisState::Yielding,
        };
        assert_eq!(
            sound_note(Some(&progress)),
            "Sounds alike uses the track analysis, so it works for tracks that are analysed: 14,950 of 18,402 so far. At 0 a likeness is not used."
        );
        progress.analyzed = progress.total;
        assert_eq!(sound_note(Some(&progress)), sound_note(None));
    }
}
