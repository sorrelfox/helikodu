// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The random numbers of shuffling: SplitMix64, seeded once per player. Shuffle
//! orders are stored, not recomputed, so the generator needs no more than to be
//! even and fast.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// A generator seeded from the time and the process.
    pub(crate) fn from_entropy() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Self(nanos ^ (u64::from(std::process::id()) << 32))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number from 0 to `bound - 1`; 0 when `bound` is 0.
    pub(crate) fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        // The bias of the modulo is far below anything a listener could notice.
        (self.next_u64() % bound as u64) as usize
    }

    /// A seed for another generator.
    pub(crate) fn next_seed(&mut self) -> u64 {
        self.next_u64()
    }

    /// A number above 0 and below 1.
    pub(crate) fn unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Orders `items` at random with each one's chance to come early as large as
    /// `weight` says (Efraimidis and Spirakis): a weighted draw without putting back.
    /// Weights at or below 0 count as the smallest positive weight.
    pub(crate) fn weighted_order<T>(&mut self, items: &mut Vec<T>, weight: impl Fn(&T) -> f64) {
        let mut keyed: Vec<(f64, T)> = items
            .drain(..)
            .map(|item| {
                let w = weight(&item);
                let w = if w > 0.0 && w.is_finite() {
                    w
                } else {
                    f64::MIN_POSITIVE
                };
                (self.unit().ln() / w, item)
            })
            .collect();
        keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
        items.extend(keyed.into_iter().map(|(_, item)| item));
    }

    /// Shuffles `items` in place (Fisher–Yates).
    pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffles_are_permutations_and_vary() {
        let mut rng = Rng::new(42);
        let mut items: Vec<u32> = (0..50).collect();
        rng.shuffle(&mut items);
        let mut sorted = items.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        assert_ne!(items, sorted);
        assert_eq!(rng.below(0), 0);
        assert!((0..100).all(|_| rng.below(3) < 3));
    }

    #[test]
    fn weighted_orders_put_heavy_items_early() {
        let mut rng = Rng::new(9);
        let mut early = 0;
        for _ in 0..200 {
            let mut items: Vec<u32> = (0..10).collect();
            rng.weighted_order(&mut items, |i| if *i == 7 { 20.0 } else { 1.0 });
            assert_eq!(items.len(), 10);
            if items[..3].contains(&7) {
                early += 1;
            }
        }
        assert!(early > 180, "{early}");
        let mut a: Vec<u32> = (0..20).collect();
        let mut b = a.clone();
        Rng::new(1).weighted_order(&mut a, |_| 1.0);
        Rng::new(1).weighted_order(&mut b, |_| 1.0);
        assert_eq!(a, b);
    }
}
