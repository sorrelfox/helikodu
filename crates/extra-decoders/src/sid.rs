// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Commodore 64 SID tunes (PSID and RSID) through libsidplayfp and its reSIDfp
//! emulation.
//!
//! Tunes are recognised by their `PSID` or `RSID` signature ([`probe`]). Each song of
//! a tune is a subsong; with no subsong chosen the tune's start song plays. A song
//! plays for the length the High Voltage SID Collection's song-length database gives
//! (when [`SongOptions::sid_song_lengths`] names it), otherwise for
//! [`SongOptions::default_length`] followed by a fade of [`SongOptions::fade`]. With
//! [`SongOptions::sid_roms`] naming a folder of C64 ROM images (files whose names
//! start with `kernal`, `basic` and `chargen` or `characters`), those are installed;
//! without them libsidplayfp's built-in replacement KERNAL serves, which plays most
//! PSID tunes. Seeking starts the song over in a new emulator and renders to the
//! exact frame. The emulation itself is not bit-for-bit repeatable (libsidplayfp
//! seeds a random generator from the clock), so the samples after a seek match a
//! straight run to within a few steps of the 16-bit output, not exactly.

use std::ffi::{CStr, CString, c_char, c_uint};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use helikodu_sidplayfp_sys as ffi;

use crate::song::text;
use crate::song::{Envelope, SongDecoder, SongError, SongInfo, SongOptions, SubsongInfo};

/// Frames rendered at a time.
const BLOCK: usize = 2048;

/// Whether `header` starts with the PSID or RSID signature.
pub fn probe(header: &[u8]) -> bool {
    header.len() >= 0x76 && (header.starts_with(b"PSID") || header.starts_with(b"RSID"))
}

/// Whether the PSID or RSID header of `data` is one libsidplayfp can be given:
/// the data offset its version prescribes (0x76 for version 1, 0x7C later), and
/// tune data after it (with the two-byte load address when the header has none).
/// libsidplayfp reads past its buffer on some headers that break these rules
/// (found by fuzzing), so they are refused first.
fn header_is_sane(data: &[u8]) -> bool {
    if !probe(data) {
        return false;
    }
    let word = |at: usize| u16::from_be_bytes([data[at], data[at + 1]]);
    let offset = match (word(4), word(6)) {
        (1, 0x76) => 0x76,
        (2..=4, 0x7C) if data.len() >= 0x7C => 0x7C,
        _ => return false,
    };
    let load_in_data = word(8) == 0;
    data.len() > offset + if load_in_data { 2 } else { 0 }
}

/// The MD5 of the tune in `data` as the HVSC song-length database lists it (32
/// lower-case hex digits), or `None` when it is not a tune libsidplayfp reads.
pub fn tune_md5(data: &[u8]) -> Option<String> {
    if !header_is_sane(data) {
        return None;
    }
    let options = SongOptions::default();
    let length = u32::try_from(data.len()).ok()?;
    let mut error = [0 as c_char; 256];
    // SAFETY: the data and the error buffer are valid for their lengths.
    let raw = unsafe {
        ffi::hk_sid_open(
            data.as_ptr(),
            length,
            options.rate,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    let tune = Tune(NonNull::new(raw)?);
    let mut md5 = [0 as c_char; ffi::MD5_LENGTH + 1];
    // SAFETY: the tune is open; the buffer holds the digits and a NUL.
    if unsafe { ffi::hk_sid_md5(tune.0.as_ptr(), md5.as_mut_ptr()) } == 0 {
        return None;
    }
    Some(message(&md5))
}

/// A tune with its emulator.
struct Tune(NonNull<ffi::hk_sid>);

// SAFETY: the shim's objects have no thread affinity; a tune is used from one thread
// at a time, through `&mut`.
unsafe impl Send for Tune {}

impl Drop for Tune {
    fn drop(&mut self) {
        // SAFETY: the tune was created by hk_sid_open and is freed once.
        unsafe { ffi::hk_sid_free(self.0.as_ptr()) };
    }
}

impl std::fmt::Debug for Tune {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tune")
    }
}

impl Tune {
    fn select(&mut self, song: u32) -> Result<(), SongError> {
        let mut error = [0 as c_char; 256];
        // SAFETY: the tune is open; the buffer is valid for its length.
        let ok = unsafe {
            ffi::hk_sid_select(
                self.0.as_ptr(),
                song as c_uint,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(SongError::Render(message(&error)));
        }
        Ok(())
    }

    fn info_string(&self, index: u32) -> Option<String> {
        // SAFETY: the tune is open; the string lives as long as the tune.
        let value = unsafe { ffi::hk_sid_info_string(self.0.as_ptr(), index) };
        if value.is_null() {
            return None;
        }
        // SAFETY: the shim returns NUL-terminated strings.
        text(&unsafe { CStr::from_ptr(value) }.to_string_lossy())
    }
}

/// The message the shim wrote into `buffer`.
fn message(buffer: &[c_char]) -> String {
    let bytes: Vec<u8> = buffer
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if text.is_empty() {
        "libsidplayfp failed".to_owned()
    } else {
        text
    }
}

/// An opened song-length database.
struct Database(NonNull<ffi::hk_sid_db>);

// SAFETY: the database is only reached under the mutex of `DATABASE`.
unsafe impl Send for Database {}

impl Drop for Database {
    fn drop(&mut self) {
        // SAFETY: the database was opened by hk_sid_db_open and is freed once.
        unsafe { ffi::hk_sid_db_free(self.0.as_ptr()) };
    }
}

/// The song-length database last opened, kept for the next tune: reading the HVSC's
/// takes a while.
type CachedDatabase = (PathBuf, Option<std::time::SystemTime>, Arc<Mutex<Database>>);
static DATABASE: Mutex<Option<CachedDatabase>> = Mutex::new(None);

/// The database at `path`, opened once and kept while the file stays the same.
fn database(path: &Path) -> Result<Arc<Mutex<Database>>, SongError> {
    let modified = fs::metadata(path)?.modified().ok();
    let mut cached = DATABASE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((p, m, db)) = cached.as_ref()
        && p == path
        && *m == modified
    {
        return Ok(Arc::clone(db));
    }
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| SongError::Unsupported("the song-length database path holds a NUL".into()))?;
    let mut error = [0 as c_char; 256];
    // SAFETY: the path is NUL-terminated; the buffer is valid for its length.
    let raw = unsafe { ffi::hk_sid_db_open(c_path.as_ptr(), error.as_mut_ptr(), error.len()) };
    let db = NonNull::new(raw).ok_or_else(|| {
        SongError::Unsupported(format!("song-length database: {}", message(&error)))
    })?;
    let db = Arc::new(Mutex::new(Database(db)));
    *cached = Some((path.to_owned(), modified, Arc::clone(&db)));
    Ok(db)
}

/// The ROM images in `folder`: KERNAL, BASIC and character ROM, each when found with
/// the right size.
fn roms(folder: &Path) -> [Option<Vec<u8>>; 3] {
    let mut found: [Option<Vec<u8>>; 3] = [None, None, None];
    let Ok(entries) = fs::read_dir(folder) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let (slot, size) = if name.starts_with("kernal") {
            (0, ffi::ROM_8K)
        } else if name.starts_with("basic") {
            (1, ffi::ROM_8K)
        } else if name.starts_with("chargen") || name.starts_with("characters") {
            (2, ffi::CHARGEN_LEN)
        } else {
            continue;
        };
        if found[slot].is_some() {
            continue;
        }
        if let Ok(bytes) = fs::read(entry.path())
            && bytes.len() == size
        {
            found[slot] = Some(bytes);
        }
    }
    found
}

/// A song of a SID tune, rendered to stereo `f32`.
#[derive(Debug)]
pub struct SidDecoder {
    tune: Tune,
    /// The file, for starting over.
    data: Vec<u8>,
    roms: Option<[Option<Vec<u8>>; 3]>,
    info: SongInfo,
    /// The song, counted from 0.
    song: u32,
    rate: u32,
    envelope: Envelope,
    position: u64,
    samples: Vec<i16>,
}

impl SidDecoder {
    /// Opens song `subsong` (counted from 0; `None` for the tune's start song) of the
    /// tune in `data`.
    pub fn open(
        data: &[u8],
        subsong: Option<u32>,
        options: &SongOptions,
    ) -> Result<Self, SongError> {
        let roms = options.sid_roms.as_deref().map(roms);
        let mut tune = Self::load(data, options.rate, roms.as_ref())?;
        // SAFETY: the tune is open.
        let (count, start) = unsafe {
            (
                ffi::hk_sid_songs(tune.0.as_ptr()),
                ffi::hk_sid_start_song(tune.0.as_ptr()),
            )
        };
        let count = count.max(1);
        let database = match &options.sid_song_lengths {
            Some(path) => Some(database(path)?),
            None => None,
        };
        let mut subsongs = Vec::with_capacity(count as usize);
        let mut envelopes = Vec::with_capacity(count as usize);
        for index in 0..count {
            let stated = database.as_ref().and_then(|db| {
                let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                // SAFETY: the database and the tune are open; songs count from 1.
                let ms =
                    unsafe { ffi::hk_sid_db_length_ms(db.0.as_ptr(), tune.0.as_ptr(), index + 1) };
                u64::try_from(ms).ok().filter(|&ms| ms > 0)
            });
            let envelope = match stated {
                // The database gives where the song ends or starts over: no fade.
                Some(ms) => {
                    Envelope::ending_at(options.frames(std::time::Duration::from_millis(ms)), 0)
                }
                None => Envelope::ending_at(
                    options.frames(options.default_length + options.fade),
                    options.frames(options.fade),
                ),
            };
            subsongs.push(SubsongInfo {
                index,
                title: None,
                length: envelope.end().map(|frames| options.duration(frames)),
            });
            envelopes.push(envelope);
        }
        // SAFETY: the tune is open; the format string lives as long as it.
        let format = unsafe { ffi::hk_sid_format(tune.0.as_ptr()) };
        let format = if format.is_null() {
            None
        } else {
            // SAFETY: NUL-terminated, from the shim.
            text(&unsafe { CStr::from_ptr(format) }.to_string_lossy())
        };
        let info = SongInfo {
            format: format.unwrap_or_else(|| "SID".to_owned()),
            title: tune.info_string(0),
            artist: tune.info_string(1),
            album: None,
            date: tune.info_string(2),
            comment: None,
            subsongs,
            default_subsong: start.saturating_sub(1).min(count - 1),
        };
        let song = subsong.unwrap_or(info.default_subsong);
        if song >= count {
            return Err(SongError::NoSuchSubsong { index: song, count });
        }
        tune.select(song + 1)?;
        Ok(Self {
            tune,
            data: data.to_vec(),
            roms,
            envelope: envelopes[song as usize],
            info,
            song,
            rate: options.rate,
            position: 0,
            samples: Vec::new(),
        })
    }

    /// Loads the tune in `data` into a new emulator, with `roms` installed.
    fn load(
        data: &[u8],
        rate: u32,
        roms: Option<&[Option<Vec<u8>>; 3]>,
    ) -> Result<Tune, SongError> {
        if !header_is_sane(data) {
            return Err(SongError::Unsupported("the SID header is damaged".into()));
        }
        let length = u32::try_from(data.len())
            .map_err(|_| SongError::Unsupported("the file is too large".into()))?;
        let mut error = [0 as c_char; 256];
        // SAFETY: the data is valid for its length (the shim copies it); the error
        // buffer is valid for its length.
        let raw = unsafe {
            ffi::hk_sid_open(data.as_ptr(), length, rate, error.as_mut_ptr(), error.len())
        };
        let tune = Tune(NonNull::new(raw).ok_or_else(|| SongError::Unsupported(message(&error)))?);
        if let Some([kernal, basic, chargen]) = roms {
            let ptr = |rom: &Option<Vec<u8>>| rom.as_ref().map_or(std::ptr::null(), |r| r.as_ptr());
            // SAFETY: the tune is open; each pointer is null or an image of the right
            // size, which the shim copies.
            unsafe { ffi::hk_sid_set_roms(tune.0.as_ptr(), ptr(kernal), ptr(basic), ptr(chargen)) };
        }
        Ok(tune)
    }

    /// Starts the song over in a new emulator: a reloaded one keeps some state of
    /// its filters and resampler, and a seek must give what a straight run gives.
    fn restart(&mut self) -> Result<(), SongError> {
        let mut tune = Self::load(&self.data, self.rate, self.roms.as_ref())?;
        tune.select(self.song + 1)?;
        self.tune = tune;
        self.position = 0;
        Ok(())
    }

    fn render(&mut self, frames: usize) -> Result<(), SongError> {
        self.samples.resize(frames * 2, 0);
        // SAFETY: the tune is open and the buffer holds `frames` stereo frames.
        let got =
            unsafe { ffi::hk_sid_render(self.tune.0.as_ptr(), self.samples.as_mut_ptr(), frames) };
        if got < 0 {
            // SAFETY: the tune is open; the error string lives as long as it.
            let error = unsafe { CStr::from_ptr(ffi::hk_sid_error(self.tune.0.as_ptr())) };
            return Err(SongError::Render(format!(
                "libsidplayfp: {}",
                error.to_string_lossy()
            )));
        }
        Ok(())
    }
}

impl SongDecoder for SidDecoder {
    fn info(&self) -> &SongInfo {
        &self.info
    }

    fn subsong(&self) -> u32 {
        self.song
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
        self.render(frames)?;
        out.extend(self.samples.iter().map(|&s| f32::from(s) / 32_768.0));
        self.envelope.apply(self.position, out, 2);
        self.position += frames as u64;
        Ok(frames)
    }

    fn seek(&mut self, frame: u64) -> Result<(), SongError> {
        let target = self.envelope.end().map_or(frame, |end| frame.min(end));
        if target < self.position {
            self.restart()?;
        }
        while self.position < target {
            let frames = usize::try_from(target - self.position)
                .unwrap_or(BLOCK)
                .min(BLOCK);
            self.render(frames)?;
            self.position += frames as u64;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PSID header whose data offset lies past the end of the file; libsidplayfp
    /// read outside its buffer on it. Found by fuzzing.
    const BAD_OFFSET: &[u8] = &[
        0x50, 0x53, 0x49, 0x44, 0x00, 0x02, 0x9f, 0x7c, 0x08, 0x00, 0x10, 0xff, 0x48, 0x65, 0x6c,
        0x69, 0x6b, 0x6f, 0x64, 0x75, 0x20, 0x63, 0x6f, 0x6e, 0x74, 0x72, 0x69, 0x62, 0x75, 0x74,
        0x6f, 0x72, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x35, 0x20, 0x48, 0x65, 0x6c, 0x69, 0x6b, 0x6f, 0x64, 0x75, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0xaa, 0xa9, 0x0f, 0x8d,
        0x18, 0xd4, 0xa9, 0x32, 0x36, 0x20, 0x48, 0x65, 0x6c, 0x69, 0x6b, 0x6f, 0x64, 0x75, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0xea, 0xea, 0xea, 0xea, 0xea, 0x60, 0xea, 0xea,
        0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0x11, 0x16,
    ];

    fn psid(version: u16, offset: u16, load: u16, data: usize) -> Vec<u8> {
        let mut b = b"PSID".to_vec();
        b.extend_from_slice(&version.to_be_bytes());
        b.extend_from_slice(&offset.to_be_bytes());
        b.extend_from_slice(&load.to_be_bytes());
        b.resize(usize::from(offset) + data, 0);
        b
    }

    #[test]
    fn damaged_headers_are_refused_before_the_library_sees_them() {
        assert!(!header_is_sane(BAD_OFFSET));
        assert!(SidDecoder::open(BAD_OFFSET, None, &SongOptions::default()).is_err());
        assert!(tune_md5(BAD_OFFSET).is_none());
        assert!(header_is_sane(&psid(2, 0x7C, 0x1000, 16)));
        assert!(header_is_sane(&psid(1, 0x76, 0, 4)));
        assert!(!header_is_sane(&psid(2, 0x7C, 0, 2)));
        assert!(!header_is_sane(&psid(2, 0x76, 0x1000, 16)));
        assert!(!header_is_sane(&psid(9, 0x7C, 0x1000, 16)));
    }
}
