-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Imports of statistics, ratings and playlists from other players and from an
-- export of Helikodu's own user data, statistics carried from a replaced file to
-- its replacement, and the statistics written into the files.
--
-- `import_baselines` keeps, per import source (a player and the path of its
-- library file) and identity, the play and skip counts that source had when it
-- was last imported, so that importing it again adds only the plays made since.
--
-- `import_runs` has one row per applied import (`kind` 0: another player; 1:
-- user data; 2: statistics carried to a replacement). `undone` is 1 once the run
-- was undone. What a run changed is kept so that it can be undone:
-- `import_run_stats` the statistics of each identity before and after (`existed`
-- 0 when there was no `stats` row before); `import_run_baselines` the baseline
-- rows before (counts NULL when there was none); `import_run_added` the time a
-- track was added, before and after; `import_run_rows` rows the run made or
-- moved (`kind` 0: a playlist made; 1: a saved search made; 2: a history entry
-- added; 3: a history entry moved from `from_identity`; 4: a playlist item moved
-- from `from_identity`).
--
-- `stats_in_files` keeps, per file, the play count and rating last written into
-- its tags, so that only the files whose statistics changed since are written.

CREATE TABLE import_baselines (
    source      TEXT NOT NULL,
    identity_id INTEGER NOT NULL REFERENCES identities ON DELETE CASCADE,
    play_count  INTEGER NOT NULL,
    skip_count  INTEGER NOT NULL,
    PRIMARY KEY (source, identity_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX import_baselines_identity ON import_baselines (identity_id);

CREATE TABLE import_runs (
    id     INTEGER PRIMARY KEY,
    kind   INTEGER NOT NULL,
    source TEXT NOT NULL,
    at     INTEGER NOT NULL,
    undone INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE import_run_stats (
    run_id              INTEGER NOT NULL REFERENCES import_runs ON DELETE CASCADE,
    identity_id         INTEGER NOT NULL REFERENCES identities ON DELETE CASCADE,
    existed             INTEGER NOT NULL,
    before_play_count   INTEGER NOT NULL,
    before_skip_count   INTEGER NOT NULL,
    before_first_played INTEGER,
    before_last_played  INTEGER,
    before_last_skipped INTEGER,
    before_rating       INTEGER,
    before_loved        INTEGER,
    after_play_count    INTEGER NOT NULL,
    after_skip_count    INTEGER NOT NULL,
    after_first_played  INTEGER,
    after_last_played   INTEGER,
    after_last_skipped  INTEGER,
    after_rating        INTEGER,
    after_loved         INTEGER,
    PRIMARY KEY (run_id, identity_id)
) STRICT;
CREATE INDEX import_run_stats_identity ON import_run_stats (identity_id);

CREATE TABLE import_run_baselines (
    run_id      INTEGER NOT NULL REFERENCES import_runs ON DELETE CASCADE,
    identity_id INTEGER NOT NULL REFERENCES identities ON DELETE CASCADE,
    play_count  INTEGER,
    skip_count  INTEGER,
    PRIMARY KEY (run_id, identity_id)
) STRICT;
CREATE INDEX import_run_baselines_identity ON import_run_baselines (identity_id);

CREATE TABLE import_run_added (
    run_id   INTEGER NOT NULL REFERENCES import_runs ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks ON DELETE CASCADE,
    before   INTEGER NOT NULL,
    after    INTEGER NOT NULL,
    PRIMARY KEY (run_id, track_id)
) STRICT;
CREATE INDEX import_run_added_track ON import_run_added (track_id);

CREATE TABLE import_run_rows (
    run_id        INTEGER NOT NULL REFERENCES import_runs ON DELETE CASCADE,
    kind          INTEGER NOT NULL,
    row_id        INTEGER NOT NULL,
    from_identity INTEGER,
    PRIMARY KEY (run_id, kind, row_id)
) STRICT;

CREATE TABLE stats_in_files (
    file_id    INTEGER PRIMARY KEY REFERENCES files ON DELETE CASCADE,
    play_count INTEGER,
    rating     INTEGER
) STRICT;
