//! Pre-built test scenarios for the Stellar fee tracker harness.

use std::path::Path;

/// Loads a scenario JSON file from the given path and returns its contents.
pub fn load_from_file(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

/// Cycles through a list of scenario names, returning the next one each call.
pub struct ScenarioRotator {
    scenarios: Vec<String>,
    index: usize,
}

impl ScenarioRotator {
    pub fn new(scenarios: Vec<String>) -> Self {
        Self {
            scenarios,
            index: 0,
        }
    }

    /// Returns the current scenario name and advances to the next.
    pub fn advance(&mut self) -> Option<&str> {
        if self.scenarios.is_empty() {
            return None;
        }
        let current = self.scenarios[self.index].as_str();
        self.index = (self.index + 1) % self.scenarios.len();
        Some(current)
    }
}
