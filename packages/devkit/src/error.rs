use thiserror::Error;

/// Unified error type for the devkit.
#[derive(Debug, Error)]
pub enum DevkitError {
    #[error("simulation error: {0}")]
    Simulation(String),

    #[error("harness error: {0}")]
    Harness(String),

    #[error("analysis error: {0}")]
    Analysis(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
