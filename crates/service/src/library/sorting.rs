// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The library sorted for the interface language as the `[appearance] language`
//! setting changes: the sort indexes are built again in the background, one
//! change at a time.

use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread::{self, JoinHandle};

use helikodu_model::settings::SettingsGroup;

use super::service::LibraryEvent;
use super::{LibraryError, MusicLibrary, location};
use crate::Event;
use crate::bus::{EventBus, Subscription};
use crate::settings::SettingsEvent;

impl MusicLibrary {
    /// Sorts the library for the interface language `language` (a locale name
    /// such as `de`; empty for the environment's locale), building the sort
    /// indexes again if they were built for another. Blocks until done.
    pub fn sort_for(&self, language: &str) -> Result<(), LibraryError> {
        Ok(self.inner().set_collation(location::collation(language))?)
    }
}

/// Follows the `[appearance] language` setting, sorting the library for each
/// new language in the background and announcing [`LibraryEvent::Changed`].
pub(crate) struct LanguageSorter {
    inner: Arc<Inner>,
    // Kept for as long as the sorter lives; in a mutex, as a receiver is not `Sync`.
    _subscription: Mutex<Subscription<()>>,
}

struct Inner {
    library: MusicLibrary,
    bus: EventBus<Event>,
    state: Mutex<State>,
}

struct State {
    /// The language asked for last, as the setting names it.
    wanted: String,
    running: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for LanguageSorter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanguageSorter").finish_non_exhaustive()
    }
}

impl LanguageSorter {
    /// Starts following the settings events on `bus`, for a library sorted for
    /// `language`.
    pub(crate) fn start(library: MusicLibrary, bus: EventBus<Event>, language: &str) -> Self {
        let inner = Arc::new(Inner {
            library,
            bus: bus.clone(),
            state: Mutex::new(State {
                wanted: language.trim().to_owned(),
                running: None,
            }),
        });
        let weak: Weak<Inner> = Arc::downgrade(&inner);
        let subscription = bus.subscribe_with(
            move |event: &Event| {
                if let Event::Settings(SettingsEvent::Changed { groups, settings }) = event
                    && groups.contains(&SettingsGroup::Appearance)
                    && let Some(inner) = weak.upgrade()
                {
                    Inner::sort_for(&inner, settings.appearance.language.trim());
                }
                None
            },
            None,
        );
        Self {
            inner,
            _subscription: Mutex::new(subscription),
        }
    }

    /// Waits for a sort that is running.
    pub(crate) fn stop(&self) {
        let running = lock(&self.inner.state).running.take();
        if let Some(running) = running {
            let _ = running.join();
        }
    }
}

impl Inner {
    fn sort_for(this: &Arc<Self>, language: &str) {
        let mut state = lock(&this.state);
        if state.wanted == language {
            return;
        }
        state.wanted = language.to_owned();
        let previous = state.running.take();
        let inner = Arc::clone(this);
        let language = language.to_owned();
        let spawned = thread::Builder::new()
            .name("helikodu-sorting".into())
            .spawn(move || {
                if let Some(previous) = previous {
                    let _ = previous.join();
                }
                // A later change replaces this one.
                if lock(&inner.state).wanted != language {
                    return;
                }
                match inner.library.sort_for(&language) {
                    Ok(()) => inner.bus.publish(&Event::Library(LibraryEvent::Changed)),
                    Err(error) => log::warn!("cannot sort the library for {language:?}: {error}"),
                }
            });
        match spawned {
            Ok(handle) => state.running = Some(handle),
            Err(error) => log::warn!("cannot sort the library again: {error}"),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use helikodu_model::settings::Settings;

    use super::*;

    fn change(bus: &EventBus<Event>, groups: Vec<SettingsGroup>, language: &str) {
        let mut settings = Settings::default();
        settings.appearance.language = language.into();
        bus.publish(&Event::Settings(SettingsEvent::Changed {
            groups,
            settings: Arc::new(settings),
        }));
    }

    #[test]
    fn a_new_language_sorts_the_library_again() {
        let library = MusicLibrary::in_memory().unwrap();
        let bus = EventBus::new();
        let changed = bus.subscribe_with(
            |e: &Event| matches!(e, Event::Library(LibraryEvent::Changed)).then_some(()),
            None,
        );
        let sorter = LanguageSorter::start(library.clone(), bus.clone(), "");
        // The language the library is sorted for already, and other groups.
        change(&bus, vec![SettingsGroup::Appearance], "");
        change(&bus, vec![SettingsGroup::Playback], "sv");
        sorter.stop();
        assert!(changed.try_recv().is_err());
        // A new language: sorted again in the background, then announced.
        change(&bus, vec![SettingsGroup::Appearance], "sv");
        assert!(changed.recv_timeout(Duration::from_secs(10)).is_ok());
        change(&bus, vec![SettingsGroup::Appearance], " sv ");
        sorter.stop();
        assert!(changed.try_recv().is_err());
        // Sorting for a language the library is sorted for changes nothing.
        library.sort_for("sv").unwrap();
    }
}
