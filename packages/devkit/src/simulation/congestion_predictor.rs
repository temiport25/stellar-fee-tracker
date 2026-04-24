//! Predicts network congestion based on simulated load and fee models.

/// Simple congestion level based on raw tx count and fee values.
#[derive(Debug, PartialEq)]
pub enum CongestionLevel {
    Low,
    Moderate,
    High,
    Critical,
}

/// Predicts network congestion based on simulated load and fee models.
pub struct CongestionPredictor;

impl CongestionPredictor {
    /// Classify congestion given `tx_count` transactions and `fee` in stroops.
    pub fn predict(tx_count: u64, fee: u64) -> CongestionLevel {
        match (tx_count, fee) {
            (t, f) if t >= 800 || f >= 5_000 => CongestionLevel::Critical,
            (t, f) if t >= 500 || f >= 1_000 => CongestionLevel::High,
            (t, f) if t >= 200 || f >= 300 => CongestionLevel::Moderate,
            _ => CongestionLevel::Low,
        }
    }
}

/// Input data for weighted congestion scoring.
pub struct CongestionInput {
    /// Average fee over a recent window (in stroops).
    pub recent_fee_window: f64,
    /// Ledger capacity usage as a fraction (0.0–1.0).
    pub capacity_usage: f64,
    /// Number of fee spikes observed in the window.
    pub spike_count: u32,
}

/// Congestion severity label derived from a weighted score.
#[derive(Debug, PartialEq)]
pub enum CongestionLabel {
    Normal,
    Rising,
    Congested,
    Critical,
}

/// Returns a congestion score in [0.0, 1.0] based on weighted inputs.
pub fn congestion_score(input: &CongestionInput) -> f64 {
    let fee_score = (input.recent_fee_window / 500_000.0).clamp(0.0, 1.0);
    let spike_score = (input.spike_count as f64 / 10.0).clamp(0.0, 1.0);
    let score = 0.5 * input.capacity_usage + 0.3 * fee_score + 0.2 * spike_score;
    score.clamp(0.0, 1.0)
}

/// Maps a congestion score to a human-readable label.
pub fn congestion_label(score: f64) -> CongestionLabel {
    match score {
        s if s < 0.3 => CongestionLabel::Normal,
        s if s < 0.6 => CongestionLabel::Rising,
        s if s <= 0.85 => CongestionLabel::Congested,
        _ => CongestionLabel::Critical,
    }
}
