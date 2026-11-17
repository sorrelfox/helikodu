// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The worker program over its pipes: the greeting, answers, a clean exit when the
//! input closes, and the address-space limit.

#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use helikodu_meta::worker::{
    FailureKind, Message, PROTOCOL_VERSION, Request, RequestOptions, Response, read_frame,
    write_frame,
};

struct Worker {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
}

fn start(args: &[&str], faults: bool) -> Worker {
    let mut command = Command::new(env!("CARGO_BIN_EXE_helikodu-scan-worker"));
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if faults {
        command.env("HELIKODU_SCAN_WORKER_FAULTS", "1");
    }
    let mut child = command.spawn().unwrap();
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let mut worker = Worker {
        child,
        input,
        output,
    };
    match worker.receive() {
        Some(Message::Hello(hello)) => assert_eq!(hello.version, PROTOCOL_VERSION),
        other => panic!("no greeting: {other:?}"),
    }
    worker
}

impl Worker {
    fn send(&mut self, id: u64, path: impl Into<PathBuf>) {
        let request = Request {
            id,
            path: path.into(),
            options: RequestOptions::default(),
        };
        write_frame(&mut self.input, &Message::Request(request).encode()).unwrap();
        self.input.flush().unwrap();
    }

    fn receive(&mut self) -> Option<Message> {
        read_frame(&mut self.output)
            .ok()
            .flatten()
            .map(|frame| Message::decode(&frame).unwrap())
    }

    fn response(&mut self) -> Response {
        match self.receive() {
            Some(Message::Response(response)) => *response,
            other => panic!("no response: {other:?}"),
        }
    }
}

#[test]
fn answers_and_exits_when_the_input_closes() {
    let mut worker = start(&[], false);
    worker.send(7, "/nonexistent/helikodu/\u{e9}.flac");
    let response = worker.response();
    assert_eq!(response.id, 7);
    assert_eq!(response.result.unwrap_err().kind, FailureKind::Io);

    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scan-worker-noise.mp3");
    std::fs::write(&tmp, [0x33_u8; 2048]).unwrap();
    worker.send(8, &tmp);
    let response = worker.response();
    assert_eq!(response.id, 8);
    assert_eq!(response.result.unwrap_err().kind, FailureKind::Unreadable);

    drop(worker.input);
    let status = worker.child.wait().unwrap();
    assert!(status.success(), "{status}");
}

#[test]
fn garbage_on_the_input_ends_the_worker() {
    let mut worker = start(&["--memory-limit", "0"], false);
    write_frame(&mut worker.input, b"\x07nonsense").unwrap();
    worker.input.flush().unwrap();
    assert!(worker.receive().is_none());
    assert!(!worker.child.wait().unwrap().success());
}

#[test]
fn bad_arguments_are_refused() {
    let status = Command::new(env!("CARGO_BIN_EXE_helikodu-scan-worker"))
        .arg("--bogus")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}

#[test]
#[cfg(debug_assertions)]
fn the_address_space_limit_stops_a_runaway_allocation() {
    let mut worker = start(&["--memory-limit", "268435456"], true);
    worker.send(1, "/nowhere/hog-worker.flac");
    assert!(worker.receive().is_none(), "the worker should have died");
    assert!(!worker.child.wait().unwrap().success());

    // Without a limit the same request would succeed in allocating; with the default
    // limit (1 GiB) it fails as well.
    let mut worker = start(&[], true);
    worker.send(1, "/nowhere/hog-worker.flac");
    assert!(worker.receive().is_none(), "the worker should have died");
}
