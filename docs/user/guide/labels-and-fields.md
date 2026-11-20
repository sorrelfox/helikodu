# Labels and your own fields

Helikodu keeps two kinds of information of your own beside the tags of your files: **labels**, free words you give tracks, albums and artists, and **fields** you define, either computed from other fields or standing for a tag. Neither is written to your files. For now they are made with [`helikodu-cli`](command-line.md#helikodu-cli); the window finds and uses them in searches, smart playlists and naming patterns.

## Labels

A label is any word or short phrase, such as `late night`, `to sort` or `road trip`, given to tracks, to an album or to an artist.

- A label of an **album** counts for every track of the album, and a label of an **artist** for every track the artist plays or is album artist of. `label:"road trip"` finds them all.
- Labels are told apart without regard to capitals and accents: `Jazz` and `jazz` are one label, written the way it was given last.
- A track keeps its labels when its file is moved or renamed, when the library is scanned again, and when it leaves the library and comes back later, just as it keeps its plays and rating. An album keeps its labels when it is scanned again, and an artist's labels follow the artist's name.
- A label that nothing has any more is removed.

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

The label field has nothing to do with the record label in a file's tags, which is the tag `LABEL` and is found with `tag:LABEL`.

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

A search cannot sort by a computed field yet: `sort:decade` is not accepted.

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

Labels and fields are kept in the library database, so a [backup](files.md) of the library keeps them. `helikodu-cli export-user-data` writes them with the rest of your user data, to read into another library with `import-user-data` (see [Moving your data to another computer](moving-from-another-player.md#moving-your-data-to-another-computer)). The running application uses fields changed from the command line after it is started again.
