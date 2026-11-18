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

## helikodu-cli

`helikodu-cli` plays files in the terminal and works with the library without opening a window. It does not need the application to be running.

```sh
helikodu-cli play FILE...          # play files in the terminal
helikodu-cli list-outputs          # list the audio devices
helikodu-cli scan [FOLDER...]      # add folders to the library and scan them
helikodu-cli search TEXT...        # find tracks in the library
helikodu-cli info FILE|TRACK-ID    # show a file's or track's tags and properties
helikodu-cli stats                 # library totals and most played tracks
helikodu-cli loudness --library    # measure the loudness of the tracks not yet measured
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

`scan`, `search`, `info`, `stats` and `loudness` use the same library as the application, so you can scan from the command line and browse the result in the window. Most commands take `--json` for output that scripts can read.

The manual page `man helikodu-cli` describes every command, option and exit code.
