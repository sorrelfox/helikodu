-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Support for plain search.
--
-- `tracks.artists_fold` holds the folded names of the track's artists and album
-- artists, separated by U+001F. Short queries scan it and ranking compares against it,
-- so neither needs a lookup in `track_people` per track. The library writes it together
-- with `track_people`.
--
-- A track that leaves the library leaves both search indexes, also when it goes because
-- its file or folder was removed (`ON DELETE CASCADE`). Inserts and tag changes refill a
-- track's rows from Rust, after its people, album and genres are written.

ALTER TABLE tracks ADD COLUMN artists_fold TEXT;

UPDATE tracks SET artists_fold = (
    SELECT group_concat(p.fold, char(31))
    FROM track_people tp JOIN people p ON p.id = tp.person_id
    WHERE tp.track_id = tracks.id AND tp.role IN (0, 1)
);

CREATE TRIGGER tracks_search_delete AFTER DELETE ON tracks
BEGIN
    DELETE FROM tracks_fts WHERE rowid = old.id;
    DELETE FROM tracks_tri WHERE rowid = old.id;
END;
