# Browsing and searching the library

The main window has three parts: the **sidebar** on the left, the **content** in the middle and the **panel** on the right, which shows the queue. Drag the borders between them to change their widths. **F9** hides or shows the sidebar, the **Show the queue** button in the header shows or hides the panel, and **F6** moves the keyboard focus from one part to the next. The **View** submenu of the main menu has the same switches: **Sidebar**, **Queue Panel** and **Menu Bar**.

The sidebar lists the views of your library (**Albums**, **Artists**, **Tracks**, **Genres** and **Folders**) and your playlists. **Alt+Left** and **Alt+Right**, or the **Back** and **Forward** buttons in the header, go back and forward between the pages you visited.

## Search

Type in the search field at the top of the window (**Search artists, albums, tracks**), or press **Ctrl+F** to go there. The results appear while you type, under **Artists**, **Albums** and **Tracks**, each with the number found. Search looks at titles, artists, albums, genres, composers and file paths.

- **Up** and **Down** move through the results without leaving the field; **Enter** plays a track or opens an artist or album.
- Only the first tracks are listed. **Show all N tracks** opens the **Tracks** view with all of them.
- **Escape** empties the field and goes back to where you were. When nothing is found, **Clear search** does the same.

### The query language

Text with a field and an operator is a query: `genre:jazz -genre:fusion`, `rating>=4`, `added<30d sort:added desc`. The field then switches to a fixed-width font, and the results are a table of the tracks the query selects (**Title**, **Artist**, **Album**, **Genre**, **Added**, **Length**). Double-click a track or press **Enter** to play the results from there; a click on a column header sorts them.

- **Fields:** `title`, `artist`, `albumartist`, `album`, `composer`, `performer`, `conductor`, `genre`, `year`, `codec`, `samplerate`, `bitdepth`, `plays`, `skips`, `rating`, `duration`, `added`, `lastplayed`, `path`, `folder`, `kind` and more; any other tag as `tag:NAME` (`tag:mood=calm`).
- **Operators:** `:` contains, `=` equals, `!=` differs, `<`, `<=`, `>`, `>=`, and `~` for a regular expression. A range is `year:1970..1979`; a relative date is `added<30d` (days, `w` weeks, `m` months, `y` years).
- **Combining:** words next to each other must all hold; `OR` gives either; a minus or `NOT` excludes; parentheses group.
- **Order and size:** `sort:added desc` orders the results, `limit:50` keeps the first 50.
- `has:lyrics`, `has:cover`, `has:rating` test for a value; `is:lossless`, `is:hires`, `is:loved` for a kind of track.

While you type a field name, a list under the field offers the fields that start with it, with an example of each; after `kind:`, `codec:`, `has:` or `is:` it offers the values. **Tab** completes, **Up** and **Down** choose, **Escape** closes the list.

When part of a query cannot be read, it is underlined, and a notice says what is wrong with an example that works. The table keeps showing the last query that could be read.

The column on the right shows four example queries; click one to search with it.

### Saved searches

**Save search** above the results saves the search under a name. Saved searches are listed under **Saved searches** in the right column of the search page, each with the number of tracks it finds now. Click one to search with it again. Right-click it (or press the Menu key) to **Rename** or **Delete** it; **F2** renames and **Delete** deletes the selected one.

Tags without a field of their own (such as `MOOD`, `BPM`, `GROUPING`, `LABEL` or `COMMENT`) are found with `tag:NAME`. Files scanned by an earlier version get them the next time they are read, for example after they change.

## Albums

**Albums** shows your albums as a grid of covers. **Sort by** orders them by **Album artist**, **Title**, **Year** or **Date added**, and the quick filter shows **All** albums, only **Lossless** ones or only **Hi-res** ones. Albums whose tracks have no album tag are grouped under **Unknown album**; compilations are listed under **Various artists**.

Covers come from pictures embedded in the files and from image files in the album folder, such as `cover.jpg`, `folder.png` or `front.webp`. Albums without a cover get a generated placeholder.

Double-click an album cover to play the album at once. A single click opens its page: the cover, the album's details (release date, genre, size, folder) and its tracks, grouped by disc.

On the album page, **Play** plays the album from the start, **Shuffle** in random order, and **Add to queue** adds it to the end of the queue; **More** opens the album's menu. Click a track to play the album from that track. Under **Details**, **Edit tags** opens the [tag editor](tags.md) for all its tracks and **Open folder** shows the album folder in your file manager.

## Artists

**Artists** lists the album artists on the left, under letter headings; sort them by **Name** or **Number of albums**. Choose an artist to see their page: **Play all**, **Shuffle**, and **More** (**Play next**, **Add to queue**), with two tabs:

- **Releases**: the artist's **Albums**, **EPs and singles**, the albums they appear on (**Appears on**), and their **Most played** tracks.
- **All tracks**: all their tracks in a track table, as in the Tracks view.

## Genres

**Genres** lists the genres on the left, sorted by **Name** or **Number of albums**. Choose a genre to see its albums as a grid, with **Play all** and **Shuffle**. The albums can be sorted by **Album artist**, **Title**, **Year** or **Date added**.

## Tracks

**Tracks** lists every track of the library in a table. The **Filter** field narrows it, and **Columns** shows or hides columns: **#**, **Title**, **Artist**, **Album**, **Year**, **Format**, **Rating**, **Plays** (hidden at first) and **Length**. Drag a column header to move the column, and its edge to change its width. Click the stars in the **Rating** column to rate a track.

Click a column header to sort by it: the first click sorts in ascending order, the second in descending order, and the third turns that column's sorting off. Every column can be sorted. Sorting by several columns at once works by clicking them one after another: the column clicked last decides first, and the columns clicked before it decide among equal values, up to four columns. The headers show the order with arrows and, with more than one column, numbers. The status bar at the bottom says what the table is **Sorted by**; without your own order, tracks are sorted by artist, album, disc and track number.

Double-click a track, or press **Enter**, to play from that track. **Ctrl+A** selects all tracks.

## Folders

**Folders** shows your library folders and the folders inside them as they are on disk, as a tree on the left; the **+** button (**Add a library folder**) adds another library folder. Choose a folder to see the audio files in it, with **File name**, **Title**, **Length** and **Size**, and whether the folder is in the library (**In the library · watched for changes**, **In the library · rescanned now and then**, **In the library · not connected**, or **Not in the library**). **Play** and **Add to queue** act on the files of that folder, and **Open in file manager** shows it in your file manager.

## The track menu

Right-click a track for its menu, or press the **Menu** key or **Shift+F10**. With several tracks selected, the menu acts on all of them.

| Entry | What it does |
|---|---|
| **Play** | Plays it now |
| **Play next** | Puts it in the queue after the playing track |
| **Add to queue** | Adds it to the end of the queue |
| **Add to playlist** | Adds it to a playlist, or to a **New playlist** |
| **Go to album**, **Go to artist** | Opens that page |
| **Edit tags…** (**Alt+Return**) | Opens the [tag editor](tags.md) |
| **Rate** | Gives it one to five stars, or **Not rated** |
| **Open containing folder** | Shows the file in your file manager |
| **Remove from playlist**, **Remove from queue** (**Delete**) | Only in a playlist or the queue |

An album's menu has **Play**, **Play next**, **Add to queue**, **Add to playlist**, **Go to album** and **Go to artist**; an artist's or genre's menu has the first four.
