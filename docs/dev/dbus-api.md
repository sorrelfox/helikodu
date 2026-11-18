# The D-Bus API

The running application serves an interface of its own beside MPRIS, for what MPRIS cannot express: adding to the queue after the playing track, playing the result of a query, rating, exporting a playlist, and the status and the queue as JSON. `helikodu-cli` uses it for its commands for the running application; any other program in the session can use it too.

| | |
|---|---|
| Bus | The session bus |
| Bus name | `io.github.sorrelfox.Helikodu` (the application ID, owned by the running application) |
| Object | `/io/github/sorrelfox/Helikodu` |
| Interface | `io.github.sorrelfox.Helikodu.Control1` |
| Introspection data | [`crates/desktop/src/app_api/interface.xml`](../../crates/desktop/src/app_api/interface.xml) |

The bus name is the one the application owns as a single instance, so the interface is there only while the application runs. A call to it while the application is not running would start the application through D-Bus activation; `helikodu-cli` asks `org.freedesktop.DBus.NameHasOwner` first and does not start it.

## Versions

The interface is versioned by its name. Version 1 is `Control1`; its `Version` property (`u`) says 1. Within a version, methods and JSON fields are only ever added, never removed or changed in meaning, so a client should ignore fields it does not know. A change that would break a client gets a new interface, `Control2`, served beside the old one for as long as practical.

## Methods

| Method | Arguments | Returns | What it does |
|---|---|---|---|
| `GetStatus` | | `s` | The status as a JSON object (below) |
| `GetQueue` | | `s` | The queue as a JSON object (below) |
| `ListPlaylists` | | `s` | The playlists as a JSON array: `id`, `name`, `kind` (`static` or `smart`), `tracks`, `length_ms` |
| `PlayPause`, `Play`, `Pause`, `Stop`, `Next`, `Previous` | | | The transport. `Pause` does nothing unless playing; `Previous` goes to the start of the track after its first seconds |
| `Seek` | `x` offset_ms | | Moves the position by the offset, back when negative |
| `SetPosition` | `t` position_ms | | Moves to the position in the playing track |
| `SetVolume` | `d` volume | | The volume control, 0.0 to 1.0 (the position of the slider) |
| `SetShuffle` | `s` mode | | `off`, `tracks`, `albums`, `folders`, `groupings` or `within-album` |
| `SetRepeat` | `s` mode | | `off`, `one`, `album` or `all` |
| `Enqueue` | `as` files, `b` next | `u` added | Adds files (absolute paths or `file://` URIs) after the playing track when `next`, at the end otherwise |
| `EnqueueQuery` | `s` query, `b` next | `u` added | Adds the library tracks the query finds (at most 10,000), as `Enqueue` |
| `PlayQuery` | `s` query, `b` shuffle | `u` tracks | Replaces the queue with the tracks the query finds and plays them; nothing changes when it finds none |
| `Rate` | `t` track, `i` stars | | Rates a library track (0 for the playing track) with 0 to 5 stars, or removes its rating with −1 |
| `ExportPlaylist` | `s` playlist, `s` path | `u` entries | Writes a playlist, given by its name or its number, to an absolute path; the extension picks the format (`m3u8`, `m3u`, `pls`, `xspf`) |

A query is text in the query language of the search field (`genre:jazz rating>=4 sort:added desc`); text without fields searches for its words.

Requests that start playback (`Play`, `PlayPause`, `PlayQuery`) pass the same confirmation as the window's own when the output plays at a fixed full volume on a device that was not confirmed yet: the window asks, and the call has returned by then.

## Errors

| Error | When |
|---|---|
| `org.freedesktop.DBus.Error.InvalidArgs` | An argument is out of range or not understood: an unknown mode, a volume outside 0 to 1, a rating outside −1 to 5, a relative path, an empty query |
| `io.github.sorrelfox.Helikodu.Error.Failed` | The call could not be done: no library track plays (`Rate` with 0), there is no such playlist, the file cannot be written; the message says why |

## The status

`GetStatus` returns a JSON object:

```json
{
  "api_version": 1,
  "state": "playing",
  "track": {
    "id": 18234, "title": "Shoreline, 6 a.m.", "artist": "Mari Kask", "album": "Tidewater Rooms",
    "year": 2021, "track_number": 3, "disc_number": null, "length_ms": 295000,
    "rating": 4, "plays": 12, "path": "/home/me/Music/Mari Kask/Tidewater Rooms/03.flac",
    "codec": "FLAC", "sample_rate": 96000, "bits": 24
  },
  "position_ms": 102000,
  "length_ms": 295000,
  "volume": 0.8,
  "shuffle": "off",
  "repeat": "all",
  "stop_after": false,
  "queue_length": 12,
  "queue_position": 2,
  "output": { "device": "Topping D10s", "signal": "bit-perfect", "sample_rate": 96000, "bits": 24 }
}
```

- `state` is `stopped`, `loading`, `playing` or `paused`. `track` and `position_ms` are `null` when nothing is current.
- `track.id` is the library track (as `helikodu-cli search` and `info` show it), `null` for a file that is not in the library. Fields the player does not know are `null`.
- `position_ms` is the position last reported to the window; while the window is hidden it is reported less often.
- `queue_position` counts from 0.
- `output.signal` is `idle`, `unreported`, `bit-perfect`, `processed` or `not-connected`.

`GetQueue` returns `{"context": "Tidewater Rooms", "current": 2, "tracks": [ … ]}`: what the queue continues from, the index of the current track, and the tracks in playing order, each as `track` above.

## Examples

```sh
gdbus call --session --dest io.github.sorrelfox.Helikodu \
    --object-path /io/github/sorrelfox/Helikodu \
    --method io.github.sorrelfox.Helikodu.Control1.GetStatus

busctl --user call io.github.sorrelfox.Helikodu /io/github/sorrelfox/Helikodu \
    io.github.sorrelfox.Helikodu.Control1 PlayQuery sb 'genre:ambient' true
```

## Safety

The session bus is trusted as the user's own (see the [threat model](threat-model.md)). The interface offers no operation that moves, renames or deletes files; `ExportPlaylist` writes only to the path the caller names.

## Where it lives

`crates/desktop/src/app_api.rs` holds the names and the introspection data; the window registers the interface on the application's own bus connection (`crates/ui-gtk/src/remote_api.rs`) and turns each call into a request of its backend, so the window and the API share one path to the player. `apps/cli/src/remote/` is the client.
