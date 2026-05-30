//! Test helpers: deterministic fee sequence generator and SQLite fixture builder.

use rand::{Rng, SeedableRng};
use rand::rngs::SmallRng;

/// Generates a deterministic fee sequence from a seed for repeatable tests.
pub struct FeeGenerator {
    rng: SmallRng,
}

impl FeeGenerator {
    /// Create a generator with the given seed.
    pub fn new(seed: u64) -> Self {
        Self { rng: SmallRng::seed_from_u64(seed) }
    }

    /// Generate `n` fee values in the range [min_fee, max_fee].
    pub fn generate(&mut self, n: usize, min_fee: u64, max_fee: u64) -> Vec<u64> {
        (0..n).map(|_| self.rng.gen_range(min_fee..=max_fee)).collect()
    }

    /// Generate a flat sequence of `n` identical fees (useful for baseline tests).
    pub fn flat(fee: u64, n: usize) -> Vec<u64> {
        vec![fee; n]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_produces_same_sequence() {
        let a = FeeGenerator::new(42).generate(10, 100, 1000);
        let b = FeeGenerator::new(42).generate(10, 100, 1000);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_produce_different_sequences() {
        let a = FeeGenerator::new(1).generate(10, 100, 1000);
        let b = FeeGenerator::new(2).generate(10, 100, 1000);
        assert_ne!(a, b);
    }

    #[test]
    fn flat_returns_uniform_sequence() {
        assert_eq!(FeeGenerator::flat(500, 5), vec![500u64; 5]);
    }
}