use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Json,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};

use super::headers::{cache_control, compute_etag, if_none_match_matches, last_modified};
use crate::cache::ResponseCache;
use crate::error::AppError;
use crate::insights::{FeeDataPoint, FeeInsightsEngine, TrendIndicator, TrendStrength};
use crate::services::horizon::HorizonClient;
use crate::store::FeeHistoryStore;

/// Shared state type for the fees route.
pub type FeesState = Arc<FeesApiState>;

#[async_trait]
pub trait FeeStatsProvider {
    async fn fetch_current_fees(&self) -> Result<CurrentFeeResponse, AppError>;
}

#[async_trait]
impl FeeStatsProvider for HorizonClient {
    async fn fetch_current_fees(&self) -> Result<CurrentFeeResponse, AppError> {
        let stats = self.fetch_fee_stats().await?;
        Ok(CurrentFeeResponse {
            base_fee: stats.last_ledger_base_fee,
            min_fee: stats.fee_charged.min,
            max_fee: stats.fee_charged.max,
            avg_fee: stats.fee_charged.avg,
            percentiles: PercentileFees {
                p10: stats.fee_charged.p10,
                p20: stats.fee_charged.p20,
                p30: stats.fee_charged.p30,
                p40: stats.fee_charged.p40,
                p50: stats.fee_charged.p50,
                p60: stats.fee_charged.p60,
                p70: stats.fee_charged.p70,
                p80: stats.fee_charged.p80,
                p90: stats.fee_charged.p90,
                p95: stats.fee_charged.p95,
                p99: stats.fee_charged.p99,
            },
        })
    }
}

#[derive(Clone)]
pub struct FeesApiState {
    pub fee_stats_provider: Option<Arc<dyn FeeStatsProvider + Send + Sync>>,
    pub fee_cache: Arc<Mutex<ResponseCache<CurrentFeeResponse>>>,
    pub fee_store: Arc<RwLock<FeeHistoryStore>>,
    pub insights_engine: Option<Arc<RwLock<FeeInsightsEngine>>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PercentileFees {
    pub p10: String,
    pub p20: String,
    pub p30: String,
    pub p40: String,
    pub p50: String,
    pub p60: String,
    pub p70: String,
    pub p80: String,
    pub p90: String,
    pub p95: String,
    pub p99: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CurrentFeeResponse {
    pub base_fee: String,
    pub min_fee: String,
    pub max_fee: String,
    pub avg_fee: String,
    pub percentiles: PercentileFees,
}

const FEES_CURRENT_MAX_AGE: u32 = 5;
const FEES_CURRENT_SWR: u32 = 10;
const FEES_HISTORY_MAX_AGE: u32 = 30;
const FEES_HISTORY_SWR: u32 = 60;

async fn resolve_last_modified(state: &FeesState) -> axum::http::HeaderValue {
    let timestamp = match state.insights_engine.as_ref() {
        Some(engine) => engine
            .read()
            .await
            .get_last_update()
            .unwrap_or_else(Utc::now),
        None => Utc::now(),
    };
    last_modified(timestamp)
}

fn not_modified_response(
    max_age: u32,
    swr: u32,
    etag: &str,
    last_modified_value: axum::http::HeaderValue,
) -> Response {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::CACHE_CONTROL, cache_control(max_age, swr))
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, last_modified_value)
        .body(Body::empty())
        .expect("304 response should be valid")
}

fn json_cache_response(
    max_age: u32,
    swr: u32,
    etag: &str,
    last_modified_value: axum::http::HeaderValue,
    body: Vec<u8>,
) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, cache_control(max_age, swr))
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, last_modified_value)
        .body(Body::from(body))
        .expect("cached response should be valid")
}

pub async fn current_fees(
    State(state): State<FeesState>,
    request_headers: HeaderMap,
) -> Result<Response, AppError> {
    let cached = {
        let cache = state.fee_cache.lock().await;
        if cache.is_fresh() {
            cache.get()
        } else {
            None
        }
    };
    let payload = if let Some(cached) = cached {
        cached
    } else {
        let provider = state.fee_stats_provider.as_ref().ok_or_else(|| {
            AppError::Config("Fee stats provider missing from fees state".to_string())
        })?;
        let fresh = provider.fetch_current_fees().await?;
        let mut cache = state.fee_cache.lock().await;
        cache.set(fresh.clone());
        fresh
    };

    let body = serde_json::to_vec(&payload).map_err(|err| AppError::Parse(err.to_string()))?;
    let etag = compute_etag(&body);
    let last_modified_value = resolve_last_modified(&state).await;

    if if_none_match_matches(&request_headers, &etag) {
        return Ok(not_modified_response(
            FEES_CURRENT_MAX_AGE,
            FEES_CURRENT_SWR,
            &etag,
            last_modified_value,
        ));
    }

    Ok(json_cache_response(
        FEES_CURRENT_MAX_AGE,
        FEES_CURRENT_SWR,
        &etag,
        last_modified_value,
        body,
    ))
}

#[derive(Debug, Deserialize)]
pub struct FeeHistoryQuery {
    pub window: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeeSummary {
    pub min: u64,
    pub max: u64,
    pub avg: f64,
    pub p50: u64,
    pub p95: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeeHistoryResponse {
    pub window: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub data_points: usize,
    pub fees: Vec<FeeDataPoint>,
    pub summary: FeeSummary,
}

pub async fn fee_history(
    State(state): State<FeesState>,
    Query(params): Query<FeeHistoryQuery>,
    request_headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let window = params.window.unwrap_or_else(|| "1h".to_string());
    let duration = parse_window(&window).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Unsupported window value: {}", window) })),
        )
    })?;

    let to = Utc::now();
    let from = to - duration;
    let fees = {
        let store = state.fee_store.read().await;
        store.get_since(from)
    };
    let summary = compute_summary(&fees);

    let payload = FeeHistoryResponse {
        window,
        from,
        to,
        data_points: fees.len(),
        fees,
        summary,
    };
    let body = serde_json::to_vec(&payload).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to serialize fee history: {}", err) })),
        )
    })?;
    let etag = compute_etag(&body);
    let last_modified_value = resolve_last_modified(&state).await;

    if if_none_match_matches(&request_headers, &etag) {
        return Ok(not_modified_response(
            FEES_HISTORY_MAX_AGE,
            FEES_HISTORY_SWR,
            &etag,
            last_modified_value,
        ));
    }

    Ok(json_cache_response(
        FEES_HISTORY_MAX_AGE,
        FEES_HISTORY_SWR,
        &etag,
        last_modified_value,
        body,
    ))
}

fn parse_window(value: &str) -> Option<Duration> {
    match value {
        "1h" => Some(Duration::hours(1)),
        "6h" => Some(Duration::hours(6)),
        "24h" => Some(Duration::hours(24)),
        _ => None,
    }
}

fn compute_summary(fees: &[FeeDataPoint]) -> FeeSummary {
    if fees.is_empty() {
        return FeeSummary {
            min: 0,
            max: 0,
            avg: 0.0,
            p50: 0,
            p95: 0,
        };
    }

    let mut values: Vec<u64> = fees.iter().map(|f| f.fee_amount).collect();
    values.sort_unstable();
    let sum: u64 = values.iter().sum();
    let len = values.len();

    FeeSummary {
        min: values[0],
        max: values[len - 1],
        avg: sum as f64 / len as f64,
        p50: percentile_nearest_rank(&values, 50),
        p95: percentile_nearest_rank(&values, 95),
    }
}

fn percentile_nearest_rank(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let n = sorted.len();
    let rank = ((percentile * n).saturating_add(99) / 100).max(1);
    sorted[rank - 1]
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrendChanges {
    #[serde(rename = "1h_pct")]
    pub one_h_pct: Option<f64>,
    #[serde(rename = "6h_pct")]
    pub six_h_pct: Option<f64>,
    #[serde(rename = "24h_pct")]
    pub twenty_four_h_pct: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeeTrendResponse {
    pub status: String,
    pub trend_strength: String,
    pub changes: TrendChanges,
    pub recent_spike_count: usize,
    pub predicted_congestion_minutes: Option<i64>,
    pub last_updated: DateTime<Utc>,
}

pub async fn fee_trend(State(state): State<FeesState>) -> Result<Json<FeeTrendResponse>, AppError> {
    let engine = state
        .insights_engine
        .as_ref()
        .ok_or_else(|| AppError::Config("Insights engine missing from fees state".to_string()))?;
    let insights = engine.read().await.get_current_insights();
    let averages = &insights.rolling_averages;
    let current_avg = averages.short_term.value;

    let changes = TrendChanges {
        one_h_pct: percent_change(current_avg, &averages.short_term),
        six_h_pct: percent_change(current_avg, &averages.medium_term),
        twenty_four_h_pct: percent_change(current_avg, &averages.long_term),
    };

    Ok(Json(FeeTrendResponse {
        status: trend_indicator_to_string(&insights.congestion_trends.current_trend),
        trend_strength: trend_strength_to_string(&insights.congestion_trends.trend_strength),
        changes,
        recent_spike_count: insights.congestion_trends.recent_spikes.len(),
        predicted_congestion_minutes: insights
            .congestion_trends
            .predicted_duration
            .map(|d| d.num_minutes()),
        last_updated: insights.last_updated,
    }))
}

fn percent_change(current_avg: f64, window_avg: &crate::insights::AverageResult) -> Option<f64> {
    if window_avg.is_partial || window_avg.value <= 0.0 {
        return None;
    }
    Some(((current_avg - window_avg.value) / window_avg.value) * 100.0)
}

fn trend_indicator_to_string(indicator: &TrendIndicator) -> String {
    match indicator {
        TrendIndicator::Normal => "Normal",
        TrendIndicator::Rising => "Rising",
        TrendIndicator::Congested => "Congested",
        TrendIndicator::Declining => "Declining",
    }
    .to_string()
}

fn trend_strength_to_string(strength: &TrendStrength) -> String {
    match strength {
        TrendStrength::Weak => "Weak",
        TrendStrength::Moderate => "Moderate",
        TrendStrength::Strong => "Strong",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration as StdDuration;

    use crate::insights::InsightsConfig;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use chrono::Duration as ChronoDuration;
    use tower::ServiceExt;

    #[derive(Clone)]
    struct MockFeeStatsProvider {
        responses: Arc<StdMutex<VecDeque<CurrentFeeResponse>>>,
        calls: Arc<AtomicUsize>,
    }

    impl MockFeeStatsProvider {
        fn new(responses: Vec<CurrentFeeResponse>) -> Self {
            Self {
                responses: Arc::new(StdMutex::new(VecDeque::from(responses))),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl FeeStatsProvider for MockFeeStatsProvider {
        async fn fetch_current_fees(&self) -> Result<CurrentFeeResponse, AppError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses
                .lock()
                .expect("mock fee stats provider lock poisoned")
                .pop_front()
                .ok_or_else(|| {
                    AppError::Unknown("No mock fee stats response configured".to_string())
                })
        }
    }

    fn default_cache() -> Arc<Mutex<ResponseCache<CurrentFeeResponse>>> {
        Arc::new(Mutex::new(ResponseCache::new(StdDuration::from_secs(5))))
    }

    fn make_fee_state_with_points(points: Vec<FeeDataPoint>) -> FeesState {
        let mut store = FeeHistoryStore::new(100);
        for point in points {
            store.push(point);
        }

        Arc::new(FeesApiState {
            fee_stats_provider: None,
            fee_cache: default_cache(),
            fee_store: Arc::new(RwLock::new(store)),
            insights_engine: None,
        })
    }

    fn make_fee_state_with_engine(engine: FeeInsightsEngine) -> FeesState {
        Arc::new(FeesApiState {
            fee_stats_provider: None,
            fee_cache: default_cache(),
            fee_store: Arc::new(RwLock::new(FeeHistoryStore::new(100))),
            insights_engine: Some(Arc::new(RwLock::new(engine))),
        })
    }

    fn make_fee_state_with_provider(
        provider: Arc<dyn FeeStatsProvider + Send + Sync>,
        ttl: StdDuration,
    ) -> FeesState {
        Arc::new(FeesApiState {
            fee_stats_provider: Some(provider),
            fee_cache: Arc::new(Mutex::new(ResponseCache::new(ttl))),
            fee_store: Arc::new(RwLock::new(FeeHistoryStore::new(100))),
            insights_engine: None,
        })
    }

    fn make_current_fee_response(base_fee: &str) -> CurrentFeeResponse {
        CurrentFeeResponse {
            base_fee: base_fee.to_string(),
            min_fee: "100".to_string(),
            max_fee: "5000".to_string(),
            avg_fee: "213".to_string(),
            percentiles: PercentileFees {
                p10: "100".to_string(),
                p20: "100".to_string(),
                p30: "100".to_string(),
                p40: "100".to_string(),
                p50: "150".to_string(),
                p60: "200".to_string(),
                p70: "250".to_string(),
                p80: "300".to_string(),
                p90: "500".to_string(),
                p95: "800".to_string(),
                p99: "1000".to_string(),
            },
        }
    }

    fn test_points(count: usize, minutes_ago_start: i64) -> Vec<FeeDataPoint> {
        (0..count)
            .map(|idx| FeeDataPoint {
                fee_amount: 100 + (idx as u64 * 100),
                timestamp: Utc::now() - ChronoDuration::minutes(minutes_ago_start - idx as i64),
                transaction_hash: format!("tx-{}", idx),
                ledger_sequence: 50_000_000 + idx as u64,
            })
            .collect()
    }

    #[test]
    fn current_fee_response_serialises_with_percentiles() {
        let response = CurrentFeeResponse {
            base_fee: "100".into(),
            min_fee: "100".into(),
            max_fee: "5000".into(),
            avg_fee: "213".into(),
            percentiles: PercentileFees {
                p10: "100".into(),
                p20: "100".into(),
                p30: "120".into(),
                p40: "130".into(),
                p50: "150".into(),
                p60: "200".into(),
                p70: "250".into(),
                p80: "300".into(),
                p90: "500".into(),
                p95: "800".into(),
                p99: "1000".into(),
            },
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["base_fee"], "100");
        assert_eq!(json["percentiles"]["p10"], "100");
        assert_eq!(json["percentiles"]["p50"], "150");
        assert_eq!(json["percentiles"]["p95"], "800");
    }

    #[test]
    fn percentile_fees_has_all_six_fields() {
        let p = PercentileFees {
            p10: "100".into(),
            p20: "100".into(),
            p30: "120".into(),
            p40: "130".into(),
            p50: "150".into(),
            p60: "200".into(),
            p70: "250".into(),
            p80: "300".into(),
            p90: "500".into(),
            p95: "800".into(),
            p99: "1000".into(),
        };
        let json = serde_json::to_value(&p).unwrap();
        for field in &["p10", "p20", "p50", "p80", "p90", "p95"] {
            assert!(json.get(field).is_some(), "missing field: {}", field);
            assert!(!json[field].as_str().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn current_fees_returns_cached_response_within_ttl() {
        let mock = MockFeeStatsProvider::new(vec![
            make_current_fee_response("100"),
            make_current_fee_response("200"),
        ]);
        let state =
            make_fee_state_with_provider(Arc::new(mock.clone()), StdDuration::from_secs(60));

        let app = Router::new()
            .route("/fees/current", get(current_fees))
            .with_state(state);

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
        let first_body = to_bytes(first.into_body(), usize::MAX).await.unwrap();
        let first_payload: CurrentFeeResponse = serde_json::from_slice(&first_body).unwrap();
        assert_eq!(first_payload.base_fee, "100");

        let second = app
            .oneshot(
                Request::builder()
                    .uri("/fees/current")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
        let second_payload: CurrentFeeResponse = serde_json::from_slice(&second_body).unwrap();
        assert_eq!(second_payload.base_fee, "100");

        assert_eq!(mock.calls(), 1, "second request should hit cache");
    }

    #[tokio::test]
    async fn current_fees_refetches_after_ttl_expiry() {
        let mock = MockFeeStatsProvider::new(vec![
            make_current_fee_response("100"),
            make_current_fee_response("200"),
        ]);
        let state =
            make_fee_state_with_provider(Arc::new(mock.clone()), StdDuration::from_millis(10));

        let app = Router::new()
            .route("/fees/current", get(current_fees))
            .with_state(state);

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
        let first_body = to_bytes(first.into_body(), usize::MAX).await.unwrap();
        let first_payload: CurrentFeeResponse = serde_json::from_slice(&first_body).unwrap();
        assert_eq!(first_payload.base_fee, "100");

        tokio::time::sleep(StdDuration::from_millis(25)).await;

        let second = app
            .oneshot(
                Request::builder()
                    .uri("/fees/current")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
        let second_payload: CurrentFeeResponse = serde_json::from_slice(&second_body).unwrap();
        assert_eq!(second_payload.base_fee, "200");

        assert_eq!(mock.calls(), 2, "expired cache should trigger refetch");
    }

    #[tokio::test]
    async fn current_fees_returns_304_when_if_none_match_matches() {
        let mock = MockFeeStatsProvider::new(vec![make_current_fee_response("100")]);
        let state = make_fee_state_with_provider(Arc::new(mock), StdDuration::from_secs(60));

        let app = Router::new()
            .route("/fees/current", get(current_fees))
            .with_state(state);

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
        let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
        assert!(body.is_empty(), "304 response should not include body");
    }

    #[tokio::test]
    async fn fee_history_returns_data_points_and_summary_for_supported_windows() {
        for window in ["1h", "6h", "24h"] {
            let state = make_fee_state_with_points(test_points(10, 10));
            let app = Router::new()
                .route("/fees/history", get(fee_history))
                .with_state(state);

            let response = app
                .oneshot(
                    Request::builder()
                        .uri(format!("/fees/history?window={}", window))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let payload: FeeHistoryResponse = serde_json::from_slice(&body).unwrap();

            assert_eq!(payload.window, window);
            assert_eq!(payload.data_points, 10);
            assert_eq!(payload.summary.min, 100);
            assert_eq!(payload.summary.max, 1000);
        }
    }

    #[tokio::test]
    async fn fee_history_invalid_window_returns_400() {
        let state = make_fee_state_with_points(test_points(10, 10));
        let app = Router::new()
            .route("/fees/history", get(fee_history))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/fees/history?window=invalid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    fn points_with_spike(high_fee: u64) -> Vec<FeeDataPoint> {
        let now = Utc::now();
        vec![
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now - ChronoDuration::minutes(60),
                transaction_hash: "tx1".to_string(),
                ledger_sequence: 1,
            },
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now - ChronoDuration::minutes(50),
                transaction_hash: "tx2".to_string(),
                ledger_sequence: 2,
            },
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now - ChronoDuration::minutes(40),
                transaction_hash: "tx3".to_string(),
                ledger_sequence: 3,
            },
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now - ChronoDuration::minutes(30),
                transaction_hash: "tx4".to_string(),
                ledger_sequence: 4,
            },
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now - ChronoDuration::minutes(20),
                transaction_hash: "tx5".to_string(),
                ledger_sequence: 5,
            },
            FeeDataPoint {
                fee_amount: high_fee,
                timestamp: now - ChronoDuration::minutes(10),
                transaction_hash: "tx6".to_string(),
                ledger_sequence: 6,
            },
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now,
                transaction_hash: "tx7".to_string(),
                ledger_sequence: 7,
            },
        ]
    }

    fn points_without_spike() -> Vec<FeeDataPoint> {
        let now = Utc::now();
        vec![
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now - ChronoDuration::minutes(50),
                transaction_hash: "n1".to_string(),
                ledger_sequence: 11,
            },
            FeeDataPoint {
                fee_amount: 110,
                timestamp: now - ChronoDuration::minutes(40),
                transaction_hash: "n2".to_string(),
                ledger_sequence: 12,
            },
            FeeDataPoint {
                fee_amount: 120,
                timestamp: now - ChronoDuration::minutes(30),
                transaction_hash: "n3".to_string(),
                ledger_sequence: 13,
            },
        ]
    }

    #[tokio::test]
    async fn fee_trend_returns_rising_status() {
        let mut engine = FeeInsightsEngine::new(InsightsConfig::default());
        let rising_points = points_with_spike(500);
        engine.process_fee_data(&rising_points).await.unwrap();
        let state = make_fee_state_with_engine(engine);

        let app = Router::new()
            .route("/fees/trend", get(fee_trend))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/fees/trend")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: FeeTrendResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.status, "Rising");
    }

    #[test]
    fn trend_indicator_declining_serialises_to_human_readable_string() {
        assert_eq!(
            trend_indicator_to_string(&TrendIndicator::Declining),
            "Declining"
        );
    }

    #[tokio::test]
    async fn fee_trend_returns_normal_status() {
        let mut engine = FeeInsightsEngine::new(InsightsConfig::default());
        let normal_points = points_without_spike();
        engine.process_fee_data(&normal_points).await.unwrap();
        let state = make_fee_state_with_engine(engine);

        let app = Router::new()
            .route("/fees/trend", get(fee_trend))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/fees/trend")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: FeeTrendResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.status, "Normal");
    }

    #[tokio::test]
    async fn fee_trend_returns_null_changes_for_partial_windows() {
        let state = make_fee_state_with_engine(FeeInsightsEngine::new(InsightsConfig::default()));
        let app = Router::new()
            .route("/fees/trend", get(fee_trend))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/fees/trend")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: FeeTrendResponse = serde_json::from_slice(&body).unwrap();
        assert!(payload.changes.one_h_pct.is_none());
        assert!(payload.changes.six_h_pct.is_none());
        assert!(payload.changes.twenty_four_h_pct.is_none());
    }
}
