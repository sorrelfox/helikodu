// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! "How alike" the tracks that endless play adds are: the named sets of
//! weights of Settings › Playback, and how the queue panel words them.

use std::cell::Cell;

use helikodu_model::settings::RelatedWeights;

use crate::i18n::tr;

/// A choice of "How alike".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Alike {
    /// Artist and genre most; years, labels and play counts a little.
    Mix,
    /// The same artist.
    Artist,
    /// The same genre.
    Genre,
    /// How the tracks sound.
    Sound,
    /// The user's own weights.
    Own,
}

/// Weights in the order the settings list them: artist, album artist, genre,
/// years, labels, play counts and ratings, sound.
const fn weights(w: [u8; 7]) -> RelatedWeights {
    RelatedWeights {
        artist: w[0],
        album_artist: w[1],
        genre: w[2],
        year: w[3],
        labels: w[4],
        statistics: w[5],
        sound: w[6],
    }
}

thread_local! {
    /// The user's own weights while a named choice is used, for the next time
    /// "Your own weights" is chosen.
    static OWN: Cell<Option<RelatedWeights>> = const { Cell::new(None) };
}

impl Alike {
    /// The choices in the order of the dropdown.
    pub(crate) const ALL: [Self; 5] =
        [Self::Mix, Self::Artist, Self::Genre, Self::Sound, Self::Own];

    /// The weights of a named choice; `None` for the user's own.
    pub(crate) fn weights(self) -> Option<RelatedWeights> {
        match self {
            Self::Mix => Some(weights([4, 3, 3, 2, 2, 1, 0])),
            Self::Artist => Some(weights([10, 6, 0, 0, 0, 0, 0])),
            Self::Genre => Some(weights([0, 0, 10, 2, 0, 0, 0])),
            Self::Sound => Some(weights([0, 0, 2, 0, 0, 0, 10])),
            Self::Own => None,
        }
    }

    /// The choice that `weights` are: a named one when they are its weights.
    pub(crate) fn of(weights: &RelatedWeights) -> Self {
        Self::ALL
            .into_iter()
            .find(|a| a.weights().as_ref() == Some(weights))
            .unwrap_or(Self::Own)
    }

    /// Its name in the dropdown.
    pub(crate) fn name(self) -> String {
        match self {
            Self::Mix => tr("A mix"),
            Self::Artist => tr("Same artist"),
            Self::Genre => tr("Same genre"),
            Self::Sound => tr("Sounds alike"),
            Self::Own => tr("Your own weights"),
        }
    }

    /// What it weighs, in a sentence beside the dropdown.
    pub(crate) fn explanation(self) -> String {
        match self {
            Self::Mix => tr("Artist and genre count most; years, labels and play counts a little."),
            Self::Artist => tr("The same artist counts most, then the same album artist."),
            Self::Genre => tr("The same genre counts most; years close together a little."),
            Self::Sound => tr("How the tracks sound counts most; the same genre a little."),
            Self::Own => tr("Set below how much each likeness counts, from 0 to 10."),
        }
    }

    /// How the tracks are chosen, after "10 at a time," in the queue panel.
    pub(crate) fn by(self) -> String {
        match self {
            Self::Mix => tr("by artist and genre"),
            Self::Artist => tr("by artist"),
            Self::Genre => tr("by genre"),
            Self::Sound => tr("by how they sound"),
            Self::Own => tr("by your own weights"),
        }
    }

    /// Keeps `weights` as the user's own for the next time they are chosen.
    pub(crate) fn keep_own(weights: RelatedWeights) {
        OWN.with(|own| own.set(Some(weights)));
    }

    /// The user's own weights kept last, or `current` when none are kept.
    pub(crate) fn own_or(current: RelatedWeights) -> RelatedWeights {
        OWN.with(Cell::get).unwrap_or(current)
    }
}

/// Whether every weight of `weights` is 0: then nothing is related.
pub(crate) fn all_zero(weights: &RelatedWeights) -> bool {
    [
        weights.artist,
        weights.album_artist,
        weights.genre,
        weights.year,
        weights.labels,
        weights.statistics,
        weights.sound,
    ]
    .iter()
    .all(|&w| w == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_weights_are_a_mix() {
        assert_eq!(Alike::of(&RelatedWeights::default()), Alike::Mix);
        for alike in Alike::ALL {
            if let Some(w) = alike.weights() {
                assert_eq!(Alike::of(&w), alike);
            }
        }
        let own = RelatedWeights {
            sound: 3,
            ..RelatedWeights::default()
        };
        assert_eq!(Alike::of(&own), Alike::Own);
        assert!(all_zero(&weights([0; 7])));
        assert!(!all_zero(&own));
    }
}
