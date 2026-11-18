# helikodu

Builds `helikodu`, the desktop application. It only wires the parts together: it starts the player service, opens the library database and runs the GTK front end over them. It holds no domain logic.

- **May depend on:** `model`, `service`, `desktop`, `ui-gtk`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

It starts the service (settings, library, player, session) with the desktop's audio device reservation (`helikodu-desktop`) for exclusive ALSA output, and runs the GTK front end, which also serves MPRIS, inhibits suspend while playing and sends track notifications when the appearance settings ask for them. As a `GApplication` it is a single instance: files on the command line or from "Open With" play in the running window, and `--gapplication-service` is how D-Bus activation starts it.

```sh
helikodu                       # settings, library and session from the XDG folders
helikodu song.flac             # play a file (in the running instance, if there is one)
helikodu --scheme light --accent blue
helikodu --demo                # invented sample content, nothing plays
```

`helikodu --help` lists every option. The output device and everything else come from `$XDG_CONFIG_HOME/helikodu/settings.toml`; the library is filled with `helikodu-cli scan <folder>` for now.

## Tests

```sh
cargo test -p helikodu
```
