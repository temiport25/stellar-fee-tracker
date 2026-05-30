//! Property-style tests for spike classifier — no false positives on flat sequences.

use stellar_devkit::analysis::spike_classifier::SpikeClassifier;

/// Helper: flat sequence of identical fees.
fn flat(fee: u64, n: usize) -> Vec<u64> {
    vec![fee; n]
}

#[test]
fn no_false_positives_flat_sequence_various_baselines() {
    for baseline in [100u64, 500, 1000, 10_000] {
        let fees = flat(baseline, 50);
        let events = SpikeClassifier::detect(&fees, baseline);
        assert!(
            events.is_empty(),
            "Expected no spikes for flat sequence at baseline {baseline}, got {:?}",
            events
        );
    }
}

#[test]
fn no_false_positives_fees_equal_baseline_single_element() {
    let events = SpikeClassifier::detect(&[100], 100);
    assert!(events.is_empty());
}

#[test]
fn no_false_positives_fees_just_below_threshold() {
    // 1.99× baseline should not trigger (threshold is 2×)
    let fees = vec![199u64; 10];
    let events = SpikeClassifier::detect(&fees, 100);
    assert!(events.is_empty());
}

#[test]
fn no_false_positives_empty_slice() {
    assert!(SpikeClassifier::detect(&[], 100).is_empty());
}

#[test]
fn spike_detected_when_fee_exceeds_threshold() {
    // 2× baseline should trigger at least one event
    let fees = vec![100u64, 200, 100];
    let events = SpikeClassifier::detect(&fees, 100);
    assert!(!events.is_empty());
}