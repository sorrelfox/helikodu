// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A small seeded random number generator and a Zipf-like sampler.
//!
//! The generator is SplitMix64: the same seed gives the same numbers on every
//! machine and with every Rust version, which a library generator needs and an
//! external crate does not promise. Floating point is used only for `+`, `*`
//! and `/`, which are exact to the last bit everywhere.

/// SplitMix64.
#[derive(Debug, Clone)]
pub(crate) struct Rng {
    state: u64,
}

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..bound`; `bound` must not be 0.
    pub(crate) fn below(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        // Lemire's multiply-shift; the bias is far below anything that matters here.
        ((u128::from(self.next_u64()) * bound as u128) >> 64) as usize
    }

    /// A number in `low..=high`.
    pub(crate) fn range(&mut self, low: u32, high: u32) -> u32 {
        low + self.below((high - low) as usize + 1) as u32
    }

    /// A number in `[0, 1)`.
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// True with probability `p`.
    pub(crate) fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// One of `items`, chosen with the given integer weights.
    pub(crate) fn weighted<T: Copy>(&mut self, items: &[(T, u32)]) -> T {
        let total: u32 = items.iter().map(|(_, weight)| weight).sum();
        let mut point = self.below(total as usize) as u32;
        for &(item, weight) in items {
            if point < weight {
                return item;
            }
            point -= weight;
        }
        items[items.len() - 1].0
    }

    /// A random 128-bit value formatted as a UUID (version 4 layout).
    pub(crate) fn uuid(&mut self) -> String {
        let high = self.next_u64();
        let low = self.next_u64();
        let high = (high & !0xf000) | 0x4000;
        let low = (low & !(0xc << 60)) | (0x8 << 60);
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            high >> 32,
            (high >> 16) & 0xffff,
            high & 0xffff,
            low >> 48,
            low & 0xffff_ffff_ffff
        )
    }
}

/// Samples ranks `0..n` with weight `1 / (rank + 1 + offset)`: a Zipf–Mandelbrot
/// law with exponent 1. The offset flattens the head, so the most popular item
/// gets a few per cent rather than a third of everything.
#[derive(Debug)]
pub(crate) struct Zipf {
    cumulative: Vec<f64>,
}

impl Zipf {
    pub(crate) fn new(n: usize, offset: f64) -> Self {
        let mut total = 0.0;
        let cumulative = (0..n.max(1))
            .map(|rank| {
                total += 1.0 / (rank as f64 + 1.0 + offset);
                total
            })
            .collect();
        Self { cumulative }
    }

    pub(crate) fn sample(&self, rng: &mut Rng) -> usize {
        let total = self.cumulative[self.cumulative.len() - 1];
        let point = rng.unit() * total;
        self.cumulative
            .partition_point(|&edge| edge <= point)
            .min(self.cumulative.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_numbers() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let mut c = Rng::new(8);
        let first: Vec<u64> = (0..16).map(|_| a.next_u64()).collect();
        let second: Vec<u64> = (0..16).map(|_| b.next_u64()).collect();
        let third: Vec<u64> = (0..16).map(|_| c.next_u64()).collect();
        assert_eq!(first, second);
        assert_ne!(first, third);
    }

    #[test]
    fn known_splitmix_output() {
        // Reference values of SplitMix64 for seed 0.
        let mut rng = Rng::new(0);
        assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    }

    #[test]
    fn ranges_stay_in_bounds() {
        let mut rng = Rng::new(1);
        for _ in 0..10_000 {
            let value = rng.range(3, 9);
            assert!((3..=9).contains(&value));
            let unit = rng.unit();
            assert!((0.0..1.0).contains(&unit));
        }
    }

    #[test]
    fn zipf_favours_the_head() {
        let zipf = Zipf::new(1000, 8.0);
        let mut rng = Rng::new(3);
        let mut counts = vec![0u32; 1000];
        for _ in 0..100_000 {
            counts[zipf.sample(&mut rng)] += 1;
        }
        let head: u32 = counts[..10].iter().sum();
        let tail: u32 = counts[990..].iter().sum();
        assert!(head > 8 * tail, "head {head}, tail {tail}");
        // Exponent 1 with offset 8: the first rank gets about 1/9 / ln(1009/9) ≈ 2.4 %.
        assert!((1500..3500).contains(&counts[0]), "{}", counts[0]);
    }

    #[test]
    fn uuid_layout() {
        let uuid = Rng::new(5).uuid();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert!(matches!(&uuid[19..20], "8" | "9" | "a" | "b"));
    }
}
