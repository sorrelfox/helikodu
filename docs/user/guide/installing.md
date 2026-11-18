# Installing Helikodu

Helikodu is published on the project's [GitHub Releases](https://github.com/sorrelfox/helikodu/releases) page. Each release has these files (`<version>` is the release's version, such as `1.0.0`):

| File | What it is |
|---|---|
| `helikodu-<version>-x86_64.flatpak`, `helikodu-<version>-aarch64.flatpak` | The [Flatpak](#the-flatpak), for any distribution with Flatpak |
| `Helikodu-<version>-x86_64.AppImage`, `Helikodu-<version>-aarch64.AppImage` | The [AppImage](#the-appimage): one file that runs without installing, on most distributions |
| `helikodu-<version>-1.fc45.x86_64.rpm`, `helikodu-<version>-1.fc45.aarch64.rpm` | The package for [Fedora 45](#fedora) |
| `helikodu_<version>-1.debian13_amd64.deb`, `helikodu_<version>-1.debian13_arm64.deb` | The package for [Debian 13](#debian-and-ubuntu) |
| `helikodu_<version>-1.ubuntu26.04_amd64.deb`, `helikodu_<version>-1.ubuntu26.04_arm64.deb` | The package for [Ubuntu 26.04 LTS](#debian-and-ubuntu) |
| `helikodu-<version>-1-x86_64.pkg.tar.zst` | The package for [Arch Linux](#arch-linux) (x86_64 only) |
| `helikodu-<version>-x86_64-linux.tar.xz`, `helikodu-<version>-aarch64-linux.tar.xz` | The [ready-built programs](#the-ready-built-programs), to copy to `/usr/local` |
| `helikodu-<version>-x86_64-linux.tar.gz`, `helikodu-<version>-aarch64-linux.tar.gz` | The same ready-built programs, packed with gzip instead of xz |
| `helikodu-<version>.tar.xz` | The [source code](#building-from-the-source-code), with everything needed to build it offline |
| `ffmpeg-<version>.tar.xz`, `vgmstream-<version>.tar.gz` | The source code of the FFmpeg and vgmstream inside the AppImage |
| `SHA256SUMS` | Checksums of the files |

Most files exist for two kinds of computer: `x86_64` (`amd64` in the Debian and Ubuntu names) for 64-bit PCs, and `aarch64` (`arm64`) for 64-bit ARM computers. `uname -m` in a terminal tells which one yours is. The steps below use the x86_64 files; on ARM, take the other one.

A pre-release, such as `1.0.0-rc1`, has a dot in place of the hyphen in the names of the Fedora, Debian and Ubuntu packages (`helikodu-1.0.0.rc1-1.fc45.x86_64.rpm`), and none in the name of the Arch package (`helikodu-1.0.0rc1-1-x86_64.pkg.tar.zst`).

## Which file to use

- **On Fedora 45, Debian 13, Ubuntu 26.04 LTS or Arch Linux:** the package for your distribution. It is installed like any other package, uses your system's libraries, and is removed with the package manager.
- **On any other distribution:** the Flatpak, if your system has Flatpak (most desktop distributions do), or else the AppImage. Both bring their own libraries, so they work whatever versions of GTK and FFmpeg your system has. The Flatpak runs in a sandbox and shares its runtime with other Flatpak applications; the AppImage is a single file that you only download and start.
- **If none of these suits you:** the ready-built programs (`.tar.xz` or `.tar.gz`), which need recent libraries from your system, or the source code.

Only the Flatpak and the AppImage play game audio streams that need vgmstream; the distributions do not provide that library. The packages are not in a repository, so your system's updates do not update them: to update, install the new release's file the same way. Helikodu never looks for updates itself.

Check the download before you install or unpack it:

```sh
sha256sum --check --ignore-missing SHA256SUMS
```

Whichever way you install Helikodu, your library and settings are kept in your home folder (see [Where Helikodu keeps its files](files.md)) and stay when you remove it; the Flatpak keeps its own (see below).

## The Flatpak

The Flatpak runs on any distribution with Flatpak installed. It brings its own decoder libraries (vgmstream included) and uses the FFmpeg of the GNOME runtime. Install it for your user:

```sh
flatpak install --user --bundle helikodu-<version>-x86_64.flatpak
```

The first time, Flatpak also installs the GNOME 51 runtime from Flathub (a download of several hundred megabytes) and, if your system does not have it yet, asks to add the Flathub remote. Helikodu then appears in your application list; from a terminal, start it with `flatpak run io.github.sorrelfox.Helikodu`, and the command-line tool with `flatpak run --command=helikodu-cli io.github.sorrelfox.Helikodu`.

To update, install the next release's file the same way. To remove it, run `flatpak uninstall --user io.github.sorrelfox.Helikodu` (add `--delete-data` to remove its library and settings too).

The Flatpak has no network access at all, and sees only your `Music` folder. Folders elsewhere are added through the folder dialog in **Settings ▸ Library**; Helikodu then rescans them now and then instead of noticing changes at once. To have such a folder watched like `Music`, grant it with `flatpak override --user --filesystem=/path/to/folder io.github.sorrelfox.Helikodu`. The Flatpak keeps its library and settings apart from other installations, in `~/.var/app/io.github.sorrelfox.Helikodu/`. It comes without a SoundFont: to play MIDI files, choose one under **MIDI files** in the **Playback** settings.

## The AppImage

The AppImage is a single file that brings its own copies of GTK, FFmpeg and the decoders for tracker modules, game music, SID tunes and game audio streams. It runs on Debian 13, Ubuntu 24.10, Fedora 41 and newer systems (it needs glibc 2.39 and PipeWire 1.2 or newer) and uses the system's sound libraries (PipeWire, PulseAudio, ALSA or JACK), fonts and graphics drivers.

Make the file executable and start it:

```sh
chmod +x Helikodu-<version>-x86_64.AppImage
./Helikodu-<version>-x86_64.AppImage
```

Some file managers start it with a double-click once it is executable (in its properties, "Allow executing file as program"). If it does not start and says FUSE is missing, run it as `./Helikodu-<version>-x86_64.AppImage --appimage-extract-and-run` or install your distribution's `fuse3` package.

The command-line tool is in the same file:

```sh
./Helikodu-<version>-x86_64.AppImage --cli --help
```

or, through a link named `helikodu-cli` (in a folder on your `PATH`):

```sh
ln -s "$PWD/Helikodu-<version>-x86_64.AppImage" ~/.local/bin/helikodu-cli
helikodu-cli status
```

The AppImage does not add itself to the application list: to update, download the new version and replace the file. Tools such as Gear Lever or AppImageLauncher can add it to the application list. Your library, playlists and settings are kept in the same folders as with a package or the ready-built programs, so moving between them keeps them.

Helikodu follows the desktop's GTK theme, icon theme and dark style as far as the theme supports the GTK version in the AppImage. File choosers are the desktop's own where a desktop portal runs.

## Fedora

On Fedora 45, install the package with `dnf`, which also installs the libraries it needs:

```sh
sudo dnf install ./helikodu-<version>-1.fc45.x86_64.rpm
```

Helikodu then appears in your application list, and `helikodu-cli` is on the path. To remove it, run `sudo dnf remove helikodu`.

Fedora's own FFmpeg (ffmpeg-free) leaves out a few formats: E-AC-3 (Dolby Digital Plus), WMA Pro, WMA Lossless, MPEG-4 ALS, compressed DSD (DST) in DFF files, aptX and ATRAC9 do not play. MP3, AAC, Opus, Vorbis, FLAC, ALAC, AC-3, DTS, WavPack, Monkey's Audio, Musepack and WMA play. For the rest, enable the free repository of [RPM Fusion](https://rpmfusion.org/Configuration) and switch to its FFmpeg; Helikodu uses it without being reinstalled:

```sh
sudo dnf swap ffmpeg-free ffmpeg --allowerasing
```

MIDI files play with the SoundFont that the package installs (`fluid-soundfont-gm`) unless you choose another one. For the JACK output, install `pipewire-jack-audio-connection-kit`.

The package is built for Fedora 45 only: on another Fedora version, use the Flatpak or the AppImage.

## Debian and Ubuntu

On Debian 13 and Ubuntu 26.04 LTS, install the package made for your system: the one with `debian13` or `ubuntu26.04` in its name, ending in `_amd64.deb` on a 64-bit PC or `_arm64.deb` on 64-bit ARM (`dpkg --print-architecture` tells which). The two are not interchangeable, because the distributions have different versions of FFmpeg and GTK. Install it with `apt`, which also installs the libraries it needs:

```sh
sudo apt install ./helikodu_<version>-1.debian13_amd64.deb
```

(The `./` matters: it tells `apt` to install the file rather than look for a package of that name.) Helikodu then appears in your application list; from a terminal, start it with `helikodu`, and the command-line tool with `helikodu-cli`.

To play MIDI files, also install a SoundFont, for example `sudo apt install fluid-soundfont-gm`. To remove Helikodu, run `sudo apt remove helikodu`. On other versions of Debian or Ubuntu, use the Flatpak or the AppImage.

## Arch Linux

On Arch Linux and distributions based on it, install the package with `pacman`, which also installs the libraries it needs from the Arch repositories:

```sh
sudo pacman -U helikodu-<version>-1-x86_64.pkg.tar.zst
```

The package is built only for x86_64. To remove Helikodu again, run `sudo pacman -R helikodu`.

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

For the `.tar.gz` archive, unpack it with `tar -xzf` instead. The programs must stay in `/usr/local`: they look for their helper program in `/usr/local/libexec/helikodu`. `INSTALL.txt` in the archive lists the same steps and how to uninstall.

Helikodu then appears in your desktop's application list. You can also start it from a terminal with `helikodu`.

If your system already has a newer FFmpeg (for example FFmpeg 8) and no longer has `libavformat.so.61`, the ready-built programs do not start. Use the Flatpak or the AppImage, or build from the source code instead.

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

Helikodu is not on Flathub yet. Besides the ready-built Flatpak above, you can build the Flatpak from the source code yourself. You need `flatpak` and `flatpak-builder`, and the Flathub remote for the runtime:

```sh
flatpak install --user flathub org.gnome.Sdk//51 \
    org.freedesktop.Sdk.Extension.rust-stable//26.08 \
    org.freedesktop.Sdk.Extension.llvm22//26.08
cd helikodu-<version>
flatpak-builder --user --install --force-clean ~/.cache/helikodu-flatpak \
    packaging/flatpak/io.github.sorrelfox.Helikodu.yml
```

The build takes a while and downloads its sources first. It is then installed and used like the [ready-built Flatpak](#the-flatpak).
