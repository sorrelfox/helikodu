-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The equalizer of a track, which replaces the equalizer of the settings while
-- the track plays. It belongs to the track's identity, like its statistics, so
-- it stays with the recording when the file is moved or retagged.
--
-- `settings` is a JSON object with the fields of the `[dsp.equalizer]` table
-- (switch, mode, preamp, graphic bands, parametric filters). `changed_at` is when
-- it was last set.
CREATE TABLE track_equalizers (
    identity_id INTEGER PRIMARY KEY REFERENCES identities ON DELETE CASCADE,
    settings    TEXT NOT NULL,
    changed_at  INTEGER NOT NULL
) STRICT;
