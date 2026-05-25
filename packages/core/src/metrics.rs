//! Prometheus metrics registry for the Stellar fee tracker.
//!
//! [`AppMetrics`] owns all registered metrics and the [`Registry`] they
//! belong to. Construct it once at startup, wrap in `Arc`, and pass it
//! to the scheduler and HTTP middleware.
//!
//! Exposed at `GET /metrics` in Prometheus text exposition format
//! (`text/plain; version=0.0.4`). The endpoint is intentionally excluded
//! from API-key auth so it can be scraped by Prometheus / Grafana agents.

use prometheus::{Counter, Gauge, Opts, Registry};

/// All application-level Prometheus metrics.
///
/// Only metrics that are actively incremented are registered here.
/// Registering metrics that are never written produces misleading zeros
/// in Prometheus/Grafana dashboards.
pub struct AppMetrics {
    /// Total number of Horizon polling attempts (success + failure).
    pub polls_total: Counter,
    /// Total number of failed Horizon polling attempts.
    pub poll_errors_total: Counter,
    /// Current number of fee data points held in the in-memory store.
    pub fee_points_stored: Gauge,
    /// Latest short-term rolling average fee (in stroops).
    pub current_avg_fee: Gauge,
    /// Total number of fee spikes detected by the insights engine.
    pub spikes_detected_total: Counter,
    /// The registry that owns all of the above metrics.
    pub registry: Registry,
}

impl AppMetrics {
    /// Create and register all metrics. Returns an error if any metric
    /// name is invalid or duplicated (should not happen in practice).
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let polls_total = Counter::with_opts(Opts::new(
            "stellar_fee_tracker_polls_total",
            "Total Horizon polling attempts",
        ))?;

        let poll_errors_total = Counter::with_opts(Opts::new(
            "stellar_fee_tracker_poll_errors_total",
            "Failed Horizon polling attempts",
        ))?;

        let fee_points_stored = Gauge::with_opts(Opts::new(
            "stellar_fee_tracker_fee_points_stored",
            "Current size of the FeeHistoryStore",
        ))?;

        let current_avg_fee = Gauge::with_opts(Opts::new(
            "stellar_fee_tracker_current_avg_fee",
            "Latest short-term rolling average fee in stroops",
        ))?;

        let spikes_detected_total = Counter::with_opts(Opts::new(
            "stellar_fee_tracker_spikes_detected_total",
            "Total fee spikes detected",
        ))?;

        registry.register(Box::new(polls_total.clone()))?;
        registry.register(Box::new(poll_errors_total.clone()))?;
        registry.register(Box::new(fee_points_stored.clone()))?;
        registry.register(Box::new(current_avg_fee.clone()))?;
        registry.register(Box::new(spikes_detected_total.clone()))?;

        Ok(Self {
            polls_total,
            poll_errors_total,
            fee_points_stored,
            current_avg_fee,
            spikes_detected_total,
            registry,
        })
    }

    /// Render all metrics as Prometheus text format (for the `/metrics` endpoint).
    pub fn render(&self) -> Result<String, prometheus::Error> {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buf = Vec::new();
        encoder.encode(&metric_families, &mut buf)?;
        Ok(String::from_utf8(buf).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_metrics_register_without_error() {
        let metrics = AppMetrics::new();
        assert!(
            metrics.is_ok(),
            "AppMetrics::new() failed: {:?}",
            metrics.err()
        );
    }

    #[test]
    fn render_produces_non_empty_output_after_increment() {
        let metrics = AppMetrics::new().unwrap();
        metrics.polls_total.inc();
        let output = metrics.render().unwrap();
        assert!(output.contains("stellar_fee_tracker_polls_total"));
    }

    #[test]
    fn counters_increment_correctly() {
        let metrics = AppMetrics::new().unwrap();
        metrics.polls_total.inc_by(3.0);
        metrics.poll_errors_total.inc();
        assert!((metrics.polls_total.get() - 3.0).abs() < f64::EPSILON);
        assert!((metrics.poll_errors_total.get() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gauge_set_and_get() {
        let metrics = AppMetrics::new().unwrap();
        metrics.fee_points_stored.set(42.0);
        assert!((metrics.fee_points_stored.get() - 42.0).abs() < f64::EPSILON);
    }

}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Method, Request},
        response::Response,
        routing::get,
        Router,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn make_metrics_app() -> (Router, Arc<AppMetrics>) {
        let metrics = Arc::new(AppMetrics::new().unwrap());
        let m = metrics.clone();
        let app = Router::new().route(
            "/metrics",
            get(move || {
                let m2 = m.clone();
                async move {
                    match m2.render() {
                        Ok(body) => Response::builder()
                            .status(200)
                            .header("content-type", "text/plain; version=0.0.4")
                            .body(Body::from(body))
                            .unwrap(),
                        Err(_) => Response::builder()
                            .status(500)
                            .body(Body::from("error"))
                            .unwrap(),
                    }
                }
            }),
        );
        (app, metrics)
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_200() {
        let (app, _) = make_metrics_app().await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn metrics_endpoint_content_type_is_prometheus_text() {
        let (app, _) = make_metrics_app().await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/plain; version=0.0.4");
    }

    #[tokio::test]
    async fn metrics_endpoint_contains_all_metric_names_after_increment() {
        let (app, metrics) = make_metrics_app().await;

        // Simulate a poll cycle
        metrics.polls_total.inc();
        metrics.poll_errors_total.inc();
        metrics.fee_points_stored.set(10.0);
        metrics.current_avg_fee.set(150.5);
        metrics.spikes_detected_total.inc();

        let req = Request::builder()
            .method(Method::GET)
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();

        assert!(body.contains("stellar_fee_tracker_polls_total"));
        assert!(body.contains("stellar_fee_tracker_poll_errors_total"));
        assert!(body.contains("stellar_fee_tracker_fee_points_stored"));
        assert!(body.contains("stellar_fee_tracker_current_avg_fee"));
        assert!(body.contains("stellar_fee_tracker_spikes_detected_total"));
    }

    #[tokio::test]
    async fn polls_total_incremented_value_appears_in_output() {
        let (app, metrics) = make_metrics_app().await;
        metrics.polls_total.inc_by(5.0);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();

        // Prometheus text format: metric_name value\n
        assert!(body.contains("stellar_fee_tracker_polls_total 5"));
    }
}
