-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Media kinds the owner chose.
--
-- The scanner gives every track a kind from its file (audio from a video file, an
-- audiobook by its extension, music otherwise). When the owner picks another kind,
-- it is kept here for the track's identity, so that it survives rescans and moves,
-- and the scanner writes it in place of the kind it found. When copies are merged
-- into one identity, the chosen kind moves with them.

CREATE TABLE chosen_kinds (
    identity_id INTEGER PRIMARY KEY REFERENCES identities ON DELETE CASCADE,
    kind        INTEGER NOT NULL
) STRICT;
