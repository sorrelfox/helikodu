# The AppImage

Each release has an AppImage for x86_64 and for aarch64: `Helikodu-<version>-<arch>.AppImage`, one file that runs on current distributions without installing (see [Limits](#limits) for what the system has to provide). It carries its own GTK 4, a minimal FFmpeg, libopenmpt, libgme, libsidplayfp and vgmstream, so it does not depend on the FFmpeg version of the system, unlike the ready-built tarball. The x86_64 image of 0.9.0 is 46 MB (112 MB unpacked). The recipe is in [`build-aux/appimage/`](../../build-aux/appimage/).

| File | What it does |
|---|---|
| `build.sh` | Downloads the pinned sources and tools into a cache folder, checks their SHA-256, builds the container and runs the build in it |
| `pins.sh` | The pinned versions, download addresses and SHA-256 checksums |
| `Containerfile` | The build environment: Debian 13 with the build tools and the development files (no system FFmpeg) |
| `build-in-container.sh` | Builds FFmpeg, vgmstream and Helikodu, assembles the AppDir, packs the image and checks it |
| `AppRun` | Starts the desktop application or, with `--cli`, the command-line tool |

## Building

```sh
build-aux/appimage/build.sh                       # from this checkout, into target/appimage/
build-aux/appimage/build.sh --source target/dist/helikodu-1.0.0.tar.xz --offline
```

It needs Podman or Docker (`HELIKODU_CONTAINER_ENGINE` picks one) and builds for the architecture of the machine; there is no cross-building. The container runs with at most 12 GB of memory and `CARGO_BUILD_JOBS` jobs (4 unless set), like the other release builds. Downloads, and for builds from a checkout the Rust toolchain and the crates, are kept in `target/appimage-cache/` (`--cache`). Nothing downloaded is committed.

The Release workflow runs it once per architecture (the `appimage` job), on the same native runners as the tarballs, from the source tarball with `--offline`, and publishes the images, together with the FFmpeg and vgmstream source archives (see [Licences](#licences)), with the other files and in `SHA256SUMS`.

### Pinned sources and tools

| What | Version | Why |
|---|---|---|
| FFmpeg | 9.0.2 | Built minimal, see below |
| vgmstream | r2168 | Game audio streams; few distributions package it |
| linuxdeploy | 1-alpha-20251107-1 | Copies the shared libraries the programs need and sets their run paths |
| appimagetool | 1.9.1 | Packs the AppDir into the image |
| AppImage runtime (type 2) | 20251108 | The program at the start of the image; statically linked, so the system needs no libfuse 2. Given to appimagetool so that it downloads nothing itself |

To update one, change its version and checksum in `pins.sh` (`sha256sum` of the downloaded file), build, and run the checks below.

## What the build does

1. **Base.** Debian 13 (`debian:13`), the oldest supported base, as for the tarballs: glibc 2.41 and GTK 4.18.
2. **FFmpeg**, LGPL configuration (no `--enable-gpl`, no `--enable-nonfree`; the build fails if configure reports another licence), shared libraries, `--disable-everything` and then only:
   - the demuxers of the bridge's format whitelist (`FORMAT_WHITELIST` in `crates/ffmpeg-bridge/src/io/lockdown.rs`; the build fails when the two lists differ, so a change to the whitelist has to be made in `build-in-container.sh` too);
   - every audio decoder (the bridge allows every audio decoder of the build), taken from the audio, PCM, DPCM and ADPCM sections of `libavcodec/allcodecs.c`, and no video, image or subtitle decoder. Decoders FFmpeg marks as needing a GPL build stay out;
   - the audio parsers the demuxers use;
   - the encoders and muxers the converter offers (`EncodeFormat` in `crates/ffmpeg-bridge/src/encode.rs`): libopus and FFmpeg's Opus, libvorbis, LAME, FFmpeg's AAC, FLAC, ALAC, 16- and 24-bit PCM; muxers `opus`, `ogg`, `mp3`, `ipod`, `flac`, `wav`;
   - `--disable-network`, no protocols at all (the bridge reads and writes through its own I/O), no devices, filters or scaling, no hardware acceleration, and `--disable-autodetect`, so nothing is picked up from the build system by chance. zlib is on (compressed MP4 and Matroska headers); libswresample is built for vgmstream.

   The exact configure line is written into the image (`usr/share/licenses/README.txt`).
3. **vgmstream** as a shared library (`libvgmstream.so`), with the FFmpeg above, mpg123, Vorbis and Speex from Debian and its built-in G.722.1, G.719 and ATRAC9 decoders. FSB CELT is off because vgmstream's build would download it; the build fails if vgmstream tries to download anything.
4. **Helikodu** with Meson (`--prefix=/usr`, `-Dbundled_sqlite=true`, `-Dvgmstream=true`) into the AppDir: the three programs, the desktop entry and metainfo with their translations, the icons, all compiled translations (the build checks every language of `po/LINGUAS`) and the user guide. The D-Bus service file and the manual pages are removed; `DBusActivatable` and `TryExec` are removed from the desktop entry, since nothing outside the image can start it over D-Bus.
5. **GTK's extra files:** GTK's GSettings schemas (compiled), the SVG loader of gdk-pixbuf (GTK's symbolic icons are SVG; PNG and JPEG are built into gdk-pixbuf), the dconf GSettings backend, and Adwaita's symbolic and scalable icons with hicolor's index as a fallback icon theme.
6. **linuxdeploy** copies every shared library the programs, the loader and the modules need into `usr/lib` and sets run paths (`$ORIGIN`), so no `LD_LIBRARY_PATH` is set. HarfBuzz and libgpg-error are added by hand (see the table below).
7. **Licences** (below), then **appimagetool** packs the image with the pinned runtime. No update information is embedded.
8. **Checks:** every `NEEDED` library of every ELF file is in the image or on the list the system is expected to provide (the build fails otherwise); the newest glibc symbol version used is printed; the image starts `--help`, `--cli --version` and `--cli --help`; and the image has no update information.

## Libraries: bundled or from the system

| Library | Where from | Why |
|---|---|---|
| glibc, libstdc++, libgcc_s | System | Never bundled (linuxdeploy's exclude list). glibc 2.41 is the floor; any system with it has a new enough libstdc++ |
| PipeWire (`libpipewire-0.3`) | System | The client library has to match the PipeWire server that runs, and loads the system's SPA plugins and modules from compiled-in paths. Every current desktop distribution installs it |
| ALSA (`libasound`) | System | Loads the system's ALSA plugins (the `pipewire` and `pulse` devices) and reads its configuration |
| JACK (`libjack`) | System, at run time | Never linked: the JACK output loads it when it is chosen (PipeWire's or JACK2's) |
| PulseAudio | — | The PulseAudio output speaks the protocol itself; no library |
| OpenGL, EGL, libdrm, libgbm, X11, xcb, Wayland client | System | Must match the graphics drivers and the display server (linuxdeploy's exclude list) |
| fontconfig, FreeType, FriBidi, expat, zlib | System | linuxdeploy's exclude list: present everywhere, and fontconfig reads the system's configuration |
| HarfBuzz, libgpg-error | Image (Debian 13) | On linuxdeploy's exclude list, but bundled anyway: GTK's `libharfbuzz-subset` needs the HarfBuzz it was built with, and libgcrypt (used by libsidplayfp) a libgpg-error at least as new as its own |
| GTK 4.18, GLib, Pango, Cairo, gdk-pixbuf, librsvg, graphene, libepoxy, libvulkan, … | Image (Debian 13) | GTK and its stack, no libadwaita |
| FFmpeg 9.0 (`libavformat`, `libavcodec`, `libavutil`, `libswresample`) | Image (built) | A known, minimal set of demuxers and decoders, independent of the system's FFmpeg version |
| libopus, libvorbis, libogg, LAME | Image (Debian 13) | The converter's encoders |
| libopenmpt, libgme, libsidplayfp, vgmstream (with mpg123, Speex) | Image (Debian 13; vgmstream built) | The long-tail decoders |
| SQLite | Built into the programs | `-Dbundled_sqlite=true` |

## Running inside the image

`AppRun` starts `usr/bin/helikodu`, or `usr/bin/helikodu-cli` when the first argument is `--cli` or when the image was started through a link whose name begins with `helikodu-cli` (the runtime passes that name as `ARGV0`). The programs find their own files beside themselves: the scan worker in `../libexec/helikodu/` (`find_worker_program`), the translations in `../share/locale/` and the user guide in `../share/helikodu/guide/`, both of which are tried before the paths compiled in. Restarting after a backup was chosen to be put back starts the image file (`$APPIMAGE`) again, not the program in the image's mount, which goes away when the program ends. The About window names the installation "AppImage".

`AppRun` sets only what the bundled GTK and GLib need, and leaves `GTK_THEME` and the desktop's other settings alone:

| Variable | Value | Why |
|---|---|---|
| `XDG_DATA_DIRS` | The system's folders, then the image's `usr/share` | The desktop's GTK and icon themes and its settings schemas come first; the image's GTK schemas and Adwaita icons are the fallback |
| `GDK_PIXBUF_MODULE_FILE` | The image's loader cache | The SVG loader of the image, not the system's loaders built for its own gdk-pixbuf |
| `GIO_MODULE_DIR` | The image's `usr/lib/gio/modules` | Only the image's dconf module; the system's GIO modules may need a newer GLib than the image's |
| `GTK_EXE_PREFIX` | The image's `usr` | GTK looks for input method modules and print backends in the image (there are none) instead of in the system's GTK folder |
| `GDK_DEBUG` | `portals` added | File choosers and opened links (the user guide in the browser) go through the desktop portal when one runs, so the dialogs are the desktop's own and the browser does not inherit the variables above |

## Theming

The GTK theme is the desktop's: GTK finds the themes in the system's `share/themes` and `~/.themes` and follows `GTK_THEME`, the dark style (through the settings portal) and the accent colour as any GTK 4 application does. A theme made for a newer GTK than 4.18 may show small differences, and GTK 3 themes do not apply to GTK 4 anywhere. Helikodu's own style sheet is part of the program. The icon theme is the desktop's, with Adwaita from the image where the desktop's theme lacks an icon.

## Network and updates

The image adds no network access: FFmpeg has no protocols and no network code, and the runtime does not look for updates. No update information (`gh-releases-zsync`) is embedded and no `.zsync` file is published, so AppImageUpdate and similar tools cannot update it; a new version is downloaded by hand from the Releases page. The build checks that `--appimage-updateinformation` is empty.

## Licences

Helikodu is GPL-3.0-or-later. The image carries in `usr/share/licenses/`:

- `helikodu/LICENSE`;
- `ffmpeg/`: FFmpeg's `COPYING.LGPLv2.1` and `LICENSE.md` (the build is LGPL 2.1 or later);
- `vgmstream/COPYING`;
- `debian/<package>/copyright` for every Debian package a bundled library comes from;
- `README.txt`: the FFmpeg version, download address and configure line, the vgmstream version, and each Debian package with its version and the libraries taken from it.

The build also writes that `README.txt` beside the image as `Helikodu-<version>-<arch>.AppImage.contents.txt`.

**Sources.** The LGPL and GPL ask that whoever distributes the binaries also offers the corresponding sources. Every release therefore carries the exact source archives FFmpeg and vgmstream were built from, `ffmpeg-<version>.tar.xz` and `vgmstream-<version>.tar.gz`: the pinned tarballs of `pins.sh`, which `build.sh` copies beside the image and the Release workflow checks against the pinned SHA-256 again before it publishes them, listed in `SHA256SUMS`. `README.txt` in the image says so. The other libraries come from the Debian 13 packages whose versions `README.txt` lists (their sources are at sources.debian.org and snapshot.debian.org).

## Limits

- The system has to provide glibc 2.39 or newer and the libstdc++ of GCC 13 or newer (the newest symbol versions any file in the image uses are `GLIBC_2.39` and `GLIBCXX_3.4.32`; the build prints the glibc one), and PipeWire's client library 1.2 or newer (`libpipewire-0.3.so.0`), which the programs link even when another output is used. That is Debian 13, Ubuntu 24.10, Fedora 41 or newer. Older systems use the source tarball.
- FUSE: the runtime mounts the image with `fusermount3`; without it, `--appimage-extract-and-run` runs it from a temporary copy.
- The image's GTK is 4.18 whatever the system has; features of newer GTK versions are not there.
- The patent-encumbered decoders FFmpeg has (such as AC-3, E-AC-3, DTS, AAC) are in the image, as in most distributions' FFmpeg; the decoders FFmpeg marks as needing a GPL build are not.
- FFmpeg's `codec2`, `lc3` and `ilbc` demuxers are built, but their decoders need external libraries that are not in the image: such files are recognised and then fail to decode, as on a system without those libraries. vgmstream's FSB CELT is off.
- Programs started without a desktop portal (no `xdg-desktop-portal` running) inherit the variables of `AppRun`.
- The image does not add itself to the application list; tools such as Gear Lever or AppImageLauncher do that.
