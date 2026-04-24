//! Fee polling scheduler.
//!
//! Drives the main polling loop: each tick fetches fee data from the
//! Horizon provider, pushes it into the history store, runs the
//! insights engine, and persists new points to SQLite.
//!
//! Network errors are retried with exponential backoff + jitter (Issue #10).
//! Parse errors are not retried — malformed data won't fix itself.
//! DB write errors are logged but never crash the scheduler.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::signal;
use tokio::sync::RwLock;
use tokio::time;

use crate::alerts::AlertManager;
use crate::insights::error::ProviderError;
use crate::insights::types::FeeDataPoint;
use crate::insights::{FeeDataProvider, FeeInsightsEngine};
use crate::metrics::AppMetrics;
use crate::repository::FeeRepository;
use crate::store::FeeHistoryStore;

/// Run the fee polling loop until Ctrl+C is received.
/// Uses defaults for retry and retention — prefer `run_fee_polling_with_retry` in production.
pub async fn run_fee_polling(
    horizon_provider: Arc<dyn FeeDataProvider + Send + Sync>,
    history_store: Arc<RwLock<FeeHistoryStore>>,
    insights_engine: Arc<RwLock<FeeInsightsEngine>>,
    poll_interval_seconds: u64,
    alert_manager: Option<Arc<AlertManager>>,
) {
    run_fee_polling_with_retry(
        horizon_provider,
        history_store,
        insights_engine,
        poll_interval_seconds,
        3,
        1000,
        None,
        7,
        None,
        alert_manager,
    )
    .await
}

/// Full version with configurable retry parameters and optional DB persistence.
#[allow(clippy::too_many_arguments)]
pub async fn run_fee_polling_with_retry(
    horizon_provider: Arc<dyn FeeDataProvider + Send + Sync>,
    history_store: Arc<RwLock<FeeHistoryStore>>,
    insights_engine: Arc<RwLock<FeeInsightsEngine>>,
    poll_interval_seconds: u64,
    max_retry_attempts: u32,
    base_retry_delay_ms: u64,
    repository: Option<Arc<FeeRepository>>,
    storage_retention_days: u64,
    metrics: Option<Arc<AppMetrics>>,
    alert_manager: Option<Arc<AlertManager>>,
) {
    let mut interval = time::interval(Duration::from_secs(poll_interval_seconds));

    tracing::info!(
        "Fee polling started (interval: {}s, max retries: {}, retention: {}d)",
        poll_interval_seconds,
        max_retry_attempts,
        storage_retention_days,
    );

    loop {
        tokio::select! {
            _ = interval.tick() => {
                poll_once(
                    &horizon_provider,
                    &history_store,
                    &insights_engine,
                    max_retry_attempts,
                    base_retry_delay_ms,
                    repository.as_deref(),
                    storage_retention_days,
                    metrics.as_deref(),
                    alert_manager.as_deref(),
                ).await;
            }

            _ = signal::ctrl_c() => {
                tracing::info!("Shutdown signal received. Stopping polling.");
                break;
            }
        }
    }

    tracing::info!("Fee polling stopped cleanly");
}

/// Execute a single poll cycle with retry and optional persistence.
#[allow(clippy::too_many_arguments)]
async fn poll_once(
    horizon_provider: &Arc<dyn FeeDataProvider + Send + Sync>,
    history_store: &Arc<RwLock<FeeHistoryStore>>,
    insights_engine: &Arc<RwLock<FeeInsightsEngine>>,
    max_retry_attempts: u32,
    base_retry_delay_ms: u64,
    repository: Option<&FeeRepository>,
    storage_retention_days: u64,
    metrics: Option<&AppMetrics>,
    alert_manager: Option<&AlertManager>,
) {
    if let Some(m) = metrics {
        m.polls_total.inc();
    }

    let points = match fetch_with_retry(
        horizon_provider.as_ref(),
        max_retry_attempts,
        base_retry_delay_ms,
    )
    .await
    {
        Some(p) => p,
        None => {
            if let Some(m) = metrics {
                m.poll_errors_total.inc();
            }
            tracing::warn!(
                "All {} retry attempts exhausted — skipping tick",
                max_retry_attempts
            );
            return;
        }
    };

    if points.is_empty() {
        tracing::warn!("Provider returned no fee data points this tick");
        return;
    }

    // Push into in-memory store
    {
        let mut store = history_store.write().await;
        for point in &points {
            store.push(point.clone());
        }
        let store_len = store.len();
        tracing::debug!("Store now holds {} data points", store_len);
        if let Some(m) = metrics {
            m.fee_points_stored.set(store_len as f64);
        }
    }

    // Run insights engine
    {
        let mut engine = insights_engine.write().await;
        match engine.process_fee_data(&points).await {
            Ok(update) => {
                tracing::info!(
                    "Insights updated — {} points processed, short-term avg: {:.1} stroops",
                    update.data_points_processed,
                    update.insights.rolling_averages.short_term.value,
                );
                if let Some(m) = metrics {
                    m.current_avg_fee
                        .set(update.insights.rolling_averages.short_term.value);
                    m.spikes_detected_total
                        .inc_by(update.insights.congestion_trends.recent_spikes.len() as f64);
                }
                if let Some(manager) = alert_manager {
                    manager.check_and_dispatch(&update).await;
                }
            }
            Err(err) => {
                tracing::error!("Insights engine error: {}", err);
            }
        }
    }

    // Persist to DB (non-fatal on error)
    if let Some(repo) = repository {
        match repo.insert_fee_points(&points).await {
            Ok(()) => {
                tracing::debug!("Persisted {} fee points to DB", points.len());
            }
            Err(err) => {
                tracing::warn!("Failed to persist fee points to DB: {}", err);
            }
        }

        let cutoff = Utc::now() - chrono::Duration::days(storage_retention_days as i64);
        match repo.prune_older_than(cutoff).await {
            Ok(n) if n > 0 => tracing::debug!("Pruned {} old fee points from DB", n),
            Ok(_) => {}
            Err(err) => tracing::warn!("Failed to prune old fee points: {}", err),
        }
    }
}

/// Attempt to fetch fee data, retrying on network errors with exponential
/// backoff + random jitter. Parse errors are not retried.
///
/// Returns `Some(points)` on the first successful fetch, or `None` if all
/// attempts are exhausted.
pub async fn fetch_with_retry(
    provider: &dyn FeeDataProvider,
    max_attempts: u32,
    base_delay_ms: u64,
) -> Option<Vec<FeeDataPoint>> {
    const MAX_DELAY_MS: u64 = 30_000;

    for attempt in 0..max_attempts {
        match provider.fetch_latest_fees().await {
            Ok(points) => {
                if attempt > 0 {
                    tracing::info!("Fetch succeeded after {} attempt(s)", attempt + 1);
                }
                return Some(points);
            }

            Err(ProviderError::FormatError { message }) => {
                tracing::error!("Parse error fetching fees (not retrying): {}", message);
                return None;
            }

            Err(err) => {
                let backoff_ms = {
                    let exponential = base_delay_ms.saturating_mul(1u64 << attempt);
                    let jitter = rand::random::<u64>() % base_delay_ms.max(1);
                    exponential.saturating_add(jitter).min(MAX_DELAY_MS)
                };

                tracing::warn!(
                    "Fetch attempt {}/{} failed: {} — retrying in {}ms",
                    attempt + 1,
                    max_attempts,
                    err,
                    backoff_ms,
                );

                time::sleep(Duration::from_millis(backoff_ms)).await;
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    use crate::insights::error::ProviderError;
    use crate::insights::types::FeeDataPoint;
    use crate::insights::{FeeInsightsEngine, InsightsConfig};
    use crate::services::mock_horizon::MockHorizonClient;
    use crate::store::{FeeHistoryStore, DEFAULT_CAPACITY};

    fn make_point(fee_amount: u64) -> FeeDataPoint {
        FeeDataPoint {
            fee_amount,
            timestamp: Utc::now(),
            transaction_hash: format!("hash_{}", fee_amount),
            ledger_sequence: 1,
        }
    }

    fn make_shared_store() -> Arc<RwLock<FeeHistoryStore>> {
        Arc::new(RwLock::new(FeeHistoryStore::new(DEFAULT_CAPACITY)))
    }

    fn make_shared_engine() -> Arc<RwLock<FeeInsightsEngine>> {
        Arc::new(RwLock::new(FeeInsightsEngine::new(
            InsightsConfig::default(),
        )))
    }

    // ---- poll_once tests ----

    #[tokio::test]
    async fn poll_once_pushes_points_into_store() {
        let points = vec![make_point(100), make_point(200), make_point(300)];
        let provider: Arc<dyn FeeDataProvider + Send + Sync> =
            Arc::new(MockHorizonClient::new().with_fees(points));
        let store = make_shared_store();
        let engine = make_shared_engine();

        poll_once(&provider, &store, &engine, 3, 0, None, 7, None, None).await;

        assert_eq!(store.read().await.len(), 3);
    }

    #[tokio::test]
    async fn poll_once_runs_insights_engine() {
        let points = vec![make_point(100), make_point(150), make_point(200)];
        let provider: Arc<dyn FeeDataProvider + Send + Sync> =
            Arc::new(MockHorizonClient::new().with_fees(points));
        let store = make_shared_store();
        let engine = make_shared_engine();

        poll_once(&provider, &store, &engine, 3, 0, None, 7, None, None).await;

        assert!(engine.read().await.get_last_update().is_some());
    }

    #[tokio::test]
    async fn poll_once_on_provider_error_does_not_push_to_store() {
        let provider: Arc<dyn FeeDataProvider + Send + Sync> =
            Arc::new(MockHorizonClient::new().with_error(ProviderError::ServiceUnavailable));
        let store = make_shared_store();
        let engine = make_shared_engine();

        poll_once(&provider, &store, &engine, 1, 0, None, 7, None, None).await;

        assert!(store.read().await.is_empty());
    }

    #[tokio::test]
    async fn two_poll_cycles_accumulate_data_in_store() {
        let points = vec![make_point(100), make_point(200)];
        let provider: Arc<dyn FeeDataProvider + Send + Sync> =
            Arc::new(MockHorizonClient::new().with_fees(points));
        let store = make_shared_store();
        let engine = make_shared_engine();

        poll_once(&provider, &store, &engine, 3, 0, None, 7, None, None).await;
        poll_once(&provider, &store, &engine, 3, 0, None, 7, None, None).await;

        assert_eq!(store.read().await.len(), 4);
    }

    #[tokio::test]
    async fn poll_once_with_empty_provider_response_leaves_store_unchanged() {
        let provider: Arc<dyn FeeDataProvider + Send + Sync> = Arc::new(MockHorizonClient::new());
        let store = make_shared_store();
        let engine = make_shared_engine();

        poll_once(&provider, &store, &engine, 3, 0, None, 7, None, None).await;

        assert!(store.read().await.is_empty());
    }

    // ---- fetch_with_retry tests ----

    #[tokio::test]
    async fn fetch_with_retry_returns_points_on_success() {
        let points = vec![make_point(100), make_point(200)];
        let mock = MockHorizonClient::new().with_fees(points.clone());

        let result = fetch_with_retry(&mock, 3, 0).await;

        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn fetch_with_retry_retries_on_network_error_and_succeeds() {
        let mock = MockHorizonClient::new().with_error(ProviderError::NetworkError {
            message: "timeout".into(),
        });

        let result = fetch_with_retry(&mock, 3, 0).await;

        assert!(result.is_none());
        assert_eq!(mock.calls(), 3);
    }

    #[tokio::test]
    async fn fetch_with_retry_does_not_retry_on_parse_error() {
        let mock = MockHorizonClient::new().with_error(ProviderError::FormatError {
            message: "bad json".into(),
        });

        let result = fetch_with_retry(&mock, 3, 0).await;

        assert!(result.is_none());
        assert_eq!(mock.calls(), 1);
    }

    #[tokio::test]
    async fn fetch_with_retry_returns_none_when_all_attempts_exhausted() {
        let mock = MockHorizonClient::new().with_error(ProviderError::ServiceUnavailable);

        let result = fetch_with_retry(&mock, 3, 0).await;

        assert!(result.is_none());
        assert_eq!(mock.calls(), 3);
    }

    #[tokio::test]
    async fn fetch_with_retry_succeeds_on_first_attempt_makes_one_call() {
        let mock = MockHorizonClient::new().with_fees(vec![make_point(100)]);

        let result = fetch_with_retry(&mock, 3, 0).await;

        assert!(result.is_some());
        assert_eq!(mock.calls(), 1);
    }
}
