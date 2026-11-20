# Organizing, checking and finding files

Three tools in the **Tools** and **Library** menus work on the files of your library. Each opens in a window of its own; nothing changes before you press the button at the bottom right.

## Organize files

**Tools ▸ Organize Files…** moves and renames your files by a naming pattern. Opened from the context menu of selected tracks (**Organize files…**), it works on those tracks only.

- **Patterns** at the left are saved patterns; edits to the pattern are kept with the chosen one. **New pattern** adds one. Right-click a pattern (or press the Menu key) to **Rename** or **Delete** it.
- A pattern names fields in braces (see [Naming patterns](#naming-patterns) below). **Insert** beside the pattern finds a field, one of your own fields or a function and puts it at the cursor (see [Expressions](expressions.md#the-insert-button)). The file's extension is always kept.
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

## The managed folder

A managed folder is a folder of your library that Helikodu keeps in order for you, as iTunes and Apple Music do with their media folder: files you import from elsewhere (a downloads folder, a memory stick) are copied or moved into it and named by a naming pattern, and the files of your library that are elsewhere can be gathered into it. It is off unless you turn it on in **Settings ▸ Library ▸ Managed folder** with **Choose a folder…**. `helikodu-cli managed-folder` does the same from the command line (see [The command line](command-line.md#the-managed-folder)).

- **The setting** (`[library.managed]` in the settings file), in Settings ▸ Library: the folder (**Change…**, and **Turn off**, which asks nothing and leaves every file where it is), **Names**, a naming pattern (the organizer's first pattern, `{albumartist}/{album}/[{disc}-]{track} {title}`, unless you choose another, with an example of a name under it), **When music is added**: **Copy the files** (the default) or **Move the files**, and **Characters in names**: names that Windows can read, names as they are, or plain ASCII only. A new pattern is for music added from then on: files already in the folder keep their names, and Organize files renames them. A pattern that cannot be read is explained under the entry, and the one before stays in use. A folder outside your library folders becomes a library folder when you choose it, and the group says so.
- **Adding music.** **Add Music to the Managed Folder…** in the main menu (and in File of the menu bar), **Add music…** in Settings, or files and folders dropped on the main window while the managed folder is on open the plan of adding them: the places they come from at the left (**Add more…** adds folders, and **Delete** takes the focused one out again), five figures (to copy, get a number, already there, left where they are, and the space the files need and the space that is free), and every file with what happens to it and, for a file that stays, why. **Copy** or **Move** above it is for this run only, the filter and the dropdown narrow the list. Nothing changes until you press **Copy N files** (or **Move N files**). When the files do not fit, a notice says so and the run cannot start; the plan is worked out again when you change the places or Copy and Move, and when the window gets the focus back. Without a managed folder, dropped files play as before.
- **Gathering the library.** **Library ▸ Gather into the Managed Folder…** and **Gather the library into it…** in Settings show the same window for the library files outside the managed folder, by library folder; a folder that is not connected keeps its files. Settings shows how many library files are outside the folder.
- **While a run works** the rows show what is placed and the footer the progress and **Stop**; the sidebar shows it too, and Settings gives the count. Closing the window does not stop the run. Afterwards the figures say what was done, and **Show in the library** opens Tracks with the folders the music went to; when the window is closed, a notice says how many files were added, with **Show**. A finished run cannot be undone.
- **Importing** takes files and folders from outside the library. Each audio file goes where the pattern puts it below the managed folder, named by its tags. Its sidecar files go with it: the covers, booklets (PDF) and rip logs of a folder go to the folder that gets most of its tracks; lyrics (`.lrc`) go beside their track under its new name; and a CUE sheet goes beside the file it describes, with its `FILE` lines changed to the new names (a sheet that names a `.wav` file that was later compressed is matched too). Of a file you name on its own, only its lyrics and its CUE sheet go with it. Other files, such as text notes, stay where they are. Files that are already in the library are left alone: consolidate them instead.
- **Consolidating** gathers the library files outside the managed folder into it, named by the pattern, with their sidecar files. The tracks stay the same tracks: their ratings, play counts, history, labels and places in playlists follow the files. When the files are copied, the library follows the copies and the originals stay on the disk.
- **Names that are taken.** Nothing is ever replaced. When an identical file is already at the destination, the file is skipped; when another file has the name, the new one gets a number: `01 Tidewater (2).flac`.
- **The plan first.** Every import and consolidation is shown first, in the window or with `--dry-run`: each file with where it goes, the names that got a number, the files that are skipped and why, and the space the files need and the space that is free. A run does not start when the files do not fit.
- **Safe at every step.** A copy is written under a temporary name, flushed to the disk, renamed into place and compared with the original byte for byte; only then is a moved file's original removed. Across drives a move is such a copy followed by the removal. When the original of a move cannot be removed (a read-only folder or memory stick), the file is copied and the original stays. Every file of a run is written down in the library before it is touched, so a run that is cut short (**Stop**, Helikodu is closed, the computer stops, the drive is pulled) loses nothing. A notice with **Details…** says so, when it happens and at the next start; the dialog it opens (also from **Details…** in Settings and in the window) shows what was placed and what waits, and offers **Continue**, which places the rest, and **Roll back**, which removes the copies the run made and moves moved files back. Closing the dialog decides nothing, and no new run starts until it is settled. From the command line: `helikodu-cli managed-folder resume` and `helikodu-cli managed-folder roll-back`.
- **The library learns the new files at once**, by reading only the folders they went to; there is no need for a full scan. Folders that a move leaves empty are removed.

## Renaming one file and moving files to the trash

The context menu of a track has **Rename file…**, which renames the file in its folder (also undoable from the notice), and **Move to trash…**, which moves the files of the selected tracks to the desktop Trash after a confirmation. From the Trash you can restore them with your file manager.

## Missing files

**Library ▸ Missing Files…** lists the tracks whose file has gone, by the library folder they were in, with how long they have been missing. A track leaves the library when it has been missing for the number of days set in [Settings ▸ Library](settings.md) (30 by default). Tracks on a drive that is not connected are hidden, not missing, and are not listed.

The window looks for the files in your library folders when it opens; **Look in a folder…** looks in another folder (with the folders inside it), and **Look again** checks once more. A file is recognised by its size and content, so a renamed file is found too. **Relink N tracks** points the ticked tracks at their files again; their ratings, play counts and places in playlists stay. **Remove N from the library** removes the tracks that were not found; the notice offers **Undo** for a few seconds before they go.

When a missing track's recording is in the library again as another file (the same title and artist and a length within 2 seconds, such as an MP3 replaced by a FLAC), it is also listed under **Replaced ▸ Replaced by another copy**, with its old and new file and the plays, rating, history and playlists that would move. **Carry to N tracks** moves them to the new files; see [Moving from another player](moving-from-another-player.md#when-a-file-is-replaced).

## Verify files

**Tools ▸ Verify Files…** decodes files from start to end and lists those that do not decode cleanly, with a sentence on what was found: parts that could not be decoded (and where), a file that stops early, a length that differs from the one the file states, or — for FLAC — audio that does not match the checksum stored in the file. **Check** chooses the whole library, one library folder, the selected tracks (when opened with **Verify files** from the context menu) or a folder of your choice; **Verify** starts. The tool changes nothing. **Copy as text** and **Save report…** keep the list. The check stops when you close the window.
