# data

Files the desktop looks for by name, installed by Meson: the `.desktop` file, the AppStream metainfo and its screenshots, the D-Bus service and interface files, the application icons and the man pages.

Every file is named after the application ID, `io.github.sorrelfox.Helikodu`. A file the code cannot run without does not belong here; it is compiled into its crate instead.

| File | What it is |
|---|---|
| `io.github.sorrelfox.Helikodu.desktop` | The launcher entry: D-Bus activatable, the quick actions Play/Pause, Next and Previous (application actions, also reachable with `gapplication action`), and the audio and video types Helikodu plays the sound of |
| `io.github.sorrelfox.Helikodu.metainfo.xml` | AppStream metadata for software centres (CC0-1.0). No screenshots and no releases yet |
| `io.github.sorrelfox.Helikodu.service.in` | D-Bus activation of the application; `@bindir@` becomes the install location of `helikodu` |
| `icons/` | The application icons |

Check them with:

```sh
desktop-file-validate data/io.github.sorrelfox.Helikodu.desktop
appstreamcli validate --no-net data/io.github.sorrelfox.Helikodu.metainfo.xml
```

`cargo test -p helikodu-desktop --test data_files` runs both when they are installed.
