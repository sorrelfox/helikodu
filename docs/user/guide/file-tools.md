# Organizing, checking and finding files

Three tools in the **Tools** and **Library** menus work on the files of your library. Each opens in a window of its own; nothing changes before you press the button at the bottom right.

## Organize files

**Tools ▸ Organize Files…** moves and renames your files by a naming pattern. Opened from the context menu of selected tracks (**Organize files…**), it works on those tracks only.

- **Patterns** at the left are saved patterns; edits to the pattern are kept with the chosen one. **New pattern** adds one.
- A pattern names fields in braces, with the field names of the search language: `{albumartist}/{year} {album}/[{disc}-]{track} {title}`. A `/` starts a folder. Text in square brackets is left out when a field inside it is empty, so `[{disc}-]` only appears on albums of several discs. Track numbers have two digits. The file's extension is always kept. **Insert field** adds a field at the cursor. A field that is empty outside brackets is written as "Unknown".
- **Into** chooses where files go: the library folder each file is already in, one of your library folders, or a folder you choose inside a library folder.
- The second dropdown chooses the characters names may hold: **Names that Windows can read** (for drives you share with other systems), **Names as they are**, or **Plain ASCII only** (accents are dropped).
- **Move covers and booklets too** moves the pictures and PDF files of a folder along when every track of that folder goes to the same new folder.

The table shows each file's path now and after the move, and what happens to it: **Move**, **Rename**, **Move and rename**, **Already in place**, or **Name taken** when a file with that name exists already (such files are not ticked and are never replaced). Untick a row to leave that file where it is; Space ticks the selected row, and a double click plays it.

**Move N files** writes every move down before it makes it, so a run can be undone: with **Undo** in the notice that follows, with **Undo the last run** in the window, or later, even after a restart. After an undo the button offers **Redo the last run**. A file that cannot move (or move back) stays where it is and the window says why. Across drives a file is copied, checked byte for byte, and only then removed from its old place. Folders that a run leaves empty are removed, and an undo puts them back. Ratings, play counts, history and playlist entries stay with the tracks.

Moves older than the limit of the undo history in [Settings ▸ Library ▸ Storage](settings.md) (90 days by default) can no longer be undone.

## Renaming one file and moving files to the trash

The context menu of a track has **Rename file…**, which renames the file in its folder (also undoable from the notice), and **Move to trash…**, which moves the files of the selected tracks to the desktop Trash after a confirmation. From the Trash you can restore them with your file manager.

## Missing files

**Library ▸ Missing Files…** lists the tracks whose file has gone, by the library folder they were in, with how long they have been missing. A track leaves the library when it has been missing for the number of days set in [Settings ▸ Library](settings.md) (30 by default). Tracks on a drive that is not connected are hidden, not missing, and are not listed.

The window looks for the files in your library folders when it opens; **Look in a folder…** looks in another folder (with the folders inside it), and **Look again** checks once more. A file is recognised by its size and content, so a renamed file is found too. **Relink N tracks** points the ticked tracks at their files again; their ratings, play counts and places in playlists stay. **Remove N from the library** removes the tracks that were not found; the notice offers **Undo** for a few seconds before they go.

## Verify files

**Tools ▸ Verify Files…** decodes files from start to end and lists those that do not decode cleanly, with a sentence on what was found: parts that could not be decoded (and where), a file that stops early, a length that differs from the one the file states, or — for FLAC — audio that does not match the checksum stored in the file. **Check** chooses the whole library, one library folder, the selected tracks (when opened with **Verify files** from the context menu) or a folder of your choice; **Verify** starts. The tool changes nothing. **Copy as text** and **Save report…** keep the list. The check stops when you close the window.
