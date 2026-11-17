// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Device reservation against the real session bus and sound server.
//!
//! Ignored by default: it needs a session bus, a sound server that owns its cards
//! through `org.freedesktop.ReserveDevice1` (WirePlumber), and a sound card that
//! may be taken away from the desktop for a moment. Taking a card removes all of
//! its PipeWire nodes (outputs and inputs) until it is given back, so pick one
//! that nothing is playing to or recording from. Only digital silence is written,
//! for less than 200 ms.
//!
//! ```sh
//! HELIKODU_RESERVE_TEST_DEVICE='hw:CARD=Generic_1,DEV=1' \
//!     cargo test -p helikodu-desktop --test reservation -- --ignored --nocapture
//! ```

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use ::alsa::pcm::{Access, Format, HwParams, PCM};
use ::alsa::{Direction, ValueOr};
use helikodu_desktop::reservation::{
    DeviceReserver, ReserveError, ReservedDevice, bus_name, card_index,
};

/// Opens `device` non-blocking, trying again while its previous holder closes
/// it, as the engine's ALSA backend does.
fn open_when_free(device: &str) -> PCM {
    let start = Instant::now();
    loop {
        match PCM::new(device, Direction::Playback, true) {
            Ok(pcm) => return pcm,
            Err(error) if error.errno() == 16 && start.elapsed() < Duration::from_secs(1) => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("cannot open {device}: {error}"),
        }
    }
}

/// Writes `ms` milliseconds of digital silence, 16-bit stereo at 48 kHz.
fn play_silence(pcm: &PCM, ms: usize) {
    {
        let hw = HwParams::any(pcm).unwrap();
        hw.set_rate_resample(false).unwrap();
        hw.set_access(Access::RWInterleaved).unwrap();
        hw.set_format(Format::S16LE).unwrap();
        hw.set_channels(2).unwrap();
        hw.set_rate(48_000, ValueOr::Nearest).unwrap();
        pcm.hw_params(&hw).unwrap();
    }
    let silence = vec![0i16; 48 * ms * 2];
    let io = pcm.io_i16().unwrap();
    let mut rest = &silence[..];
    while !rest.is_empty() {
        match io.writei(rest) {
            Ok(frames) => rest = &rest[frames * 2..],
            Err(error) if error.errno() == 11 => {
                let _ = pcm.wait(Some(50));
            }
            Err(error) => panic!("writing silence: {error}"),
        }
    }
    let _ = pcm.drop();
}

/// The unique name that owns `name` on the session bus, if any.
fn owner(name: &str) -> Option<String> {
    zbus::block_on(async {
        let connection = zbus::Connection::session().await.ok()?;
        let reply = connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(name,),
            )
            .await
            .ok()?;
        reply.body().deserialize::<String>().ok()
    })
}

/// The names of the sound server's sinks and sources, from `pactl`; `None` if it
/// cannot be run.
fn nodes() -> Option<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for kind in ["sinks", "sources"] {
        let output = Command::new("pactl")
            .args(["list", "short", kind])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        names.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|l| l.split('\t').nth(1).map(str::to_owned)),
        );
    }
    Some(names)
}

/// Waits up to `timeout` for `done`, returning how long it took.
fn wait_for(timeout: Duration, mut done: impl FnMut() -> bool) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if done() {
            return Some(start.elapsed());
        }
        thread::sleep(Duration::from_millis(5));
    }
    None
}

#[test]
#[ignore = "needs a session bus, WirePlumber and a sound card it may lose for a moment"]
fn takes_a_card_from_the_sound_server_plays_silence_and_gives_it_back() {
    let Ok(device) = std::env::var("HELIKODU_RESERVE_TEST_DEVICE") else {
        println!("skipped: set HELIKODU_RESERVE_TEST_DEVICE to a hw: device to take");
        return;
    };
    let card_id = device
        .split_once("CARD=")
        .map(|(_, rest)| rest.split(',').next().unwrap_or(rest))
        .unwrap();
    let card = card_index(card_id).unwrap();
    let name = bus_name(card);
    let server = owner(&name).expect("the sound server owns the card's reservation");
    let nodes_before = nodes();
    println!("{name} owned by {server}; device {device}");

    // 1. A second program asking for a held card is refused, by name.
    let held: ReservedDevice = DeviceReserver::new().acquire(card, &device).unwrap();
    let ours = owner(&name).unwrap();
    assert_ne!(ours, server);
    let refused = DeviceReserver::new().acquire(card, &device).unwrap_err();
    assert!(
        matches!(&refused, ReserveError::Refused { holder: Some(h) } if h == "Helikodu"),
        "{refused}"
    );
    assert!(held.release_requested());
    held.release().unwrap();
    let back = wait_for(Duration::from_secs(5), || {
        owner(&name).as_deref() == Some(&server)
    });
    println!("first round: the sound server owned the card again after {back:?}");
    assert!(back.is_some());
    nodes_are_back(nodes_before.as_ref());

    // 2. Take the card again, open hw: directly and play silence. The engine's
    //    ALSA backend does the same through its injected reservation.
    let held = DeviceReserver::new().acquire(card, &device).unwrap();
    let t = Instant::now();
    let pcm = open_when_free(&device);
    println!("opened {device} {:?} after taking the card", t.elapsed());
    if let Some(nodes) = nodes() {
        let gone: Vec<_> = nodes_before
            .iter()
            .flatten()
            .filter(|n| !nodes.contains(*n))
            .collect();
        println!("PipeWire nodes gone while the card is ours: {gone:?}");
    }
    // Digital silence only, 150 ms of it.
    play_silence(&pcm, 150);
    drop(pcm);
    println!(
        "the sound server asked for the card back while it was ours: {}",
        held.release_requested()
    );
    drop(held);

    // 3. The sound server takes the card and its nodes back.
    let back = wait_for(Duration::from_secs(5), || {
        owner(&name).as_deref() == Some(&server)
    });
    println!("the sound server owned the card again after {back:?}");
    assert!(back.is_some());
    nodes_are_back(nodes_before.as_ref());
}

/// Waits until the sound server's nodes are the ones of `before`, under the same
/// names.
fn nodes_are_back(before: Option<&BTreeSet<String>>) {
    let Some(before) = before else {
        println!("pactl is not available; the nodes were not compared");
        return;
    };
    let restored = wait_for(Duration::from_secs(5), || nodes().as_ref() == Some(before));
    println!("all PipeWire nodes back under their names after {restored:?}");
    assert!(
        restored.is_some(),
        "nodes before: {before:?}, now: {:?}",
        nodes()
    );
}
