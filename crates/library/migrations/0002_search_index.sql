-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The search indexes. The rowid of both tables is `tracks.id`, and both hold the folded
-- text (`hk_fold()`), so that the same folding applies to the index and to the query.
--
-- Both tables are contentless with `contentless_delete=1`, so that a track's row can be
-- deleted and inserted again when its tags change. This needs SQLite 3.43 or newer.

-- Whole words and word prefixes.
CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title, artists, album, album_artist, genre, composer, path,
    content = '', contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- Substrings of three characters or more, for scripts written without spaces between
-- words (Chinese, Japanese, Thai). Shorter queries scan the folded columns instead.
CREATE VIRTUAL TABLE tracks_tri USING fts5(
    title, artists, album,
    content = '', contentless_delete = 1,
    tokenize = 'trigram'
);
