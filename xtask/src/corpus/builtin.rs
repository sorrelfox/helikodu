// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The steps `xtask` performs itself: `copy`, `truncate`, `garbage` and `empty`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::recipe::Sample;
use super::signal;

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
        other => bail!("`{other}` is not a built-in step"),
    };
    fs::write(output, bytes).with_context(|| format!("could not write {}", output.display()))
}
