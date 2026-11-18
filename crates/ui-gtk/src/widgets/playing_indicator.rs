// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! PlayingIndicator: three bars in the accent fill that replace the track
//! number of the playing row. The bars move while playback runs and stand
//! still when it is paused or stopped, and always under reduced motion. The
//! frame clock only ticks for an indicator that is mapped and moving, so a
//! paused player draws no frames.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene};

const SIDE: i32 = 12;
/// The bars' heights when they stand still.
const STILL: [f32; 3] = [7.0, 12.0, 9.0];

thread_local! {
    static RUNNING: Cell<bool> = const { Cell::new(false) };
    static ALL: RefCell<Vec<glib::WeakRef<PlayingIndicator>>> = const { RefCell::new(Vec::new()) };
}

/// Tells every indicator whether playback runs.
pub(crate) fn set_running(running: bool) {
    if RUNNING.replace(running) == running {
        return;
    }
    refresh_all();
}

fn refresh_all() {
    let live: Vec<PlayingIndicator> = ALL.with_borrow_mut(|all| {
        all.retain(|w| w.upgrade().is_some());
        all.iter().filter_map(glib::WeakRef::upgrade).collect()
    });
    for indicator in live {
        indicator.refresh();
    }
}

/// The height of bar `i` at `t` seconds: three out-of-step waves.
fn bar_height(i: usize, t: f64) -> f32 {
    const SPEED: [f64; 3] = [7.1, 9.3, 5.7];
    const PHASE: [f64; 3] = [0.0, 1.9, 4.1];
    let s = (t * SPEED[i] + PHASE[i]).sin() * 0.5 + 0.5;
    (3.0 + 9.0 * s) as f32
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct PlayingIndicator {
        pub(super) tick: RefCell<Option<gtk::TickCallbackId>>,
        pub(super) time: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayingIndicator {
        const NAME: &'static str = "HkPlayingIndicator";
        type Type = super::PlayingIndicator;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for PlayingIndicator {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.update_property(&[gtk::accessible::Property::Label(&crate::i18n::tr(
                "Playing",
            ))]);
            ALL.with_borrow_mut(|all| all.push(obj.downgrade()));
            let weak = obj.downgrade();
            crate::style::connect_changed(move |_| {
                if let Some(obj) = weak.upgrade() {
                    obj.refresh();
                    obj.queue_draw();
                }
            });
        }

        fn dispose(&self) {
            if let Some(id) = self.tick.take() {
                id.remove();
            }
        }
    }

    impl WidgetImpl for PlayingIndicator {
        fn map(&self) {
            self.parent_map();
            self.obj().refresh();
        }

        fn unmap(&self) {
            if let Some(id) = self.tick.take() {
                id.remove();
            }
            self.parent_unmap();
        }

        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (SIDE, SIDE, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let look = crate::style::current();
            let color = look.accent.text.to_gdk();
            let x0 = (obj.width() - SIDE) as f32 / 2.0;
            let y0 = (obj.height() - SIDE) as f32 / 2.0;
            let moving = self.tick.borrow().is_some();
            for (i, still) in STILL.into_iter().enumerate() {
                let h = if moving {
                    bar_height(i, self.time.get())
                } else {
                    still
                };
                let x = x0 + i as f32 * 4.5;
                snapshot.append_color(&color, &graphene::Rect::new(x, y0 + 12.0 - h, 3.0, h));
            }
        }
    }
}

glib::wrapper! {
    /// Three bars that mark the playing track.
    pub(crate) struct PlayingIndicator(ObjectSubclass<imp::PlayingIndicator>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for PlayingIndicator {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl PlayingIndicator {
    /// Starts or stops the tick callback to match playback, reduced motion
    /// and whether the indicator is on screen.
    fn refresh(&self) {
        let imp = self.imp();
        let want = self.is_mapped() && RUNNING.get() && !crate::style::current().reduced_motion;
        let has = imp.tick.borrow().is_some();
        if want && !has {
            let id = self.add_tick_callback(|obj, clock| {
                obj.imp().time.set(clock.frame_time() as f64 / 1e6);
                obj.queue_draw();
                glib::ControlFlow::Continue
            });
            imp.tick.replace(Some(id));
        } else if !want && has {
            if let Some(id) = imp.tick.take() {
                id.remove();
            }
            self.queue_draw();
        }
    }
}
