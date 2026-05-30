use crate::simulation::fee_model::{FeeModel, FeeModelConfig};
use crate::types::FeeRecord;

/// Returns a deterministic fee sequence of `count` records seeded by `seed`.
pub fn make_fee_sequence(count: usize, seed: u64) -> Vec<FeeRecord> {
    let config = FeeModelConfig {
        seed: Some(seed),
        ..Default::default()
    };
    FeeModel::new(config).generate(count, 0)
}

/// Returns a fee sequence where every record is flagged as a spike.
pub fn make_spike_sequence(count: usize) -> Vec<FeeRecord> {
    let config = FeeModelConfig {
        spike_probability: 1.0,
        seed: Some(0),
        ..Default::default()
    };
    FeeModel::new(config).generate(count, 0)
}

/// Returns a fee sequence with no spikes (baseline load only).
pub fn make_baseline_sequence(count: usize) -> Vec<FeeRecord> {
    let config = FeeModelConfig {
        spike_probability: 0.0,
        seed: Some(1),
        ..Default::default()
    };
    FeeModel::new(config).generate(count, 0)
}
﻿//! Test helpers: deterministic fee sequence generator and SQLite fixture builder.

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

/// A simple in-memory fee record for fixture use.
#[derive(Debug, Clone, PartialEq)]
pub struct FeeRecord {
    pub timestamp: u64,
    pub fee_amount: u64,
    pub ledger_sequence: u64,
    pub tx_hash: String,
}

/// Builds a vec of FeeRecord fixtures for testing.
pub struct FixtureBuilder;

impl FixtureBuilder {
    /// Build `n` sequential fee records starting at `base_timestamp`.
    pub fn build(n: usize, base_timestamp: u64, base_fee: u64) -> Vec<FeeRecord> {
        (0..n)
            .map(|i| FeeRecord {
                timestamp: base_timestamp + i as u64,
                fee_amount: base_fee,
                ledger_sequence: 1000 + i as u64,
                tx_hash: format!("txhash_{i:04}"),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_builder_produces_correct_count() {
        let records = FixtureBuilder::build(5, 0, 100);
        assert_eq!(records.len(), 5);
    }

    #[test]
    fn fixture_builder_sequential_timestamps() {
        let records = FixtureBuilder::build(3, 1000, 100);
        assert_eq!(records[0].timestamp, 1000);
        assert_eq!(records[2].timestamp, 1002);
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
