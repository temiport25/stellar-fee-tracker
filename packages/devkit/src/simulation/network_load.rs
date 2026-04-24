/// Generates synthetic network load profiles for simulation.
pub struct NetworkLoad;

/// Configuration for simulated network load (#120).
#[derive(Debug, Clone)]
pub struct NetworkLoadConfig {
    /// Maximum transactions the network can process per ledger.
    pub ledger_capacity: u32,
    /// Number of transactions submitted per ledger.
    pub tx_per_ledger: u32,
    /// Time between ledger closes in milliseconds.
    pub ledger_interval_ms: u64,
}

/// A single simulated ledger produced by the throughput simulator (#121).
#[derive(Debug, Clone)]
pub struct SimulatedLedger {
    pub ledger_seq: u64,
    pub tx_count: u32,
    /// Capacity pressure in [0.0, 1.0]: tx_per_ledger / ledger_capacity (#122).
    pub pressure: f64,
}

impl NetworkLoadConfig {
    /// Returns the capacity pressure ratio (#122).
    pub fn pressure(&self) -> f64 {
        self.tx_per_ledger as f64 / self.ledger_capacity as f64
    }
}

impl NetworkLoad {
    /// Simulate `ledger_count` ledger closes and return the resulting ledgers (#121).
    pub fn simulate(config: &NetworkLoadConfig, ledger_count: u64) -> Vec<SimulatedLedger> {
        (0..ledger_count)
            .map(|seq| SimulatedLedger {
                ledger_seq: seq + 1,
                tx_count: config.tx_per_ledger,
                pressure: config.pressure(),
            })
            .collect()
    }
}
