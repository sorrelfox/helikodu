// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the user pinned to the sidebar: albums, artists, playlists and generator
//! playlists, in the order they were pinned. A pin whose album, artist or playlist
//! has gone is not listed.

use helikodu_meta::covers::CoverHash;
use rusqlite::{Connection, params};

use super::{PlaylistId, PlaylistType};
use crate::catalog::{AlbumId, PersonId};
use crate::generators::GeneratorId;

/// Something that can be pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinTarget {
    /// An album.
    Album(AlbumId),
    /// An artist.
    Artist(PersonId),
    /// A playlist or a folder.
    Playlist(PlaylistId),
    /// A generator playlist.
    Generator(GeneratorId),
}

impl PinTarget {
    fn sql(self) -> (i64, i64) {
        match self {
            Self::Album(id) => (0, id.get()),
            Self::Artist(id) => (1, id.get()),
            Self::Playlist(id) => (2, id.0),
            Self::Generator(id) => (3, id.get()),
        }
    }
}

/// One pinned row of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// What is pinned.
    pub target: PinTarget,
    /// Its name.
    pub name: String,
    /// The cover of a pinned album.
    pub cover: Option<CoverHash>,
    /// What a pinned playlist is.
    pub playlist: Option<PlaylistType>,
}

/// Pins `target` after the other pins; nothing changes if it is pinned already.
pub(crate) fn pin(c: &Connection, target: PinTarget) -> rusqlite::Result<()> {
    let (kind, id) = target.sql();
    c.prepare_cached(
        "INSERT INTO pins (kind, target, position)
         VALUES (?1, ?2, (SELECT coalesce(max(position), -1) + 1 FROM pins))
         ON CONFLICT (kind, target) DO NOTHING",
    )?
    .execute(params![kind, id])?;
    Ok(())
}

/// Unpins `target`. Returns whether it was pinned.
pub(crate) fn unpin(c: &Connection, target: PinTarget) -> rusqlite::Result<bool> {
    let (kind, id) = target.sql();
    let removed = c
        .prepare_cached("DELETE FROM pins WHERE kind = ?1 AND target = ?2")?
        .execute(params![kind, id])?;
    Ok(removed > 0)
}

/// The pins in their order.
pub(crate) fn pins(c: &Connection) -> rusqlite::Result<Vec<Pin>> {
    let rows = c
        .prepare_cached(
            "SELECT p.kind, p.target,
                    CASE p.kind WHEN 0 THEN a.title WHEN 1 THEN pe.name WHEN 2 THEN pl.name
                        ELSE g.name END,
                    cv.hash, pl.kind
             FROM pins p
             LEFT JOIN albums a ON p.kind = 0 AND a.id = p.target
             LEFT JOIN covers cv ON cv.id = a.cover_id
             LEFT JOIN people pe ON p.kind = 1 AND pe.id = p.target
             LEFT JOIN playlists pl ON p.kind = 2 AND pl.id = p.target
             LEFT JOIN generators g ON p.kind = 3 AND g.id = p.target
             WHERE (p.kind = 0 AND a.id IS NOT NULL) OR (p.kind = 1 AND pe.id IS NOT NULL)
                OR (p.kind = 2 AND pl.id IS NOT NULL) OR (p.kind = 3 AND g.id IS NOT NULL)
             ORDER BY p.position, p.id",
        )?
        .query_map([], |row| {
            let kind: i64 = row.get(0)?;
            let id: i64 = row.get(1)?;
            let target = match kind {
                0 => PinTarget::Album(AlbumId::from_raw(id)),
                1 => PinTarget::Artist(PersonId::from_raw(id)),
                3 => PinTarget::Generator(GeneratorId::from_raw(id)),
                _ => PinTarget::Playlist(PlaylistId(id)),
            };
            Ok(Pin {
                target,
                name: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                cover: row
                    .get::<_, Option<Vec<u8>>>(3)?
                    .and_then(|hash| CoverHash::from_bytes(&hash)),
                playlist: row.get::<_, Option<i64>>(4)?.map(PlaylistType::from_sql),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
