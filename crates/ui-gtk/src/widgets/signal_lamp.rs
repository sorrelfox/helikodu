// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SignalLamp: a 7 px disc with a soft glow in the colour of the signal state.
//! The label beside it always carries the meaning; the colour is secondary.

use std::cell::Cell;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use crate::backend::SignalState;
use crate::style::color::Rgba;
use crate::style::tokens::signal;

const LAMP: f32 = 7.0;

/// The lamp colour of a state.
pub(crate) fn lamp_color(state: SignalState) -> Rgba {
    match state {
        SignalState::BitPerfect => signal::bit_perfect::LAMP,
        SignalState::Processed { .. } => signal::processed::LAMP,
        SignalState::Idle | SignalState::Unreported | SignalState::NotConnected => {
            signal::idle::LAMP
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct SignalLamp {
        pub(super) state: Cell<SignalState>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SignalLamp {
        const NAME: &'static str = "HkSignalLamp";
        type Type = super::SignalLamp;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for SignalLamp {}

    impl WidgetImpl for SignalLamp {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (LAMP as i32 + 2, LAMP as i32 + 2, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let x = (obj.width() as f32 - LAMP) / 2.0;
            let y = (obj.height() as f32 - LAMP) / 2.0;
            let disc =
                gsk::RoundedRect::from_rect(graphene::Rect::new(x, y, LAMP, LAMP), LAMP / 2.0);
            let state = self.state.get();
            let color = lamp_color(state).to_gdk();
            if matches!(
                state,
                SignalState::BitPerfect | SignalState::Processed { .. }
            ) {
                snapshot.append_outset_shadow(&disc, &color, 0.0, 0.0, 0.0, 8.0);
            }
            snapshot.push_rounded_clip(&disc);
            snapshot.append_color(&color, disc.bounds());
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    /// The signal lamp of the output button.
    pub(crate) struct SignalLamp(ObjectSubclass<imp::SignalLamp>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SignalLamp {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SignalLamp {
    /// Shows `state`.
    pub(crate) fn set_state(&self, state: SignalState) {
        if self.imp().state.replace(state) != state {
            self.queue_draw();
        }
    }
}
