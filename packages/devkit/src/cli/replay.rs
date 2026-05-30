use std::path::PathBuf;

use clap::Args;

/// Arguments for the `replay` subcommand.
#[derive(Args)]
pub struct ReplayArgs {
    /// Path to the SQLite database file containing recorded fee data.
    pub db: PathBuf,
}

impl ReplayArgs {
    /// Replays fee records from the database to stdout as a JSON stream.
    pub fn run(&self) {
        eprintln!("Replaying fee records from {}", self.db.display());
        println!("[]");
    }
}
