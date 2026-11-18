# The Debian and Ubuntu packages

Each GitHub Release has a `.deb` package for Debian 13 and one for Ubuntu 26.04 LTS, for x86_64 (`amd64`) and 64-bit ARM (`arm64`). They are built by the `deb` job of the [Release workflow](../../.github/workflows/release.yml) from the release source tarball, offline, with the distribution's own libraries. They are not in any repository: users download and install the file (see [installing.md](../user/guide/installing.md#debian-and-ubuntu)).

| File | What it is |
|---|---|
| `packaging/deb/debian/` | The packaging template: `control`, `rules`, `copyright` and `source/format`. The build copies it into the unpacked source tarball as `debian/` and writes `debian/changelog` |
| `packaging/deb/build-deb.sh` | Builds the package inside a container of its distribution |
| `packaging/deb/test-deb.sh` | Installs the package in a fresh container, tries it and removes it |

## One package per distribution

A single `.deb` cannot serve both distributions. Debian 13 has FFmpeg 7.1 (`libavformat61`) and GTK 4.18; Ubuntu 26.04 has FFmpeg 8.0 (`libavformat62`) and GTK 4.22. A package built on one depends on library packages the other does not have. So the same source is built twice, each time in a container of the distribution, and each package records its own dependencies:

| Package version | Release file | Built in |
|---|---|---|
| `<version>-1~debian13` | `helikodu_<version>-1.debian13_amd64.deb`, `…_arm64.deb` | `debian:13` |
| `<version>-1~ubuntu26.04` | `helikodu_<version>-1.ubuntu26.04_amd64.deb`, `…_arm64.deb` | `ubuntu:26.04` |

The tilde in the version sorts before anything else, so a later package of the distribution itself (`<version>-1`) replaces ours. A pre-release suffix gets a tilde too: `0.9.0-rc1` is the package version `0.9.0~rc1-1~debian13`, which sorts before `0.9.0-1~debian13`. The release files have dots in place of the tildes (`helikodu_0.9.0.rc1-1.debian13_amd64.deb`), because GitHub renames uploaded files whose names have a tilde, and `SHA256SUMS` has to name the files as they are published.

Older releases are not supported: the GTK baseline is 4.18 (`gtk4` with the `v4_18` feature), which rules out Ubuntu 24.04 LTS (GTK 4.14, FFmpeg 6.1). Ubuntu 25.04 and 25.10 had GTK 4.18 and 4.20 but are past their end of life. A new Ubuntu or Debian release gets its own matrix entry in the workflow.

## What the package contains

The package is built with debhelper (`dh --buildsystem=meson`), so Meson installs what it installs for any distribution, with the prefix `/usr`:

- `/usr/bin/helikodu` and `/usr/bin/helikodu-cli`, and the scan worker in `/usr/libexec/helikodu/` (where the programs look for it);
- the desktop entry, the AppStream metainfo, the D-Bus service file and the icons;
- the manual pages, the translations of every language in `po/LINGUAS`, and the user guide in `/usr/share/helikodu/guide/` (opened by Help › User Guide);
- `/usr/share/doc/helikodu/copyright` and the changelog.

The Meson options are `-Doffline=true` (the vendored crates of the tarball), `-Dbundled_sqlite=false` (the system SQLite, 3.46 on both) and `-Dvgmstream=false` (neither distribution packages libvgmstream, so game audio streams that need it are not played; the Formats page of the settings says so).

**Dependencies.** `Depends` is `${shlibs:Depends}`: `dpkg-shlibdeps` reads the libraries the programs link (GTK 4, FFmpeg, PipeWire, ALSA, SQLite, libopenmpt, libgme, libsidplayfp and their own dependencies) and names the packages that provide them, with the versions the build saw. Two things are loaded only when needed and are therefore `Suggests`: a SoundFont for MIDI files (`fluid-soundfont-gm` or `timgm6mb-soundfont`, found in `/usr/share/sounds/sf2/`) and the JACK library for the JACK output (`libjack-jackd2-0`, `libjack0` or `pipewire-jack`).

**Rust.** The distributions' `rustc` is older than the minimum supported Rust version, so `Build-Depends` does not list Rust: `build-deb.sh` installs rustup and the toolchain of `rust-toolchain.toml` into the build container before `dpkg-buildpackage` runs. Everything else comes from `Build-Depends` (`apt-get build-dep`).

**Debug symbols.** No `-dbgsym` package is made (`dh_strip --no-automatic-dbgsym`): only the package itself is published. The release profile already strips the Rust programs' debug information, so `dh_dwz` is skipped.

## Building locally

Make the source tarball, then build in a container of the distribution. The scripts and the tarball go into one folder that is mounted into the container:

```sh
cargo xtask dist --out /tmp/helikodu-deb
cp packaging/deb/*.sh /tmp/helikodu-deb/
podman run --rm --memory 12g --memory-swap 12g -e CARGO_BUILD_JOBS=4 \
    -v /tmp/helikodu-deb:/w:Z docker.io/library/debian:13 \
    sh /w/build-deb.sh /w/helikodu-<version>.tar.xz /w/out
```

Use `docker.io/library/ubuntu:26.04` for the Ubuntu package. The package and the lintian report (`….deb.lintian.txt`) land in `/tmp/helikodu-deb/out/`. Then try it in a fresh container of the same distribution:

```sh
podman run --rm -v /tmp/helikodu-deb:/w:Z docker.io/library/debian:13 \
    sh /w/test-deb.sh /w/out/helikodu_<version>-1.debian13_amd64.deb
```

`test-deb.sh` installs the package with `apt-get` (which installs its dependencies), runs `helikodu --version` (checking that it prints the package's upstream version) and `helikodu-cli --version`, checks the installed files and the 11 translations (after removing the minimal container image's rule that leaves out manual pages and translations), scans a generated WAV file into a new library and plays it to the WAV and null backends (no sound), then purges the package and checks that none of its files are left.

The build peaks at about the memory of `cargo build --release` (see [ci/README.md](../../ci/README.md#memory-limit)); with four Cargo jobs it takes about five minutes after the build dependencies are installed.

## lintian

`build-deb.sh` runs lintian on the package; an error fails the build, and warnings and notes are kept in the report beside the package. Both packages are clean apart from two findings that are expected, overridden in `debian/helikodu.lintian-overrides` with the reason:

- `package-contains-documentation-outside-usr-share-doc` for the user guide: it is application data that Help opens, and systems that leave out `/usr/share/doc` (such as minimal Ubuntu images) must still install it.
- `hardening-no-fortify-functions` (Debian 13): the programs are Rust and call no C functions that have fortified variants.

## In the Release workflow

The `deb` job runs after `source`, for each distribution on each architecture (x86_64 on `ubuntu-latest`, aarch64 on `ubuntu-24.04-arm`; both images have arm64 variants). It runs `build-deb.sh` in the plain distribution image with the 12 GB memory cap, with `SOURCE_DATE_EPOCH` set to the time of the tagged commit (the date of `debian/changelog`), then `test-deb.sh` in a second, fresh container of the same image, and uploads the package. `assemble` puts the packages into `SHA256SUMS` and `publish` attaches them to the release.
