// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keeping the search indexes in step with the tracks.
//!
//! Both indexes are contentless, so a track's rows are deleted and inserted again
//! whenever its tags or its path change. The text comes from the database itself (the
//! folded columns, people, album, genres and path), so the index always holds what the
//! tables hold. Deleting a track removes its rows through a trigger.

use rusqlite::Connection;

/// The text of the word index, per track; `{filter}` selects the tracks.
const WORD_ROWS: &str = "
    SELECT t.id, t.title_fold,
        (SELECT group_concat(p.fold, ' ') FROM track_people tp JOIN people p ON p.id = tp.person_id
         WHERE tp.track_id = t.id AND tp.role = 0),
        a.title_fold,
        (SELECT group_concat(p.fold, ' ') FROM track_people tp JOIN people p ON p.id = tp.person_id
         WHERE tp.track_id = t.id AND tp.role = 1),
        (SELECT group_concat(g.fold, ' ') FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
         WHERE tg.track_id = t.id),
        (SELECT group_concat(p.fold, ' ') FROM track_people tp JOIN people p ON p.id = tp.person_id
         WHERE tp.track_id = t.id AND tp.role = 2),
        hk_fold(d.path_text || '/' || f.name_text)
    FROM tracks t
    JOIN files f ON f.id = t.file_id
    JOIN dirs d ON d.id = f.dir_id
    LEFT JOIN albums a ON a.id = t.album_id
    {filter}";

/// The text of the trigram index, per track: title, artists and album artists, album.
const TRIGRAM_ROWS: &str = "
    SELECT t.id, t.title_fold,
        (SELECT group_concat(p.fold, ' ') FROM track_people tp JOIN people p ON p.id = tp.person_id
         WHERE tp.track_id = t.id AND tp.role IN (0, 1)),
        a.title_fold
    FROM tracks t
    LEFT JOIN albums a ON a.id = t.album_id
    {filter}";

fn fill(c: &Connection, filter: &str, id: Option<i64>) -> rusqlite::Result<()> {
    let words = format!(
        "INSERT INTO tracks_fts (rowid, title, artists, album, album_artist, genre, composer, path) {}",
        WORD_ROWS.replace("{filter}", filter)
    );
    let trigrams = format!(
        "INSERT INTO tracks_tri (rowid, title, artists, album) {}",
        TRIGRAM_ROWS.replace("{filter}", filter)
    );
    for sql in [words, trigrams] {
        let mut statement = c.prepare_cached(&sql)?;
        match id {
            Some(id) => statement.execute([id])?,
            None => statement.execute([])?,
        };
    }
    Ok(())
}

/// Writes the index rows of `track` again.
pub(crate) fn reindex_track(c: &Connection, track: i64) -> rusqlite::Result<()> {
    c.prepare_cached("DELETE FROM tracks_fts WHERE rowid = ?1")?
        .execute([track])?;
    c.prepare_cached("DELETE FROM tracks_tri WHERE rowid = ?1")?
        .execute([track])?;
    fill(c, "WHERE t.id = ?1", Some(track))
}

/// Writes the index rows of every track of the file `file` again.
pub(crate) fn reindex_file(c: &Connection, file: i64) -> rusqlite::Result<()> {
    let tracks = c
        .prepare_cached("SELECT id FROM tracks WHERE file_id = ?1")?
        .query_map([file], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for track in tracks {
        reindex_track(c, track)?;
    }
    Ok(())
}

/// Empties both indexes and fills them from the tracks.
pub(crate) fn rebuild(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "INSERT INTO tracks_fts (tracks_fts) VALUES ('delete-all');
         INSERT INTO tracks_tri (tracks_tri) VALUES ('delete-all');",
    )?;
    fill(c, "", None)
}
