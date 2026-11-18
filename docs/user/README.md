# User documentation

The documentation for people who use Helikodu. It is installed with the application and opened locally from the Help menu (**Help ▸ User Guide**, F1); a copy is published on the website.

`build-guide.py` turns the guide and the FAQ into one HTML page with its own styles and no scripts, fonts or network references. Meson runs it (the `user-guide` target) and installs the page as `share/helikodu/guide/index.html`; the website (`docs/site/build.py`) puts the same chapters into a page of its own at `sorrelfox.github.io/helikodu/guide/`. It fails on a link to a chapter or heading that does not exist, and on a chapter missing from the contents in `guide/README.md`, so `python3 docs/user/build-guide.py --out /tmp/guide.html` also checks the guide. A new chapter goes into the contents and into the `user-guide` input list in `meson.build`.

- [User guide](guide/README.md): installing, first steps, browsing and search, playing, audio output, loudness, equalizer and sound processing, playlists, tag editing, settings, shortcuts, the command line, files and troubleshooting.
- [FAQ](faq.md): offline by design, formats, video files, exclusive mode and PipeWire sample rates, desktop themes, settings, bug reports and privacy.

The manual pages `helikodu(1)`, `helikodu-cli(1)` and `helikodu-scan-worker(1)` are in [`data/man/`](../../data/man/) and are installed with the programs. When a command or an option changes, change its manual page in the same commit; compare with `--help`.

Describe only what the current version does. When a feature arrives, add it here in the same change.
