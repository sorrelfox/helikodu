// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Backends that the program running the engine brings itself.

use std::fmt;
use std::sync::Arc;

use helikodu_model::audio::OutputBackendKind;

use super::{EventSender, OutputBackend, OutputError};

/// Connects backends in place of the built-in ones: for tests that script a device
/// going away, and for programs with an output of their own.
pub trait BackendConnector: Send + Sync + fmt::Debug {
    /// Connects to the backend of `kind`, which reports its events on `events`.
    /// `None` leaves `kind` to the built-in backend.
    fn connect(
        &self,
        kind: OutputBackendKind,
        events: &EventSender,
    ) -> Option<Result<Box<dyn OutputBackend>, OutputError>>;
}

/// A [`BackendConnector`] shared by the engine and its owner. The default has none:
/// every backend is the built-in one.
#[derive(Clone, Default)]
pub struct SharedConnector(Option<Arc<dyn BackendConnector>>);

impl SharedConnector {
    /// Shares `connector`.
    pub fn new(connector: impl BackendConnector + 'static) -> Self {
        Self(Some(Arc::new(connector)))
    }

    /// Asks the connector for a backend of `kind`; `None` when it has none.
    pub fn connect(
        &self,
        kind: OutputBackendKind,
        events: &EventSender,
    ) -> Option<Result<Box<dyn OutputBackend>, OutputError>> {
        self.0.as_ref()?.connect(kind, events)
    }
}

impl PartialEq for SharedConnector {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl fmt::Debug for SharedConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SharedConnector").field(&self.0).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::output::NullBackend;

    #[derive(Debug)]
    struct OnlyNull;

    impl BackendConnector for OnlyNull {
        fn connect(
            &self,
            kind: OutputBackendKind,
            _events: &EventSender,
        ) -> Option<Result<Box<dyn OutputBackend>, OutputError>> {
            (kind == OutputBackendKind::Jack).then(|| {
                let backend: Box<dyn OutputBackend> = Box::new(NullBackend::new());
                Ok(backend)
            })
        }
    }

    #[test]
    fn connects_only_what_it_serves() {
        let (events, _) = mpsc::channel();
        assert!(
            SharedConnector::default()
                .connect(OutputBackendKind::Jack, &events)
                .is_none()
        );
        let shared = SharedConnector::new(OnlyNull);
        assert!(shared.connect(OutputBackendKind::Alsa, &events).is_none());
        let backend = shared
            .connect(OutputBackendKind::Jack, &events)
            .unwrap()
            .unwrap();
        assert_eq!(backend.kind(), OutputBackendKind::Null);
        assert_eq!(shared, shared.clone());
        assert_ne!(shared, SharedConnector::new(OnlyNull));
        assert_eq!(SharedConnector::default(), SharedConnector::default());
    }
}
