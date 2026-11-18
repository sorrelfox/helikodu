// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MIDI files (MID, MIDI, KAR, and RMI with a standard MIDI file inside) through
//! rustysynth and a SoundFont 2 file.
//!
//! Files are recognised by their `MThd` header or the RIFF `RMID` form ([`probe`]).
//! The SoundFont is the one [`SongOptions::soundfont`] names, otherwise one installed
//! on the system ([`system_soundfont`]); without either, opening fails with
//! [`SongError::NeedsSoundFont`]. SoundFonts are large, so the last one loaded is kept
//! for the next file. A file plays once, then for [`RELEASE`] longer so that the last
//! notes die away, fading out over that time. Seeking renders from the start to the
//! exact frame. The title and copyright come from the file's text events.

use std::fs::{self, File};
use std::io::BufReader;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use rustysynth::{MidiFile, MidiFileSequencer, SoundFont, Synthesizer, SynthesizerSettings};

use crate::song::text;
use crate::song::{Envelope, SongDecoder, SongError, SongInfo, SongOptions, SubsongInfo};

/// How long a file plays after its last event, fading out.
pub const RELEASE: Duration = Duration::from_millis(1500);

/// Frames rendered at a time.
const BLOCK: usize = 2048;

/// Where SoundFonts installed by distributions are looked for, best first.
const SYSTEM_SOUNDFONTS: [&str; 8] = [
    "/app/share/soundfonts/default.sf2",
    "/usr/share/soundfonts/default.sf2",
    "/usr/share/sounds/sf2/default-GM.sf2",
    "/usr/share/sounds/sf2/default.sf2",
    "/usr/share/soundfonts/FluidR3_GM.sf2",
    "/usr/share/sounds/sf2/FluidR3_GM.sf2",
    "/usr/share/soundfonts/TimGM6mb.sf2",
    "/usr/share/sounds/sf2/TimGM6mb.sf2",
];

/// Whether `header` is the start of a standard MIDI file or of an RMI file.
pub fn probe(header: &[u8]) -> bool {
    header.starts_with(b"MThd")
        || (header.len() >= 12 && header.starts_with(b"RIFF") && &header[8..12] == b"RMID")
}

/// A SoundFont installed on the system, if there is one.
pub fn system_soundfont() -> Option<PathBuf> {
    SYSTEM_SOUNDFONTS
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// The SoundFont MIDI files are played with under `options`: the chosen one, or one
/// installed on the system.
pub fn soundfont_for(options: &SongOptions) -> Option<PathBuf> {
    options.soundfont.clone().or_else(system_soundfont)
}

/// The SoundFont last loaded, kept while the file stays the same.
type Cached = (PathBuf, Option<SystemTime>, Arc<SoundFont>);
static SOUNDFONT: Mutex<Option<Cached>> = Mutex::new(None);

/// Loads the SoundFont at `path`, or takes the one kept from before.
pub fn load_soundfont(path: &Path) -> Result<Arc<SoundFont>, SongError> {
    let modified = fs::metadata(path)?.modified().ok();
    let mut cached = SOUNDFONT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((p, m, font)) = cached.as_ref()
        && p == path
        && *m == modified
    {
        return Ok(Arc::clone(font));
    }
    let mut reader = BufReader::new(File::open(path)?);
    let font = panic::catch_unwind(AssertUnwindSafe(|| SoundFont::new(&mut reader)))
        .map_err(|_| SongError::Unsupported("the SoundFont is damaged".into()))?
        .map_err(|e| SongError::Unsupported(format!("cannot read the SoundFont: {e}")))?;
    let font = Arc::new(font);
    *cached = Some((path.to_owned(), modified, Arc::clone(&font)));
    Ok(font)
}

/// The standard MIDI file in `data`: the file itself, or the `data` chunk of an RMI
/// file.
fn smf(data: &[u8]) -> Result<&[u8], SongError> {
    if data.starts_with(b"MThd") {
        return Ok(data);
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"RMID" {
        let mut at = 12;
        while at + 8 <= data.len() {
            let id = &data[at..at + 4];
            let size = u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]);
            let size = usize::try_from(size).unwrap_or(usize::MAX);
            let body = at + 8;
            let end = body.saturating_add(size).min(data.len());
            if id == b"data" {
                return Ok(&data[body..end]);
            }
            at = end + (size & 1);
        }
    }
    Err(SongError::Unsupported("not a standard MIDI file".into()))
}

/// The title and copyright of a standard MIDI file: the first track's name and the
/// first copyright notice (meta events 3 and 2).
fn text_events(smf: &[u8]) -> (Option<String>, Option<String>) {
    let mut title = None;
    let mut copyright = None;
    let mut at = 8 + read_u32(smf, 4).map_or(6, |len| len as usize);
    let mut track = 0;
    while at + 8 <= smf.len() && (title.is_none() || copyright.is_none()) {
        let Some(len) = read_u32(smf, at + 4) else {
            break;
        };
        let body = at + 8;
        let end = body.saturating_add(len as usize).min(smf.len());
        if &smf[at..at + 4] == b"MTrk" {
            scan_track(&smf[body..end], |kind, bytes| {
                let value = text(&decode_text(bytes));
                match kind {
                    3 if track == 0 && title.is_none() => title = value,
                    2 if copyright.is_none() => copyright = value,
                    _ => {}
                }
            });
            track += 1;
        }
        at = end;
    }
    (title, copyright)
}

/// MIDI text is mostly ASCII or Latin-1: UTF-8 when it is valid, Latin-1 otherwise.
fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// A variable-length quantity at `*at`.
fn read_vlq(data: &[u8], at: &mut usize) -> Option<usize> {
    let mut value = 0usize;
    for _ in 0..4 {
        let byte = *data.get(*at)?;
        *at += 1;
        value = (value << 7) | usize::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Calls `meta` with the type and data of every meta event of a track, until the
/// track ends or cannot be read further.
fn scan_track(track: &[u8], mut meta: impl FnMut(u8, &[u8])) {
    let mut at = 0;
    let mut running = 0u8;
    while at < track.len() {
        if read_vlq(track, &mut at).is_none() {
            return;
        }
        let Some(&first) = track.get(at) else {
            return;
        };
        let status = if first & 0x80 != 0 {
            at += 1;
            first
        } else {
            running
        };
        match status {
            0xff => {
                let Some(&kind) = track.get(at) else {
                    return;
                };
                at += 1;
                let Some(len) = read_vlq(track, &mut at) else {
                    return;
                };
                let end = at.saturating_add(len).min(track.len());
                meta(kind, &track[at..end]);
                if kind == 0x2f {
                    return;
                }
                at = end;
            }
            0xf0 | 0xf7 => {
                let Some(len) = read_vlq(track, &mut at) else {
                    return;
                };
                at = at.saturating_add(len);
            }
            0x80..=0xef => {
                running = status;
                at += if matches!(status & 0xf0, 0xc0 | 0xd0) {
                    1
                } else {
                    2
                };
            }
            // A data byte without a running status, or a system message: give up.
            _ => return,
        }
    }
}

/// A MIDI file rendered to stereo `f32` with a SoundFont.
pub struct MidiDecoder {
    midi: Arc<MidiFile>,
    font: Arc<SoundFont>,
    sequencer: MidiFileSequencer,
    info: SongInfo,
    rate: u32,
    envelope: Envelope,
    position: u64,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl std::fmt::Debug for MidiDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiDecoder")
            .field("info", &self.info)
            .field("rate", &self.rate)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

/// Parses the MIDI file in `data`: the file and what it says about itself, with its
/// length (the release included). No SoundFont is needed.
fn parse(data: &[u8]) -> Result<(MidiFile, SongInfo, Duration), SongError> {
    let smf = smf(data)?;
    let midi = panic::catch_unwind(AssertUnwindSafe(|| MidiFile::new(&mut &smf[..])))
        .map_err(|_| SongError::Unsupported("the MIDI file is damaged".into()))?
        .map_err(|e| SongError::Unsupported(format!("cannot read the MIDI file: {e}")))?;
    // The library panics on some damaged files here too (a file without tracks).
    let seconds = panic::catch_unwind(AssertUnwindSafe(|| midi.get_length()))
        .map_err(|_| SongError::Unsupported("the MIDI file is damaged".into()))?;
    let length = if seconds.is_finite() && seconds > 0.0 {
        Duration::from_secs_f64(seconds)
    } else {
        Duration::ZERO
    };
    let total = length + RELEASE;
    let (title, copyright) = text_events(smf);
    let info = SongInfo {
        format: "MIDI".to_owned(),
        title,
        artist: None,
        album: None,
        date: copyright,
        comment: None,
        subsongs: vec![SubsongInfo {
            index: 0,
            title: None,
            length: Some(total),
        }],
        default_subsong: 0,
    };
    Ok((midi, info, total))
}

/// What the MIDI file in `data` says about itself, without a SoundFont: for the
/// library scanner.
pub fn read_info(data: &[u8]) -> Result<SongInfo, SongError> {
    parse(data).map(|(_, info, _)| info)
}

impl MidiDecoder {
    /// Opens the MIDI file in `data`. It has one subsong.
    pub fn open(
        data: &[u8],
        subsong: Option<u32>,
        options: &SongOptions,
    ) -> Result<Self, SongError> {
        if let Some(index) = subsong.filter(|&s| s > 0) {
            return Err(SongError::NoSuchSubsong { index, count: 1 });
        }
        let (midi, info, total) = parse(data)?;
        let path = soundfont_for(options).ok_or(SongError::NeedsSoundFont)?;
        let font = load_soundfont(&path)?;
        let midi = Arc::new(midi);
        let envelope = Envelope::ending_at(options.frames(total), options.frames(RELEASE));
        let sequencer = Self::sequencer(&font, &midi, options.rate)?;
        Ok(Self {
            midi,
            font,
            sequencer,
            info,
            rate: options.rate,
            envelope,
            position: 0,
            left: Vec::new(),
            right: Vec::new(),
        })
    }

    fn sequencer(
        font: &Arc<SoundFont>,
        midi: &Arc<MidiFile>,
        rate: u32,
    ) -> Result<MidiFileSequencer, SongError> {
        let settings = SynthesizerSettings::new(i32::try_from(rate).unwrap_or(44_100));
        let synthesizer = Synthesizer::new(font, &settings)
            .map_err(|e| SongError::Unsupported(format!("cannot start the synthesizer: {e}")))?;
        let mut sequencer = MidiFileSequencer::new(synthesizer);
        sequencer.play(midi, false);
        Ok(sequencer)
    }

    fn render(&mut self, frames: usize) {
        self.left.resize(frames, 0.0);
        self.right.resize(frames, 0.0);
        self.sequencer.render(&mut self.left, &mut self.right);
    }
}

impl SongDecoder for MidiDecoder {
    fn info(&self) -> &SongInfo {
        &self.info
    }

    fn subsong(&self) -> u32 {
        0
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> u16 {
        2
    }

    fn frames(&self) -> Option<u64> {
        self.envelope.end()
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn read(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<usize, SongError> {
        out.clear();
        let frames = self.envelope.room(self.position, max_frames.min(BLOCK * 4));
        if frames == 0 {
            return Ok(0);
        }
        self.render(frames);
        out.reserve(frames * 2);
        for (l, r) in self.left.iter().zip(&self.right) {
            out.push(*l);
            out.push(*r);
        }
        self.envelope.apply(self.position, out, 2);
        self.position += frames as u64;
        Ok(frames)
    }

    fn seek(&mut self, frame: u64) -> Result<(), SongError> {
        let target = self.envelope.end().map_or(frame, |end| frame.min(end));
        if target < self.position {
            self.sequencer = Self::sequencer(&self.font, &self.midi, self.rate)?;
            self.position = 0;
        }
        while self.position < target {
            let frames = usize::try_from(target - self.position)
                .unwrap_or(BLOCK)
                .min(BLOCK);
            self.render(frames);
            self.position += frames as u64;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header with no tracks made the library panic while measuring the
    /// length; found by fuzzing.
    #[test]
    fn a_file_without_tracks_is_refused() {
        let data = [
            0x4d, 0x54, 0x68, 0x64, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x55, 0x60,
            0x4d, 0x54, 0x72, 0x6b, 0x00, 0x00, 0x00, 0x5a, 0x00, 0xff, 0x03, 0x11, 0x48, 0x65,
            0x6c, 0x69, 0x6b, 0x6f, 0x6d, 0x00, 0x20, 0x41, 0x72, 0x70, 0x65, 0x67, 0x67, 0x69,
            0x6f, 0x00, 0x5a, 0x00, 0xff, 0x03, 0x11, 0x48, 0x65, 0x6c, 0x69, 0x6b, 0x6f, 0x6d,
            0x00, 0x20, 0x41, 0x72, 0x70, 0x06, 0x00, 0x00, 0x00, 0x01, 0x00, 0x60, 0x4d, 0x54,
            0x72, 0x6b, 0x00, 0x00, 0x00, 0x5a, 0x64, 0x60, 0x90, 0x48, 0x00, 0x00, 0xff, 0x2f,
            0x00,
        ];
        assert!(read_info(&data).is_err());
    }

    #[test]
    fn reads_text_events() {
        // Header, then one track: a title, a copyright, a note with running status,
        // and the end of the track.
        let mut track = Vec::new();
        track.extend_from_slice(&[0x00, 0xff, 0x03, 0x04]);
        track.extend_from_slice(b"Tune");
        track.extend_from_slice(&[0x00, 0xff, 0x02, 0x05]);
        track.extend_from_slice(b"(c) X");
        track.extend_from_slice(&[0x00, 0x90, 60, 100, 0x10, 60, 0]);
        track.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
        let mut file = b"MThd\0\0\0\x06\0\0\0\x01\x00\x60MTrk".to_vec();
        file.extend_from_slice(&(track.len() as u32).to_be_bytes());
        file.extend_from_slice(&track);
        assert_eq!(
            text_events(&file),
            (Some("Tune".into()), Some("(c) X".into()))
        );
        let mut rmi = b"RIFF\0\0\0\0RMIDdata".to_vec();
        rmi.extend_from_slice(&(file.len() as u32).to_le_bytes());
        rmi.extend_from_slice(&file);
        assert!(probe(&rmi));
        assert_eq!(smf(&rmi).unwrap(), &file[..]);
    }

    #[test]
    fn damaged_tracks_end_the_scan() {
        let file = b"MThd\0\0\0\x06\0\0\0\x01\x00\x60MTrk\0\0\0\x03\x00\xff\x03";
        assert_eq!(text_events(file), (None, None));
        assert!(!probe(b"MTh"));
    }
}
