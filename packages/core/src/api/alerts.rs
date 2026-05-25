//! CRUD endpoints for alert webhook configurations.
//!
//! All routes require API key authentication (via the `X-Api-Key` header
//! checked by the auth middleware from Issue #33).
//!
//! Routes:
//! - `POST   /alerts/config`        — register a new webhook
//! - `GET    /alerts/config`        — list all webhook configs
//! - `PATCH  /alerts/config/:id`    — update threshold / enabled state
//! - `DELETE /alerts/config/:id`    — soft-delete (sets enabled = 0)

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::repository::{AlertConfig, AlertEvent, FeeRepository, VALID_THRESHOLDS};

/// Shared state for the alerts routes.
pub type AlertsState = Arc<FeeRepository>;

// ---- Request / response shapes ----

#[derive(Debug, Deserialize)]
pub struct CreateAlertRequest {
    pub webhook_url: String,
    pub threshold: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAlertRequest {
    pub threshold: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct CreateAlertResponse {
    pub id: i64,
}

// ---- Helpers ----

fn is_valid_threshold(t: &str) -> bool {
    VALID_THRESHOLDS.contains(&t)
}

/// Validate that a webhook URL is safe to call:
/// - Must use HTTPS.
/// - Host must not be a loopback, link-local, or private-range IP (SSRF guard).
/// - Must have a non-empty hostname.
fn is_safe_webhook_url(url: &str) -> bool {
    use std::net::IpAddr;

    // Must start with https://
    if !url.starts_with("https://") {
        return false;
    }

    // Extract the host portion (after the scheme, before path/query/port).
    let after_scheme = &url["https://".len()..];
    let host_and_maybe_port = after_scheme.split('/').next().unwrap_or("");
    // Strip optional port.
    let host = host_and_maybe_port.split(':').next().unwrap_or("").trim();

    if host.is_empty() {
        return false;
    }

    // Reject well-known loopback hostnames.
    if matches!(host, "localhost" | "ip6-localhost" | "ip6-loopback") {
        return false;
    }

    // If the host is an IP address, reject private / loopback / link-local ranges.
    if let Ok(ip) = host.parse::<IpAddr>() {
        match ip {
            IpAddr::V4(v4) => {
                if v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                {
                    return false;
                }
            }
            IpAddr::V6(v6) => {
                if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                    return false;
                }
            }
        }
    }

    // Catch bare numeric prefixes for common internal ranges not covered above.
    if host.starts_with("169.254.") // link-local / AWS metadata
        || host.starts_with("100.64.")  // shared address space (RFC 6598)
    {
        return false;
    }

    true
}

// ---- Handlers ----

/// `POST /alerts/config` — register a new webhook target.
pub async fn create_alert(
    State(repo): State<AlertsState>,
    Json(body): Json<CreateAlertRequest>,
) -> Result<(StatusCode, Json<CreateAlertResponse>), (StatusCode, Json<serde_json::Value>)> {
    let threshold = body.threshold.as_deref().unwrap_or("Major");

    if !is_valid_threshold(threshold) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "Invalid threshold '{}'. Must be one of: {}",
                    threshold,
                    VALID_THRESHOLDS.join(", ")
                )
            })),
        ));
    }

    if !is_safe_webhook_url(&body.webhook_url) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Invalid webhook_url: must be an HTTPS URL with a public hostname"
            })),
        ));
    }

    let id = repo
        .insert_alert_config(&body.webhook_url, threshold)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;

    Ok((StatusCode::CREATED, Json(CreateAlertResponse { id })))
}

/// `GET /alerts/config` — list all registered webhook configs.
pub async fn list_alerts(
    State(repo): State<AlertsState>,
) -> Result<Json<Vec<AlertConfig>>, (StatusCode, Json<serde_json::Value>)> {
    let configs = repo.list_alert_configs().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;

    Ok(Json(configs))
}

/// `PATCH /alerts/config/:id` — update threshold and/or enabled state.
pub async fn update_alert(
    State(repo): State<AlertsState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateAlertRequest>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    // Fetch current config to apply partial updates.
    let configs = repo.list_alert_configs().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;

    let current = configs.iter().find(|c| c.id == id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Alert config not found" })),
        )
    })?;

    let threshold = body.threshold.as_deref().unwrap_or(&current.threshold);
    let enabled = body.enabled.unwrap_or(current.enabled);

    if !is_valid_threshold(threshold) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "Invalid threshold '{}'. Must be one of: {}",
                    threshold,
                    VALID_THRESHOLDS.join(", ")
                )
            })),
        ));
    }

    let updated = repo
        .update_alert_config(id, threshold, enabled)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;

    if updated {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Alert config not found" })),
        ))
    }
}

/// `DELETE /alerts/config/:id` — soft-delete by setting enabled = 0.
pub async fn delete_alert(
    State(repo): State<AlertsState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let deleted = repo.delete_alert_config(id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Alert config not found" })),
        ))
    }
}

// ---- Alert history ----

#[derive(Debug, Deserialize)]
pub struct AlertHistoryQuery {
    pub limit: Option<i64>,
    pub severity: Option<String>,
    pub delivered: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct AlertHistoryResponse {
    pub total: i64,
    pub items: Vec<AlertEvent>,
}

/// `GET /alerts/history` — paginated alert event log.
///
/// Query params:
/// - `limit`    — max items to return (default 20, clamped to 100)
/// - `severity` — optional filter: Minor | Major | Critical
/// - `delivered` — optional bool filter
pub async fn get_alert_history(
    State(repo): State<AlertsState>,
    Query(params): Query<AlertHistoryQuery>,
) -> Result<Json<AlertHistoryResponse>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let severity = params.severity.as_deref();
    let delivered = params.delivered;

    if let Some(sev) = severity {
        if !is_valid_threshold(sev) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "Invalid severity '{}'. Must be one of: {}",
                        sev,
                        VALID_THRESHOLDS.join(", ")
                    )
                })),
            ));
        }
    }

    let (items, total) = tokio::try_join!(
        repo.query_alert_history(limit, severity, delivered),
        repo.count_alert_events(severity, delivered),
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;

    Ok(Json(AlertHistoryResponse { total, items }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Method, Request},
        routing::{delete, get, patch, post},
        Router,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::db::create_pool;

    async fn make_app() -> Router {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        let repo = Arc::new(FeeRepository::new(pool));

        Router::new()
            .route("/alerts/config", post(create_alert))
            .route("/alerts/config", get(list_alerts))
            .route("/alerts/config/:id", patch(update_alert))
            .route("/alerts/config/:id", delete(delete_alert))
            .with_state(repo)
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn post_creates_alert_config() {
        let app = make_app().await;
        let req = Request::builder()
            .method(Method::POST)
            .uri("/alerts/config")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"webhook_url":"https://example.com/hook","threshold":"Major"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["id"], 1);
    }

    #[tokio::test]
    async fn post_invalid_threshold_returns_400() {
        let app = make_app().await;
        let req = Request::builder()
            .method(Method::POST)
            .uri("/alerts/config")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"webhook_url":"https://example.com/hook","threshold":"Catastrophic"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_lists_alert_configs() {
        let app = make_app().await;

        // Create one first
        let create_req = Request::builder()
            .method(Method::POST)
            .uri("/alerts/config")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"webhook_url":"https://example.com/hook"}"#))
            .unwrap();
        app.clone().oneshot(create_req).await.unwrap();

        let list_req = Request::builder()
            .method(Method::GET)
            .uri("/alerts/config")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(list_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn patch_updates_alert_config() {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        let repo = Arc::new(FeeRepository::new(pool));
        let id = repo
            .insert_alert_config("https://example.com/hook", "Minor")
            .await
            .unwrap();

        let app = Router::new()
            .route("/alerts/config/:id", patch(update_alert))
            .with_state(repo);

        let req = Request::builder()
            .method(Method::PATCH)
            .uri(format!("/alerts/config/{}", id))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"threshold":"Critical","enabled":false}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn patch_invalid_threshold_returns_400() {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        let repo = Arc::new(FeeRepository::new(pool));
        let id = repo
            .insert_alert_config("https://example.com/hook", "Minor")
            .await
            .unwrap();

        let app = Router::new()
            .route("/alerts/config/:id", patch(update_alert))
            .with_state(repo);

        let req = Request::builder()
            .method(Method::PATCH)
            .uri(format!("/alerts/config/{}", id))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"threshold":"Bad"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_soft_deletes_alert_config() {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        let repo = Arc::new(FeeRepository::new(pool.clone()));
        let id = repo
            .insert_alert_config("https://example.com/hook", "Major")
            .await
            .unwrap();

        let app = Router::new()
            .route("/alerts/config/:id", delete(delete_alert))
            .with_state(repo.clone());

        let req = Request::builder()
            .method(Method::DELETE)
            .uri(format!("/alerts/config/{}", id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify still in DB but disabled
        let configs = repo.list_alert_configs().await.unwrap();
        assert_eq!(configs.len(), 1);
        assert!(!configs[0].enabled);
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_404() {
        let app = make_app().await;
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/alerts/config/9999")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Method, Request},
        routing::get,
        Router,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::db::create_pool;
    use crate::repository::AlertEvent;

    async fn make_app() -> Router {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        let repo = Arc::new(FeeRepository::new(pool));
        Router::new()
            .route("/alerts/history", get(get_alert_history))
            .with_state(repo)
    }

    async fn make_app_with_events(events: Vec<AlertEvent>) -> Router {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        let repo = Arc::new(FeeRepository::new(pool));
        for e in &events {
            repo.log_alert_event(e).await.unwrap();
        }
        Router::new()
            .route("/alerts/history", get(get_alert_history))
            .with_state(repo)
    }

    fn make_event(severity: &str, delivered: bool) -> AlertEvent {
        AlertEvent {
            id: None,
            config_id: None,
            severity: severity.to_string(),
            peak_fee: 8000,
            baseline_fee: 130.5,
            spike_ratio: 61.3,
            webhook_url: "https://hooks.example.com/test".to_string(),
            delivered,
            triggered_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn history_returns_empty_when_no_events() {
        let app = make_app().await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/alerts/history")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["total"], 0);
        assert_eq!(json["items"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn history_returns_all_events_with_total() {
        let events: Vec<_> = (0..5).map(|_| make_event("Major", true)).collect();
        let app = make_app_with_events(events).await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/alerts/history")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["total"], 5);
        assert_eq!(json["items"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn history_filters_by_severity() {
        let events = vec![
            make_event("Minor", true),
            make_event("Major", true),
            make_event("Critical", false),
        ];
        let app = make_app_with_events(events).await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/alerts/history?severity=Major")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["total"], 1);
        assert_eq!(json["items"][0]["severity"], "Major");
    }

    #[tokio::test]
    async fn history_filters_by_delivered() {
        let events = vec![
            make_event("Major", true),
            make_event("Major", false),
            make_event("Major", true),
        ];
        let app = make_app_with_events(events).await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/alerts/history?delivered=true")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["total"], 2);
        assert_eq!(json["items"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn history_limit_clamped() {
        let events: Vec<_> = (0..5).map(|_| make_event("Major", true)).collect();
        let app = make_app_with_events(events).await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/alerts/history?limit=2")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let json = body_json(resp.into_body()).await;
        // total reflects all matching rows, items is limited
        assert_eq!(json["total"], 5);
        assert_eq!(json["items"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn history_invalid_severity_returns_400() {
        let app = make_app().await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/alerts/history?severity=Catastrophic")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
