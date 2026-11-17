-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The album and artist model, sections of files, and masked library folders.
--
-- Albums. A track's album is decided from the album tags stored on the track itself:
-- `album_group` is its group key (the MusicBrainz release ID, or the folded album
-- artist, album title and year), and `album_dir_id` its album folder (its folder, or
-- the folder above a disc folder such as `CD1`). The library forms the albums from the
-- group keys and writes `albums.album_key`: the group key, followed by the album folder
-- when tracks of one key in different folders share a disc and track number.
--
-- People. `album_people` holds an album's artists as the library decided them (from
-- the album artist tags, or from the track artists); compilations without an album
-- artist have none. `track_people.instrument` holds a performer's instrument or part.
--
-- Orphans. Triggers remove an album when its last track leaves it, and a person or
-- genre when nothing refers to it any more.
--
-- Sections. `tracks.section` tells what part of its file a track is: 0 the whole
-- file, 1 a CUE sheet track, 2 a container chapter.
--
-- Masking. `roots.masked` is set while a library folder is gone (an unmounted or
-- unplugged device); together with `mask_when_missing` it hides the folder's tracks.
--
-- Every file that was read is read again by the next scan (its modification time is
-- cleared), so that the new tag fields, chapters and albums are filled in.

ALTER TABLE tracks ADD COLUMN section INTEGER NOT NULL DEFAULT 0;
-- The track artist as credited, for display.
ALTER TABLE tracks ADD COLUMN artist TEXT;
-- The album tags of the track, from which its album is decided.
ALTER TABLE tracks ADD COLUMN album_title TEXT;
ALTER TABLE tracks ADD COLUMN album_fold TEXT;
ALTER TABLE tracks ADD COLUMN album_sort TEXT;
ALTER TABLE tracks ADD COLUMN album_artist TEXT;
ALTER TABLE tracks ADD COLUMN album_artist_sort TEXT;
-- The first track artist without `feat.` and what follows.
ALTER TABLE tracks ADD COLUMN main_artist TEXT;
ALTER TABLE tracks ADD COLUMN compilation INTEGER CHECK (compilation IN (0, 1));
-- The release types, primary first, separated by "; ".
ALTER TABLE tracks ADD COLUMN release_type TEXT;
ALTER TABLE tracks ADD COLUMN release_mbid TEXT;
ALTER TABLE tracks ADD COLUMN release_group_mbid TEXT;
ALTER TABLE tracks ADD COLUMN album_dir_id INTEGER REFERENCES dirs ON DELETE SET NULL;
ALTER TABLE tracks ADD COLUMN album_group TEXT;
CREATE INDEX tracks_album_group ON tracks (album_group) WHERE album_group IS NOT NULL;
CREATE INDEX tracks_album_partition ON tracks (album_dir_id, album_fold)
    WHERE album_fold IS NOT NULL;

-- The album artist as credited, NULL for compilations without one.
ALTER TABLE albums ADD COLUMN artist TEXT;
ALTER TABLE albums ADD COLUMN artist_sort TEXT COLLATE HK_LOCALE;
ALTER TABLE albums ADD COLUMN original_year INTEGER;
ALTER TABLE albums ADD COLUMN disc_total INTEGER;
ALTER TABLE albums ADD COLUMN release_group_mbid TEXT;
CREATE INDEX albums_artist_sort ON albums (artist_sort);
CREATE INDEX albums_added ON albums (added_at);

CREATE TABLE album_people (
    album_id  INTEGER NOT NULL REFERENCES albums ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    person_id INTEGER NOT NULL REFERENCES people,
    PRIMARY KEY (album_id, position)
) STRICT, WITHOUT ROWID;
CREATE INDEX album_people_person ON album_people (person_id);

ALTER TABLE track_people ADD COLUMN instrument TEXT;
CREATE INDEX track_people_role ON track_people (role, person_id);
CREATE INDEX people_mbid ON people (mbid) WHERE mbid IS NOT NULL;

ALTER TABLE roots ADD COLUMN masked INTEGER NOT NULL DEFAULT 0 CHECK (masked IN (0, 1));
-- When the folder was masked, in seconds since the Unix epoch.
ALTER TABLE roots ADD COLUMN masked_since INTEGER;

-- Orphans.

CREATE TRIGGER tracks_album_orphan_delete AFTER DELETE ON tracks
WHEN old.album_id IS NOT NULL
BEGIN
    DELETE FROM albums WHERE id = old.album_id
        AND NOT EXISTS (SELECT 1 FROM tracks WHERE album_id = old.album_id);
END;

CREATE TRIGGER tracks_album_orphan_update AFTER UPDATE OF album_id ON tracks
WHEN old.album_id IS NOT NULL AND old.album_id IS NOT new.album_id
BEGIN
    DELETE FROM albums WHERE id = old.album_id
        AND NOT EXISTS (SELECT 1 FROM tracks WHERE album_id = old.album_id);
END;

CREATE TRIGGER track_people_orphan_delete AFTER DELETE ON track_people
BEGIN
    DELETE FROM people WHERE id = old.person_id
        AND NOT EXISTS (SELECT 1 FROM track_people WHERE person_id = old.person_id)
        AND NOT EXISTS (SELECT 1 FROM album_people WHERE person_id = old.person_id);
END;

CREATE TRIGGER track_people_orphan_update AFTER UPDATE OF person_id ON track_people
WHEN old.person_id IS NOT new.person_id
BEGIN
    DELETE FROM people WHERE id = old.person_id
        AND NOT EXISTS (SELECT 1 FROM track_people WHERE person_id = old.person_id)
        AND NOT EXISTS (SELECT 1 FROM album_people WHERE person_id = old.person_id);
END;

CREATE TRIGGER album_people_orphan_delete AFTER DELETE ON album_people
BEGIN
    DELETE FROM people WHERE id = old.person_id
        AND NOT EXISTS (SELECT 1 FROM track_people WHERE person_id = old.person_id)
        AND NOT EXISTS (SELECT 1 FROM album_people WHERE person_id = old.person_id);
END;

CREATE TRIGGER album_people_orphan_update AFTER UPDATE OF person_id ON album_people
WHEN old.person_id IS NOT new.person_id
BEGIN
    DELETE FROM people WHERE id = old.person_id
        AND NOT EXISTS (SELECT 1 FROM track_people WHERE person_id = old.person_id)
        AND NOT EXISTS (SELECT 1 FROM album_people WHERE person_id = old.person_id);
END;

CREATE TRIGGER track_genres_orphan_delete AFTER DELETE ON track_genres
BEGIN
    DELETE FROM genres WHERE id = old.genre_id
        AND NOT EXISTS (SELECT 1 FROM track_genres WHERE genre_id = old.genre_id);
END;

-- The orphans left by earlier versions.
DELETE FROM albums WHERE NOT EXISTS (SELECT 1 FROM tracks WHERE album_id = albums.id);
DELETE FROM people WHERE NOT EXISTS (SELECT 1 FROM track_people WHERE person_id = people.id);
DELETE FROM genres WHERE NOT EXISTS (SELECT 1 FROM track_genres WHERE genre_id = genres.id);

-- Read every file again.
UPDATE files SET mtime_ns = NULL WHERE coalesce(scan_state, 0) = 0;
