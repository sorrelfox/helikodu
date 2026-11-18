// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Game console music through libgme (Game_Music_Emu): AY, GBS, GYM, HES, KSS,
//! NSF/NSFE, SAP, SPC and VGM/VGZ.
//!
//! libgme recognises its formats by their first four bytes ([`probe`]). Each track of
//! a multi-track file (NSF, GBS, HES, KSS, ...) is a subsong. A track plays for the
//! length the file states; a track with a loop plays its intro and
//! [`SongOptions::loops`] passes of the loop; anything else plays for
//! [`SongOptions::default_length`]. Then it fades out over [`SongOptions::fade`] (or
//! the fade the file states). libgme's own silence detection and fade are off, so the
//! length is known in advance and the same on every run. Seeking renders from the
//! start of the track to the exact frame; the emulators run far faster than real
//! time.

use std::ffi::{CStr, c_char, c_int, c_long};
use std::ptr::{self, NonNull};
use std::time::Duration;

use helikodu_gme_sys as ffi;

use crate::song::text;
use crate::song::{Envelope, SongDecoder, SongError, SongInfo, SongOptions, SubsongInfo};

/// Frames rendered at a time.
const BLOCK: usize = 2048;

/// Whether `header`, the first bytes of a file, carries the signature of a format
/// libgme plays.
pub fn probe(header: &[u8]) -> bool {
    if header.len() < 4 {
        return false;
    }
    // SAFETY: gme_identify_header reads the first four bytes, which exist; it
    // returns a static string, empty when the header is unknown.
    let kind = unsafe { CStr::from_ptr(ffi::gme_identify_header(header.as_ptr().cast())) };
    !kind.to_bytes().is_empty()
}

/// An emulator with a file loaded.
struct Emu(NonNull<ffi::Music_Emu>);

// SAFETY: a libgme emulator has no thread affinity; it is used from one thread at a
// time, through `&mut`.
unsafe impl Send for Emu {}

impl Drop for Emu {
    fn drop(&mut self) {
        // SAFETY: the emulator was created by gme_open_data and is deleted once.
        unsafe { ffi::gme_delete(self.0.as_ptr()) };
    }
}

impl std::fmt::Debug for Emu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Emu")
    }
}

/// A libgme error message, or `Ok` for null.
fn check(error: *const c_char) -> Result<(), String> {
    if error.is_null() {
        return Ok(());
    }
    // SAFETY: libgme's errors are static NUL-terminated strings.
    Err(unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned())
}

/// A string field of a track's info.
fn field(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: the info's strings are NUL-terminated and live as long as the info.
    text(&unsafe { CStr::from_ptr(value) }.to_string_lossy())
}

/// What a track's info says about its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrackTiming {
    /// The time before the fade starts.
    play: Duration,
    /// The fade.
    fade: Duration,
}

impl TrackTiming {
    fn of(info: &ffi::gme_info_t, options: &SongOptions) -> Self {
        let ms = |value: c_int| {
            u64::try_from(value)
                .ok()
                .filter(|&v| v > 0)
                .map(Duration::from_millis)
        };
        let stated_fade = ms(info.fade_length);
        if let Some(length) = ms(info.length) {
            Self {
                play: length,
                fade: stated_fade.unwrap_or(options.fade),
            }
        } else if let Some(each) = ms(info.loop_length) {
            let intro = ms(info.intro_length).unwrap_or_default();
            Self {
                play: intro + each * options.loops.max(1),
                fade: options.fade,
            }
        } else {
            Self {
                play: options.default_length,
                fade: options.fade,
            }
        }
    }

    fn total(self) -> Duration {
        self.play + self.fade
    }
}

/// A track of a game music file, rendered to stereo `f32`.
#[derive(Debug)]
pub struct GmeDecoder {
    emu: Emu,
    info: SongInfo,
    track: u32,
    rate: u32,
    envelope: Envelope,
    position: u64,
    samples: Vec<i16>,
}

impl GmeDecoder {
    /// Opens track `subsong` (counted from 0; `None` for the first) of the file in
    /// `data`.
    pub fn open(
        data: &[u8],
        subsong: Option<u32>,
        options: &SongOptions,
    ) -> Result<Self, SongError> {
        let size = c_long::try_from(data.len())
            .map_err(|_| SongError::Unsupported("the file is too large".into()))?;
        let rate = c_int::try_from(options.rate).unwrap_or(44_100);
        let mut raw = ptr::null_mut();
        // SAFETY: the data is valid for its length and libgme copies what it keeps;
        // `raw` receives the emulator.
        let error = unsafe { ffi::gme_open_data(data.as_ptr().cast(), size, &raw mut raw, rate) };
        check(error).map_err(SongError::Unsupported)?;
        let emu = Emu(NonNull::new(raw)
            .ok_or_else(|| SongError::Unsupported("libgme cannot open the file".into()))?);
        // SAFETY: the emulator is open.
        unsafe {
            ffi::gme_ignore_silence(emu.0.as_ptr(), 1);
            ffi::gme_set_autoload_playback_limit(emu.0.as_ptr(), 0);
        }

        // SAFETY: the emulator is open.
        let count = unsafe { ffi::gme_track_count(emu.0.as_ptr()) }.max(1);
        let count = u32::try_from(count).unwrap_or(1);
        // SAFETY: the emulator is open; gme_type_system returns a static string.
        let system = field(unsafe { ffi::gme_type_system(ffi::gme_type(emu.0.as_ptr())) });
        let mut info = SongInfo {
            format: system.clone().unwrap_or_else(|| "Game music".to_owned()),
            default_subsong: 0,
            ..SongInfo::default()
        };
        let mut timings = Vec::with_capacity(count as usize);
        for index in 0..count {
            let mut track: *mut ffi::gme_info_t = ptr::null_mut();
            // SAFETY: the emulator is open; `track` receives an info freed below.
            let error =
                unsafe { ffi::gme_track_info(emu.0.as_ptr(), &raw mut track, index as c_int) };
            if check(error).is_err() || track.is_null() {
                let timing = TrackTiming {
                    play: options.default_length,
                    fade: options.fade,
                };
                timings.push(timing);
                info.subsongs.push(SubsongInfo {
                    index,
                    title: None,
                    length: Some(timing.total()),
                });
                continue;
            }
            // SAFETY: gme_track_info succeeded, so `track` points to a valid info.
            let t = unsafe { &*track };
            let timing = TrackTiming::of(t, options);
            timings.push(timing);
            if index == 0 {
                info.album = field(t.game);
                info.artist = field(t.author);
                info.date = field(t.copyright);
                info.comment = field(t.comment);
                if count == 1 {
                    info.title = field(t.song);
                }
            }
            info.subsongs.push(SubsongInfo {
                index,
                title: field(t.song),
                length: Some(timing.total()),
            });
            // SAFETY: the info came from gme_track_info and is freed once.
            unsafe { ffi::gme_free_info(track) };
        }
        if info.title.is_none() && count > 1 {
            info.title.clone_from(&info.album);
        }

        let track = subsong.unwrap_or(0);
        if track >= count {
            return Err(SongError::NoSuchSubsong {
                index: track,
                count,
            });
        }
        let timing = timings[track as usize];
        let envelope =
            Envelope::ending_at(options.frames(timing.total()), options.frames(timing.fade));
        let mut decoder = Self {
            emu,
            info,
            track,
            rate: options.rate,
            envelope,
            position: 0,
            samples: Vec::new(),
        };
        decoder.restart()?;
        Ok(decoder)
    }

    fn restart(&mut self) -> Result<(), SongError> {
        // SAFETY: the emulator is open; the track exists.
        let error = unsafe { ffi::gme_start_track(self.emu.0.as_ptr(), self.track as c_int) };
        check(error).map_err(SongError::Unsupported)?;
        self.position = 0;
        Ok(())
    }

    /// Renders `frames` frames into `self.samples`.
    fn render(&mut self, frames: usize) -> Result<(), SongError> {
        self.samples.resize(frames * 2, 0);
        let count = c_int::try_from(frames * 2).unwrap_or(c_int::MAX);
        // SAFETY: the emulator is open and the buffer holds `count` samples.
        let error = unsafe { ffi::gme_play(self.emu.0.as_ptr(), count, self.samples.as_mut_ptr()) };
        check(error).map_err(SongError::Render)
    }
}

impl SongDecoder for GmeDecoder {
    fn info(&self) -> &SongInfo {
        &self.info
    }

    fn subsong(&self) -> u32 {
        self.track
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
