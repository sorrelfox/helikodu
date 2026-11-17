// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The launcher runs its own probes under the filter: Internet sockets must
//! kill the program with `SIGSYS`, `AF_UNIX` sockets must keep working.
//!
//! These tests need no namespace, so they run wherever the test suite runs,
//! including inside the offline audit, where the filters stack.

use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Output};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_helikodu-offline-launcher");

fn launch(args: &[&str]) -> Output {
    Command::new(LAUNCHER).args(args).output().unwrap()
}

fn probe_under_filter(kind: &str) -> Output {
    launch(&["--", LAUNCHER, "--probe", kind])
}

fn describe(status: ExitStatus, output: &Output) -> String {
    format!(
        "{status:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Whether this test already runs under a seccomp filter (the offline audit).
fn already_filtered() -> bool {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Seccomp:"))
        .is_some_and(|mode| mode.trim() != "0")
}

#[test]
fn an_af_inet_socket_kills_the_program() {
    let output = probe_under_filter("inet");
    assert_eq!(
        output.status.signal(),
        Some(libc::SIGSYS),
        "{}",
        describe(output.status, &output)
    );
}

#[test]
fn an_af_inet6_socket_kills_the_program() {
    let output = probe_under_filter("inet6");
    assert_eq!(
        output.status.signal(),
        Some(libc::SIGSYS),
        "{}",
        describe(output.status, &output)
    );
}

#[test]
fn af_unix_sockets_still_work() {
    let output = probe_under_filter("unix");
    assert!(
        output.status.success(),
        "{}",
        describe(output.status, &output)
    );
}

#[test]
fn without_the_filter_the_internet_probes_survive() {
    // The control: the probes themselves do not die for some other reason.
    if already_filtered() {
        eprintln!("skipped: this test already runs under a seccomp filter");
        return;
    }
    for kind in ["inet", "inet6"] {
        let output = launch(&["--probe", kind]);
        assert!(
            output.status.success(),
            "{}",
            describe(output.status, &output)
        );
    }
}

#[test]
fn the_program_keeps_its_arguments_and_exit_code() {
    let output = launch(&[
        "--",
        "sh",
        "-c",
        "echo \"$0 $1\"; exit 3",
        "first",
        "second",
    ]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "first second\n");
}

#[test]
fn a_missing_program_is_reported() {
    let output = launch(&["/nonexistent/program"]);
    assert_eq!(output.status.code(), Some(127));
}

#[test]
fn no_program_is_an_error() {
    assert_eq!(launch(&[]).status.code(), Some(125));
    assert_eq!(launch(&["--"]).status.code(), Some(125));
}
