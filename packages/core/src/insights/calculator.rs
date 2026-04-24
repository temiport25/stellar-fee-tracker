//! Rolling Average Calculator

use chrono::{DateTime, Utc};
use std::collections::{HashMap, VecDeque};

use crate::insights::{config::AverageConfig, error::InsightsError, types::*};

/// Circular buffer for efficient storage of fee data points
#[derive(Debug, Clone)]
struct CircularBuffer<T> {
    data: VecDeque<T>,
    max_size: usize,
}

impl<T> CircularBuffer<T> {
    fn new(max_size: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    fn push(&mut self, item: T) {
        if self.data.len() >= self.max_size {
            self.data.pop_front();
        }
        self.data.push_back(item);
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        self.data.iter()
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Calculator for rolling averages across multiple time windows
pub struct RollingAverageCalculator {
    #[allow(dead_code)]
    config: AverageConfig,
    windows: HashMap<TimeWindow, CircularBuffer<FeeDataPoint>>,
    time_windows: Vec<TimeWindow>,
}

impl RollingAverageCalculator {
    /// Create a new rolling average calculator
    pub fn new(config: AverageConfig, time_windows: Vec<TimeWindow>) -> Self {
        let mut windows = HashMap::new();

        // Initialize circular buffers for each time window
        for window in &time_windows {
            windows.insert(window.clone(), CircularBuffer::new(config.max_buffer_size));
        }

        Self {
            config,
            windows,
            time_windows,
        }
    }

    /// Add a new data point to all relevant time windows
    pub fn add_data_point(&mut self, point: FeeDataPoint) {
        let now = Utc::now();

        // Add to each time window if the point is within the window duration
        for window in &self.time_windows {
            if let Some(buffer) = self.windows.get_mut(window) {
                // Check if the data point is within the time window
                let window_start = now - window.duration;
                if point.timestamp >= window_start {
                    buffer.push(point.clone());
                }
            }
        }

        // Clean old data points from all buffers
        self.clean_old_data(now);
    }

    /// Clean old data points that are outside their respective time windows
    fn clean_old_data(&mut self, current_time: DateTime<Utc>) {
        for (window, buffer) in &mut self.windows {
            let window_start = current_time - window.duration;

            // Remove data points that are too old
            while let Some(front) = buffer.data.front() {
                if front.timestamp < window_start {
                    buffer.data.pop_front();
                } else {
                    break;
                }
            }
        }
    }

    /// Calculate averages for all time windows
    pub fn calculate_averages(&self) -> Result<RollingAverages, InsightsError> {
        let now = Utc::now();

        // Calculate average for each predefined window type
        let short_term = self.calculate_average_for_window("short_term", now)?;
        let medium_term = self.calculate_average_for_window("medium_term", now)?;
        let long_term = self.calculate_average_for_window("long_term", now)?;

        Ok(RollingAverages {
            short_term,
            medium_term,
            long_term,
        })
    }

    /// Calculate average for a specific time window by name
    fn calculate_average_for_window(
        &self,
        window_name: &str,
        calculated_at: DateTime<Utc>,
    ) -> Result<AverageResult, InsightsError> {
        // Find the time window by name
        let time_window = self
            .time_windows
            .iter()
            .find(|w| w.name == window_name)
            .ok_or_else(|| {
                InsightsError::config_error(format!("Time window '{}' not found", window_name))
            })?;

        let buffer = self.windows.get(time_window).ok_or_else(|| {
            InsightsError::config_error(format!("Buffer for window '{}' not found", window_name))
        })?;

        if buffer.is_empty() {
            return Ok(AverageResult {
                value: 0.0,
                sample_count: 0,
                is_partial: true,
                calculated_at,
                time_window: time_window.clone(),
            });
        }

        // Calculate the average fee
        let total_fee: u64 = buffer.iter().map(|point| point.fee_amount).sum();
        let sample_count = buffer.len();
        let average = total_fee as f64 / sample_count as f64;

        // Determine if this is a partial result (insufficient samples)
        let is_partial = sample_count < time_window.min_samples;

        Ok(AverageResult {
            value: average,
            sample_count,
            is_partial,
            calculated_at,
            time_window: time_window.clone(),
        })
    }

    /// Get average for a specific time window
    pub fn get_average_for_window(&self, window: &TimeWindow) -> Option<AverageResult> {
        let buffer = self.windows.get(window)?;

        if buffer.is_empty() {
            return None;
        }

        let total_fee: u64 = buffer.iter().map(|point| point.fee_amount).sum();
        let sample_count = buffer.len();
        let average = total_fee as f64 / sample_count as f64;
        let is_partial = sample_count < window.min_samples;

        Some(AverageResult {
            value: average,
            sample_count,
            is_partial,
            calculated_at: Utc::now(),
            time_window: window.clone(),
        })
    }

    /// Get the number of data points in a specific time window
    pub fn get_sample_count(&self, window: &TimeWindow) -> usize {
        self.windows
            .get(window)
            .map(|buffer| buffer.len())
            .unwrap_or(0)
    }

    /// Check if a time window has sufficient data for reliable calculations
    pub fn has_sufficient_data(&self, window: &TimeWindow) -> bool {
        let sample_count = self.get_sample_count(window);
        sample_count >= window.min_samples
    }
}
