// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Avatar: a round tile with the artist's photo, or else their initials on a
//! colour of the cover placeholder palettes, picked by a stable hash of the
//! name.
//!
//! The photo's thumbnail is loaded through the window's cover cache; until it
//! arrives the initials show, at the same size, so nothing moves.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk, pango};

use crate::backend::CoverKey;
use crate::models::CoverCache;
use crate::models::covers::thumbnail_size;
use crate::style::tokens::cover_placeholder;

/// The initials of `name`: the first letters of its first two words, without
/// a leading "The".
pub(crate) fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| w.chars().next().is_some_and(char::is_alphanumeric))
        .collect();
    let words = match words.as_slice() {
        [first, rest @ ..] if first.eq_ignore_ascii_case("the") && !rest.is_empty() => rest,
        all => all,
    };
    words
        .iter()
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

fn hash(text: &str) -> u64 {
    // FNV-1a: stable across runs and platforms.
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct Avatar {
        pub(super) side: Cell<i32>,
        pub(super) name: RefCell<String>,
        /// The photo asked for.
        pub(super) photo: Cell<Option<CoverKey>>,
        /// Its thumbnail, once loaded.
        pub(super) texture: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Avatar {
        const NAME: &'static str = "HkAvatar";
        type Type = super::Avatar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for Avatar {}

    impl WidgetImpl for Avatar {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let side = self.side.get();
            (side, side, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let side = self.side.get() as f32;
            let name = self.name.borrow();
            let palettes = cover_placeholder::PALETTES;
            let [ground, shape, _] = palettes[(hash(&name) % palettes.len() as u64) as usize];
            let rect = graphene::Rect::new(0.0, 0.0, side, side);
            let round = gsk::RoundedRect::from_rect(rect, side / 2.0);
            snapshot.push_rounded_clip(&round);
            snapshot.append_color(&ground.to_gdk(), &rect);
            if let Some(texture) = self.texture.borrow().as_ref() {
                // Fills the circle, cut to the middle of the picture.
                let (w, h) = (texture.width() as f32, texture.height() as f32);
                let scale = side / w.min(h).max(1.0);
                let (w, h) = (w * scale, h * scale);
                let area = graphene::Rect::new((side - w) / 2.0, (side - h) / 2.0, w, h);
                snapshot.append_scaled_texture(texture, gsk::ScalingFilter::Trilinear, &area);
                snapshot.pop();
                return;
            }
            snapshot.pop();
            let obj = self.obj();
            let layout = obj.create_pango_layout(Some(&initials(&name)));
            let mut font = pango::FontDescription::new();
            font.set_weight(pango::Weight::Semibold);
            font.set_absolute_size(f64::from(side * 0.36) * f64::from(pango::SCALE));
            layout.set_font_description(Some(&font));
            let (_, logical) = layout.pixel_extents();
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (side - logical.width() as f32) / 2.0,
                (side - logical.height() as f32) / 2.0,
            ));
            snapshot.append_layout(&layout, &shape.to_gdk());
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    /// A round avatar with initials.
    pub(crate) struct Avatar(ObjectSubclass<imp::Avatar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Avatar {
    /// An avatar of `side` pixels.
    pub(crate) fn new(side: i32) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().side.set(side);
        obj.set_valign(gtk::Align::Center);
        obj
    }

    /// Shows the initials of `name`.
    pub(crate) fn set_name(&self, name: &str) {
        self.imp().name.replace(name.to_owned());
        self.queue_draw();
    }

    /// Shows `photo` from `covers`, or the initials when there is none; the
    /// initials stay until the thumbnail is loaded.
    pub(crate) fn set_photo(&self, covers: &Rc<CoverCache>, photo: Option<CoverKey>) {
        let imp = self.imp();
        if imp.photo.get() == photo && (photo.is_none() || imp.texture.borrow().is_some()) {
            return;
        }
        imp.photo.set(photo);
        imp.texture.replace(None);
        self.queue_draw();
        let Some(photo) = photo else { return };
        let size = thumbnail_size(imp.side.get(), self.scale_factor());
        let weak = self.downgrade();
        let ready = Box::new(move |texture: &gdk::Texture| {
            if let Some(avatar) = weak.upgrade()
                && avatar.imp().photo.get() == Some(photo)
            {
                avatar.imp().texture.replace(Some(texture.clone()));
                avatar.queue_draw();
            }
        });
        if let Some(texture) = covers.get(photo, size, ready) {
            imp.texture.replace(Some(texture));
        }
    }

    /// The photo still waited for, with its thumbnail size.
    pub(crate) fn waiting(&self) -> Option<(CoverKey, u32)> {
        let imp = self.imp();
        let photo = imp.photo.get().filter(|_| imp.texture.borrow().is_none())?;
        Some((photo, thumbnail_size(imp.side.get(), self.scale_factor())))
    }

    /// The photo shown or asked for (for tests).
    pub(crate) fn photo(&self) -> Option<CoverKey> {
        self.imp().photo.get()
    }

    /// Whether the photo's thumbnail is drawn (for tests).
    pub(crate) fn shows_photo(&self) -> bool {
        self.imp().texture.borrow().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn initials_skip_the_article() {
        assert_eq!(initials("Mari Kask"), "MK");
        assert_eq!(initials("The Linden Quartet"), "LQ");
        assert_eq!(initials("Lumen"), "L");
        assert_eq!(initials("The"), "T");
        assert_eq!(initials("Moss & Ember"), "ME");
    }
}
