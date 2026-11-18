// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A private session bus for one test, and a recorder of the signals on it.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use futures_util::StreamExt;
use zbus::message::Type;
use zbus::{Connection, MatchRule, Message, MessageStream};

/// A `dbus-daemon` of its own, ended when dropped.
pub(crate) struct PrivateBus {
    daemon: Child,
    pub(crate) address: String,
}

impl PrivateBus {
    /// Starts one, or returns `None` (and says why) when `dbus-daemon` cannot
    /// be run here.
    pub(crate) fn start() -> Option<Self> {
        Self::spawn(Command::new("dbus-daemon").args(["--session"]))
    }

    /// Starts one on which connections may own only the name `allowed`, as a
    /// sandbox's bus allows only the names it was granted.
    pub(crate) fn start_owning_only(allowed: &str, config_dir: &std::path::Path) -> Option<Self> {
        let config = config_dir.join("bus.conf");
        let text = format!(
            "<busconfig>\
             <type>session</type>\
             <listen>unix:tmpdir={}</listen>\
             <auth>EXTERNAL</auth>\
             <policy context=\"default\">\
             <allow send_destination=\"*\" eavesdrop=\"true\"/>\
             <allow eavesdrop=\"true\"/>\
             <allow own=\"{allowed}\"/>\
             </policy>\
             </busconfig>\n",
            config_dir.display()
        );
        if let Err(error) = std::fs::write(&config, text) {
            eprintln!("skipped: the bus configuration cannot be written: {error}");
            return None;
        }
        Self::spawn(Command::new("dbus-daemon").arg(format!("--config-file={}", config.display())))
    }

    fn spawn(command: &mut Command) -> Option<Self> {
        let daemon = command
            .args(["--nofork", "--nopidfile", "--print-address"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut daemon = match daemon {
            Ok(daemon) => daemon,
            Err(error) => {
                eprintln!("skipped: dbus-daemon cannot be started: {error}");
                return None;
            }
        };
        let stdout = daemon.stdout.take()?;
        let mut address = String::new();
        if BufReader::new(stdout).read_line(&mut address).is_err() || address.trim().is_empty() {
            eprintln!("skipped: dbus-daemon printed no address");
            let _ = daemon.kill();
            let _ = daemon.wait();
            return None;
        }
        Some(Self {
            daemon,
            address: address.trim().to_owned(),
        })
    }

    /// A new connection to the bus.
    pub(crate) fn connect(&self) -> Connection {
        zbus::block_on(zbus::connection::Builder::address(self.address.as_str()).build())
            .expect("connect to the private bus")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// The signals of one interface on one path, as they arrive.
pub(crate) struct Signals {
    receiver: Receiver<Message>,
}

impl Signals {
    /// Starts recording; every signal sent after this returns is recorded.
    pub(crate) fn record(connection: &Connection, path: &str) -> Self {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .path(path.to_owned())
            .build()
            .expect("valid match rule");
        let stream = zbus::block_on(MessageStream::for_match_rule(rule, connection, None))
            .expect("subscribe to signals");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            zbus::block_on(async move {
                let mut stream = std::pin::pin!(stream);
                while let Some(Ok(message)) = stream.next().await {
                    if sender.send(message).is_err() {
                        break;
                    }
                }
            });
        });
        Self { receiver }
    }

    /// The next signal named `member`, skipping others; `None` after `timeout`.
    pub(crate) fn next(&self, member: &str, timeout: Duration) -> Option<Message> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let message = self.receiver.recv_timeout(left).ok()?;
            if message
                .header()
                .member()
                .is_some_and(|m| m.as_str() == member)
            {
                return Some(message);
            }
        }
    }

    /// Every signal that arrives within `wait`.
    pub(crate) fn drain(&self, wait: Duration) -> Vec<Message> {
        let mut messages = Vec::new();
        while let Ok(message) = self.receiver.recv_timeout(wait) {
            messages.push(message);
        }
        messages
    }
}
