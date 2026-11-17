// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `--probe <kind>`: one socket call, for the audit's self-test.
//!
//! The `inet` and `inet6` probes exit successfully whenever the process
//! survives the call, whether the socket was created or refused by the
//! kernel; under the filter they must not survive. The `unix` probe fails
//! unless both calls succeed. The `interfaces` probe fails unless `lo` is the
//! only network interface, which is what a fresh network namespace has.

use std::fs;
use std::net::UdpSocket;
use std::os::unix::net::UnixDatagram;
use std::process::ExitCode;

pub(crate) fn run(kind: &str) -> ExitCode {
    match kind {
        "inet" => internet("127.0.0.1:0"),
        "inet6" => internet("[::1]:0"),
        "unix" => unix(),
        "interfaces" => interfaces(),
        other => {
            eprintln!(
                "helikodu-offline-launcher: unknown probe `{other}` (inet, inet6, unix, interfaces)"
            );
            ExitCode::from(2)
        }
    }
}

/// Creates an Internet socket: `socket()` comes before the `bind()`.
fn internet(address: &str) -> ExitCode {
    match UdpSocket::bind(address) {
        Ok(socket) => println!("probe: created an Internet socket {socket:?}"),
        Err(error) => println!("probe: survived the Internet socket call ({error})"),
    }
    ExitCode::SUCCESS
}

/// Creates an unbound `AF_UNIX` socket and a connected pair, and sends one
/// datagram through the pair.
fn unix() -> ExitCode {
    let result = UnixDatagram::unbound().and_then(|_| {
        let (left, right) = UnixDatagram::pair()?;
        left.send(b"ok")?;
        let mut buffer = [0; 2];
        right.recv(&mut buffer)?;
        Ok(buffer)
    });
    match result {
        Ok(buffer) if &buffer == b"ok" => {
            println!("probe: AF_UNIX sockets work");
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!("probe: an AF_UNIX socket pair lost data");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("probe: an AF_UNIX socket call failed: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Lists the interfaces of this process's network namespace.
fn interfaces() -> ExitCode {
    let table = match fs::read_to_string("/proc/self/net/dev") {
        Ok(table) => table,
        Err(error) => {
            eprintln!("probe: could not read /proc/self/net/dev: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Two header lines, then one `name: counters...` line per interface.
    let names: Vec<&str> = table
        .lines()
        .skip(2)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name.trim())
        .collect();
    println!("probe: network interfaces: {}", names.join(", "));
    if names.iter().all(|name| *name == "lo") {
        ExitCode::SUCCESS
    } else {
        eprintln!("probe: interfaces other than `lo` exist");
        ExitCode::FAILURE
    }
}
