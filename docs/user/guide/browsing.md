# Browsing and searching the library

The main window has three parts: the **sidebar** on the left, the **content** in the middle and the **panel** on the right, which shows the queue. Drag the borders between them to change their widths. **F9** hides or shows the sidebar, the **Show the queue** button in the header shows or hides the panel, and **F6** moves the keyboard focus from one part to the next. The **View** submenu of the main menu has the same switches: **Sidebar**, **Queue Panel** and **Menu Bar**.

The sidebar lists the views of your library (**Home**, **Albums**, **Artists**, **Tracks**, **Genres**, **Composers**, **Years** and **Folders**) and your playlists. **Alt+Left** and **Alt+Right**, or the **Back** and **Forward** buttons in the header, go back and forward between the pages you visited.

## Home

**Home** shows the albums added most recently and those played most recently, each in one row of covers (as many as fit the window); **Show all** opens Albums sorted by date added, and **History** opens the play history. Below, **Most played** lists the five tracks played most often (double-click one to play the list from there; right-click one for the context menu of tracks), and **Rediscover** lists albums you have not played for three months, or never, with when you played them last; click one to open it.

### A narrow window

When the window is too narrow for the content to keep 560 pixels beside the panes, the panes leave the row and open over the content instead: first the queue panel (below a window width of about 1100 pixels at the usual pane widths), then the sidebar (below about 800 pixels). With the sidebar hidden (**F9**), the queue panel stays beside the content down to about 880 pixels. The header then starts with a sidebar button, without the **Forward** button, and has a queue button at the right. An open pane covers the rest of the content with a dark layer; a click beside it, **Esc**, its key (**F9** for the sidebar) or choosing a view closes it. The player deck stays in use.

The pages make room in a narrow content area too: the list of Artists, Genres, Composers, Years and Folders gets narrower, the buttons of a page's header go under its title, the **Details** column of an album goes under the tracks, and the query syntax and saved searches go under the search results.

Below 720 pixels the deck is compact: the cover, title and artist at the left, **Previous**, **Play** and **Next** in the middle, and at the right a volume button, whose slider opens above it, and the output button with only its lamp and the output's rate and bits. Shuffle and repeat are then in the menu of the **Play** button (right-click or long press).

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
- **Labels:** `label:jazz` finds the tracks with a label containing "jazz", given to them, their album or an artist; `label=jazz` the label itself, `has:label` any label. Plain text finds labels too. See [Labels and your own fields](labels-and-fields.md).
- **Your own fields:** a computed or custom field you defined is named like any other: `decade=1990s`, `mood:calm`. A computed field is compared as text and cannot sort a search.

While you type a field name, a list under the field offers the fields that start with it, with an example of each; after `kind:`, `codec:`, `has:` or `is:` it offers the values. **Tab** completes, **Up** and **Down** choose, **Escape** closes the list.

The list knows your [labels and your own fields](labels-and-fields.md) too. After `label:` it lists your labels with the number of tracks each covers, a name of several words in quotes (`label:"late night"`); with more than 200 labels it lists those that start with the letters typed. Your own fields are offered among the others while their names are typed, and after the name of a custom field it offers the field's values when there are few. A computed field offers no values, and `sort:` with one is refused with a notice.

When part of a query cannot be read, it is underlined, and a notice says what is wrong with an example that works. The table keeps showing the last query that could be read.

The column on the right shows four example queries; click one to search with it.

### Saved searches

**Save search** above the results saves the search under a name. Saved searches are listed under **Saved searches** in the right column of the search page, each with the number of tracks it finds now. Click one to search with it again. Right-click it (or press the Menu key) to **Rename** or **Delete** it; **F2** renames and **Delete** deletes the selected one.

Tags without a field of their own (such as `MOOD`, `BPM`, `GROUPING`, `LABEL` or `COMMENT`) are found with `tag:NAME`. Files scanned by an earlier version get them the next time they are read, for example after they change.

## Albums

**Albums** shows your albums as a grid of covers. **Sort By** orders them by **Album Artist**, **Title**, **Year** or **Date Added**, and the quick filter shows **All** albums, only **Lossless** ones or only **Hi-res** ones. Albums whose tracks have no album tag are grouped under **Unknown album**; compilations are listed under **Various artists**.

Covers come from pictures embedded in the files and from image files in the album folder, such as `cover.jpg`, `folder.png` or `front.webp`. Albums without a cover get a generated placeholder.

Double-click an album cover to play the album at once. A single click opens its page: the cover, the album's details (release date, genre, size, folder) and its tracks, grouped by disc.

On the album page, **Play** plays the album from the start, **Shuffle** in random order, and **Add to queue** adds it to the end of the queue; **More** opens the album's menu. Click a track to play the album from that track. Under **Details**, **Edit tags** opens the [tag editor](tags.md) for all its tracks and **Open folder** shows the album folder in your file manager.

A click on a column heading of the track table (**#**, **Title**, **Plays**, **Rating**, **Length**) sorts the tracks by it, a second click sorts the other way round, and a third goes back to the album's own order; while sorted, the discs' headings are left out, and **Play** and a double-click play the tracks in the order shown. Another album starts in its own order. An audiobook keeps its chapters in order.

Select a track to see its own facts above the album's: its artist when it differs from the album artist, composer, number, length, format, track gain, plays, when it was last played, rating, when it was added, whether it has lyrics, its copies, its file size and its file. With a track selected, **Open folder** shows that track's folder.

Drag the left border of the **Details** column to make it wider or narrower; when the page is too narrow for it beside the tracks, it is under them. The width is kept for every album and audiobook page and the next time Helikodu starts.

## Artists

**Artists** lists the album artists on the left, under letter headings; sort them by **Name** or **Number of Albums**. Choose an artist to see their page: **Play all**, **Shuffle**, and **More** (**Play Next**, **Add to Queue**), with two tabs:

- **Releases**: the artist's **Albums**, **EPs and singles**, the albums they appear on (**Appears on**), and their **Most played** tracks, which have the context menu of tracks.
- **All tracks**: all their tracks in a track table, as in the Tracks view.

### Artist photos

An artist is shown with their photo, round, in the list, at the top of their page and in search results; without a photo, their initials. The photo is, in this order:

1. a picture you chose: right-click the artist in the list or the round picture at the top of their page, or open **More**, and choose **Set Artist Photo…** (a JPEG, PNG or WebP image). You can also drop an image file onto the round picture. Helikodu keeps a copy in `artist-photos` beside the library database (`~/.local/share/helikodu/artist-photos`); your file is left as it is;
2. an `artist.jpg` (or `.jpeg`, `.png`, `.webp`) in the artist's folder: the folder that holds all of their album folders. For an artist with one album folder, that folder and the one above it are looked in. A new or changed file shows the next time the artist list is loaded, for example after a scan.

**Remove Photo** forgets the picture you chose, and the photo from the artist's folder, or the initials, shows again. Photos are never downloaded.

## Genres

**Genres** lists the genres on the left, sorted by **Name** or **Number of Albums**. Choose a genre to see its albums as a grid, with **Play all** and **Shuffle**. The albums can be sorted by **Album Artist**, **Title**, **Year** or **Date Added**.

## Composers

**Composers** lists the people tagged as composers, in the order of their sort names, so "Maurice Ravel" is under R. Choose a composer to see every album with at least one track by them; the second line of a cover names the album artist, that is, the performer. An album with several composers is listed under each of them. **Play all** and **Shuffle** play only the composer's tracks, album by album.

## Years

**Years** lists the decades and, under each, its years, newest first, with the number of albums of each. Choose a year to see its albums, or a decade to see every album of the decade. The year of an album is its original release year when the tags give one, otherwise the year of the release; albums without a year are under **Unknown**, the last row. **Play all** and **Shuffle** play the tracks of the year or decade.

## Audiobooks

Files with the extension `.m4b` are audiobooks in the library, and the audio of video files (when **Include video files** is on) is **Audio from a video**. Under **Kind** in the **Details** column of an album page you can change what kind an album is (**Audiobook**, **Music** or **Audio from a video**); the choice survives rescans.

An audiobook's page shows its chapters with where each starts, a check mark on the chapters you have heard, and how much is left. **Play** continues where you stopped, and **Start again** plays it from the start. Audiobooks are left out of shuffle, and **Play all** in Genres, Composers, Years and on an artist's page leaves them out too.

While a chapter plays, the deck shows the chapter's number and title, and the seek bar covers the whole file with a gap at each chapter; **Previous** and **Next** move by chapter, and a click in another chapter plays it from there.

## Tracks

**Tracks** lists every track of the library in a table. The **Filter** field narrows it, and **Columns** shows or hides columns: **#**, **Title**, **Artist**, **Album**, **Year**, **Format**, **Rating**, **Plays** (hidden at first) and **Length**, and, hidden at first, **Genre**, **Added** and **Labels**: a track's own labels and those of its album and artists, in alphabetical order (the tooltip lists them all). Under **Your Fields** the menu lists each of [your own fields](labels-and-fields.md) as a column too; their values appear as the rows are shown. Drag a column header to move the column, and its edge to change its width. Click the stars in the **Rating** column to rate a track.

Click a column header to sort by it: the first click sorts in ascending order, the second in descending order, and the third turns that column's sorting off. Every column can be sorted except **Labels** and the columns of computed fields, whose headers say so in their tooltip; a custom field sorts like any text column. Sorting by several columns at once works by clicking them one after another: the column clicked last decides first, and the columns clicked before it decide among equal values, up to four columns. The headers show the order with arrows and, with more than one column, numbers. The status bar at the bottom says what the table is **Sorted by**; without your own order, tracks are sorted by artist, album, disc and track number.

Double-click a track, or press **Enter**, to play from that track. **Ctrl+A** selects all tracks.

### Grouping the tracks

**Group by**, beside **Columns**, puts the tracks in groups: by album artist, album, genre, year, format or label, by one of [your own fields](labels-and-fields.md) (a computed field such as `decade` too, although the table cannot be sorted by one), or by **An Expression…** of the [expression language](expressions.md), typed into a small box (**Insert** adds a field) and applied with **Enter**. Each group starts with a row that shows its value and how many tracks it has; tracks without a value form the group **No value**, which comes first. The groups follow their values, and within a group the tracks keep the order the column headers show. Double-click a group row, press **Enter**, or use **Left** and **Right** to close and open it; its context menu acts on all of its tracks. The status bar says what the tracks are grouped by, and **No Groups** turns grouping off.

### Browsing by genre, artist and album

The **Browse** button opens three linked lists above the table: **Genre**, **Album artist** and **Album**, each with the number of tracks of every value and an **All …** row at the top. Choosing a value narrows the lists to its right and the table; **Ctrl**-click and **Shift**-click choose several values. The count in the toolbar then says how many of all tracks are shown. The field of each list can be changed with the button in its head: **Genre**, **Album Artist**, **Album**, **Artist**, **Composer**, **Year**, **Format** or **Label**. A list of labels counts each track under every label it has, its own and those of its album and artists. Text in the **Filter** field filters first, so the lists show only the values of the tracks it finds. **Browse** again closes the lists and shows every track.

## Library statistics

**Library ▸ Statistics** in the main menu counts your library: the tracks (and how many were added this month), albums (and how many have no cover), artists (and composers), the playing time and the size of the files. Below are the **Formats** and their **Quality** (lossless at CD quality, lossless hi-res, lossy), with the share of each; what you listened to this year (plays, time listened, skips, and the share of the library never played); and the most played artists and albums. A row of **Formats** or **Quality** opens **Tracks** with those tracks; an artist or album opens its page. The figures are counted when the page opens; **Copy as text** puts them on the clipboard.

## Tracks that sound alike

**Show Similar Tracks** in the menu of a track, an album or an artist lists the tracks whose sound comes closest to it: how bright each track is, how noisy or tonal, how busy its rhythm and how much its loudness varies. Tags are not used, so the list can cross genres; take it as a guide. For an album or an artist their own tracks are left out.

- **How alike** shows a bar and a word: **Very close**, **Close** or **Somewhat alike**. Tracks that are less alike are never listed.
- The list shows the 50 closest tracks; **Show 50 more** adds the next ones while there are any.
- **Play**, **Add to queue** and **Start radio** act on the list; **Save as playlist** makes an ordinary playlist of it. Tracks play on a double click or **Enter**, have the usual menu, and can be dragged to the queue or a playlist. **Back** returns to where you came from.
- The list needs the [track analysis](settings.md#tracks-that-sound-alike). Until the analysis has reached the track (or while too few tracks are analysed, or with the analysis turned off) the page says so and shows how far the analysis is; the list appears by itself once it is there.

## Folders

**Folders** shows your library folders and the folders inside them as they are on disk, as a tree on the left; the **+** button (**Add a library folder**) adds another library folder. Choose a folder to see the audio files in it, with **File name**, **Title**, **Length** and **Size**, and whether the folder is in the library (**In the library · watched for changes**, **In the library · rescanned now and then**, **In the library · not connected**, or **Not in the library**). **Play** and **Add to queue** act on the files of that folder, and **Open in file manager** shows it in your file manager.

Under **This computer** the tree also shows your home folder, connected drives and the whole **File system**, so you can play folders that are not in the library; a folder that is already in the library is marked **in the library**. Such a folder is read when you open it and nothing about it is stored: plays from there are not counted and ratings are not kept. **Add to library** adds the folder to your library folders.

## The track menu

Right-click a track for its menu, or press the **Menu** key or **Shift+F10**. With several tracks selected, the menu acts on all of them.

| Entry | What it does |
|---|---|
| **Play** | Plays it now |
| **Play Next** | Puts it in the queue after the playing track |
| **Add to Queue** | Adds it to the end of the queue |
| **Add to Playlist** | Adds it to a playlist, or to a **New Playlist** |
| **Go to Album**, **Go to Artist** | Opens that page |
| **Show Similar Tracks** | Opens the tracks that sound most like it (one track only; albums and artists have it too) |
| **Edit Tags…** (**Alt+Return**) | Opens the [tag editor](tags.md) |
| **Rate** | Gives it one to five stars, or **Not Rated** |
| **Open Containing Folder** | Shows the file in your file manager |
| **Remove from Playlist**, **Remove from Queue** (**Delete**) | Only in a playlist or the queue |

An album's menu has **Play**, **Play Next**, **Add to Queue**, **Add to Playlist**, **Go to Album** and **Go to Artist**; an artist's or genre's menu has the first four.
