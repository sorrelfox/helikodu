# The Fedora RPM

Each release has a Fedora RPM for x86_64 and aarch64, `helikodu-<version>-1.fc45.<arch>.rpm`, built by the `rpm` job of the [Release workflow](../../.github/workflows/release.yml). It is not in a repository (no COPR, no signing): users download it from the release page and install it with `dnf`.

| File | What it is |
|---|---|
| `packaging/rpm/helikodu.spec` | The spec: build dependencies, the Meson build and the file list |
| `packaging/rpm/build.sh` | Builds the RPMs from a source tarball in a Fedora container and checks them with rpmlint |
| `packaging/rpm/smoke-test.sh` | Installs the RPM in a clean Fedora container, runs the programs and removes it again |
| `packaging/rpm/helikodu.rpmlintrc` | The rpmlint findings that are intended, each with its reason |

## How it is built

- **From the source tarball, offline.** `rpmbuild` unpacks the release source tarball (`cargo xtask dist`), whose `vendor/` folder and `.cargo/config.toml` hold every crate of `Cargo.lock`, and runs Meson with `-Doffline=true`. The spec does not use `%cargo_prep`, which would point Cargo at Fedora's packaged crates instead; it is a self-contained build, as for a package outside the Fedora repositories. `%cargo_vendor_manifest` writes `cargo-vendor.txt` (installed as a license file), from which RPM generates a `bundled(crate(…))` provide for every crate built in.
- **Fedora's Rust and flags.** Fedora's own `rust` and `cargo` build it (Fedora 45 has Rust 1.100; `rust-toolchain.toml` is ignored without rustup), with `%build_rustflags`: Fedora's optimization, hardening and debug information, so the build also makes `-debuginfo` and `-debugsource` packages (not published). The workspace's release profile (thin LTO, one codegen unit) still applies.
- **Fedora's libraries.** GTK 4, FFmpeg, PipeWire, ALSA, SQLite (not the bundled copy), libopenmpt, libgme and libsidplayfp come from Fedora; the spec names them as `pkgconfig(…)` build dependencies, and RPM adds the library dependencies (`libavformat.so.63()(64bit)` and so on) to the package by itself. `jack.pc` is only needed to build; `libjack` is loaded when the JACK output is chosen, so the package only suggests `pipewire-jack-audio-connection-kit`. It recommends `fluid-soundfont-gm`, the SoundFont that MIDI files play with.
- **License.** The `License` field lists Helikodu's license and those of the crates linked into the programs on Linux; the spec's comment has the `cargo tree` command that lists them. When a change to `Cargo.lock` brings a crate with a license not yet listed, add it there.
- **No vgmstream.** Fedora does not package libvgmstream, so the decoder for game audio streams is left out (`-Dvgmstream=false`), as on any build without it.
- **Checks.** `%check` validates the desktop entry and the metainfo, runs `meson test`, `helikodu --version` and `helikodu-cli --version`. `build.sh` then runs rpmlint on the spec and on every RPM; `smoke-test.sh` installs the binary RPM in a fresh container, runs `helikodu --version` (checking that it prints the package's version) and `helikodu-cli --version`, checks that the scan worker, the user guide, a translation and the manual pages are installed, scans a folder with one WAV file, plays it to a WAV file (never to a sound device), and removes the package, checking that nothing is left behind.

## FFmpeg: ffmpeg-free or RPM Fusion

Fedora ships FFmpeg as **ffmpeg-free**, built without the codecs Fedora cannot distribute. The RPM is built against `ffmpeg-free-devel` and needs the libraries by their names (`libavcodec.so.63` and so on; FFmpeg 9), which two packages provide:

- `libavcodec-free`, `libavformat-free` and the rest, from Fedora (installed by default on a desktop);
- `ffmpeg-libs`, from RPM Fusion, the full build.

So the same RPM works with either. With ffmpeg-free, the audio decoders missing compared with the full build are E-AC-3 (Dolby Digital Plus), WMA Pro, WMA Lossless, MPEG-4 ALS, DST (compressed DSD in DFF files), aptX, ATRAC9, Xbox XMA and a few rare ADPCM variants; AAC (including HE-AAC, through libfdk-aac), MP3, Opus, Vorbis, FLAC, ALAC, AC-3, DTS, TrueHD, Monkey's Audio, WavPack, Musepack, TAK, TTA and WMA 1/2 all play. To get the rest, a user switches to RPM Fusion's FFmpeg (`sudo dnf swap ffmpeg-free ffmpeg --allowerasing` with RPM Fusion's free repository enabled); Helikodu needs no reinstall. The user guide says the same in [installing.md](../user/guide/installing.md#fedora).

The FFmpeg major version is part of those library names: the RPM built for Fedora 45 (FFmpeg 9) does not install on a Fedora with another FFmpeg major version. Each release builds for one Fedora version, set by `FEDORA` in the workflow job; move it to the new Fedora when one is released.

## Building it locally

From the top folder of the repository, with a source tarball made by `cargo xtask dist` (in `target/dist/`):

```sh
podman run --rm --memory 12g --memory-swap 12g -e CARGO_BUILD_JOBS=4 \
    -v "$PWD:/src:Z" registry.fedoraproject.org/fedora:45 \
    sh /src/packaging/rpm/build.sh /src/target/dist/helikodu-0.9.0.tar.xz /src/target/rpm
podman run --rm -v "$PWD:/src:Z" registry.fedoraproject.org/fedora:45 \
    sh /src/packaging/rpm/smoke-test.sh /src/target/rpm/helikodu-0.9.0-1.fc45.x86_64.rpm
```

`build.sh` installs the build dependencies itself (`dnf builddep` on the spec). To build as the Release workflow does, add `-e CARGO_BUILD_JOBS=3 -e HELIKODU_EXTRA_RUSTFLAGS="-C debuginfo=line-tables-only"` (see [Memory](#memory)).

## Memory

Fedora's `%build_rustflags` include full debug information (`-Cdebuginfo=2`), which the other release builds do not have; with thin LTO and one codegen unit, the link of each program then holds the debug information of every crate. Measured in a container capped at 12 GB with six Cargo jobs, the cgroup peak, page cache included, was 11.5 GB: no out-of-memory kill, but too close to the runners' limit (see [ci/README.md](../../ci/README.md#memory-limit)).

The `rpm` job therefore runs with three Cargo jobs instead of four, and `build.sh` adds `-C debuginfo=line-tables-only` after Fedora's flags (`HELIKODU_EXTRA_RUSTFLAGS`, passed to the spec as `helikodu_extra_rustflags`; the last `-C debuginfo` wins). Line tables are enough for the file and line numbers of a backtrace; the `-debuginfo` package, which is not published, loses only the variables and types. Measured the same way with these settings (x86_64), the peak was 6.2 GB, page cache included, and the -debuginfo package shrank from 84 MB to 31 MB.

Outside a container, on Fedora with `rpm-build` installed: copy the tarball into `~/rpmbuild/SOURCES/`, then `sudo dnf builddep packaging/rpm/helikodu.spec` and `rpmbuild -ba --define "upstream_version 0.9.0" packaging/rpm/helikodu.spec`.

## The version

The workflow passes the tag's version to rpmbuild (`--define "upstream_version …"`), so the RPM always has the version of the source tarball. A pre-release suffix becomes a tilde (`0.9.0-rc1` is the RPM version `0.9.0~rc1`, which sorts before `0.9.0`). GitHub renames uploaded files with a tilde in their name, so the workflow publishes that RPM with a dot in its place (`helikodu-0.9.0.rc1-1.fc45.x86_64.rpm`); `dnf` reads the version from the package, not from the file name. The spec's default for `upstream_version`, used when it is not passed, is updated with the version bump. The release is always `1`: a published release is never rebuilt. The `%changelog` entry is written from the version and the build date and points to `CHANGELOG.md`.
