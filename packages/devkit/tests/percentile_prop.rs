//! Property-style tests for percentile functions.
//! Verifies invariants on arbitrary fee sequences without external proptest crate.

use stellar_devkit::analysis::percentile::Percentile;

/// Helper: generate a sorted sequence of length n starting at base with step.
fn sorted_seq(base: u64, step: u64, n: usize) -> Vec<u64> {
    (0..n).map(|i| base + i as u64 * step).collect()
}

#[test]
fn p0_always_returns_minimum() {
    for base in [1u64, 10, 100] {
        let data = sorted_seq(base, 1, 20);
        assert_eq!(Percentile::nearest_rank(&data, 1), base);
    }
}

#[test]
fn p100_always_returns_maximum() {
    for n in [5usize, 10, 50] {
        let data = sorted_seq(1, 1, n);
        assert_eq!(Percentile::nearest_rank(&data, 100), n as u64);
    }
}

#[test]
fn percentile_is_monotone_increasing() {
    let data: Vec<u64> = (1..=100).collect();
    let p25 = Percentile::nearest_rank(&data, 25);
    let p50 = Percentile::nearest_rank(&data, 50);
    let p75 = Percentile::nearest_rank(&data, 75);
    assert!(p25 <= p50);
    assert!(p50 <= p75);
}

#[test]
fn uniform_sequence_all_percentiles_equal() {
    let data = vec![42u64; 20];
    for p in [10, 25, 50, 75, 90, 99] {
        assert_eq!(Percentile::nearest_rank(&data, p), 42);
    }
}

#[test]
fn interpolation_p50_of_two_elements_is_midpoint() {
    for (a, b) in [(10u64, 20), (0, 100), (5, 15)] {
        let data = vec![a, b];
        let mid = Percentile::linear_interpolation(&data, 50);
        assert_eq!(mid, (a + b) / 2);
    }
}
