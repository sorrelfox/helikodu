// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Device reservation probe: takes `org.freedesktop.ReserveDevice1.Audio<N>`
//! from its current owner (WirePlumber on a PipeWire desktop) the way JACK
//! does, checks that the ALSA device becomes free, then gives the name back.
//!
//! ```text
//! reserve --card N --device hw:CARD=...,DEV=... [--hold-ms MS] [--priority P]
//! ```
//!
//! It opens and closes the device only; it never writes audio.

use std::time::{Duration, Instant};

use alsa::Direction;
use alsa::pcm::PCM;
use anyhow::{Result, anyhow};
use spike_output::run;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::names::BusName;
use zbus::{Connection, OwnedVariant, interface};

/// Our side of the protocol: while we own the name, others may ask us to
/// release it. A player in exclusive mode says no while it is playing.
struct Reservation {
    priority: i32,
    t0: Instant,
}

#[interface(name = "org.freedesktop.ReserveDevice1")]
impl Reservation {
    async fn request_release(&self, priority: i32) -> bool {
        println!(
            "{:8.3}s RequestRelease({priority}) received; answering false",
            self.t0.elapsed().as_secs_f64()
        );
        false
    }

    #[zbus(property)]
    async fn priority(&self) -> i32 {
        self.priority
    }

    #[zbus(property)]
    async fn application_name(&self) -> String {
        "helikodu-output-spike".into()
    }

    #[zbus(property)]
    async fn application_device_name(&self) -> String {
        String::new()
    }
}

fn card_nodes(card_pci_hint: &str) -> String {
    let sinks = run("pactl", &["list", "short", "sinks"]);
    let sources = run("pactl", &["list", "short", "sources"]);
    let found: Vec<&str> = sinks
        .lines()
        .chain(sources.lines())
        .filter(|l| l.contains(card_pci_hint))
        .filter_map(|l| l.split('\t').nth(1))
        .collect();
    if found.is_empty() {
        "none".into()
    } else {
        found.join(", ")
    }
}

fn try_open(device: &str, within: Duration) -> String {
    let t = Instant::now();
    let mut attempts = 0;
    loop {
        attempts += 1;
        match PCM::new(device, Direction::Playback, true) {
            Ok(pcm) => {
                drop(pcm);
                return format!("opened after {:?} ({attempts} attempts)", t.elapsed());
            }
            Err(e) if t.elapsed() >= within => {
                return format!("still failing after {:?}: {e}", t.elapsed());
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut card: Option<u32> = None;
    let mut device: Option<String> = None;
    let mut hold_ms = 500;
    let mut priority = i32::MAX;
    let mut hint = String::new();
    while let Some(k) = args.next() {
        let mut val = || args.next().ok_or_else(|| anyhow!("{k} needs a value"));
        match k.as_str() {
            "--card" => card = Some(val()?.parse()?),
            "--device" => device = Some(val()?),
            "--hold-ms" => hold_ms = val()?.parse()?,
            "--priority" => priority = val()?.parse()?,
            // A substring of the card's PipeWire node names, to watch them.
            "--node-hint" => hint = val()?,
            other => return Err(anyhow!("unknown argument {other}")),
        }
    }
    let card = card.ok_or_else(|| anyhow!("--card"))?;
    let device = device.ok_or_else(|| anyhow!("--device"))?;
    let name = format!("org.freedesktop.ReserveDevice1.Audio{card}");
    let path = format!("/org/freedesktop/ReserveDevice1/Audio{card}");
    let t0 = Instant::now();
    let ts = || format!("{:8.3}s", t0.elapsed().as_secs_f64());

    let before = card_nodes(&hint);
    if !hint.is_empty() {
        println!("{} PipeWire nodes of the card before: {before}", ts());
    }
    println!(
        "{} {device} before: {}",
        ts(),
        try_open(&device, Duration::ZERO)
    );

    let conn = zbus::block_on(async {
        let conn = Connection::session().await?;
        let dbus = DBusProxy::new(&conn).await?;
        let bus_name = BusName::try_from(name.as_str())?;
        match dbus.get_name_owner(bus_name.clone()).await {
            Ok(owner) => {
                let pid = dbus
                    .get_connection_unix_process_id(BusName::from(owner.clone()))
                    .await
                    .unwrap_or(0);
                let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
                let get = |prop: &'static str| {
                    let conn = conn.clone();
                    let owner = owner.clone();
                    let path = path.clone();
                    async move {
                        conn.call_method(
                            Some(owner.as_str()),
                            path.as_str(),
                            Some("org.freedesktop.DBus.Properties"),
                            "Get",
                            &("org.freedesktop.ReserveDevice1", prop),
                        )
                        .await
                        .ok()
                        .and_then(|m| m.body().deserialize::<OwnedVariant>().ok())
                        .map(|v| format!("{v:?}"))
                        .unwrap_or_else(|| "?".into())
                    }
                };
                println!(
                    "{} {name} owned by {owner} (pid {pid}, {}); Priority={} ApplicationName={} ApplicationDeviceName={}",
                    ts(),
                    comm.trim(),
                    get("Priority").await,
                    get("ApplicationName").await,
                    get("ApplicationDeviceName").await
                );
            }
            Err(e) => println!("{} {name} has no owner ({e})", ts()),
        }

        // Serve our object before taking the name, as the protocol expects.
        conn.object_server()
            .at(path.as_str(), Reservation { priority, t0 })
            .await?;

        // 1. Plain request: fails while someone owns the name.
        // zbus reports "Exists" as an error, not as a reply value.
        let r = conn
            .request_name_with_flags(name.as_str(), RequestNameFlags::DO_NOT_QUEUE)
            .await;
        println!("{} RequestName(DO_NOT_QUEUE) -> {r:?}", ts());
        if !matches!(r, Ok(RequestNameReply::PrimaryOwner)) {
            // 2. Ask the owner to release, then replace it.
            let t = Instant::now();
            let reply = conn
                .call_method(
                    Some(name.as_str()),
                    path.as_str(),
                    Some("org.freedesktop.ReserveDevice1"),
                    "RequestRelease",
                    &(priority,),
                )
                .await;
            let granted = match reply {
                Ok(m) => m.body().deserialize::<bool>().unwrap_or(false),
                Err(e) => {
                    println!("{} RequestRelease call failed: {e}", ts());
                    false
                }
            };
            println!(
                "{} RequestRelease({priority}) -> {granted} in {:?}",
                ts(),
                t.elapsed()
            );
            if !granted {
                return Err(anyhow!("owner refused to release"));
            }
            let r = conn
                .request_name_with_flags(
                    name.as_str(),
                    RequestNameFlags::DO_NOT_QUEUE | RequestNameFlags::REPLACE_EXISTING,
                )
                .await?;
            println!("{} RequestName(REPLACE_EXISTING) -> {r:?}", ts());
            if r != RequestNameReply::PrimaryOwner {
                return Err(anyhow!("could not take the name"));
            }
        }
        Ok::<_, anyhow::Error>(conn)
    })?;

    // We own the reservation. The device should be free now, or soon.
    println!(
        "{} {device} while reserved: {}",
        ts(),
        try_open(&device, Duration::from_millis(3000))
    );
    if !hint.is_empty() {
        println!(
            "{} PipeWire nodes of the card while reserved: {}",
            ts(),
            card_nodes(&hint)
        );
    }
    std::thread::sleep(Duration::from_millis(hold_ms));

    let t = Instant::now();
    zbus::block_on(conn.release_name(name.as_str()))?;
    println!("{} released {name}", ts());
    // Wait for the previous owner to take it back.
    let dbus = zbus::block_on(DBusProxy::new(&conn))?;
    let bus_name = BusName::try_from(name.as_str())?;
    loop {
        if let Ok(owner) = zbus::block_on(dbus.get_name_owner(bus_name.clone())) {
            println!(
                "{} {name} owned again by {owner} after {:?}",
                ts(),
                t.elapsed()
            );
            break;
        }
        if t.elapsed() > Duration::from_secs(5) {
            println!("{} nobody took {name} back within 5 s", ts());
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(conn);
    if !hint.is_empty() {
        // Wait until WirePlumber has restored every node of the card.
        let t = Instant::now();
        loop {
            let nodes = card_nodes(&hint);
            if nodes == before || t.elapsed() > Duration::from_secs(3) {
                println!(
                    "{} PipeWire nodes of the card after {:?}: {nodes}",
                    ts(),
                    t.elapsed()
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}
