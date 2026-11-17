// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask offline-audit`: every test program runs without a network.
//!
//! Each test program of the workspace runs
//! - inside a new network namespace with no interfaces but a loopback device
//!   that is down, made without privileges by `unshare --user --net`, and
//! - under the seccomp filter of `helikodu-offline-launcher`
//!   (`tests/system/offline-launcher/`), which kills the program with `SIGSYS`
//!   as soon as it calls `socket()` or `socketpair()` with the domain
//!   `AF_INET` or `AF_INET6`. `AF_UNIX` (D-Bus, PipeWire) and `AF_NETLINK`
//!   sockets keep working.
//!
//! A test program that fails or is killed fails the audit, which names it.
//!
//! Before the test programs run, a self-test proves that the harness works:
//! the namespace has only the loopback device, an Internet socket probe
//! survives without the filter and is killed under it, and `AF_UNIX` sockets
//! still work under it. `--self-test` runs only that.
//!
//! The filter is installed by a separate program because installing it from
//! here would need a `pre_exec` hook, which is `unsafe`; the launcher uses the
//! safe `seccompiler::apply_filter` and then replaces itself with the test
//! program.

use std::fmt::Write as _;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::Value;

const USAGE: &str = "\
usage: cargo xtask offline-audit [--self-test] [-- <test argument>...]

  --self-test   only check that the harness catches Internet sockets
  -- ...        arguments for every test program, such as --test-threads=1
";

const LAUNCHER_PACKAGE: &str = "helikodu-offline-launcher";

/// `SIGSYS` on every architecture the filter supports (x86-64, AArch64, RISC-V).
const SIGSYS: i32 = 31;

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut self_test_only = false;
    let mut test_args: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "--self-test" => self_test_only = true,
            "--" => {
                test_args = &args[index + 1..];
                break;
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown option `{other}` for `offline-audit`\n\n{USAGE}"),
        }
    }

    let root = crate::repository_root()?;
    let namespace = Namespace::detect().map_err(|reason| {
        anyhow::anyhow!(
            "cannot create an unprivileged network namespace: {reason}\n\
             The offline audit needs `unshare` from util-linux and unprivileged user \
             namespaces; see tests/system/README.md"
        )
    })?;

    println!("==> offline audit: building {LAUNCHER_PACKAGE}");
    let launcher = build_launcher(&root)?;

    self_test(&namespace, &launcher)?;
    if self_test_only {
        println!("==> offline audit: the self-test passed");
        return Ok(());
    }

    println!("==> offline audit: building the test programs");
    let programs = build_test_programs(&root)?;
    if programs.is_empty() {
        bail!("cargo built no test programs");
    }

    let mut failures = Vec::new();
    for program in &programs {
        println!("==> offline audit: {}", program.name);
        let status = namespace
            .command(&launcher)
            .arg(&program.executable)
            .args(test_args)
            .current_dir(&program.package_dir)
            .env("CARGO_MANIFEST_DIR", &program.package_dir)
            .env("CARGO_PKG_NAME", &program.package)
            .status()
            .with_context(|| format!("could not start `unshare` for {}", program.name))?;
        if !status.success() {
            failures.push(format!(
                "{}: {}\n    {}",
                program.name,
                explain(status),
                program.executable.display()
            ));
        }
    }

    if failures.is_empty() {
        println!(
            "==> offline audit: {} test programs passed without a network",
            programs.len()
        );
        Ok(())
    } else {
        let mut message = format!(
            "{} of {} test programs failed the offline audit:",
            failures.len(),
            programs.len()
        );
        for failure in &failures {
            let _ = write!(message, "\n  {failure}");
        }
        bail!(message)
    }
}

/// Whether this machine can run the audit; the reason if it cannot.
pub(crate) fn availability() -> Result<(), String> {
    Namespace::detect().map(|_| ())
}

/// How `unshare` is called to make the namespace.
#[derive(Debug)]
struct Namespace {
    /// The option that maps the user into the new user namespace.
    map_option: &'static str,
}

impl Namespace {
    /// Finds an `unshare` option that works here. `--map-current-user` keeps
    /// the caller's user ID, so file permissions behave as in a normal test
    /// run; older util-linux releases only have `--map-root-user`.
    fn detect() -> Result<Self, String> {
        let mut reason = String::new();
        for map_option in ["--map-current-user", "--map-root-user"] {
            let output = Command::new("unshare")
                .args(["--user", "--net", map_option, "--", "true"])
                .stdin(Stdio::null())
                .output();
            match output {
                Ok(output) if output.status.success() => {
                    if map_option != "--map-current-user" {
                        println!(
                            "==> offline audit: notice: `unshare --map-current-user` is not \
                             available; the tests run as root inside the user namespace"
                        );
                    }
                    return Ok(Self { map_option });
                }
                Ok(output) => {
                    reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                }
                Err(error) => return Err(format!("could not run `unshare`: {error}")),
            }
        }
        Err(reason)
    }

    /// `unshare ... -- <launcher> --`: append the program and its arguments.
    fn command(&self, launcher: &Path) -> Command {
        let mut command = self.unfiltered(launcher);
        command.arg("--");
        command
    }

    /// `unshare ... -- <program>`, without the filter.
    fn unfiltered(&self, program: &Path) -> Command {
        let mut command = Command::new("unshare");
        command
            .args(["--user", "--net", self.map_option, "--"])
            .arg(program);
        command
    }
}

/// The checks that prove the harness catches what it should.
fn self_test(namespace: &Namespace, launcher: &Path) -> Result<()> {
    println!("==> offline audit: self-test");
    let probe = |filtered: bool, kind: &str| -> Result<Output> {
        let mut command = if filtered {
            let mut command = namespace.command(launcher);
            command.arg(launcher);
            command
        } else {
            namespace.unfiltered(launcher)
        };
        command
            .args(["--probe", kind])
            .stdin(Stdio::null())
            .output()
            .context("could not start `unshare`")
    };
    let checks: [(&str, bool, &str, Expect); 5] = [
        (
            "the namespace has no interface but loopback",
            false,
            "interfaces",
            Expect::Success,
        ),
        (
            "without the filter, an AF_INET socket call survives",
            false,
            "inet",
            Expect::Success,
        ),
        (
            "under the filter, an AF_INET socket kills the program",
            true,
            "inet",
            Expect::Sigsys,
        ),
        (
            "under the filter, an AF_INET6 socket kills the program",
            true,
            "inet6",
            Expect::Sigsys,
        ),
        (
            "under the filter, AF_UNIX sockets still work",
            true,
            "unix",
            Expect::Success,
        ),
    ];
    for (title, filtered, kind, expect) in checks {
        let output = probe(filtered, kind)?;
        let passed = match expect {
            Expect::Success => output.status.success(),
            Expect::Sigsys => output.status.signal() == Some(SIGSYS),
        };
        if !passed {
            bail!(
                "self-test failed: {title}: expected {}, got {}\nstdout: {}\nstderr: {}",
                expect.describe(),
                explain(output.status),
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        println!("    ok: {title}");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Expect {
    Success,
    Sigsys,
}

impl Expect {
    fn describe(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Sigsys => "death by SIGSYS",
        }
    }
}

/// Explains how a test program ended.
fn explain(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (_, Some(SIGSYS)) => String::from(
            "killed by SIGSYS: it tried to create an AF_INET or AF_INET6 socket \
             (or made a system call for another architecture)",
        ),
        (_, Some(signal)) => format!("killed by signal {signal}"),
        (Some(125), None) => String::from("the launcher failed (exit code 125)"),
        (Some(code), None) => format!("failed with exit code {code}"),
        (None, None) => format!("ended with {status}"),
    }
}

/// A test program built by `cargo test --no-run`.
#[derive(Debug)]
struct TestProgram {
    /// For messages: package, target kind and target name.
    name: String,
    package: String,
    package_dir: PathBuf,
    executable: PathBuf,
}

fn build_launcher(root: &Path) -> Result<PathBuf> {
    let artifacts = cargo_json(
        root,
        &["build", "--package", LAUNCHER_PACKAGE, "--bins", "--locked"],
    )?;
    artifacts
        .iter()
        .filter(|artifact| artifact["target"]["name"].as_str() == Some(LAUNCHER_PACKAGE))
        .find_map(|artifact| artifact["executable"].as_str())
        .map(PathBuf::from)
        .with_context(|| format!("cargo did not report the {LAUNCHER_PACKAGE} executable"))
}

fn build_test_programs(root: &Path) -> Result<Vec<TestProgram>> {
    let artifacts = cargo_json(
        root,
        &[
            "test",
            "--workspace",
            "--all-features",
            "--locked",
            "--no-run",
        ],
    )?;
    let mut programs = Vec::new();
    for artifact in &artifacts {
        let Some(executable) = artifact["executable"].as_str() else {
            continue;
        };
        if artifact["profile"]["test"].as_bool() != Some(true) {
            // Binaries built only so that integration tests can run them.
            continue;
        }
        let manifest = Path::new(
            artifact["manifest_path"]
                .as_str()
                .context("a test artifact has no manifest path")?,
        );
        let package_dir = manifest
            .parent()
            .context("a manifest has no folder")?
            .to_path_buf();
        let package = package_name(artifact).unwrap_or_else(|| package_dir.display().to_string());
        let target = artifact["target"]["name"].as_str().unwrap_or("?");
        let kind = artifact["target"]["kind"]
            .as_array()
            .and_then(|kinds| kinds.first())
            .and_then(Value::as_str)
            .unwrap_or("?");
        programs.push(TestProgram {
            name: format!("{package} {kind} `{target}`"),
            package,
            package_dir,
            executable: PathBuf::from(executable),
        });
    }
    programs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(programs)
}

/// The package name from a package ID: `path+file:///…/name#version` or
/// `registry+…#name@version`.
fn package_name(artifact: &Value) -> Option<String> {
    let id = artifact["package_id"].as_str()?;
    let (location, fragment) = id.rsplit_once('#')?;
    match fragment.split_once('@') {
        Some((name, _)) => Some(name.to_owned()),
        None => location
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
    }
}

/// Runs Cargo with `--message-format=json-render-diagnostics` and returns the
/// `compiler-artifact` messages. Diagnostics are shown as usual.
fn cargo_json(root: &Path, args: &[&str]) -> Result<Vec<Value>> {
    let output = Command::new(crate::cargo())
        .args(args)
        .arg("--message-format=json-render-diagnostics")
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("could not run `cargo {}`", args.join(" ")))?;
    if !output.status.success() {
        bail!("`cargo {}` failed", args.join(" "));
    }
    let mut artifacts = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message["reason"].as_str() == Some("compiler-artifact") {
            artifacts.push(message);
        }
    }
    Ok(artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_come_from_package_ids() {
        let path = serde_json::json!({
            "package_id": "path+file:///src/crates/model#helikodu-model@0.0.0"
        });
        assert_eq!(package_name(&path).as_deref(), Some("helikodu-model"));
        let short = serde_json::json!({ "package_id": "path+file:///src/xtask#0.0.0" });
        assert_eq!(package_name(&short).as_deref(), Some("xtask"));
    }

    #[test]
    fn sigsys_is_explained() {
        let status = ExitStatus::from_raw(SIGSYS);
        assert!(explain(status).contains("AF_INET"));
        let status = ExitStatus::from_raw(3 << 8);
        assert_eq!(explain(status), "failed with exit code 3");
    }
}
