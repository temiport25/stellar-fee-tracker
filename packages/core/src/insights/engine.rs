//! Fee Insights Engine - Central orchestrator for fee analysis

use chrono::{DateTime, Utc};
use std::time::Instant;

use crate::insights::{
    calculator::RollingAverageCalculator,
    config::{AverageConfig, ExtremesConfig, InsightsConfig},
    detector::CongestionDetector,
    error::InsightsError,
    tracker::ExtremesTracker,
    types::*,
};

/// Central fee insights engine that orchestrates all analysis operations
pub struct FeeInsightsEngine {
    config: InsightsConfig,
    calculator: RollingAverageCalculator,
    tracker: ExtremesTracker,
    detector: CongestionDetector,
    last_update: Option<DateTime<Utc>>,
    last_insights: Option<CurrentInsights>,
}

impl FeeInsightsEngine {
    /// Create a new fee insights engine with the given configuration
    pub fn new(config: InsightsConfig) -> Self {
        // Create component configurations
        let average_config = AverageConfig::default();
        let extremes_config = ExtremesConfig::default();

        // Initialize components
        let calculator = RollingAverageCalculator::new(average_config, config.time_windows.clone());
        let tracker = ExtremesTracker::new(extremes_config);
        let detector = CongestionDetector::new(config.spike_detection.clone());

        Self {
            config,
            calculator,
            tracker,
            detector,
            last_update: None,
            last_insights: None,
        }
    }

    /// Process new fee data and update insights
    pub async fn process_fee_data(
        &mut self,
        data: &[FeeDataPoint],
    ) -> Result<InsightsUpdate, InsightsError> {
        let start_time = Instant::now();
        let processing_start = Utc::now();

        if data.is_empty() {
            return Err(InsightsError::invalid_data("No fee data provided"));
        }

        // Validate fee data
        self.validate_fee_data(data)?;

        // Update rolling averages
        for fee_point in data {
            self.calculator.add_data_point(fee_point.clone());
        }

        // Update extremes tracking
        self.tracker.update_with_fees(data)?;

        // Calculate rolling averages to get baseline for congestion detection
        let rolling_averages = self.calculator.calculate_averages()?;
        let baseline = rolling_averages.medium_term.value; // Use medium-term as baseline

        // Update congestion detection
        let congestion_trends = self.detector.analyze_congestion(data, baseline)?;

        // Get current extremes
        let extremes = self
            .tracker
            .get_current_extremes()
            .unwrap_or_else(|_| self.create_default_extremes());

        // Calculate data quality
        let data_quality = self.calculate_data_quality(data, processing_start);

        // Create current insights
        let insights = CurrentInsights {
            rolling_averages,
            extremes,
            congestion_trends,
            last_updated: processing_start,
            data_quality,
        };

        // Update last update time
        self.last_update = Some(processing_start);
        self.last_insights = Some(insights.clone());

        // Calculate processing time
        let processing_time = chrono::Duration::from_std(start_time.elapsed())
            .unwrap_or_else(|_| chrono::Duration::zero());

        Ok(InsightsUpdate {
            insights,
            processing_time,
            data_points_processed: data.len(),
        })
    }

    /// Validate fee data for basic correctness
    pub fn validate_fee_data(&self, data: &[FeeDataPoint]) -> Result<(), InsightsError> {
        for (i, fee_point) in data.iter().enumerate() {
            // Check for reasonable fee amounts (not zero, not excessively large)
            if fee_point.fee_amount == 0 {
                return Err(InsightsError::invalid_data(format!(
                    "Zero fee amount at index {}",
                    i
                )));
            }

            // Check for reasonable fee amounts (Stellar fees are typically in stroops)
            if fee_point.fee_amount > 1_000_000_000 {
                // 1000 XLM in stroops
                return Err(InsightsError::invalid_data(format!(
                    "Unreasonably large fee amount {} at index {}",
                    fee_point.fee_amount, i
                )));
            }

            // Check for valid transaction hash
            if fee_point.transaction_hash.is_empty() {
                return Err(InsightsError::invalid_data(format!(
                    "Empty transaction hash at index {}",
                    i
                )));
            }

            // Check for reasonable timestamp (not too far in the future)
            let now = Utc::now();
            if fee_point.timestamp > now + chrono::Duration::hours(1) {
                return Err(InsightsError::invalid_data(format!(
                    "Future timestamp at index {}: {}",
                    i, fee_point.timestamp
                )));
            }
        }

        Ok(())
    }

    /// Calculate data quality metrics
    fn calculate_data_quality(
        &self,
        data: &[FeeDataPoint],
        processing_time: DateTime<Utc>,
    ) -> DataQuality {
        let expected_points = self.estimate_expected_data_points();
        let actual_points = data.len();

        // Calculate completeness (0.0 to 1.0)
        let completeness = if expected_points > 0 {
            (actual_points as f64 / expected_points as f64).min(1.0)
        } else {
            1.0
        };

        // Calculate freshness (time since last update)
        let freshness = match self.last_update {
            Some(last) => processing_time - last,
            None => chrono::Duration::zero(),
        };

        // Check for gaps in data (simplified - just check if we have recent data)
        let has_gaps = data.is_empty()
            || data
                .iter()
                .any(|point| processing_time - point.timestamp > chrono::Duration::hours(1));

        let last_gap = if has_gaps {
            Some(processing_time)
        } else {
            None
        };

        DataQuality {
            completeness,
            freshness,
            has_gaps,
            last_gap,
        }
    }

    /// Estimate expected number of data points based on polling interval
    fn estimate_expected_data_points(&self) -> usize {
        // Simple estimation: assume 1 data point per polling interval
        // In reality, this would depend on network activity
        match self.last_update {
            Some(last) => {
                let time_diff = Utc::now() - last;
                let intervals =
                    time_diff.num_seconds() / self.config.polling_interval.num_seconds();
                intervals.max(1) as usize
            }
            None => 1,
        }
    }

    /// Create default extremes when no data is available
    fn create_default_extremes(&self) -> FeeExtremes {
        let now = Utc::now();
        let default_extreme = ExtremeValue {
            value: 100, // Default Stellar base fee in stroops
            timestamp: now,
            transaction_hash: "unknown".to_string(),
        };

        FeeExtremes {
            current_min: default_extreme.clone(),
            current_max: default_extreme,
            period_start: now,
            period_end: now,
        }
    }

    /// Get current insights
    pub fn get_current_insights(&self) -> CurrentInsights {
        if let Some(insights) = &self.last_insights {
            return insights.clone();
        }

        let rolling_averages = self
            .calculator
            .calculate_averages()
            .unwrap_or_else(|_| self.create_default_rolling_averages());

        let extremes = self
            .tracker
            .get_current_extremes()
            .unwrap_or_else(|_| self.create_default_extremes());

        let congestion_trends = CongestionTrends {
            current_trend: TrendIndicator::Normal,
            recent_spikes: self.detector.get_recent_spikes(),
            trend_strength: self.detector.calculate_trend_strength(),
            predicted_duration: None,
        };

        let data_quality = DataQuality {
            completeness: 1.0,
            freshness: chrono::Duration::zero(),
            has_gaps: false,
            last_gap: None,
        };

        CurrentInsights {
            rolling_averages,
            extremes,
            congestion_trends,
            last_updated: self.last_update.unwrap_or_else(Utc::now),
            data_quality,
        }
    }

    /// Create default rolling averages when no data is available
    fn create_default_rolling_averages(&self) -> RollingAverages {
        let now = Utc::now();
        let default_result = AverageResult {
            value: 100.0, // Default Stellar base fee
            sample_count: 0,
            is_partial: true,
            calculated_at: now,
            time_window: TimeWindow {
                name: "default".to_string(),
                duration: chrono::Duration::hours(1),
                min_samples: 1,
            },
        };

        RollingAverages {
            short_term: default_result.clone(),
            medium_term: default_result.clone(),
            long_term: default_result,
        }
    }

    /// Get rolling averages
    pub fn get_rolling_averages(&self) -> RollingAverages {
        self.calculator
            .calculate_averages()
            .unwrap_or_else(|_| self.create_default_rolling_averages())
    }

    /// Get fee extremes
    pub fn get_extremes(&self) -> FeeExtremes {
        self.tracker
            .get_current_extremes()
            .unwrap_or_else(|_| self.create_default_extremes())
    }

    /// Get congestion trends
    pub fn get_congestion_trends(&self) -> CongestionTrends {
        CongestionTrends {
            current_trend: TrendIndicator::Normal,
            recent_spikes: self.detector.get_recent_spikes(),
            trend_strength: self.detector.calculate_trend_strength(),
            predicted_duration: None,
        }
    }

    /// Get engine configuration
    pub fn get_config(&self) -> &InsightsConfig {
        &self.config
    }

    /// Get last update time
    pub fn get_last_update(&self) -> Option<DateTime<Utc>> {
        self.last_update
    }
}
