// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Another player's library as the readers return it, the players, and where
//! their files usually are.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::ImportError;
use crate::stats::Rating;

/// A player whose library can be imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Player {
    /// Rhythmbox: `rhythmdb.xml` and `playlists.xml`.
    Rhythmbox,
    /// Strawberry: its SQLite database.
    Strawberry,
    /// Clementine: its SQLite database.
    Clementine,
    /// Lollypop: `lollypop.db` and `playlists.db`.
    Lollypop,
    /// iTunes or Apple Music: an exported `Library.xml`.
    ITunes,
}

impl Player {
    /// Every player, in the order they are offered.
    pub const ALL: [Player; 5] = [
        Player::Rhythmbox,
        Player::Strawberry,
        Player::Clementine,
        Player::Lollypop,
        Player::ITunes,
    ];

    /// The player's name, as it calls itself.
    pub fn name(self) -> &'static str {
        match self {
            Player::Rhythmbox => "Rhythmbox",
            Player::Strawberry => "Strawberry",
            Player::Clementine => "Clementine",
            Player::Lollypop => "Lollypop",
            Player::ITunes => "iTunes",
        }
    }

    /// The short name the command line uses: `rhythmbox`, `strawberry`,
    /// `clementine`, `lollypop` or `itunes`.
    pub fn id(self) -> &'static str {
        match self {
            Player::Rhythmbox => "rhythmbox",
            Player::Strawberry => "strawberry",
            Player::Clementine => "clementine",
            Player::Lollypop => "lollypop",
            Player::ITunes => "itunes",
        }
    }

    /// The player of a short name (any case); `music` is iTunes.
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase();
        if id == "music" || id == "apple-music" {
            return Some(Player::ITunes);
        }
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The places where the player keeps its library file, most likely first:
    /// `data` and `config` are the XDG data and configuration folders.
    fn usual_paths(self, home: &Path, data: &Path, config: &Path) -> Vec<PathBuf> {
        let flatpak = |app: &str, rest: &str| home.join(".var/app").join(app).join(rest);
        match self {
            Player::Rhythmbox => vec![
                data.join("rhythmbox/rhythmdb.xml"),
                flatpak("org.gnome.Rhythmbox3", "data/rhythmbox/rhythmdb.xml"),
            ],
            Player::Strawberry => vec![
                data.join("strawberry/strawberry/strawberry.db"),
                flatpak(
                    "org.strawberrymusicplayer.strawberry",
                    "data/strawberry/strawberry/strawberry.db",
                ),
            ],
            Player::Clementine => vec![
                config.join("Clementine/clementine.db"),
                home.join(".config/Clementine/clementine.db"),
                flatpak(
                    "org.clementine_player.Clementine",
                    "config/Clementine/clementine.db",
                ),
            ],
            Player::Lollypop => vec![
                data.join("lollypop/lollypop.db"),
                flatpak("org.gnome.Lollypop", "data/lollypop/lollypop.db"),
            ],
            Player::ITunes => vec![
                home.join("Music/iTunes/iTunes Music Library.xml"),
                home.join("Music/iTunes/iTunes Library.xml"),
                home.join("Music/Library.xml"),
            ],
        }
    }
}

/// A player's library file to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSource {
    /// The player.
    pub player: Player,
    /// Its library file: `rhythmdb.xml`, the SQLite database, `lollypop.db` or
    /// `Library.xml`. A folder holding it works too.
    pub path: PathBuf,
}

impl ImportSource {
    /// The source as the import records remember it: the player and the
    /// canonical path of its library file.
    pub(crate) fn key(&self) -> String {
        // The file itself, however it was named, so that a folder and the file in
        // it count as one source.
        let file = self.file();
        let path = std::fs::canonicalize(&file)
            .or_else(|_| std::path::absolute(&file))
            .unwrap_or(file);
        format!("{}:{}", self.player.id(), path.to_string_lossy())
    }

    /// The library file, when `path` names its folder.
    pub(crate) fn file(&self) -> PathBuf {
        if !self.path.is_dir() {
            return self.path.clone();
        }
        let name = match self.player {
            Player::Rhythmbox => "rhythmdb.xml",
            Player::Strawberry => "strawberry.db",
            Player::Clementine => "clementine.db",
            Player::Lollypop => "lollypop.db",
            Player::ITunes => {
                return [
                    "iTunes Music Library.xml",
                    "iTunes Library.xml",
                    "Library.xml",
                ]
                .into_iter()
                .map(|n| self.path.join(n))
                .find(|p| p.is_file())
                .unwrap_or_else(|| self.path.join("Library.xml"));
            }
        };
        self.path.join(name)
    }
}

/// The players' library files found at their usual places (Flatpak folders
/// included), from `HOME`, `XDG_DATA_HOME` and `XDG_CONFIG_HOME`. Only files that
/// exist are listed.
pub fn default_sources() -> Vec<ImportSource> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let var = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    default_sources_in(
        &home,
        var("XDG_DATA_HOME").as_deref(),
        var("XDG_CONFIG_HOME").as_deref(),
    )
}

/// [`default_sources`] for the home folder `home` and the XDG folders `data` and
/// `config` (`~/.local/share` and `~/.config` when `None`).
pub fn default_sources_in(
    home: &Path,
    data: Option<&Path>,
    config: Option<&Path>,
) -> Vec<ImportSource> {
    let data = data.map_or_else(|| home.join(".local/share"), Path::to_path_buf);
    let config = config.map_or_else(|| home.join(".config"), Path::to_path_buf);
    let mut found: Vec<ImportSource> = Vec::new();
    for player in Player::ALL {
        for path in player.usual_paths(home, &data, &config) {
            if path.is_file() && !found.iter().any(|s| s.path == path) {
                found.push(ImportSource { player, path });
            }
        }
    }
    found
}

/// Reads the library of `source`, changing nothing.
pub fn read_source(source: &ImportSource) -> Result<ForeignLibrary, ImportError> {
    let file = source.file();
    match source.player {
        Player::Rhythmbox => super::rhythmbox::read(&file),
        Player::Strawberry | Player::Clementine => super::clementine::read(&file, source.player),
        Player::Lollypop => super::lollypop::read(&file),
        Player::ITunes => super::itunes::read(&file),
    }
}

/// One entry of another player's library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignTrack {
    /// Where the player found the file: a `file://` URI or a path, as it stores it.
    pub location: Option<String>,
    /// The title.
    pub title: Option<String>,
    /// The artist.
    pub artist: Option<String>,
    /// The album.
    pub album: Option<String>,
    /// The track number.
    pub track_number: Option<u32>,
    /// The length.
    pub duration: Option<Duration>,
    /// How often it was played there.
    pub play_count: u64,
    /// How often it was skipped there.
    pub skip_count: u64,
    /// Its rating; `None` when it is not rated.
    pub rating: Option<Rating>,
    /// Whether it is loved (a heart, a favourite).
    pub loved: bool,
    /// When it was first played.
    pub first_played: Option<SystemTime>,
    /// When it was last played.
    pub last_played: Option<SystemTime>,
    /// When it was last skipped.
    pub last_skipped: Option<SystemTime>,
    /// When it came into that library.
    pub added: Option<SystemTime>,
    /// Single listens, newest or oldest first (only an export of Helikodu's own data
    /// has them).
    pub history: Vec<ForeignListen>,
}

/// One listen of a [`ForeignTrack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignListen {
    /// When it started.
    pub started_at: SystemTime,
    /// How long it played.
    pub played: Duration,
    /// Whether it counted as a play.
    pub counted: bool,
    /// Where it was played from.
    pub context: Option<String>,
}

/// An entry of a static playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForeignItem {
    /// The track at this index of [`ForeignLibrary::tracks`].
    Track(usize),
    /// A file that is not among the tracks: its location and what is known of it.
    Other(Box<ForeignTrack>),
}

/// What a playlist of another player becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForeignPlaylistKind {
    /// A static playlist with these entries. `from_rules` is set for a playlist
    /// the player fills from rules that could not be read: its current entries
    /// are imported instead.
    Static {
        /// The entries, in order.
        items: Vec<ForeignItem>,
        /// Whether the player fills it from rules.
        from_rules: bool,
    },
    /// A smart playlist with this query (in the query language).
    Smart(String),
    /// A playlist that is not imported, and why.
    Unmapped(String),
}

/// A playlist of another player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignPlaylist {
    /// Its name.
    pub name: String,
    /// The folders it is in, outermost first (empty at the top).
    pub folder: Vec<String>,
    /// What it becomes.
    pub kind: ForeignPlaylistKind,
}

/// An album a label was given to, by its album artist and title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignAlbum {
    /// The album artist; `None` for an album of various artists.
    pub album_artist: Option<String>,
    /// The title.
    pub album: String,
}

/// A label and what it was given to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignLabel {
    /// The name.
    pub name: String,
    /// The tracks, by location and tags.
    pub tracks: Vec<ForeignTrack>,
    /// The albums.
    pub albums: Vec<ForeignAlbum>,
    /// The artists, by name.
    pub artists: Vec<String>,
}

/// What a reader found in another player's library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignLibrary {
    /// The entries with their statistics.
    pub tracks: Vec<ForeignTrack>,
    /// The playlists.
    pub playlists: Vec<ForeignPlaylist>,
    /// The favourites (only in an export of Helikodu's own data).
    pub favourites: Vec<ForeignItem>,
    /// Saved searches as names and query text (only in an export of Helikodu's own
    /// data).
    pub saved_searches: Vec<(String, String)>,
    /// Labels with what was given them (only in an export of Helikodu's own data).
    pub labels: Vec<ForeignLabel>,
    /// The user's computed and custom fields, in their order (only in an export of
    /// Helikodu's own data).
    pub fields: Vec<crate::UserField>,
    /// What the reader left out, in words.
    pub notes: Vec<String>,
}
