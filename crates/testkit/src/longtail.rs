// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small files of the long-tail formats, written from scratch for tests: a tracker
//! module, a NES sound file, a SID tune, a MIDI file and a SoundFont. They hold
//! nothing but what is written here, so they are the project's own work and carry
//! its licence.
//!
//! This file depends on nothing but `std`: `xtask` includes it as a module of its
//! own to write the same files into the test corpus.

/// A ProTracker module (`M.K.`, four channels) with two subsongs.
///
/// One 32-byte looped square-wave sample. Order 0 plays pattern 0, which jumps back
/// to order 0 after 16 rows (subsong 0: 16 rows, 1.92 s at speed 6 and 125 BPM).
/// Order 1 is reached by nothing else, so it starts subsong 1: pattern 1, which
/// breaks to the end of the song after 8 rows (0.96 s). The title is
/// [`MODULE_TITLE`].
pub fn tracker_module() -> Vec<u8> {
    let mut b = Vec::new();
    push_padded(&mut b, MODULE_TITLE.as_bytes(), 20);
    // Sample 1: 32 bytes (16 words), volume 48, looped over all of it.
    push_padded(&mut b, b"square", 22);
    b.extend_from_slice(&16u16.to_be_bytes());
    b.push(0); // finetune
    b.push(48); // volume
    b.extend_from_slice(&0u16.to_be_bytes()); // loop start (words)
    b.extend_from_slice(&16u16.to_be_bytes()); // loop length (words)
    for _ in 1..31 {
        b.extend_from_slice(&[0; 22]);
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    }
    b.push(2); // song length
    b.push(127);
    let mut orders = [0u8; 128];
    orders[1] = 1;
    b.extend_from_slice(&orders);
    b.extend_from_slice(b"M.K.");
    // Pattern 0: a C-2 on channel 1, a G-2 on channel 2 at row 8, a jump to order 0
    // at row 15.
    let mut pattern = [[0u8; 16]; 64];
    pattern[0][0..4].copy_from_slice(&cell(1, 428, 0, 0));
    pattern[8][4..8].copy_from_slice(&cell(1, 285, 0, 0));
    pattern[15][12..16].copy_from_slice(&cell(0, 0, 0xB, 0));
    b.extend(pattern.iter().flatten());
    // Pattern 1: an A-2 on channel 3, a pattern break at row 7 (to order 2: the end).
    let mut pattern = [[0u8; 16]; 64];
    pattern[0][8..12].copy_from_slice(&cell(1, 254, 0, 0));
    pattern[7][12..16].copy_from_slice(&cell(0, 0, 0xD, 0));
    b.extend(pattern.iter().flatten());
    // The sample: 16 high and 16 low bytes.
    b.extend(std::iter::repeat_n(64u8, 16));
    b.extend(std::iter::repeat_n((-64i8) as u8, 16));
    b
}

/// The title of [`tracker_module`].
pub const MODULE_TITLE: &str = "Helikodu test";

/// One cell of a ProTracker pattern.
fn cell(sample: u8, period: u16, effect: u8, param: u8) -> [u8; 4] {
    [
        (sample & 0xf0) | ((period >> 8) as u8 & 0x0f),
        (period & 0xff) as u8,
        ((sample & 0x0f) << 4) | (effect & 0x0f),
        param,
    ]
}

/// An NSF file (NES sound format) with three songs, each holding one square-wave
/// note on the first pulse channel at its own pitch, with no stated length. The
/// title, artist and copyright are [`NSF_TITLE`], [`NSF_ARTIST`] and
/// [`NSF_COPYRIGHT`].
pub fn nsf_file() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"NESM\x1a");
    b.push(1); // version
    b.push(3); // songs
    b.push(1); // first song
    b.extend_from_slice(&0x8000u16.to_le_bytes()); // load
    b.extend_from_slice(&0x8000u16.to_le_bytes()); // init
    b.extend_from_slice(&0x801cu16.to_le_bytes()); // play
    push_padded(&mut b, NSF_TITLE.as_bytes(), 32);
    push_padded(&mut b, NSF_ARTIST.as_bytes(), 32);
    push_padded(&mut b, NSF_COPYRIGHT.as_bytes(), 32);
    b.extend_from_slice(&16_639u16.to_le_bytes()); // NTSC play period, µs
    b.extend_from_slice(&[0; 8]); // no bank switching
    b.extend_from_slice(&19_997u16.to_le_bytes()); // PAL play period, µs
    b.push(0); // NTSC
    b.push(0); // no expansion chips
    b.extend_from_slice(&[0; 4]);
    debug_assert_eq!(b.len(), 0x80);
    let code: [u8; 0x23] = [
        0xaa, // 8000 TAX            ; X = song
        0xa9, 0x0f, // LDA #$0F
        0x8d, 0x15, 0x40, // STA $4015    ; channels on
        0xa9, 0xbf, // LDA #$BF      ; duty 50 %, halt, constant volume 15
        0x8d, 0x00, 0x40, // STA $4000
        0xa9, 0x00, // LDA #$00      ; no sweep
        0x8d, 0x01, 0x40, // STA $4001
        0xbd, 0x20, 0x80, // LDA $8020,X  ; the song's period
        0x8d, 0x02, 0x40, // STA $4002
        0xa9, 0x01, // LDA #$01      ; period high bits, length load
        0x8d, 0x03, 0x40, // STA $4003
        0x60, // 801B RTS
        0x60, // 801C RTS          ; play
        0xea, 0xea, 0xea, // NOPs to $8020
        0xfd, 0xa9, 0x7e, // 8020 periods
    ];
    b.extend_from_slice(&code);
    b
}

/// The title of [`nsf_file`].
pub const NSF_TITLE: &str = "Helikodu Tones";
/// The artist of [`nsf_file`].
pub const NSF_ARTIST: &str = "Helikodu contributors";
/// The copyright of [`nsf_file`].
pub const NSF_COPYRIGHT: &str = "2026 Helikodu";

/// A PSID v2 tune with two songs: each sets the SID's volume and plays one
/// triangle-wave note on voice 1 at its own pitch. The default song is the second.
/// The title, author and release are [`SID_TITLE`], [`SID_AUTHOR`] and
/// [`SID_RELEASED`].
pub fn psid_file() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"PSID");
    b.extend_from_slice(&2u16.to_be_bytes()); // version
    b.extend_from_slice(&0x7cu16.to_be_bytes()); // data offset
    b.extend_from_slice(&0u16.to_be_bytes()); // load address: in the data
    b.extend_from_slice(&0x1000u16.to_be_bytes()); // init
    b.extend_from_slice(&0x1030u16.to_be_bytes()); // play
    b.extend_from_slice(&2u16.to_be_bytes()); // songs
    b.extend_from_slice(&2u16.to_be_bytes()); // start song
    b.extend_from_slice(&0u32.to_be_bytes()); // speed: vertical blank
    push_padded(&mut b, SID_TITLE.as_bytes(), 32);
    push_padded(&mut b, SID_AUTHOR.as_bytes(), 32);
    push_padded(&mut b, SID_RELEASED.as_bytes(), 32);
    b.extend_from_slice(&0x0014u16.to_be_bytes()); // flags: PAL, MOS 6581
    b.extend_from_slice(&[0, 0, 0, 0]); // relocation, second and third SID
    debug_assert_eq!(b.len(), 0x7c);
    b.extend_from_slice(&0x1000u16.to_le_bytes()); // load address
    let mut code = vec![
        0xaa, // 1000 TAX          ; X = song
        0xa9, 0x0f, // LDA #$0F
        0x8d, 0x18, 0xd4, // STA $D418  ; volume 15
        0xa9, 0x00, // LDA #$00
        0x8d, 0x05, 0xd4, // STA $D405  ; attack and decay 0
        0xa9, 0xf0, // LDA #$F0
        0x8d, 0x06, 0xd4, // STA $D406  ; sustain 15, release 0
        0xa9, 0x00, // LDA #$00
        0x8d, 0x00, 0xd4, // STA $D400  ; frequency low
        0xbd, 0x40, 0x10, // LDA $1040,X ; frequency high
        0x8d, 0x01, 0xd4, // STA $D401
        0xa9, 0x11, // LDA #$11      ; triangle, gate on
        0x8d, 0x04, 0xd4, // STA $D404
        0x60, // RTS
    ];
    code.resize(0x30, 0xea);
    code.push(0x60); // 1030 RTS        ; play
    code.resize(0x40, 0xea);
    code.extend_from_slice(&[0x11, 0x16]); // 1040 frequencies
    b.extend_from_slice(&code);
    b
}

/// The title of [`psid_file`].
pub const SID_TITLE: &str = "Helikodu Triangle";
/// The author of [`psid_file`].
pub const SID_AUTHOR: &str = "Helikodu contributors";
/// The release line of [`psid_file`].
pub const SID_RELEASED: &str = "2026 Helikodu";

/// A standard MIDI file (format 0, 96 ticks per quarter note, 120 BPM) of four
/// quarter notes, 2 s long, with the title [`MIDI_TITLE`] and the copyright
/// [`MIDI_COPYRIGHT`].
pub fn midi_file() -> Vec<u8> {
    let mut track = Vec::new();
    let meta = |track: &mut Vec<u8>, kind: u8, text: &[u8]| {
        track.extend_from_slice(&[0x00, 0xff, kind, text.len() as u8]);
        track.extend_from_slice(text);
    };
    meta(&mut track, 0x03, MIDI_TITLE.as_bytes());
    meta(&mut track, 0x02, MIDI_COPYRIGHT.as_bytes());
    track.extend_from_slice(&[0x00, 0xc0, 0x00]); // program 1
    for key in [60u8, 64, 67, 72] {
        track.extend_from_slice(&[0x00, 0x90, key, 100]);
        // A quarter note later (96 ticks), the note off as a note on with velocity 0.
        track.extend_from_slice(&[0x60, 0x90, key, 0]);
    }
    track.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let mut b = Vec::new();
    b.extend_from_slice(b"MThd");
    b.extend_from_slice(&6u32.to_be_bytes());
    b.extend_from_slice(&0u16.to_be_bytes()); // format 0
    b.extend_from_slice(&1u16.to_be_bytes()); // one track
    b.extend_from_slice(&96u16.to_be_bytes());
    b.extend_from_slice(b"MTrk");
    b.extend_from_slice(&(track.len() as u32).to_be_bytes());
    b.extend_from_slice(&track);
    b
}

/// The title of [`midi_file`].
pub const MIDI_TITLE: &str = "Helikodu Arpeggio";
/// The copyright of [`midi_file`].
pub const MIDI_COPYRIGHT: &str = "2026 Helikodu contributors";

/// A SoundFont 2 file with one preset (bank 0, program 0) that plays a looped sine
/// wave over the whole keyboard, tuned so that key 60 sounds at 261.6 Hz.
pub fn soundfont() -> Vec<u8> {
    // One cycle of a sine over 100 samples at 26 163 Hz is 261.63 Hz: key 60.
    const CYCLE: u32 = 100;
    const CYCLES: u32 = 20;
    let mut wave: Vec<u8> = Vec::new();
    for i in 0..CYCLE * CYCLES {
        let phase = f64::from(i % CYCLE) / f64::from(CYCLE) * std::f64::consts::TAU;
        let sample = (phase.sin() * 16_000.0).round() as i16;
        wave.extend_from_slice(&sample.to_le_bytes());
    }
    // The format asks for 46 zero samples after each sample.
    wave.extend_from_slice(&[0; 92]);

    let info = list(
        b"INFO",
        &[
            chunk(b"ifil", &[2, 0, 1, 0]),
            chunk(b"isng", b"EMU8000\0"),
            chunk(b"INAM", b"Helikodu test sine\0\0"),
        ],
    );
    let sdta = list(b"sdta", &[chunk(b"smpl", &wave)]);

    let mut phdr = Vec::new();
    for (name, preset, bag) in [(&b"Sine"[..], 0u16, 0u16), (b"EOP", 0, 1)] {
        push_padded(&mut phdr, name, 20);
        phdr.extend_from_slice(&preset.to_le_bytes());
        phdr.extend_from_slice(&0u16.to_le_bytes()); // bank
        phdr.extend_from_slice(&bag.to_le_bytes());
        phdr.extend_from_slice(&[0; 12]); // library, genre, morphology
    }
    let bags = |entries: &[(u16, u16)]| -> Vec<u8> {
        entries
            .iter()
            .flat_map(|(generator, modulator)| {
                let mut e = generator.to_le_bytes().to_vec();
                e.extend_from_slice(&modulator.to_le_bytes());
                e
            })
            .collect()
    };
    let generators = |entries: &[(u16, u16)]| bags(entries);
    let mut inst = Vec::new();
    for (name, bag) in [(&b"Sine"[..], 0u16), (b"EOI", 1)] {
        push_padded(&mut inst, name, 20);
        inst.extend_from_slice(&bag.to_le_bytes());
    }
    let mut shdr = Vec::new();
    let end = CYCLE * CYCLES;
    for (name, start, end, loop_start, loop_end, rate, kind) in [
        (&b"Sine"[..], 0u32, end, CYCLE, end - CYCLE, 26_163u32, 1u16),
        (b"EOS", 0, 0, 0, 0, 0, 0),
    ] {
        push_padded(&mut shdr, name, 20);
        for value in [start, end, loop_start, loop_end, rate] {
            shdr.extend_from_slice(&value.to_le_bytes());
        }
        shdr.push(60); // original key
        shdr.push(0); // correction
        shdr.extend_from_slice(&0u16.to_le_bytes()); // link
        shdr.extend_from_slice(&kind.to_le_bytes());
    }
    let pdta = list(
        b"pdta",
        &[
            chunk(b"phdr", &phdr),
            chunk(b"pbag", &bags(&[(0, 0), (1, 0)])),
            chunk(b"pmod", &[0; 10]),
            // Preset zone: instrument 0.
            chunk(b"pgen", &generators(&[(41, 0), (0, 0)])),
            chunk(b"inst", &inst),
            chunk(b"ibag", &bags(&[(0, 0), (2, 0)])),
            chunk(b"imod", &[0; 10]),
            // Instrument zone: loop continuously (sampleModes 1), then sample 0.
            chunk(b"igen", &generators(&[(54, 1), (53, 0), (0, 0)])),
            chunk(b"shdr", &shdr),
        ],
    );
    let mut body = b"sfbk".to_vec();
    body.extend_from_slice(&info);
    body.extend_from_slice(&sdta);
    body.extend_from_slice(&pdta);
    chunk(b"RIFF", &body)
}

/// A RIFF chunk, padded to an even length.
fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut b = id.to_vec();
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(data);
    if data.len() % 2 == 1 {
        b.push(0);
    }
    b
}

/// A RIFF `LIST` of `kind` holding `chunks`.
fn list(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut data = kind.to_vec();
    for c in chunks {
        data.extend_from_slice(c);
    }
    chunk(b"LIST", &data)
}

/// `text` padded with zeros (or cut) to `len` bytes.
fn push_padded(b: &mut Vec<u8>, text: &[u8], len: usize) {
    let n = text.len().min(len);
    b.extend_from_slice(&text[..n]);
    b.extend(std::iter::repeat_n(0, len - n));
}
