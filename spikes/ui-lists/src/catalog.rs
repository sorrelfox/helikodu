// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic synthetic library data, standing in for the read-only
//! database connection of the real player. A row is computed from its ID only
//! when a list asks for it.

/// Small, fast pseudo-random number source (SplitMix64), seeded from an ID.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

const WORDS: &[&str] = &[
    "Amber",
    "Northern",
    "Lights",
    "Silent",
    "River",
    "Glass",
    "Harbour",
    "Echoes",
    "Velvet",
    "Midnight",
    "Paper",
    "Moons",
    "Signal",
    "Distant",
    "Shore",
    "Iron",
    "Garden",
    "Winter",
    "Static",
    "Bloom",
    "Hollow",
    "Crown",
    "Satellite",
    "Ember",
    "Lantern",
    "Tide",
    "Copper",
    "Fields",
    "Arcade",
    "Ghost",
    "Meridian",
    "Saltwater",
    "Orchard",
    "Neon",
    "Cinder",
    "Parade",
    "Kestrel",
    "Quartz",
    "Monsoon",
    "Atlas",
    "Ünïcødé",
    "Ljós",
    "Сирень",
    "東京",
];
const GENRES: &[&str] = &[
    "Rock",
    "Jazz",
    "Electronic",
    "Classical",
    "Folk",
    "Ambient",
    "Hip-Hop",
    "Soul",
    "Metal",
    "Pop",
    "Blues",
    "Soundtrack",
];
const FORMATS: &[&str] = &[
    "FLAC 16/44.1",
    "FLAC 24/96",
    "FLAC 24/192",
    "MP3 320",
    "Opus 160",
    "AAC 256",
    "ALAC 24/48",
    "WAV 16/44.1",
];

fn words(rng: &mut Rng, n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(WORDS[rng.below(WORDS.len() as u64) as usize]);
    }
    s
}

/// One row of the track table.
#[derive(Debug, Clone)]
pub struct Track {
    pub number: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: u32,
    pub genre: &'static str,
    pub duration: String,
    pub format: &'static str,
    pub composer: String,
    pub bitrate: String,
    pub plays: u32,
    pub rating: u8,
}

/// Number of albums the synthetic library has per track count.
pub fn album_of(track_id: u32) -> u32 {
    track_id / 12
}

pub fn track(id: u32) -> Track {
    let album = album(album_of(id));
    let mut rng = Rng::new(u64::from(id) * 7919);
    let secs = 90 + rng.below(420) as u32;
    let title_len = 1 + rng.below(4) as usize;
    Track {
        number: id % 12 + 1,
        title: words(&mut rng, title_len),
        artist: album.artist.clone(),
        album: album.title,
        year: album.year,
        genre: album.genre,
        duration: format!("{}:{:02}", secs / 60, secs % 60),
        format: FORMATS[rng.below(FORMATS.len() as u64) as usize],
        composer: words(&mut rng, 2),
        bitrate: format!("{} kbps", 128 + rng.below(1300)),
        plays: rng.below(400) as u32,
        rating: rng.below(6) as u8,
    }
}

/// One tile of the album grid.
#[derive(Debug, Clone)]
pub struct Album {
    pub title: String,
    pub artist: String,
    pub year: u32,
    pub genre: &'static str,
}

pub fn album(id: u32) -> Album {
    let mut rng = Rng::new(u64::from(id) * 104_729 + 17);
    let title_len = 1 + rng.below(3) as usize;
    Album {
        title: words(&mut rng, title_len),
        artist: words(&mut rng, 2),
        year: 1960 + rng.below(66) as u32,
        genre: GENRES[rng.below(GENRES.len() as u64) as usize],
    }
}
