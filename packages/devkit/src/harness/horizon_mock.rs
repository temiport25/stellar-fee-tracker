/// Mock implementation of the Horizon API server for use in tests.
pub struct HorizonMock {
    /// Name of the currently active scenario.
    pub scenario: String,
    /// Optional simulated response delay in milliseconds.
    pub delay_ms: Option<u64>,
    /// Optional explicit path to a scenario JSON file. When set, takes precedence
    /// over the convention-based `src/harness/scenarios/{scenario}.json` path.
    pub scenario_path: Option<std::path::PathBuf>,
    /// Probability [0.0, 1.0] of returning a 500/503 error response.
    pub error_rate: f64,
    /// Optional canned JSON response for `GET /fee_stats`. When set, takes
    /// precedence over `scenario_path` and the convention-based file path.
    pub fee_stats_response: Option<String>,
}

impl HorizonMock {
    pub fn new(scenario: impl Into<String>) -> Self {
        Self {
            scenario: scenario.into(),
            delay_ms: None,
            scenario_path: None,
            error_rate: 0.0,
            fee_stats_response: None,
        }
    }

    /// Configures a canned in-memory JSON response for `GET /fee_stats`,
    /// bypassing file I/O entirely. Takes highest precedence over file-based loading.
    pub fn with_fee_stats_response(mut self, response: impl Into<String>) -> Self {
        self.fee_stats_response = Some(response.into());
        self
    }

    /// Sets the simulated network latency delay.
    pub fn with_delay_ms(mut self, ms: u64) -> Self {
        self.delay_ms = Some(ms);
        self
    }

    /// Sets an explicit path to load scenario JSON from, overriding the convention-based path.
    pub fn with_scenario_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.scenario_path = Some(path.into());
        self
    }

    /// Sets the error injection rate (0.0 = never, 1.0 = always).
    pub fn with_error_rate(mut self, rate: f64) -> Self {
        self.error_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Applies the configured delay, if any. Call before serving a response.
    pub fn apply_delay(&self) {
        if let Some(ms) = self.delay_ms {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }

    /// Returns true if this request should be failed based on the configured error rate.
    pub fn should_inject_error(&self) -> bool {
        self.error_rate > 0.0 && rand_f64() < self.error_rate
    }

    /// Switches to the next scenario from the rotator and updates the active scenario.
    pub fn rotate(&mut self, rotator: &mut crate::harness::scenarios::ScenarioRotator) {
        if let Some(next) = rotator.advance() {
            self.scenario = next.to_string();
        }
    }

    /// Logs a request to stdout with timestamp, method, path, and active scenario name.
    pub fn log_request(&self, method: &str, path: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        println!("[{}] {} {} scenario={}", now, method, path, self.scenario);
    }

    /// Returns the JSON body for `GET /health`.
    pub fn health_payload(&self) -> String {
        format!(r#"{{"status":"ok","scenario":"{}"}}"#, self.scenario)
    }

    /// Loads and returns the scenario JSON to be served at `GET /fee_stats`.
    ///
    /// Resolution order (highest precedence first):
    /// 1. `fee_stats_response` — in-memory canned response (no I/O).
    /// 2. `scenario_path` — explicit file path.
    /// 3. Convention-based path `src/harness/scenarios/{scenario}.json`.
    pub fn fee_stats_payload(&self) -> std::io::Result<String> {
        if let Some(ref canned) = self.fee_stats_response {
            return Ok(canned.clone());
        }
        let path = self.scenario_path.clone().unwrap_or_else(|| {
            std::path::PathBuf::from(format!("src/harness/scenarios/{}.json", self.scenario))
        });
        crate::harness::scenarios::load_from_file(&path)
    }

    /// Loads, validates via typed deserialization, and returns the `fee_stats` JSON.
    /// Falls back to `fee_stats_payload()` if `serde_json` serialization fails.
    pub fn fee_stats_payload_validated(&self) -> std::io::Result<String> {
        if let Some(ref canned) = self.fee_stats_response {
            return Ok(canned.clone());
        }
        let path = self.scenario_path.clone().unwrap_or_else(|| {
            std::path::PathBuf::from(format!("src/harness/scenarios/{}.json", self.scenario))
        });
        crate::harness::scenarios::load_scenario(&path)
            .map(|s| serde_json::to_string(&s.fee_stats).unwrap_or_else(|_| "{}".to_string()))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}

/// Configuration bundle for constructing a HorizonMock server.
#[derive(Debug, Clone)]
pub struct HorizonMockConfig {
    /// TCP port the server will bind to.
    pub port: u16,
    /// Path to the scenario JSON file on disk.
    pub scenario_path: std::path::PathBuf,
    /// Simulated response delay in milliseconds.
    pub delay_ms: u64,
    /// Probability [0.0, 1.0] of injecting a 500 error response.
    pub error_rate: f64,
}

impl Default for HorizonMockConfig {
    fn default() -> Self {
        Self {
            port: 3001,
            scenario_path: std::path::PathBuf::from("src/harness/scenarios/normal.json"),
            delay_ms: 0,
            error_rate: 0.0,
        }
    }
}

impl HorizonMock {
    /// Constructs a HorizonMock from the given config bundle.
    pub fn from_config(config: HorizonMockConfig) -> Self {
        Self {
            scenario: config
                .scenario_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("normal")
                .to_string(),
            delay_ms: if config.delay_ms > 0 {
                Some(config.delay_ms)
            } else {
                None
            },
            scenario_path: Some(config.scenario_path),
            error_rate: config.error_rate,
            fee_stats_response: None,
        }
    }
}

/// Starts an axum HTTP server serving mock Horizon responses.
///
/// Routes:
/// - `GET /fee_stats` — returns scenario fee stats JSON
/// - `GET /health` — returns `{"status":"ok","scenario":"<name>"}`
///
/// Binds to `0.0.0.0:port`. Returns when the server shuts down.
pub async fn serve(mock: std::sync::Arc<HorizonMock>, port: u16) -> std::io::Result<()> {
    use axum::{routing::get, Router};
    use std::net::SocketAddr;

    let m1 = mock.clone();
    let m2 = mock.clone();

    let app = Router::new()
        .route(
            "/fee_stats",
            get(move || {
                let m = m1.clone();
                async move {
                    match m.fee_stats_payload() {
                        Ok(json) => (
                            axum::http::StatusCode::OK,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            json,
                        ),
                        Err(e) => (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            format!(r#"{{"error":"{}"}}"#, e),
                        ),
                    }
                }
            }),
        )
        .route(
            "/health",
            get(move || {
                let m = m2.clone();
                async move { m.health_payload() }
            }),
        );

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(std::io::Error::other)?;
    axum::serve(listener, app)
        .await
        .map_err(std::io::Error::other)
}

/// Minimal pseudo-random float in [0.0, 1.0) using system time as entropy.
fn rand_f64() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 1_000_000) as f64 / 1_000_000.0
}
