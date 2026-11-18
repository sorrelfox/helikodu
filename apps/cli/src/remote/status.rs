// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The status, the queue and the playlists as text.

use std::io::{self, Write};
use std::time::Duration;

use serde_json::Value;

use crate::offline::format;

fn time(value: &Value) -> Option<String> {
    value
        .as_u64()
        .map(|ms| format::length(Duration::from_millis(ms)))
}

/// "Title - Artist · Album" of a track.
fn describe(track: &Value) -> String {
    let title = track["title"].as_str().unwrap_or("?");
    let artist = track["artist"].as_str().filter(|a| !a.is_empty());
    let album = track["album"].as_str().filter(|a| !a.is_empty());
    match (artist, album) {
        (Some(artist), Some(album)) => format!("{title} - {artist} · {album}"),
        (Some(artist), None) => format!("{title} - {artist}"),
        (None, Some(album)) => format!("{title} · {album}"),
        (None, None) => title.to_owned(),
    }
}

pub(crate) fn print_status(out: &mut impl Write, status: &Value) -> io::Result<()> {
    let state = status["state"].as_str().unwrap_or("stopped");
    let label = match state {
        "playing" => "Playing",
        "paused" => "Paused",
        "loading" => "Loading",
        _ => "Stopped",
    };
    match status.get("track").filter(|t| !t.is_null()) {
        Some(track) => writeln!(out, "{label}: {}", describe(track))?,
        None => writeln!(out, "{label}")?,
    }
    if let Some(position) = time(&status["position_ms"]) {
        match time(&status["length_ms"]) {
            Some(length) => writeln!(out, "Position: {position} / {length}")?,
            None => writeln!(out, "Position: {position}")?,
        }
    }
    let volume = status["volume"].as_f64().unwrap_or(0.0);
    writeln!(out, "Volume: {:.0} %", volume * 100.0)?;
    writeln!(
        out,
        "Shuffle: {}, repeat: {}{}",
        status["shuffle"].as_str().unwrap_or("off"),
        status["repeat"].as_str().unwrap_or("off"),
        if status["stop_after"].as_bool() == Some(true) {
            ", stops after this track"
        } else {
            ""
        }
    )?;
    let length = status["queue_length"].as_u64().unwrap_or(0);
    match status["queue_position"].as_u64() {
        Some(position) => writeln!(out, "Queue: {} of {length}", position + 1)?,
        None => writeln!(out, "Queue: {length} tracks")?,
    }
    let output = &status["output"];
    if let Some(device) = output["device"].as_str().filter(|d| !d.is_empty()) {
        let mut line = format!(
            "Output: {device}, {}",
            output["signal"].as_str().unwrap_or("idle")
        );
        if let Some(rate) = output["sample_rate"].as_u64() {
            line.push_str(&format!(", {} Hz", rate));
        }
        if let Some(bits) = output["bits"].as_u64() {
            line.push_str(&format!(", {bits}-bit"));
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

pub(crate) fn print_queue(out: &mut impl Write, queue: &Value) -> io::Result<()> {
    let current = queue["current"].as_u64();
    let tracks = queue["tracks"].as_array().map_or(&[][..], Vec::as_slice);
    if let Some(context) = queue["context"].as_str() {
        writeln!(out, "From: {context}")?;
    }
    if tracks.is_empty() {
        writeln!(out, "The queue is empty.")?;
    }
    for (i, track) in tracks.iter().enumerate() {
        let mark = if current == Some(i as u64) { '>' } else { ' ' };
        let length = time(&track["length_ms"]).unwrap_or_default();
        writeln!(
            out,
            "{mark}{:>4}  {:>7}  {}",
            i + 1,
            length,
            describe(track)
        )?;
    }
    Ok(())
}

pub(crate) fn print_playlists(out: &mut impl Write, list: &Value) -> io::Result<()> {
    let playlists = list.as_array().map_or(&[][..], Vec::as_slice);
    if playlists.is_empty() {
        writeln!(out, "There are no playlists.")?;
    }
    for playlist in playlists {
        writeln!(
            out,
            "{:>6}  {:<6}  {:>5} tracks  {}",
            playlist["id"].as_u64().unwrap_or(0),
            playlist["kind"].as_str().unwrap_or(""),
            playlist["tracks"].as_u64().unwrap_or(0),
            playlist["name"].as_str().unwrap_or("")
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_status_reads_as_text() {
        let status = json!({
            "state": "playing",
            "track": {"title": "Shoreline", "artist": "Mari Kask", "album": "Tidewater Rooms"},
            "position_ms": 102_000,
            "length_ms": 295_000,
            "volume": 0.8,
            "shuffle": "off",
            "repeat": "all",
            "stop_after": false,
            "queue_length": 12,
            "queue_position": 2,
            "output": {"device": "Topping D10s", "signal": "bit-perfect", "sample_rate": 96000, "bits": 24},
        });
        let mut out = Vec::new();
        print_status(&mut out, &status).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text,
            "Playing: Shoreline - Mari Kask · Tidewater Rooms\n\
             Position: 1:42 / 4:55\n\
             Volume: 80 %\n\
             Shuffle: off, repeat: all\n\
             Queue: 3 of 12\n\
             Output: Topping D10s, bit-perfect, 96000 Hz, 24-bit\n"
        );
    }
}
