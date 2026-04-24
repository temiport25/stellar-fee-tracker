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
            std::path::PathBuf::from(format!(
                "src/harness/scenarios/{}.json",
                self.scenario
            ))
        });
        crate::harness::scenarios::load_from_file(&path)
    }
}

/// Minimal pseudo-random float in [0.0, 1.0) using system time as entropy.
fn rand_f64() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 1_000_000) as f64 / 1_000_000.0
}
