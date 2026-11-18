// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The events of the whole service, and what a subscriber can ask for.

use std::fmt;
use std::time::Duration;

use helikodu_model::audio::MediaTime;

use crate::bus::{EventBus, Subscription, Waker};
use crate::clock::SharedClock;
use crate::library::LibraryEvent;
use crate::player::PlayerEvent;
use crate::settings::SettingsEvent;

/// Everything the service announces.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// From the player.
    Player(PlayerEvent),
    /// From the settings store.
    Settings(SettingsEvent),
    /// From the library service.
    Library(LibraryEvent),
}

/// What a subscriber wants: which kinds of events, how often positions, and how it
/// is woken.
#[derive(Clone)]
pub struct SubscribeOptions {
    /// Player events.
    pub player: bool,
    /// Settings events.
    pub settings: bool,
    /// Library events.
    pub library: bool,
    /// At most one position per this interval (the window wants 4 to 10 a second;
    /// it interpolates between them). Positions after a seek, at a new track or
    /// after a pause always pass. `None` passes every position the engine sends.
    pub position_interval: Option<Duration>,
    /// Called after each event is queued (see [`Waker`]).
    pub waker: Option<Waker>,
}

impl SubscribeOptions {
    /// Every event, every position, no waker.
    pub fn all() -> Self {
        Self {
            player: true,
            settings: true,
            library: true,
            position_interval: None,
            waker: None,
        }
    }

    /// Only the player's events.
    pub fn player() -> Self {
        Self {
            settings: false,
            library: false,
            ..Self::all()
        }
    }

    /// The same options with positions at most every `interval`.
    #[must_use]
    pub fn with_position_interval(self, interval: Duration) -> Self {
        Self {
            position_interval: Some(interval),
            ..self
        }
    }

    /// The same options with `waker`.
    #[must_use]
    pub fn with_waker(self, waker: Waker) -> Self {
        Self {
            waker: Some(waker),
            ..self
        }
    }

    fn wants(&self, event: &Event) -> bool {
        match event {
            Event::Player(_) => self.player,
            Event::Settings(_) => self.settings,
            Event::Library(_) => self.library,
        }
    }
}

impl Default for SubscribeOptions {
    fn default() -> Self {
        Self::all()
    }
}

impl fmt::Debug for SubscribeOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscribeOptions")
            .field("player", &self.player)
            .field("settings", &self.settings)
            .field("library", &self.library)
            .field("position_interval", &self.position_interval)
            .field("waker", &self.waker.is_some())
            .finish()
    }
}

/// Subscribes to `bus` with `options`, mapping each wanted event with `map`.
pub(crate) fn subscribe<T: Send + 'static>(
    bus: &EventBus<Event>,
    clock: SharedClock,
    options: SubscribeOptions,
    map: fn(&Event) -> Option<T>,
) -> Subscription<T> {
    let mut throttle = options
        .position_interval
        .map(|interval| PositionThrottle::new(interval, clock));
    let waker = options.waker.clone();
    bus.subscribe_with(
        move |event: &Event| {
            if !options.wants(event) {
                return None;
            }
            if let (Some(throttle), Event::Player(player_event)) = (&mut throttle, event)
                && !throttle.pass(player_event)
            {
                return None;
            }
            map(event)
        },
        waker,
    )
}

/// Coalesces position events: one per interval, except where the position is not
/// where interpolation would put it.
pub(crate) struct PositionThrottle {
    interval: Duration,
    clock: SharedClock,
    /// The last position passed: its entry index, the position and when.
    last: Option<(usize, MediaTime, Duration)>,
}

/// How far a position may be from where interpolation puts it and still be
/// dropped.
const DRIFT: Duration = Duration::from_millis(400);

impl PositionThrottle {
    pub(crate) fn new(interval: Duration, clock: SharedClock) -> Self {
        Self {
            interval,
            clock,
            last: None,
        }
    }

    /// Whether `event` goes to the subscriber.
    pub(crate) fn pass(&mut self, event: &PlayerEvent) -> bool {
        let now = self.clock.elapsed();
        match event {
            PlayerEvent::Position {
                index, position, ..
            } => {
                let pass = match self.last {
                    None => true,
                    Some((last_index, last_position, at)) => {
                        let since = now.saturating_sub(at);
                        let expected = last_position.as_duration() + since;
                        let actual = position.as_duration();
                        let drift = expected.abs_diff(actual);
                        last_index != *index || since >= self.interval || drift > DRIFT
                    }
                };
                if pass {
                    self.last = Some((*index, *position, now));
                }
                pass
            }
            // After a state change (a pause, a seek) the next position always passes.
            PlayerEvent::State { .. } | PlayerEvent::TrackChanged { .. } => {
                self.last = None;
                true
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use helikodu_model::player::{PlaybackState, TransportState};

    fn position(index: usize, millis: u64) -> PlayerEvent {
        PlayerEvent::Position {
            index,
            position: MediaTime::from_millis(millis),
            length: None,
        }
    }

    #[test]
    fn positions_are_thinned_but_jumps_pass() {
        let clock = FakeClock::new(std::time::SystemTime::UNIX_EPOCH);
        let mut throttle = PositionThrottle::new(Duration::from_millis(250), clock.shared());
        let mut passed = Vec::new();
        for step in 0..10u64 {
            let event = position(0, step * 100);
            if throttle.pass(&event) {
                passed.push(step * 100);
            }
            clock.advance(Duration::from_millis(100));
        }
        assert_eq!(passed, vec![0, 300, 600, 900]);
        // A seek: the position is far from where it would be.
        assert!(throttle.pass(&position(0, 30_000)));
        // Another track.
        assert!(throttle.pass(&position(1, 0)));
        clock.advance(Duration::from_millis(100));
        assert!(!throttle.pass(&position(1, 100)));
        // After a pause the next one passes.
        assert!(throttle.pass(&PlayerEvent::State {
            playback: PlaybackState::Paused,
            transport: TransportState::Paused
        }));
        assert!(throttle.pass(&position(1, 120)));
    }
}
