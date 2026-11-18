# Installing Helikodu

Helikodu is published on the project's [GitHub Releases](https://github.com/sorrelfox/helikodu/releases) page. Each release has three files:

| File | What it is |
|---|---|
| `helikodu-<version>-x86_64-linux.tar.xz` | The ready-built programs for 64-bit PCs (x86_64) |
| `helikodu-<version>.tar.xz` | The source code, with everything needed to build it offline |
| `SHA256SUMS` | Checksums of both files |

Check the download before you unpack it:

```sh
sha256sum --check --ignore-missing SHA256SUMS
```

## The ready-built programs

The programs were built on Debian 13 and need these libraries from your system, or newer versions with the same library names: glibc 2.41, GTK 4.18, FFmpeg 7.1 (`libavformat.so.61`, `libavcodec.so.61`, `libavutil.so.59`), PipeWire 1.2 and ALSA. SQLite is built in.

Unpack the archive and copy its contents to `/usr/local`:

```sh
tar -xJf helikodu-<version>-x86_64-linux.tar.xz
cd helikodu-<version>-x86_64-linux
sudo cp -r bin libexec share /usr/local/
sudo gtk4-update-icon-cache -q -t -f /usr/local/share/icons/hicolor
sudo update-desktop-database -q /usr/local/share/applications
```

The programs must stay in `/usr/local`: they look for their helper program in `/usr/local/libexec/helikodu`. `INSTALL.txt` in the archive lists the same steps and how to uninstall.

Helikodu then appears in your desktop's application list. You can also start it from a terminal with `helikodu`.

If your system already has a newer FFmpeg (for example FFmpeg 8) and no longer has `libavformat.so.61`, the ready-built programs do not start. Build from the source code instead.

## Building from the source code

You need Rust, Meson 1.0 or newer, gettext, and the development files of GTK 4.18, FFmpeg 7.1, PipeWire 1.2, ALSA and SQLite 3.43 (or newer versions of each). Then:

```sh
tar -xJf helikodu-<version>.tar.xz
cd helikodu-<version>
meson setup _build --prefix=/usr/local --buildtype=release -Doffline=true
meson compile -C _build
sudo meson install -C _build --no-rebuild
```

`-Doffline=true` builds from the Rust libraries included in the archive, so no network connection is needed. Add `-Dbundled_sqlite=true` to build SQLite in instead of using your system's copy.

This installs two programs: `helikodu`, the desktop application, and `helikodu-cli`, the command-line tool.
