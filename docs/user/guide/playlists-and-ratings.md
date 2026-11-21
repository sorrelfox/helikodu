# Playlists and ratings

## Playlists

Your playlists are listed under **Playlists** in the sidebar.

- **New Playlist** in the main menu (**Ctrl+N**) creates an empty one. **New playlist** in the sidebar offers a **Playlist**, a **Smart Playlist** or a **Folder**.
- **Add to Playlist** in a track's right-click menu adds tracks to an existing playlist or to a **New Playlist**. You can also drag tracks onto a playlist in the sidebar, or into an open playlist at the place where you drop them.
- **Save as playlist** in the queue panel saves the queue as a playlist.
- Open a playlist to see its tracks, with **Play**, **Shuffle**, **Export**, the lock and **More**. Drag entries to change their order. **Remove from Playlist** in the right-click menu, or **Delete**, takes tracks out of it.
- Right-click a playlist in the sidebar for **Play**, **Rename…**, **Duplicate**, **Move to Folder**, **Pin to the Sidebar**, **Export…** and **Delete Playlist…**. Deleting a playlist keeps its tracks in the library.

### Favourites

**Favourites** is always the first playlist. The heart on the player deck adds the playing track to it, or takes it out again; you can also add tracks to it like to any other playlist. It cannot be renamed, moved into a folder, locked or deleted.

### Undo and the lock

Every change of a playlist shows a toast with **Undo**. **Ctrl+Z** undoes the last change of the open playlist, even after the toast is gone, and **Ctrl+Shift+Z** (or **Ctrl+Y**) does it again. Only deleting a playlist asks first.

The lock button, or **Lock Against Changes** in **More**, protects a playlist: its drag handles disappear, and adding, removing, moving and the commands below are refused until you unlock it.

### Tidying and grouping

**More** in a playlist also has:

- **Group by Album**: tracks that follow each other on one album get a header with the cover, the album, the artist and the year.
- **Remove Duplicates**: takes out entries that repeat an earlier one. The count shows how many there are.
- **Remove Missing Tracks**: takes out entries whose file is gone or that never matched a track of your library.
- **Shuffle the Order…**: puts the entries in a random order for good.

### Folders and pins

Folders hold playlists and other folders. Click a folder in the sidebar to open or close it. Drag a playlist onto a folder, or use **Move to Folder**, to put it there. Deleting a folder keeps its playlists; they move up a level.

**Pin to the Sidebar**, in the right-click menu of an album, an artist or a playlist, lists it under **Pinned** at the top of the sidebar. **Unpin** takes it away.

## Smart playlists

A smart playlist holds the tracks that match its rules, and follows your library: tracks that start to match come in, tracks that stop matching leave. Create one with **New playlist** ▸ **Smart Playlist** in the sidebar, or **Save as smart playlist** on the search results of a query.

Opened from the sidebar, a smart playlist shows its rules in words above its tracks. A click on a column header sorts the table for this visit; the rules keep their own order. **Columns**, at the end of the rule line, shows or hides columns for this smart playlist only: **#**, **Artist**, **Album**, **Genre**, **Year**, **Format**, **Added**, **Plays**, **Rating** and **Length** (the title always shows). Each smart playlist remembers its own choice. Click the stars in the **Rating** column to rate a track. **Edit rules** (or a click on a rule) opens the rule builder:

- **Match all** or **any** of the rules. Each rule is a field, a condition and a value. **Add group** makes a group of rules that matches **any** or **all** of them.
- **Limit to** keeps the first tracks, minutes, hours or gigabytes, in the order chosen under **sorted by**.
- The box below shows the same rules as query text (see the query language in [Browsing](browsing.md)). **Edit as text** lets you type them; text that cannot be read keeps the last rules and says why.

The right side shows how many tracks match, and the tracks themselves, as you edit. **Done editing** returns to the playlist. To reorder the tracks by hand, **Save a Copy as a Playlist** in **More**.

## Generator playlists

A generator playlist is made by steps: a source, then filters, sorts, caps and a limit, each working on what the step before left. It keeps its last result and is made again each time it is opened, or only when you ask (`--keep`). Generator playlists have no place in the sidebar yet; they are made and played with `helikodu-cli generator`:

```text
# The source: library, playlist "Name" or query <query>.
from library
# Keeps what a query finds (the query language of the search field).
where rating>=4
# Leaves out the tracks of a playlist, smart playlist or other generator.
exclude playlist "Heard this week"
# rating, plays, skips, last-played, added, year, title, artist, album, length
# or random, then asc (the default) or desc.
sort last-played
per-artist 2
per-album 1
# Or: limit 90 minutes.
limit 50
```

- `helikodu-cli generator add "Forgotten favourites" --steps-file steps.txt` adds one (`--steps TEXT` takes the steps directly, `--steps-file -` reads them from the standard input).
- `generator list`, `show NAME` (the steps as text, to edit and give back with `edit NAME --steps-file`), `edit NAME --name NEW --on-open yes|no`, `remove NAME`.
- `generator run NAME` makes it again and lists its tracks; `generator open NAME` lists them as opening shows them; `generator preview --steps TEXT` tries steps without saving them.
- `generator play NAME` plays it in the running application.

## Playlist files in the library folders

With **Import playlist files found in the folders** on (**Settings** ▸ **Library**), the M3U, M3U8, PLS and XSPF files in your library folders become playlists after each scan. They show a file icon in the sidebar and follow their files: when a file changes, its playlist changes too (and **Ctrl+Z** brings back the previous entries), unless the playlist is locked.

Playlists remember tracks, not file paths: when you move or rename files in your library folders, they stay in your playlists. A track whose file is gone stays in the playlist, shown as missing, and comes back when the file returns.

## Importing and exporting

**Import Playlist…** in the main menu reads an M3U, M3U8, PLS or XSPF file and adds it to your playlists.

**Export…** writes a playlist to a file, with the paths of the tracks relative to the playlist file. The extension of the file name chooses the format:

| Extension | Format |
|---|---|
| `.m3u8` (the default) | M3U8 |
| `.m3u` | M3U |
| `.pls` | PLS |
| `.xspf` | XSPF |

Any other extension writes M3U8.

## Ratings

Give a track one to five stars with **Rate** in its right-click menu, by clicking the stars in the **Rating** column of a track table, or in the [tag editor](tags.md); choose **Not Rated** to clear it. Ratings are kept in Helikodu's library and survive moves and renames of the file. They are written into the files only if you turn that on; see [Ratings](tags.md#ratings).

When Helikodu first reads a file, it takes over a rating that another player wrote into the tags (for example with MusicBee, Quod Libet, foobar2000 or Picard).
