# Debian and Ubuntu

The `.deb` packages for Debian 13 and Ubuntu 26.04 LTS, attached to each GitHub Release.

| File | What it is |
|---|---|
| `debian/` | The packaging template, copied into the unpacked source tarball as `debian/` at build time; the build writes `debian/changelog` |
| `build-deb.sh` | Builds the package from the source tarball inside a `debian:13` or `ubuntu:26.04` container |
| `test-deb.sh` | Installs the package in a fresh container, tries the programs and removes it again |

How to build and test them, and why there is one package per distribution: [docs/dev/packaging-deb.md](../../docs/dev/packaging-deb.md).
