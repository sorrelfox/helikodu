// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask dist`: the release source tarball.
//!
//! Writes `helikodu-<version>.tar.xz` into `target/dist/` (or `--out DIR`): the
//! files Git tracks (plus untracked files that are not ignored, so a tarball can
//! be tried before a commit), the crates of `Cargo.lock` vendored into
//! `vendor/` with `cargo vendor`, and a `.cargo/config.toml` that builds from
//! them, so the tarball builds without network access. Beside it go
//! `helikodu-<version>.tar.xz.sha256` and `release-notes.md`, the changelog
//! section of the version (or of `[Unreleased]` when there is none yet).
//!
//! The archive is reproducible: entries sorted by name, owner and group 0, every
//! modification time set to `SOURCE_DATE_EPOCH` or else the time of the last
//! commit, and single-threaded xz. It needs GNU tar and xz.
//!
//! `--tag vX.Y.Z` is for releases: the tag must be `v` followed by the
//! workspace version, the working tree must be clean, and both the changelog
//! and the AppStream metainfo must have an entry for the version.

use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};

const METAINFO: &str = "data/io.github.sorrelfox.Helikodu.metainfo.xml";

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut tag = None;
    let mut out = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tag" => tag = Some(args.next().context("`--tag` needs a value")?.clone()),
            "--out" => {
                out = Some(PathBuf::from(args.next().context("`--out` needs a value")?));
            }
            other => bail!("unknown option `{other}` for `dist`"),
        }
    }

    let root = crate::repository_root()?;
    let version = workspace_version(&read(&root.join("Cargo.toml"))?)?;
    let changelog = read(&root.join("CHANGELOG.md"))?;
    let notes = match &tag {
        Some(tag) => {
            check_tag(tag, &version)?;
            ensure!(
                git_output(&root, &["status", "--porcelain"])?
                    .trim()
                    .is_empty(),
                "the working tree has changes; a release is made from a clean checkout"
            );
            ensure!(
                has_metainfo_release(&read(&root.join(METAINFO))?, &version),
                "{METAINFO} has no <release version=\"{version}\"> entry"
            );
            changelog_section(&changelog, &version)
                .with_context(|| format!("CHANGELOG.md has no `## [{version}]` section"))?
        }
        None => changelog_section(&changelog, &version)
            .or_else(|| changelog_section(&changelog, "Unreleased"))
            .unwrap_or_default(),
    };

    let out = out.unwrap_or_else(|| root.join("target").join("dist"));
    let name = format!("helikodu-{version}");
    let staging_parent = out.join("staging");
    let staging = staging_parent.join(&name);
    if staging_parent.exists() {
        fs::remove_dir_all(&staging_parent)
            .with_context(|| format!("could not remove {}", staging_parent.display()))?;
    }
    fs::create_dir_all(&staging)
        .with_context(|| format!("could not create {}", staging.display()))?;

    println!("==> copying the source files");
    let files = source_files(&root)?;
    for file in &files {
        copy_entry(&root.join(file), &staging.join(file))?;
    }
    println!("    {} files", files.len());

    println!("==> vendoring the crates");
    let vendor_config = vendor(&staging)?;
    let config_path = staging.join(".cargo").join("config.toml");
    let mut config = if config_path.exists() {
        read(&config_path)?
    } else {
        fs::create_dir_all(staging.join(".cargo"))?;
        String::new()
    };
    if !config.is_empty() && !config.ends_with('\n') {
        config.push('\n');
    }
    config.push_str(
        "\n# Added by `cargo xtask dist`: build from the vendored crates in vendor/,\n\
         # without network access.\n",
    );
    config.push_str(vendor_config.trim_start());
    fs::write(&config_path, config)
        .with_context(|| format!("could not write {}", config_path.display()))?;

    let tarball = out.join(format!("{name}.tar.xz"));
    println!("==> writing {}", tarball.display());
    let epoch = source_date_epoch(&root);
    write_tarball(&staging_parent, &name, epoch, &tarball)?;
    fs::remove_dir_all(&staging_parent)
        .with_context(|| format!("could not remove {}", staging_parent.display()))?;

    let checksum = crate::corpus::manifest::sha256_file(&tarball)?;
    let checksum_path = out.join(format!("{name}.tar.xz.sha256"));
    fs::write(&checksum_path, format!("{checksum}  {name}.tar.xz\n"))
        .with_context(|| format!("could not write {}", checksum_path.display()))?;
    let notes_path = out.join("release-notes.md");
    fs::write(&notes_path, notes)
        .with_context(|| format!("could not write {}", notes_path.display()))?;

    println!("==> {checksum}  {name}.tar.xz");
    println!("    release notes: {}", notes_path.display());
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))
}

/// `[workspace.package] version` of the root `Cargo.toml`: the one place the
/// version is set.
fn workspace_version(cargo_toml: &str) -> Result<String> {
    let manifest: toml::Table =
        toml::from_str(cargo_toml).context("Cargo.toml is not valid TOML")?;
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(String::from)
        .context("Cargo.toml has no [workspace.package] version")
}

fn check_tag(tag: &str, version: &str) -> Result<()> {
    ensure!(
        tag.strip_prefix('v') == Some(version),
        "the tag `{tag}` does not match the workspace version {version} (expected `v{version}`)"
    );
    Ok(())
}

/// The body of the `## [<heading>]` section of a Keep a Changelog file, without
/// its heading, or `None` if there is no such section.
fn changelog_section(changelog: &str, heading: &str) -> Option<String> {
    let start = format!("## [{heading}]");
    let mut lines = changelog.lines();
    lines.by_ref().find(|line| line.starts_with(&start))?;
    let body: Vec<&str> = lines
        .take_while(|line| !line.starts_with("## ") && !line.starts_with("[Unreleased]:"))
        .collect();
    let body = body.join("\n");
    Some(format!("{}\n", body.trim()))
}

fn has_metainfo_release(metainfo: &str, version: &str) -> bool {
    metainfo.contains(&format!("<release version=\"{version}\""))
}

/// The files to ship, relative to the repository root, sorted.
fn source_files(root: &Path) -> Result<Vec<PathBuf>> {
    let listing = git_output(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let mut files: Vec<PathBuf> = listing
        .split('\0')
        .filter(|path| !path.is_empty() && shipped(path))
        .map(PathBuf::from)
        // A deleted file is still listed until the deletion is committed.
        .filter(|path| root.join(path).symlink_metadata().is_ok())
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Whether a path Git lists belongs in the tarball: build folders never do,
/// even where they are not ignored.
fn shipped(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let build_dir = |part: &&str| matches!(*part, "target" | "_build" | "builddir");
    !(parts.first().is_some_and(build_dir)
        || (parts.first() == Some(&"spikes") && parts.get(2).is_some_and(build_dir)))
}

fn copy_entry(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let metadata = from
        .symlink_metadata()
        .with_context(|| format!("could not read {}", from.display()))?;
    if metadata.is_symlink() {
        let target = fs::read_link(from)?;
        std::os::unix::fs::symlink(&target, to)
            .with_context(|| format!("could not link {}", to.display()))?;
    } else {
        fs::copy(from, to).with_context(|| format!("could not copy {}", from.display()))?;
    }
    Ok(())
}

/// Runs `cargo vendor` in the staged tree and returns the configuration it
/// prints for building from the vendored crates.
fn vendor(staging: &Path) -> Result<String> {
    let output = Command::new(crate::cargo())
        .args(["vendor", "--locked", "--versioned-dirs", "vendor"])
        .current_dir(staging)
        .stderr(Stdio::inherit())
        .output()
        .context("could not start `cargo vendor`")?;
    ensure!(output.status.success(), "`cargo vendor` failed");
    let config = String::from_utf8(output.stdout).context("`cargo vendor` printed non-UTF-8")?;
    ensure!(
        config.contains("[source.vendored-sources]"),
        "`cargo vendor` printed no source configuration"
    );
    Ok(config)
}

/// The modification time of every entry: `SOURCE_DATE_EPOCH`, else the time of
/// the last commit, else 0.
fn source_date_epoch(root: &Path) -> u64 {
    env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .or_else(|| {
            git_output(root, &["log", "-1", "--format=%ct"])
                .ok()
                .and_then(|value| value.trim().parse().ok())
        })
        .unwrap_or(0)
}

fn write_tarball(parent: &Path, name: &str, epoch: u64, tarball: &Path) -> Result<()> {
    let output =
        File::create(tarball).with_context(|| format!("could not create {}", tarball.display()))?;
    let mut tar = Command::new("tar")
        .args([
            "--create",
            "--file=-",
            "--sort=name",
            "--format=posix",
            "--pax-option=exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime",
            &format!("--mtime=@{epoch}"),
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "--mode=u+rw,go+r,go-w",
            "--directory",
        ])
        .arg(parent)
        .arg(name)
        .stdout(Stdio::piped())
        .spawn()
        .context("could not start `tar` (GNU tar is needed)")?;
    let tar_stdout = tar.stdout.take().context("no output from `tar`")?;
    let xz_status = Command::new("xz")
        .args(["--compress", "--stdout", "-6", "--threads=1"])
        .stdin(tar_stdout)
        .stdout(output)
        .status()
        .context("could not start `xz`")?;
    let tar_status = tar.wait().context("`tar` did not finish")?;
    ensure!(tar_status.success(), "`tar` failed");
    ensure!(xz_status.success(), "`xz` failed");
    Ok(())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .context("could not start `git`")?;
    ensure!(
        output.status.success(),
        "`git {}` failed (the tarball is made from a Git checkout)",
        args.join(" ")
    );
    String::from_utf8(output.stdout).context("`git` printed non-UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_read_from_the_workspace() {
        let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.0\"\n";
        assert_eq!(workspace_version(manifest).unwrap(), "0.1.0");
        assert!(workspace_version("[workspace]\n").is_err());
    }

    #[test]
    fn the_tag_must_match_the_version() {
        assert!(check_tag("v0.1.0", "0.1.0").is_ok());
        assert!(check_tag("0.1.0", "0.1.0").is_err());
        assert!(check_tag("v0.1.1", "0.1.0").is_err());
        assert!(check_tag("v0.1.0-rc1", "0.1.0").is_err());
    }

    #[test]
    fn a_changelog_section_ends_at_the_next_heading() {
        let changelog = "# Changelog\n\nIntro.\n\n## [Unreleased]\n\n### Added\n- New.\n\n\
                         ## [0.1.0] - 2026-12-01\n\n### Added\n- First.\n\n\
                         [Unreleased]: https://example.org/compare\n";
        assert_eq!(
            changelog_section(changelog, "Unreleased").unwrap(),
            "### Added\n- New.\n"
        );
        assert_eq!(
            changelog_section(changelog, "0.1.0").unwrap(),
            "### Added\n- First.\n"
        );
        assert!(changelog_section(changelog, "0.2.0").is_none());
        // `[0.1.0]` is not a prefix match for `[0.1.0-rc1]` and the like.
        assert!(changelog_section(changelog, "0.1").is_none());
    }

    #[test]
    fn the_metainfo_release_is_found() {
        let metainfo =
            "<releases>\n  <release version=\"0.1.0\" date=\"2026-12-01\"/>\n</releases>";
        assert!(has_metainfo_release(metainfo, "0.1.0"));
        assert!(!has_metainfo_release(metainfo, "0.1"));
        assert!(!has_metainfo_release("<releases/>", "0.1.0"));
    }

    #[test]
    fn build_folders_are_not_shipped() {
        assert!(shipped("crates/engine/src/lib.rs"));
        assert!(shipped("spikes/decoding/src/main.rs"));
        assert!(shipped("crates/target/README.md"));
        assert!(!shipped("target/debug/helikodu"));
        assert!(!shipped("spikes/decoding/target/debug/x"));
        assert!(!shipped("_build/build.ninja"));
    }
}
