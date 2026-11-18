# The Flatpak

`packaging/flatpak/io.github.sorrelfox.Helikodu.yml` builds Helikodu as a Flatpak on the GNOME runtime. It is not on Flathub yet: the first versions are released only on GitHub (see [releasing.md](releasing.md)), where every release carries the Flatpak as a single-file bundle per architecture ([below](#the-release-bundle)).

| File | What it is |
|---|---|
| `packaging/flatpak/io.github.sorrelfox.Helikodu.yml` | The manifest: runtime, permissions and modules |
| `packaging/flatpak/cargo-sources.json` | Every crate of `Cargo.lock` as a download with its checksum, so that Cargo builds offline (Flathub builds have no network) |

## Building

Install Flatpak, `flatpak-builder` and the Flathub remote, then the runtime, the SDK and the two SDK extensions the build uses. The extensions' branch is the runtime's base (`26.08` for GNOME 51):

```sh
flatpak install --user flathub org.gnome.Platform//51 org.gnome.Sdk//51 \
    org.freedesktop.Sdk.Extension.rust-stable//26.08 \
    org.freedesktop.Sdk.Extension.llvm22//26.08
```

Build and install it for your user, from the top folder of the repository:

```sh
flatpak-builder --user --install --force-clean \
    --state-dir=/tmp/helikodu-flatpak/state /tmp/helikodu-flatpak/build \
    packaging/flatpak/io.github.sorrelfox.Helikodu.yml
flatpak run io.github.sorrelfox.Helikodu
```

Any folder outside the repository does for the build folders (inside it they would be copied into the next build). `--jobs=N` limits the parallel jobs; the whole build peaks at about the same memory as `cargo build --release` (see [ci/README.md](../../ci/README.md#memory-limit)). The first build takes a while, as it builds the five decoder libraries and every crate; later builds reuse the libraries.

The manifest takes the working tree as it is (`type: dir`), uncommitted changes included. Other ways to run it:

```sh
flatpak run --command=helikodu-cli io.github.sorrelfox.Helikodu --help
flatpak run --command=sh io.github.sorrelfox.Helikodu          # a shell inside the sandbox
flatpak-builder --run /tmp/helikodu-flatpak/build \
    packaging/flatpak/io.github.sorrelfox.Helikodu.yml helikodu  # without installing
```

A single-file bundle to try on another computer:

```sh
flatpak build-export /tmp/helikodu-flatpak/repo /tmp/helikodu-flatpak/build
flatpak build-bundle /tmp/helikodu-flatpak/repo helikodu.flatpak io.github.sorrelfox.Helikodu \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
```

Check the manifest and the built repository with Flathub's linter (`flatpak install --user flathub org.flatpak.Builder` first):

```sh
flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest packaging/flatpak/io.github.sorrelfox.Helikodu.yml
flatpak run --filesystem=/tmp/helikodu-flatpak --command=flatpak-builder-lint org.flatpak.Builder repo /tmp/helikodu-flatpak/repo
```

The manifest check passes without findings. Newer releases of the linter (as in Flathub's CI image) also report `finish-args-login1-system-talk-name` and `finish-args-own-name-wildcard-org.freedesktop.ReserveDevice1`; both permissions are deliberate (see the permission list above), the release workflow lists them as exceptions with their reasons, and the Flathub submission must ask for the same exceptions. The repository check (on a repository made with `flatpak-builder --repo=…`) reports only `metainfo-missing-screenshots` and `appstream-screenshots-not-mirrored-in-ostree`: the metainfo has no screenshots yet; they are added for the Flathub submission (see below), and Flathub's build mirrors them. The linter runs in a sandbox of its own: for a repository outside your home folder, add `--filesystem=<that folder>` after `flatpak run`.

## The release bundle

The Release workflow's `flatpak` job builds `helikodu-<version>-<arch>.flatpak` for x86_64 and aarch64 and attaches both to the GitHub Release (see [releasing.md](releasing.md#how-the-files-are-built)). Per architecture it runs the same three steps as above, on a native runner in Flathub's `ghcr.io/flathub-infra/flatpak-github-actions:gnome-51` image (which has the GNOME 51 runtime and SDK, `flatpak-builder` and `flatpak-builder-lint` for both architectures):

```sh
flatpak-builder --user --force-clean --disable-rofiles-fuse --install-deps-from=flathub --jobs=$CARGO_BUILD_JOBS \
    --state-dir=STATE --repo=REPO BUILD packaging/flatpak/io.github.sorrelfox.Helikodu.yml
flatpak-builder-lint --exceptions --user-exceptions EXCEPTIONS.json repo REPO
flatpak build-bundle --arch=ARCH --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    REPO helikodu-<version>-ARCH.flatpak io.github.sorrelfox.Helikodu
```

The `dir` source takes the checked-out tag. The linter may report only the two screenshot findings described above; anything else stops the release. The job then installs the bundle in its container, checks that `flatpak info` shows the release's version and architecture, and runs `helikodu --version` and `helikodu-cli --version` in it with `flatpak run`.

`--runtime-repo` puts Flathub's `.flatpakrepo` into the bundle. When a user installs it, Flatpak offers to add the Flathub remote (if it is missing) and installs `org.gnome.Platform//51` and its extensions from there, so the bundle needs nothing installed beforehand except Flatpak:

```sh
flatpak install --user --bundle helikodu-<version>-x86_64.flatpak
```

The bundle holds only the application (about 16 MB); the runtime is a separate download of several hundred megabytes the first time. A bundle does not update itself: the next release's bundle is installed over it the same way. The installation's origin remote has no URL, so `flatpak update` does not touch the application (it updates the runtime).

## What is built

| Module | Version | Built how | Why |
|---|---|---|---|
| libopenmpt | 0.8.10 | Autotools, library only (no `openmpt123`, examples or tests); MPEG and Vorbis samples through the runtime's mpg123 and libvorbis | Tracker modules. Not in the runtime |
| libgme | 0.6.5 | CMake, shared library only | Game console music. Not in the runtime |
| libresidfp | 1.2.3 | Autotools, library only | The reSIDfp SID chip emulation, a separate library since libsidplayfp 3; without it libsidplayfp 3 builds without the emulation Helikodu uses |
| libsidplayfp | 3.1.2 | Autotools, without the USBSID and exSID hardware drivers | SID tunes. Not in the runtime. Version 3 rather than 2.x (which the native release builds link) because only the 3.x series has the fix for CVE-2026-111148; the binding's shim supports both |
| vgmstream | r2168 | CMake, only the shared library with the public `libvgmstream` API (no CLI, no player, no Audacious plugin); MPEG, Vorbis, Speex and FFmpeg from the runtime | Game audio streams. Rarely packaged, so the Flatpak is where most users get it. CELT is off: vgmstream's CMake script downloads the CELT sources during the build, which an offline build cannot do; only some old FSB files use it |
| helikodu | the working tree | Meson (`-Doffline=true -Dbundled_sqlite=true -Dvgmstream=true`), which runs Cargo | The programs, the data files, the translations and the user guide |

Every download is pinned by URL and SHA-256. The `x-checker-data` entries let Flathub's external data checker propose updates of the libraries.

**FFmpeg** is the runtime's, not a module. Since the 25.08 base, the patent-encumbered decoders come from the `org.freedesktop.Platform.codecs-extra` extension, which Flatpak installs with the runtime, so the manifest declares nothing for it. A minimal FFmpeg of our own, with only the demuxers and decoders the bridge's format whitelist allows, was not chosen: Helikodu would then ship patent-encumbered decoders (AAC, AC-3, E-AC-3, DTS, WMA and more) itself, which the project avoids, and would carry FFmpeg's security updates instead of the runtime's maintainers; the converter needs the runtime's encoders as well. The bridge applies its own locks (custom I/O, `protocol_whitelist=file`, the demuxer whitelist) whatever FFmpeg it gets.

**SQLite** is the copy bundled with rusqlite (`-Dbundled_sqlite=true`), so the runtime's version does not matter.

**No SoundFont** is included. The plan is to install a small, freely licensed General MIDI SoundFont with the Flatpak after a licence review; no SoundFont has passed one yet. Until then MIDI files need a SoundFont chosen in **Settings ▸ Playback ▸ MIDI files** (the file chooser grants access to it). The MIDI decoder already looks for `/app/share/soundfonts/default.sf2`, so adding one later is a module that installs a file there, plus its licence in `/app/share/licenses/`.

**Rust** comes from the `rust-stable` SDK extension, not from `rust-toolchain.toml` (which only rustup reads). It must be at least the minimum supported Rust version in `Cargo.toml` (`rust-version`); the extension follows stable Rust a few weeks behind. **libclang** for bindgen (the FFmpeg and PipeWire bindings) comes from the `llvm22` extension, at build time only.

## Paths inside the sandbox

Meson installs into `/app`, so the paths built into the programs are the Flatpak's:

| What | Where | Found by |
|---|---|---|
| Programs | `/app/bin/helikodu`, `/app/bin/helikodu-cli` | |
| Scan worker | `/app/libexec/helikodu/helikodu-scan-worker` | `find_worker_program`, as `../libexec/helikodu` from the program |
| Translations | `/app/share/locale/<code>/LC_MESSAGES/helikodu.mo` (all eleven) | `HELIKODU_LOCALEDIR`, set by Meson |
| User guide | `/app/share/helikodu/guide/index.html` | `HELIKODU_GUIDEDIR`, set by Meson; Help opens it through the OpenURI portal |
| Library, settings, cache | `~/.var/app/io.github.sorrelfox.Helikodu/{data,config,cache}/helikodu` | The XDG variables Flatpak sets |
| MPRIS cover | `$XDG_RUNTIME_DIR/app/io.github.sorrelfox.Helikodu/` | Shared with the host at the same path |

All eleven translations stay in the application (`separate-locales: false`) rather than in a `.Locale` extension, because Helikodu can show its interface in a language other than the desktop's (**Settings ▸ Appearance ▸ Language**), and a `.Locale` extension holds only the desktop's languages.

The library and settings of a native installation (`~/.local/share/helikodu`, `~/.config/helikodu`) are not visible to the Flatpak and not copied into it.

## Permissions

Kept to what a feature needs; each one is in the manifest with a comment.

| Permission | Why |
|---|---|
| `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc`, `--device=dri` | The window: Wayland, X11 only where there is no Wayland (with the shared memory X11 needs), and the GPU for GTK's renderer |
| `--socket=pulseaudio` | The PulseAudio output (also through PipeWire's PulseAudio service). Flatpak also gives `/dev/snd` with it, which the ALSA output (`hw:` devices, exclusive mode) uses; `--device=all` is therefore not needed |
| `--filesystem=xdg-run/pipewire-0:ro` | The native PipeWire output, its device list, and JACK through PipeWire's JACK library. `:ro` is enough to connect to the socket. GNOME Software shows it as "potentially unsafe"; other music players use it too. Inside the sandbox WirePlumber gives Helikodu only read and execute permissions, so it cannot change the graph's clock settings (the settings hint covers that) |
| `--filesystem=xdg-music` | The music folder, read and write: the library, the tag editor, the file organizer, cover files beside albums, and live updates (inotify does not work through the document portal). Other folders, removable drives included, are added through the file chooser; they are then rescanned periodically instead of watched |
| `--talk-name=org.kde.StatusNotifierWatcher` | The tray icon. The item registers with its unique bus name, since the sandbox cannot own `org.kde.StatusNotifierItem-<pid>-<n>` |
| `--own-name=org.freedesktop.ReserveDevice1.*` | Exclusive ALSA output while PipeWire or PulseAudio holds the card: the device reservation protocol asks the sound server to hand the card over and owns `org.freedesktop.ReserveDevice1.Audio<N>` while playing. Owning a name includes talking to it, so no `--talk-name` for these names is added (Flatpak keeps one policy per name, and a `--talk-name` after it would lower the policy to talk) |
| `--system-talk-name=org.freedesktop.login1` | **Pause before sleep** (logind's `PrepareForSleep` and a short delay lock so that the pause happens before the system sleeps) and the screen lock where the portal does not report it. Without it these settings do nothing in the Flatpak |

Granted by Flatpak without a permission: the application ID's own bus name `io.github.sorrelfox.Helikodu` (the single instance, D-Bus activation and the `io.github.sorrelfox.Helikodu.Control1` interface that `helikodu-cli` and scripts use) and the MPRIS name `org.mpris.MediaPlayer2.io.github.sorrelfox.Helikodu`. The portals Helikodu uses need none either: FileChooser (adding folders, choosing a SoundFont, exporting), OpenURI (Help, "Open containing folder"), Inhibit (keeping the computer awake while playing, the screen-lock monitor), Settings (colour scheme, accent, contrast, reduced motion), GlobalShortcuts, Notification and Realtime.

**No `--share=network`.** Helikodu works offline by design; without this permission the sandbox has no network interface except loopback, so nothing in the process can reach another computer whatever a library tries. `helikodu-cli` runs in the same sandbox.

Not requested: `--device=all` (see `pulseaudio` above), `--filesystem=home` or `host`, `/run/media`, `/media` and `/mnt` (added through the file chooser instead), `--talk-name=org.freedesktop.FileManager1` (the OpenURI portal opens the folder instead), and the native installation's data folders.

Users can grant more themselves, for example a music folder outside `~/Music` that should be watched live:

```sh
flatpak override --user --filesystem=/srv/music io.github.sorrelfox.Helikodu
```

## Updating

**Cargo crates.** Regenerate `cargo-sources.json` whenever `Cargo.lock` changes, and commit both together. It is made by `flatpak-cargo-generator.py` from [flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools) (`cargo/`); `uv` installs its Python dependencies (aiohttp, PyYAML, tomlkit) on the fly:

```sh
curl -LO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
uv run flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

(Without `uv`, install the three packages with pip and run it with `python3`.) The file was last made with the script at commit `41c20aa1` of flatpak-builder-tools.

**Libraries.** Change the version in the module's `url`, download the file and put its `sha256sum` in `sha256`. Check the library's release notes for changed build options, then build and play a file of that kind. vgmstream's source is the GitHub archive of the release tag; its public API version (`LIBVGMSTREAM_API_VERSION_*` in `src/libvgmstream.h`) must stay 1.x with a minor version of at least 1, which `helikodu-vgmstream-sys` expects.

**Runtime.** A new GNOME runtime changes `runtime-version` and the extensions' branch (`rust-stable` and `llvmNN` follow the runtime's freedesktop base; pick the `llvm` version the base has). Check that FFmpeg's major version is one the bridge supports (the runtime's FFmpeg is its FFmpeg; `flatpak run --command=sh org.gnome.Sdk//NN -c 'pkg-config --modversion libavformat'`), and that GTK still meets the baseline.

## Testing it

After a build, check at least:

- `flatpak run --command=helikodu-cli io.github.sorrelfox.Helikodu --version`, and `… play --backend null FILE` for a file in `~/Music` (`--backend wav --wav-out FILE` writes what it would play).
- The window opens, a folder in `~/Music` is scanned, a file plays and the formats page lists vgmstream, libopenmpt, libgme and libsidplayfp as available.
- Help ▸ User Guide (F1) opens the guide.
- No network: `flatpak run --command=sh io.github.sorrelfox.Helikodu -c 'cat /proc/net/dev'` lists only `lo`.
- `flatpak info --show-permissions io.github.sorrelfox.Helikodu` shows the permissions above and no `network`.

## Towards Flathub

Flathub builds from its own repository, `flathub/io.github.sorrelfox.Helikodu`, which holds a copy of the manifest and `cargo-sources.json`. For the submission:

1. In the copy, replace the `dir` source of the `helikodu` module with the release:

   ```yaml
   - type: git
     url: https://github.com/sorrelfox/helikodu.git
     tag: v1.0.0
     commit: <the tag's commit>
   ```

   (or the release's source tarball as an `archive` with its SHA-256), and keep `cargo-sources.json` from the same tag.
2. The metainfo needs a `<release>` entry for that version and screenshots taken from the running application (`<screenshots>` with image URLs on GitHub, for example the website's pictures); the content rating is already there. `flatpak-builder-lint repo` reports what is still missing.
3. Open a pull request against `flathub/flathub` (branch `new-pr`) as the Flathub submission guide describes. Expect questions about `xdg-run/pipewire-0`, the `ReserveDevice1` wildcards and `org.freedesktop.login1`; the table above has the reasons.
4. `io.github.*` IDs are verified through the GitHub account that owns `github.com/sorrelfox`; no website or DNS record is needed.
