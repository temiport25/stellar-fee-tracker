mod alerts;
mod api;
mod cache;
mod cli;
mod config;
mod db;
mod error;
mod insights;
mod logging;
mod metrics;
mod middleware;
mod repository;
mod scheduler;
mod services;
mod store;

use std::sync::Arc;

use axum::http::{HeaderName, Method};
use axum::{routing::get, Router};
use clap::Parser;
use dotenvy::dotenv;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::alerts::AlertManager;
use crate::cache::ResponseCache;
use crate::cli::Cli;
use crate::config::Config;
use crate::error::AppError;
use crate::insights::{FeeInsightsEngine, HorizonFeeDataProvider, InsightsConfig};
use crate::logging::init_logging;
use crate::metrics::AppMetrics;
use crate::middleware::auth::require_api_key;
use crate::middleware::rate_limit::{enforce_rate_limit, RateLimitState};
use crate::repository::FeeRepository;
use crate::scheduler::run_fee_polling_with_retry;
use crate::services::horizon::HorizonClient;
use crate::store::{FeeHistoryStore, DEFAULT_CAPACITY};

#[tokio::main]
async fn main() {
    // Load .env file (if present)
    dotenv().ok();

    // Initialize structured logging
    init_logging();

    // Parse CLI flags
    let cli = Cli::parse();

    // Build configuration (CLI overrides env)
    let config = Config::from_sources(&cli)
        .map_err(AppError::Config)
        .unwrap_or_else(|err| {
            tracing::error!("{}", err);
            std::process::exit(1);
        });

    tracing::info!(
        "Configuration loaded: network={:?}, horizon_url={}, poll_interval_seconds={}, cache_ttl_seconds={}, rate_limit_per_minute={}, api_port={}, allowed_origins={:?}, retry_attempts={}, base_retry_delay_ms={}, database_url={}, storage_retention_days={}, api_key_configured={}, webhook_configured={}, alert_threshold={:?}",
        config.stellar_network,
        config.horizon_url,
        config.poll_interval_seconds,
        config.cache_ttl_seconds,
        config.rate_limit_per_minute,
        config.api_port,
        config.allowed_origins,
        config.retry_attempts,
        config.base_retry_delay_ms,
        config.database_url,
        config.storage_retention_days,
        config.api_key.is_some(),
        config.webhook_url.is_some(),
        config.alert_threshold,
    );

    // ---- Database ----
    let db_pool = db::create_pool(&config.database_url)
        .await
        .unwrap_or_else(|err| {
            tracing::error!("Failed to initialise database: {}", err);
            std::process::exit(1);
        });
    tracing::info!("Database initialised: {}", config.database_url);

    // ---- Metrics ----
    let app_metrics = Arc::new(AppMetrics::new().unwrap_or_else(|err| {
        tracing::error!("Failed to initialise Prometheus metrics: {}", err);
        std::process::exit(1);
    }));

    let repository = Arc::new(FeeRepository::new(db_pool));

    // ---- Shared state ----
    let horizon_client = Arc::new(HorizonClient::new(config.horizon_url.clone()));
    tracing::info!("Horizon client initialized: {}", horizon_client.base_url());

    let fee_store = Arc::new(RwLock::new(FeeHistoryStore::new(DEFAULT_CAPACITY)));

    let insights_engine = Arc::new(RwLock::new(FeeInsightsEngine::new(
        InsightsConfig::default(),
    )));
    let current_fees_cache = Arc::new(Mutex::new(ResponseCache::new(Duration::from_secs(
        config.cache_ttl_seconds,
    ))));

    // ---- Startup rehydration ----
    let rehydration_window = chrono::Utc::now() - chrono::Duration::hours(24);
    match repository.fetch_since(rehydration_window).await {
        Ok(points) if !points.is_empty() => {
            let count = points.len();
            {
                let mut store = fee_store.write().await;
                for point in &points {
                    store.push(point.clone());
                }
            }
            {
                let mut engine = insights_engine.write().await;
                if let Err(err) = engine.process_fee_data(&points).await {
                    tracing::warn!("Insights engine error during rehydration: {}", err);
                }
            }
            tracing::info!("Restored {} fee data points from database", count);
        }
        Ok(_) => tracing::info!("No historical fee data found — starting cold"),
        Err(err) => tracing::warn!("Failed to rehydrate store from database: {}", err),
    }
    let horizon_provider = Arc::new(HorizonFeeDataProvider::new((*horizon_client).clone()));
    let fee_stats_provider: Arc<dyn api::fees::FeeStatsProvider + Send + Sync> =
        horizon_client.clone();
    let alert_manager = Arc::new(AlertManager::new(
        config.webhook_url.clone(),
        config.alert_threshold.clone(),
        config.stellar_network.as_str().to_string(),
    ));
    let rate_limit_state = Arc::new(RateLimitState::new(config.rate_limit_per_minute));

    // ---- CORS policy ----
    // Log and skip invalid origins rather than panicking at startup.
    let origins: Vec<axum::http::HeaderValue> = config
        .allowed_origins
        .iter()
        .filter_map(|o| match o.parse() {
            Ok(v) => Some(v),
            Err(err) => {
                tracing::warn!(
                    "Skipping invalid ALLOWED_ORIGINS entry '{}': {}",
                    o,
                    err
                );
                None
            }
        })
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("x-api-key"),
        ])
        .expose_headers([
            HeaderName::from_static("etag"),
            HeaderName::from_static("cache-control"),
            HeaderName::from_static("last-modified"),
            HeaderName::from_static("x-ratelimit-limit"),
            HeaderName::from_static("x-ratelimit-remaining"),
            HeaderName::from_static("x-ratelimit-reset"),
            HeaderName::from_static("retry-after"),
        ])
        .max_age(Duration::from_secs(3600));

    // ---- Axum router ----
    //
    // Route tiers (from least to most restricted):
    //
    //  /health   — no rate limit, no auth (must always respond for load-balancer probes)
    //  /metrics  — rate limited, NO API-key auth (must be scrapeable by Prometheus agents)
    //  all else  — rate limited + optional API-key auth
    //
    // fees routes get shared state (Horizon client, store, insights engine)
    // insights routes get Arc<RwLock<FeeInsightsEngine>> as their own state
    // Both sub-routers are Router<()> after with_state, so merge works fine

    let fees_router = Router::new()
        .route("/fees/current", get(api::fees::current_fees))
        .route("/fees/history", get(api::fees::fee_history))
        .route("/fees/trend", get(api::fees::fee_trend))
        .with_state(Arc::new(api::fees::FeesApiState {
            fee_stats_provider: Some(fee_stats_provider),
            fee_cache: current_fees_cache,
            fee_store: fee_store.clone(),
            insights_engine: Some(insights_engine.clone()),
        }));

    // Business routes that require optional API-key auth.
    let api_routes = Router::new()
        .merge(fees_router)
        .merge(api::insights::create_insights_router(insights_engine.clone()))
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
                .with_state(repository.clone()),
        );

    let api_routes = match config.api_key.clone() {
        Some(expected_key) => {
            tracing::info!("API key authentication is enabled for protected routes");
            api_routes.layer(axum::middleware::from_fn_with_state(
                Some(expected_key),
                require_api_key,
            ))
        }
        None => api_routes,
    };

    // /metrics: rate limited but NOT behind API-key auth (Prometheus scrapers
    // should not need to know the API key).
    let metrics_for_handler = app_metrics.clone();
    let metrics_route = Router::new().route(
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
                        .body(axum::body::Body::from(body))
                        .unwrap_or_else(|_| {
                            axum::response::Response::builder()
                                .status(500)
                                .body(axum::body::Body::from("internal error"))
                                .unwrap()
                        }),
                    Err(err) => {
                        tracing::error!("Failed to render metrics: {}", err);
                        axum::response::Response::builder()
                            .status(500)
                            .body(axum::body::Body::from("metrics error"))
                            .unwrap_or_else(|_| {
                                axum::response::Response::new(axum::body::Body::empty())
                            })
                    }
                }
            }
        }),
    );

    // Rate-limited tier: metrics + business API routes.
    let rate_limited = Router::new()
        .merge(metrics_route)
        .merge(api_routes)
        .layer(axum::middleware::from_fn_with_state(
            rate_limit_state,
            enforce_rate_limit,
        ));

    // Final app: /health bypasses the rate limiter entirely.
    let app = Router::new()
        .route("/health", get(api::health::health))
        .merge(rate_limited)
        .layer(cors);

    // ---- TCP listener ----
    let addr = format!("0.0.0.0:{}", config.api_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|err| {
            tracing::error!("Failed to bind to {}: {}", addr, err);
            std::process::exit(1);
        });

    tracing::info!("API server listening on {}", addr);

    // ---- Run server + scheduler concurrently ----
    tokio::join!(
        async {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap_or_else(|err| tracing::error!("Server error: {}", err));
        },
        run_fee_polling_with_retry(
            horizon_provider,
            fee_store,
            insights_engine,
            config.poll_interval_seconds,
            config.retry_attempts,
            config.base_retry_delay_ms,
            Some(repository),
            config.storage_retention_days,
            Some(app_metrics),
            Some(alert_manager),
        ),
    );

    tracing::info!("Application shut down cleanly");
}
