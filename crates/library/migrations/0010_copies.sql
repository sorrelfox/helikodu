-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Copies: byte-identical files in the library.
--
-- Files.
-- - `dev` is the device number beside `inode`, so that two paths to one file (a hard
--   link, a bind mount, a library folder reached through a symbolic link) are told
--   apart from two copies.
-- - `added_at` is when the file entered the library, in seconds since the epoch; the
--   copy that entered first is the main copy of its group. Files read before this
--   migration take the time their first track was added.
-- - `content_hash` is the SHA-256 of the whole file, computed only for files that
--   share their size and quick hash with another file. `hashed_size` and
--   `hashed_mtime_ns` are the file's stamp when it was hashed: the hash is valid
--   only while they match `size` and `mtime_ns`.
-- - `copy_group` is the file's group of copies, NULL for a file without copies.
--
-- `copy_groups` holds one row per group of byte-identical files: their size and
-- content hash, and the main copy (`primary_file`), whose file plays the group's
-- tracks. The main copy changes only when the owner picks another or when it goes
-- missing; a main copy that comes back does not take the role back.
--
-- Tracks.
-- - `is_copy` is 1 for a track of a file that is not the main copy of its group when
--   the main copy has the same track (the same section of the file). With the
--   setting to show one copy, the queries leave these tracks out.
-- - `copies` is the number of other files in the group that hold the same track and
--   are not the same file reached by another path.
--
-- `stats.rated_at` is when the rating was last set, so that merging the statistics
-- of copies can keep the newest rating.

CREATE TABLE copy_groups (
    id           INTEGER PRIMARY KEY,
    size         INTEGER NOT NULL,
    content_hash BLOB NOT NULL,
    primary_file INTEGER REFERENCES files ON DELETE SET NULL,
    UNIQUE (size, content_hash)
) STRICT;

ALTER TABLE files ADD COLUMN dev INTEGER;
ALTER TABLE files ADD COLUMN added_at INTEGER;
ALTER TABLE files ADD COLUMN content_hash BLOB;
ALTER TABLE files ADD COLUMN hashed_size INTEGER;
ALTER TABLE files ADD COLUMN hashed_mtime_ns INTEGER;
ALTER TABLE files ADD COLUMN copy_group INTEGER REFERENCES copy_groups ON DELETE SET NULL;
CREATE INDEX files_copy_group ON files (copy_group) WHERE copy_group IS NOT NULL;
CREATE INDEX files_device_inode ON files (dev, inode) WHERE dev IS NOT NULL;

UPDATE files SET added_at = coalesce(
    (SELECT min(t.added_at) FROM tracks t WHERE t.file_id = files.id),
    unixepoch());

CREATE TRIGGER files_added_at AFTER INSERT ON files
WHEN new.added_at IS NULL
BEGIN
    UPDATE files SET added_at = unixepoch() WHERE id = new.id;
END;

ALTER TABLE tracks ADD COLUMN is_copy INTEGER NOT NULL DEFAULT 0 CHECK (is_copy IN (0, 1));
ALTER TABLE tracks ADD COLUMN copies INTEGER NOT NULL DEFAULT 0;
CREATE INDEX tracks_is_copy ON tracks (file_id) WHERE is_copy = 1;

ALTER TABLE stats ADD COLUMN rated_at INTEGER;
