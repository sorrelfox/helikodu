// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Artist photos, from local files only.
//!
//! An artist's photo is, in this order:
//!
//! 1. the picture the user chose in the app ([`Library::set_artist_photo`]): the
//!    library copies it into its own folder, named by its hash, and never changes
//!    the file that was picked;
//! 2. an `artist.jpg` (`.jpeg`, `.png` or `.webp`, in any case) in the artist's
//!    folder, found by [`Library::find_artist_folder_photos`]. The artist's folder is
//!    the folder that holds all of the album artist's album folders; for an artist
//!    with one album folder, that folder and the one above it are looked in;
//! 3. none: the interface draws the artist's initials.
//!
//! Photos are kept by the artist's folded name, so a chosen photo stays when the
//! artist's last track goes and the artist comes back with a later scan. A photo is
//! identified by the hash of its bytes ([`CoverHash`]), like a cover, so the
//! thumbnail cache and the artwork pool make its thumbnails: the photo's file is
//! one of the places [`Library::cover_locations`] returns for that hash.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use helikodu_meta::covers::{CoverError, CoverHash, CoverLimits, make_thumbnail, probe_picture};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Library;
use crate::catalog::{CoverLocation, PersonId};
use crate::db::{DbError, path_from_blob, path_to_blob};

/// The file names of a folder's artist photo, without the extension.
const STEM: &str = "artist";
/// The extensions of an artist photo.
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// An artist's photo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistPhoto {
    /// The hash of the picture: the key of the thumbnail cache.
    pub hash: CoverHash,
    /// The file it is read from: the library's copy of a chosen photo, or the
    /// image in the artist's folder.
    pub path: PathBuf,
    /// Whether the user chose it in the app, rather than it being found in the
    /// artist's folder.
    pub chosen: bool,
}

/// Why an artist photo could not be set or removed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ArtistPhotoError {
    /// The database failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The library has no such artist.
    #[error("the library has no such artist")]
    UnknownArtist,
    /// The chosen file cannot be read.
    #[error("cannot read {}: {source}", .path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
    /// The chosen file is not a picture the library can show, or is too large.
    #[error(transparent)]
    Picture(#[from] CoverError),
    /// The copy cannot be written into the library's folder.
    #[error("cannot store the photo in {}: {source}", .folder.display())]
    Store {
        /// The library's photo folder.
        folder: PathBuf,
        /// Why.
        source: io::Error,
    },
}

impl Library {
    /// The photo of `person`: the one chosen in the app, else the one found in
    /// their folder; `None` when there is neither, or no such person.
    pub fn artist_photo(&self, person: PersonId) -> Result<Option<ArtistPhoto>, DbError> {
        self.database().read(move |c| {
            let fold: Option<String> = c
                .prepare_cached("SELECT fold FROM people WHERE id = ?1")?
                .query_row([person.0], |row| row.get(0))
                .optional()?;
            match fold {
                Some(fold) => photo_of(c, &fold),
                None => Ok(None),
            }
        })
    }

    /// The photo of every person that has one, as [`artist_photo`](Self::artist_photo)
    /// gives it.
    pub fn artist_photos(&self) -> Result<HashMap<PersonId, ArtistPhoto>, DbError> {
        self.database().read(|c| {
            let mut out = HashMap::new();
            for (table, chosen) in [("artist_folder_photos", false), ("artist_photos", true)] {
                let mut statement = c.prepare_cached(&format!(
                    "SELECT p.id, x.hash, x.path FROM people p JOIN {table} x ON x.fold = p.fold"
                ))?;
                let rows = statement.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })?;
                for row in rows {
                    let (id, hash, path) = row?;
                    if let Some(hash) = CoverHash::from_bytes(&hash) {
                        out.insert(
                            PersonId(id),
                            ArtistPhoto {
                                hash,
                                path: path_from_blob(path),
                                chosen,
                            },
                        );
                    }
                }
            }
            Ok(out)
        })
    }

    /// Makes the picture at `source` the photo of `person`. The picture is checked
    /// (JPEG, PNG or WebP within the [`CoverLimits`]) and copied into `store`, the
    /// library's photo folder, as `<hash>.<ext>`; `source` itself is not changed.
    /// The copy of a photo chosen before is deleted when no artist uses it any more.
    pub fn set_artist_photo(
        &self,
        person: PersonId,
        source: &Path,
        store: &Path,
    ) -> Result<ArtistPhoto, ArtistPhotoError> {
        let fold = self.fold_of(person)?;
        let limits = CoverLimits::default();
        let bytes = read_limited(source, limits.max_bytes).map_err(|e| match e {
            ReadError::Io(source_error) => ArtistPhotoError::Read {
                path: source.to_path_buf(),
                source: source_error,
            },
            ReadError::TooLarge(bytes) => ArtistPhotoError::Picture(CoverError::TooLarge {
                bytes,
                limit: limits.max_bytes as u64,
            }),
        })?;
        let info = probe_picture(&bytes, &limits)?;
        // Decoded once, so that a broken picture is refused now rather than shown
        // as initials later.
        make_thumbnail(&bytes, 48, &limits)?;
        let hash = CoverHash::of(&bytes);
        let path = store.join(format!("{}.{}", hash.to_hex(), info.format.extension()));
        let stored = || -> io::Result<()> {
            fs::create_dir_all(store)?;
            if path.is_file() {
                return Ok(());
            }
            let temp = store.join(format!(".tmp-{}-{}", hash.to_hex(), std::process::id()));
            let written = (|| {
                let mut file = File::create(&temp)?;
                file.write_all(&bytes)?;
                file.sync_data()?;
                fs::rename(&temp, &path)
            })();
            if written.is_err() {
                let _ = fs::remove_file(&temp);
            }
            written
        };
        stored().map_err(|source| ArtistPhotoError::Store {
            folder: store.to_path_buf(),
            source,
        })?;
        let row = (
            fold,
            hash.as_bytes().to_vec(),
            path_to_blob(&path).to_vec(),
            info.format.extension().to_owned(),
            info.width,
            info.height,
            i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            now_seconds(),
        );
        let unused = self.database().writer()?.write(move |c| {
            let (fold, hash, path, format, width, height, byte_len, at) = row;
            let old = stored_path(c, &fold)?;
            c.prepare_cached(
                "INSERT INTO artist_photos
                     (fold, hash, path, format, width, height, byte_len, chosen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (fold) DO UPDATE SET
                     hash = excluded.hash, path = excluded.path, format = excluded.format,
                     width = excluded.width, height = excluded.height,
                     byte_len = excluded.byte_len, chosen_at = excluded.chosen_at",
            )?
            .execute(params![
                fold, hash, path, format, width, height, byte_len, at
            ])?;
            unused_copy(c, old)
        })?;
        if let Some(unused) = unused {
            let _ = fs::remove_file(unused);
        }
        Ok(ArtistPhoto {
            hash,
            path,
            chosen: true,
        })
    }

    /// Forgets the photo chosen for `person` and deletes the library's copy when no
    /// artist uses it any more. Returns the photo shown now: the one in the
    /// artist's folder, or none.
    pub fn remove_artist_photo(
        &self,
        person: PersonId,
    ) -> Result<Option<ArtistPhoto>, ArtistPhotoError> {
        let fold = self.fold_of(person)?;
        let key = fold.clone();
        let unused = self.database().writer()?.write(move |c| {
            let old = stored_path(c, &key)?;
            c.prepare_cached("DELETE FROM artist_photos WHERE fold = ?1")?
                .execute([&key])?;
            unused_copy(c, old)
        })?;
        if let Some(unused) = unused {
            let _ = fs::remove_file(unused);
        }
        Ok(self.database().read(move |c| photo_of(c, &fold))?)
    }

    /// Looks for an `artist.*` picture in the folder of every album artist, and
    /// records what it finds: a picture that is new or changed is read and hashed,
    /// one that is gone is forgotten. Only file metadata is read for pictures that
    /// did not change. Returns the number of artists with a folder photo.
    pub fn find_artist_folder_photos(&self) -> Result<usize, DbError> {
        let (folders, known) = self.database().read(|c| {
            let mut statement = c.prepare_cached(
                "SELECT p.fold, d.path, r.path FROM album_people ap
                 JOIN people p ON p.id = ap.person_id
                 JOIN tracks t ON t.album_id = ap.album_id
                 JOIN files f ON f.id = t.file_id
                 JOIN dirs d ON d.id = coalesce(t.album_dir_id, f.dir_id)
                 JOIN roots r ON r.id = d.root_id
                 WHERE f.missing_since IS NULL
                 GROUP BY p.fold, d.id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    path_from_blob(row.get(1)?),
                    path_from_blob(row.get(2)?),
                ))
            })?;
            let mut folders: HashMap<String, Vec<(PathBuf, PathBuf)>> = HashMap::new();
            for row in rows {
                let (fold, dir, root) = row?;
                folders.entry(fold).or_default().push((dir, root));
            }
            let mut statement =
                c.prepare_cached("SELECT fold, path, size, mtime_ns FROM artist_folder_photos")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    Known {
                        path: path_from_blob(row.get(1)?),
                        size: row.get(2)?,
                        mtime_ns: row.get(3)?,
                    },
                ))
            })?;
            let known = rows.collect::<rusqlite::Result<HashMap<_, _>>>()?;
            Ok((folders, known))
        })?;
        let limits = CoverLimits::default();
        let mut found: HashMap<String, Found> = HashMap::new();
        let mut looked: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
        for (fold, dirs) in &folders {
            let picture = artist_folders(dirs).into_iter().find_map(|folder| {
                looked
                    .entry(folder.clone())
                    .or_insert_with(|| photo_in(&folder))
                    .clone()
            });
            let Some(picture) = picture else { continue };
            let Ok(metadata) = fs::metadata(&picture) else {
                continue;
            };
            let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
            let mtime_ns = metadata.modified().ok().map(nanos);
            if let Some(old) = known.get(fold)
                && old.path == picture
                && old.size == size
                && old.mtime_ns == mtime_ns
            {
                found.insert(fold.clone(), Found::Same);
                continue;
            }
            let Ok(bytes) = read_limited(&picture, limits.max_bytes) else {
                continue;
            };
            let Ok(info) = probe_picture(&bytes, &limits) else {
                continue;
            };
            found.insert(
                fold.clone(),
                Found::New {
                    hash: CoverHash::of(&bytes).as_bytes().to_vec(),
                    path: path_to_blob(&picture).to_vec(),
                    size,
                    mtime_ns,
                    width: info.width,
                    height: info.height,
                },
            );
        }
        let count = found.len();
        let gone: Vec<String> = known
            .into_keys()
            .filter(|fold| !found.contains_key(fold))
            .collect();
        let changed: Vec<(String, Found)> = found
            .into_iter()
            .filter(|(_, f)| matches!(f, Found::New { .. }))
            .collect();
        if gone.is_empty() && changed.is_empty() {
            return Ok(count);
        }
        self.database().writer()?.write(move |c| {
            for fold in &gone {
                c.prepare_cached("DELETE FROM artist_folder_photos WHERE fold = ?1")?
                    .execute([fold])?;
            }
            for (fold, found) in &changed {
                if let Found::New {
                    hash,
                    path,
                    size,
                    mtime_ns,
                    width,
                    height,
                } = found
                {
                    c.prepare_cached(
                        "INSERT INTO artist_folder_photos
                             (fold, hash, path, size, mtime_ns, width, height)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT (fold) DO UPDATE SET
                             hash = excluded.hash, path = excluded.path, size = excluded.size,
                             mtime_ns = excluded.mtime_ns, width = excluded.width,
                             height = excluded.height",
                    )?
                    .execute(params![fold, hash, path, size, mtime_ns, width, height])?;
                }
            }
            Ok(())
        })?;
        Ok(count)
    }

    /// The folded name of `person`.
    fn fold_of(&self, person: PersonId) -> Result<String, ArtistPhotoError> {
        self.database()
            .read(move |c| {
                c.prepare_cached("SELECT fold FROM people WHERE id = ?1")?
                    .query_row([person.0], |row| row.get(0))
                    .optional()
            })?
            .ok_or(ArtistPhotoError::UnknownArtist)
    }
}

/// A folder photo recorded before.
#[derive(Debug)]
struct Known {
    path: PathBuf,
    size: i64,
    mtime_ns: Option<i64>,
}

/// A folder photo found now.
#[derive(Debug)]
enum Found {
    /// The one recorded before, unchanged.
    Same,
    /// New or changed.
    New {
        hash: Vec<u8>,
        path: Vec<u8>,
        size: i64,
        mtime_ns: Option<i64>,
        width: u32,
        height: u32,
    },
}

/// The photo of the artist folded as `fold`: chosen, else from the folder.
fn photo_of(c: &Connection, fold: &str) -> rusqlite::Result<Option<ArtistPhoto>> {
    for (table, chosen) in [("artist_photos", true), ("artist_folder_photos", false)] {
        let found: Option<(Vec<u8>, Vec<u8>)> = c
            .prepare_cached(&format!("SELECT hash, path FROM {table} WHERE fold = ?1"))?
            .query_row([fold], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        if let Some((hash, path)) = found
            && let Some(hash) = CoverHash::from_bytes(&hash)
        {
            return Ok(Some(ArtistPhoto {
                hash,
                path: path_from_blob(path),
                chosen,
            }));
        }
    }
    Ok(None)
}

/// The library's copy of the photo chosen for `fold`.
fn stored_path(c: &Connection, fold: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    c.prepare_cached("SELECT path FROM artist_photos WHERE fold = ?1")?
        .query_row([fold], |row| row.get(0))
        .optional()
}

/// `old`, if no chosen photo uses it any more.
fn unused_copy(c: &Connection, old: Option<Vec<u8>>) -> rusqlite::Result<Option<PathBuf>> {
    let Some(old) = old else { return Ok(None) };
    let used: bool = c
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM artist_photos WHERE path = ?1)")?
        .query_row([&old], |row| row.get(0))?;
    Ok((!used).then(|| path_from_blob(old)))
}

/// Adds the files of the artist photos with `hash` to `out`.
pub(crate) fn locations(
    c: &Connection,
    hash: &CoverHash,
    out: &mut Vec<CoverLocation>,
) -> rusqlite::Result<()> {
    for table in ["artist_photos", "artist_folder_photos"] {
        let mut statement = c.prepare_cached(&format!(
            "SELECT DISTINCT path FROM {table} WHERE hash = ?1"
        ))?;
        let rows =
            statement.query_map([hash.as_bytes().as_slice()], |row| row.get::<_, Vec<u8>>(0))?;
        for row in rows {
            let location = CoverLocation::File {
                path: path_from_blob(row?),
            };
            if !out.contains(&location) {
                out.push(location);
            }
        }
    }
    Ok(())
}

/// Adds the hashes of every artist photo to `out`.
pub(crate) fn hashes(c: &Connection, out: &mut HashSet<CoverHash>) -> rusqlite::Result<()> {
    for table in ["artist_photos", "artist_folder_photos"] {
        let mut statement = c.prepare_cached(&format!("SELECT hash FROM {table}"))?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        for row in rows {
            if let Some(hash) = CoverHash::from_bytes(&row?) {
                out.insert(hash);
            }
        }
    }
    Ok(())
}

/// The folders to look in for an artist photo, best first, given the artist's
/// album folders with their library folders: the folder that holds all album
/// folders, and for a single album folder also the one above it, never above a
/// library folder.
pub(crate) fn artist_folders(dirs: &[(PathBuf, PathBuf)]) -> Vec<PathBuf> {
    let Some(((first, _), rest)) = dirs.split_first() else {
        return Vec::new();
    };
    let mut common = first.clone();
    for (dir, _) in rest {
        while !dir.starts_with(&common) {
            if !common.pop() {
                return Vec::new();
            }
        }
    }
    let inside = |folder: &Path| dirs.iter().all(|(_, root)| folder.starts_with(root));
    let mut out = Vec::new();
    if inside(&common) {
        out.push(common.clone());
        let single = dirs.iter().all(|(dir, _)| *dir == common);
        if single
            && let Some(parent) = common.parent()
            && inside(parent)
        {
            out.push(parent.to_path_buf());
        }
    }
    out
}

/// The artist photo in `folder`, if there is one: `artist.<ext>` in any case,
/// the extensions in the order of [`EXTENSIONS`].
fn photo_in(folder: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(folder).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        if !stem.eq_ignore_ascii_case(STEM) {
            continue;
        }
        let Some(rank) = EXTENSIONS
            .iter()
            .position(|e| e.eq_ignore_ascii_case(extension))
        else {
            continue;
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r)
            && entry
                .file_type()
                .is_ok_and(|t| t.is_file() || t.is_symlink())
        {
            best = Some((rank, entry.path()));
        }
    }
    best.map(|(_, path)| path)
}

#[derive(Debug)]
enum ReadError {
    Io(io::Error),
    TooLarge(u64),
}

/// The file at `path`, unless it is larger than `max_bytes`.
fn read_limited(path: &Path, max_bytes: usize) -> Result<Vec<u8>, ReadError> {
    let file = File::open(path).map_err(ReadError::Io)?;
    let limit = max_bytes as u64;
    let len = file.metadata().map_err(ReadError::Io)?.len();
    if len > limit {
        return Err(ReadError::TooLarge(len));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(ReadError::Io)?;
    if bytes.len() as u64 > limit {
        return Err(ReadError::TooLarge(bytes.len() as u64));
    }
    Ok(bytes)
}

fn nanos(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(list: &[(&str, &str)]) -> Vec<(PathBuf, PathBuf)> {
        list.iter()
            .map(|(d, r)| (PathBuf::from(d), PathBuf::from(r)))
            .collect()
    }

    #[test]
    fn the_artist_folder() {
        assert_eq!(
            artist_folders(&dirs(&[("/m/A/One", "/m"), ("/m/A/Two", "/m")])),
            vec![PathBuf::from("/m/A")]
        );
        assert_eq!(
            artist_folders(&dirs(&[("/m/A/One", "/m"), ("/m/A/One", "/m")])),
            vec![PathBuf::from("/m/A/One"), PathBuf::from("/m/A")]
        );
        // Never above the library folder.
        assert_eq!(
            artist_folders(&dirs(&[("/m/One", "/m")])),
            vec![PathBuf::from("/m/One"), PathBuf::from("/m")]
        );
        assert_eq!(
            artist_folders(&dirs(&[("/m", "/m")])),
            vec![PathBuf::from("/m")]
        );
        assert!(artist_folders(&dirs(&[("/m/A/One", "/m"), ("/n/A/Two", "/n")])).is_empty());
        assert!(artist_folders(&[]).is_empty());
    }
}
