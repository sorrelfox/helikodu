-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The sound features of the track analysis, for finding tracks that sound
-- alike: a format byte (1), then 34 little-endian 32-bit floats (see
-- `SoundFeatures` in the model crate). NULL for a silent track, a failed
-- analysis and analyses made before version 2, which counts as not analysed.
ALTER TABLE track_analysis ADD COLUMN features BLOB;
