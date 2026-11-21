# Editing tags

The tag editor changes the tags in your music files: titles, artists, album, track numbers, genres, cover pictures and lyrics.

## Opening the tag editor

Select one or several tracks, then:

- choose **Edit Tags…** in their right-click menu,
- press **Alt+Return**, or choose **Edit ▸ Track Properties** in the main menu,
- or, on an album page, click **Edit tags** under **Details** to edit all its tracks.

The **Edit tags** window lists the files on the left, under **Editing**, and has four tabs.

## Tags

**Title**, **Artist**, **Album artist**, **Album**, **Track** and **Disc** (each with its total), **Year**, **Genre** (several genres are possible: type one in **Add genre**), **Composer**, **Comment** and **Rating**. Other fields that are in the files are listed under **Other fields**.

When you edit several tracks, a field whose files have different values shows `<keep>` and how many different values there are. Leave it untouched and every file keeps its own value; type something and all files get it. Changed fields are marked **Edited**.

## Cover

Shows the front cover of the files and says whether they share it. **Choose image…** takes a JPEG or PNG picture as the new front cover (you can also drop a picture on the area below the buttons), **Save as a file…** saves the current front cover as a picture file, **Remove** removes it, and the button **Keep the cover** undoes your choice before you save.

Under **Keep the cover**, a new picture goes **Inside each file** (ticked by default) and, if you tick it, also **As cover.jpg in the album folder** (`cover.png` for a PNG picture). A folder that already has that file keeps it.

## Lyrics

The lyrics of one track; with several tracks selected, open the editor for one track to change them. Lines that start with a time, such as `[01:23.45]`, are kept as they are, and the **Lyrics** tab of the right panel shows them in step with the music. **Clear** removes them.

## File info

The facts about the file, which you cannot change here: name, folder, size, the audio format, the tags in the file, the loudness values (ReplayGain), and what the library knows (when it was added, last played, plays and skips). **Open folder** shows the file in your file manager.

**Analysis**, under the two columns, shows what the background analysis found for the track, so you can see why a crossfade starts where it does: a curve of how loud the track is over its length (one bar for every two seconds, silence as flat bars, and a dashed line where the fade-out starts), where the sound starts and ends, the silences inside the track and where the fade-out starts. Until the track is analysed it says so; a file that could not be read for the analysis is tried again when it changes. Nothing here is part of **Save**.

## Saving

**Save N files** writes the tags into the files, one file after another. Each file is written to a copy, verified, then swapped in, so a power failure cannot damage it (very large files are an exception; see **Very large files** in [Settings](settings.md#advanced)). The library is updated at once; there is no need to rescan.

- A toast says **Saved N files**. If some files could not be saved, the editor stays open, those files are marked in the list with the reason, and a toast says how many could not be saved (**N of M files could not be saved**).
- If another program changed a file since the editor opened it, Helikodu does not overwrite it: **The file changed since it was opened. Open it again to edit it.**
- A track of a CUE sheet shares its file with other tracks, so its tags cannot be changed here. Files in formats whose tags Helikodu cannot write (Matroska, WMA/ASF, DSF, DFF, TAK and TTA) also keep their tags. The editor says so above the fields (**The tags of these files cannot be changed here**, or **N files keep their tags** when only some of the selected files are affected).

**Cancel** closes the editor without changing anything.

## Undo

Every save can be undone. The toast **Saved N files** has an **Undo** button, and **Ctrl+Z** in the main window undoes the newest tag change even after the toast is gone (unless a playlist is open, where Ctrl+Z undoes the playlist's last change). Tag changes and runs of [Organize Files](file-tools.md) share one history: Ctrl+Z undoes whichever came last, and a notice says what was undone, such as "Undid tag edit of 3 files" or "Undid moving 120 files". **Ctrl+Shift+Z** does it again. Each file gets back the tags, cover and lyrics it had before; a file that was changed since (by another program, or by a later edit you did not undo) keeps its tags, and the toast says how many. Helikodu keeps the last 100 changes, for 90 days.

## Tag tools

The **Tag tools** button at the end of the tab strip opens a window with tools for the files in the editor. Each tool shows every file with the value it would get: in normal text when it changes the tag, in grey when the tag already holds it, with the old value in grey beside a new one. Nothing is written by the tool: its button puts the values into the editor as **Edited** fields, and **Save N files** writes them (and Ctrl+Z undoes them). A field that gets a different value in each file shows `<N new values>`; typing in it replaces them.

- **Fill tags from file names**: a **Pattern** such as `{track} - {artist} - {title}`; **Insert field** adds a field. The fields are `{track}`, `{disc}`, `{title}`, `{artist}`, `{albumartist}`, `{album}`, `{year}`, `{genre}`, `{composer}`, `{comment}`, and `{skip}` for a part you do not need. The extension is ignored; with a `/` the pattern also covers folder names, as in `{album}/{track} {title}`. Untick a field under **Fill** to leave it, or tick **Only where the tag is empty**. File names that do not fit say **Does not fit the pattern**.
- **Number the tracks**: numbers the files in the order of the editor, from the number you choose, **Start again for each disc** (by the disc number), and **Write the total**.
- **Change capital letters**: Title Case, Sentence case, lower case or UPPER CASE, for the fields you tick.
- **Find and replace**: plain text, or a **Regular expression** (use `$1` for a group in the replacement); **Match capital letters** makes the search case-sensitive.
- **Repair garbled text**: text that old programs stored in the code page of their system (such as `Êèíî` for `Кино`). The charset is detected; choose another if the result is wrong. Only values that change are listed.
- **Create sort names**: fills `ARTISTSORT`, `ALBUMARTISTSORT` and `COMPOSERSORT` where they are empty, as "Beatles, The" and, if ticked, "Surname, Name".

## Editing in the track table

In **Tracks**, press **F2** to edit a cell of the selected row: the column you last clicked, or the title. With several rows selected, the other rows show what you type. **Enter** saves into all selected tracks, **Tab** saves and goes on to the next column (**Shift+Tab** the one before), **Esc** cancels. The columns **#**, **Title**, **Artist**, **Album**, **Genre** and **Year** can be edited; the rating is set on its stars. Saving works as in the tag editor and can be undone with Ctrl+Z.

## Ratings

The **Rating** you give in the tag editor, like **Rate** in the track menu or the stars in the tables, is kept in Helikodu's library. With **Settings → Advanced → Also write ratings to the tags of the files** (off by default), it is also written into the file, so that other players can read it: as `POPM` and `FMPS_Rating` in ID3 tags, `FMPS_RATING` and `RATING` in Vorbis comments and APEv2 tags, and `FMPS_Rating` and `RATING` in MP4 files. A track of a CUE sheet shares its file with other tracks, so its rating stays in the library. Play counts can be written into the files too; see [Moving from another player](moving-from-another-player.md#writing-play-counts-and-ratings-into-the-files).

In **Settings → Advanced**, **Tags in MP3 files** chooses the ID3 version of new tags in MP3 files (ID3v2.4 by default; ID3v2.3 for old car stereos and portable players).
