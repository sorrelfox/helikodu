// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The smarter playback settings: weighted and filtered shuffle
//! (`[playback.shuffle]`), endless play from a seed (`[playback.endless]`) and
//! the Auto-DJ (`[playback.auto_dj]`), with the play weights they share.

use serde::{Deserialize, Serialize};

use super::{SettingsError, check_range};

/// What a weighted pick favours. Several multiply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlayWeight {
    /// Higher rated tracks come up more often; tracks without a rating count as
    /// three stars.
    Rating,
    /// Tracks not played for a long time come up more often; never played tracks
    /// most.
    NotRecentlyPlayed,
    /// Tracks with fewer plays come up more often.
    LessPlayed,
}

/// What a weighted pick knows of a track's statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PlayFacts {
    /// The rating in half stars (0 to 10); `None` when not rated.
    pub rating: Option<u8>,
    /// The play count.
    pub plays: u32,
    /// Whole days since the track last played; `None` when it never played.
    pub days_since_played: Option<u32>,
}

/// The rating a track without one counts as, in half stars.
const UNRATED_HALF_STARS: u8 = 6;

/// The most days "not recently played" tells apart; older counts as this.
const OLDEST_DAYS: u32 = 365;

impl PlayWeight {
    /// The weight of a track with `facts`: larger is more likely, always above 0.
    pub fn weight(self, facts: &PlayFacts) -> f64 {
        match self {
            Self::Rating => {
                let half_stars = facts.rating.unwrap_or(UNRATED_HALF_STARS).min(10);
                let w = f64::from(1 + half_stars) / f64::from(1 + UNRATED_HALF_STARS);
                w * w
            }
            Self::NotRecentlyPlayed => {
                let days = facts
                    .days_since_played
                    .unwrap_or(OLDEST_DAYS)
                    .min(OLDEST_DAYS);
                f64::from(days + 1).sqrt()
            }
            Self::LessPlayed => 1.0 / (f64::from(facts.plays) + 1.0).sqrt(),
        }
    }
}

/// The weight of `facts` under every weight of `weights` together; 1 without any.
pub fn play_weight(weights: &[PlayWeight], facts: &PlayFacts) -> f64 {
    weights.iter().map(|w| w.weight(facts)).product()
}

/// Whether a track with `rating` (half stars) is passed over by `min_rating`: a
/// rated track below it is; an unrated track never is; 0 passes nothing over.
pub fn below_rating(min_rating: u8, rating: Option<u8>) -> bool {
    min_rating > 0 && rating.is_some_and(|r| r < min_rating)
}

/// Weighted and filtered shuffle: the `[playback.shuffle]` table.
///
/// ```toml
/// [playback.shuffle]
/// weight_by = ["rating"]          # rating | not-recently-played | less-played
/// min_rating = 4                  # half stars: skip tracks rated below two stars
/// skip_query = "genre:Christmas"  # tracks this query finds are passed over
/// ```
///
/// The weights apply to shuffling by tracks; the rating and the query pass tracks
/// over in every shuffle mode, as "Skip when shuffling" does.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShuffleSettings {
    /// What shuffling by tracks favours; empty for an even shuffle.
    pub weight_by: Vec<PlayWeight>,
    /// The lowest rating that plays while shuffling, in half stars (0 to 10); 0
    /// plays every track. Tracks without a rating always play.
    pub min_rating: u8,
    /// A query in the language of the search field; the tracks it finds are passed
    /// over while shuffling. Empty for none.
    pub skip_query: String,
}

impl ShuffleSettings {
    /// Whether anything but an even shuffle is asked for.
    pub fn is_plain(&self) -> bool {
        self.weight_by.is_empty() && self.min_rating == 0 && self.skip_query.trim().is_empty()
    }
}

/// What endless play continues from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeedKind {
    /// The last track of the queue.
    #[default]
    Track,
    /// The album of the last track.
    Album,
    /// The artist of the last track.
    Artist,
}

/// How much each likeness counts when endless play scores a track against its
/// seed, from 0 (not at all) to 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct RelatedWeights {
    /// The same track artist.
    pub artist: u8,
    /// The same album artist.
    pub album_artist: u8,
    /// A shared genre.
    pub genre: u8,
    /// A release year within the year range.
    pub year: u8,
    /// A shared label.
    pub labels: u8,
    /// A similar rating and play count.
    pub statistics: u8,
    /// How alike the tracks sound, once the sound analysis knows; tracks it has not
    /// analysed are scored on the rest.
    pub sound: u8,
}

impl Default for RelatedWeights {
    fn default() -> Self {
        Self {
            artist: 4,
            album_artist: 3,
            genre: 3,
            year: 2,
            labels: 2,
            statistics: 1,
            sound: 0,
        }
    }
}

impl RelatedWeights {
    fn validate(&self, table: &str) -> Result<(), SettingsError> {
        for (key, value) in [
            ("artist", self.artist),
            ("album_artist", self.album_artist),
            ("genre", self.genre),
            ("year", self.year),
            ("labels", self.labels),
            ("statistics", self.statistics),
            ("sound", self.sound),
        ] {
            check_range(&format!("{table}.{key}"), value, 0..=10)?;
        }
        Ok(())
    }
}

/// Endless play: what the repeat mode "Related tracks" adds when the queue ends,
/// the `[playback.endless]` table.
///
/// ```toml
/// [playback.endless]
/// seed = "track"        # track | album | artist
/// batch = 10            # tracks added at a time
/// window = 100          # no track repeats within this many
/// year_range = 5        # years either side that count as "the same time"
/// filter = ""           # a query the added tracks must match
///
/// [playback.endless.weights]
/// artist = 4
/// genre = 3
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EndlessSettings {
    /// What the added tracks are related to.
    pub seed: SeedKind,
    /// How many tracks are added each time the queue runs out (1 to 100).
    pub batch: u32,
    /// How many tracks played or queued last are not added again (0 to 10000).
    pub window: u32,
    /// How many years either side of the seed count as close (0 to 50).
    pub year_range: u32,
    /// A query the added tracks must match; empty for any.
    pub filter: String,
    /// How much each likeness counts.
    pub weights: RelatedWeights,
}

impl Default for EndlessSettings {
    fn default() -> Self {
        Self {
            seed: SeedKind::Track,
            batch: 10,
            window: 100,
            year_range: 5,
            filter: String::new(),
            weights: RelatedWeights::default(),
        }
    }
}

/// The Auto-DJ: the repeat mode that keeps tracks from the library queued ahead,
/// the `[playback.auto_dj]` table.
///
/// ```toml
/// [playback.auto_dj]
/// ahead = 5                         # tracks kept queued after the playing one
/// filter = "genre:Jazz OR genre:Soul"
/// weight_by = ["rating", "less-played"]
/// min_rating = 0
/// window = 200
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoDjSettings {
    /// How many tracks are kept queued after the playing one (1 to 50).
    pub ahead: u32,
    /// A query the added tracks must match; empty for the whole library.
    pub filter: String,
    /// What the picks favour.
    pub weight_by: Vec<PlayWeight>,
    /// The lowest rating added, in half stars (0 to 10); 0 for any. Tracks without
    /// a rating are added.
    pub min_rating: u8,
    /// How many tracks played or queued last are not added again (0 to 10000).
    pub window: u32,
}

impl Default for AutoDjSettings {
    fn default() -> Self {
        Self {
            ahead: 5,
            filter: String::new(),
            weight_by: vec![PlayWeight::Rating, PlayWeight::LessPlayed],
            min_rating: 0,
            window: 200,
        }
    }
}

/// The smarter playback settings together, as the player takes them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmartPlaySettings {
    /// Weighted and filtered shuffle.
    pub shuffle: ShuffleSettings,
    /// Endless play.
    pub endless: EndlessSettings,
    /// The Auto-DJ.
    pub auto_dj: AutoDjSettings,
    /// How many seconds of each track intro scan plays.
    pub intro_scan_s: u32,
}

pub(super) fn validate(
    shuffle: &ShuffleSettings,
    endless: &EndlessSettings,
    auto_dj: &AutoDjSettings,
) -> Result<(), SettingsError> {
    check_range("playback.shuffle.min_rating", shuffle.min_rating, 0..=10)?;
    check_range("playback.endless.batch", endless.batch, 1..=100)?;
    check_range("playback.endless.window", endless.window, 0..=10_000)?;
    check_range("playback.endless.year_range", endless.year_range, 0..=50)?;
    endless.weights.validate("playback.endless.weights")?;
    check_range("playback.auto_dj.ahead", auto_dj.ahead, 1..=50)?;
    check_range("playback.auto_dj.min_rating", auto_dj.min_rating, 0..=10)?;
    check_range("playback.auto_dj.window", auto_dj.window, 0..=10_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn weights_favour_what_they_name() {
        let five = PlayFacts {
            rating: Some(10),
            ..PlayFacts::default()
        };
        let one = PlayFacts {
            rating: Some(2),
            ..PlayFacts::default()
        };
        let unrated = PlayFacts::default();
        assert!(PlayWeight::Rating.weight(&five) > PlayWeight::Rating.weight(&unrated));
        assert!(PlayWeight::Rating.weight(&unrated) > PlayWeight::Rating.weight(&one));
        let fresh = PlayFacts {
            days_since_played: Some(0),
            plays: 30,
            ..PlayFacts::default()
        };
        assert!(
            PlayWeight::NotRecentlyPlayed.weight(&unrated)
                > PlayWeight::NotRecentlyPlayed.weight(&fresh)
        );
        assert!(PlayWeight::LessPlayed.weight(&unrated) > PlayWeight::LessPlayed.weight(&fresh));
        assert!((play_weight(&[], &fresh) - 1.0).abs() < f64::EPSILON);
        for weight in [
            PlayWeight::Rating,
            PlayWeight::NotRecentlyPlayed,
            PlayWeight::LessPlayed,
        ] {
            assert!(
                weight.weight(&PlayFacts {
                    rating: Some(0),
                    plays: u32::MAX,
                    days_since_played: Some(0),
                }) > 0.0
            );
        }
    }

    #[test]
    fn the_rating_floor_keeps_unrated_tracks() {
        assert!(!below_rating(0, Some(1)));
        assert!(below_rating(4, Some(3)));
        assert!(!below_rating(4, Some(4)));
        assert!(!below_rating(4, None));
    }

    #[test]
    fn written_and_read_in_kebab_case() {
        let mut settings = Settings::default();
        settings.playback.shuffle.weight_by = vec![PlayWeight::NotRecentlyPlayed];
        settings.playback.endless.seed = SeedKind::Artist;
        let text = toml::to_string(&settings).unwrap();
        assert!(
            text.contains("weight_by = [\"not-recently-played\"]"),
            "{text}"
        );
        assert!(text.contains("seed = \"artist\""), "{text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back, settings);
        let partial: Settings = toml::from_str("[playback.auto_dj]\nahead = 9\n").unwrap();
        assert_eq!(partial.playback.auto_dj.ahead, 9);
        assert_eq!(partial.playback.auto_dj.window, 200);
    }

    #[test]
    fn ranges_are_checked() {
        let mut settings = Settings::default();
        assert_eq!(settings.validate(), Ok(()));
        settings.playback.endless.batch = 0;
        assert!(settings.validate().is_err());
        settings.playback.endless.batch = 10;
        settings.playback.endless.weights.sound = 11;
        assert!(settings.validate().is_err());
        settings.playback.endless.weights.sound = 0;
        settings.playback.auto_dj.ahead = 51;
        assert!(settings.validate().is_err());
    }
}
