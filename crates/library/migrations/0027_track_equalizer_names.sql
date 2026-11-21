-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The name of a track's equalizer, as the tag editor shows it: the preset it
-- was taken from, "Copy of" the main equalizer's preset, or the name of an
-- imported AutoEQ file. NULL for an equalizer set without a name.
ALTER TABLE track_equalizers ADD COLUMN name TEXT;
