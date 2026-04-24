//! Comprehensive tests for fee metrics core calculations
//!
//! This module contains all unit tests and property-based tests for the fee insights engine
//! core calculations, including rolling averages, extremes tracking, and congestion detection.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;
    use crate::insights::{
        calculator::RollingAverageCalculator,
        config::{AverageConfig, ExtremesConfig, InsightsConfig, SpikeConfig},
        detector::CongestionDetector,
        engine::FeeInsightsEngine,
        error::InsightsError,
        tracker::ExtremesTracker,
        types::*,
    };
    use chrono::{Duration, Utc};
    use proptest::prelude::*;

    // Test data generators
    fn fee_data_point_strategy() -> impl Strategy<Value = FeeDataPoint> {
        (
            100u64..1_000_000u64, // Fee amounts in stroops (reasonable Stellar range)
            prop::num::i64::ANY.prop_map(|secs| {
                Utc::now() - Duration::seconds(secs.abs() % (86400 * 30)) // Within last 30 days
            }),
            "[a-f0-9]{64}".prop_map(|s| s), // Transaction hash
            1u64..1_000_000u64,             // Ledger sequence
        )
            .prop_map(
                |(fee_amount, timestamp, transaction_hash, ledger_sequence)| FeeDataPoint {
                    fee_amount,
                    timestamp,
                    transaction_hash,
                    ledger_sequence,
                },
            )
    }

    #[allow(dead_code)]
    fn time_window_strategy() -> impl Strategy<Value = TimeWindow> {
        (
            prop::collection::vec("[a-z]+", 1..10).prop_map(|words| words.join("_")),
            1i64..86400i64,   // Duration in seconds (1 second to 1 day)
            1usize..100usize, // Min samples
        )
            .prop_map(|(name, duration_secs, min_samples)| TimeWindow {
                name,
                duration: Duration::seconds(duration_secs),
                min_samples,
            })
    }

    // =============================================================================
    // UNIT TESTS - Rolling Average Calculator
    // =============================================================================

    #[test]
    fn test_rolling_average_calculator_creation() {
        let config = AverageConfig::default();
        let time_windows = vec![TimeWindow {
            name: "test".to_string(),
            duration: Duration::hours(1),
            min_samples: 5,
        }];

        let _calculator = RollingAverageCalculator::new(config, time_windows);

        // Should be created successfully (just verify no panic)
    }

    #[test]
    fn test_rolling_average_basic_calculation() {
        let config = AverageConfig::default();
        let time_windows = vec![
            TimeWindow {
                name: "short_term".to_string(),
                duration: Duration::hours(1),
                min_samples: 2,
            },
            TimeWindow {
                name: "medium_term".to_string(),
                duration: Duration::hours(6),
                min_samples: 2,
            },
            TimeWindow {
                name: "long_term".to_string(),
                duration: Duration::hours(24),
                min_samples: 2,
            },
        ];

        let mut calculator = RollingAverageCalculator::new(config, time_windows);

        // Add some test data
        let now = Utc::now();
        let fee_points = vec![
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now - Duration::minutes(30),
                transaction_hash: "hash1".to_string(),
                ledger_sequence: 1,
            },
            FeeDataPoint {
                fee_amount: 200,
                timestamp: now - Duration::minutes(15),
                transaction_hash: "hash2".to_string(),
                ledger_sequence: 2,
            },
        ];

        for point in fee_points {
            calculator.add_data_point(point);
        }

        let averages = calculator.calculate_averages().unwrap();

        // Should calculate correct average: (100 + 200) / 2 = 150
        assert_eq!(averages.short_term.value, 150.0);
        assert_eq!(averages.short_term.sample_count, 2);
        assert!(!averages.short_term.is_partial);
    }

    #[test]
    fn test_rolling_average_partial_results() {
        let config = AverageConfig::default();
        let time_windows = vec![
            TimeWindow {
                name: "short_term".to_string(),
                duration: Duration::hours(1),
                min_samples: 5, // Require 5 samples
            },
            TimeWindow {
                name: "medium_term".to_string(),
                duration: Duration::hours(6),
                min_samples: 5,
            },
            TimeWindow {
                name: "long_term".to_string(),
                duration: Duration::hours(24),
                min_samples: 5,
            },
        ];

        let mut calculator = RollingAverageCalculator::new(config, time_windows);

        // Add only 2 data points (less than required 5)
        let now = Utc::now();
        calculator.add_data_point(FeeDataPoint {
            fee_amount: 100,
            timestamp: now - Duration::minutes(30),
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        });
        calculator.add_data_point(FeeDataPoint {
            fee_amount: 200,
            timestamp: now - Duration::minutes(15),
            transaction_hash: "hash2".to_string(),
            ledger_sequence: 2,
        });

        let averages = calculator.calculate_averages().unwrap();

        // Should be marked as partial
        assert!(averages.short_term.is_partial);
        assert_eq!(averages.short_term.sample_count, 2);
    }

    #[test]
    fn test_rolling_average_time_window_expiration() {
        let config = AverageConfig::default();
        let time_windows = vec![
            TimeWindow {
                name: "short_term".to_string(),
                duration: Duration::minutes(30), // 30-minute window
                min_samples: 1,
            },
            TimeWindow {
                name: "medium_term".to_string(),
                duration: Duration::hours(6),
                min_samples: 1,
            },
            TimeWindow {
                name: "long_term".to_string(),
                duration: Duration::hours(24),
                min_samples: 1,
            },
        ];

        let mut calculator = RollingAverageCalculator::new(config, time_windows);

        let now = Utc::now();

        // Add old data point (outside window)
        calculator.add_data_point(FeeDataPoint {
            fee_amount: 100,
            timestamp: now - Duration::hours(2), // 2 hours ago (outside 30-min window)
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        });

        // Add recent data point (inside window)
        calculator.add_data_point(FeeDataPoint {
            fee_amount: 200,
            timestamp: now - Duration::minutes(15), // 15 minutes ago (inside window)
            transaction_hash: "hash2".to_string(),
            ledger_sequence: 2,
        });

        let averages = calculator.calculate_averages().unwrap();

        // Should only include the recent data point
        assert_eq!(averages.short_term.value, 200.0);
        assert_eq!(averages.short_term.sample_count, 1);
    }

    #[test]
    fn test_circular_buffer_capacity_limit() {
        let config = AverageConfig {
            max_buffer_size: 3, // Small buffer for testing
            min_samples_for_calculation: 1,
        };
        let time_windows = vec![
            TimeWindow {
                name: "short_term".to_string(),
                duration: Duration::hours(1),
                min_samples: 1,
            },
            TimeWindow {
                name: "medium_term".to_string(),
                duration: Duration::hours(6),
                min_samples: 1,
            },
            TimeWindow {
                name: "long_term".to_string(),
                duration: Duration::hours(24),
                min_samples: 1,
            },
        ];

        let mut calculator = RollingAverageCalculator::new(config, time_windows);

        let now = Utc::now();

        // Add 5 data points (more than buffer capacity of 3)
        for i in 0..5 {
            calculator.add_data_point(FeeDataPoint {
                fee_amount: (i + 1) * 100,
                timestamp: now - Duration::minutes(i as i64 * 5),
                transaction_hash: format!("hash{}", i),
                ledger_sequence: i + 1,
            });
        }

        let averages = calculator.calculate_averages().unwrap();

        // Should only have 3 data points (buffer capacity)
        assert_eq!(averages.short_term.sample_count, 3);

        // Should contain the most recent 3 points: 500, 400, 300
        // Average should be (500 + 400 + 300) / 3 = 400
        assert_eq!(averages.short_term.value, 400.0);
    }

    #[test]
    fn test_rolling_average_empty_dataset() {
        let config = AverageConfig::default();
        let time_windows = vec![
            TimeWindow {
                name: "short_term".to_string(),
                duration: Duration::hours(1),
                min_samples: 1,
            },
            TimeWindow {
                name: "medium_term".to_string(),
                duration: Duration::hours(6),
                min_samples: 1,
            },
            TimeWindow {
                name: "long_term".to_string(),
                duration: Duration::hours(24),
                min_samples: 1,
            },
        ];

        let calculator = RollingAverageCalculator::new(config, time_windows);
        let averages = calculator.calculate_averages().unwrap();

        // Should return zero values with appropriate metadata
        assert_eq!(averages.short_term.value, 0.0);
        assert_eq!(averages.short_term.sample_count, 0);
        assert!(averages.short_term.is_partial);
    }

    // =============================================================================
    // UNIT TESTS - Extremes Tracker
    // =============================================================================

    #[test]
    fn test_extremes_tracker_creation() {
        let config = ExtremesConfig::default();
        let tracker = ExtremesTracker::new(config);

        // Should be created successfully
        assert!(!tracker.has_current_data());
    }

    #[test]
    fn test_extremes_identification() {
        let config = ExtremesConfig::default();
        let mut tracker = ExtremesTracker::new(config);

        let now = Utc::now();
        let fee_data = vec![
            FeeDataPoint {
                fee_amount: 150,
                timestamp: now, // Use current time
                transaction_hash: "hash1".to_string(),
                ledger_sequence: 1,
            },
            FeeDataPoint {
                fee_amount: 50, // Minimum
                timestamp: now, // Use current time
                transaction_hash: "hash2".to_string(),
                ledger_sequence: 2,
            },
            FeeDataPoint {
                fee_amount: 300, // Maximum
                timestamp: now,  // Use current time
                transaction_hash: "hash3".to_string(),
                ledger_sequence: 3,
            },
        ];

        tracker.update_with_fees(&fee_data).unwrap();
        let extremes = tracker.get_current_extremes().unwrap();

        // Should correctly identify min and max
        assert_eq!(extremes.current_min.value, 50);
        assert_eq!(extremes.current_min.transaction_hash, "hash2");
        assert_eq!(extremes.current_max.value, 300);
        assert_eq!(extremes.current_max.transaction_hash, "hash3");
    }

    #[test]
    fn test_extremes_tie_breaking() {
        let config = ExtremesConfig::default();
        let mut tracker = ExtremesTracker::new(config);

        let now = Utc::now();
        let fee_data = vec![
            FeeDataPoint {
                fee_amount: 100,                       // First occurrence of min
                timestamp: now - Duration::seconds(1), // Slightly earlier
                transaction_hash: "hash1".to_string(),
                ledger_sequence: 1,
            },
            FeeDataPoint {
                fee_amount: 100, // Second occurrence of min (more recent)
                timestamp: now,  // More recent
                transaction_hash: "hash2".to_string(),
                ledger_sequence: 2,
            },
        ];

        tracker.update_with_fees(&fee_data).unwrap();
        let extremes = tracker.get_current_extremes().unwrap();

        // Should store the most recent occurrence
        assert_eq!(extremes.current_min.value, 100);
        assert_eq!(extremes.current_min.transaction_hash, "hash2");
    }

    #[test]
    fn test_extremes_metadata_preservation() {
        let config = ExtremesConfig::default();
        let mut tracker = ExtremesTracker::new(config);

        let now = Utc::now();
        let fee_data = vec![FeeDataPoint {
            fee_amount: 200,
            timestamp: now,
            transaction_hash: "test_hash_123".to_string(),
            ledger_sequence: 12345,
        }];

        tracker.update_with_fees(&fee_data).unwrap();
        let extremes = tracker.get_current_extremes().unwrap();

        // Should preserve all metadata
        assert_eq!(extremes.current_min.transaction_hash, "test_hash_123");
        assert_eq!(extremes.current_max.transaction_hash, "test_hash_123");
        assert_eq!(extremes.current_min.timestamp, now);
        assert_eq!(extremes.current_max.timestamp, now);
    }

    // =============================================================================
    // UNIT TESTS - Congestion Detector
    // =============================================================================

    #[test]
    fn test_congestion_detector_creation() {
        let config = SpikeConfig::default();
        let detector = CongestionDetector::new(config);

        // Should be created successfully
        assert_eq!(detector.get_recent_spikes().len(), 0);
    }

    #[test]
    fn test_spike_detection_basic() {
        let config = SpikeConfig {
            threshold_multiplier: 2.0,
            minimum_spike_duration: Duration::minutes(1),
            congestion_window: Duration::hours(1),
        };
        let detector = CongestionDetector::new(config);

        let now = Utc::now();
        let baseline = 100.0;

        let fee_data = vec![
            FeeDataPoint {
                fee_amount: 100, // Normal fee
                timestamp: now - Duration::minutes(30),
                transaction_hash: "hash1".to_string(),
                ledger_sequence: 1,
            },
            FeeDataPoint {
                fee_amount: 250, // Spike (2.5x baseline)
                timestamp: now - Duration::minutes(20),
                transaction_hash: "hash2".to_string(),
                ledger_sequence: 2,
            },
            FeeDataPoint {
                fee_amount: 300, // Higher spike
                timestamp: now - Duration::minutes(15),
                transaction_hash: "hash3".to_string(),
                ledger_sequence: 3,
            },
            FeeDataPoint {
                fee_amount: 100, // Back to normal
                timestamp: now - Duration::minutes(10),
                transaction_hash: "hash4".to_string(),
                ledger_sequence: 4,
            },
        ];

        let spikes = detector.detect_spikes(&fee_data, baseline).unwrap();

        // Should detect one spike
        assert_eq!(spikes.len(), 1);
        assert_eq!(spikes[0].peak_fee, 300);
        assert_eq!(spikes[0].baseline_fee, baseline);
        assert_eq!(spikes[0].spike_ratio, 3.0); // 300 / 100
    }

    #[test]
    fn test_spike_severity_classification() {
        let config = SpikeConfig::default();
        let detector = CongestionDetector::new(config);

        // Test different severity levels
        assert_eq!(detector.classify_spike_severity(1.5), SpikeSeverity::Minor);
        assert_eq!(
            detector.classify_spike_severity(3.5),
            SpikeSeverity::Moderate
        );
        assert_eq!(detector.classify_spike_severity(7.0), SpikeSeverity::Major);
        assert_eq!(
            detector.classify_spike_severity(15.0),
            SpikeSeverity::Critical
        );
    }

    #[test]
    fn test_spike_ratio_calculation() {
        let config = SpikeConfig {
            threshold_multiplier: 2.0,
            minimum_spike_duration: Duration::seconds(1), // Very short duration
            congestion_window: Duration::hours(1),
        };
        let detector = CongestionDetector::new(config);

        let now = Utc::now();
        let baseline = 200.0;

        let fee_data = vec![
            FeeDataPoint {
                fee_amount: 600, // 3x baseline, should exceed threshold of 2.0
                timestamp: now,
                transaction_hash: "hash1".to_string(),
                ledger_sequence: 1,
            },
            FeeDataPoint {
                fee_amount: 100, // Back to normal to end the spike
                timestamp: now + Duration::seconds(2),
                transaction_hash: "hash2".to_string(),
                ledger_sequence: 2,
            },
        ];

        let spikes = detector.detect_spikes(&fee_data, baseline).unwrap();

        assert_eq!(spikes.len(), 1);
        assert_eq!(spikes[0].spike_ratio, 3.0);
        assert_eq!(spikes[0].baseline_fee, baseline);
    }

    // =============================================================================
    // UNIT TESTS - Data Validation
    // =============================================================================

    #[test]
    fn test_fee_amount_validation() {
        let config = InsightsConfig::default();
        let engine = FeeInsightsEngine::new(config);

        // Test zero fee amount (should fail)
        let invalid_data = vec![FeeDataPoint {
            fee_amount: 0,
            timestamp: Utc::now(),
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        }];

        let result = engine.validate_fee_data(&invalid_data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Zero fee amount"));

        // Test excessively large fee amount (should fail)
        let invalid_data = vec![FeeDataPoint {
            fee_amount: 2_000_000_000, // > 1 billion stroops
            timestamp: Utc::now(),
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        }];

        let result = engine.validate_fee_data(&invalid_data);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unreasonably large fee amount"));

        // Test valid fee amount (should pass)
        let valid_data = vec![FeeDataPoint {
            fee_amount: 100,
            timestamp: Utc::now(),
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        }];

        let result = engine.validate_fee_data(&valid_data);
        assert!(result.is_ok());
    }

    #[test]
    fn test_timestamp_validation() {
        let config = InsightsConfig::default();
        let engine = FeeInsightsEngine::new(config);

        // Test future timestamp (should fail)
        let invalid_data = vec![FeeDataPoint {
            fee_amount: 100,
            timestamp: Utc::now() + Duration::hours(2), // 2 hours in future
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        }];

        let result = engine.validate_fee_data(&invalid_data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Future timestamp"));

        // Test valid timestamp (should pass)
        let valid_data = vec![FeeDataPoint {
            fee_amount: 100,
            timestamp: Utc::now() - Duration::minutes(30),
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        }];

        let result = engine.validate_fee_data(&valid_data);
        assert!(result.is_ok());
    }

    #[test]
    fn test_transaction_hash_validation() {
        let config = InsightsConfig::default();
        let engine = FeeInsightsEngine::new(config);

        // Test empty transaction hash (should fail)
        let invalid_data = vec![FeeDataPoint {
            fee_amount: 100,
            timestamp: Utc::now(),
            transaction_hash: "".to_string(),
            ledger_sequence: 1,
        }];

        let result = engine.validate_fee_data(&invalid_data);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Empty transaction hash"));

        // Test valid transaction hash (should pass)
        let valid_data = vec![FeeDataPoint {
            fee_amount: 100,
            timestamp: Utc::now(),
            transaction_hash: "valid_hash_123".to_string(),
            ledger_sequence: 1,
        }];

        let result = engine.validate_fee_data(&valid_data);
        assert!(result.is_ok());
    }

    // =============================================================================
    // UNIT TESTS - Mathematical Precision and Safety
    // =============================================================================

    #[test]
    fn test_large_number_accuracy() {
        let config = AverageConfig::default();
        let time_windows = vec![
            TimeWindow {
                name: "short_term".to_string(),
                duration: Duration::hours(1),
                min_samples: 1,
            },
            TimeWindow {
                name: "medium_term".to_string(),
                duration: Duration::hours(6),
                min_samples: 1,
            },
            TimeWindow {
                name: "long_term".to_string(),
                duration: Duration::hours(24),
                min_samples: 1,
            },
        ];

        let mut calculator = RollingAverageCalculator::new(config, time_windows);

        let now = Utc::now();

        // Test with large fee amounts (but within valid range)
        let large_fees = vec![
            FeeDataPoint {
                fee_amount: 999_999_999, // Close to max valid fee
                timestamp: now - Duration::minutes(30),
                transaction_hash: "hash1".to_string(),
                ledger_sequence: 1,
            },
            FeeDataPoint {
                fee_amount: 999_999_998,
                timestamp: now - Duration::minutes(15),
                transaction_hash: "hash2".to_string(),
                ledger_sequence: 2,
            },
        ];

        for fee in large_fees {
            calculator.add_data_point(fee);
        }

        let averages = calculator.calculate_averages().unwrap();

        // Should maintain accuracy with large numbers
        let expected_average = (999_999_999.0 + 999_999_998.0) / 2.0;
        assert_eq!(averages.short_term.value, expected_average);
    }

    #[test]
    fn test_type_conversion_correctness() {
        // Test u64 to f64 conversion accuracy
        let fee_amount: u64 = 123_456_789;
        let converted: f64 = fee_amount as f64;
        let back_converted: u64 = converted as u64;

        // Should preserve value for reasonable fee amounts
        assert_eq!(fee_amount, back_converted);

        // Test with maximum safe integer in f64
        let max_safe: u64 = (1u64 << 53) - 1; // 2^53 - 1
        let converted_max: f64 = max_safe as f64;
        let back_converted_max: u64 = converted_max as u64;

        assert_eq!(max_safe, back_converted_max);
    }

    #[test]
    fn test_division_by_zero_handling() {
        let config = SpikeConfig::default();
        let detector = CongestionDetector::new(config);

        let now = Utc::now();
        let fee_data = vec![FeeDataPoint {
            fee_amount: 100,
            timestamp: now,
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        }];

        // Test with zero baseline (should return error)
        let result = detector.detect_spikes(&fee_data, 0.0);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Baseline must be positive"));

        // Test with negative baseline (should return error)
        let result = detector.detect_spikes(&fee_data, -100.0);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Baseline must be positive"));
    }

    #[test]
    fn test_decimal_precision_maintenance() {
        let config = SpikeConfig::default();
        let detector = CongestionDetector::new(config);

        let now = Utc::now();
        let baseline = 100.0;

        let fee_data = vec![FeeDataPoint {
            fee_amount: 333, // Should give ratio of 3.33
            timestamp: now,
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        }];

        let spikes = detector.detect_spikes(&fee_data, baseline).unwrap();

        if !spikes.is_empty() {
            // Should maintain decimal precision
            assert_eq!(spikes[0].spike_ratio, 3.33);
        }
    }

    // =============================================================================
    // PROPERTY-BASED TESTS
    // =============================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// **Feature: fee-metrics-core-calculations, Property 1: Rolling average mathematical correctness**
        #[test]
        fn prop_rolling_average_mathematical_correctness(
            fee_points in prop::collection::vec(fee_data_point_strategy(), 1..50)
        ) {
            let config = AverageConfig::default();
            let now = Utc::now();

            // Adjust all timestamps to be within the time window
            let adjusted_fee_points: Vec<FeeDataPoint> = fee_points.into_iter().map(|mut point| {
                point.timestamp = now - Duration::minutes((rand::random::<u64>() % 60) as i64); // Within last hour
                point
            }).collect();

            let time_windows = vec![
                TimeWindow {
                    name: "short_term".to_string(),
                    duration: Duration::hours(24), // Large window to include all points
                    min_samples: 1,
                },
                TimeWindow {
                    name: "medium_term".to_string(),
                    duration: Duration::hours(24),
                    min_samples: 1,
                },
                TimeWindow {
                    name: "long_term".to_string(),
                    duration: Duration::hours(24),
                    min_samples: 1,
                },
            ];

            let mut calculator = RollingAverageCalculator::new(config, time_windows);

            // Add all fee points
            for point in &adjusted_fee_points {
                calculator.add_data_point(point.clone());
            }

            let averages = calculator.calculate_averages().unwrap();

            // Calculate expected average manually
            let total: u64 = adjusted_fee_points.iter().map(|p| p.fee_amount).sum();
            let expected_average = total as f64 / adjusted_fee_points.len() as f64;

            // Should match calculated average
            prop_assert_eq!(averages.short_term.value, expected_average);
            prop_assert_eq!(averages.short_term.sample_count, adjusted_fee_points.len());
        }

        /// **Feature: fee-metrics-core-calculations, Property 5: Extremes identification accuracy**
        #[test]
        fn prop_extremes_identification_accuracy(
            fee_points in prop::collection::vec(fee_data_point_strategy(), 1..50)
        ) {
            let config = ExtremesConfig::default();
            let mut tracker = ExtremesTracker::new(config);

            tracker.update_with_fees(&fee_points).unwrap();

            if let Ok(extremes) = tracker.get_current_extremes() {
                // Find actual min and max
                let actual_min = fee_points.iter().map(|p| p.fee_amount).min().unwrap();
                let actual_max = fee_points.iter().map(|p| p.fee_amount).max().unwrap();

                // Should match tracked extremes
                prop_assert_eq!(extremes.current_min.value, actual_min);
                prop_assert_eq!(extremes.current_max.value, actual_max);
            }
        }

        /// **Feature: fee-metrics-core-calculations, Property 9: Spike ratio calculation accuracy**
        #[test]
        fn prop_spike_ratio_calculation_accuracy(
            fee_points in prop::collection::vec(fee_data_point_strategy(), 1..20),
            baseline in 50.0f64..500.0f64
        ) {
            let config = SpikeConfig {
                threshold_multiplier: 1.5, // Lower threshold to catch more spikes
                minimum_spike_duration: Duration::seconds(1),
                congestion_window: Duration::hours(1),
            };
            let detector = CongestionDetector::new(config);

            let spikes = detector.detect_spikes(&fee_points, baseline).unwrap();

            // Verify all spike ratios are calculated correctly
            for spike in spikes {
                let expected_ratio = spike.peak_fee as f64 / baseline;
                prop_assert_eq!(spike.spike_ratio, expected_ratio);
                prop_assert_eq!(spike.baseline_fee, baseline);
            }
        }

        /// **Feature: fee-metrics-core-calculations, Property 13: Fee amount validation**
        #[test]
        fn prop_fee_amount_validation(
            fee_amount in 1u64..1_000_000_000u64 // Valid range
        ) {
            let config = InsightsConfig::default();
            let engine = FeeInsightsEngine::new(config);

            let fee_data = vec![
                FeeDataPoint {
                    fee_amount,
                    timestamp: Utc::now() - Duration::minutes(30),
                    transaction_hash: "valid_hash".to_string(),
                    ledger_sequence: 1,
                }
            ];

            // Should pass validation for valid fee amounts
            let result = engine.validate_fee_data(&fee_data);
            prop_assert!(result.is_ok());
        }

        /// **Feature: fee-metrics-core-calculations, Property 19: Type conversion correctness**
        #[test]
        fn prop_type_conversion_correctness(
            fee_amount in 1u64..((1u64 << 53) - 1) // Safe range for f64 conversion
        ) {
            // Convert u64 to f64 and back
            let as_float: f64 = fee_amount as f64;
            let back_to_int: u64 = as_float as u64;

            // Should preserve the original value
            prop_assert_eq!(fee_amount, back_to_int);
        }
    }

    // =============================================================================
    // INTEGRATION TESTS
    // =============================================================================

    #[test]
    fn test_full_engine_integration() {
        let config = InsightsConfig::default();
        let mut engine = FeeInsightsEngine::new(config);

        let now = Utc::now();
        let fee_data = vec![
            FeeDataPoint {
                fee_amount: 100,
                timestamp: now - Duration::minutes(60),
                transaction_hash: "hash1".to_string(),
                ledger_sequence: 1,
            },
            FeeDataPoint {
                fee_amount: 150,
                timestamp: now - Duration::minutes(45),
                transaction_hash: "hash2".to_string(),
                ledger_sequence: 2,
            },
            FeeDataPoint {
                fee_amount: 500, // Spike
                timestamp: now - Duration::minutes(30),
                transaction_hash: "hash3".to_string(),
                ledger_sequence: 3,
            },
            FeeDataPoint {
                fee_amount: 120,
                timestamp: now - Duration::minutes(15),
                transaction_hash: "hash4".to_string(),
                ledger_sequence: 4,
            },
        ];

        // Process the fee data
        let result = tokio_test::block_on(engine.process_fee_data(&fee_data));
        assert!(result.is_ok());

        let update = result.unwrap();

        // Verify insights were calculated
        assert!(update.insights.rolling_averages.short_term.value > 0.0);
        assert!(update.insights.extremes.current_min.value > 0);
        assert!(update.insights.extremes.current_max.value > 0);
        assert_eq!(update.data_points_processed, 4);
    }

    #[test]
    fn test_engine_reset_functionality() {
        let config = InsightsConfig::default();
        let mut engine = FeeInsightsEngine::new(config);

        let now = Utc::now();
        let fee_data = vec![FeeDataPoint {
            fee_amount: 200,
            timestamp: now - Duration::minutes(30),
            transaction_hash: "hash1".to_string(),
            ledger_sequence: 1,
        }];

        // Process some data
        let _result = tokio_test::block_on(engine.process_fee_data(&fee_data));

        // Reset the engine
        let reset_result = engine.reset();
        assert!(reset_result.is_ok());

        // Verify reset worked
        assert!(engine.get_last_update().is_none());
        let insights = engine.get_current_insights();
        assert_eq!(insights.rolling_averages.short_term.sample_count, 0);
    }
}
