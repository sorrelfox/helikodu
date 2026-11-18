// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one context menu of every list, and the window actions behind it.
//!
//! A view says what the menu acts on (a [`Target`]: tracks, an album, an
//! artist or a genre) and pops the menu up; the window keeps the target while
//! the menu is open, and its `win.sel-*` actions act on it: play, play next,
//! add to the queue or a playlist, go to the album or artist, edit tags, rate,
//! open the containing folder, and remove from a playlist or the queue.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::{
    AlbumKey, ArtistKey, Backend, GenreKey, PlayerRequest, PlaylistKey, PlaylistRow, TrackKey,
    TrackRow,
};
use crate::i18n::{fill, ntr, tr};
use crate::window::MainWindow;

/// Where selected tracks are listed, for "Remove from …".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A library list: nothing to remove from.
    Library,
    /// Entries of a playlist, by position.
    Playlist {
        /// The playlist.
        key: PlaylistKey,
        /// The positions of the selected entries.
        positions: Vec<usize>,
    },
    /// Entries of the queue.
    Queue {
        /// The selected entries.
        entries: Vec<u64>,
    },
}

/// What a context menu acts on.
#[derive(Debug, Clone)]
pub(crate) enum Target {
    /// Tracks, with the list they were chosen in.
    Tracks {
        /// The rows, in list order.
        rows: Vec<Rc<TrackRow>>,
        /// Where they were chosen.
        origin: Origin,
    },
    /// An album.
    Album(AlbumKey),
    /// An artist.
    Artist(ArtistKey),
    /// A genre.
    Genre(GenreKey),
}

impl Target {
    /// Tracks chosen in a library list.
    pub(crate) fn tracks(rows: Vec<Rc<TrackRow>>) -> Self {
        Self::Tracks {
            rows,
            origin: Origin::Library,
        }
    }

    /// The selected library tracks, without missing entries.
    fn rows(&self) -> Vec<Rc<TrackRow>> {
        match self {
            Self::Tracks { rows, .. } => rows
                .iter()
                .filter(|r| !r.missing && r.key.0 != 0)
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The single album of the target, if it has one.
    fn album(&self) -> Option<AlbumKey> {
        match self {
            Self::Album(key) => Some(*key),
            Self::Tracks { rows, .. } => {
                let first = rows.first()?.album;
                (first.0 != 0 && rows.iter().all(|r| r.album == first)).then_some(first)
            }
            _ => None,
        }
    }

    /// The single artist name of the selected tracks, if they share one.
    fn artist_name(&self) -> Option<String> {
        match self {
            Self::Tracks { rows, .. } => {
                let first = &rows.first()?.artist;
                (!first.is_empty() && rows.iter().all(|r| &r.artist == first))
                    .then(|| first.clone())
            }
            _ => None,
        }
    }

    /// The track keys, looked up in the library for an album, artist or genre.
    pub(crate) async fn keys(&self, backend: &Backend) -> Vec<TrackKey> {
        use crate::backend::{TrackQuery, TrackScope};
        match self {
            Self::Tracks { .. } => self.rows().iter().map(|r| r.key).collect(),
            Self::Album(key) => {
                let key = *key;
                backend
                    .query(move |l| l.album(key))
                    .await
                    .ok()
                    .flatten()
                    .map(|d| d.tracks().map(|t| t.key).collect())
                    .unwrap_or_default()
            }
            Self::Artist(key) => {
                let query = TrackQuery {
                    scope: TrackScope::Artist(*key),
                    ..TrackQuery::default()
                };
                backend
                    .query(move |l| l.track_keys(&query))
                    .await
                    .unwrap_or_default()
            }
            Self::Genre(key) => {
                let query = TrackQuery {
                    scope: TrackScope::Genre(*key),
                    ..TrackQuery::default()
                };
                backend
                    .query(move |l| l.track_keys(&query))
                    .await
                    .unwrap_or_default()
            }
        }
    }
}

/// Which of the actions that depend on the target apply to `target`: go to
/// album, go to artist, remove.
pub(crate) fn applies(target: &Target) -> (bool, bool, bool) {
    let album = target.album().is_some();
    let artist = match target {
        Target::Tracks { .. } => target.artist_name().is_some(),
        _ => true,
    };
    let remove = matches!(
        target,
        Target::Tracks {
            origin: Origin::Playlist { .. } | Origin::Queue { .. },
            ..
        }
    );
    (album, artist, remove)
}

fn item(label: &str, action: &str) -> gio::MenuItem {
    gio::MenuItem::new(Some(label), Some(action))
}

/// The context menu of `target`, with `playlists` in "Add to playlist".
pub(crate) fn menu(target: &Target, playlists: &[PlaylistRow]) -> gio::Menu {
    let menu = gio::Menu::new();
    let play = gio::Menu::new();
    play.append_item(&item(&tr("Play"), "win.sel-play"));
    play.append_item(&item(&tr("Play next"), "win.sel-play-next"));
    play.append_item(&item(&tr("Add to queue"), "win.sel-enqueue"));
    let add = gio::Menu::new();
    let new = gio::MenuItem::new(Some(&tr("New playlist")), None);
    new.set_action_and_target_value(Some("win.sel-add-to-playlist"), Some(&0u64.to_variant()));
    add.append_item(&new);
    let lists = gio::Menu::new();
    let editing = match target {
        Target::Tracks {
            origin: Origin::Playlist { key, .. },
            ..
        } => Some(*key),
        _ => None,
    };
    for playlist in playlists.iter().filter(|p| Some(p.key) != editing) {
        let entry = gio::MenuItem::new(Some(&playlist.name), None);
        entry.set_action_and_target_value(
            Some("win.sel-add-to-playlist"),
            Some(&playlist.key.0.to_variant()),
        );
        lists.append_item(&entry);
    }
    add.append_section(None, &lists);
    play.append_submenu(Some(&tr("Add to playlist")), &add);
    match target {
        Target::Tracks { rows, .. } if rows.len() > 1 => {
            let label = fill(
                &ntr("{} track selected", "{} tracks selected", rows.len()),
                &[&rows.len().to_string()],
            );
            menu.append_section(Some(&label), &play);
        }
        _ => menu.append_section(None, &play),
    }
    let go = gio::Menu::new();
    if matches!(target, Target::Tracks { .. } | Target::Album(_)) {
        go.append_item(&item(&tr("Go to album"), "win.sel-go-album"));
        go.append_item(&item(&tr("Go to artist"), "win.sel-go-artist"));
    }
    if go.n_items() > 0 {
        menu.append_section(None, &go);
    }
    if let Target::Tracks { .. } = target {
        let edit = gio::Menu::new();
        let tags = item(&tr("Edit tags…"), "win.sel-edit-tags");
        tags.set_attribute_value("accel", Some(&"<Alt>Return".to_variant()));
        edit.append_item(&tags);
        let rate = gio::Menu::new();
        for stars in (1..=5).rev() {
            let label = "★".repeat(stars as usize);
            let entry = gio::MenuItem::new(Some(&label), None);
            entry.set_action_and_target_value(Some("win.sel-rate"), Some(&stars.to_variant()));
            rate.append_item(&entry);
        }
        let none = gio::MenuItem::new(Some(&tr("Not rated")), None);
        none.set_action_and_target_value(Some("win.sel-rate"), Some(&0i32.to_variant()));
        rate.append_item(&none);
        edit.append_submenu(Some(&tr("Rate")), &rate);
        menu.append_section(None, &edit);
        let folder = gio::Menu::new();
        folder.append_item(&item(&tr("Open containing folder"), "win.sel-open-folder"));
        menu.append_section(None, &folder);
    }
    let remove_label = match target {
        Target::Tracks {
            origin: Origin::Playlist { .. },
            ..
        } => Some(tr("Remove from playlist")),
        Target::Tracks {
            origin: Origin::Queue { .. },
            ..
        } => Some(tr("Remove from queue")),
        _ => None,
    };
    if let Some(label) = remove_label {
        let remove = gio::Menu::new();
        let entry = item(&label, "win.sel-remove");
        entry.set_attribute_value("accel", Some(&"Delete".to_variant()));
        remove.append_item(&entry);
        menu.append_section(None, &remove);
    }
    menu
}

/// Pops up the context menu of `target` on `widget`, at the pointer position
/// `at` in its coordinates, or at its middle for the keyboard.
pub(crate) fn popup(
    widget: &impl IsA<gtk::Widget>,
    target: Target,
    at: Option<(f64, f64)>,
) -> Option<gtk::PopoverMenu> {
    let widget = widget.as_ref();
    let window = widget.root().and_downcast::<MainWindow>()?;
    let model = menu(&target, &window.playlist_rows());
    window.set_target(Some(target));
    let popover = gtk::PopoverMenu::from_model(Some(&model));
    popover.set_parent(widget);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    let (x, y) = at.unwrap_or((
        f64::from(widget.width()) / 2.0,
        f64::from(widget.height()) / 2.0,
    ));
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
    Some(popover)
}

/// Whether a key press asks for the context menu: the Menu key or Shift+F10.
pub(crate) fn is_menu_key(key: gdk::Key, state: gdk::ModifierType) -> bool {
    key == gdk::Key::Menu || (key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK))
}

/// Runs `f` with the window's backend and target, then forgets the target.
fn with_target(window: &MainWindow, f: impl FnOnce(&MainWindow, Backend, Target) + 'static) {
    let Some(target) = window.take_target() else {
        return;
    };
    let Some(backend) = window.backend() else {
        return;
    };
    f(window, backend, target);
}

/// Plays, queues or adds to a playlist the tracks of `target`.
pub(crate) fn play(window: &MainWindow, backend: Backend, target: Target, next: Option<bool>) {
    let weak = window.downgrade();
    glib::spawn_future_local(async move {
        match (&target, next) {
            (Target::Album(album), None) => backend.send(PlayerRequest::PlayAlbum {
                album: *album,
                start: 0,
                shuffle: false,
            }),
            (Target::Artist(artist), None) => backend.send(PlayerRequest::PlayArtist {
                artist: *artist,
                shuffle: false,
            }),
            (Target::Genre(genre), None) => backend.send(PlayerRequest::PlayGenre {
                genre: *genre,
                shuffle: false,
            }),
            (_, None) => {
                let tracks = target.keys(&backend).await;
                backend.send(PlayerRequest::PlayTracks {
                    tracks,
                    start: 0,
                    title: tr("Selection"),
                    shuffle: false,
                });
            }
            (_, Some(next)) => {
                let tracks = target.keys(&backend).await;
                let count = tracks.len();
                if count == 0 {
                    return;
                }
                backend.send(PlayerRequest::Enqueue { tracks, next });
                if let Some(window) = weak.upgrade() {
                    let text = if next {
                        ntr("{} track plays next", "{} tracks play next", count)
                    } else {
                        ntr(
                            "{} track added to the queue",
                            "{} tracks added to the queue",
                            count,
                        )
                    };
                    window.toast(&fill(&text, &[&count.to_string()]), false);
                }
            }
        }
    });
}

/// Adds the tracks of `target` to `playlist`, or to a new playlist for `None`.
pub(crate) fn add_to_playlist(
    window: &MainWindow,
    backend: Backend,
    target: Target,
    playlist: Option<PlaylistKey>,
    position: Option<usize>,
) {
    let weak = window.downgrade();
    glib::spawn_future_local(async move {
        let tracks = target.keys(&backend).await;
        if tracks.is_empty() {
            return;
        }
        let count = tracks.len();
        let name = tr("New playlist");
        let result = backend
            .query(move |l| {
                let key = match playlist {
                    Some(key) => key,
                    None => l.create_playlist(&name)?,
                };
                l.add_to_playlist(key, &tracks, position)?;
                Ok(key)
            })
            .await;
        let Some(window) = weak.upgrade() else { return };
        match result {
            Ok(key) => {
                window.playlists_changed(Some(key));
                let name = window
                    .playlist_rows()
                    .iter()
                    .find(|p| p.key == key)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                window.toast(
                    &fill(
                        &ntr("{} track added to {}", "{} tracks added to {}", count),
                        &[&count.to_string(), &name],
                    ),
                    false,
                );
                if playlist.is_none() {
                    window.open_playlist(key);
                }
            }
            Err(error) => window.toast(&error.0, true),
        }
    });
}

/// Removes the entries of `target` from its playlist or the queue, with Undo.
fn remove(window: &MainWindow, backend: Backend, target: Target) {
    let Target::Tracks { rows, origin } = target else {
        return;
    };
    match origin {
        Origin::Queue { entries } => backend.send(PlayerRequest::RemoveEntries(entries)),
        Origin::Playlist { key, positions } => {
            let removed: Vec<(usize, TrackKey)> = positions
                .iter()
                .copied()
                .zip(rows.iter().map(|r| r.key))
                .collect();
            let count = removed.len();
            let weak = window.downgrade();
            glib::spawn_future_local(async move {
                let result = backend
                    .query(move |l| l.remove_from_playlist(key, &positions))
                    .await;
                let Some(window) = weak.upgrade() else { return };
                if let Err(error) = result {
                    window.toast(&error.0, true);
                    return;
                }
                window.playlists_changed(Some(key));
                let text = fill(
                    &ntr(
                        "{} track removed from the playlist",
                        "{} tracks removed from the playlist",
                        count,
                    ),
                    &[&count.to_string()],
                );
                let undo_window = window.downgrade();
                window.toast_undo(&text, move || {
                    let (Some(window), Some(backend)) = (
                        undo_window.upgrade(),
                        undo_window.upgrade().and_then(|w| w.backend()),
                    ) else {
                        return;
                    };
                    let removed = removed.clone();
                    let weak = window.downgrade();
                    glib::spawn_future_local(async move {
                        let result = backend
                            .query(move |l| {
                                // Put each entry back at its old position, first
                                // the lowest, so the later positions hold again.
                                // A missing entry names no track and stays out; the
                                // entries after it move up by one.
                                let mut removed = removed;
                                removed.sort_by_key(|(p, _)| *p);
                                let mut skipped = 0;
                                for (position, track) in removed {
                                    if track.0 == 0 {
                                        skipped += 1;
                                    } else {
                                        l.add_to_playlist(key, &[track], Some(position - skipped))?;
                                    }
                                }
                                Ok(())
                            })
                            .await;
                        if let Some(window) = weak.upgrade() {
                            if let Err(error) = result {
                                window.toast(&error.0, true);
                            }
                            window.playlists_changed(Some(key));
                        }
                    });
                });
            });
        }
        Origin::Library => {}
    }
}

/// Sets the rating of the target's tracks.
fn rate(window: &MainWindow, backend: Backend, target: Target, stars: Option<u8>) {
    let keys: Vec<TrackKey> = target.rows().iter().map(|r| r.key).collect();
    let weak = window.downgrade();
    glib::spawn_future_local(async move {
        let changed = keys.clone();
        let result = backend
            .query(move |l| {
                for key in keys {
                    l.set_rating(key, stars)?;
                }
                Ok(())
            })
            .await;
        if let Some(window) = weak.upgrade() {
            match result {
                Ok(()) => window.ratings_changed(&changed, stars),
                Err(error) => window.toast(&error.0, true),
            }
        }
    });
}

/// Shows the file of the first selected track in the file manager.
pub(crate) fn open_folder(window: &MainWindow, path: Option<std::path::PathBuf>) {
    let Some(path) = path else {
        window.toast(&tr("This track has no file to show."), true);
        return;
    };
    let (sender, receiver) = async_channel::bounded(1);
    let started = helikodu_desktop::open_folder::show_in_folder_later(path, None, move |r| {
        let _ = sender.send_blocking(r.map_err(|e| e.to_string()));
    });
    if let Err(error) = started {
        window.toast(&error.to_string(), true);
        return;
    }
    let weak = window.downgrade();
    glib::spawn_future_local(async move {
        if let Ok(Err(error)) = receiver.recv().await
            && let Some(window) = weak.upgrade()
        {
            window.toast(
                &fill(&tr("The folder cannot be shown: {}"), &[&error]),
                true,
            );
        }
    });
}

/// Goes to the artist of the target.
fn go_to_artist(window: &MainWindow, backend: Backend, target: Target) {
    let name = match &target {
        Target::Artist(key) => {
            window.open_artist(*key);
            return;
        }
        Target::Album(key) => {
            let key = *key;
            let weak = window.downgrade();
            glib::spawn_future_local(async move {
                let artist = backend
                    .query(move |l| {
                        let Some(album) = l.album(key)? else {
                            return Ok(None);
                        };
                        l.find_artist(&album.row.artist)
                    })
                    .await;
                if let (Some(window), Ok(Some(artist))) = (weak.upgrade(), artist) {
                    window.open_artist(artist);
                }
            });
            return;
        }
        _ => target.artist_name(),
    };
    let Some(name) = name else { return };
    let weak = window.downgrade();
    glib::spawn_future_local(async move {
        // A track artist "A & B" may only be found as "A".
        let artist = backend
            .query(move |l| {
                if let Some(key) = l.find_artist(&name)? {
                    return Ok(Some(key));
                }
                let first = name.split([',', '&']).next().unwrap_or_default().trim();
                l.find_artist(first)
            })
            .await;
        let Some(window) = weak.upgrade() else { return };
        match artist {
            Ok(Some(key)) => window.open_artist(key),
            _ => window.toast(&tr("This artist has no page of their own."), false),
        }
    });
}

/// Adds the `win.sel-*` actions to `window`.
pub(crate) fn install(window: &MainWindow) {
    let simple = |name: &str, run: fn(&MainWindow, Backend, Target)| {
        gio::ActionEntry::builder(name)
            .activate(move |window: &MainWindow, _, _| with_target(window, run))
            .build()
    };
    let play_now = simple("sel-play", |w, b, t| play(w, b, t, None));
    let play_next = simple("sel-play-next", |w, b, t| play(w, b, t, Some(true)));
    let enqueue = simple("sel-enqueue", |w, b, t| play(w, b, t, Some(false)));
    let go_album = simple("sel-go-album", |w, _, t| {
        if let Some(album) = t.album() {
            w.open_album(album);
        }
    });
    let go_artist = simple("sel-go-artist", go_to_artist);
    let edit = simple("sel-edit-tags", |w, b, t| {
        let w2 = w.downgrade();
        glib::spawn_future_local(async move {
            let keys = t.keys(&b).await;
            if let Some(window) = w2.upgrade() {
                window.open_tag_editor(keys);
            }
        });
    });
    let folder = simple("sel-open-folder", |w, _, t| {
        let path = t.rows().first().and_then(|r| r.path.clone());
        open_folder(w, path);
    });
    let remove_entries = simple("sel-remove", remove);
    let add = gio::ActionEntry::builder("sel-add-to-playlist")
        .parameter_type(Some(glib::VariantTy::UINT64))
        .activate(|window: &MainWindow, _, target| {
            let key = target.and_then(|t| t.get::<u64>()).unwrap_or(0);
            let playlist = (key != 0).then_some(PlaylistKey(key));
            with_target(window, move |w, b, t| {
                add_to_playlist(w, b, t, playlist, None)
            });
        })
        .build();
    let rating = gio::ActionEntry::builder("sel-rate")
        .parameter_type(Some(glib::VariantTy::INT32))
        .activate(|window: &MainWindow, _, target| {
            let stars = target.and_then(|t| t.get::<i32>()).unwrap_or(0);
            let stars = u8::try_from(stars).ok().filter(|s| *s > 0);
            with_target(window, move |w, b, t| rate(w, b, t, stars));
        })
        .build();
    window.add_action_entries([
        play_now,
        play_next,
        enqueue,
        go_album,
        go_artist,
        edit,
        folder,
        remove_entries,
        add,
        rating,
    ]);
}
