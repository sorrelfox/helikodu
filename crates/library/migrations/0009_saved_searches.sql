-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Saved searches: a name and the text of a query, in the order the user keeps them.
--
-- The query is stored as text in its canonical form (the query tree printed), so that a
-- later version reads it with its own parser. `position` is 0, 1, 2, … without gaps.

CREATE TABLE saved_searches (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    query       TEXT NOT NULL,
    position    INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    modified_at INTEGER NOT NULL
) STRICT;
CREATE INDEX saved_searches_position ON saved_searches (position);
