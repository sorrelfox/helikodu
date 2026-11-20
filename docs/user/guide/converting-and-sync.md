# Converting, syncing and covers

Three tools in the **Tools** menu work on the files of the library: **Convert…**, **Sync and Export…** and **Cover Manager…**. Each opens a window of its own that does not block the main window. Converting and syncing run in the background and go on when you close their window; open it again to see where they stand.

Everything happens on this computer. Helikodu never downloads covers or sends anything anywhere, and the originals in your library are never changed by converting or syncing.

## Convert

**Convert…** converts the tracks of the library into another format, for a phone, an old player or a CD.

- **Presets** on the left: Opus 128 (about 1 MB a minute), Opus 192, MP3 V0, AAC 256, FLAC, ALAC and WAV 16-bit / 44.1 kHz. **New preset** saves the current choices as a preset of your own; right-click one of your own (or press the Menu key, F2 or Delete) to rename or delete it.
- **Format**, **Quality**, **Sample rate** and **Channels**. Opus is always written at 48 kHz. The formats are those the FFmpeg on your computer can write; a format without an encoder says so, and cannot be used.
- **Save to**: the folder the new files go to. **Folders and names**: a naming pattern (see below); **Insert** beside it adds a field or a function at the cursor.
- **Copy tags and covers** writes the tags and the front cover into each new file. **Replace files that are already there** overwrites files of the same name; without it they are left alone.
- **Work on** sets how many files are converted at a time (4 by default).

The table shows each track, its format now and after, and the result. A track that is already in the target format (at no more than its bit rate) is copied as it is. While the conversion runs, the footer shows the progress, what has been written and how long it will still take; **Stop** ends it, and a file that was not finished is removed. Afterwards, **Open folder** shows the files.

Tracks of a CUE sheet are converted into files of their own.

## Naming patterns

A pattern says where a file goes and what it is called, from its tags. The converter, folder sync and [Organize Files](file-tools.md#naming-patterns) read patterns the same way, so one pattern gives a track the same name in each of them.

- `{field}` is a field of the track, with the names of the search: `{albumartist}`, `{artist}`, `{album}`, `{title}`, `{track}`, `{disc}`, `{year}`, `{genre}`, `{composer}`, `{tag:MOOD}` and others.
- `/` starts a folder.
- Text in square brackets is left out when a field in it is empty: `{album}[ ({year})]`. Brackets can be nested (an inner bracket is left out on its own) and can hold a folder: `[Disc {disc}/]`.
- Track numbers have two digits (three on an album of a hundred tracks or more). `{disc}` has a value only on an album of several discs, so `[{disc}-]{track}` gives `01` on an album of one disc and `2-01` on the second of two. `{track}` never includes the disc.
- A field that is empty outside square brackets is written as "Unknown".

The default, `{albumartist}/{album}/[{disc}-]{track} {title}`, gives `Mari Kask/Tidewater Rooms/01 Tidewater.opus`. Characters that file names on memory sticks and phones cannot hold are replaced by `_`, and a name is cut to 255 bytes. When two tracks would get the same name, the second gets ` (2)`. A pattern that cannot be read is explained under the entry, with the character where the problem is.

The braces can also hold functions and conditions, such as `{upper(album)}` or `{if(year < 2000, "Old", "New")}`; see [Expressions](expressions.md).

## Sync and export

**Sync and Export…** keeps a folder (a phone, a memory stick, another player) in step with chosen playlists and searches.

- **Destinations** on the left; **New destination** asks for the folder (before the first one, the window shows **Add destination…**); Cancel leaves everything as it was. A destination whose drive is not connected says so.
- **Copy these**: tick playlists, playlist folders, smart playlists and saved searches. **Add an album, an artist or a search…** takes a search such as `album:"Tidewater Rooms"` or `artist:Okto`.
- **Files**: convert lossless files to a preset (the others are copied), copy every file as it is, or convert everything to a preset.
- **Write each playlist as an .m3u8 file** puts the chosen playlists and searches into a `Playlists` folder on the destination, so the other player finds them.
- **Remove files that are no longer chosen** removes what you untick at the next sync.

The figures show how many tracks will be copied and converted, how many are there already, how many files will be removed, and the space needed with the space that is free. **Check again** looks again; **Sync now** starts.

Copies go one way, from the library to the destination. Helikodu removes only files it put there itself: it keeps a list of them in `.helikodu-sync.json` at the top of the destination. Files that someone else put there are left alone, also when one has the name a copy would get.

A playlist's **Export** button offers **Copy the files to a folder…**, which opens this window with the playlist ticked.

## Cover manager

**Cover Manager…** shows which albums have a cover, and where:

- **No cover**: neither inside the files nor in the folder. Any picture in the album folder (or a folder below it, such as `booklet/`) is offered: **Use it**. Otherwise **Choose image…** takes a JPEG or PNG file.
- **Only in the folder**: **Put it into the files**.
- **Only inside the files**: **Save as cover.jpg** writes the picture beside the files.
- **Smaller than 500 px** and **Not the same in every file**.

Tick albums and use the button at the right of the footer to do the same for all of them. **Save** chooses where a picture goes: into the files and as `cover.jpg`, into the files only, or as `cover.jpg` only (a PNG picture is saved as `cover.png`). **Remove** takes the front cover out of the files of the ticked albums; pictures in the folders stay.
