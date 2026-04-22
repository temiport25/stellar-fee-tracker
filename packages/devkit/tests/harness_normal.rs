/// Integration test: load normal scenario and assert p50 fee_charged == 100 stroops.
#[test]
fn normal_scenario_p50_is_baseline() {
    let path = std::path::Path::new(
        "src/harness/scenarios/normal.json",
    );
    let raw = std::fs::read_to_string(path).expect("normal.json not found");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("invalid JSON");

    let p50: u64 = json["fee_stats"]["fee_charged"]["p50"]
        .as_str()
        .expect("p50 missing")
        .parse()
        .expect("p50 not a number");

    assert_eq!(p50, 100, "expected p50 == 100, got {}", p50);
}
