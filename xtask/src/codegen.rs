// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask codegen`: files generated from the design tokens.
//!
//! `crates/ui-gtk/resources/style/tokens.json` holds the colours, type styles and
//! sizes of the desktop design. Two files are generated from it and committed:
//!
//! - `crates/ui-gtk/src/style/tokens.rs`: the `Palette` of each colour scheme and
//!   every other token as Rust constants, for widgets that draw themselves;
//! - `crates/ui-gtk/resources/style/tokens.css`: the same colours as CSS custom
//!   properties on `.hk-dark`, `.hk-light`, `.hk-glass` and `.hk`, for the
//!   stylesheet.
//!
//! With `--check` nothing is written; the command fails when a generated file
//! differs from what it would write. `cargo xtask ci` runs it that way.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

const USAGE: &str = "\
usage: cargo xtask codegen [--check]

  --check   write nothing; fail if a generated file is out of date
";

const TOKENS_JSON: &str = "crates/ui-gtk/resources/style/tokens.json";
const TOKENS_RS: &str = "crates/ui-gtk/src/style/tokens.rs";
const TOKENS_CSS: &str = "crates/ui-gtk/resources/style/tokens.css";

// Split so that licence scanners do not take these for this file's own tags.
const HEADER: &str = concat!(
    "SPDX-",
    "FileCopyrightText: 2026 sorrelfox and Helikodu contributors"
);
const LICENCE: &str = concat!("SPDX-", "License-Identifier: GPL-3.0-or-later");

/// Which glass token stands in for each scheme variable inside `.hk-glass`.
const GLASS_VARIABLES: &[(&str, &str)] = &[
    ("win", "bg"),
    ("side", "node"),
    ("pane", "bg"),
    ("field", "popover"),
    ("hover", "hover"),
    ("seg", "seg"),
    ("line", "line"),
    ("line2", "line2"),
    ("menu", "popover"),
    ("t1", "t1"),
    ("t2", "t2"),
    ("t3", "t3"),
];

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown option `{other}` for `codegen`\n\n{USAGE}"),
        }
    }
    let root = crate::repository_root()?;
    let outputs = generate(&root)?;
    let mut stale = Vec::new();
    for (relative, contents) in &outputs {
        let path = root.join(relative);
        let current = fs::read_to_string(&path).unwrap_or_default();
        if current == *contents {
            continue;
        }
        if check {
            stale.push(relative.display().to_string());
        } else {
            fs::write(&path, contents)
                .with_context(|| format!("could not write {}", path.display()))?;
            println!("wrote {}", relative.display());
        }
    }
    if !stale.is_empty() {
        bail!(
            "generated files are out of date: {}; run `cargo xtask codegen`",
            stale.join(", ")
        );
    }
    if check {
        println!("generated files are up to date");
    }
    Ok(())
}

/// The generated files, as paths relative to the repository root and contents.
fn generate(root: &Path) -> Result<Vec<(PathBuf, String)>> {
    let path = root.join(TOKENS_JSON);
    let text =
        fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
    let tokens: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let tokens = Tokens::new(&tokens)?;
    let rust = format_rust(root, &tokens.rust()?)?;
    let css = tokens.css()?;
    Ok(vec![
        (PathBuf::from(TOKENS_RS), rust),
        (PathBuf::from(TOKENS_CSS), css),
    ])
}

/// Formats generated Rust with the repository's rustfmt settings, so that
/// `cargo fmt --check` accepts the committed file.
fn format_rust(root: &Path, source: &str) -> Result<String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout", "--config-path"])
        .arg(root.join("rustfmt.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start rustfmt")?;
    child
        .stdin
        .take()
        .context("rustfmt has no input")?
        .write_all(source.as_bytes())
        .context("could not write to rustfmt")?;
    let output = child.wait_with_output().context("rustfmt did not finish")?;
    if !output.status.success() {
        bail!(
            "rustfmt rejected the generated code: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("rustfmt printed invalid UTF-8")
}

struct Tokens<'a> {
    color: &'a Map<String, Value>,
    typography: &'a Map<String, Value>,
    metric: &'a Map<String, Value>,
}

impl<'a> Tokens<'a> {
    fn new(root: &'a Value) -> Result<Self> {
        Ok(Self {
            color: object(root, "color")?,
            typography: object(root, "type")?,
            metric: object(root, "metric")?,
        })
    }

    fn scheme(&self, name: &str) -> Result<&'a Map<String, Value>> {
        object_in(self.color, name)
    }

    fn accent(&self) -> Result<&'a Map<String, Value>> {
        object_in(self.color, "accent")
    }

    /// The colour keys of the dark scheme; the light scheme must have the same.
    fn palette_keys(&self) -> Result<Vec<String>> {
        let dark = self.scheme("dark")?;
        let light = self.scheme("light")?;
        let keys: Vec<String> = dark
            .iter()
            .filter(|(_, v)| v.as_str().and_then(parse_color).is_some())
            .map(|(k, _)| k.clone())
            .collect();
        for key in &keys {
            if light
                .get(key)
                .and_then(Value::as_str)
                .and_then(parse_color)
                .is_none()
            {
                bail!("color.light has no colour `{key}`, but color.dark has");
            }
        }
        Ok(keys)
    }

    /// The worked example of the default accent: the variants the stylesheet
    /// starts with before the run-time provider replaces them.
    fn default_example(&self) -> Result<(Color, &'a Map<String, Value>)> {
        let accent = self.accent()?;
        let default = string_in(accent, "default")?;
        let color = parse_color(default).with_context(|| format!("bad accent `{default}`"))?;
        let examples = object_in(accent, "examples")?;
        let example = examples
            .iter()
            .find(|(name, _)| name.to_uppercase().ends_with(&default.to_uppercase()))
            .and_then(|(_, v)| v.as_object())
            .context("color.accent.examples has no example for the default accent")?;
        Ok((color, example))
    }

    /// The alpha of the selection tint in `scheme`, from the rule text
    /// "accent at N% alpha".
    fn selection_alpha(&self, scheme: &str) -> Result<f64> {
        let rules = object_in(self.accent()?, "rules")?;
        let rule = string_in(rules, &format!("{scheme}.sel"))?;
        let percent = rule
            .strip_prefix("accent at ")
            .and_then(|r| r.strip_suffix("% alpha"))
            .and_then(|n| n.trim().parse::<f64>().ok())
            .with_context(|| format!("cannot read the alpha from the rule `{rule}`"))?;
        Ok(percent / 100.0)
    }

    fn rust(&self) -> Result<String> {
        let mut out = String::new();
        writeln!(out, "// {HEADER}\n// {LICENCE}\n")?;
        out.push_str(
            "//! The design tokens as Rust constants.\n//!\n\
             //! Generated by `cargo xtask codegen` from `resources/style/tokens.json`. Do not\n\
             //! edit: change the JSON and run the command again. `tokens.css` holds the same\n\
             //! colours for the stylesheet.\n\n\
             #![allow(dead_code)]\n\n\
             use super::color::Rgba;\n\n",
        );

        // Palette.
        let keys = self.palette_keys()?;
        out.push_str(
            "/// The surface and text colours of one colour scheme.\n\
             #[derive(Debug, Clone, Copy, PartialEq)]\npub(crate) struct Palette {\n",
        );
        for key in &keys {
            writeln!(out, "    pub(crate) {}: Rgba,", snake(key))?;
        }
        out.push_str("}\n\n");
        for scheme in ["dark", "light"] {
            let map = self.scheme(scheme)?;
            writeln!(
                out,
                "/// The {scheme} palette.\npub(crate) const {}: Palette = Palette {{",
                scheme.to_uppercase()
            )?;
            for key in &keys {
                let value = string_in(map, key)?;
                writeln!(out, "    {}: {},", snake(key), rust_color(value)?)?;
            }
            out.push_str("};\n\n");
        }
        for scheme in ["dark", "light"] {
            let map = self.scheme(scheme)?;
            let mut rest = Map::new();
            for (k, v) in map {
                if !keys.contains(k) {
                    rest.insert(k.clone(), v.clone());
                }
            }
            if !rest.is_empty() {
                writeln!(
                    out,
                    "/// The {scheme} scheme's tokens that are not colours.\npub(crate) mod {scheme}_extra {{"
                )?;
                constants(&mut out, &rest)?;
                out.push_str("}\n\n");
            }
        }

        // Glass and signal colours.
        for group in ["glass", "signal"] {
            writeln!(
                out,
                "/// `color.{group}`.\npub(crate) mod {group} {{\n    use super::Rgba;"
            )?;
            constants(&mut out, object_in(self.color, group)?)?;
            out.push_str("}\n\n");
        }

        // Accent.
        let accent = self.accent()?;
        out.push_str("/// `color.accent`.\npub(crate) mod accent {\n    use super::Rgba;\n");
        writeln!(
            out,
            "    /// The accent used when the desktop reports none.\n    pub(crate) const DEFAULT: Rgba = {};",
            rust_color(string_in(accent, "default")?)?
        )?;
        writeln!(
            out,
            "    /// Text on an accent fill, when it has enough contrast.\n    pub(crate) const ON_ACCENT: Rgba = {};",
            rust_color(string_in(accent, "onAccent")?)?
        )?;
        writeln!(
            out,
            "    /// The alpha of the selection tint in the dark scheme.\n    pub(crate) const DARK_SEL_ALPHA: f64 = {:?};",
            self.selection_alpha("dark")?
        )?;
        writeln!(
            out,
            "    /// The alpha of the selection tint in the light scheme.\n    pub(crate) const LIGHT_SEL_ALPHA: f64 = {:?};",
            self.selection_alpha("light")?
        )?;
        out.push_str(
            "    /// A worked example of the accent rules.\n    #[derive(Debug, Clone, Copy)]\n    pub(crate) struct Example {\n        pub(crate) name: &'static str,\n        pub(crate) accent: Rgba,\n",
        );
        let examples = object_in(accent, "examples")?;
        let fields: Vec<String> = examples
            .values()
            .next()
            .and_then(Value::as_object)
            .context("color.accent.examples is empty")?
            .keys()
            .cloned()
            .collect();
        for field in &fields {
            writeln!(out, "        pub(crate) {}: Rgba,", snake(field))?;
        }
        out.push_str("    }\n");
        writeln!(
            out,
            "    /// The worked examples, to test the accent maths against.\n    pub(crate) const EXAMPLES: [Example; {}] = [",
            examples.len()
        )?;
        for (name, example) in examples {
            let example = example
                .as_object()
                .with_context(|| format!("example `{name}` is not an object"))?;
            let (label, hex) = name
                .rsplit_once(' ')
                .with_context(|| format!("example `{name}` does not end with its colour"))?;
            writeln!(
                out,
                "        Example {{ name: {label:?}, accent: {},",
                rust_color(hex)?
            )?;
            for field in &fields {
                writeln!(
                    out,
                    "            {}: {},",
                    snake(field),
                    rust_color(string_in(example, field)?)?
                )?;
            }
            out.push_str("        },\n");
        }
        out.push_str("    ];\n}\n\n");

        // Cover placeholders.
        out.push_str(
            "/// `color.coverPlaceholder`: the generated covers of albums without art.\npub(crate) mod cover_placeholder {\n    use super::Rgba;\n",
        );
        constants(&mut out, object_in(self.color, "coverPlaceholder")?)?;
        out.push_str("}\n\n");

        // Typography.
        out.push_str(
            "/// A type style: size in logical pixels, weight, letter spacing in em.\n\
             #[derive(Debug, Clone, Copy, PartialEq)]\npub(crate) struct TypeStyle {\n\
             pub(crate) size: f64,\npub(crate) weight: u16,\npub(crate) letter_spacing_em: f64,\n\
             pub(crate) line_height: f64,\npub(crate) uppercase: bool,\npub(crate) mono: bool,\n}\n\n",
        );
        let line_height = self
            .typography
            .get("lineHeight")
            .and_then(Value::as_f64)
            .context("type.lineHeight is missing")?;
        out.push_str(
            "/// `type.styles`.\npub(crate) mod typography {\n    use super::TypeStyle;\n",
        );
        for (name, style) in object_in(self.typography, "styles")? {
            let style = style
                .as_object()
                .with_context(|| format!("type style `{name}` is not an object"))?;
            let number = |key: &str| style.get(key).and_then(Value::as_f64);
            let spacing = style
                .get("letterSpacing")
                .and_then(Value::as_str)
                .map(|s| {
                    s.trim_end_matches("em")
                        .parse::<f64>()
                        .with_context(|| format!("bad letter spacing `{s}` in `{name}`"))
                })
                .transpose()?
                .unwrap_or(0.0);
            writeln!(
                out,
                "    pub(crate) const {}: TypeStyle = TypeStyle {{ size: {:?}, weight: {}, letter_spacing_em: {:?}, line_height: {:?}, uppercase: {}, mono: {} }};",
                screaming(name),
                number("size").with_context(|| format!("type style `{name}` has no size"))?,
                number("weight").unwrap_or(400.0).round(),
                spacing,
                number("lineHeight").unwrap_or(line_height),
                style.get("transform").and_then(Value::as_str) == Some("uppercase"),
                style.get("family").and_then(Value::as_str) == Some("mono"),
            )?;
        }
        out.push_str("}\n\n");

        // Metrics.
        out.push_str("/// `metric`: sizes in logical pixels.\npub(crate) mod metric {\n");
        constants(&mut out, self.metric)?;
        out.push_str("}\n");
        Ok(out)
    }

    fn css(&self) -> Result<String> {
        let mut out = String::new();
        writeln!(out, "/* {HEADER} */\n/* {LICENCE} */\n")?;
        out.push_str(
            "/* The design tokens as CSS custom properties.\n\
             \x20* Generated by `cargo xtask codegen` from tokens.json. Do not edit: change the\n\
             \x20* JSON and run the command again. The accent variables are the defaults for the\n\
             \x20* default accent; the application replaces them at run time. */\n",
        );
        let (accent, example) = self.default_example()?;
        let dark = self.scheme("dark")?;
        for scheme in ["dark", "light"] {
            let map = self.scheme(scheme)?;
            writeln!(out, "\n.hk-{scheme} {{")?;
            for (key, value) in map {
                if key.starts_with('_') {
                    continue;
                }
                let value = value
                    .as_str()
                    .with_context(|| format!("color.{scheme}.{key} is not a string"))?;
                writeln!(out, "  --hk-{}: {value};", kebab(key))?;
            }
            let alpha = self.selection_alpha(scheme)?;
            writeln!(out, "  --hk-accent: {};", accent.hex())?;
            for variant in ["fill", "text"] {
                let value = string_in(example, &format!("{scheme}.{variant}"))?;
                writeln!(out, "  --hk-accent-{variant}: {};", normalise(value)?)?;
            }
            writeln!(out, "  --hk-sel: {};", accent.css_with_alpha(alpha))?;
            writeln!(
                out,
                "  --hk-on-accent: {};",
                normalise(string_in(self.accent()?, "onAccent")?)?
            )?;
            out.push_str("}\n");
        }

        let glass = object_in(self.color, "glass")?;
        out.push_str("\n.hk-glass {\n");
        for (variable, token) in GLASS_VARIABLES {
            writeln!(
                out,
                "  --hk-{variable}: {};",
                normalise(string_in(glass, token)?)?
            )?;
        }
        writeln!(out, "  --hk-zebra: {};", string_in(dark, "zebra")?)?;
        writeln!(out, "  --hk-accent-fill: {};", accent.hex())?;
        writeln!(out, "  --hk-accent-text: {};", accent.hex())?;
        writeln!(
            out,
            "  --hk-sel: {};",
            accent.css_with_alpha(self.selection_alpha("dark")?)
        )?;
        out.push_str("}\n");

        out.push_str("\n.hk {\n");
        for (key, value) in glass {
            if key.starts_with('_') {
                continue;
            }
            if let Some(color) = value.as_str().filter(|v| parse_color(v).is_some()) {
                writeln!(out, "  --hk-glass-{}: {};", kebab(key), normalise(color)?)?;
            }
        }
        for (state, value) in object_in(self.color, "signal")? {
            let Some(map) = value.as_object() else {
                continue;
            };
            for (key, value) in map {
                if let Some(color) = value.as_str().filter(|v| parse_color(v).is_some()) {
                    writeln!(
                        out,
                        "  --hk-signal-{}-{}: {};",
                        kebab(state),
                        kebab(key),
                        normalise(color)?
                    )?;
                }
            }
        }
        out.push_str("}\n");
        Ok(out)
    }
}

/// Writes the entries of `map` as constants, objects as nested modules.
fn constants(out: &mut String, map: &Map<String, Value>) -> Result<()> {
    for (key, value) in map {
        if key.starts_with('_') {
            continue;
        }
        match value {
            Value::Object(inner) => {
                writeln!(
                    out,
                    "pub(crate) mod {} {{\n#[allow(unused_imports)]\nuse super::*;",
                    module_name(key)
                )?;
                constants(out, inner)?;
                out.push_str("}\n");
            }
            _ => {
                let (ty, literal) = rust_value(value)
                    .with_context(|| format!("cannot express `{key}` as a constant"))?;
                writeln!(
                    out,
                    "pub(crate) const {}: {ty} = {literal};",
                    screaming(key)
                )?;
            }
        }
    }
    Ok(())
}

/// The Rust type and literal of a JSON value.
fn rust_value(value: &Value) -> Result<(String, String)> {
    Ok(match value {
        Value::Bool(b) => ("bool".into(), b.to_string()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                (
                    "i32".into(),
                    i32::try_from(i).context("number out of range")?.to_string(),
                )
            } else {
                (
                    "f64".into(),
                    format!("{:?}", n.as_f64().context("bad number")?),
                )
            }
        }
        Value::String(s) => match parse_color(s) {
            Some(_) => ("Rgba".into(), rust_color(s)?),
            None => ("&str".into(), format!("{s:?}")),
        },
        Value::Array(items) => {
            let mut parts = Vec::new();
            let mut types = Vec::new();
            let any_float = items
                .iter()
                .any(|v| v.as_number().is_some_and(|n| !n.is_i64()));
            for item in items {
                let (ty, literal) = if any_float {
                    let f = item.as_f64().context("mixed array")?;
                    ("f64".to_string(), format!("{f:?}"))
                } else {
                    rust_value(item)?
                };
                types.push(ty);
                parts.push(literal);
            }
            let ty = types.first().cloned().unwrap_or_else(|| "i32".into());
            if types.iter().any(|t| *t != ty) {
                bail!("an array mixes types");
            }
            (
                format!("[{ty}; {}]", parts.len()),
                format!("[{}]", parts.join(", ")),
            )
        }
        Value::Null | Value::Object(_) => bail!("no constant form"),
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
    a: f64,
}

impl Color {
    fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    fn css_with_alpha(self, alpha: f64) -> String {
        format!("rgba({},{},{},{})", self.r, self.g, self.b, trim(alpha))
    }
}

/// `#RRGGBB` or `rgba(r,g,b,a)`.
fn parse_color(text: &str) -> Option<Color> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        return Some(Color {
            r: channel(0)?,
            g: channel(2)?,
            b: channel(4)?,
            a: 1.0,
        });
    }
    let inner = text.strip_prefix("rgba(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return None;
    }
    Some(Color {
        r: parts[0].parse().ok()?,
        g: parts[1].parse().ok()?,
        b: parts[2].parse().ok()?,
        a: parts[3].parse().ok()?,
    })
}

fn rust_color(text: &str) -> Result<String> {
    let c = parse_color(text).with_context(|| format!("`{text}` is not a colour"))?;
    Ok(if c.a >= 1.0 {
        format!("Rgba::rgb(0x{:02X}, 0x{:02X}, 0x{:02X})", c.r, c.g, c.b)
    } else {
        format!(
            "Rgba::new(0x{:02X}, 0x{:02X}, 0x{:02X}, {:?})",
            c.r, c.g, c.b, c.a
        )
    })
}

/// A colour in the stylesheet's spelling: upper-case hex, or `rgba()` as given.
fn normalise(text: &str) -> Result<String> {
    let c = parse_color(text).with_context(|| format!("`{text}` is not a colour"))?;
    Ok(if c.a >= 1.0 {
        c.hex()
    } else {
        format!("rgba({},{},{},{})", c.r, c.g, c.b, trim(c.a))
    })
}

fn trim(value: f64) -> String {
    let text = format!("{value:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Splits camelCase, dotted and spaced keys into lower-case words.
fn words(key: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for c in key.chars() {
        if c == '.' || c == ' ' || c == '-' || c == '_' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else if c.is_ascii_uppercase() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            current.push(c.to_ascii_lowercase());
        } else if c.is_ascii_alphanumeric() {
            current.push(c);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn snake(key: &str) -> String {
    words(key).join("_")
}

fn screaming(key: &str) -> String {
    snake(key).to_uppercase()
}

fn kebab(key: &str) -> String {
    words(key).join("-")
}

fn module_name(key: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "as", "box", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
        "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "try",
        "type", "unsafe", "use", "where", "while", "yield",
    ];
    let name = snake(key);
    if KEYWORDS.contains(&name.as_str()) {
        format!("r#{name}")
    } else {
        name
    }
}

fn object<'a>(value: &'a Value, key: &str) -> Result<&'a Map<String, Value>> {
    value
        .get(key)
        .and_then(Value::as_object)
        .with_context(|| format!("tokens.json has no object `{key}`"))
}

fn object_in<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a Map<String, Value>> {
    map.get(key)
        .and_then(Value::as_object)
        .with_context(|| format!("tokens.json has no object `{key}`"))
}

fn string_in<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("tokens.json has no string `{key}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(snake("menuShadow"), "menu_shadow");
        assert_eq!(screaming("dark.selOnPane"), "DARK_SEL_ON_PANE");
        assert_eq!(kebab("bitPerfect"), "bit-perfect");
        assert_eq!(module_name("type"), "r#type");
    }

    #[test]
    fn colours() {
        assert_eq!(
            parse_color("rgba(255,255,255,0.024)").map(|c| c.a),
            Some(0.024)
        );
        assert_eq!(normalise("#f0b23c").unwrap(), "#F0B23C");
        assert!(parse_color("0 14px 34px rgba(0,0,0,0.45)").is_none());
    }

    #[test]
    fn committed_files_are_current() {
        let root = crate::repository_root().unwrap();
        for (relative, contents) in generate(&root).unwrap() {
            let current = fs::read_to_string(root.join(&relative)).unwrap();
            assert!(
                current == contents,
                "{} is out of date; run `cargo xtask codegen`",
                relative.display()
            );
        }
    }
}
