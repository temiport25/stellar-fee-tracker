use crate::simulation::fee_model::FeePoint;
use std::fmt::Write as FmtWrite;

/// Exports devkit results to external formats.
pub struct Export;

impl Export {
    /// Serialize fee points to CSV: timestamp,fee,ledger,is_spike.
    pub fn to_csv(points: &[FeePoint]) -> String {
        let mut out = String::from("timestamp,fee,ledger,is_spike\n");
        for p in points {
            writeln!(out, "{},{},{},{}", p.timestamp, p.fee, p.ledger, p.is_spike).unwrap();
        }
        out
    }

    /// Write fee points to a CSV file.
    pub fn write_csv(points: &[FeePoint], path: &std::path::Path) -> std::io::Result<()> {
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

    fn sample() -> Vec<FeePoint> {
        vec![FeePoint { timestamp: 1000, fee: 100, ledger: 1, is_spike: false }]
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