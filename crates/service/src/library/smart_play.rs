// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smarter playback over the library: the statistics a weighted shuffle weighs, the
//! tracks endless play and the Auto-DJ add, and generator playlists.

use std::collections::{HashMap, HashSet};

use helikodu_model::library::TrackId;
use helikodu_model::settings::{AutoDjSettings, EndlessSettings, PlayFacts};

pub use helikodu_library::{
    Generator, GeneratorError, GeneratorId, GeneratorSort, GeneratorSource, GeneratorStep,
    GeneratorSteps, NoSoundSimilarity, PickError, PlaySeed, SeedProfile, SoundSimilarity,
    StepError, TrackFacts, related_score, time_seed,
};

use super::{LibraryError, MusicLibrary};

impl MusicLibrary {
    /// The statistics of `tracks` that weighted picks weigh, by track.
    pub fn play_facts(
        &self,
        tracks: &[TrackId],
    ) -> Result<HashMap<TrackId, PlayFacts>, LibraryError> {
        Ok(self.library.play_facts(tracks)?)
    }

    /// The tracks `tracks`, in that order, as lists show them.
    pub fn summaries_of(
        &self,
        tracks: &[TrackId],
    ) -> Result<Vec<super::TrackSummary>, LibraryError> {
        Ok(self.library.summaries_of(tracks)?)
    }

    /// What the picks know of `tracks`.
    pub fn track_facts(&self, tracks: &[TrackId]) -> Result<Vec<TrackFacts>, LibraryError> {
        Ok(self.library.track_facts(tracks)?)
    }

    /// The tracks the query `text` finds; `None` for empty text (every track).
    pub fn tracks_matching(&self, text: &str) -> Result<Option<HashSet<TrackId>>, LibraryError> {
        Ok(self.library.tracks_matching(text)?)
    }

    /// Up to `count` tracks related to `seed`, for endless play; see
    /// [`helikodu_library::Library::related_tracks`].
    pub fn related_tracks(
        &self,
        seed: PlaySeed,
        settings: &EndlessSettings,
        avoid: &HashSet<TrackId>,
        count: usize,
        seed_value: u64,
        sound: &dyn SoundSimilarity,
    ) -> Result<Vec<TrackId>, LibraryError> {
        Ok(self
            .library
            .related_tracks(seed, settings, avoid, count, seed_value, sound)?)
    }

    /// Up to `count` tracks for the Auto-DJ; see
    /// [`helikodu_library::Library::auto_dj_tracks`].
    pub fn auto_dj_tracks(
        &self,
        settings: &AutoDjSettings,
        avoid: &HashSet<TrackId>,
        count: usize,
        seed_value: u64,
    ) -> Result<Vec<TrackId>, LibraryError> {
        Ok(self
            .library
            .auto_dj_tracks(settings, avoid, count, seed_value)?)
    }

    /// Every generator playlist, by name.
    pub fn generators(&self) -> Result<Vec<Generator>, LibraryError> {
        Ok(self.library.generators()?)
    }

    /// The generator named or numbered `name`.
    pub fn find_generator(&self, name: &str) -> Result<Option<Generator>, LibraryError> {
        Ok(self.library.find_generator(name)?)
    }

    /// Stores a new generator playlist.
    pub fn create_generator(
        &self,
        name: &str,
        steps: &GeneratorSteps,
        on_open: bool,
    ) -> Result<GeneratorId, LibraryError> {
        Ok(self.library.create_generator(name, steps, on_open)?)
    }

    /// Changes the name, the steps or `on_open` of a generator.
    pub fn update_generator(
        &self,
        id: GeneratorId,
        name: Option<&str>,
        steps: Option<&GeneratorSteps>,
        on_open: Option<bool>,
    ) -> Result<(), LibraryError> {
        Ok(self.library.update_generator(id, name, steps, on_open)?)
    }

    /// Removes a generator; whether it was there.
    pub fn remove_generator(&self, id: GeneratorId) -> Result<bool, LibraryError> {
        Ok(self.library.remove_generator(id)?)
    }

    /// Runs steps without storing anything.
    pub fn preview_generator(
        &self,
        steps: &GeneratorSteps,
        seed: u64,
    ) -> Result<Vec<TrackId>, LibraryError> {
        Ok(self.library.preview_generator(steps, seed)?)
    }

    /// Runs a generator again and stores its result.
    pub fn regenerate(&self, id: GeneratorId, seed: u64) -> Result<Vec<TrackId>, LibraryError> {
        Ok(self.library.regenerate(id, seed)?)
    }

    /// The tracks of a generator as opening it shows them.
    pub fn open_generator(&self, id: GeneratorId, seed: u64) -> Result<Vec<TrackId>, LibraryError> {
        Ok(self.library.open_generator(id, seed)?)
    }
}
