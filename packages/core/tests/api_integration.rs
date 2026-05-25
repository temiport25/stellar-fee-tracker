//! Integration tests for all API endpoints.
//!
//! Each test boots the full Axum router (same assembly as `main.rs`) using
//! `tower::ServiceExt::oneshot` — no live server or live Horizon node needed.
//!
//! `build_test_app()` wires together:
//! - A wiremocked Horizon `/fee_stats` endpoint used by the `/fees/current`
//!   fee-stats provider implementation
//! - An in-memory SQLite pool with all migrations applied
//! - A `FeeHistoryStore` pre-populated with data points
//! - A `FeeInsightsEngine` pre-warmed with the same data
//! - Prometheus `AppMetrics`
//! - The complete merged `Router<()>` returned ready for `oneshot`

use std::sync::Arc;
use std::time::Duration as StdDuration;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use chrono::{Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

use stellar_fee_tracker::{
    api,
    cache::ResponseCache,
    db,
    insights::types::FeeDataPoint,
    insights::{FeeInsightsEngine, InsightsConfig},
    metrics::AppMetrics,
    repository::FeeRepository,
    services::horizon::HorizonClient,
    store::{FeeHistoryStore, DEFAULT_CAPACITY},
};

// ---- Helpers ----------------------------------------------------------------

/// Fake Horizon fee_stats JSON returned by the wiremock server.
const FAKE_FEE_STATS: &str = r#"{
    "last_ledger": "1000",
    "last_ledger_base_fee": "100",
    "ledger_usage": "0.1",
    "fee_charged": {
        "max": "5000",
        "min": "100",
        "mode": "100",
        "p10": "100",
        "p20": "100",
        "p25": "100",
        "p30": "100",
        "p40": "100",
        "p50": "150",
        "p60": "200",
        "p70": "250",
        "p75": "300",
        "p80": "350",
        "p90": "500",
        "p95": "800",
        "p99": "1000",
        "avg": "213"
    },
    "max_fee": {
        "max": "5000",
        "min": "100",
        "mode": "100",
        "p10": "100",
        "p20": "100",
        "p25": "100",
        "p30": "100",
        "p40": "100",
        "p50": "150",
        "p60": "200",
        "p70": "250",
        "p75": "300",
        "p80": "350",
        "p90": "500",
        "p95": "800",
        "p99": "1000",
        "avg": "213"
    }
}"#;

/// Build a set of realistic fee data points spanning the last hour.
fn make_fee_points(count: usize) -> Vec<FeeDataPoint> {
    let now = Utc::now();
    (0..count)
        .map(|i| FeeDataPoint {
            fee_amount: 100 + (i as u64 * 10),
            timestamp: now - ChronoDuration::minutes((count - i) as i64),
            transaction_hash: format!("txhash{:06}", i),
            ledger_sequence: 50_000_000 + i as u64,
        })
        .collect()
}

/// Build the complete test router.
///
/// - Starts a wiremock server that stubs `GET /fee_stats` so the
///   `/fees/current` handler resolves without hitting a real Horizon node.
/// - Uses in-memory SQLite for the alerts repository.
/// - Pre-seeds the FeeHistoryStore and InsightsEngine with `make_fee_points`.
///
/// Returns `(Router, MockServer)`.  The `MockServer` must stay alive for the
/// duration of the test because `HorizonClient` holds a reference to its URL.
async fn build_test_app() -> (Router, MockServer) {
    // ---- Wiremock server for /fees/current ----
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fee_stats"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(FAKE_FEE_STATS, "application/json"))
        .mount(&mock_server)
        .await;

    // ---- Fee data points ----
    let points = make_fee_points(20);

    // ---- In-memory DB + repository ----
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    let repository = Arc::new(FeeRepository::new(pool));

    // ---- Shared state ----
    let horizon_client = Arc::new(HorizonClient::new(mock_server.uri()));
    let fee_stats_provider: Arc<dyn api::fees::FeeStatsProvider + Send + Sync> =
        horizon_client.clone();
    let fee_cache = Arc::new(Mutex::new(ResponseCache::new(StdDuration::from_secs(5))));

    let fee_store = Arc::new(RwLock::new(FeeHistoryStore::new(DEFAULT_CAPACITY)));
    {
        let mut store = fee_store.write().await;
        for p in &points {
            store.push(p.clone());
        }
    }

    let insights_engine = Arc::new(RwLock::new(FeeInsightsEngine::new(
        InsightsConfig::default(),
    )));
    {
        let mut engine = insights_engine.write().await;
        engine.process_fee_data(&points).await.unwrap();
    }

    // ---- Metrics ----
    let app_metrics = Arc::new(AppMetrics::new().unwrap());
    let metrics_for_handler = app_metrics.clone();

    // ---- Fees router ----
    let fees_router = Router::new()
        .route("/fees/current", get(api::fees::current_fees))
        .route("/fees/history", get(api::fees::fee_history))
        .route("/fees/trend", get(api::fees::fee_trend))
        .with_state(Arc::new(api::fees::FeesApiState {
            fee_stats_provider: Some(fee_stats_provider),
            fee_cache,
            fee_store: fee_store.clone(),
            insights_engine: Some(insights_engine.clone()),
        }));

    // ---- Full router (mirrors main.rs assembly) ----
    let app = Router::new()
        .route("/health", get(api::health::health))
        .route(
            "/metrics",
            get(move || {
                let m = metrics_for_handler.clone();
                async move {
                    match m.render() {
                        Ok(body) => axum::response::Response::builder()
                            .status(200)
                            .header(
                                axum::http::header::CONTENT_TYPE,
                                "text/plain; version=0.0.4",
                            )
                            .body(Body::from(body))
                            .unwrap(),
                        Err(_) => axum::response::Response::builder()
                            .status(500)
                            .body(Body::from("metrics error"))
                            .unwrap(),
                    }
                }
            }),
        )
        .merge(fees_router)
        .merge(api::insights::create_insights_router(
            insights_engine.clone(),
        ))
        .merge(
            Router::new()
                .route(
                    "/alerts/config",
                    axum::routing::post(api::alerts::create_alert),
                )
                .route(
                    "/alerts/config",
                    axum::routing::get(api::alerts::list_alerts),
                )
                .route(
                    "/alerts/config/:id",
                    axum::routing::patch(api::alerts::update_alert),
                )
                .route(
                    "/alerts/config/:id",
                    axum::routing::delete(api::alerts::delete_alert),
                )
                .route(
                    "/alerts/history",
                    axum::routing::get(api::alerts::get_alert_history),
                )
                .with_state(repository),
        );

    (app, mock_server)
}

/// Convenience: collect body bytes and parse as JSON.
async fn json_body(body: Body) -> Value {
    let bytes = body.collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

// ---- GET /health ------------------------------------------------------------

#[tokio::test]
async fn health_returns_200_with_ok_body() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"ok");
}

// ---- GET /fees/current ------------------------------------------------------

#[tokio::test]
async fn fees_current_returns_200_with_required_fields() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/current")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert!(json["base_fee"].is_string(), "missing base_fee");
    assert!(
        json["percentiles"]["p50"].is_string(),
        "missing percentiles.p50"
    );
    assert!(
        json["percentiles"]["p10"].is_string(),
        "missing percentiles.p10"
    );
    assert!(
        json["percentiles"]["p95"].is_string(),
        "missing percentiles.p95"
    );
    assert!(json["min_fee"].is_string(), "missing min_fee");
    assert!(json["max_fee"].is_string(), "missing max_fee");
    assert!(json["avg_fee"].is_string(), "missing avg_fee");
}

#[tokio::test]
async fn fees_current_base_fee_matches_mock_value() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/current")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert_eq!(json["base_fee"], "100");
    assert_eq!(json["percentiles"]["p50"], "150");
}

#[tokio::test]
async fn fees_current_sets_cache_control_max_age_5() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/current")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("max-age=5, stale-while-revalidate=10")
    );
    assert!(resp.headers().contains_key("etag"));
    assert!(resp.headers().contains_key("last-modified"));
}

#[tokio::test]
async fn fees_current_if_none_match_returns_304() {
    let (app, _mock) = build_test_app().await;
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/fees/current")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let etag = first
        .headers()
        .get("etag")
        .expect("missing etag header")
        .to_str()
        .unwrap()
        .to_string();

    let second = app
        .oneshot(
            Request::builder()
                .uri("/fees/current")
                .header("if-none-match", etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    let body = second.into_body().collect().await.unwrap().to_bytes();
    assert!(body.is_empty(), "304 response should not include body");
}

// ---- GET /fees/history ------------------------------------------------------

#[tokio::test]
async fn fees_history_default_window_returns_200_with_fees_array() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("max-age=30, stale-while-revalidate=60")
    );
    assert!(resp.headers().contains_key("etag"));
    assert!(resp.headers().contains_key("last-modified"));
    let json = json_body(resp.into_body()).await;
    assert!(json["fees"].is_array(), "missing fees array");
    assert!(json["summary"].is_object(), "missing summary");
    assert!(json["data_points"].is_number(), "missing data_points");
    assert!(json["window"].is_string(), "missing window");
}

#[tokio::test]
async fn fees_history_1h_window_returns_200() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/history?window=1h")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert_eq!(json["window"], "1h");
    assert!(json["fees"].is_array());
    // The 20 points were seeded within the last hour
    assert!(json["data_points"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn fees_history_6h_window_returns_200() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/history?window=6h")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert_eq!(json["window"], "6h");
}

#[tokio::test]
async fn fees_history_invalid_window_returns_400() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/history?window=invalid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = json_body(resp.into_body()).await;
    assert!(json["error"].is_string());
}

#[tokio::test]
async fn fees_history_summary_contains_required_fields() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/history?window=1h")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    let summary = &json["summary"];
    assert!(summary["min"].is_number(), "missing summary.min");
    assert!(summary["max"].is_number(), "missing summary.max");
    assert!(summary["avg"].is_number(), "missing summary.avg");
    assert!(summary["p50"].is_number(), "missing summary.p50");
    assert!(summary["p95"].is_number(), "missing summary.p95");
}

// ---- GET /fees/trend --------------------------------------------------------

#[tokio::test]
async fn fees_trend_returns_200_with_status_and_changes() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/trend")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert!(json["status"].is_string(), "missing status");
    assert!(json["changes"].is_object(), "missing changes");
    assert!(json["trend_strength"].is_string(), "missing trend_strength");
    assert!(
        json["recent_spike_count"].is_number(),
        "missing recent_spike_count"
    );
    assert!(json["last_updated"].is_string(), "missing last_updated");
}

#[tokio::test]
async fn fees_trend_changes_object_has_expected_keys() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/fees/trend")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    let changes = &json["changes"];
    // Keys exist (may be null for partial windows)
    assert!(changes.get("1h_pct").is_some(), "missing 1h_pct");
    assert!(changes.get("6h_pct").is_some(), "missing 6h_pct");
    assert!(changes.get("24h_pct").is_some(), "missing 24h_pct");
}

// ---- GET /insights ----------------------------------------------------------

#[tokio::test]
async fn insights_returns_200_with_rolling_averages() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/insights")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert!(
        json["rolling_averages"].is_object(),
        "missing rolling_averages"
    );
}

#[tokio::test]
async fn insights_rolling_averages_has_short_term() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/insights")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert!(
        json["rolling_averages"]["short_term"].is_object(),
        "missing rolling_averages.short_term"
    );
}

#[tokio::test]
async fn insights_sets_cache_and_validation_headers() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/insights")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("max-age=10, stale-while-revalidate=20")
    );
    assert!(resp.headers().contains_key("etag"));
    assert!(resp.headers().contains_key("last-modified"));
}

#[tokio::test]
async fn insights_if_none_match_returns_304() {
    let (app, _mock) = build_test_app().await;
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/insights")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let etag = first
        .headers()
        .get("etag")
        .expect("missing etag header")
        .to_str()
        .unwrap()
        .to_string();

    let second = app
        .oneshot(
            Request::builder()
                .uri("/insights")
                .header("if-none-match", etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    let body = second.into_body().collect().await.unwrap().to_bytes();
    assert!(body.is_empty(), "304 response should not include body");
}

// ---- GET /insights/averages -------------------------------------------------

#[tokio::test]
async fn insights_averages_returns_200() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/insights/averages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert!(json["short_term"].is_object(), "missing short_term");
    assert!(json["medium_term"].is_object(), "missing medium_term");
    assert!(json["long_term"].is_object(), "missing long_term");
}

// ---- GET /insights/extremes -------------------------------------------------

#[tokio::test]
async fn insights_extremes_returns_200() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/insights/extremes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    // Response is a JSON object
    let json = json_body(resp.into_body()).await;
    assert!(
        json.is_object(),
        "expected JSON object from /insights/extremes"
    );
}

// ---- GET /insights/congestion -----------------------------------------------

#[tokio::test]
async fn insights_congestion_returns_200() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/insights/congestion")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert!(
        json.is_object(),
        "expected JSON object from /insights/congestion"
    );
}

// ---- GET /insights/health ---------------------------------------------------

#[tokio::test]
async fn insights_health_returns_200_with_status() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/insights/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert_eq!(json["status"], "healthy");
}

// ---- GET /metrics -----------------------------------------------------------

#[tokio::test]
async fn metrics_returns_200() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_content_type_is_prometheus_text() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let ct = resp
        .headers()
        .get("content-type")
        .expect("missing content-type header")
        .to_str()
        .unwrap();
    assert_eq!(ct, "text/plain; version=0.0.4");
}

#[tokio::test]
async fn metrics_body_contains_metric_names() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("stellar_fee_tracker_polls_total"));
    assert!(body.contains("stellar_fee_tracker_spikes_detected_total"));
}

// ---- GET /alerts/config (quick smoke) ---------------------------------------

#[tokio::test]
async fn alerts_config_list_returns_200_empty_array() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/alerts/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert!(json.is_array(), "expected JSON array");
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn alerts_history_returns_200_empty() {
    let (app, _mock) = build_test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/alerts/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp.into_body()).await;
    assert_eq!(json["total"], 0);
    assert!(json["items"].as_array().unwrap().is_empty());
}
