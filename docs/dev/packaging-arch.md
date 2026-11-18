# The Arch Linux package

`packaging/arch/PKGBUILD` builds Helikodu as an Arch Linux package with `makepkg`. The Release workflow builds it from the release's source tarball and attaches it to the GitHub Release as `helikodu-<version>-1-x86_64.pkg.tar.zst`. It is not in the AUR yet; the same PKGBUILD is meant to go there later.

| File | What it is |
|---|---|
| `packaging/arch/PKGBUILD` | The package recipe: dependencies, the Meson build, the check and the install |
| `packaging/arch/build-package.sh` | Builds the package in an `archlinux` container, checks it with namcap, installs it, tries the programs and removes it again; the Release workflow runs it |

## What the package holds

- **Built from the source tarball, offline.** `build()` runs `arch-meson` with `-Doffline=true` and `-Dhelperdir=/usr/lib/helikodu`, so Cargo uses the crates vendored in the tarball and needs no network. `check()` runs `meson test` (the desktop entry and the metainfo are validated). `package()` runs `meson install`.
- **The system's libraries.** FFmpeg, GTK 4, PipeWire, ALSA, SQLite, libopenmpt, libgme and libsidplayfp come from the Arch repositories as package dependencies (SQLite is not built in). JACK is needed only to build (`jack.pc`); at run time `libjack` is loaded when the JACK output is chosen, so it is an optional dependency.
- **No vgmstream.** vgmstream is only in the AUR, not in the Arch repositories, so the package is built without its decoder (the meson option `vgmstream` stays off). Game audio streams that only vgmstream decodes do not play with this package; the Flatpak has them.
- **Rust.** `makedepends` names `cargo`, which the `rust` package and `rustup` both provide. `RUSTUP_TOOLCHAIN=stable` makes a rustup installation use its stable toolchain instead of the version pinned in `rust-toolchain.toml`. Arch's Rust is current, well above `rust-version` in `Cargo.toml`.
- **Paths.** The programs go to `/usr/bin`, the scan worker to `/usr/lib/helikodu`, Arch's place for a package's helper programs (Meson's `helperdir` option; the programs are built to look for it there), and the user guide to `/usr/share/helikodu/guide`.
- **`options=('!lto')`.** The release profile already links with thin LTO; makepkg's own `-flto` would reach only the C parts of the build.
- **x86_64 only.** Arch Linux itself supports only x86_64, and the `archlinux` container image has no arm64 variant; Arch Linux ARM is a separate project.

## The version and the checksum

`_srcver` is the upstream version (`0.9.0`); `pkgver` is the same without the hyphen of a pre-release suffix, so `1.0.0-rc1` becomes `1.0.0rc1`, which pacman sorts before `1.0.0`. The source is the tarball of the GitHub Release.

The checksum in the repository is `SKIP`: the PKGBUILD is part of the source tarball, so the tarball's checksum cannot be written into it beforehand. `build-package.sh` sets `_srcver` from the tarball's name and `sha256sums` from the tarball itself, and puts the tarball beside the PKGBUILD, where makepkg uses it instead of downloading. For the AUR, set `_srcver` to the released version and fill in the checksum of the published tarball with `updpkgsums` (from `pacman-contrib`), which matches the line in `SHA256SUMS`.

## Building it locally

Make the source tarball, then build in a container (Podman or Docker; makepkg does not need to be installed on the host):

```sh
cargo xtask dist --out /tmp/helikodu-dist
mkdir -p /tmp/helikodu-arch
podman run --rm --memory 12g --memory-swap 12g -e CARGO_BUILD_JOBS=4 \
    -v "$PWD:/src:ro,Z" -v /tmp/helikodu-dist:/dist:ro,Z -v /tmp/helikodu-arch:/out:Z \
    docker.io/library/archlinux:latest \
    sh /src/packaging/arch/build-package.sh /dist/helikodu-<version>.tar.xz /out
```

The script updates the container, installs `base-devel` and `namcap`, and runs `makepkg --syncdeps` as an unprivileged user (makepkg refuses to run as root). After the build it runs namcap on the PKGBUILD and the package, installs the package, runs `helikodu --version` (checking that it prints the package's version) and `helikodu-cli --version`, checks with `ldd` that every library of the three programs is found and that the scan worker, the user guide, the data files and every translation of `po/LINGUAS` are in place, scans a generated test file and plays it into a WAV file with `helikodu-cli play --backend wav` (no sound), and removes the package again. The package lands in the output folder.

On an Arch system, `makepkg -si` in a folder with the PKGBUILD (and the tarball, or a network connection to download it) builds and installs it directly.

## namcap

`build-package.sh` runs namcap on the PKGBUILD and on the package and prints its findings without failing on them. Both are clean: with the scan worker in `/usr/lib/helikodu`, no file is outside the paths Arch expects.
