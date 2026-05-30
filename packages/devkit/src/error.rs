use thiserror::Error;

/// Unified error type for the devkit.
#[derive(Debug, Error)]
pub enum DevkitError {
    /// An error originating from the simulation engine (e.g. invalid parameters
    /// or model configuration failures).
    #[error("simulation error: {0}")]
    Simulation(String),

    /// An error originating from the test harness (e.g. failed to start the
    /// mock server or load a scenario file).
    #[error("harness error: {0}")]
    Harness(String),

    /// An error originating from the analysis layer (e.g. insufficient data
    /// points for percentile computation or rolling-window operations).
    #[error("analysis error: {0}")]
    Analysis(String),

    /// A transparent wrapper around [`std::io::Error`] for file and network I/O
    /// failures encountered while loading scenario data or writing output.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
