use crate::simulation::fee_model::FeePoint;
use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};

/// Arguments for the `export` subcommand.
pub struct ExportArgs {
    /// Source SQLite database path.
    pub db: PathBuf,
    /// Output file path. Writes to stdout when `None`.
    pub output: Option<PathBuf>,
}

impl ExportArgs {
    /// Run the export, writing CSV to file or stdout.
    pub fn run(&self, points: &[FeePoint]) {
        match &self.output {
            Some(path) => Export::write_csv(points, path).expect("failed to write output"),
            None => print!("{}", Export::to_csv(points)),
        }
    }
}

/// Time window filter for exports.
#[derive(Debug, Clone, Copy)]
pub enum Window {
    OneHour,
    SixHours,
    TwentyFourHours,
    All,
}

impl Window {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "1h" => Some(Self::OneHour),
            "6h" => Some(Self::SixHours),
            "24h" => Some(Self::TwentyFourHours),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn cutoff_seconds(&self) -> Option<u64> {
        match self {
            Self::OneHour => Some(3600),
            Self::SixHours => Some(21600),
            Self::TwentyFourHours => Some(86400),
            Self::All => None,
        }
    }
}

/// Exports devkit results to external formats.
pub struct Export;

impl Export {
    /// Filter points by window relative to the latest timestamp.
    pub fn filter_window(points: &[FeePoint], window: Window) -> &[FeePoint] {
        match window.cutoff_seconds() {
            None => points,
            Some(secs) => {
                let max_ts = points.iter().map(|p| p.timestamp).max().unwrap_or(0);
                let cutoff = max_ts.saturating_sub(secs);
                let start = points.partition_point(|p| p.timestamp < cutoff);
                &points[start..]
            }
        }
    }

    /// Serialize fee points to CSV: timestamp,fee,ledger,is_spike.
    pub fn to_csv(points: &[FeePoint]) -> String {
        let mut out = String::from("timestamp,fee,ledger,is_spike\n");
        for p in points {
            writeln!(out, "{},{},{},{}", p.timestamp, p.fee, p.ledger, p.is_spike).unwrap();
        }
        out
    }

    /// Write fee points to a CSV file.
    pub fn write_csv(points: &[FeePoint], path: &Path) -> std::io::Result<()> {
        std::fs::write(path, Self::to_csv(points))
    }

    /// Serialize fee points to a JSON array.
    pub fn to_json(points: &[FeePoint]) -> String {
        let items: Vec<String> = points
            .iter()
            .map(|p| {
                format!(
                    r#"{{"timestamp":{},"fee":{},"ledger":{},"is_spike":{}}}"#,
                    p.timestamp, p.fee, p.ledger, p.is_spike
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// Write fee points to a JSON file.
    pub fn write_json(points: &[FeePoint], path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, Self::to_json(points))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::fee_model::FeePoint;

    fn pts() -> Vec<FeePoint> {
        vec![
            FeePoint {
                timestamp: 0,
                fee: 100,
                ledger: 1,
                is_spike: false,
            },
            FeePoint {
                timestamp: 7200,
                fee: 200,
                ledger: 2,
                is_spike: true,
            },
        ]
    }

    fn sample() -> Vec<FeePoint> {
        vec![FeePoint {
            timestamp: 1000,
            fee: 100,
            ledger: 1,
            is_spike: false,
        }]
    }

    #[test]
    fn window_1h_filters() {
        let p = pts();
        let filtered = Export::filter_window(&p, Window::OneHour);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].timestamp, 7200);
    }

    #[test]
    fn window_all_keeps_all() {
        let p = pts();
        assert_eq!(Export::filter_window(&p, Window::All).len(), 2);
    }

    #[test]
    fn csv_has_header() {
        let csv = Export::to_csv(&sample());
        assert!(csv.starts_with("timestamp,fee,ledger,is_spike\n"));
    }

    #[test]
    fn json_is_array() {
        let json = Export::to_json(&sample());
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
    }
}
