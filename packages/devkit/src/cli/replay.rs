use std::path::PathBuf;

use clap::Args;

/// Arguments for the `replay` subcommand.
#[derive(Args)]
pub struct ReplayArgs {
    /// Path to the SQLite database file.
    pub db: PathBuf,
    /// Playback speed multiplier (1.0 = real-time).
    #[arg(long, default_value = "1.0")]
    pub speed: f32,
    /// Start of the replay window (ISO-8601 timestamp).
    #[arg(long)]
    pub from: Option<String>,
    /// End of the replay window (ISO-8601 timestamp).
    #[arg(long)]
    pub to: Option<String>,
}

impl ReplayArgs {
    /// Replays fee records filtered by the given time window.
    pub fn run(&self) {
        eprintln!(
            "Replaying from {} at {:.1}x speed, window {:?}..{:?}",
            self.db.display(),
            self.speed,
            self.from,
            self.to
        );
    /// Path to the SQLite database file containing recorded fee data.
    pub db: PathBuf,
    /// Playback speed multiplier (1.0 = real-time, 10.0 = 10x faster).
    #[arg(long, default_value = "1.0")]
    pub speed: f32,
}

impl ReplayArgs {
    /// Replays fee records at the specified speed multiplier.
    pub fn run(&self) {
        eprintln!(
            "Replaying from {} at {:.1}x speed",
            self.db.display(),
            self.speed
        );
}

impl ReplayArgs {
    /// Replays fee records from the database to stdout as a JSON stream.
    pub fn run(&self) {
        eprintln!("Replaying fee records from {}", self.db.display());
        println!("[]");
    }
}
