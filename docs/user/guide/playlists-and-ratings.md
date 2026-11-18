# Playlists and ratings

## Playlists

Your playlists are listed under **Playlists** in the sidebar.

- **New Playlist** in the main menu (**Ctrl+N**) creates an empty one. **New playlist** in the sidebar offers a **Playlist**, a **Smart playlist** or a **Folder**.
- **Add to playlist** in a track's right-click menu adds tracks to an existing playlist or to a **New playlist**. You can also drag tracks onto a playlist in the sidebar, or into an open playlist at the place where you drop them.
- **Save as playlist** in the queue panel saves the queue as a playlist.
- Open a playlist to see its tracks, with **Play**, **Shuffle**, **Export**, the lock and **More**. Drag entries to change their order. **Remove from playlist** in the right-click menu, or **Delete**, takes tracks out of it.
- Right-click a playlist in the sidebar for **Play**, **Rename…**, **Duplicate**, **Move to folder**, **Pin to the sidebar**, **Export…** and **Delete playlist…**. Deleting a playlist keeps its tracks in the library.

### Undo and the lock

Every change of a playlist shows a toast with **Undo**. **Ctrl+Z** undoes the last change of the open playlist, even after the toast is gone, and **Ctrl+Shift+Z** (or **Ctrl+Y**) does it again. Only deleting a playlist asks first.

The lock button, or **Lock against changes** in **More**, protects a playlist: its drag handles disappear, and adding, removing, moving and the commands below are refused until you unlock it.

### Tidying and grouping

**More** in a playlist also has:

- **Group by album**: tracks that follow each other on one album get a header with the cover, the album, the artist and the year.
- **Remove duplicates**: takes out entries that repeat an earlier one. The count shows how many there are.
- **Remove missing tracks**: takes out entries whose file is gone or that never matched a track of your library.
- **Shuffle the order…**: puts the entries in a random order for good.

### Folders and pins

Folders hold playlists and other folders. Click a folder in the sidebar to open or close it. Drag a playlist onto a folder, or use **Move to folder**, to put it there. Deleting a folder keeps its playlists; they move up a level.

**Pin to the sidebar**, in the right-click menu of an album, an artist or a playlist, lists it under **Pinned** at the top of the sidebar. **Unpin** takes it away.

## Smart playlists

A smart playlist holds the tracks that match its rules, and follows your library: tracks that start to match come in, tracks that stop matching leave. Create one with **New playlist** ▸ **Smart playlist** in the sidebar, or **Save as smart playlist** on the search results of a query.

Opened from the sidebar, a smart playlist shows its rules in words above its tracks. A click on a column header sorts the table for this visit; the rules keep their own order. **Edit rules** (or a click on a rule) opens the rule builder:

- **Match all** or **any** of the rules. Each rule is a field, a condition and a value. **Add group** makes a group of rules that matches **any** or **all** of them.
- **Limit to** keeps the first tracks, minutes, hours or gigabytes, in the order chosen under **sorted by**.
- The box below shows the same rules as query text (see the query language in [Browsing](browsing.md)). **Edit as text** lets you type them; text that cannot be read keeps the last rules and says why.

The right side shows how many tracks match, and the tracks themselves, as you edit. **Done editing** returns to the playlist. To reorder the tracks by hand, **Save a copy as a playlist** in **More**.

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

Give a track one to five stars with **Rate** in its right-click menu, by clicking the stars in the **Rating** column of a track table, or in the [tag editor](tags.md); choose **Not rated** to clear it. Ratings are kept in Helikodu's library and survive moves and renames of the file. They are not written into the files.

When Helikodu first reads a file, it takes over a rating that another player wrote into the tags (for example with MusicBee, Quod Libet, foobar2000 or Picard).
