// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cover thumbnails. The library makes them (newest request first) into its
//! cache folder; this loads the files on worker threads into a small cache of
//! textures. A cover shows its generated placeholder until the thumbnail
//! arrives. The album grid runs a visibility pass once per frame after
//! scrolling: requests of visible tiles move to the front again, and requests of
//! tiles that were recycled are dropped.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::backend::{Backend, CoverKey, ThumbnailState};
use crate::widgets::CoverArt;

/// The pixel memory of the textures kept (about 160 thumbnails at 192 px);
/// the covers on screen hold theirs anyway, so this only spares reloading
/// the ones just scrolled past.
const CACHE_BYTES: usize = 24 << 20;

type Ready = Box<dyn Fn(&gdk::Texture)>;
type Key = (CoverKey, u32);

/// The thumbnail loader and cache of one window.
#[derive(Default)]
pub(crate) struct CoverCache {
    backend: RefCell<Option<Backend>>,
    textures: RefCell<HashMap<Key, gdk::Texture>>,
    order: RefCell<VecDeque<Key>>,
    bytes: Cell<usize>,
    waiting: RefCell<HashMap<Key, Vec<Ready>>>,
    loading: RefCell<HashSet<Key>>,
}

impl std::fmt::Debug for CoverCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoverCache")
            .field("textures", &self.textures.borrow().len())
            .field("waiting", &self.waiting.borrow().len())
            .finish()
    }
}

/// The thumbnail size for a cover of `logical` pixels at `scale`.
pub(crate) fn thumbnail_size(logical: i32, scale: i32) -> u32 {
    let wanted = u32::try_from(logical.max(1) * scale.max(1)).unwrap_or(192);
    [48, 96, 192, 384]
        .into_iter()
        .find(|s| *s >= wanted)
        .unwrap_or(384)
}

impl CoverCache {
    pub(crate) fn new(backend: &Backend) -> Rc<Self> {
        let cache = Rc::new(Self::default());
        cache.backend.replace(Some(backend.clone()));
        cache
    }

    /// The texture of `cover` at `size` if cached; otherwise asks for it and calls
    /// `ready` when it arrives.
    pub(crate) fn get(
        self: &Rc<Self>,
        cover: CoverKey,
        size: u32,
        ready: Ready,
    ) -> Option<gdk::Texture> {
        let key = (cover, size);
        if let Some(texture) = self.textures.borrow().get(&key) {
            return Some(texture.clone());
        }
        let first = !self.waiting.borrow().contains_key(&key);
        self.waiting
            .borrow_mut()
            .entry(key)
            .or_default()
            .push(ready);
        if first {
            self.ask(key);
        }
        None
    }

    /// Asks the library again; a scheduled request moves to the front.
    fn ask(self: &Rc<Self>, key: Key) {
        let Some(backend) = self.backend.borrow().clone() else {
            return;
        };
        match backend.library().thumbnail(&key.0, key.1) {
            ThumbnailState::Ready(path) => self.load(key, path),
            ThumbnailState::Pending => {}
            ThumbnailState::Unavailable => {
                self.waiting.borrow_mut().remove(&key);
            }
        }
    }

    /// Shows `cover` on `art` (or the placeholder for `seed`), at the thumbnail
    /// size for a cover of `logical` pixels.
    pub(crate) fn show(
        self: &Rc<Self>,
        art: &CoverArt,
        cover: Option<CoverKey>,
        seed: u64,
        logical: i32,
    ) {
        art.want(cover);
        let Some(cover) = cover else {
            art.show(seed, None);
            return;
        };
        let size = thumbnail_size(logical, art.scale_factor());
        let weak = art.downgrade();
        let ready = Box::new(move |texture: &gdk::Texture| {
            if let Some(art) = weak.upgrade()
                && art.wanted() == Some(cover)
            {
                art.show(seed, Some(texture.clone()));
            }
        });
        let texture = self.get(cover, size, ready);
        art.show(seed, texture);
    }

    /// Shows `cover` on `art` if its texture is cached, else the placeholder
    /// for `seed`, without asking for it.
    pub(crate) fn show_cached(
        &self,
        art: &CoverArt,
        cover: Option<CoverKey>,
        seed: u64,
        logical: i32,
    ) {
        art.want(cover);
        let size = thumbnail_size(logical, art.scale_factor());
        let texture = cover.and_then(|c| self.textures.borrow().get(&(c, size)).cloned());
        art.show(seed, texture);
    }

    /// A thumbnail the library announced.
    pub(crate) fn thumbnail_ready(
        self: &Rc<Self>,
        cover: CoverKey,
        size: u32,
        path: Option<PathBuf>,
    ) {
        let key = (cover, size);
        if !self.waiting.borrow().contains_key(&key) {
            return;
        }
        match path {
            Some(path) => self.load(key, path),
            None => {
                self.waiting.borrow_mut().remove(&key);
            }
        }
    }

    fn load(self: &Rc<Self>, key: Key, path: PathBuf) {
        if !self.loading.borrow_mut().insert(key) {
            return;
        }
        let Some(backend) = self.backend.borrow().clone() else {
            self.loading.borrow_mut().remove(&key);
            return;
        };
        let me = self.clone();
        glib::spawn_future_local(async move {
            let texture = backend
                .work(move || gdk::Texture::from_filename(&path).ok())
                .await
                .flatten();
            me.loading.borrow_mut().remove(&key);
            let readies = me.waiting.borrow_mut().remove(&key).unwrap_or_default();
            if let Some(texture) = texture {
                me.store(key, texture.clone());
                for ready in readies {
                    ready(&texture);
                }
            }
        });
    }

    fn store(&self, key: Key, texture: gdk::Texture) {
        let size = |t: &gdk::Texture| {
            usize::try_from(t.width()).unwrap_or(0) * usize::try_from(t.height()).unwrap_or(0) * 4
        };
        let mut order = self.order.borrow_mut();
        let mut textures = self.textures.borrow_mut();
        let mut bytes = self.bytes.get() + size(&texture);
        match textures.insert(key, texture) {
            Some(old) => bytes -= size(&old),
            None => order.push_back(key),
        }
        while bytes > CACHE_BYTES && order.len() > 1 {
            if let Some(old) = order.pop_front().and_then(|k| textures.remove(&k)) {
                bytes -= size(&old);
            }
        }
        self.bytes.set(bytes);
    }

    /// Forgets the requests for `cover` at `size` (a recycled tile).
    pub(crate) fn cancel(&self, cover: CoverKey, size: u32) {
        self.waiting.borrow_mut().remove(&(cover, size));
    }

    /// The visibility pass: asks again for the covers in `visible` that are still
    /// waiting, newest last so they are made first, and lets the library drop the
    /// requests nobody waits for any more.
    pub(crate) fn prioritise(self: &Rc<Self>, visible: &[(CoverKey, u32)]) {
        for key in visible {
            if self.waiting.borrow().contains_key(key) && !self.loading.borrow().contains(key) {
                self.ask(*key);
            }
        }
        let Some(backend) = self.backend.borrow().clone() else {
            return;
        };
        let waiting = self.waiting.borrow();
        backend
            .library()
            .retain_thumbnails(&mut |cover, size| waiting.contains_key(&(*cover, size)));
    }

    /// Whether a cover is still waiting (for tests and the visibility pass).
    pub(crate) fn is_waiting(&self, cover: CoverKey, size: u32) -> bool {
        self.waiting.borrow().contains_key(&(cover, size))
    }
}

#[cfg(test)]
mod tests {
    use super::thumbnail_size;

    #[test]
    fn sizes_follow_the_display() {
        assert_eq!(thumbnail_size(40, 1), 48);
        assert_eq!(thumbnail_size(40, 2), 96);
        assert_eq!(thumbnail_size(152, 1), 192);
        assert_eq!(thumbnail_size(184, 2), 384);
    }
}
