// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The cover manager: which albums have a cover where, and putting local pictures
//! into the files or beside them. It never downloads anything: the pictures are
//! those already in the album folders, or one the user chooses.
//!
//! Albums fall into groups ([`CoverShow`]): no cover at all, a picture only in the
//! folder, only inside the files, smaller than 500 px, or not the same in every
//! file. For an album without a cover, any image file in its folder (or a folder
//! below it, such as `booklet/`) is offered.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use helikodu_library::pictures::{CoverLimits, PictureFormat, probe_picture};
use helikodu_library::tag_editing::{BatchItem, CoverChange, TagChanges};
use helikodu_library::{AlbumCoverFacts, AlbumId, AlbumQuery, CoverHash, MAX_PAGE, Page};
use helikodu_model::settings::{AdvancedSettings, LibrarySettings};

use crate::files::write_atomic;
use crate::library::{LibraryError, MusicLibrary};

/// Below this many pixels on the shorter side a cover counts as small.
pub const SMALL_COVER: u32 = 500;

/// The groups of the cover manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoverShow {
    /// Every album.
    All,
    /// Albums with no cover anywhere.
    NoCover,
    /// A picture in the folder, none in the files.
    OnlyInFolder,
    /// A picture in the files, none in the folder.
    OnlyInFiles,
    /// A cover smaller than [`SMALL_COVER`] pixels.
    Small,
    /// Some files carry another picture, or none.
    NotSame,
}

impl CoverShow {
    /// Every group, in the order the manager lists them.
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::NoCover,
        Self::OnlyInFolder,
        Self::OnlyInFiles,
        Self::Small,
        Self::NotSame,
    ];
}

/// An image file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPicture {
    /// The file.
    pub path: PathBuf,
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
    /// Its size in bytes.
    pub bytes: u64,
}

/// One album as the cover manager shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumCovers {
    /// The album.
    pub album: AlbumId,
    /// Its title.
    pub title: Option<String>,
    /// Its artist.
    pub artist: Option<String>,
    /// Its year.
    pub year: Option<i32>,
    /// Its cover, for the thumbnail.
    pub cover: Option<CoverHash>,
    /// Where its covers are.
    pub facts: AlbumCoverFacts,
}

impl AlbumCovers {
    /// The album's main folder.
    pub fn folder(&self) -> Option<&Path> {
        self.facts.folders.first().map(PathBuf::as_path)
    }

    /// Whether the album is in `group`.
    pub fn is_in(&self, group: CoverShow) -> bool {
        let f = &self.facts;
        let in_folder = f.folder_image.is_some();
        match group {
            CoverShow::All => true,
            CoverShow::NoCover => !in_folder && !f.in_files(),
            CoverShow::OnlyInFolder => in_folder && !f.in_files(),
            CoverShow::OnlyInFiles => !in_folder && f.in_files(),
            CoverShow::Small => f
                .cover
                .as_ref()
                .and_then(|c| Some(c.width?.min(c.height?)))
                .is_some_and(|side| side < SMALL_COVER),
            CoverShow::NotSame => f.in_files() && !f.same_in_every_file(),
        }
    }
}

/// Every album with its covers, in the library's album order.
pub fn album_covers(library: &MusicLibrary) -> Result<Vec<AlbumCovers>, LibraryError> {
    let facts: HashMap<AlbumId, AlbumCoverFacts> = library
        .inner()
        .album_cover_facts()?
        .into_iter()
        .map(|f| (f.album, f))
        .collect();
    let mut out = Vec::new();
    let mut offset = 0;
    loop {
        let page = library.albums(&AlbumQuery {
            page: Page::new(offset, MAX_PAGE),
            ..AlbumQuery::default()
        })?;
        let read = page.items.len();
        for summary in page.items {
            if let Some(f) = facts.get(&summary.id) {
                out.push(AlbumCovers {
                    album: summary.id,
                    title: summary.title,
                    artist: summary.artist,
                    year: summary.year,
                    cover: summary.cover,
                    facts: f.clone(),
                });
            }
        }
        offset += read;
        if read == 0 || offset >= page.total {
            return Ok(out);
        }
    }
}

/// How many albums each group holds.
pub fn group_counts(albums: &[AlbumCovers]) -> Vec<(CoverShow, usize)> {
    CoverShow::ALL
        .iter()
        .map(|&g| (g, albums.iter().filter(|a| a.is_in(g)).count()))
        .collect()
}

/// The image files in `folder` and the folders right below it, largest first,
/// leaving out `except` (the cover the album has already).
pub fn find_pictures(folder: &Path, except: Option<&Path>) -> Vec<FolderPicture> {
    let mut found = Vec::new();
    let look = |dir: &Path, depth: u8, found: &mut Vec<FolderPicture>| -> Vec<PathBuf> {
        let mut below = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return below;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth == 0 {
                    below.push(path);
                }
                continue;
            }
            if Some(path.as_path()) == except || !kind.is_file() {
                continue;
            }
            if let Some(picture) = picture_at(&path) {
                found.push(picture);
            }
        }
        below
    };
    for dir in look(folder, 0, &mut found) {
        look(&dir, 1, &mut found);
    }
    found.sort_by(|a, b| {
        (u64::from(b.width) * u64::from(b.height))
            .cmp(&(u64::from(a.width) * u64::from(a.height)))
            .then_with(|| a.path.cmp(&b.path))
    });
    found
}

/// The picture at `path`, if it is an image file of a format covers come in.
pub fn picture_at(path: &Path) -> Option<FolderPicture> {
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let info = probe_picture(&bytes, &CoverLimits::default()).ok()?;
    Some(FolderPicture {
        path: path.to_owned(),
        width: info.width,
        height: info.height,
        bytes: bytes.len() as u64,
    })
}

/// Where a picture is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoverSave {
    /// Into the files and as `cover.jpg` in the folder.
    #[default]
    FilesAndFolder,
    /// Into the files only.
    FilesOnly,
    /// As `cover.jpg` only.
    FolderOnly,
}

/// What happened to one file of a cover change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverResult {
    /// The file (an audio file, or the picture written).
    pub path: PathBuf,
    /// Why it failed, if it did.
    pub error: Option<String>,
}

/// The settings a write into library files follows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WriteContext {
    /// The `[library]` settings.
    pub library: LibrarySettings,
    /// The `[advanced]` settings.
    pub advanced: AdvancedSettings,
    /// The scan worker program, when it is not beside the running one.
    pub worker_program: Option<PathBuf>,
}

/// The file name a picture of `format` gets in the folder: `cover.jpg`, or
/// `cover.png` for a PNG picture.
fn folder_name(format: PictureFormat) -> &'static str {
    match format {
        PictureFormat::Png => "cover.png",
        _ => "cover.jpg",
    }
}

/// Puts the picture `bytes` (JPEG or PNG) into the album's files and/or its folder,
/// as `save` says, and reads the files again.
pub fn use_picture(
    library: &MusicLibrary,
    album: &AlbumCovers,
    bytes: &[u8],
    save: CoverSave,
    context: &WriteContext,
) -> Result<Vec<CoverResult>, LibraryError> {
    let format = PictureFormat::sniff(bytes);
    if !matches!(format, Some(PictureFormat::Jpeg | PictureFormat::Png)) {
        return Ok(vec![CoverResult {
            path: PathBuf::new(),
            error: Some("only JPEG and PNG pictures can be used".into()),
        }]);
    }
    let mut results = Vec::new();
    if save != CoverSave::FilesOnly
        && let Some(folder) = album.folder()
        && let Some(format) = format
    {
        let target = folder.join(folder_name(format));
        let error = write_atomic(&target, bytes).err().map(|e| e.to_string());
        results.push(CoverResult {
            path: target,
            error,
        });
    }
    if save != CoverSave::FolderOnly {
        let mut changes = TagChanges::new();
        changes.cover = Some(CoverChange::SetFront(bytes.to_vec()));
        results.extend(write_files(library, album, &changes, context)?);
    }
    Ok(results)
}

/// Saves the picture inside the album's files as `cover.jpg` (`cover.png`) in its
/// folder.
pub fn save_embedded(
    library: &MusicLibrary,
    album: &AlbumCovers,
) -> Result<Vec<CoverResult>, LibraryError> {
    let Some(&(track, ref file)) = album.facts.files.first() else {
        return Ok(Vec::new());
    };
    let Some(bytes) = crate::convert::cover_of(library, track) else {
        return Ok(vec![CoverResult {
            path: file.clone(),
            error: Some("the picture inside the file cannot be read".into()),
        }]);
    };
    let Some(folder) = album.folder() else {
        return Ok(Vec::new());
    };
    let format = PictureFormat::sniff(&bytes).unwrap_or(PictureFormat::Jpeg);
    let target = folder.join(folder_name(format));
    let error = write_atomic(&target, &bytes).err().map(|e| e.to_string());
    Ok(vec![CoverResult {
        path: target,
        error,
    }])
}

/// Removes the front cover from the album's files. Pictures in the folder stay.
pub fn remove_from_files(
    library: &MusicLibrary,
    album: &AlbumCovers,
    context: &WriteContext,
) -> Result<Vec<CoverResult>, LibraryError> {
    let mut changes = TagChanges::new();
    changes.cover = Some(CoverChange::RemoveFront);
    write_files(library, album, &changes, context)
}

fn write_files(
    library: &MusicLibrary,
    album: &AlbumCovers,
    changes: &TagChanges,
    context: &WriteContext,
) -> Result<Vec<CoverResult>, LibraryError> {
    let items: Vec<BatchItem> = album
        .facts
        .files
        .iter()
        .map(|(_, path)| BatchItem {
            path: path.clone(),
            expected: None,
            changes: changes.clone(),
        })
        .collect();
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let report = library.write_tags(
        &items,
        &context.library,
        &context.advanced,
        context.worker_program.as_deref(),
        &mut |_| {},
    )?;
    Ok(report
        .batch
        .results
        .into_iter()
        .map(|(path, result)| CoverResult {
            path,
            error: result.err().map(|e| e.to_string()),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_in_the_folder_and_below() {
        let dir = std::env::temp_dir().join(format!("helikodu-covers-{}", std::process::id()));
        fs::create_dir_all(dir.join("booklet")).unwrap();
        // A 1×1 PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        fs::write(dir.join("booklet/page-01.png"), png).unwrap();
        fs::write(dir.join("scan.png"), png).unwrap();
        fs::write(dir.join("notes.txt"), b"x").unwrap();
        let found = find_pictures(&dir, Some(&dir.join("scan.png")));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, dir.join("booklet/page-01.png"));
        assert_eq!((found[0].width, found[0].height), (1, 1));
        let _ = fs::remove_dir_all(dir);
    }
}
