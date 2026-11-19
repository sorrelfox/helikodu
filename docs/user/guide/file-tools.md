# Organizing, checking and finding files

Three tools in the **Tools** and **Library** menus work on the files of your library. Each opens in a window of its own; nothing changes before you press the button at the bottom right.

## Organize files

**Tools ▸ Organize Files…** moves and renames your files by a naming pattern. Opened from the context menu of selected tracks (**Organize files…**), it works on those tracks only.

- **Patterns** at the left are saved patterns; edits to the pattern are kept with the chosen one. **New pattern** adds one. Right-click a pattern (or press the Menu key) to **Rename** or **Delete** it.
- A pattern names fields in braces (see [Naming patterns](#naming-patterns) below). **Insert field** adds a field at the cursor. The file's extension is always kept.
- **Into** chooses where files go: the library folder each file is already in, one of your library folders, or a folder you choose inside a library folder.
- The second dropdown chooses the characters names may hold: **Names that Windows can read** (for drives you share with other systems), **Names as they are**, or **Plain ASCII only** (accents are dropped).
- **Move covers and booklets too** moves the pictures and PDF files of a folder along when every track of that folder goes to the same new folder.

The table shows each file's path now and after the move, and what happens to it: **Move**, **Rename**, **Move and rename**, **Already in place**, or **Name taken** when a file with that name exists already (such files are not ticked and are never replaced). Untick a row to leave that file where it is; Space ticks the selected row, and a double click plays it.

**Move N files** writes every move down before it makes it, so a run can be undone: with **Undo** in the notice that follows, with **Undo the last run** in the window, with **Ctrl+Z** in the main window (which steps through tag changes and runs together, newest first), or later, even after a restart. After an undo the button offers **Redo the last run**. A file that cannot move (or move back) stays where it is and the window says why. Across drives a file is copied, checked byte for byte, and only then removed from its old place. Folders that a run leaves empty are removed, and an undo puts them back. Ratings, play counts, history and playlist entries stay with the tracks.

Moves older than the limit of the undo history in [Settings ▸ Library ▸ Storage](settings.md) (90 days by default) can no longer be undone.

## Naming patterns

The organizer, the [converter and folder sync](converting-and-sync.md#naming-patterns) read patterns the same way, so one pattern gives a track the same name in each of them.

- `{field}` is a field of the track, with the field names of the search language: `{albumartist}/{year} {album}/[{disc}-]{track} {title}`. Other spellings work too (`{album_artist}`, `{tracknumber}`), and `{tag:MOOD}` is any other tag.
- A `/` starts a folder. A value never starts a folder of its own: a `/` in a tag is replaced.
- Text in square brackets is left out when a field inside it is empty. Brackets can be nested, and an inner bracket is left out on its own: `[{composer} [({year})] ]`. A bracket can hold a folder: `{album}/[Disc {disc}/]{track} {title}` makes a folder per disc only on albums of several discs.
- Track numbers have two digits (three on an album of a hundred tracks or more). `{disc}` has a value only on an album of several discs, with as many digits as the number of discs needs, so `[{disc}-]` only appears there. `{track}` never includes the disc.
- A field that is empty outside brackets is written as "Unknown"; a track without a title is named by its file name.
- Each folder and file name is cut to 255 bytes, leaving room for the extension.

The braces can hold more than a field: functions, conditions and comparisons such as `{if(composer, composer, artist)}` or `{initial(noarticle(albumartist))}`. See [Expressions](expressions.md).

A pattern that cannot be read is explained under the entry, with the character where the problem is.

## Renaming one file and moving files to the trash

The context menu of a track has **Rename file…**, which renames the file in its folder (also undoable from the notice), and **Move to trash…**, which moves the files of the selected tracks to the desktop Trash after a confirmation. From the Trash you can restore them with your file manager.

## Missing files

**Library ▸ Missing Files…** lists the tracks whose file has gone, by the library folder they were in, with how long they have been missing. A track leaves the library when it has been missing for the number of days set in [Settings ▸ Library](settings.md) (30 by default). Tracks on a drive that is not connected are hidden, not missing, and are not listed.

The window looks for the files in your library folders when it opens; **Look in a folder…** looks in another folder (with the folders inside it), and **Look again** checks once more. A file is recognised by its size and content, so a renamed file is found too. **Relink N tracks** points the ticked tracks at their files again; their ratings, play counts and places in playlists stay. **Remove N from the library** removes the tracks that were not found; the notice offers **Undo** for a few seconds before they go.

## Verify files

**Tools ▸ Verify Files…** decodes files from start to end and lists those that do not decode cleanly, with a sentence on what was found: parts that could not be decoded (and where), a file that stops early, a length that differs from the one the file states, or — for FLAC — audio that does not match the checksum stored in the file. **Check** chooses the whole library, one library folder, the selected tracks (when opened with **Verify files** from the context menu) or a folder of your choice; **Verify** starts. The tool changes nothing. **Copy as text** and **Save report…** keep the list. The check stops when you close the window.
