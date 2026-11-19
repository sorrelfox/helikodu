// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A column beside a view's main content whose width the user drags: the album
//! page's details column. It asks for its chosen width however wide its
//! contents would like to be, so long text wraps instead of widening it. An
//! invisible grip lies over its start edge (the column's 1 px border) and
//! changes the width within its limits, keeping the content beside it at least
//! as wide as its own minimum.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene};

use crate::i18n::tr;

/// The width of the grip, centred on the column's start edge.
const GRIP: i32 = 7;

type Changed = Box<dyn Fn(i32)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub(crate) struct SideColumn {
        pub(super) child: RefCell<Option<gtk::Widget>>,
        pub(super) grip: RefCell<Option<gtk::Box>>,
        /// The width the user chose.
        pub(super) width: Cell<i32>,
        pub(super) min: Cell<i32>,
        pub(super) max: Cell<i32>,
        /// How wide the content beside the column stays at least while dragging.
        pub(super) beside_min: Cell<i32>,
        pub(super) changed: RefCell<Option<Changed>>,
    }

    impl std::fmt::Debug for SideColumn {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SideColumn")
                .field("width", &self.width)
                .finish_non_exhaustive()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SideColumn {
        const NAME: &'static str = "HkSideColumn";
        type Type = super::SideColumn;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SideColumn {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let grip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            grip.add_css_class("hk-pane-handle");
            grip.set_cursor_from_name(Some("col-resize"));
            grip.set_tooltip_text(Some(&tr("Drag to change the width")));
            grip.set_parent(&*obj);
            let drag = gtk::GestureDrag::new();
            // Where the drag started, across the parent (the column itself
            // moves as it widens), and the width then.
            let start: Rc<Cell<(Option<f32>, i32)>> = Rc::default();
            let weak = obj.downgrade();
            let s = start.clone();
            drag.connect_drag_begin(move |gesture, _, _| {
                if let Some(column) = weak.upgrade() {
                    s.set((column.pointer_in_parent(gesture), column.width()));
                }
            });
            let weak = obj.downgrade();
            drag.connect_drag_update(move |gesture, dx, _| {
                if let Some(column) = weak.upgrade() {
                    let (x0, w0) = start.get();
                    // Without a pointer (a drag from a test) the offset serves.
                    let delta = match (x0, column.pointer_in_parent(gesture)) {
                        (Some(x0), Some(x)) => f64::from(x0 - x),
                        _ => -dx,
                    };
                    column.drag_to(w0 + delta.round() as i32);
                }
            });
            let weak = obj.downgrade();
            drag.connect_drag_end(move |_, _, _| {
                if let Some(column) = weak.upgrade()
                    && let Some(changed) = column.imp().changed.borrow().as_ref()
                {
                    changed(column.imp().width.get());
                }
            });
            grip.add_controller(drag);
            self.grip.replace(Some(grip));
        }

        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
            if let Some(grip) = self.grip.take() {
                grip.unparent();
            }
        }
    }

    impl WidgetImpl for SideColumn {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.child
                .borrow()
                .as_ref()
                .map_or(gtk::SizeRequestMode::ConstantSize, |c| c.request_mode())
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.child.borrow().clone() else {
                return (0, 0, -1, -1);
            };
            if orientation == gtk::Orientation::Horizontal {
                // The chosen width, whatever the content's natural width. The
                // minimum is the narrowest the content wraps to at any height:
                // a wrapping label measured for a given height asks for the
                // width of its unwrapped text.
                let (min, ..) = child.measure(orientation, -1);
                let min = min.max(self.min.get());
                (min, self.width.get().max(min), -1, -1)
            } else {
                // Without a width, the height at the chosen width: the height
                // at the narrowest width would be far taller than the column
                // ever is.
                let for_size = if for_size < 0 {
                    let (min, ..) = child.measure(gtk::Orientation::Horizontal, -1);
                    self.width.get().max(min)
                } else {
                    for_size
                };
                let (min, nat, ..) = child.measure(orientation, for_size);
                (min, nat, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
            if let Some(grip) = self.grip.borrow().as_ref() {
                // A column as wide as its parent is under the content, not
                // beside it: there is no width to drag.
                let beside = self.obj().parent().is_none_or(|p| width < p.width());
                grip.set_child_visible(beside);
                // Half outside the column, so the grip lies across its border.
                let at = gsk_offset(-(GRIP / 2));
                grip.allocate(GRIP, height, -1, Some(at));
            }
        }
    }
}

fn gsk_offset(x: i32) -> gtk::gsk::Transform {
    gtk::gsk::Transform::new().translate(&graphene::Point::new(x as f32, 0.0))
}

glib::wrapper! {
    /// A side column with a draggable width.
    pub(crate) struct SideColumn(ObjectSubclass<imp::SideColumn>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SideColumn {
    /// A column showing `child`, `width` wide, between `min` and `max`.
    pub(crate) fn new(child: &impl IsA<gtk::Widget>, width: i32, (min, max): (i32, i32)) -> Self {
        let column: Self = glib::Object::new();
        let imp = column.imp();
        imp.min.set(min);
        imp.max.set(max);
        imp.width.set(width.clamp(min, max));
        let child = child.clone().upcast::<gtk::Widget>();
        // Before the grip, so the grip is drawn and picked on top.
        child.insert_after(&column, None::<&gtk::Widget>);
        imp.child.replace(Some(child));
        column
    }

    /// Keeps the content beside the column at least `width` wide while the
    /// user drags.
    pub(crate) fn set_beside_min(&self, width: i32) {
        self.imp().beside_min.set(width);
    }

    /// Calls `changed` with the new width after the user drags the grip.
    pub(crate) fn connect_changed(&self, changed: impl Fn(i32) + 'static) {
        self.imp().changed.replace(Some(Box::new(changed)));
    }

    /// Sets the chosen width within the column's limits.
    pub(crate) fn set_chosen_width(&self, width: i32) {
        let imp = self.imp();
        let width = width.clamp(imp.min.get(), imp.max.get());
        if imp.width.replace(width) != width {
            self.queue_resize();
        }
    }

    /// Sets the width the user dragged to, within the column's limits and the
    /// room the content beside it leaves.
    fn drag_to(&self, width: i32) {
        let mut width = width;
        if let Some(parent) = self.parent() {
            let mut beside = 0;
            let mut sibling = parent.first_child();
            while let Some(widget) = sibling {
                if &widget != self.upcast_ref::<gtk::Widget>() && widget.is_visible() {
                    let (min, ..) = widget.measure(gtk::Orientation::Horizontal, -1);
                    beside += min;
                }
                sibling = widget.next_sibling();
            }
            let room = parent.width() - beside.max(self.imp().beside_min.get());
            width = width.min(room.max(self.imp().min.get()));
        }
        self.set_chosen_width(width);
    }

    /// Where the drag's pointer is, across the column's parent.
    fn pointer_in_parent(&self, gesture: &gtk::GestureDrag) -> Option<f32> {
        let (x, y) = gesture.point(None)?;
        let parent = self.parent()?;
        let grip = gesture.widget()?;
        let point = grip.compute_point(&parent, &graphene::Point::new(x as f32, y as f32))?;
        Some(point.x())
    }
}
