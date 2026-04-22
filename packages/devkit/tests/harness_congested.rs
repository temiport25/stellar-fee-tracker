/// Integration test: load congested scenario and assert p95 fee_charged > 100,000 stroops.
#[test]
fn congested_scenario_p95_exceeds_100k() {
    let path = std::path::Path::new(
        "src/harness/scenarios/congested.json",
    );
    let raw = std::fs::read_to_string(path).expect("congested.json not found");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("invalid JSON");

    let p95: u64 = json["fee_stats"]["fee_charged"]["p95"]
        .as_str()
        .expect("p95 missing")
        .parse()
        .expect("p95 not a number");

    assert!(p95 > 100_000, "expected p95 > 100000, got {}", p95);
}
