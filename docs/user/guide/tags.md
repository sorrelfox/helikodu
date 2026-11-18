# Editing tags

The tag editor changes the tags in your music files: titles, artists, album, track numbers, genres, cover pictures and lyrics.

## Opening the tag editor

Select one or several tracks, then:

- choose **Edit tags…** in their right-click menu,
- press **Alt+Return**, or choose **Edit ▸ Track Properties** in the main menu,
- or, on an album page, click **Edit tags** under **Details** to edit all its tracks.

The **Edit tags** window lists the files on the left, under **Editing**, and has four tabs.

## Tags

**Title**, **Artist**, **Album artist**, **Album**, **Track** and **Disc** (each with its total), **Year**, **Genre** (several genres are possible: type one in **Add genre**), **Composer**, **Comment** and **Rating**. Other fields that are in the files are listed under **Other fields**.

When you edit several tracks, a field whose files have different values shows `<keep>` and how many different values there are. Leave it untouched and every file keeps its own value; type something and all files get it. Changed fields are marked **Edited**.

## Cover

Shows the front cover of the files. **Choose image…** takes a JPEG or PNG picture as the new front cover, **Remove** removes it, and **Keep the cover** undoes your choice before you save.

## Lyrics

The lyrics of one track; with several tracks selected, open the editor for one track to change them. Lines that start with a time, such as `[01:23.45]`, are kept as they are; the lyrics panel that shows them in step with the music comes in a later version. **Clear** removes them.

## File info

The facts about the file, which you cannot change here: name, folder, size, the audio format, the tags in the file, the loudness values (ReplayGain), and what the library knows (when it was added, last played, plays and skips). **Open folder** shows the file in your file manager.

## Saving

**Save N files** writes the tags into the files, one file after another. Each file is written to a copy, verified, then swapped in, so a power failure cannot damage it (very large files are an exception; see **Very large files** in [Settings](settings.md#advanced)). The library is updated at once; there is no need to rescan.

- A toast says **Saved N files**. If some files could not be saved, the editor stays open, those files are marked in the list with the reason, and a toast says how many could not be saved (**N of M files could not be saved**).
- If another program changed a file since the editor opened it, Helikodu does not overwrite it: **The file changed since it was opened. Open it again to edit it.**
- A track of a CUE sheet shares its file with other tracks, so its tags cannot be changed here. Files in formats whose tags Helikodu cannot write (Matroska, WMA/ASF, DSF, DFF, TAK and TTA) also keep their tags. The editor says so above the fields (**The tags of these files cannot be changed here**, or **N files keep their tags** when only some of the selected files are affected).

**Cancel** closes the editor without changing anything.

## Ratings

The **Rating** you give in the tag editor, like **Rate** in the track menu, is kept in Helikodu's library, not in the files. **Settings → Advanced** shows **Also write ratings to the tags of the files**, but in this version that switch cannot be turned on yet.

In **Settings → Advanced**, **Tags in MP3 files** chooses the ID3 version of new tags in MP3 files (ID3v2.4 by default; ID3v2.3 for old car stereos and portable players).
