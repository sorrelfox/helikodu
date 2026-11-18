// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving a settings file written by an older version to the current layout.
//!
//! Each [`Migration`] takes the TOML of one version to the next: entry `n` of
//! [`MIGRATIONS`] turns version `n` into version `n + 1`. A released migration is
//! never changed; a change of layout is a new entry, and
//! [`Settings::VERSION`](helikodu_model::settings::Settings::VERSION) goes
//! up by one with it. Migrations work on the TOML table, before it is read into the
//! typed settings, so they can rename, move and reinterpret keys.

use toml::{Table, Value};

/// One step: changes `table`, written by one version, into the next version's
/// layout.
pub type Migration = fn(&mut Table);

/// The migrations of this build, version 0 first. There are as many as
/// [`Settings::VERSION`](helikodu_model::settings::Settings::VERSION) (a unit test
/// checks it).
pub const MIGRATIONS: &[Migration] = &[from_0];

/// Migrates `table` with `steps` (normally [`MIGRATIONS`]) to the version after the
/// last step and returns the version it was written with. A table without a
/// `version` is version 0. A table newer than the steps know is left as it is.
pub fn migrate(table: &mut Table, steps: &[Migration]) -> Result<u32, String> {
    let from = match table.get("version") {
        None => 0,
        Some(Value::Integer(version)) => u32::try_from(*version)
            .map_err(|_| format!("the settings version {version} is not valid"))?,
        Some(other) => {
            return Err(format!(
                "the settings version is {}, not a number",
                other.type_str()
            ));
        }
    };
    let target = u32::try_from(steps.len()).unwrap_or(u32::MAX);
    if from >= target {
        return Ok(from);
    }
    for step in &steps[from as usize..] {
        step(table);
    }
    table.insert("version".into(), Value::Integer(i64::from(target)));
    Ok(from)
}

/// Version 0, before versions were recorded, held only the `[output]` table, in the
/// layout version 1 still uses.
fn from_0(_table: &mut Table) {}

#[cfg(test)]
mod tests {
    use helikodu_model::settings::Settings;

    use super::*;

    #[test]
    fn this_build_migrates_to_its_own_version() {
        assert_eq!(MIGRATIONS.len(), Settings::VERSION as usize);
    }

    #[test]
    fn a_file_without_a_version_is_version_0() {
        let mut table: Table = toml::from_str("[output]\nbackend = \"alsa\"\n").unwrap();
        assert_eq!(migrate(&mut table, MIGRATIONS), Ok(0));
        assert_eq!(table["version"], Value::Integer(1));
        let settings: Settings = table.try_into().unwrap();
        assert_eq!(settings.output.backend.to_string(), "alsa");
    }

    /// A made-up history: version 1 called the seek step `seek_seconds` in
    /// `[player]`, version 2 moved it to `[playback]` as `seek_step_s`, version 3
    /// counted it in tenths.
    #[test]
    fn steps_run_in_order_from_the_file_version() {
        fn rename(table: &mut Table) {
            if let Some(Value::Table(player)) = table.remove("player") {
                let mut playback = Table::new();
                if let Some(value) = player.get("seek_seconds") {
                    playback.insert("seek_step_s".into(), value.clone());
                }
                table.insert("playback".into(), Value::Table(playback));
            }
        }
        fn tenths(table: &mut Table) {
            if let Some(Value::Table(playback)) = table.get_mut("playback")
                && let Some(Value::Integer(s)) = playback.get_mut("seek_step_s")
            {
                *s *= 10;
            }
        }
        let steps: &[Migration] = &[from_0, rename, tenths];

        let mut table: Table = toml::from_str("version = 1\n[player]\nseek_seconds = 2\n").unwrap();
        assert_eq!(migrate(&mut table, steps), Ok(1));
        assert_eq!(table["version"], Value::Integer(3));
        assert_eq!(table["playback"]["seek_step_s"], Value::Integer(20));

        let mut current: Table =
            toml::from_str("version = 3\n[playback]\nseek_step_s = 7\n").unwrap();
        assert_eq!(migrate(&mut current, steps), Ok(3));
        assert_eq!(current["playback"]["seek_step_s"], Value::Integer(7));

        let mut newer: Table = toml::from_str("version = 9\n").unwrap();
        assert_eq!(migrate(&mut newer, steps), Ok(9));
        assert_eq!(newer["version"], Value::Integer(9));
    }

    #[test]
    fn a_bad_version_is_an_error() {
        let mut table: Table = toml::from_str("version = \"one\"\n").unwrap();
        assert!(migrate(&mut table, MIGRATIONS).is_err());
        let mut table: Table = toml::from_str("version = -1\n").unwrap();
        assert!(migrate(&mut table, MIGRATIONS).is_err());
    }
}
