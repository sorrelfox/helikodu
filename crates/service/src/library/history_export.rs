// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing the listening history to a local file, in a format that listening
//! services can import later. Nothing is sent anywhere.
//!
//! Two formats, chosen by the file's extension:
//!
//! - **ListenBrainz JSON Lines** (any extension but `.log`): one listen per line,
//!   each an object in the listen format of the ListenBrainz API
//!   (`listened_at`, and `track_metadata` with `artist_name`, `track_name`,
//!   `release_name` and `additional_info`). A submission of the `import` type
//!   wraps up to 1000 of these lines in its `payload`. Only listens that
//!   counted as plays are written.
//! - **Audioscrobbler portable log** (`.log`, as `.scrobbler.log`): the
//!   tab-separated format of version 1.1, with the times in UTC. Plays are
//!   rated `L`, skips `S`; Last.fm tools submit only the `L` lines.
//!
//! Both list the oldest listen first. Listens of tracks that left the library,
//! and of tracks without an artist or a title, are left out.

use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{HistoryEntry, HistoryQuery, LibraryError, MusicLibrary};

/// The formats the history can be written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryFormat {
    /// ListenBrainz listens, one JSON object per line.
    ListenBrainz,
    /// The Audioscrobbler portable player log, version 1.1.
    ScrobblerLog,
}

impl HistoryFormat {
    /// The format the extension of `path` names: `.log` is the scrobbler log,
    /// anything else ListenBrainz JSON Lines.
    pub fn for_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some(ext) if ext.eq_ignore_ascii_case("log") => Self::ScrobblerLog,
            _ => Self::ListenBrainz,
        }
    }
}

/// One listen with the names a listening service needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedListen {
    /// When playback started.
    pub started_at: SystemTime,
    /// Whether it counted as a play.
    pub counted: bool,
    /// The artists as one name.
    pub artist: String,
    /// The title.
    pub title: String,
    /// The album, if known.
    pub album: Option<String>,
    /// The track number, if known.
    pub track_number: Option<u32>,
    /// The track's length, if known.
    pub length: Option<Duration>,
}

impl MusicLibrary {
    /// Writes the listens since `since` (all of them with `None`) to `path` in
    /// the format its extension names; returns how many listens were written.
    pub fn export_history(
        &self,
        path: &Path,
        since: Option<SystemTime>,
    ) -> Result<usize, LibraryError> {
        let entries = self.history(&HistoryQuery {
            since,
            limit: usize::MAX,
            ..HistoryQuery::default()
        })?;
        let listens: Vec<ExportedListen> = entries
            .iter()
            .rev()
            .filter_map(|entry| self.exported(entry))
            .collect();
        let io_error = |source| LibraryError::Io {
            context: format!("cannot write {}", path.display()),
            source,
        };
        let mut file = io::BufWriter::new(std::fs::File::create(path).map_err(io_error)?);
        let written = write_history(HistoryFormat::for_path(path), &listens, &mut file)
            .and_then(|n| file.flush().map(|()| n))
            .map_err(io_error)?;
        Ok(written)
    }

    fn exported(&self, entry: &HistoryEntry) -> Option<ExportedListen> {
        let details = self.track_details(entry.track?).ok().flatten()?;
        let tags = details.tags;
        let artist = tags.artists.join(", ");
        let title = tags.title.filter(|t| !t.trim().is_empty())?;
        if artist.trim().is_empty() {
            return None;
        }
        Some(ExportedListen {
            started_at: entry.started_at,
            counted: entry.counted,
            artist,
            title,
            album: tags.album.filter(|a| !a.trim().is_empty()),
            track_number: tags.track_no,
            length: details.audio.duration,
        })
    }
}

/// Writes `listens` (oldest first) to `out` in `format`; returns how many were
/// written (ListenBrainz takes plays only).
pub fn write_history(
    format: HistoryFormat,
    listens: &[ExportedListen],
    out: &mut impl Write,
) -> io::Result<usize> {
    match format {
        HistoryFormat::ListenBrainz => write_listenbrainz(listens, out),
        HistoryFormat::ScrobblerLog => write_scrobbler_log(listens, out),
    }
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn write_listenbrainz(listens: &[ExportedListen], out: &mut impl Write) -> io::Result<usize> {
    let mut written = 0;
    for listen in listens.iter().filter(|l| l.counted) {
        let mut info = serde_json::Map::new();
        info.insert("media_player".into(), "Helikodu".into());
        info.insert("submission_client".into(), "Helikodu".into());
        info.insert(
            "submission_client_version".into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        if let Some(length) = listen.length {
            let millis = u64::try_from(length.as_millis()).unwrap_or(u64::MAX);
            info.insert("duration_ms".into(), millis.into());
        }
        if let Some(number) = listen.track_number {
            info.insert("tracknumber".into(), number.into());
        }
        let mut metadata = serde_json::Map::new();
        metadata.insert("artist_name".into(), listen.artist.clone().into());
        metadata.insert("track_name".into(), listen.title.clone().into());
        if let Some(album) = &listen.album {
            metadata.insert("release_name".into(), album.clone().into());
        }
        metadata.insert("additional_info".into(), info.into());
        let line = serde_json::json!({
            "listened_at": unix_seconds(listen.started_at),
            "track_metadata": metadata,
        });
        serde_json::to_writer(&mut *out, &line)?;
        out.write_all(b"\n")?;
        written += 1;
    }
    Ok(written)
}

/// A field of the scrobbler log: tabs and line breaks would end it.
fn field(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn write_scrobbler_log(listens: &[ExportedListen], out: &mut impl Write) -> io::Result<usize> {
    writeln!(out, "#AUDIOSCROBBLER/1.1")?;
    writeln!(out, "#TZ/UTC")?;
    writeln!(out, "#CLIENT/Helikodu {}", env!("CARGO_PKG_VERSION"))?;
    for listen in listens {
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t",
            field(&listen.artist),
            field(listen.album.as_deref().unwrap_or("")),
            field(&listen.title),
            listen
                .track_number
                .map(|n| n.to_string())
                .unwrap_or_default(),
            listen.length.map_or(0, |l| l.as_secs()),
            if listen.counted { 'L' } else { 'S' },
            unix_seconds(listen.started_at),
        )?;
    }
    Ok(listens.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listen(seconds: u64, counted: bool) -> ExportedListen {
        ExportedListen {
            started_at: UNIX_EPOCH + Duration::from_secs(seconds),
            counted,
            artist: "Mari Kask".into(),
            title: format!("Glass\tHarbour {seconds}"),
            album: Some("Tidewater Rooms".into()),
            track_number: Some(3),
            length: Some(Duration::from_millis(312_400)),
        }
    }

    #[test]
    fn the_extension_chooses_the_format() {
        assert_eq!(
            HistoryFormat::for_path(Path::new("/tmp/.scrobbler.log")),
            HistoryFormat::ScrobblerLog
        );
        assert_eq!(
            HistoryFormat::for_path(Path::new("/tmp/listens.jsonl")),
            HistoryFormat::ListenBrainz
        );
        assert_eq!(
            HistoryFormat::for_path(Path::new("/tmp/history")),
            HistoryFormat::ListenBrainz
        );
    }

    #[test]
    fn listenbrainz_lines_hold_plays_only() {
        let mut out = Vec::new();
        let listens = [listen(1_700_000_000, true), listen(1_700_000_400, false)];
        let written = write_history(HistoryFormat::ListenBrainz, &listens, &mut out).unwrap();
        assert_eq!(written, 1);
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1);
        let value: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(value["listened_at"], 1_700_000_000);
        let meta = &value["track_metadata"];
        assert_eq!(meta["artist_name"], "Mari Kask");
        assert_eq!(meta["track_name"], "Glass\tHarbour 1700000000");
        assert_eq!(meta["release_name"], "Tidewater Rooms");
        assert_eq!(meta["additional_info"]["duration_ms"], 312_400);
        assert_eq!(meta["additional_info"]["tracknumber"], 3);
        assert_eq!(meta["additional_info"]["media_player"], "Helikodu");
    }

    #[test]
    fn the_scrobbler_log_rates_plays_and_skips() {
        let mut out = Vec::new();
        let mut unknown = listen(1_700_000_400, false);
        unknown.album = None;
        unknown.track_number = None;
        unknown.length = None;
        let listens = [listen(1_700_000_000, true), unknown];
        let written = write_history(HistoryFormat::ScrobblerLog, &listens, &mut out).unwrap();
        assert_eq!(written, 2);
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "#AUDIOSCROBBLER/1.1");
        assert_eq!(lines[1], "#TZ/UTC");
        assert!(lines[2].starts_with("#CLIENT/Helikodu "));
        assert_eq!(
            lines[3],
            "Mari Kask\tTidewater Rooms\tGlass Harbour 1700000000\t3\t312\tL\t1700000000\t"
        );
        assert_eq!(
            lines[4],
            "Mari Kask\t\tGlass Harbour 1700000400\t\t0\tS\t1700000400\t"
        );
    }
}
