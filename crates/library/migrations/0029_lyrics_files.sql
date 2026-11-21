-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Lyrics files beside a track: `tracks.lyrics_file` is 1 when a `.lrc` or `.txt`
-- file with the base name of the track's file is in its folder. `has_lyrics` stays
-- "the file's tags carry lyrics". Scans keep the flag up to date; it is 0 until
-- the next scan of the track's folder.

ALTER TABLE tracks ADD COLUMN lyrics_file INTEGER NOT NULL DEFAULT 0
    CHECK (lyrics_file IN (0, 1));
