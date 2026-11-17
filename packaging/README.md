# packaging

One folder per package target. Each one calls the project's own build (Cargo and Meson) and adds nothing else.

| Folder | Target |
|---|---|
| `flatpak/` | The Flatpak manifest; the primary package, published on Flathub |
| `aur/` | `PKGBUILD` files for the Arch User Repository |
| `rpm/` | The RPM spec for Fedora COPR and openSUSE OBS |
| `debian/` | Debian packaging, built on OBS |
