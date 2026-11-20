# Moving from another player

Helikodu can take over the play counts, ratings and playlists you built up in another player, carry statistics over when you replace a file with a better copy, write play counts and ratings into your files, and move all of its own user data to another computer. For now these are done with [`helikodu-cli`](command-line.md); the application has no window for them yet.

Close Helikodu before you run these commands, so that it does not write to the library at the same time.

## Importing from another player

```sh
helikodu-cli import rhythmbox                 # show what would be imported
helikodu-cli import rhythmbox --apply         # import it
```

Helikodu reads these players:

| Player | What is read | Where it is looked for |
|---|---|---|
| Rhythmbox | play counts, ratings, last plays, dates added; playlists and automatic playlists | `~/.local/share/rhythmbox/rhythmdb.xml` and `playlists.xml` beside it |
| Strawberry | play and skip counts, ratings, last plays; playlists | `~/.local/share/strawberry/strawberry/strawberry.db` |
| Clementine | play and skip counts, ratings, last plays; playlists | `~/.config/Clementine/clementine.db` |
| Lollypop | play counts (popularity), ratings, loved tracks, last plays; playlists | `~/.local/share/lollypop/lollypop.db` and `playlists.db` beside it |
| iTunes or Music | play and skip counts, ratings, loved tracks, last plays and skips, dates added; playlists, smart playlists and playlist folders | an exported `Library.xml`: give its place with `--path` |

The Flatpak versions of these players keep their files under `~/.var/app/`; Helikodu looks there too. If a player keeps its library elsewhere, or you copied it from another computer, name the file (or its folder) with `--path`:

```sh
helikodu-cli import itunes --path ~/Backup/iTunes/Library.xml
```

The other player's files are only read, never changed. Its databases are copied to a temporary folder and read from there, so the player may even be running.

### What the preview shows

Without `--apply`, the command only shows what an import would do:

- how many entries were **matched** to tracks of your library, **by path** or **by tags**; how many are **ambiguous** (they fit more than one track and are left out) and how many are **not in the library** (with the first of them by name);
- how many tracks get more plays and skips, a rating, a first or last play, or an earlier date added; and how many tracks keep their own rating although the other player rated them differently;
- each playlist and what it becomes.

### How entries are matched

An entry is matched to a track by its file first. When the path is not found, by its tags: the same artist and title, and the same album, track number and a length within 2 seconds where both sides have them.

If your music has moved since, or the other library was on Windows or a Mac, tell Helikodu where the old folder is now with `--map OLD=NEW` (as often as needed). Windows paths are matched without regard to case:

```sh
helikodu-cli import itunes --path Library.xml --map 'C:\Users\me\Music=/home/me/Music'
```

### How the statistics are merged

- **Play and skip counts are added** to the ones Helikodu counted. Helikodu remembers what each player's file gave, so importing the same file again later adds only the plays made there since, never the same plays twice.
- **Ratings** are taken only for tracks you have not rated in Helikodu. `--replace-ratings` takes the other player's rating also where you have one.
- The **earliest first play** and the **latest last play** win, and a track's **date added** becomes the earlier one.
- **Loved** tracks are marked loved (`is:loved` finds them).
- **Playlists** are created beside yours. Entries that are not in your library stay in them as missing entries, and come back to life when the file shows up. A playlist imported before from the same file is not made again. `--no-playlists` leaves the playlists out.

Rhythmbox's automatic playlists become smart playlists when each of their rules has a counterpart in the [query language](browsing.md#the-query-language); the others are listed as left out. The smart playlists of iTunes are imported with the tracks they held, as ordinary playlists, because their rules cannot be read from `Library.xml`. The smart playlists of Strawberry, Clementine and Lollypop are left out for the same reason.

### Undoing an import

```sh
helikodu-cli undo-import            # undo the newest import
helikodu-cli undo-import --list     # list the imports
```

An import is undone as a whole, newest first. The counts lose what it added (plays you made since stay), ratings, plays and dates go back where they still hold what it set, and the playlists it made are deleted. **Ctrl+Z** in the application undoes tag edits and file changes, not imports.

## When a file is replaced

When you replace a file with another copy of the same recording, for example an MP3 with a FLAC, the new file comes into the library as a new track without the old one's plays. Once the old file is gone (missing), Helikodu can carry them over:

```sh
helikodu-cli carry-stats            # list what could be carried
helikodu-cli carry-stats --apply    # carry it
```

"The same recording" means the same title and artist and a length within 2 seconds, as in the duplicate finder. The plays, skips, history and playlist entries move to the new track, and its counts add up; the new track keeps its own rating if it has one. When a missing file has several possible replacements, name the pair with `--from OLD --to NEW`. `undo-import` undoes this too.

## Writing play counts and ratings into the files

Helikodu keeps statistics in its library. To let other players and tools see them, they can also be written into the files' tags:

- ratings with **Settings → Advanced → Also write ratings to the tags of the files** (see [Ratings](tags.md#ratings)), or `write_ratings_to_tags = true` in the `[advanced]` table of the settings file;
- play counts with `write_play_counts_to_tags = true` in the same table.

Play counts are written as the counter of the `POPM` frame and as `FMPS_Playcount` in ID3 tags, and as `FMPS_PLAYCOUNT` in Vorbis comments, APEv2 tags and MP4 files. They are not written after every play. While Helikodu runs, it writes them by itself in the background: a few minutes after the last play, never during a scan, and never into the file that is playing (that file waits until another track plays or playback stops). It also writes what is left shortly after it starts, at once when you switch the setting on, and when it closes, for at most about two seconds; whatever does not fit is written next time. Only files whose values changed since they were last written are written, and these writes are not on the Undo history of tag changes. To write them now:

```sh
helikodu-cli write-stats                          # as the settings say
helikodu-cli write-stats --ratings --play-counts  # these, whatever the settings say
```

A track of a CUE sheet shares its file with other tracks, so its statistics stay in the library.

## Moving your data to another computer

```sh
helikodu-cli export-user-data ~/helikodu-data.json
helikodu-cli import-user-data ~/helikodu-data.json            # show what would change
helikodu-cli import-user-data ~/helikodu-data.json --apply    # import it
```

The export holds your ratings, play counts, listening history, playlists and smart playlists, Favourites and saved searches, with each track's path and tags. On the other computer, scan your music first; the import then matches the tracks by path or by tags as above (`--map` works here too), and is undone with `undo-import`. Counts take the higher of the two values instead of being added, so importing the file again, or into the library it came from, changes nothing. The format is plain JSON, described in the source file `crates/library/src/interchange/user_data.rs`.
