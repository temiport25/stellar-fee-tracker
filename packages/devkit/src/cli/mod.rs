pub mod benchmark;
pub mod export;
pub mod replay;

use clap::{Parser, Subcommand};

/// Developer toolkit for the Stellar fee tracker.
#[derive(Parser)]
#[command(name = "devkit", about = "Stellar fee tracker developer toolkit")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Replay recorded fee scenarios
    Replay,
    /// Export data to external formats
    Export,
    /// Run performance benchmarks
    Benchmark,
    /// Serve mock fee data
    Mock,
}
