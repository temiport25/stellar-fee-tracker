use stellar_devkit::simulation::congestion_predictor::{
    congestion_label, congestion_score, CongestionInput, CongestionLabel, CongestionLevel,
    CongestionPredictor,
};
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
        assert!(
            (100.0..=1_000_000.0).contains(fee),
            "fee out of range: {fee}"
        );
    }
}

#[test]
fn baseline_no_nans() {
    let fees = FeeModel::baseline(50);
    for fee in &fees {
        assert!(!fee.is_nan(), "unexpected NaN in baseline output");
    }
}

// ── Issue #178: FeeModelConfig::validate() ────────────────────────────────────

#[test]
fn validate_zero_base_fee_errors() {
    let config = FeeModelConfig {
        base_fee: 0,
        ..Default::default()
    };
    assert!(
        config.validate().is_err(),
        "zero base_fee should fail validation"
    );
}

#[test]
fn validate_spike_probability_above_one_errors() {
    let config = FeeModelConfig {
        spike_probability: 1.1,
        ..Default::default()
    };
    assert!(
        config.validate().is_err(),
        "spike_probability > 1.0 should fail"
    );
}

#[test]
fn validate_negative_spike_probability_errors() {
    let config = FeeModelConfig {
        spike_probability: -0.1,
        ..Default::default()
    };
    assert!(
        config.validate().is_err(),
        "negative spike_probability should fail"
    );
}

#[test]
fn validate_zero_spike_multiplier_errors() {
    let config = FeeModelConfig {
        spike_multiplier: 0,
        ..Default::default()
    };
    assert!(
        config.validate().is_err(),
        "zero spike_multiplier should fail"
    );
}

#[test]
fn validate_valid_config_succeeds() {
    let config = FeeModelConfig::default();
    assert!(config.validate().is_ok(), "default config should be valid");
}

// ── Issue #178: FeeModel::run() ───────────────────────────────────────────────

#[test]
fn run_output_len_equals_ledger_count() {
    let config = FeeModelConfig {
        ledger_count: 50,
        seed: Some(7),
        ..Default::default()
    };
    let points = FeeModel::run(&config);
    assert_eq!(
        points.len(),
        50,
        "run() should produce exactly ledger_count points"
    );
}

#[test]
fn run_all_fees_greater_than_zero() {
    let config = FeeModelConfig {
        ledger_count: 100,
        seed: Some(13),
        ..Default::default()
    };
    let points = FeeModel::run(&config);
    for p in &points {
        assert!(p.fee > 0, "fee at ledger {} must be > 0", p.ledger);
    }
}

#[test]
fn run_identical_seeds_produce_identical_output() {
    let config = FeeModelConfig {
        seed: Some(42),
        ledger_count: 200,
        ..Default::default()
    };
    let a = FeeModel::run(&config);
    let b = FeeModel::run(&config);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.fee, y.fee);
        assert_eq!(x.is_spike, y.is_spike);
        assert_eq!(x.timestamp, y.timestamp);
    }
}

// ── Issue #178: CongestionPredictor::predict() boundary values ─────────────────

#[test]
fn predict_boundary_tx_200_is_moderate() {
    assert_eq!(
        CongestionPredictor::predict(200, 0),
        CongestionLevel::Moderate
    );
}

#[test]
fn predict_boundary_fee_300_is_moderate() {
    assert_eq!(
        CongestionPredictor::predict(0, 300),
        CongestionLevel::Moderate
    );
}

#[test]
fn predict_boundary_tx_500_is_high() {
    assert_eq!(CongestionPredictor::predict(500, 0), CongestionLevel::High);
}

#[test]
fn predict_boundary_fee_1000_is_high() {
    assert_eq!(
        CongestionPredictor::predict(0, 1_000),
        CongestionLevel::High
    );
}

#[test]
fn predict_boundary_tx_800_is_critical() {
    assert_eq!(
        CongestionPredictor::predict(800, 0),
        CongestionLevel::Critical
    );
}

#[test]
fn predict_boundary_fee_5000_is_critical() {
    assert_eq!(
        CongestionPredictor::predict(0, 5_000),
        CongestionLevel::Critical
    );
}

#[test]
fn predict_all_four_congestion_levels() {
    assert_eq!(CongestionPredictor::predict(10, 50), CongestionLevel::Low);
    assert_eq!(
        CongestionPredictor::predict(300, 100),
        CongestionLevel::Moderate
    );
    assert_eq!(
        CongestionPredictor::predict(600, 100),
        CongestionLevel::High
    );
    assert_eq!(
        CongestionPredictor::predict(900, 100),
        CongestionLevel::Critical
    );
}

// ── Issue #178: congestion_score() ────────────────────────────────────────────

#[test]
fn congestion_score_result_in_0_1() {
    let input = CongestionInput {
        recent_fee_window: 250_000.0,
        capacity_usage: 0.5,
        spike_count: 5,
    };
    let score = congestion_score(&input);
    assert!((0.0..=1.0).contains(&score), "score {score} out of [0,1]");
}

#[test]
fn congestion_score_zero_inputs_is_zero() {
    let input = CongestionInput {
        recent_fee_window: 0.0,
        capacity_usage: 0.0,
        spike_count: 0,
    };
    let score = congestion_score(&input);
    assert!(
        (score - 0.0).abs() < 1e-9,
        "all-zero inputs should score 0.0"
    );
}

#[test]
fn congestion_score_higher_spike_count_increases_score() {
    let base = CongestionInput {
        recent_fee_window: 1_000.0,
        capacity_usage: 0.3,
        spike_count: 0,
    };
    let elevated = CongestionInput {
        recent_fee_window: 1_000.0,
        capacity_usage: 0.3,
        spike_count: 10,
    };
    assert!(
        congestion_score(&elevated) > congestion_score(&base),
        "higher spike_count should produce a higher congestion score"
    );
}

#[test]
fn congestion_score_full_inputs_clamps_to_1() {
    let input = CongestionInput {
        recent_fee_window: 1_000_000.0,
        capacity_usage: 1.0,
        spike_count: 100,
    };
    let score = congestion_score(&input);
    assert!(
        (score - 1.0).abs() < 1e-9,
        "saturated inputs should clamp to 1.0"
    );
}

#[test]
fn congestion_label_all_variants() {
    assert_eq!(congestion_label(0.1), CongestionLabel::Normal);
    assert_eq!(congestion_label(0.45), CongestionLabel::Rising);
    assert_eq!(congestion_label(0.7), CongestionLabel::Congested);
    assert_eq!(congestion_label(0.9), CongestionLabel::Critical);
}
