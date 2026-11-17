-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The library schema, version 1.
--
-- Conventions:
-- - File-system paths and names are BLOBs holding the bytes the kernel returned. The
--   `*_text` columns beside them hold a decoded form for display and search only; it is
--   never used to open a file, and uniqueness and lookups use the bytes.
-- - `*_fold` columns hold the folded form (`hk_fold()`): lower case, compatibility
--   decomposed, Latin, Greek and Cyrillic diacritics removed, NFC. Grouping and search use it.
-- - `*_sort` columns and `genres.name` sort with the `HK_LOCALE` collation, which every
--   connection registers for the interface language. Changing the language rebuilds the
--   indexes that use it (`REINDEX HK_LOCALE`). Unique constraints never use it, so that
--   the rebuild cannot fail.
-- - Times are integers: wall-clock times in seconds since the Unix epoch, file
--   modification times in nanoseconds, durations in milliseconds.
-- - Every table is STRICT, so a path can never be stored as TEXT by mistake.

-- Values about the database file itself, such as the locale the collation indexes were
-- built for.
CREATE TABLE db_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- Library folders chosen by the user.
CREATE TABLE roots (
    id                INTEGER PRIMARY KEY,
    path              BLOB NOT NULL UNIQUE,
    path_text         TEXT NOT NULL,
    -- 0: watched with inotify; 1: periodic rescans (network, FUSE, portal paths).
    watch_mode        INTEGER NOT NULL DEFAULT 0,
    -- 1: the tracks under a root that is not mounted are hidden, not removed.
    mask_when_missing INTEGER NOT NULL DEFAULT 1 CHECK (mask_when_missing IN (0, 1))
) STRICT;

-- Folders below the roots. `path` is the full path.
CREATE TABLE dirs (
    id        INTEGER PRIMARY KEY,
    root_id   INTEGER NOT NULL REFERENCES roots ON DELETE CASCADE,
    path      BLOB NOT NULL UNIQUE,
    path_text TEXT NOT NULL,
    mtime_ns  INTEGER
) STRICT;
CREATE INDEX dirs_root ON dirs (root_id);

-- Files in those folders. `name` is the last path component.
CREATE TABLE files (
    id            INTEGER PRIMARY KEY,
    dir_id        INTEGER NOT NULL REFERENCES dirs ON DELETE CASCADE,
    name          BLOB NOT NULL,
    name_text     TEXT NOT NULL,
    size          INTEGER,
    mtime_ns      INTEGER,
    inode         INTEGER,
    -- A hash of the size and the first and last 64 KiB, for move detection.
    quick_hash    BLOB,
    scan_state    INTEGER,
    scan_error    TEXT,
    -- Set when the file disappeared; the row is removed after the grace period.
    missing_since INTEGER,
    UNIQUE (dir_id, name)
) STRICT;
CREATE INDEX files_move_candidates ON files (size, quick_hash);
CREATE INDEX files_missing ON files (missing_since) WHERE missing_since IS NOT NULL;

-- A track's identity outlives its path: statistics, ratings and history refer to it, so
-- they survive moves and renames.
CREATE TABLE identities (
    id         INTEGER PRIMARY KEY,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE covers (
    id          INTEGER PRIMARY KEY,
    -- The hash of the image data; equal images are stored once.
    hash        BLOB NOT NULL UNIQUE,
    -- 0: embedded; 1: an image file in the folder.
    source      INTEGER,
    source_path BLOB,
    width       INTEGER,
    height      INTEGER
) STRICT;

CREATE TABLE albums (
    id             INTEGER PRIMARY KEY,
    -- The MusicBrainz release ID, or the folded album artist, title and year.
    album_key      TEXT NOT NULL UNIQUE,
    title          TEXT,
    title_sort     TEXT COLLATE HK_LOCALE,
    title_fold     TEXT,
    year           INTEGER,
    is_compilation INTEGER NOT NULL DEFAULT 0 CHECK (is_compilation IN (0, 1)),
    release_type   TEXT,
    mbid           TEXT,
    cover_id       INTEGER REFERENCES covers ON DELETE SET NULL,
    added_at       INTEGER
) STRICT;
CREATE INDEX albums_title_sort ON albums (title_sort);
CREATE INDEX albums_cover ON albums (cover_id);

-- One row per playable item: a file, a CUE sheet track or a subsong.
CREATE TABLE tracks (
    id            INTEGER PRIMARY KEY,
    file_id       INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
    -- The CUE track or subsong number, or 0 for a whole file.
    sub_index     INTEGER NOT NULL DEFAULT 0,
    -- The range within the file for CUE tracks, in sample frames.
    start_frame   INTEGER,
    end_frame     INTEGER,
    identity_id   INTEGER NOT NULL REFERENCES identities,
    album_id      INTEGER REFERENCES albums ON DELETE SET NULL,
    -- 0: music; other values for audiobooks, podcasts, audio from video files and so on.
    kind          INTEGER NOT NULL DEFAULT 0,
    title         TEXT,
    -- The sort tag, or the title when the file has none.
    title_sort    TEXT COLLATE HK_LOCALE,
    title_fold    TEXT,
    track_no      INTEGER,
    track_total   INTEGER,
    disc_no       INTEGER,
    disc_total    INTEGER,
    disc_subtitle TEXT,
    date          TEXT,
    year          INTEGER,
    original_year INTEGER,
    duration_ms   INTEGER,
    codec         TEXT,
    container     TEXT,
    sample_rate   INTEGER,
    bit_depth     INTEGER,
    channels      INTEGER,
    bitrate       INTEGER,
    audio_stream  INTEGER,
    rg_track_gain REAL,
    rg_track_peak REAL,
    rg_album_gain REAL,
    rg_album_peak REAL,
    work          TEXT,
    movement      TEXT,
    movement_no   INTEGER,
    has_lyrics    INTEGER NOT NULL DEFAULT 0 CHECK (has_lyrics IN (0, 1)),
    added_at      INTEGER NOT NULL,
    modified_at   INTEGER,
    UNIQUE (file_id, sub_index)
) STRICT;
CREATE INDEX tracks_identity ON tracks (identity_id);
CREATE INDEX tracks_album ON tracks (album_id, disc_no, track_no);
CREATE INDEX tracks_title_sort ON tracks (title_sort);
CREATE INDEX tracks_added ON tracks (added_at);

-- Artists, composers, performers, conductors and other people, one row per name.
CREATE TABLE people (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL,
    sort_name TEXT COLLATE HK_LOCALE,
    fold      TEXT NOT NULL,
    mbid      TEXT
) STRICT;
CREATE INDEX people_sort_name ON people (sort_name);
CREATE INDEX people_fold ON people (fold);

CREATE TABLE track_people (
    track_id  INTEGER NOT NULL REFERENCES tracks ON DELETE CASCADE,
    person_id INTEGER NOT NULL REFERENCES people,
    -- 0: artist; 1: album artist; 2: composer; 3: performer; 4: conductor; and so on.
    role      INTEGER NOT NULL,
    position  INTEGER NOT NULL,
    PRIMARY KEY (track_id, role, position)
) STRICT, WITHOUT ROWID;
CREATE INDEX track_people_person ON track_people (person_id, role);

CREATE TABLE genres (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    fold TEXT NOT NULL
) STRICT;
CREATE INDEX genres_name_locale ON genres (name COLLATE HK_LOCALE);

CREATE TABLE track_genres (
    track_id INTEGER NOT NULL REFERENCES tracks ON DELETE CASCADE,
    genre_id INTEGER NOT NULL REFERENCES genres,
    position INTEGER NOT NULL,
    PRIMARY KEY (track_id, genre_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX track_genres_genre ON track_genres (genre_id);

-- Every other tag, multi-valued.
CREATE TABLE track_tags (
    track_id INTEGER NOT NULL REFERENCES tracks ON DELETE CASCADE,
    key      TEXT NOT NULL,
    value    TEXT NOT NULL
) STRICT;
CREATE INDEX track_tags_track ON track_tags (track_id);
CREATE INDEX track_tags_key ON track_tags (key, value);

-- Statistics and ratings, per identity.
CREATE TABLE stats (
    identity_id  INTEGER PRIMARY KEY REFERENCES identities,
    play_count   INTEGER NOT NULL DEFAULT 0,
    skip_count   INTEGER NOT NULL DEFAULT 0,
    first_played INTEGER,
    last_played  INTEGER,
    last_skipped INTEGER,
    -- Half stars from 0 to 10. NULL means not rated, which is not the same as zero stars.
    rating       INTEGER CHECK (rating BETWEEN 0 AND 10),
    loved        INTEGER CHECK (loved IN (0, 1)),
    resume_ms    INTEGER,
    flags        INTEGER
) STRICT;

CREATE TABLE history (
    id          INTEGER PRIMARY KEY,
    identity_id INTEGER NOT NULL REFERENCES identities,
    started_at  INTEGER NOT NULL,
    played_ms   INTEGER NOT NULL,
    counted     INTEGER NOT NULL CHECK (counted IN (0, 1)),
    context     TEXT
) STRICT;
CREATE INDEX history_identity ON history (identity_id, started_at);
CREATE INDEX history_started ON history (started_at);

CREATE TABLE playlists (
    id          INTEGER PRIMARY KEY,
    parent_id   INTEGER REFERENCES playlists ON DELETE CASCADE,
    name        TEXT NOT NULL,
    -- 0: folder; 1: static; 2: smart.
    kind        INTEGER NOT NULL,
    query       TEXT,
    sort        TEXT,
    limit_spec  TEXT,
    live        INTEGER,
    pinned      INTEGER,
    locked      INTEGER,
    position    INTEGER,
    created_at  INTEGER,
    modified_at INTEGER
) STRICT;
CREATE INDEX playlists_parent ON playlists (parent_id, position);

CREATE TABLE playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES playlists ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    track_id    INTEGER REFERENCES tracks ON DELETE SET NULL,
    -- The path (bytes) and tags of an entry that is not in the library, to relink it later.
    missing_ref BLOB,
    PRIMARY KEY (playlist_id, position)
) STRICT;
CREATE INDEX playlist_items_track ON playlist_items (track_id);

-- The play queue, its position and the play modes, restored at start-up.
CREATE TABLE session (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
) STRICT;

-- Tag edits and file moves that can be undone.
CREATE TABLE undo_journal (
    id      INTEGER PRIMARY KEY,
    at      INTEGER NOT NULL,
    kind    TEXT NOT NULL,
    payload BLOB NOT NULL
) STRICT;

CREATE TABLE analysis (
    identity_id  INTEGER PRIMARY KEY REFERENCES identities,
    version      INTEGER NOT NULL,
    loudness     REAL,
    true_peak    REAL,
    waveform_ref TEXT
) STRICT;
