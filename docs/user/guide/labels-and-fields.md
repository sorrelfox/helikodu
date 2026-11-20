# Labels and your own fields

Helikodu keeps two kinds of information of your own beside the tags of your files: **labels**, free words you give tracks, albums and artists, and **fields** you define, either computed from other fields or standing for a tag. Neither is written to your files. Labels are given in the window; fields are made with [`helikodu-cli`](command-line.md#helikodu-cli), and the window finds and uses them in searches, smart playlists and naming patterns.

## Labels

A label is any word or short phrase, such as `late night`, `to sort` or `road trip`, given to tracks, to an album or to an artist.

- A label of an **album** counts for every track of the album, and a label of an **artist** for every track the artist plays or is album artist of. `label:"road trip"` finds them all.
- Labels are told apart without regard to capitals and accents: `Jazz` and `jazz` are one label, written the way it was given last.
- A track keeps its labels when its file is moved or renamed, when the library is scanned again, and when it leaves the library and comes back later, just as it keeps its plays and rating. An album keeps its labels when it is scanned again, and an artist's labels follow the artist's name.
- A label that nothing has any more is removed.

### Giving labels in the window

Choose **Labels…** in the menu of selected tracks, of an album or of an artist, or click **Add label** under the title of an album or artist page. A list of your labels opens with a box beside each:

- A ticked box means everything you chose has the label; a box with a dash means only part of it has it. Click a label to give it to everything you chose, click it again to take it from all of it.
- A label the tracks have only through their album or artist is greyed out and says "from the album" or "from the artist": change it on the album or the artist.
- Type to find a label. Press Enter to give the label you typed, a new one or one you already have.
- Each click counts at once and the list stays open. The first change shows a notice with **Undo**, which takes back everything you did until the list closed; Ctrl+Z does the same while the notice is shown.

The labels of an album or an artist show as small tags under its title; click one to see everything with that label.

**Labels** in the sidebar (it appears once you have a label) and **Library ▸ Labels** in the main menu open the list of your labels, with the number of tracks each covers. Choosing one shows the artists, albums and tracks it was given to, with **Play all** and **Shuffle**, which play every track that has it. Right-click a label (or use the button with three dots) to:

- **Rename** it, also with F2. Renaming it to the name of another label offers to merge the two.
- **Merge into** another label: everything that has it gets the other label instead, and it disappears.
- **Remove from everything**.

Right-click an artist, album or track shown under a label and choose **Remove the label** to take the label from it alone. Each of these changes shows a notice with **Undo** as well.

The record label of an album, from the `LABEL` tag of its files, is shown as **Record label** among the album's details; it has nothing to do with your labels.

### Labels from the command line

```sh
helikodu-cli labels add "late night" --query 'genre:jazz year<1970'
helikodu-cli labels add favourite --album-of 1234     # the album of track 1234
helikodu-cli labels add "road trip" --artist "Mari Kask"
helikodu-cli labels list
helikodu-cli labels show --track 1234
helikodu-cli labels remove favourite --album-of 1234
helikodu-cli labels rename "late night" "after hours"
helikodu-cli labels merge "after hours" jazz      # "after hours" goes into "jazz"
```

The track ids are the ones `helikodu-cli search` prints. `--query` takes the tracks a search of the [query language](browsing.md#the-query-language) finds.

### Finding labelled tracks

In the [query language](browsing.md#the-query-language), `label` is a text field like `genre`:

| Query | Finds |
|---|---|
| `label:jazz` | tracks with a label that contains "jazz" |
| `label=jazz` | tracks with the label "jazz" itself |
| `-label:jazz` | tracks without such a label |
| `has:label` | tracks with any label |
| `label~^road` | a regular expression on the labels |
| `sort:label` | ordered by the first label of each track |

Plain search text finds labels too: searching `jazz` lists the tracks labelled `jazz` along with those whose title, artist or genre has the word. Naming patterns can use labels as well: `{join(label)}` writes all of a track's labels.

In the main window, the **Labels** column of **Tracks** shows each track's labels, the **Browse** lists can list them (choose **Label** in a list's head), and the search field's completion offers your labels after `label:` (see [Browsing](browsing.md)).

The label field has nothing to do with the record label in a file's tags, which is the tag `LABEL` and is found with `tag:LABEL`.

## Your Fields

**Library ▸ Your Fields…** opens a window with your fields at the left and **New field** under them. Choose one to change it:

- **Name**: the name you write in searches and patterns (see the rules [below](#rules-for-names-and-definitions)).
- **Kind**: **Computed from other fields** or **A tag of the files**.
- **Expression** (for a computed field): the expression, over several lines if you like. **Insert** beside it adds a field, one of your own fields or a function at the cursor (see [Expressions](expressions.md#the-insert-button)). For a tag, **Tag name** takes the tag instead.
- **Example values** show what the field gives for the tracks selected in the main window, or, when nothing is selected, for the first tracks of the library. Type a search into **Or the tracks of a search** to see the first tracks it finds instead. The values follow the expression as you type.

A name or an expression that cannot be used is underlined, and a note under it says what is wrong and at which character, often with an example that works. Nothing is saved while there is a problem. **Save** (**Add field** for a new one) puts the field in use at once: searches, smart playlists, naming patterns and the Insert lists know it from then on. **Remove** removes the field; it cannot be used while another field uses this one, and the line at the bottom says which. Choosing another field or closing the window with changes that are not saved asks whether to save them.

## Computed fields

A computed field is a name for an [expression](expressions.md): its value for a track is the text the expression gives.

```sh
helikodu-cli fields define decade '{left(year,3)}0s'
helikodu-cli fields define era '{if(decade, decade, "unknown")}'
helikodu-cli fields value decade 1234
```

Once defined, the name works wherever a field does:

- in **naming patterns** of the organizer, the converter and folder sync: `{decade}/{albumartist}/{album}`;
- in **expressions** of other computed fields, as `era` uses `decade` above, as long as none of them comes back to itself;
- in **searches** and smart playlists, compared as text: `decade=1990s`, `decade:199`, `era~^19`, `has:decade`. Helikodu works the field out for every track before such a search runs; on a library of a few thousand tracks that takes a fraction of a second, on a very large library noticeably longer.

A search cannot sort by a computed field yet: `sort:decade` is not accepted, and the column of a computed field in **Tracks** cannot be sorted either. Each of your fields can be shown as a column of **Tracks** (**Columns ▸ Your fields**), and the search field's completion offers them among the other fields.

## Custom fields

A custom field is a name for a tag that has no field of its own, so that `mood:calm` works where you would otherwise write `tag:MOOD:calm`:

```sh
helikodu-cli fields define mood --tag MOOD
```

Give the tag as its Vorbis comment name. `TXXX:MOOD` (MP3) and `----:com.apple.iTunes:MOOD` (MP4) mean the same tag and are accepted too; Helikodu reads all three spellings into one. The field then reads like `tag:MOOD` in searches, naming patterns and expressions, and a search with it is saved as `tag:MOOD`, so it keeps working if the field is removed. The tag editor writes the tag the way it writes any other field.

## Rules for names and definitions

- A field's name has only the letters a to z and `_`, starts with a letter, and is at most 32 characters long. Capitals and small letters count as the same.
- It cannot be the name of a field Helikodu already has (`year`, `length`, `label` and the others of the query language), a function of the expression language (`upper`, `left`, …), or one of the words `and`, `or`, `not`, `sort`, `limit`, `has`, `is`, `tag`, `random`, `asc`, `desc`, `lyrics` and `cover`.
- A definition that cannot be used is refused with what is wrong and where: `helikodu-cli fields define` marks the characters under the definition.
- A field that a computed field uses cannot be removed until that computed field is changed or removed.

Labels and fields are kept in the library database, so a [backup](files.md) of the library keeps them. `helikodu-cli export-user-data` writes them with the rest of your user data, to read into another library with `import-user-data` (see [Moving your data to another computer](moving-from-another-player.md#moving-your-data-to-another-computer)). The running application uses fields changed from the command line after it is started again; a field saved in Your Fields applies at once.
