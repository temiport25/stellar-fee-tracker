/// Models for simulating Stellar transaction fee behaviour.
pub struct FeeModel;

/// A fee curve snapshot matching the Horizon `fee_stats` shape (#119).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FeeCurve {
    pub last_ledger: String,
    pub last_ledger_base_fee: String,
    pub ledger_capacity_usage: String,
    pub fee_charged: FeePercentiles,
    pub max_fee: FeePercentiles,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FeePercentiles {
    pub max: String,
    pub min: String,
    pub mode: String,
    pub p10: String,
    pub p20: String,
    pub p30: String,
    pub p40: String,
    pub p50: String,
    pub p60: String,
    pub p70: String,
    pub p80: String,
    pub p90: String,
    pub p95: String,
    pub p99: String,
}

impl FeeModel {
    /// Generate a `FeeCurve` scaled by `pressure` (0.0–1.0) (#122).
    ///
    /// At pressure 0 fees stay at `base_fee`; at pressure 1 they reach `max_fee`.
    pub fn generate(base_fee: u64, max_fee: u64, pressure: f64, ledger_seq: u64) -> FeeCurve {
        let pressure = pressure.clamp(0.0, 1.0);
        let fee = |pct: f64| -> String {
            let scaled = base_fee as f64 + (max_fee - base_fee) as f64 * pressure * pct;
            (scaled as u64).to_string()
        };

        FeeCurve {
            last_ledger: ledger_seq.to_string(),
            last_ledger_base_fee: base_fee.to_string(),
            ledger_capacity_usage: format!("{:.2}", pressure),
            fee_charged: FeePercentiles {
                min: base_fee.to_string(),
                max: fee(1.0),
                mode: fee(0.6),
                p10: fee(0.1),
                p20: fee(0.2),
                p30: fee(0.3),
                p40: fee(0.4),
                p50: fee(0.5),
                p60: fee(0.6),
                p70: fee(0.7),
                p80: fee(0.8),
                p90: fee(0.9),
                p95: fee(0.95),
                p99: fee(0.99),
            },
            max_fee: FeePercentiles {
                min: base_fee.to_string(),
                max: fee(1.0),
                mode: fee(0.7),
                p10: fee(0.15),
                p20: fee(0.25),
                p30: fee(0.35),
                p40: fee(0.45),
                p50: fee(0.55),
                p60: fee(0.65),
                p70: fee(0.75),
                p80: fee(0.85),
                p90: fee(0.92),
                p95: fee(0.97),
                p99: fee(1.0),
            },
        }
    }

    /// Serialise a `FeeCurve` to a JSON string (#119).
    pub fn to_json(curve: &FeeCurve) -> Result<String, serde_json::Error> {
        serde_json::to_string(curve)
    }
}
