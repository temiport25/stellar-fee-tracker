use stellar_devkit::simulation::fee_model::{FeeModel, FeeModelConfig};

#[test]
fn replay_generates_expected_record_count() {
    let config = FeeModelConfig { seed: Some(42), ..Default::default() };
    let mut model = FeeModel::new(config);
    let records = model.generate(100, 0);
    assert_eq!(
        records.len(),
        100,
        "expected 100 records for replay, got {}",
        records.len()
    );
}

#[test]
fn replay_records_have_sequential_timestamps() {
    let config = FeeModelConfig {
        seed: Some(1),
        ledger_interval_secs: 5,
        ..Default::default()
    };
    let mut model = FeeModel::new(config);
    let records = model.generate(5, 1_000);
    for (i, rec) in records.iter().enumerate() {
        let expected = 1_000 + (i as u64 * 5);
        assert_eq!(
            rec.timestamp, expected,
            "record {i}: expected timestamp {expected}, got {}",
            rec.timestamp
        );
    }
}

#[test]
fn replay_is_deterministic_with_same_seed() {
    let make = || {
        let cfg = FeeModelConfig { seed: Some(77), ..Default::default() };
        FeeModel::new(cfg).generate(100, 0)
    };
    let a = make();
    let b = make();
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.fee, y.fee);
        assert_eq!(x.timestamp, y.timestamp);
    }
}
