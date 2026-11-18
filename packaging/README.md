# packaging

One folder per package target. Each one calls the project's own build (Cargo and Meson) and adds nothing else.

The first version of Helikodu is published only as GitHub Releases: the source tarball and the built program. The package targets below are planned for later versions.

| Folder | Target |
|---|---|
| `flatpak/` | The Flatpak manifest, built locally for now and for a later Flathub release ([docs/dev/flatpak.md](../docs/dev/flatpak.md)) |
| `aur/` | `PKGBUILD` files for the Arch User Repository |
| `rpm/` | The RPM spec for Fedora COPR and openSUSE OBS |
| `debian/` | Debian packaging, built on OBS |
