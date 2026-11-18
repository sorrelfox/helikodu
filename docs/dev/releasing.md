# Releasing

Helikodu is published as GitHub Releases. Pushing a version tag (`v0.1.0`) starts the [Release workflow](../../.github/workflows/release.yml), which publishes:

| File | What it is |
|---|---|
| `helikodu-<version>.tar.xz` | The source tarball, made by `cargo xtask dist`: the files of the tagged commit, every crate of `Cargo.lock` vendored into `vendor/`, and a `.cargo/config.toml` that builds from them, so it builds without network access |
| `helikodu-<version>-x86_64-linux.tar.xz` | The programs, built from that source tarball and installed by Meson for the prefix `/usr/local`: `bin/`, `libexec/helikodu/`, the desktop entry, the AppStream metainfo, the D-Bus service file, the icons and the translations, with an `INSTALL.txt` note |
| `SHA256SUMS` | The SHA-256 checksums of both tarballs, for `sha256sum --check` |

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
2. **Translations.** The template and the `.po` files are up to date: `meson compile -C _build helikodu-pot helikodu-update-po`, and the result committed.
3. **Version bump** in `Cargo.toml` and `Cargo.lock` (see above).
4. **Checks.** `cargo xtask ci` passes on a clean checkout (a fresh `git clone`, not the working copy). The extended checks (the "Extended checks" workflow, started by hand in the Actions tab) and the CI workflow (also started by hand) are green for the commit to be tagged.
5. **Manual QA** sign-off on the commit to be tagged.
6. **Dry run.** Start the Release workflow by hand on the branch (Actions → Release → Run workflow). It builds everything and keeps the files as a workflow artifact, `release-assets`, without publishing. Download it, then unpack the binary tarball and start the programs on a desktop. Locally, `cargo xtask dist` makes the source tarball in `target/dist/`; check that it builds offline:

   ```sh
   mkdir /tmp/helikodu-dist && tar -xJf target/dist/helikodu-0.1.0.tar.xz -C /tmp/helikodu-dist
   cd /tmp/helikodu-dist/helikodu-0.1.0
   meson setup _build --buildtype=release -Doffline=true && meson compile -C _build
   ```

7. **Signed tag.** Tag the release commit on `main` and push the tag:

   ```sh
   git tag -s v0.1.0 -m "Helikodu 0.1.0"
   git push origin v0.1.0
   ```

   The push starts the Release workflow (and the CI workflow). It runs the build and tests (`cargo xtask ci --tests-only`), makes the source tarball, builds the programs from it offline, and creates the GitHub Release. The build runs with at most four Cargo jobs inside a container capped at 12 GB of memory, the runner's limit; see [ci/README.md](../../ci/README.md#memory-limit).
8. **Check the release page**: the notes, the pre-release mark and the three files. Download the files and run `sha256sum --check SHA256SUMS`.

If the workflow fails, fix the cause on `main`, delete the tag (`git push --delete origin v0.1.0`, `git tag -d v0.1.0`) and tag again. A published release is never replaced: a mistake in it is fixed by a new patch version.

## How the files are built

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

The tarball holds the files Git tracks, plus untracked files that are not ignored (so it can be tried before a commit; with `--tag` the tree must be clean). Build folders (`target/`, `spikes/*/target/`, `_build/`) are never included. `release-notes.md` is the version's changelog section, or the `[Unreleased]` section while there is none. The command needs Git, GNU tar and xz, and the crates of `Cargo.lock` in Cargo's cache or a network connection for `cargo vendor`.
