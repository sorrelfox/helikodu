# Playlists and ratings

## Playlists

Your playlists are listed under **Playlists** in the sidebar.

- **New playlist** in the sidebar, or **New Playlist** in the main menu (**Ctrl+N**), creates an empty one.
- **Add to playlist** in a track's right-click menu adds tracks to an existing playlist or to a **New playlist**. You can also drag tracks onto a playlist in the sidebar, or into an open playlist at the place where you drop them.
- **Save as playlist** in the queue panel saves the queue as a playlist.
- Open a playlist to see its tracks, with **Play**, **Shuffle**, **Export** and **More** (**Rename…**, **Delete…**). Drag entries to change their order. **Remove from playlist** in the right-click menu, or **Delete**, takes tracks out of it; **Undo** in the toast, or **Ctrl+Z** while the toast is shown, puts them back.
- Right-click a playlist in the sidebar for **Play**, **Rename…**, **Export…** and **Delete…**. Deleting a playlist keeps its tracks in the library.

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
