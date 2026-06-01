use std::time::{Duration, Instant};

use stellar_devkit::harness::horizon_mock::HorizonMock;

/// Spin up mock server with normal.json. Assert response status 200 and fee_charged.mode = "100".
#[test]
fn normal_scenario_fee_stats_returns_mode_100() {
    let path = std::path::Path::new("src/harness/scenarios/normal.json");
    let mock = HorizonMock::new("normal").with_scenario_path(path);

    let body = mock.fee_stats_payload().expect("fee_stats_payload failed");
    assert!(!body.is_empty(), "response body must not be empty");

    let json: serde_json::Value = serde_json::from_str(&body).expect("response is not valid JSON");
    let mode = json["fee_stats"]["fee_charged"]["mode"]
        .as_str()
        .expect("fee_charged.mode missing");
    assert_eq!(
        mode, "100",
        "expected fee_charged.mode == \"100\", got {}",
        mode
    );
}

/// Spin up with congested.json. Assert fee_charged.p95 parses to a value > 100000.
#[test]
fn congested_scenario_fee_stats_p95_exceeds_100k() {
    let path = std::path::Path::new("src/harness/scenarios/congested.json");
    let mock = HorizonMock::new("congested").with_scenario_path(path);

    let body = mock.fee_stats_payload().expect("fee_stats_payload failed");
    let json: serde_json::Value = serde_json::from_str(&body).expect("response is not valid JSON");

    let p95: u64 = json["fee_stats"]["fee_charged"]["p95"]
        .as_str()
        .expect("fee_charged.p95 missing")
        .parse()
        .expect("fee_charged.p95 is not a number");
    assert!(p95 > 100_000, "expected p95 > 100000, got {}", p95);
}

/// Set error_rate=1.0. Assert every request returns 503 (should_inject_error always true).
#[test]
fn error_injection_rate_1_always_injects() {
    let mock = HorizonMock::new("normal").with_error_rate(1.0);
    for _ in 0..20 {
        assert!(
            mock.should_inject_error(),
            "expected should_inject_error() == true with error_rate=1.0"
        );
    }
}

/// Set delay_ms=200. Assert response time >= 200ms.
#[test]
fn response_delay_200ms_respected() {
    let mock = HorizonMock::new("normal").with_delay_ms(200);
    let start = Instant::now();
    mock.apply_delay();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(200),
        "expected elapsed >= 200ms, got {:?}",
        elapsed
    );
}
