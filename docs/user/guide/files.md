# Where Helikodu keeps its files

Helikodu follows the XDG base directory rules. The folders below are the defaults; when `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_DATA_HOME` or `XDG_CACHE_HOME` is set, Helikodu uses that folder instead of `~/.config`, `~/.local/state`, `~/.local/share` or `~/.cache`.

| Path | What it holds |
|---|---|
| `~/.config/helikodu/settings.toml` | Your settings |
| `~/.local/share/helikodu/library.db` | The library: tracks, albums, ratings, play counts and playlists |
| `~/.local/share/helikodu/library.db.backup-before-copies` | A copy of the library made once, before the play counts and ratings of copies of the same file were first combined (see [Copies of the same file](settings.md#copies-of-the-same-file)). You can delete it once you are happy with the result |
| (in `library.db`) | The journal of the files the organizer moved, so that a run can be undone; kept for the undo limit of the settings |
| `~/.local/state/helikodu/session.json` | The queue, position, volume, modes and output device of the last session |
| `~/.local/state/helikodu/playing.json` | A marker that exists only while something plays; see [Troubleshooting](troubleshooting.md#helikodu-skips-a-track-it-used-to-play) |
| `~/.local/state/helikodu/ui-state.toml` | The window size and the widths of its panes |
| `~/.cache/helikodu/thumbnails/` | Cover thumbnails, up to 2 GiB by default. Safe to delete; they are made again when needed |

Helikodu never copies your music files, and changes them only when you save tags in the [tag editor](tags.md), or move, rename or trash them with the [file tools](file-tools.md). Ratings, play counts and playlists stay in the library database, not in the files.

## Backing up

To keep your ratings, play counts and playlists, back up `~/.local/share/helikodu/` and `~/.config/helikodu/`. Close Helikodu first, so that the database is complete on disk.

## Removing everything

To remove all traces of Helikodu from your account (your music is not touched):

```sh
rm -r ~/.config/helikodu ~/.local/share/helikodu ~/.local/state/helikodu ~/.cache/helikodu
```
