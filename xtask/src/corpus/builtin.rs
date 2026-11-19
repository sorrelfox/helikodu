// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The steps `xtask` performs itself: `copy`, `truncate`, `garbage`, `empty`;
//! `write`, which writes one of the long-tail files the project makes itself (the
//! same writers as the tests use, from `crates/testkit/src/longtail.rs`); `tag`,
//! which writes tags into a copy of its input (see [`super::tagging`]); and
//! `text`, which writes a text such as a CUE sheet in a given charset.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::recipe::Sample;
use super::signal;

// The writers depend on nothing but `std`; they are shared with the tests without
// making `xtask` depend on a workspace crate.
#[allow(dead_code, unreachable_pub)]
#[path = "../../../crates/testkit/src/longtail.rs"]
mod longtail;

pub(crate) fn run(recipe: &Sample, input: Option<&Path>, output: &Path) -> Result<()> {
    let read_input = || -> Result<Vec<u8>> {
        let input = input.with_context(|| format!("`{}` needs an input", recipe.tool))?;
        fs::read(input).with_context(|| format!("could not read {}", input.display()))
    };
    let bytes = match recipe.tool.as_str() {
        "copy" => read_input()?,
        "empty" => Vec::new(),
        "truncate" => {
            let keep = recipe.keep.context("`truncate` needs `keep`")?;
            if !(0.0..1.0).contains(&keep) {
                bail!("`keep` must be at least 0 and less than 1");
            }
            let mut bytes = read_input()?;
            let length = (bytes.len() as f64 * keep) as usize;
            bytes.truncate(length);
            bytes
        }
        "garbage" => {
            let keep = recipe.keep_bytes.context("`garbage` needs `keep_bytes`")?;
            let mut bytes = read_input()?;
            let keep = usize::try_from(keep).unwrap_or(usize::MAX).min(bytes.len());
            let noise = signal::noise_bytes(recipe.seed.unwrap_or(1), bytes.len() - keep);
            bytes.truncate(keep);
            bytes.extend(noise);
            bytes
        }
        "write" => match recipe.content.as_deref() {
            Some("tracker-module") => longtail::tracker_module(),
            Some("nsf") => longtail::nsf_file(),
            Some("psid") => longtail::psid_file(),
            Some("midi") => longtail::midi_file(),
            Some("soundfont") => longtail::soundfont(),
            Some(other) => bail!("`write` cannot make `{other}`"),
            None => bail!("`write` needs `content`"),
        },
        "tag" => super::tagging::apply(&read_input()?, &recipe.tag)?,
        "text" => {
            let text = recipe.text.as_deref().context("`text` needs `text`")?;
            super::charset::encode(text, recipe.charset.as_deref().unwrap_or("utf-8"))?
        }
        other => bail!("`{other}` is not a built-in step"),
    };
    fs::write(output, bytes).with_context(|| format!("could not write {}", output.display()))
}
