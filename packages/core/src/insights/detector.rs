//! Congestion Detection System

use chrono::{Duration, Utc};
use std::collections::VecDeque;

use crate::insights::{config::SpikeConfig, error::InsightsError, types::*};

/// Analyzer for trend patterns
#[derive(Debug, Clone)]
struct TrendAnalyzer {
    recent_spikes: VecDeque<FeeSpike>,
    congestion_window: Duration,
}

impl TrendAnalyzer {
    fn new(congestion_window: Duration) -> Self {
        Self {
            recent_spikes: VecDeque::new(),
            congestion_window,
        }
    }

    fn add_spike(&mut self, spike: FeeSpike) {
        self.recent_spikes.push_back(spike);
        self.clean_old_spikes();
    }

    fn clean_old_spikes(&mut self) {
        let cutoff_time = Utc::now() - self.congestion_window;

        while let Some(front_spike) = self.recent_spikes.front() {
            if front_spike.start_time < cutoff_time {
                self.recent_spikes.pop_front();
            } else {
                break;
            }
        }
    }

    fn calculate_trend_strength(&self) -> TrendStrength {
        let spike_count = self.recent_spikes.len();
        let total_severity_score: f64 = self
            .recent_spikes
            .iter()
            .map(|spike| match spike.severity {
                SpikeSeverity::Minor => 1.0,
                SpikeSeverity::Moderate => 2.0,
                SpikeSeverity::Major => 4.0,
                SpikeSeverity::Critical => 8.0,
            })
            .sum();

        // Calculate strength based on both count and severity
        let strength_score = total_severity_score + (spike_count as f64 * 0.5);

        match strength_score {
            s if s >= 10.0 => TrendStrength::Strong,
            s if s >= 4.0 => TrendStrength::Moderate,
            s if s > 0.0 => TrendStrength::Weak,
            _ => TrendStrength::Weak,
        }
    }

    fn determine_trend_indicator(&self) -> TrendIndicator {
        if self.recent_spikes.is_empty() {
            return TrendIndicator::Normal;
        }

        let recent_spikes: Vec<_> = self.recent_spikes.iter().collect();
        let spike_count = recent_spikes.len();

        // Analyze trend based on recent spike patterns
        if spike_count >= 3 {
            // Check if spikes are increasing in severity
            let avg_recent_ratio = recent_spikes
                .iter()
                .rev()
                .take(2)
                .map(|s| s.spike_ratio)
                .sum::<f64>()
                / 2.0;

            let avg_older_ratio = recent_spikes
                .iter()
                .take(recent_spikes.len() - 2)
                .map(|s| s.spike_ratio)
                .sum::<f64>()
                / (recent_spikes.len() - 2) as f64;

            if avg_recent_ratio > avg_older_ratio * 1.2 {
                TrendIndicator::Rising
            } else if avg_recent_ratio < avg_older_ratio * 0.8 {
                TrendIndicator::Declining
            } else {
                TrendIndicator::Congested
            }
        } else if spike_count >= 1 {
            // Single spike or few spikes
            let latest_spike = recent_spikes.last().unwrap();
            match latest_spike.severity {
                SpikeSeverity::Critical | SpikeSeverity::Major => TrendIndicator::Congested,
                _ => TrendIndicator::Rising,
            }
        } else {
            TrendIndicator::Normal
        }
    }

    fn predict_duration(&self) -> Option<Duration> {
        if self.recent_spikes.is_empty() {
            return None;
        }

        // Simple prediction based on recent spike patterns
        let avg_duration: i64 = self
            .recent_spikes
            .iter()
            .map(|spike| spike.duration.num_minutes())
            .sum::<i64>()
            / self.recent_spikes.len() as i64;

        // Predict based on trend strength
        let multiplier = match self.calculate_trend_strength() {
            TrendStrength::Strong => 2.0,
            TrendStrength::Moderate => 1.5,
            TrendStrength::Weak => 1.0,
        };

        Some(Duration::minutes((avg_duration as f64 * multiplier) as i64))
    }
}

/// Detector for network congestion through fee spike analysis
pub struct CongestionDetector {
    config: SpikeConfig,
    trend_analyzer: TrendAnalyzer,
    historical_spikes: VecDeque<FeeSpike>,
}

impl CongestionDetector {
    /// Create a new congestion detector
    pub fn new(config: SpikeConfig) -> Self {
        Self {
            trend_analyzer: TrendAnalyzer::new(config.congestion_window),
            config,
            historical_spikes: VecDeque::new(),
        }
    }

    /// Analyze congestion patterns
    pub fn analyze_congestion(
        &mut self,
        current_fees: &[FeeDataPoint],
        baseline: f64,
    ) -> Result<CongestionTrends, InsightsError> {
        // Detect new spikes in the current fee data
        let new_spikes = self.detect_spikes(current_fees, baseline)?;

        // Add new spikes to the trend analyzer
        for spike in &new_spikes {
            self.trend_analyzer.add_spike(spike.clone());
            self.historical_spikes.push_back(spike.clone());
        }

        // Maintain historical spike buffer (keep last 1000 spikes)
        while self.historical_spikes.len() > 1000 {
            self.historical_spikes.pop_front();
        }

        // Clean old spikes from trend analyzer
        self.trend_analyzer.clean_old_spikes();

        // Calculate current trend indicators
        let current_trend = self.trend_analyzer.determine_trend_indicator();
        let trend_strength = self.trend_analyzer.calculate_trend_strength();
        let predicted_duration = self.trend_analyzer.predict_duration();

        // Get recent spikes for the response
        let recent_spikes: Vec<FeeSpike> =
            self.trend_analyzer.recent_spikes.iter().cloned().collect();

        Ok(CongestionTrends {
            current_trend,
            recent_spikes,
            trend_strength,
            predicted_duration,
        })
    }

    /// Detect fee spikes in the given fee data
    pub fn detect_spikes(
        &self,
        fees: &[FeeDataPoint],
        baseline: f64,
    ) -> Result<Vec<FeeSpike>, InsightsError> {
        if fees.is_empty() {
            return Ok(Vec::new());
        }

        if baseline <= 0.0 {
            return Err(InsightsError::invalid_data("Baseline must be positive"));
        }

        let mut spikes = Vec::new();
        let threshold = baseline * self.config.threshold_multiplier;

        // Sort fees by timestamp to process in chronological order
        let mut sorted_fees = fees.to_vec();
        sorted_fees.sort_by_key(|a| a.timestamp);

        let mut current_spike: Option<FeeSpike> = None;

        for fee_point in &sorted_fees {
            let fee_amount = fee_point.fee_amount as f64;

            if fee_amount >= threshold {
                // This is a spike
                match &mut current_spike {
                    None => {
                        // Start a new spike
                        current_spike = Some(FeeSpike {
                            peak_fee: fee_point.fee_amount,
                            baseline_fee: baseline,
                            spike_ratio: fee_amount / baseline,
                            start_time: fee_point.timestamp,
                            duration: Duration::zero(),
                            severity: self.classify_spike_severity(fee_amount / baseline),
                        });
                    }
                    Some(spike) => {
                        // Continue existing spike, update peak if necessary
                        if fee_point.fee_amount > spike.peak_fee {
                            spike.peak_fee = fee_point.fee_amount;
                            spike.spike_ratio = fee_amount / baseline;
                            spike.severity = self.classify_spike_severity(fee_amount / baseline);
                        }
                        spike.duration = fee_point.timestamp - spike.start_time;
                    }
                }
            } else {
                // Not a spike, end current spike if it exists
                if let Some(mut spike) = current_spike.take() {
                    spike.duration = fee_point.timestamp - spike.start_time;

                    // Only include spikes that meet minimum duration
                    if spike.duration >= self.config.minimum_spike_duration {
                        spikes.push(spike);
                    }
                }
            }
        }

        // Handle case where spike continues to the end of data
        if let Some(mut spike) = current_spike {
            if let Some(last_fee) = sorted_fees.last() {
                spike.duration = last_fee.timestamp - spike.start_time;

                if spike.duration >= self.config.minimum_spike_duration {
                    spikes.push(spike);
                }
            }
        }

        Ok(spikes)
    }

    /// Classify the severity of a spike based on its ratio to baseline
    pub fn classify_spike_severity(&self, spike_ratio: f64) -> SpikeSeverity {
        match spike_ratio {
            r if r >= 10.0 => SpikeSeverity::Critical,
            r if r >= 5.0 => SpikeSeverity::Major,
            r if r >= 3.0 => SpikeSeverity::Moderate,
            _ => SpikeSeverity::Minor,
        }
    }

    /// Calculate trend strength based on recent spike activity
    pub fn calculate_trend_strength(&self) -> TrendStrength {
        self.trend_analyzer.calculate_trend_strength()
    }

    /// Get recent spikes within the congestion window
    pub fn get_recent_spikes(&self) -> Vec<FeeSpike> {
        self.trend_analyzer.recent_spikes.iter().cloned().collect()
    }

    /// Get all historical spikes
    pub fn get_historical_spikes(&self) -> Vec<FeeSpike> {
        self.historical_spikes.iter().cloned().collect()
    }

    /// Clear all spike history (useful for testing or reset scenarios)
    pub fn clear_history(&mut self) {
        self.trend_analyzer.recent_spikes.clear();
        self.historical_spikes.clear();
    }
}
