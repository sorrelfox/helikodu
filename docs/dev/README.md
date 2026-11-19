# Developer documentation

Guides for contributors: the architecture overview, the testing and test-corpus guides, and how to add a decoder or an output backend. Start with [CONTRIBUTING.md](../../CONTRIBUTING.md).

| Document | Content |
|---|---|
| [architecture.md](architecture.md) | Crates, processes and threads, the command/event model, data on disk and the rules that apply everywhere |
| [dbus-api.md](dbus-api.md) | The application's D-Bus interface for scripts and `helikodu-cli`: its methods, the status JSON, errors and versions |
| [fuzzing.md](fuzzing.md) | The fuzz targets, how a campaign is run and a finding handled, the malformed-media corpus, and what each campaign found |
| [track-analysis.md](track-analysis.md) | The per-track analysis (silence, loudness envelope, fade points): what it finds, how it is stored and the background job |
| [threat-model.md](threat-model.md) | What can harm a user, where it enters, and how Helikodu defends against it |
| [performance.md](performance.md) | The performance budgets, how they are measured over the synthetic libraries, and the latest results |
| [bug-severity.md](bug-severity.md) | The four severity levels and how each is handled |
| [flatpak.md](flatpak.md) | The Flatpak: building it, updating its crates and libraries, its permissions and the way to Flathub |
| [releasing.md](releasing.md) | How a release is made: the version, the checklist, and what the Release workflow builds and publishes |
| [appimage.md](appimage.md) | The AppImage: how it is built, what it carries and from where, its licences and limits |
| [packaging-rpm.md](packaging-rpm.md) | The Fedora RPM: how it is built, FFmpeg (ffmpeg-free or RPM Fusion), building it locally |
| [packaging-deb.md](packaging-deb.md) | The Debian and Ubuntu packages: one per distribution, how they are built from the source tarball, tested and checked with lintian |
| [packaging-arch.md](packaging-arch.md) | The Arch Linux package: its PKGBUILD, how the Release workflow builds it, and building it locally |
