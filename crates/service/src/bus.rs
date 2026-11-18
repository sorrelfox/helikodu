// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The event bus: one way for every front end to follow the service.
//!
//! The service publishes each [`Event`](crate::Event) once; every subscriber gets it
//! on its own channel, filtered and coalesced as it asked
//! ([`SubscribeOptions`](crate::SubscribeOptions)). Events from one publisher (the
//! player's actor, the settings store, the library service) arrive in the order they
//! were published.
//!
//! A front end with a main loop attaches a [`Waker`]: it is called after each event
//! is queued for that subscriber, from the publishing thread, and should only wake
//! the main loop (a GLib front end calls `MainContext::wakeup` or schedules an idle
//! source that drains [`Subscription::try_iter`]). A command-line tool simply blocks
//! on [`Subscription::recv`].
//!
//! A subscription ends when it is dropped; the bus forgets it at its next event.

use std::fmt;
use std::ops::Deref;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};

/// Called after an event was queued for a subscriber. It runs on the publishing
/// thread while the bus is locked, so it must return at once and must not publish.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// One subscriber: takes an event, and says whether it still wants more.
trait Subscriber<E>: Send {
    fn deliver(&mut self, event: &E) -> bool;
}

/// A subscriber that filters and maps events onto a channel.
struct Channel<T, F> {
    filter: F,
    sender: Sender<T>,
    waker: Option<Waker>,
}

impl<E, T, F> Subscriber<E> for Channel<T, F>
where
    T: Send,
    F: FnMut(&E) -> Option<T> + Send,
{
    fn deliver(&mut self, event: &E) -> bool {
        let Some(value) = (self.filter)(event) else {
            return true;
        };
        if self.sender.send(value).is_err() {
            return false;
        }
        if let Some(waker) = &self.waker {
            waker();
        }
        true
    }
}

/// Publishes events to every subscriber. Clones publish to the same subscribers.
pub struct EventBus<E> {
    subscribers: Arc<Mutex<Vec<Box<dyn Subscriber<E>>>>>,
}

impl<E> Clone for EventBus<E> {
    fn clone(&self) -> Self {
        Self {
            subscribers: Arc::clone(&self.subscribers),
        }
    }
}

impl<E> fmt::Debug for EventBus<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventBus")
            .field("subscribers", &self.subscriber_count())
            .finish()
    }
}

impl<E> Default for EventBus<E> {
    fn default() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl<E> EventBus<E> {
    /// A bus without subscribers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sends `event` to every subscriber that wants it.
    pub fn publish(&self, event: &E) {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subscribers.retain_mut(|subscriber| subscriber.deliver(event));
    }

    /// Subscribes with `filter`, which picks (and may change) the events this
    /// subscriber gets. It runs on the publishing thread with the bus locked.
    pub fn subscribe_with<T, F>(&self, filter: F, waker: Option<Waker>) -> Subscription<T>
    where
        E: 'static,
        T: Send + 'static,
        F: FnMut(&E) -> Option<T> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(Channel {
                filter,
                sender,
                waker,
            }));
        Subscription { receiver }
    }

    /// The number of subscribers, including those dropped since the last event.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// The receiving end of a subscription. It derefs to the channel's
/// [`Receiver`], so `recv`, `recv_timeout`, `try_recv`, `iter` and `try_iter` work
/// as on any channel. Dropping it ends the subscription.
#[derive(Debug)]
pub struct Subscription<T> {
    receiver: Receiver<T>,
}

impl<T> Subscription<T> {
    /// The channel.
    pub fn receiver(&self) -> &Receiver<T> {
        &self.receiver
    }

    /// The channel, for a front end that wants to own it.
    pub fn into_receiver(self) -> Receiver<T> {
        self.receiver
    }
}

impl<T> Deref for Subscription<T> {
    type Target = Receiver<T>;

    fn deref(&self) -> &Receiver<T> {
        &self.receiver
    }
}

impl<'a, T> IntoIterator for &'a Subscription<T> {
    type Item = T;
    type IntoIter = mpsc::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.receiver.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn every_subscriber_gets_what_it_asked_for() {
        let bus = EventBus::<u32>::new();
        let all = bus.subscribe_with(|n: &u32| Some(*n), None);
        let even = bus.subscribe_with(|n: &u32| n.is_multiple_of(2).then(|| n.to_string()), None);
        for n in 1..=4 {
            bus.publish(&n);
        }
        assert_eq!(all.try_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(even.try_iter().collect::<Vec<_>>(), vec!["2", "4"]);
    }

    #[test]
    fn dropped_subscriptions_are_forgotten() {
        let bus = EventBus::<u32>::new();
        let kept = bus.subscribe_with(|n: &u32| Some(*n), None);
        drop(bus.subscribe_with(|n: &u32| Some(*n), None));
        assert_eq!(bus.subscriber_count(), 2);
        bus.publish(&1);
        assert_eq!(bus.subscriber_count(), 1);
        assert_eq!(kept.recv().unwrap(), 1);
    }

    #[test]
    fn the_waker_runs_after_each_queued_event() {
        let bus = EventBus::<u32>::new();
        let woken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&woken);
        let waker: Waker = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let odd = bus.subscribe_with(|n: &u32| (!n.is_multiple_of(2)).then_some(*n), Some(waker));
        for n in 1..=5 {
            bus.publish(&n);
        }
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        assert_eq!(
            (&odd).into_iter().take(3).collect::<Vec<_>>(),
            vec![1, 3, 5]
        );
    }
}
