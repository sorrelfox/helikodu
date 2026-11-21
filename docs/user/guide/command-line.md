# The command line

## Starting the application

```sh
helikodu [OPTIONS] [FILE...]
```

Files given on the command line are played. If Helikodu is already running, they go to the running window. The options override the appearance settings for this start only:

| Option | Meaning |
|---|---|
| `--scheme system\|light\|dark` | The colour scheme |
| `--accent system\|amber\|blue\|green\|violet` | The accent colour |
| `--style integrated\|classic` | The window style |
| `--demo` | Shows invented sample albums and plays nothing, to look around without a library |
| `--help` | Shows a short help |
| `--version` | Shows the version |

## helikodu-cli

`helikodu-cli` plays files in the terminal and works with the library without opening a window. The commands below do not need the application to be running; those that control it are under [Controlling the running application](#controlling-the-running-application).

```sh
helikodu-cli play FILE...          # play files in the terminal
helikodu-cli list-outputs          # list the audio devices
helikodu-cli scan [FOLDER...]      # add folders to the library and scan them
helikodu-cli search TEXT...        # find tracks in the library
helikodu-cli info FILE|TRACK-ID    # show a file's or track's tags and properties
helikodu-cli stats                 # library totals and most played tracks
helikodu-cli loudness --library    # measure the loudness of the tracks not yet measured
helikodu-cli analyze               # analyse the tracks not yet analysed, as the app does
helikodu-cli similar FILE|TRACK-ID # the tracks that sound most like a track (--album, --artist)
helikodu-cli duplicates            # list the copies of files in the library (lists only)
helikodu-cli import PLAYER         # import plays, ratings and playlists from another player
helikodu-cli undo-import           # undo the newest import
helikodu-cli export-user-data FILE # write ratings, plays, history, playlists, labels and fields to a file
helikodu-cli import-user-data FILE # read such a file into the library
helikodu-cli carry-stats           # carry statistics from replaced files to their replacements
helikodu-cli write-stats           # write changed play counts and ratings into the files
helikodu-cli managed-folder ...    # the managed folder: show, set, import, consolidate
helikodu-cli labels list|add|...   # labels of tracks, albums and artists
helikodu-cli fields list|define|... # computed and custom fields
```

Some examples:

```sh
# Play an album to a USB DAC, exclusively, with album gain
helikodu-cli list-outputs --backend alsa
helikodu-cli play ~/Music/Album/*.flac --backend alsa \
    --device 'hw:CARD=D10s,DEV=0' --mode exclusive --replaygain album

# Scan a folder, leaving out partial downloads
helikodu-cli scan ~/Music --exclude '*.part'

# The ten best matches as JSON
helikodu-cli search miles davis --limit 10 --json
```

While `play` runs in a terminal, Space pauses, `n` and `p` go to the next and previous file, the arrow keys seek by 5 seconds, `+` and `-` change the volume, `o` moves playback to the next output device, and `q` quits.

`import`, `import-user-data` and `carry-stats` only show what they would change unless you add `--apply`; [Moving from another player](moving-from-another-player.md) describes them and `export-user-data`, `undo-import` and `write-stats`.

`scan`, `search`, `info`, `stats`, `loudness` and `duplicates` use the same library as the application, so you can scan from the command line and browse the result in the window. Most commands take `--json` for output that scripts can read.
`scan`, `search`, `info`, `stats`, `loudness`, `duplicates`, `labels` and `fields` use the same library as the application, so you can scan from the command line and browse the result in the window. Most commands take `--json` for output that scripts can read. `labels` and `fields` are described in [Labels and your own fields](labels-and-fields.md).

## The managed folder

`helikodu-cli managed-folder` sets the [managed folder](file-tools.md#the-managed-folder) and imports files into it. It reads and writes the same settings file as the application; change the setting while Helikodu is closed, or the application may write its own copy over it.

```sh
helikodu-cli managed-folder show [--json]          # the folder, pattern, mode, and a run cut short
helikodu-cli managed-folder set FOLDER [--pattern PATTERN] [--mode copy|move] \
    [--characters portable|as-is|ascii]            # turn it on, or change it
helikodu-cli managed-folder off                    # turn it off; the files stay
helikodu-cli managed-folder import PATH... [--move|--copy] [--dry-run] [--json]
helikodu-cli managed-folder consolidate [--move|--copy] [--dry-run] [--json]
helikodu-cli managed-folder resume [--json]        # continue a run that was cut short
helikodu-cli managed-folder roll-back [--json]     # undo a run that was cut short
```

`--move` and `--copy` override the mode of the setting for one run. `--dry-run` prints the plan (each file and where it goes, numbered names, skipped files, the space needed and free) and changes nothing. `--settings PATH` and `--database PATH` choose another settings file and library. The command ends with exit code 1 when a file could not be placed or a run was not finished.

```sh
# Turn the managed folder on inside ~/Music, moving files on import
helikodu-cli managed-folder set ~/Music/Library --mode move

# See what importing a memory stick would do, then copy it in
helikodu-cli managed-folder import /run/media/me/STICK/Music --dry-run
helikodu-cli managed-folder import /run/media/me/STICK/Music --copy
```

## Controlling the running application

These `helikodu-cli` commands talk to the running Helikodu window over D-Bus. They never start it: when it is not running they say so and end with exit code 4.

```sh
helikodu-cli status [--json]       # what plays, the position, volume, modes and output
helikodu-cli queue [--json]        # the queue in playing order
helikodu-cli playlists [--json]    # the playlists with their numbers
helikodu-cli play-pause | pause | resume | stop | next | previous
helikodu-cli seek 1:23             # a position; +10 and -5 seek by seconds
helikodu-cli volume [50|+5|-5]     # print or set the volume (0 to 100)
helikodu-cli shuffle off|tracks|albums|folders|groupings|within-album
helikodu-cli repeat off|one|album|all
helikodu-cli enqueue [--next] FILE...           # add files at the end, or after the playing track
helikodu-cli enqueue [--next] --query QUERY     # add the library tracks a query finds
helikodu-cli play-query [--shuffle] QUERY...    # replace the queue with them and play
helikodu-cli rate 4 | none [--track ID]         # rate the playing track (or a library track)
helikodu-cli export-playlist PLAYLIST FILE      # write a playlist as M3U8, M3U, PLS or XSPF
```

A query is written as in the search field, for example `helikodu-cli play-query --shuffle 'genre:jazz rating>=4'`. Playback asked for this way asks for the same confirmation as in the window when the output plays at full volume. Other programs can use the same interface; its description is in the developer documentation (`docs/dev/dbus-api.md`).

The manual page `man helikodu-cli` describes every command, option and exit code.
