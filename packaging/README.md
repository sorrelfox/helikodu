# packaging

One folder per package target. Each one calls the project's own build (Cargo and Meson) and adds nothing else.

Helikodu is published only as GitHub Releases. Every release carries, besides the source tarball, the ready-built programs and the AppImage (`build-aux/appimage/`), a file built from each folder below except `aur/`; the Release workflow builds and tests them ([docs/dev/releasing.md](../docs/dev/releasing.md)). Repositories (Flathub, the AUR, COPR and the like) come later.

| Folder | Target |
|---|---|
| `flatpak/` | The Flatpak manifest: a single-file bundle on every release, and later the Flathub build ([docs/dev/flatpak.md](../docs/dev/flatpak.md)) |
| `rpm/` | The Fedora RPM ([docs/dev/packaging-rpm.md](../docs/dev/packaging-rpm.md)) |
| `deb/` | The `.deb` packages for Debian 13 and Ubuntu 26.04 ([docs/dev/packaging-deb.md](../docs/dev/packaging-deb.md)) |
| `arch/` | The Arch Linux `PKGBUILD` ([docs/dev/packaging-arch.md](../docs/dev/packaging-arch.md)) |
| `aur/` | `PKGBUILD` files for the Arch User Repository, for later |
