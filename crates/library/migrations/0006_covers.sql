-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Covers, and the loudness values of the tags.
--
-- A cover is stored once per content hash (`covers.hash`, SHA-256 of the encoded
-- picture), wherever it was found. `covers.source` is 0 for a picture embedded in an
-- audio file (`source_path` is that file, `source_index` the picture's position
-- among the file's pictures) and 1 for an image file (`source_path` is the image).
-- That is where the picture was first seen; the thumbnail cache also tries the
-- other tracks and folders that have it.
--
-- `tracks.cover_id` is the track's embedded cover and `tracks.cover_index` its
-- position in the file. `dirs.cover_id` is the image file a folder's tracks found
-- (in the folder, or above a disc folder) and `dirs.cover_path` that file.
-- `albums.cover_id` is decided from its tracks: the embedded cover most of them
-- have, or else the folder image most of them have.
--
-- Triggers remove a cover when nothing refers to it any more.
--
-- Every file that was read is read again by the next scan, so that the covers and
-- the loudness columns (`rg_*`) are filled in.

ALTER TABLE covers ADD COLUMN source_index INTEGER;
-- `jpeg`, `png` or `webp`.
ALTER TABLE covers ADD COLUMN format TEXT;
ALTER TABLE covers ADD COLUMN byte_len INTEGER;

ALTER TABLE tracks ADD COLUMN cover_id INTEGER REFERENCES covers ON DELETE SET NULL;
ALTER TABLE tracks ADD COLUMN cover_index INTEGER;
CREATE INDEX tracks_cover ON tracks (cover_id) WHERE cover_id IS NOT NULL;

ALTER TABLE dirs ADD COLUMN cover_id INTEGER REFERENCES covers ON DELETE SET NULL;
ALTER TABLE dirs ADD COLUMN cover_path BLOB;
CREATE INDEX dirs_cover ON dirs (cover_id) WHERE cover_id IS NOT NULL;

-- Orphans.

CREATE TRIGGER tracks_cover_orphan_delete AFTER DELETE ON tracks
WHEN old.cover_id IS NOT NULL
BEGIN
    DELETE FROM covers WHERE id = old.cover_id
        AND NOT EXISTS (SELECT 1 FROM tracks WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM dirs WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM albums WHERE cover_id = old.cover_id);
END;

CREATE TRIGGER tracks_cover_orphan_update AFTER UPDATE OF cover_id ON tracks
WHEN old.cover_id IS NOT NULL AND old.cover_id IS NOT new.cover_id
BEGIN
    DELETE FROM covers WHERE id = old.cover_id
        AND NOT EXISTS (SELECT 1 FROM tracks WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM dirs WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM albums WHERE cover_id = old.cover_id);
END;

CREATE TRIGGER dirs_cover_orphan_delete AFTER DELETE ON dirs
WHEN old.cover_id IS NOT NULL
BEGIN
    DELETE FROM covers WHERE id = old.cover_id
        AND NOT EXISTS (SELECT 1 FROM tracks WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM dirs WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM albums WHERE cover_id = old.cover_id);
END;

CREATE TRIGGER dirs_cover_orphan_update AFTER UPDATE OF cover_id ON dirs
WHEN old.cover_id IS NOT NULL AND old.cover_id IS NOT new.cover_id
BEGIN
    DELETE FROM covers WHERE id = old.cover_id
        AND NOT EXISTS (SELECT 1 FROM tracks WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM dirs WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM albums WHERE cover_id = old.cover_id);
END;

CREATE TRIGGER albums_cover_orphan_delete AFTER DELETE ON albums
WHEN old.cover_id IS NOT NULL
BEGIN
    DELETE FROM covers WHERE id = old.cover_id
        AND NOT EXISTS (SELECT 1 FROM tracks WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM dirs WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM albums WHERE cover_id = old.cover_id);
END;

CREATE TRIGGER albums_cover_orphan_update AFTER UPDATE OF cover_id ON albums
WHEN old.cover_id IS NOT NULL AND old.cover_id IS NOT new.cover_id
BEGIN
    DELETE FROM covers WHERE id = old.cover_id
        AND NOT EXISTS (SELECT 1 FROM tracks WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM dirs WHERE cover_id = old.cover_id)
        AND NOT EXISTS (SELECT 1 FROM albums WHERE cover_id = old.cover_id);
END;

-- No covers were written before this version.
UPDATE albums SET cover_id = NULL;
DELETE FROM covers;

-- Read every file again.
UPDATE files SET mtime_ns = NULL WHERE coalesce(scan_state, 0) = 0;
