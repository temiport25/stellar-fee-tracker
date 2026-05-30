use stellar_devkit::cli::export::Export;
use stellar_devkit::simulation::fee_model::{FeeModel, FeeModelConfig};

#[test]
fn export_csv_has_correct_header() {
    let config = FeeModelConfig { seed: Some(1), ..Default::default() };
    let mut model = FeeModel::new(config);
    let points = model.generate(5, 0);
    let csv = Export::to_csv(&points);
    assert!(
        csv.starts_with("timestamp,fee,ledger,is_spike\n"),
        "CSV header mismatch: {csv}"
    );
}

#[test]
fn export_csv_row_count_matches_input() {
    let config = FeeModelConfig { seed: Some(2), ..Default::default() };
    let mut model = FeeModel::new(config);
    let points = model.generate(10, 0);
    let csv = Export::to_csv(&points);
    let rows: Vec<&str> = csv.trim().lines().collect();
    assert_eq!(
        rows.len(),
        11,
        "expected 10 data rows + 1 header, got {}",
        rows.len() - 1
    );
}

#[test]
fn export_csv_columns_are_parseable() {
    let config = FeeModelConfig { seed: Some(3), ..Default::default() };
    let mut model = FeeModel::new(config);
    let points = model.generate(1, 1_000);
    let csv = Export::to_csv(&points);
    let data_line = csv.lines().nth(1).expect("missing data row");
    let cols: Vec<&str> = data_line.split(',').collect();
    assert_eq!(cols.len(), 4, "expected 4 columns, got {}", cols.len());
    assert!(cols[0].parse::<u64>().is_ok(), "timestamp not a u64");
    assert!(cols[1].parse::<u64>().is_ok(), "fee not a u64");
}
