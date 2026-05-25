use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{connect_info::ConnectInfo, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use dashmap::DashMap;
use serde_json::json;

const X_FORWARDED_FOR_HEADER: &str = "x-forwarded-for";
const X_RATE_LIMIT_LIMIT_HEADER: &str = "x-ratelimit-limit";
const X_RATE_LIMIT_REMAINING_HEADER: &str = "x-ratelimit-remaining";
const X_RATE_LIMIT_RESET_HEADER: &str = "x-ratelimit-reset";

#[derive(Clone)]
pub struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_rate: f64,
}

impl TokenBucket {
    pub fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            tokens: capacity,
            last_refill: Instant::now(),
            capacity,
            refill_rate,
        }
    }

    pub fn try_consume(&mut self) -> Result<u32, u64> {
        self.refill();

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Ok(self.tokens.floor() as u32);
        }

        Err(self.seconds_until_next_token())
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        if elapsed <= 0.0 {
            return;
        }

        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = now;
    }

    fn seconds_until_next_token(&self) -> u64 {
        if self.tokens >= 1.0 {
            return 0;
        }

        let needed = 1.0 - self.tokens;
        ((needed / self.refill_rate).ceil() as u64).max(1)
    }
}

pub struct RateLimitState {
    buckets: DashMap<IpAddr, TokenBucket>,
    capacity: u32,
    refill_rate: f64,
}

impl RateLimitState {
    pub fn new(rate_limit_per_minute: u32) -> Self {
        let capacity = rate_limit_per_minute.max(1);
        let refill_rate = f64::from(capacity) / 60.0;
        Self::with_refill_rate(capacity, refill_rate)
    }

    #[cfg(test)]
    pub(crate) fn with_refill_rate_for_tests(capacity: u32, refill_rate: f64) -> Self {
        Self::with_refill_rate(capacity.max(1), refill_rate.max(f64::EPSILON))
    }

    fn with_refill_rate(capacity: u32, refill_rate: f64) -> Self {
        Self {
            buckets: DashMap::new(),
            capacity,
            refill_rate,
        }
    }
}

/// Remove token buckets that have not been refilled in the last 2 minutes.
/// Called probabilistically to avoid taking a write lock on every request.
fn evict_stale_buckets(state: &RateLimitState) {
    // 2× the typical replenish window gives every IP a fair grace period.
    let cutoff = Instant::now() - Duration::from_secs(120);
    state.buckets.retain(|_, bucket| bucket.last_refill >= cutoff);
}

pub async fn enforce_rate_limit(
    State(state): State<Arc<RateLimitState>>,
    request: Request,
    next: Next,
) -> Response {
    let client_ip = extract_client_ip(&request);

    // Evict stale entries once the map grows large (probabilistic amortisation).
    if state.buckets.len() > 10_000 {
        evict_stale_buckets(&state);
    }

    let (allowed, remaining, reset_secs, retry_after_secs) = {
        let mut bucket = state
            .buckets
            .entry(client_ip)
            .or_insert_with(|| TokenBucket::new(f64::from(state.capacity), state.refill_rate));

        match bucket.try_consume() {
            Ok(remaining) => (true, remaining, bucket.seconds_until_next_token(), None),
            Err(retry_after) => (false, 0, retry_after, Some(retry_after)),
        }
    };

    if !allowed {
        let retry_after = retry_after_secs.unwrap_or(1);
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": format!("Rate limit exceeded. Try again in {} seconds.", retry_after)
            })),
        )
            .into_response();

        attach_rate_limit_headers(&mut response, state.capacity, remaining, reset_secs);
        insert_number_header(&mut response, header::RETRY_AFTER.as_str(), retry_after);
        return response;
    }

    let mut response = next.run(request).await;
    attach_rate_limit_headers(&mut response, state.capacity, remaining, reset_secs);
    response
}

fn extract_client_ip(request: &Request) -> IpAddr {
    // Determine the direct TCP peer address first.
    let peer_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());

    // Only honour X-Forwarded-For when the connection arrives from a trusted
    // proxy (localhost), so an attacker cannot spoof their IP to bypass the
    // per-IP rate limit by simply setting this header.
    let from_trusted_proxy = peer_ip.map_or(false, |ip| ip.is_loopback());

    if from_trusted_proxy {
        if let Some(forwarded_ip) = request
            .headers()
            .get(X_FORWARDED_FOR_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_x_forwarded_for)
        {
            return forwarded_ip;
        }
    }

    peer_ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

fn parse_x_forwarded_for(value: &str) -> Option<IpAddr> {
    let first_hop = value.split(',').next()?.trim();
    first_hop
        .parse::<IpAddr>()
        .ok()
        .or_else(|| first_hop.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
}

fn attach_rate_limit_headers(response: &mut Response, limit: u32, remaining: u32, reset_secs: u64) {
    insert_number_header(response, X_RATE_LIMIT_LIMIT_HEADER, u64::from(limit));
    insert_number_header(
        response,
        X_RATE_LIMIT_REMAINING_HEADER,
        u64::from(remaining),
    );
    insert_number_header(response, X_RATE_LIMIT_RESET_HEADER, reset_secs);
}

fn insert_number_header(response: &mut Response, name: &'static str, value: u64) {
    if let Ok(header_value) = HeaderValue::from_str(&value.to_string()) {
        response.headers_mut().insert(name, header_value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        extract::connect_info::ConnectInfo,
        http::Request,
        middleware::from_fn_with_state,
        routing::get,
        Router,
    };
    use std::net::SocketAddr;
    use std::time::Duration;
    use tower::ServiceExt;

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn build_test_app(state: Arc<RateLimitState>) -> Router {
        Router::new()
            .route("/test", get(ok_handler))
            .layer(from_fn_with_state(state, enforce_rate_limit))
    }

    async fn request_from_ip(app: &Router, ip: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header(X_FORWARDED_FOR_HEADER, ip)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn request_with_connect_info(app: &Router, addr: SocketAddr) -> Response {
        let mut request = Request::builder().uri("/test").body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(addr));

        app.clone().oneshot(request).await.unwrap()
    }

    fn assert_rate_limit_headers(response: &Response, limit: u32) {
        let headers = response.headers();
        assert_eq!(
            headers
                .get(X_RATE_LIMIT_LIMIT_HEADER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u32>().ok()),
            Some(limit)
        );
        assert!(headers.get(X_RATE_LIMIT_REMAINING_HEADER).is_some());
        assert!(headers.get(X_RATE_LIMIT_RESET_HEADER).is_some());
    }

    #[tokio::test]
    async fn first_n_requests_within_limit_succeed() {
        let app = build_test_app(Arc::new(RateLimitState::new(3)));

        for _ in 0..3 {
            let response = request_from_ip(&app, "203.0.113.1").await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_rate_limit_headers(&response, 3);
        }
    }

    #[tokio::test]
    async fn n_plus_one_request_returns_429_with_retry_after() {
        let app = build_test_app(Arc::new(RateLimitState::new(2)));

        let _ = request_from_ip(&app, "203.0.113.10").await;
        let _ = request_from_ip(&app, "203.0.113.10").await;
        let response = request_from_ip(&app, "203.0.113.10").await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_rate_limit_headers(&response, 2);
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap();
        assert!(retry_after >= 1);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            payload["error"],
            format!("Rate limit exceeded. Try again in {} seconds.", retry_after)
        );
    }

    #[tokio::test]
    async fn bucket_refills_and_later_request_succeeds() {
        let state = Arc::new(RateLimitState::with_refill_rate_for_tests(1, 50.0));
        let app = build_test_app(state);

        let first = request_from_ip(&app, "198.51.100.30").await;
        assert_eq!(first.status(), StatusCode::OK);

        let blocked = request_from_ip(&app, "198.51.100.30").await;
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);

        tokio::time::sleep(Duration::from_millis(25)).await;

        let recovered = request_from_ip(&app, "198.51.100.30").await;
        assert_eq!(recovered.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn different_ips_have_independent_buckets() {
        // Use real socket addresses so each client has its own bucket.
        // X-Forwarded-For is no longer trusted from non-loopback peers.
        let app = build_test_app(Arc::new(RateLimitState::new(1)));
        let addr_a: SocketAddr = "192.0.2.11:40000".parse().unwrap();
        let addr_b: SocketAddr = "192.0.2.12:40001".parse().unwrap();

        let first_a = request_with_connect_info(&app, addr_a).await;
        assert_eq!(first_a.status(), StatusCode::OK);

        let second_a = request_with_connect_info(&app, addr_a).await;
        assert_eq!(second_a.status(), StatusCode::TOO_MANY_REQUESTS);

        let first_b = request_with_connect_info(&app, addr_b).await;
        assert_eq!(first_b.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn xff_is_trusted_when_connection_is_from_loopback() {
        // Simulate a reverse-proxy running on localhost forwarding the real client IP.
        let app = build_test_app(Arc::new(RateLimitState::new(1)));
        let loopback_proxy: SocketAddr = "127.0.0.1:80".parse().unwrap();

        // Two requests from the same real client (via XFF), coming through loopback proxy.
        let mut req1 = Request::builder()
            .uri("/test")
            .header(X_FORWARDED_FOR_HEADER, "203.0.113.99")
            .body(Body::empty())
            .unwrap();
        req1.extensions_mut().insert(ConnectInfo(loopback_proxy));

        let mut req2 = Request::builder()
            .uri("/test")
            .header(X_FORWARDED_FOR_HEADER, "203.0.113.99")
            .body(Body::empty())
            .unwrap();
        req2.extensions_mut().insert(ConnectInfo(loopback_proxy));

        let first = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app.clone().oneshot(req2).await.unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn falls_back_to_connection_ip_when_forwarded_for_is_missing() {
        let app = build_test_app(Arc::new(RateLimitState::new(1)));
        let client_addr: SocketAddr = "198.51.100.44:40000".parse().unwrap();

        let first = request_with_connect_info(&app, client_addr).await;
        assert_eq!(first.status(), StatusCode::OK);

        let second = request_with_connect_info(&app, client_addr).await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
