// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The chain of the DSP page: its own row ("Chain", a line on how to use it,
//! "Saved chains" and "Default order") and eight stage boxes in two rows of
//! four, numbered in the order the sound passes them.
//!
//! A stage moves by dragging its box (a 3 px accent bar marks where it would
//! go), with Alt+Left and Alt+Right while its name has the focus, or from its
//! context menu. Dither is always last: it cannot be moved and takes no drop
//! after it. A new order applies at once.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use helikodu_model::settings::{ChainStage, DspSettings, SavedChain};

use super::{Facts, Live, Stage};
use crate::dialogs::common::compact_button;
use crate::i18n::{fill, tr};
use crate::widgets::label;

/// A stage dragged in the chain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "HkDraggedStage")]
struct DraggedStage(usize);

/// Places in the chain: seven stages, then Dither.
const PLACES: usize = 8;
/// Boxes in a row.
const COLUMNS: usize = 4;

/// One stage box.
struct StageBox {
    stage: Stage,
    /// The grid cell: the box, with the insertion bar at its start.
    cell: gtk::Overlay,
    root: gtk::Box,
    button: gtk::ToggleButton,
    place: gtk::Label,
    summary: gtk::Label,
    warning: gtk::Image,
    switch: gtk::Switch,
    bar: gtk::Box,
}

/// The Dither box.
struct DitherBox {
    cell: gtk::Overlay,
    bar: gtk::Box,
}

/// A joint between two boxes of a row, with the insertion bar in its middle.
struct Joint {
    cell: gtk::Overlay,
    bar: gtk::Box,
}

/// What a stage's name does when activated.
type SelectFn = Box<dyn Fn(Stage)>;

/// The chain.
pub(super) struct Chain {
    live: Rc<Live>,
    root: gtk::Box,
    grid: gtk::Grid,
    boxes: Vec<StageBox>,
    dither: DitherBox,
    joints: Vec<Joint>,
    default_order: gtk::Button,
    saved: SavedChains,
    /// The order shown.
    order: RefCell<Vec<Stage>>,
    selected: Cell<Stage>,
    on_select: RefCell<Option<SelectFn>>,
    /// The stage whose context menu is open.
    menu_stage: Cell<Stage>,
    actions: gio::SimpleActionGroup,
}

/// A bar of the insertion mark, hidden until a drag needs it.
fn insertion_bar() -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bar.add_css_class("hk-stage-drop");
    bar.set_size_request(3, -1);
    bar.set_valign(gtk::Align::Fill);
    bar.set_can_target(false);
    bar.set_visible(false);
    bar
}

fn stage_box(stage: Stage) -> StageBox {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("hk-stage-box");
    root.add_css_class("hk-stage");
    root.set_hexpand(true);
    let overlay = gtk::Overlay::new();
    let button = gtk::ToggleButton::new();
    button.add_css_class("hk-stage-select");
    button.set_widget_name(&format!("stage-{}", stage.id()));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let place = label("", &["place"]);
    place.set_widget_name(&format!("stage-place-{}", stage.id()));
    place.set_valign(gtk::Align::Baseline);
    head.append(&place);
    let name = label(&stage.title(), &["hk-stage-name", "name"]);
    name.set_ellipsize(gtk::pango::EllipsizeMode::None);
    name.set_valign(gtk::Align::Baseline);
    // Room for the switch at the right.
    name.set_margin_end(40);
    head.append(&name);
    text.append(&head);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let warning = gtk::Image::from_icon_name("helikodu-warning-symbolic");
    warning.set_pixel_size(12);
    warning.add_css_class("hk-t2");
    warning.set_visible(false);
    line.append(&warning);
    let summary = label("", &["summary"]);
    summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
    summary.set_widget_name(&format!("stage-summary-{}", stage.id()));
    line.append(&summary);
    text.append(&line);
    button.set_child(Some(&text));
    overlay.set_child(Some(&button));
    let switch = gtk::Switch::new();
    switch.add_css_class("hk-switch");
    switch.set_halign(gtk::Align::End);
    switch.set_valign(gtk::Align::Start);
    switch.set_margin_top(7);
    switch.set_margin_end(10);
    switch.set_widget_name(&format!("stage-switch-{}", stage.id()));
    switch.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("{} on"),
        &[&stage.title()],
    ))]);
    overlay.add_overlay(&switch);
    root.append(&overlay);
    let cell = gtk::Overlay::new();
    cell.set_child(Some(&root));
    let bar = insertion_bar();
    bar.set_halign(gtk::Align::Start);
    cell.add_overlay(&bar);
    StageBox {
        stage,
        cell,
        root,
        button,
        place,
        summary,
        warning,
        switch,
        bar,
    }
}

/// The Dither box: always on, automatic, last, without an editor.
fn dither_box() -> DitherBox {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 3);
    root.add_css_class("hk-stage-box");
    root.add_css_class("hk-stage");
    root.add_css_class("on");
    root.add_css_class("fixed");
    root.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = label(&tr("Dither"), &["hk-stage-name", "name", "hk-strong"]);
    name.set_hexpand(true);
    head.append(&name);
    head.append(&label(&tr("Auto"), &["hk-t3", "hk-secondary"]));
    root.append(&head);
    root.append(&label("TPDF", &["summary"]));
    root.update_property(&[gtk::accessible::Property::Label(&tr(
        "Dither: automatic, TPDF, always last",
    ))]);
    let cell = gtk::Overlay::new();
    cell.set_child(Some(&root));
    let bar = insertion_bar();
    bar.set_halign(gtk::Align::Start);
    cell.add_overlay(&bar);
    DitherBox { cell, bar }
}

fn joint() -> Joint {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.add_css_class("hk-stage-joint");
    line.set_size_request(10, 1);
    line.set_valign(gtk::Align::Center);
    let cell = gtk::Overlay::new();
    cell.set_child(Some(&line));
    let bar = insertion_bar();
    bar.set_halign(gtk::Align::Center);
    cell.add_overlay(&bar);
    Joint { cell, bar }
}

/// The order of `dsp` as stages.
fn order_of(dsp: &DspSettings) -> Vec<Stage> {
    dsp.effective_order().into_iter().map(Stage::of).collect()
}

/// The order with `stage` moved to be before the stage at `place` (7: at the
/// end); `None` when that leaves it where it is.
pub(super) fn moved(order: &[Stage], stage: Stage, place: usize) -> Option<Vec<Stage>> {
    let from = order.iter().position(|s| *s == stage)?;
    let to = if place > from { place - 1 } else { place };
    let to = to.min(order.len() - 1);
    if to == from {
        return None;
    }
    let mut order = order.to_vec();
    order.remove(from);
    order.insert(to, stage);
    Some(order)
}

/// "Compressor, stage 6 of 7, off".
fn spoken(stage: Stage, place: usize, on: bool) -> String {
    let state = if on { tr("on") } else { tr("off") };
    fill(
        &tr("{}, stage {} of {}, {}"),
        &[
            &stage.title(),
            &(place + 1).to_string(),
            &Stage::ALL.len().to_string(),
            &state,
        ],
    )
}

impl Chain {
    pub(super) fn new(live: &Rc<Live>, dsp: &DspSettings) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("hk-dsp-chain");

        // The chain's own row.
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        head.set_size_request(-1, 30);
        let title = label(&tr("Chain"), &["hk-medium"]);
        title.set_valign(gtk::Align::Center);
        head.append(&title);
        let how = gtk::Label::new(Some(&tr(
            "The sound passes the stages in this order. Drag a stage to move it.",
        )));
        how.set_xalign(0.0);
        how.set_wrap(true);
        how.set_hexpand(true);
        how.set_valign(gtk::Align::Center);
        how.add_css_class("hk-note");
        head.append(&how);
        let saved = SavedChains::new();
        head.append(&saved.button);
        let default_order = compact_button(&tr("Default order"));
        default_order.set_widget_name("chain-default-order");
        head.append(&default_order);
        root.append(&head);

        let grid = gtk::Grid::new();
        grid.set_row_spacing(6);
        grid.set_column_homogeneous(false);
        grid.add_css_class("hk-stage-chain");
        grid.update_property(&[gtk::accessible::Property::Label(&tr("Processing chain"))]);
        root.append(&grid);

        let boxes: Vec<StageBox> = Stage::ALL.iter().map(|s| stage_box(*s)).collect();
        let dither = dither_box();
        // Every box as wide as the others.
        let widths = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        for b in &boxes {
            widths.add_widget(&b.cell);
        }
        widths.add_widget(&dither.cell);
        let joints: Vec<Joint> = (0..(COLUMNS - 1) * 2).map(|_| joint()).collect();
        for (i, joint) in joints.iter().enumerate() {
            let row = i / (COLUMNS - 1);
            let column = (i % (COLUMNS - 1)) * 2 + 1;
            grid.attach(&joint.cell, column as i32, row as i32, 1, 1);
        }

        let chain = Rc::new(Self {
            live: live.clone(),
            root,
            grid,
            boxes,
            dither,
            joints,
            default_order,
            saved,
            order: RefCell::new(Vec::new()),
            selected: Cell::new(Stage::Equalizer),
            on_select: RefCell::new(None),
            menu_stage: Cell::new(Stage::Equalizer),
            actions: gio::SimpleActionGroup::new(),
        });
        chain.place(&order_of(dsp));
        chain.connect_boxes();
        chain.connect_actions();
        chain.install_delete();
        chain.saved.connect(&chain);
        let weak = Rc::downgrade(&chain);
        chain.default_order.connect_clicked(move |_| {
            if let Some(chain) = weak.upgrade() {
                chain.reset_order();
            }
        });
        chain
    }

    /// The chain's widget.
    pub(super) fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Calls `select` when a stage's name is activated.
    pub(super) fn connect_select(&self, select: impl Fn(Stage) + 'static) {
        self.on_select.replace(Some(Box::new(select)));
    }

    fn stage_box(&self, stage: Stage) -> &StageBox {
        let index = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
        &self.boxes[index]
    }

    /// Puts the boxes in the grid in `order`.
    fn place(&self, order: &[Stage]) {
        if *self.order.borrow() == order {
            return;
        }
        for b in &self.boxes {
            if b.cell.parent().is_some() {
                self.grid.remove(&b.cell);
            }
        }
        if self.dither.cell.parent().is_some() {
            self.grid.remove(&self.dither.cell);
        }
        for (place, stage) in order.iter().enumerate() {
            let cell = &self.stage_box(*stage).cell;
            let (row, column) = (place / COLUMNS, (place % COLUMNS) * 2);
            self.grid.attach(cell, column as i32, row as i32, 1, 1);
        }
        let last = PLACES - 1;
        self.grid.attach(
            &self.dither.cell,
            ((last % COLUMNS) * 2) as i32,
            (last / COLUMNS) as i32,
            1,
            1,
        );
        self.order.replace(order.to_vec());
    }

    /// Shows the insertion bar before `place` (7: before Dither), or none.
    fn show_bar(&self, place: Option<usize>) {
        for b in &self.boxes {
            b.bar.set_visible(false);
        }
        self.dither.bar.set_visible(false);
        for joint in &self.joints {
            joint.bar.set_visible(false);
        }
        let Some(place) = place else {
            return;
        };
        if place % COLUMNS == 0 {
            // At the start of a row: at the box's left edge.
            match self.order.borrow().get(place) {
                Some(stage) => self.stage_box(*stage).bar.set_visible(true),
                None => self.dither.bar.set_visible(true),
            }
        } else {
            let joint = (place / COLUMNS) * (COLUMNS - 1) + place % COLUMNS - 1;
            if let Some(joint) = self.joints.get(joint) {
                joint.bar.set_visible(true);
            }
        }
    }

    /// Where a drop at `x` over the box at `place` would put a stage: before
    /// it in its left half, after it in its right half.
    fn drop_place(&self, place: usize, x: f64, width: i32) -> usize {
        if place >= Stage::ALL.len() {
            return Stage::ALL.len();
        }
        if x < f64::from(width) / 2.0 {
            place
        } else {
            place + 1
        }
    }

    /// Moves `stage` to be before `place` and says its new place.
    fn move_to(&self, stage: Stage, place: usize) {
        let order = self.order.borrow().clone();
        let Some(order) = moved(&order, stage, place) else {
            return;
        };
        let new: Vec<ChainStage> = order.iter().map(|s| s.chain()).collect();
        if self.live.apply(move |s| s.dsp.order.clone_from(&new)) {
            let at = order.iter().position(|s| *s == stage).unwrap_or(0);
            let b = self.stage_box(stage);
            b.button.grab_focus();
            b.button.announce(
                &fill(
                    &tr("{}, stage {} of {}"),
                    &[
                        &stage.title(),
                        &(at + 1).to_string(),
                        &Stage::ALL.len().to_string(),
                    ],
                ),
                gtk::AccessibleAnnouncementPriority::Medium,
            );
        }
    }

    /// Moves `stage` one place earlier (`-1`) or later (`1`).
    fn step(&self, stage: Stage, by: isize) {
        let Some(at) = self.order.borrow().iter().position(|s| *s == stage) else {
            return;
        };
        let Some(to) = at.checked_add_signed(by).filter(|t| *t < Stage::ALL.len()) else {
            return;
        };
        // Before the stage at `to`, or after it when moving later.
        let place = if by > 0 { to + 1 } else { to };
        self.move_to(stage, place);
    }

    /// Puts the order back to the default, without touching the switches.
    fn reset_order(&self) {
        self.live
            .apply(|s| s.dsp.order = ChainStage::DEFAULT_ORDER.to_vec());
    }

    pub(super) fn select(&self, stage: Stage) {
        self.selected.set(stage);
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
    }

    fn connect_boxes(self: &Rc<Self>) {
        for b in &self.boxes {
            let stage = b.stage;
            let weak = Rc::downgrade(self);
            b.button.connect_clicked(move |button| {
                let Some(chain) = weak.upgrade() else {
                    return;
                };
                if let Some(select) = chain.on_select.borrow().as_ref() {
                    select(stage);
                }
                if !button.is_active() {
                    button.set_active(true);
                }
            });
            let weak = Rc::downgrade(self);
            b.switch.connect_active_notify(move |switch| {
                let Some(chain) = weak.upgrade() else {
                    return;
                };
                let on = switch.is_active();
                let stored = chain.live.settings().is_some_and(|s| stage.enabled(&s.dsp));
                if stored != on && !chain.live.apply(|s| stage.set_enabled(s, on)) {
                    switch.set_active(stored);
                }
            });

            // Keys: Alt+Left and Alt+Right move the stage; the Menu key and
            // Shift+F10 open its menu.
            let keys = gtk::EventControllerKey::new();
            let weak = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(chain) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let alt = state.contains(gdk::ModifierType::ALT_MASK);
                match key {
                    gdk::Key::Left | gdk::Key::KP_Left if alt => chain.step(stage, -1),
                    gdk::Key::Right | gdk::Key::KP_Right if alt => chain.step(stage, 1),
                    _ if crate::views::actions::is_menu_key(key, state) => {
                        chain.popup_menu(stage, None);
                    }
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            });
            b.button.add_controller(keys);
            let click = gtk::GestureClick::new();
            click.set_button(gdk::BUTTON_SECONDARY);
            let weak = Rc::downgrade(self);
            click.connect_pressed(move |gesture, _, x, y| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let Some(chain) = weak.upgrade() {
                    chain.popup_menu(stage, Some((x, y)));
                }
            });
            b.cell.add_controller(click);

            // Dragging the box.
            let source = gtk::DragSource::new();
            source.set_actions(gdk::DragAction::MOVE);
            let index = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
            source.connect_prepare(move |_, _, _| {
                Some(gdk::ContentProvider::for_value(
                    &DraggedStage(index).to_value(),
                ))
            });
            let root = b.root.clone();
            source.connect_drag_begin(move |source, _| {
                // A copy of the box follows the pointer; the box stays, faded.
                let copy = gtk::WidgetPaintable::new(Some(&root)).current_image();
                source.set_icon(Some(&copy), root.width() / 2, root.height() / 2);
                root.add_css_class("dragged");
            });
            let root = b.root.clone();
            let weak = Rc::downgrade(self);
            source.connect_drag_end(move |_, _, _| {
                root.remove_css_class("dragged");
                if let Some(chain) = weak.upgrade() {
                    chain.show_bar(None);
                }
            });
            b.cell.add_controller(source);
            self.drop_target(&b.cell, Some(stage));
        }
        self.drop_target(&self.dither.cell.clone(), None);
    }

    /// Lets `cell`, the cell of `stage` (`None`: Dither), take a dragged
    /// stage.
    fn drop_target(self: &Rc<Self>, cell: &gtk::Overlay, stage: Option<Stage>) {
        let target = gtk::DropTarget::new(DraggedStage::static_type(), gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        let place_of = move |chain: &Self| match stage {
            Some(stage) => chain
                .order
                .borrow()
                .iter()
                .position(|s| *s == stage)
                .unwrap_or(0),
            None => Stage::ALL.len(),
        };
        let place_at = Rc::new(place_of);
        let at = place_at.clone();
        target.connect_motion(move |target, x, _| {
            let Some(chain) = weak.upgrade() else {
                return gdk::DragAction::empty();
            };
            let width = target.widget().map_or(1, |w| w.width());
            let place = chain.drop_place(at(&chain), x, width);
            let dragged = target
                .value()
                .and_then(|v| v.get::<DraggedStage>().ok())
                .and_then(|d| Stage::ALL.get(d.0).copied());
            let order = chain.order.borrow().clone();
            let changes = dragged.is_some_and(|d| moved(&order, d, place).is_some());
            chain.show_bar(changes.then_some(place));
            gdk::DragAction::MOVE
        });
        let weak = Rc::downgrade(self);
        target.connect_leave(move |_| {
            if let Some(chain) = weak.upgrade() {
                chain.show_bar(None);
            }
        });
        let weak = Rc::downgrade(self);
        target.connect_drop(move |target, value, x, _| {
            let (Some(chain), Ok(dragged)) = (weak.upgrade(), value.get::<DraggedStage>()) else {
                return false;
            };
            let Some(stage) = Stage::ALL.get(dragged.0).copied() else {
                return false;
            };
            chain.show_bar(None);
            let width = target.widget().map_or(1, |w| w.width());
            let place = chain.drop_place(place_at(&chain), x, width);
            chain.move_to(stage, place);
            true
        });
        cell.add_controller(target);
    }

    fn connect_actions(self: &Rc<Self>) {
        let action = |name: &str, run: fn(&Self)| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(chain) = weak.upgrade() {
                    run(&chain);
                }
            });
            self.actions.add_action(&action);
        };
        action("earlier", |chain| chain.step(chain.menu_stage.get(), -1));
        action("later", |chain| chain.step(chain.menu_stage.get(), 1));
        action("toggle", |chain| {
            let stage = chain.menu_stage.get();
            let on = chain.live.settings().is_some_and(|s| stage.enabled(&s.dsp));
            chain.live.apply(|s| stage.set_enabled(s, !on));
        });
        action("default", Self::reset_order);
        self.root.insert_action_group("chain", Some(&self.actions));
    }

    fn enable(&self, name: &str, on: bool) {
        if let Some(action) = self
            .actions
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(on);
        }
    }

    /// The context menu of `stage`, at `at` in its cell or at the middle.
    fn popup_menu(&self, stage: Stage, at: Option<(f64, f64)>) {
        self.menu_stage.set(stage);
        let place = self
            .order
            .borrow()
            .iter()
            .position(|s| *s == stage)
            .unwrap_or(0);
        self.enable("earlier", place > 0);
        self.enable("later", place + 1 < Stage::ALL.len());
        let is_default = *self.order.borrow() == Stage::ALL;
        self.enable("default", !is_default);
        let on = self.live.settings().is_some_and(|s| stage.enabled(&s.dsp));
        let menu = gio::Menu::new();
        let moves = gio::Menu::new();
        let item = |text: &str, action: &str, accel: Option<&str>| {
            let item = gio::MenuItem::new(Some(text), Some(action));
            if let Some(accel) = accel {
                item.set_attribute_value("accel", Some(&accel.to_variant()));
            }
            item
        };
        moves.append_item(&item(
            &tr("Move Earlier"),
            "chain.earlier",
            Some("<Alt>Left"),
        ));
        moves.append_item(&item(&tr("Move Later"), "chain.later", Some("<Alt>Right")));
        let switch = if on { tr("Turn Off") } else { tr("Turn On") };
        moves.append_item(&item(&switch, "chain.toggle", None));
        menu.append_section(None, &moves);
        let reset = gio::Menu::new();
        reset.append_item(&item(&tr("Default Order"), "chain.default", None));
        menu.append_section(None, &reset);
        let cell = &self.stage_box(stage).cell;
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        crate::widgets::menu_names::name_menus(&popover);
        popover.add_css_class("hk-context-menu");
        popover.set_widget_name("chain-menu");
        popover.set_parent(cell);
        popover.set_has_arrow(false);
        let (x, y) = at.unwrap_or((
            f64::from(cell.width()) / 2.0,
            f64::from(cell.height()) / 2.0,
        ));
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        crate::views::actions::popup_when_built(&popover);
    }

    /// Shows the stored settings.
    pub(super) fn refresh(&self, dsp: &DspSettings, facts: &Facts) {
        let order = order_of(dsp);
        self.place(&order);
        for b in &self.boxes {
            let place = order.iter().position(|s| *s == b.stage).unwrap_or(0);
            let on = b.stage.enabled(dsp);
            if b.switch.is_active() != on {
                b.switch.set_active(on);
            }
            b.place.set_text(&(place + 1).to_string());
            let (summary, warning) = b.stage.summary(dsp, facts);
            b.summary.set_text(&summary);
            b.warning.set_visible(warning);
            let tooltip = match (&facts.track_eq, b.stage) {
                (Some(track), Stage::Equalizer) if on => {
                    Some(fill(&tr("Track equalizer: {}"), &[&track.name]))
                }
                _ => None,
            };
            b.summary.set_tooltip_text(tooltip.as_deref());
            if on {
                b.root.add_css_class("on");
            } else {
                b.root.remove_css_class("on");
            }
            b.button.update_property(&[
                gtk::accessible::Property::Label(&spoken(b.stage, place, on)),
                gtk::accessible::Property::Description(&summary),
            ]);
        }
        self.default_order
            .set_sensitive(order != Stage::ALL.to_vec());
        self.saved.refresh(dsp);
    }
}

/// The "Saved chains" popover: the saved chains, a line to save the chain
/// as it is now, and a sentence on what a chain keeps.
struct SavedChains {
    button: gtk::MenuButton,
    list: gtk::ListBox,
    empty: gtk::Label,
    entry: gtk::Entry,
    save: gtk::Button,
    /// The chains shown, and which one is marked.
    shown: RefCell<Option<(Vec<SavedChain>, Option<usize>)>>,
}

/// The stages that are on in `dsp`, in order.
fn stages_on(dsp: &DspSettings) -> Vec<ChainStage> {
    dsp.effective_order()
        .into_iter()
        .filter(|s| dsp.is_enabled(*s))
        .collect()
}

/// "Equalizer → Compressor → Limiter".
fn stage_line(stages: &[ChainStage]) -> String {
    if stages.is_empty() {
        return tr("Every stage off");
    }
    stages
        .iter()
        .map(|s| Stage::of(*s).title())
        .collect::<Vec<_>>()
        .join(" → ")
}

impl SavedChains {
    fn new() -> Self {
        let button = gtk::MenuButton::new();
        button.set_label(&tr("Saved chains"));
        button.set_always_show_arrow(true);
        button.add_css_class("hk-sort");
        button.add_css_class("hk-chains-btn");
        button.set_valign(gtk::Align::Center);
        button.set_widget_name("saved-chains");
        let popover = gtk::Popover::new();
        popover.add_css_class("hk-saved-chains");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_halign(gtk::Align::End);
        popover.update_property(&[gtk::accessible::Property::Label(&tr("Saved chains"))]);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_size_request(336, -1);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("hk-saved-chain-list");
        list.set_widget_name("saved-chain-list");
        list.update_property(&[gtk::accessible::Property::Label(&tr("Saved chains"))]);
        column.append(&list);
        let empty = label(&tr("No saved chains yet."), &["hk-t2"]);
        empty.set_margin_top(6);
        empty.set_margin_bottom(6);
        empty.set_margin_start(6);
        column.append(&empty);
        column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some(&tr("Name for the chain as it is now")));
        entry.update_property(&[gtk::accessible::Property::Label(&tr(
            "Name for the chain as it is now",
        ))]);
        entry.set_widget_name("chain-name");
        line.append(&entry);
        let save = compact_button(&tr("Save"));
        save.set_widget_name("chain-save");
        save.set_sensitive(false);
        line.append(&save);
        column.append(&line);
        let note = crate::dialogs::common::note(&tr(
            "A chain keeps the order of the stages and which are on. Each stage keeps its own values.",
        ));
        note.set_max_width_chars(40);
        column.append(&note);
        popover.set_child(Some(&column));
        button.set_popover(Some(&popover));
        Self {
            button,
            list,
            empty,
            entry,
            save,
            shown: RefCell::new(None),
        }
    }

    fn connect(&self, chain: &Rc<Chain>) {
        let weak = Rc::downgrade(chain);
        let save_now = move || {
            let Some(chain) = weak.upgrade() else {
                return;
            };
            let name = chain.saved.entry.text().trim().to_owned();
            if name.is_empty() {
                return;
            }
            if chain.live.apply(|s| s.dsp.save_chain(&name)) {
                chain.saved.entry.set_text("");
            }
        };
        let save_now = Rc::new(save_now);
        let run = save_now.clone();
        self.save.connect_clicked(move |_| run());
        self.entry.connect_activate(move |_| save_now());
        let weak = Rc::downgrade(chain);
        self.entry.connect_changed(move |entry| {
            if let Some(chain) = weak.upgrade() {
                chain.saved.update_save(entry.text().trim());
            }
        });
        let weak = Rc::downgrade(chain);
        self.list.connect_row_activated(move |_, row| {
            let Some(chain) = weak.upgrade() else {
                return;
            };
            let name = row.widget_name().to_string();
            // The popover stays open.
            chain.live.apply(|s| {
                s.dsp.apply_chain(&name);
            });
            row.grab_focus();
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(chain);
        keys.connect_key_pressed(move |controller, key, _, _| {
            if key != gdk::Key::Delete && key != gdk::Key::KP_Delete {
                return glib::Propagation::Proceed;
            }
            let (Some(chain), Some(list)) = (weak.upgrade(), controller.widget()) else {
                return glib::Propagation::Proceed;
            };
            let focused = list
                .root()
                .and_then(|r| r.focus())
                .and_then(|f| {
                    if f.is::<gtk::ListBoxRow>() {
                        Some(f)
                    } else {
                        f.ancestor(gtk::ListBoxRow::static_type())
                    }
                })
                .filter(|row| row.is_ancestor(&list));
            match focused {
                Some(row) => {
                    delete(&chain, &row.widget_name());
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        self.list.add_controller(keys);
    }

    /// "Save", or "Replace" for a name that exists.
    fn update_save(&self, name: &str) {
        let exists = self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|(chains, _)| chains.iter().any(|c| c.name == name));
        self.save
            .set_label(&if exists { tr("Replace") } else { tr("Save") });
        self.save.set_sensitive(!name.is_empty());
    }

    fn refresh(&self, dsp: &DspSettings) {
        let now = stages_on(dsp);
        let marked = dsp.chains.iter().position(|c| c.stages == now);
        let key = (dsp.chains.clone(), marked);
        if self.shown.borrow().as_ref() == Some(&key) {
            return;
        }
        let focused_row = self
            .list
            .root()
            .and_then(|r| r.focus())
            .and_then(|f| f.ancestor(gtk::ListBoxRow::static_type()))
            .filter(|r| r.is_ancestor(&self.list))
            .map(|r| r.widget_name().to_string());
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for (i, saved) in dsp.chains.iter().enumerate() {
            let row = self.row(saved, marked == Some(i));
            self.list.append(&row);
            if focused_row.as_deref() == Some(saved.name.as_str()) {
                row.grab_focus();
            }
        }
        self.list.set_visible(!dsp.chains.is_empty());
        self.empty.set_visible(dsp.chains.is_empty());
        self.shown.replace(Some(key));
        self.update_save(self.entry.text().trim());
    }

    fn row(&self, saved: &SavedChain, marked: bool) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("hk-saved-chain");
        row.set_widget_name(&saved.name);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        mark.set_size_request(16, -1);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("hk-chain-dot");
        dot.set_size_request(6, 6);
        dot.set_halign(gtk::Align::Center);
        dot.set_valign(gtk::Align::Center);
        dot.set_hexpand(true);
        dot.set_visible(marked);
        mark.append(&dot);
        line.append(&mark);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let name = label(
            &saved.name,
            if marked {
                &["hk-strong"]
            } else {
                &["hk-medium"]
            },
        );
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&name);
        let stages = label(&stage_line(&saved.stages), &["hk-note"]);
        stages.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&stages);
        line.append(&text);
        let bin = gtk::Button::from_icon_name("helikodu-waste-bin-symbolic");
        bin.add_css_class("hk-icon-btn");
        bin.add_css_class("flat");
        bin.set_size_request(28, 28);
        bin.set_valign(gtk::Align::Center);
        let text = fill(&tr("Delete the chain {}"), &[&saved.name]);
        bin.set_tooltip_text(Some(&text));
        bin.update_property(&[gtk::accessible::Property::Label(&text)]);
        bin.set_widget_name(&format!("delete-chain-{}", saved.name));
        bin.set_action_name(Some("chain.delete"));
        bin.set_action_target_value(Some(&saved.name.to_variant()));
        line.append(&bin);
        row.set_child(Some(&line));
        let mut description = stage_line(&saved.stages);
        if marked {
            description = fill(&tr("{}, the chain as it is now"), &[&description]);
        }
        row.update_property(&[gtk::accessible::Property::Description(&description)]);
        row
    }
}

/// Deletes the saved chain `name` and offers to undo it in a toast.
fn delete(chain: &Rc<Chain>, name: &str) {
    let Some(settings) = chain.live.settings() else {
        return;
    };
    let Some(index) = settings.dsp.chains.iter().position(|c| c.name == name) else {
        return;
    };
    let removed = settings.dsp.chains[index].clone();
    if !chain
        .live
        .apply(|s| s.dsp.chains.retain(|c| c.name != removed.name))
    {
        return;
    }
    if let Some(first) = chain.saved.list.first_child() {
        first.grab_focus();
    } else {
        chain.saved.entry.grab_focus();
    }
    let Some(main) = chain.live.context().and_then(|c| c.main.upgrade()) else {
        return;
    };
    let live = Rc::downgrade(&chain.live);
    main.toast_undo(
        &fill(&tr("Deleted the chain “{}”"), &[&removed.name]),
        move || {
            let Some(live) = live.upgrade() else {
                return;
            };
            let removed = removed.clone();
            live.apply(move |s| {
                if s.dsp.chain(&removed.name).is_none() {
                    let at = index.min(s.dsp.chains.len());
                    s.dsp.chains.insert(at, removed.clone());
                }
            });
        },
    );
}

impl Chain {
    /// Adds the action that deletes a saved chain by name.
    fn install_delete(self: &Rc<Self>) {
        let action = gio::SimpleAction::new("delete", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        action.connect_activate(move |_, name| {
            let (Some(chain), Some(name)) = (weak.upgrade(), name.and_then(|n| n.str())) else {
                return;
            };
            delete(&chain, name);
        });
        self.actions.add_action(&action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stage_moves_before_the_place_it_is_dropped_at() {
        let order = Stage::ALL.to_vec();
        // The compressor (place 6) before Crossfeed (place 4).
        let new = moved(&order, Stage::Compressor, 3).unwrap();
        assert_eq!(
            new,
            [
                Stage::Pitch,
                Stage::Equalizer,
                Stage::Convolution,
                Stage::Compressor,
                Stage::Crossfeed,
                Stage::ChannelMix,
                Stage::Limiter,
            ]
        );
        // Before itself or after itself: nothing changes.
        assert_eq!(moved(&order, Stage::Compressor, 5), None);
        assert_eq!(moved(&order, Stage::Compressor, 6), None);
        // To the end, before Dither.
        let last = moved(&order, Stage::Pitch, 7).unwrap();
        assert_eq!(last.last(), Some(&Stage::Pitch));
    }
}
