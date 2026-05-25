//! Extremes Tracker for min/max fee values

use chrono::{DateTime, Utc};
use std::collections::VecDeque;

use crate::insights::{config::ExtremesConfig, error::InsightsError, types::*};

/// Represents a tracking period for extremes
#[derive(Debug, Clone)]
struct ExtremePeriod {
    min_value: Option<ExtremeValue>,
    max_value: Option<ExtremeValue>,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
}

impl ExtremePeriod {
    fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            min_value: None,
            max_value: None,
            period_start: start,
            period_end: end,
        }
    }

    fn update_with_fee(&mut self, fee_point: &FeeDataPoint) {
        let extreme_value = ExtremeValue {
            value: fee_point.fee_amount,
            timestamp: fee_point.timestamp,
            transaction_hash: fee_point.transaction_hash.clone(),
        };

        // Update minimum
        match &self.min_value {
            None => self.min_value = Some(extreme_value.clone()),
            Some(current_min) => {
                if fee_point.fee_amount < current_min.value {
                    self.min_value = Some(extreme_value.clone());
                }
            }
        }

        // Update maximum
        match &self.max_value {
            None => self.max_value = Some(extreme_value),
            Some(current_max) => {
                if fee_point.fee_amount > current_max.value {
                    self.max_value = Some(extreme_value);
                }
            }
        }
    }

    fn to_fee_extremes(&self) -> Option<FeeExtremes> {
        match (&self.min_value, &self.max_value) {
            (Some(min), Some(max)) => Some(FeeExtremes {
                current_min: min.clone(),
                current_max: max.clone(),
                period_start: self.period_start,
                period_end: self.period_end,
            }),
            _ => None,
        }
    }
}

/// Tracker for minimum and maximum fee values
pub struct ExtremesTracker {
    config: ExtremesConfig,
    current_period: ExtremePeriod,
    historical_periods: VecDeque<ExtremePeriod>,
}

impl ExtremesTracker {
    /// Create a new extremes tracker
    pub fn new(config: ExtremesConfig) -> Self {
        let now = Utc::now();
        let period_start = now;
        let period_end = now + config.tracking_period;

        Self {
            config,
            current_period: ExtremePeriod::new(period_start, period_end),
            historical_periods: VecDeque::new(),
        }
    }

    /// Update with new fee data
    pub fn update_with_fees(&mut self, fees: &[FeeDataPoint]) -> Result<(), InsightsError> {
        let now = Utc::now();

        // Check if we need to rotate to a new period
        if now >= self.current_period.period_end {
            self.rotate_period(now)?;
        }

        // Update current period with new fees
        for fee_point in fees {
            // Only process fees that are within the current tracking period
            if fee_point.timestamp >= self.current_period.period_start
                && fee_point.timestamp <= self.current_period.period_end
            {
                self.current_period.update_with_fee(fee_point);
            }
        }

        Ok(())
    }

    /// Rotate to a new tracking period, preserving the current period as historical
    fn rotate_period(&mut self, current_time: DateTime<Utc>) -> Result<(), InsightsError> {
        // Move current period to historical periods
        let completed_period = std::mem::replace(
            &mut self.current_period,
            ExtremePeriod::new(current_time, current_time + self.config.tracking_period),
        );

        self.historical_periods.push_back(completed_period);

        // Maintain the configured number of historical periods
        while self.historical_periods.len() > self.config.historical_periods_to_keep {
            self.historical_periods.pop_front();
        }

        Ok(())
    }

    /// Get current extremes
    pub fn get_current_extremes(&self) -> Result<FeeExtremes, InsightsError> {
        self.current_period.to_fee_extremes().ok_or_else(|| {
            InsightsError::insufficient_data("No fee data available for current period")
        })
    }
}
