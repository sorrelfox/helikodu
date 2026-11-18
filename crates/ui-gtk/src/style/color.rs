// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Colours and the accent maths: OKLab mixing, WCAG contrast, and the accent
//! variants for each surface.

use std::fmt;

use gtk::gdk;

use super::tokens::{self, Palette, accent, glass};

/// An sRGB colour with alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rgba {
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
    pub(crate) a: f32,
}

impl Rgba {
    /// An opaque colour.
    pub(crate) const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// A colour with alpha.
    pub(crate) const fn new(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub(crate) const WHITE: Self = Self::rgb(0xFF, 0xFF, 0xFF);
    pub(crate) const BLACK: Self = Self::rgb(0, 0, 0);

    /// The same colour with another alpha.
    pub(crate) const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    fn from_unit(r: f64, g: f64, b: f64) -> Self {
        let channel = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::rgb(channel(r), channel(g), channel(b))
    }

    fn unit(self) -> [f64; 3] {
        [self.r, self.g, self.b].map(|c| f64::from(c) / 255.0)
    }

    /// `#RRGGBB`, without alpha.
    pub(crate) fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// The CSS spelling, with alpha when it is not opaque.
    pub(crate) fn css(self) -> String {
        if self.a >= 1.0 {
            self.hex()
        } else {
            format!(
                "rgba({},{},{},{})",
                self.r,
                self.g,
                self.b,
                (f64::from(self.a) * 1000.0).round() / 1000.0
            )
        }
    }

    /// For drawing.
    pub(crate) fn to_gdk(self) -> gdk::RGBA {
        gdk::RGBA::new(
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
            self.a,
        )
    }

    /// From GTK's or the portal's unit floats.
    pub(crate) fn from_floats(r: f64, g: f64, b: f64) -> Option<Self> {
        let ok = |v: f64| (0.0..=1.0).contains(&v);
        (ok(r) && ok(g) && ok(b)).then(|| Self::from_unit(r, g, b))
    }

    /// This colour, with its alpha, painted over the opaque `under`.
    #[cfg(test)]
    pub(crate) fn over(self, under: Self) -> Self {
        let a = f64::from(self.a);
        let [r1, g1, b1] = self.unit();
        let [r0, g0, b0] = under.unit();
        Self::from_unit(r0 + (r1 - r0) * a, g0 + (g1 - g0) * a, b0 + (b1 - b0) * a)
    }

    /// Relative luminance (WCAG 2).
    pub(crate) fn luminance(self) -> f64 {
        let [r, g, b] = self.unit().map(to_linear);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// WCAG contrast ratio against `other`.
    pub(crate) fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn to_oklab(self) -> [f64; 3] {
        let [r, g, b] = self.unit().map(to_linear);
        let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
        let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
        let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;
        let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
        [
            0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
            1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
            0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
        ]
    }

    fn from_oklab([l, a, b]: [f64; 3]) -> Self {
        let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
        let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
        let s_ = l - 0.089_484_177_5 * a - 1.291_485_548_0 * b;
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        let r = 4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s;
        let g = -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s;
        let b = -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s;
        Self::from_unit(from_linear(r), from_linear(g), from_linear(b))
    }

    /// OKLab interpolation; `t` is the share of `other`.
    pub(crate) fn mix(self, other: Self, t: f64) -> Self {
        let a = self.to_oklab();
        let b = other.to_oklab();
        Self::from_oklab([0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t))
    }

    /// Steps toward white (on a dark surface) or black (on a light one) by 5 % in
    /// OKLab until the contrast against `surface` reaches `min`.
    pub(crate) fn ensure(self, surface: Self, min: f64) -> Self {
        let target = if surface.luminance() < 0.18 {
            Self::WHITE
        } else {
            Self::BLACK
        };
        let mut color = self;
        for _ in 0..20 {
            if color.contrast(surface) >= min {
                break;
            }
            color = color.mix(target, 0.05);
        }
        color
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.css())
    }
}

fn to_linear(v: f64) -> f64 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// The accent variants of one scheme (section "Accent" of the design guide).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AccentSet {
    /// The accent as the desktop or the setting gave it.
    pub(crate) raw: Rgba,
    /// Play button, waveform, progress, tab underline, playing ring, switches.
    pub(crate) fill: Rgba,
    /// The playing row and matched search text.
    pub(crate) text: Rgba,
    /// Selected rows: the accent with alpha.
    pub(crate) sel: Rgba,
    /// Text on the fill.
    pub(crate) on_fill: Rgba,
    /// Text on the raw accent (primary buttons).
    pub(crate) on_raw: Rgba,
    /// The fill on glass surfaces.
    pub(crate) glass_fill: Rgba,
}

fn on_accent(fill: Rgba) -> Rgba {
    if accent::ON_ACCENT.contrast(fill) >= 4.5 {
        accent::ON_ACCENT
    } else {
        Rgba::WHITE
    }
}

impl AccentSet {
    /// The variants of `raw` for the dark (`dark == true`) or light scheme;
    /// `high_contrast` raises the selection alpha by 10 points.
    pub(crate) fn new(raw: Rgba, dark: bool, high_contrast: bool) -> Self {
        let raw = raw.with_alpha(1.0);
        let palette = if dark { tokens::DARK } else { tokens::LIGHT };
        let (fill, text, alpha) = if dark {
            (
                raw.ensure(palette.pane, 3.0),
                raw.mix(Rgba::WHITE, 0.18).ensure(palette.hover, 4.5),
                accent::DARK_SEL_ALPHA,
            )
        } else {
            (
                raw.mix(Rgba::BLACK, 0.20).ensure(palette.pane, 3.0),
                raw.mix(Rgba::BLACK, 0.45).ensure(palette.hover, 4.5),
                accent::LIGHT_SEL_ALPHA,
            )
        };
        let alpha = if high_contrast { alpha + 0.10 } else { alpha };
        Self {
            raw,
            fill,
            text,
            sel: raw.with_alpha(alpha as f32),
            on_fill: on_accent(fill),
            on_raw: on_accent(raw),
            glass_fill: raw.ensure(glass::BG, 3.0),
        }
    }
}

/// The palette of a scheme, adjusted for high contrast.
pub(crate) fn palette(dark: bool, high_contrast: bool) -> Palette {
    let mut palette = if dark { tokens::DARK } else { tokens::LIGHT };
    if high_contrast {
        palette.line = palette.line2;
        palette.t3 = palette.t2;
    }
    palette
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rgba, b: Rgba) -> bool {
        let d = |x: u8, y: u8| x.abs_diff(y);
        d(a.r, b.r) <= 2 && d(a.g, b.g) <= 2 && d(a.b, b.b) <= 2
    }

    #[test]
    fn the_worked_examples_hold() {
        for example in accent::EXAMPLES {
            let dark = AccentSet::new(example.accent, true, false);
            let light = AccentSet::new(example.accent, false, false);
            let checks = [
                ("dark fill", dark.fill, example.dark_fill),
                ("dark text", dark.text, example.dark_text),
                ("light fill", light.fill, example.light_fill),
                ("light text", light.text, example.light_text),
                (
                    "dark sel on pane",
                    dark.sel.over(tokens::DARK.pane),
                    example.dark_sel_on_pane,
                ),
                (
                    "light sel on pane",
                    light.sel.over(tokens::LIGHT.pane),
                    example.light_sel_on_pane,
                ),
            ];
            for (what, got, want) in checks {
                assert!(
                    close(got, want),
                    "{}: {what} is {}, expected {}",
                    example.name,
                    got.hex(),
                    want.hex()
                );
            }
        }
    }

    #[test]
    fn variants_have_their_contrast() {
        // Dark and light system accents, and a very light one.
        for hex in [
            (0x35, 0x84, 0xE4),
            (0x3D, 0xAE, 0xE9),
            (0x91, 0x41, 0xAC),
            (0xF6, 0xD3, 0x2D),
        ] {
            let raw = Rgba::rgb(hex.0, hex.1, hex.2);
            for dark in [true, false] {
                let set = AccentSet::new(raw, dark, false);
                let palette = palette(dark, false);
                assert!(set.fill.contrast(palette.pane) >= 3.0, "{}", raw.hex());
                assert!(set.text.contrast(palette.hover) >= 4.5, "{}", raw.hex());
                assert!(set.on_fill.contrast(set.fill) >= 4.5 || set.on_fill == Rgba::WHITE);
                assert!(set.glass_fill.contrast(glass::BG) >= 3.0);
            }
        }
    }

    /// Text in the token colours stays readable (WCAG AA, 4.5:1) on the
    /// surfaces it is drawn on, in both schemes and with high contrast.
    #[test]
    fn text_tokens_have_their_contrast() {
        for dark in [true, false] {
            for high in [false, true] {
                let p = palette(dark, high);
                let surfaces = [
                    ("win", p.win),
                    ("pane", p.pane),
                    ("side", p.side),
                    ("field", p.field),
                    ("hover", p.hover),
                    ("menu", p.menu),
                ];
                let mut texts = vec![("t1", p.t1), ("t2", p.t2), ("t3", p.t3)];
                for (text, color) in texts.drain(..) {
                    for (surface, under) in surfaces {
                        let ratio = color.contrast(under);
                        assert!(
                            ratio >= 4.5,
                            "{text} on {surface} (dark {dark}, high contrast {high}): {ratio:.2}"
                        );
                    }
                }
                // The seg fill is a hover and checked background: t1 and t2
                // only are drawn on it.
                assert!(p.t2.contrast(p.seg) >= 4.5, "t2 on seg, dark {dark}");
                // The accent text on the selection tint, as in the playing row.
                for raw in [accent::DEFAULT, Rgba::rgb(0x35, 0x84, 0xE4)] {
                    let set = AccentSet::new(raw, dark, high);
                    let selected = set.sel.over(p.pane);
                    assert!(
                        set.text.contrast(selected) >= 4.5 || set.text.contrast(p.hover) >= 4.5
                    );
                    assert!(
                        p.t1.contrast(selected) >= 4.5,
                        "t1 on the selection, dark {dark}"
                    );
                }
            }
        }
        for (text, color) in [("t1", glass::T1), ("t2", glass::T2), ("t3", glass::T3)] {
            for (surface, under) in [
                ("bg", glass::BG),
                ("popover", glass::POPOVER),
                ("hover", glass::HOVER),
                ("tile", glass::TILE),
            ] {
                let ratio = color.contrast(under);
                assert!(ratio >= 4.5, "glass {text} on {surface}: {ratio:.2}");
            }
        }
        assert!(glass::T4.contrast(glass::BG) >= 4.5);
    }

    #[test]
    fn oklab_round_trips() {
        for c in [Rgba::rgb(0xF0, 0xB2, 0x3C), Rgba::rgb(1, 2, 3), Rgba::WHITE] {
            assert!(close(Rgba::from_oklab(c.to_oklab()), c));
        }
    }

    #[test]
    fn high_contrast_strengthens_lines_and_selection() {
        let p = palette(true, true);
        assert_eq!(p.line, tokens::DARK.line2);
        assert_eq!(p.t3, tokens::DARK.t2);
        let set = AccentSet::new(accent::DEFAULT, true, true);
        assert!((set.sel.a - 0.26).abs() < 1e-6);
    }
}
