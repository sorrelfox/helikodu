-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Artist photos.
--
-- Both tables are keyed by the artist's folded name (`people.fold`) rather than by
-- the person's row, so that a photo stays when the artist's last track goes and the
-- artist comes back with a later scan.
--
-- `artist_photos` is the photo the user chose in the app. `path` is the copy the
-- library keeps in its own folder (named by the hash); the file the user picked is
-- never changed. `hash` is the SHA-256 of the picture, the key of the thumbnail
-- cache, as for covers.
--
-- `artist_folder_photos` is an `artist.jpg` (`.jpeg`, `.png` or `.webp`) found in
-- the artist's folder: the folder that holds all of the artist's album folders.
-- `size` and `mtime_ns` tell whether the file changed since it was hashed.
--
-- A chosen photo comes before a folder photo.

CREATE TABLE artist_photos (
    fold      TEXT PRIMARY KEY,
    hash      BLOB NOT NULL,
    path      BLOB NOT NULL,
    -- `jpeg`, `png` or `webp`.
    format    TEXT,
    width     INTEGER,
    height    INTEGER,
    byte_len  INTEGER,
    chosen_at INTEGER NOT NULL
) STRICT;
CREATE INDEX artist_photos_hash ON artist_photos (hash);

CREATE TABLE artist_folder_photos (
    fold     TEXT PRIMARY KEY,
    hash     BLOB NOT NULL,
    path     BLOB NOT NULL,
    size     INTEGER NOT NULL,
    mtime_ns INTEGER,
    width    INTEGER,
    height   INTEGER
) STRICT;
CREATE INDEX artist_folder_photos_hash ON artist_folder_photos (hash);
