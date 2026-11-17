# Bug severity

Every confirmed bug gets one of four severity levels. The level decides how quickly it is handled and whether it can block a release.

| Level | Meaning | Handling |
|---|---|---|
| **P0** | Loss or corruption of music files, tags, the library or playlists; a security vulnerability; loud output the user did not ask for | Stops a release; fixed at once |
| **P1** | A crash or hang in a core flow; a playback fault (gaps, clicks, the wrong output device, "bit-perfect" shown when it is not); a core flow that cannot be used with the keyboard or a screen reader | Fixed before the next release |
| **P2** | A feature works wrongly, but there is a workaround | Scheduled |
| **P3** | Cosmetic | Backlog |

The core flows are: browsing and searching the library, playing and queueing music, editing playlists and tags, choosing the output device, and resuming after a restart.

A release ships with no open P0 or P1 bugs.

## Labels

Issues carry one of the labels `P0`, `P1`, `P2` or `P3` once triaged. A new issue starts with `triage`. When it is unclear which of two levels applies, choose the higher one.
