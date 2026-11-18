-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The undo journal of tag edits.
--
-- `tag_journal` has one row per saved edit (a step), newest with the highest id;
-- `undone` is 1 once the step was undone and it waits to be done again. Each step
-- lists the files it changed in `tag_journal_files` (`path` as bytes), and each file
-- the fields it changed in `tag_journal_values`: `field` is the tag name (`@cover`
-- for the front cover, `@lyrics` for the lyrics), `before` and `after` the values
-- read from the file before and after the write, joined by U+001F; an empty text is
-- no value. For `@cover` the values are the hex SHA-256 of the picture, whose bytes
-- are kept once in `tag_journal_pictures`.

CREATE TABLE tag_journal (
    id     INTEGER PRIMARY KEY,
    at     INTEGER NOT NULL,
    undone INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE tag_journal_files (
    id      INTEGER PRIMARY KEY,
    step_id INTEGER NOT NULL REFERENCES tag_journal ON DELETE CASCADE,
    path    BLOB NOT NULL
);
CREATE INDEX tag_journal_files_step ON tag_journal_files (step_id);

CREATE TABLE tag_journal_values (
    file_id INTEGER NOT NULL REFERENCES tag_journal_files ON DELETE CASCADE,
    field   TEXT NOT NULL,
    before  TEXT NOT NULL,
    after   TEXT NOT NULL,
    PRIMARY KEY (file_id, field)
) WITHOUT ROWID;

CREATE TABLE tag_journal_pictures (
    hash TEXT PRIMARY KEY,
    data BLOB NOT NULL
) WITHOUT ROWID;
