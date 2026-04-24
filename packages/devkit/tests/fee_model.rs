use stellar_devkit::simulation::fee_model::{FeeModel, FeeModelConfig};

/// Assert spike rate is within 10% of configured probability over 10,000 samples.
#[test]
fn spike_rate_within_10_percent_of_configured_probability() {
    let spike_probability = 0.05;
    let config = FeeModelConfig {
        spike_probability,
        seed: Some(42),
        ..Default::default()
    };
    let mut model = FeeModel::new(config);
    let points = model.generate(10_000, 0);

    let spike_count = points.iter().filter(|p| p.is_spike).count();
    let actual_rate = spike_count as f64 / 10_000.0;

    let tolerance = spike_probability * 0.10;
    assert!(
        (actual_rate - spike_probability).abs() <= tolerance,
        "spike rate {actual_rate:.4} not within 10% of {spike_probability}"
    );
}

/// Assert spike ledgers carry the multiplied fee and non-spike ledgers carry base fee.
#[test]
fn spike_fee_equals_base_times_multiplier() {
    let config = FeeModelConfig {
        base_fee: 100,
        spike_multiplier: 10,
        spike_probability: 1.0, // force all spikes
        seed: Some(1),
        ..Default::default()
    };
    let mut model = FeeModel::new(config);
    let points = model.generate(5, 0);
    for p in &points {
        assert!(p.is_spike);
        assert_eq!(p.fee, 1_000);
    }
}

/// Assert timestamps are spaced by ledger_interval_secs.
#[test]
fn timestamps_spaced_by_ledger_interval() {
    let config = FeeModelConfig {
        ledger_interval_secs: 5,
        seed: Some(0),
        ..Default::default()
    };
    let mut model = FeeModel::new(config);
    let points = model.generate(4, 1_000);
    assert_eq!(points[0].timestamp, 1_000);
    assert_eq!(points[1].timestamp, 1_005);
    assert_eq!(points[2].timestamp, 1_010);
    assert_eq!(points[3].timestamp, 1_015);
}

/// Assert seeded runs are deterministic.
#[test]
fn seeded_run_is_deterministic() {
    let make = || {
        let config = FeeModelConfig {
            seed: Some(99),
            ..Default::default()
        };
        FeeModel::new(config).generate(100, 0)
    };
    let a = make();
    let b = make();
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.is_spike, y.is_spike);
        assert_eq!(x.fee, y.fee);
        assert_eq!(x.timestamp, y.timestamp);
    }
}

#[test]
fn baseline_output_length() {
    let fees = FeeModel::baseline(10);
    assert_eq!(fees.len(), 10);
}

#[test]
fn baseline_values_in_range() {
    let fees = FeeModel::baseline(50);
    for fee in &fees {
        assert!(*fee >= 100.0 && *fee <= 1_000_000.0, "fee out of range: {fee}");
    }
}

#[test]
fn baseline_no_nans() {
    let fees = FeeModel::baseline(50);
    for fee in &fees {
        assert!(!fee.is_nan(), "unexpected NaN in baseline output");
    }
}
