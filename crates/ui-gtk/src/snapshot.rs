// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rendering a window to a PNG file, for screenshot checks and tests. Works
//! headless: it uses the window's own renderer.

use std::path::Path;

use gtk::prelude::*;

/// Why a window could not be rendered.
#[derive(Debug)]
pub struct RenderError(pub String);

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RenderError {}

/// Renders `window` as it is now into the PNG file `path`.
pub fn save_png(window: &impl IsA<gtk::Window>, path: &Path) -> Result<(), RenderError> {
    let window = window.upcast_ref::<gtk::Window>();
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    let (w, h) = (window.width(), window.height());
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot
        .to_node()
        .ok_or_else(|| RenderError("the window drew nothing".into()))?;
    let renderer = window
        .renderer()
        .ok_or_else(|| RenderError("the window is not realised".into()))?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32);
    let texture = renderer.render_texture(&node, Some(&bounds));
    texture
        .save_to_png(path)
        .map_err(|e| RenderError(format!("cannot write {}: {e}", path.display())))
}
