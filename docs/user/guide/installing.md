# Installing Helikodu

Helikodu is published on the project's [GitHub Releases](https://github.com/sorrelfox/helikodu/releases) page. Each release has these files:

| File | What it is |
|---|---|
| `helikodu-<version>-x86_64-linux.tar.xz` | The ready-built programs for 64-bit PCs (x86_64) |
| `helikodu-<version>-aarch64-linux.tar.xz` | The ready-built programs for 64-bit ARM computers (aarch64) |
| `helikodu-<version>.tar.xz` | The source code, with everything needed to build it offline |
| `SHA256SUMS` | Checksums of the files |

`uname -m` in a terminal tells which of the two ready-built archives your computer needs. The steps below use the x86_64 one; for ARM, write `aarch64` in its place.

Check the download before you unpack it:

```sh
sha256sum --check --ignore-missing SHA256SUMS
```

## The ready-built programs

The programs were built on Debian 13 and need these libraries from your system, or newer versions with the same library names: glibc 2.41, GTK 4.18, FFmpeg 7.1 (`libavformat.so.61`, `libavcodec.so.61`, `libavutil.so.59`), PipeWire 1.2, ALSA, libopenmpt (`libopenmpt.so.0`), libgme (`libgme.so.0`) and libsidplayfp (`libsidplayfp.so.6`). SQLite is built in. Most desktops have all but the last three installed already; install the ones that are missing with your distribution's package manager.

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

You need Rust, a C and a C++ compiler, clang, Meson 1.0 or newer, gettext, and the development files of GTK 4.18, FFmpeg 7.1, PipeWire 1.2, ALSA, JACK, SQLite 3.43, libopenmpt, libgme and libsidplayfp (or newer versions of each). `README.md` in the source code lists them with their versions. Then:

```sh
tar -xJf helikodu-<version>.tar.xz
cd helikodu-<version>
meson setup _build --prefix=/usr/local --buildtype=release -Doffline=true
meson compile -C _build
sudo meson install -C _build --no-rebuild
```

`-Doffline=true` builds from the Rust libraries included in the archive, so no network connection is needed. Add `-Dbundled_sqlite=true` to build SQLite in instead of using your system's copy.

This installs two programs: `helikodu`, the desktop application, and `helikodu-cli`, the command-line tool.

## Building the Flatpak yourself

Helikodu is not on Flathub yet. Until it is, you can build the Flatpak from the source code yourself. It brings its own decoder libraries (vgmstream included) and uses the FFmpeg of the Flatpak runtime, so it does not depend on what your distribution has installed. You need `flatpak` and `flatpak-builder`, and the Flathub remote for the runtime:

```sh
flatpak install --user flathub org.gnome.Sdk//51 \
    org.freedesktop.Sdk.Extension.rust-stable//26.08 \
    org.freedesktop.Sdk.Extension.llvm22//26.08
cd helikodu-<version>
flatpak-builder --user --install --force-clean ~/.cache/helikodu-flatpak \
    packaging/flatpak/io.github.sorrelfox.Helikodu.yml
```

The build takes a while and downloads its sources first. Helikodu then appears in your application list; from a terminal, start it with `flatpak run io.github.sorrelfox.Helikodu`, and the command-line tool with `flatpak run --command=helikodu-cli io.github.sorrelfox.Helikodu`.

The Flatpak has no network access at all, and sees only your `Music` folder. Folders elsewhere are added through the folder dialog in **Settings ▸ Library**; Helikodu then rescans them now and then instead of noticing changes at once. To have such a folder watched like `Music`, grant it with `flatpak override --user --filesystem=/path/to/folder io.github.sorrelfox.Helikodu`. The Flatpak keeps its library and settings apart from a native installation, in `~/.var/app/io.github.sorrelfox.Helikodu/`. It comes without a SoundFont: to play MIDI files, choose one under **MIDI files** in the **Playback** settings.
