-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Labels and the user's fields. Neither is ever written to a file.
--
-- Labels. `labels` holds each label once: `name` as the user last wrote it, `fold`
-- its folded form (`hk_fold()`), which tells two labels apart. A label is given to
-- tracks, albums or artists, each keyed so that it outlives a rescan and a move:
-- - `label_tracks` by the track's identity, as statistics and ratings are;
-- - `label_albums` by the album's `album_key` (the MusicBrainz release ID, or the
--   folded album artist, title and year), which a rescan gives the album again;
-- - `label_artists` by the folded name of the artist (`people.fold`).
-- A label of an album or an artist counts for every track of the album, or every
-- track the artist plays or is album artist of. A label nothing has is removed.
--
-- The user's fields. `user_fields` holds the fields the user defined, in the
-- user's order (`position`): `kind` 0 is a computed field, whose `definition` is
-- an expression of the expression language; `kind` 1 a custom field, whose
-- `definition` is a tag key in upper case. `name` is in lower case.

CREATE TABLE labels (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    fold       TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE label_tracks (
    label_id    INTEGER NOT NULL REFERENCES labels ON DELETE CASCADE,
    identity_id INTEGER NOT NULL REFERENCES identities ON DELETE CASCADE,
    PRIMARY KEY (label_id, identity_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX label_tracks_identity ON label_tracks (identity_id);

CREATE TABLE label_albums (
    label_id  INTEGER NOT NULL REFERENCES labels ON DELETE CASCADE,
    album_key TEXT NOT NULL,
    PRIMARY KEY (label_id, album_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX label_albums_key ON label_albums (album_key);

CREATE TABLE label_artists (
    label_id    INTEGER NOT NULL REFERENCES labels ON DELETE CASCADE,
    artist_fold TEXT NOT NULL,
    PRIMARY KEY (label_id, artist_fold)
) STRICT, WITHOUT ROWID;
CREATE INDEX label_artists_fold ON label_artists (artist_fold);

CREATE TABLE user_fields (
    name       TEXT PRIMARY KEY,
    kind       INTEGER NOT NULL CHECK (kind IN (0, 1)),
    definition TEXT NOT NULL,
    position   INTEGER NOT NULL
) STRICT;
