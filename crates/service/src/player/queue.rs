// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The play queue: files in the order they play.

use std::path::{Path, PathBuf};

/// Which way playback moves through the queue, and so which way an unplayable file
/// is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Forward,
    Backward,
}

/// An ordered list of files.
#[derive(Debug, Clone, Default)]
pub(super) struct Queue {
    files: Vec<PathBuf>,
}

impl Queue {
    pub(super) fn new(files: Vec<PathBuf>) -> Self {
        Self { files }
    }

    pub(super) fn len(&self) -> usize {
        self.files.len()
    }

    pub(super) fn get(&self, index: usize) -> Option<&Path> {
        self.files.get(index).map(PathBuf::as_path)
    }

    /// The index one step from `index` in `direction`, if there is one.
    pub(super) fn step(&self, index: usize, direction: Direction) -> Option<usize> {
        let to = match direction {
            Direction::Forward => index.checked_add(1)?,
            Direction::Backward => index.checked_sub(1)?,
        };
        (to < self.len()).then_some(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_stay_inside() {
        let queue = Queue::new(vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(queue.step(0, Direction::Forward), Some(1));
        assert_eq!(queue.step(2, Direction::Forward), None);
        assert_eq!(queue.step(2, Direction::Backward), Some(1));
        assert_eq!(queue.step(0, Direction::Backward), None);
        assert_eq!(queue.get(1), Some(Path::new("b")));
        assert_eq!(queue.get(3), None);
        assert_eq!(Queue::default().step(0, Direction::Forward), None);
    }
}
