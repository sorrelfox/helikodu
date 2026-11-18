# Releasing

Helikodu is published as GitHub Releases. Pushing a version tag (`v0.1.0`) starts the [Release workflow](../../.github/workflows/release.yml), which publishes:

| File | What it is |
|---|---|
| `helikodu-<version>.tar.xz` | The source tarball, made by `cargo xtask dist`: the files of the tagged commit (without the website, `docs/site`, which no build uses), every crate of `Cargo.lock` vendored into `vendor/`, and a `.cargo/config.toml` that builds from them, so it builds without network access |
| `helikodu-<version>-x86_64-linux.tar.xz` | The programs for x86_64, built from that source tarball and installed by Meson for the prefix `/usr/local`: `bin/`, `libexec/helikodu/`, the desktop entry and the AppStream metainfo (both with their translations), the D-Bus service file, the icons, the compiled translations (`share/locale/<code>/LC_MESSAGES/helikodu.mo`, found beside the programs wherever the archive is unpacked; the workflow checks that every language of `po/LINGUAS` is there), the manual pages and the user guide (`share/helikodu/guide/index.html`, opened by Help › User Guide), with an `INSTALL.txt` note |
| `helikodu-<version>-aarch64-linux.tar.xz` | The same for 64-bit ARM (aarch64), built on GitHub's native arm64 runner |
| `SHA256SUMS` | The SHA-256 checksums of the three tarballs, for `sha256sum --check` |

The release notes are the version's section of `CHANGELOG.md`. Every 0.x version, and every version with a suffix such as `-rc1`, is marked as a pre-release.

Flatpak, Flathub and distribution packages come later; the files in `packaging/` are not used yet.

## The version

The version is set in one place: `version` under `[workspace.package]` in the root `Cargo.toml`. Every crate inherits it, and Meson reads it from there. To bump it, for example to 0.1.0:

1. Change `version = "0.0.0"` to `version = "0.1.0"` in `Cargo.toml`.
2. Run `cargo update --workspace` so that `Cargo.lock` records the new version of the workspace crates (every build with `--locked` fails until it does).
3. Add the release to `data/io.github.sorrelfox.Helikodu.metainfo.xml`, replacing `<releases/>` the first time:

   ```xml
   <releases>
     <release version="0.1.0" date="2026-12-01" type="development"/>
   </releases>
   ```

   (`type="development"` while the version is 0.x; a stable release leaves `type` out.)
4. Rename `## [Unreleased]` in `CHANGELOG.md` to `## [0.1.0] - 2026-12-01` and start a new, empty `## [Unreleased]` above it.

`cargo xtask dist --tag v0.1.0` checks all of this: the tag must be `v` followed by the workspace version, the working tree must be clean, `CHANGELOG.md` must have a `## [0.1.0]` section and the metainfo a `<release version="0.1.0">` entry. The Release workflow runs it with the pushed tag, so a tag that does not match stops the release before anything is published.

## Checklist

1. **Changelog and metainfo.** `CHANGELOG.md` (Keep a Changelog style) lists the user-visible changes under the new version, and the metainfo has its `<release>` entry (see above).
2. **Translations.** The string freeze has run its course and the translations are merged (see [Translations](#translations) below).
3. **Website.** The start page (`docs/site/pages/index.html`) says what the release can do, and the screenshots are drawn again if the interface has changed (`docs/site/shots.py`). `docs/site/build.py --out target/site` builds the site and checks its links; the Pages workflow publishes it when the tag is pushed (see [docs/site/README.md](../site/README.md)).
4. **Version bump** in `Cargo.toml` and `Cargo.lock` (see above).
5. **Checks.** `cargo xtask ci` passes on a clean checkout (a fresh `git clone`, not the working copy). The extended checks (the "Extended checks" workflow, started by hand in the Actions tab) and the CI workflow (also started by hand) are green for the commit to be tagged.
6. **Manual QA** sign-off on the commit to be tagged.
7. **Dry run.** Start the Release workflow by hand on the branch (Actions → Release → Run workflow). It builds everything and keeps the files as a workflow artifact, `release-assets`, without publishing. Download it, then unpack the binary tarball and start the programs on a desktop. Locally, `cargo xtask dist` makes the source tarball in `target/dist/`; check that it builds offline:

   ```sh
   mkdir /tmp/helikodu-dist && tar -xJf target/dist/helikodu-0.1.0.tar.xz -C /tmp/helikodu-dist
   cd /tmp/helikodu-dist/helikodu-0.1.0
   meson setup _build --buildtype=release -Doffline=true && meson compile -C _build
   ```

8. **Signed tag.** Tag the release commit on `main` and push the tag:

   ```sh
   git tag -s v0.1.0 -m "Helikodu 0.1.0"
   git push origin v0.1.0
   ```

   The push starts the Release workflow (and the CI workflow). It makes the source tarball, then on an x86_64 and an aarch64 runner runs the build and tests (`cargo xtask ci --tests-only`) and builds the programs from the tarball offline, and creates the GitHub Release once both builds have passed. Each build runs with at most four Cargo jobs inside a container capped at 12 GB of memory, the runner's limit; see [ci/README.md](../../ci/README.md#memory-limit).
9. **Check the release page**: the notes, the pre-release mark and the four files. Download the files and run `sha256sum --check SHA256SUMS`.

If the workflow fails, fix the cause on `main`, delete the tag (`git push --delete origin v0.1.0`, `git tag -d v0.1.0`) and tag again. A published release is never replaced: a mistake in it is fixed by a new patch version.

## How the files are built

- **Architectures.** The workflow has four jobs: `source` makes the source tarball and the release notes; `build` runs once per architecture, x86_64 on `ubuntu-latest` and aarch64 on `ubuntu-24.04-arm` (GitHub's arm64 runner, free for public repositories), each natively, without emulation, so the tests also run on ARM; `assemble` collects the files and writes `SHA256SUMS`; `publish` creates the release. A failure in either architecture stops the release. The build step checks that the container runs on the architecture its archive is named for.
- **Container.** Everything is built in `ci/containers/debian.Containerfile` (Debian 13), the oldest supported base: glibc 2.41, GTK 4.18, FFmpeg 7.1, PipeWire 1.4. A binary built there runs on these and newer versions of the libraries with the same library names. The FFmpeg libraries are the exception: a new FFmpeg major version renames them (`libavformat.so.61` is FFmpeg 7), so on a system with only newer FFmpeg libraries the binary tarball does not start, and Helikodu is built from the source tarball instead. SQLite is built in (`-Dbundled_sqlite=true`), so the system's SQLite version does not matter.
- **From the source tarball.** The binaries are built from the unpacked source tarball with `-Doffline=true`, as a distribution would build them. This also proves on every release that the tarball builds without network access.
- **Reproducibility.** The toolchain is pinned in `rust-toolchain.toml`, `Cargo.lock` is committed and every Cargo command uses `--locked`. Both tarballs list their files sorted by name, with owner and group 0, the modification time of the tagged commit (or `SOURCE_DATE_EPOCH` if set) and single-threaded xz, so the same commit gives the same source tarball. The release build uses no caches.
- **Permissions.** The workflow has read-only access to the repository; only its last job, which creates the release from the built files, may write (`contents: write`).

## `cargo xtask dist`

```sh
cargo xtask dist                   # target/dist/helikodu-<version>.tar.xz, .sha256 and release-notes.md
cargo xtask dist --tag v0.1.0      # the same, with the release checks above
cargo xtask dist --out DIR         # write into DIR instead of target/dist
```

The tarball holds the files Git tracks, plus untracked files that are not ignored (so it can be tried before a commit; with `--tag` the tree must be clean). Build folders (`target/`, `spikes/*/target/`, `_build/`) and the website (`docs/site/`, mostly screenshots) are never included. `release-notes.md` is the version's changelog section, or the `[Unreleased]` section while there is none. The command needs Git, GNU tar and xz, and the crates of `Cargo.lock` in Cargo's cache or a network connection for `cargo vendor`.

## Translations

Translations come in as pull requests (see [po/README.md](../../po/README.md)). Before a release the user-visible strings are frozen, so that translators work on text that no longer changes.

1. **Sources complete.** `cargo xtask check-potfiles` (also part of `cargo xtask ci`) checks that `po/POTFILES.in` lists exactly the files with translatable strings: every Rust file that calls `tr(`, `ntr(`, `trc(` or `ntrc(` and every GtkBuilder file with `translatable="yes"`, plus the desktop entry and the metainfo. Add a new file there in the change that adds its first string. It also fails on a call of these helpers whose messages are not string literals (such as `tr(name)` or `tr(match …)`), because xgettext extracts literals only: pass each text to the helper as a literal, and use `trc(context, text)` for an English text that means two different things.
2. **Template.** Regenerate `po/helikodu.pot` and commit it:

   ```sh
   meson setup _build && meson compile -C _build helikodu-pot
   ```

   Meson needs every `.po` file that `po/LINGUAS` lists. `meson compile -C _build helikodu-update-po` merges the new template into every `.po` file; the messages that are new or changed then wait for their translators.
3. **Freeze.** Announce the string freeze in the release's tracking issue, with the date it ends, at least two weeks before the planned tag. From then until the tag, no change adds or changes a user-visible string (labels, messages, tooltips, accessible names, the desktop entry and the metainfo); a string that has to change anyway is announced to translators in the same issue. Fixes that change no string go in as usual.
4. **Import.** Merge the translation pull requests that arrive during the freeze. `cargo xtask check-translations` (also part of `cargo xtask ci`) checks every language of `po/LINGUAS`: `msgfmt --check` (GNU gettext 0.24 or newer, which checks the Rust placeholders), that each file matches the current template, that nothing is fuzzy or untranslated, the placeholders msgfmt does not check, markup, and the language's plural rule. A file behind the template is updated with `msgmerge --update --backup=none po/<code>.po po/helikodu.pot`.
5. **End.** The freeze ends with the tag; the next version starts with a new template.
