pub mod webhook;

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::Mutex;

use crate::insights::{InsightsUpdate, SpikeSeverity};

use self::webhook::{AlertPayload, WebhookDelivery};

#[derive(Clone)]
pub struct AlertManager {
    webhook_delivery: Option<WebhookDelivery>,
    alert_threshold: SpikeSeverity,
    network: String,
    seen_spikes: Arc<Mutex<HashSet<String>>>,
}

impl AlertManager {
    pub fn new(
        webhook_url: Option<String>,
        alert_threshold: SpikeSeverity,
        network: String,
    ) -> Self {
        let webhook_delivery = webhook_url.map(WebhookDelivery::new);
        Self {
            webhook_delivery,
            alert_threshold,
            network,
            seen_spikes: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn check_and_dispatch(&self, update: &InsightsUpdate) {
        let Some(delivery) = self.webhook_delivery.clone() else {
            return;
        };

        for spike in &update.insights.congestion_trends.recent_spikes {
            if !meets_threshold(&spike.severity, &self.alert_threshold) {
                continue;
            }

            let spike_id = format!(
                "{}:{}:{}",
                severity_to_str(&spike.severity),
                spike.start_time.timestamp(),
                spike.peak_fee
            );
            let should_dispatch = {
                let mut seen = self.seen_spikes.lock().await;
                seen.insert(spike_id)
            };

            if !should_dispatch {
                continue;
            }

            let payload = AlertPayload {
                event: "fee_spike_detected".to_string(),
                severity: severity_to_str(&spike.severity).to_string(),
                peak_fee: spike.peak_fee,
                baseline_fee: spike.baseline_fee,
                spike_ratio: spike.spike_ratio,
                start_time: spike.start_time,
                duration_seconds: spike.duration.num_seconds().max(0),
                network: self.network.clone(),
                timestamp: Utc::now(),
            };

            let delivery = delivery.clone();
            tokio::spawn(async move {
                if let Err(err) = delivery.send_with_retry(&payload).await {
                    tracing::error!("Webhook dispatch failed: {}", err);
                }
            });
        }
    }
}

fn severity_rank(severity: &SpikeSeverity) -> u8 {
    match severity {
        SpikeSeverity::Minor => 0,
        SpikeSeverity::Moderate => 1,
        SpikeSeverity::Major => 2,
        SpikeSeverity::Critical => 3,
    }
}

fn meets_threshold(severity: &SpikeSeverity, threshold: &SpikeSeverity) -> bool {
    severity_rank(severity) >= severity_rank(threshold)
}

fn severity_to_str(severity: &SpikeSeverity) -> &'static str {
    match severity {
        SpikeSeverity::Minor => "Minor",
        SpikeSeverity::Moderate => "Moderate",
        SpikeSeverity::Major => "Major",
        SpikeSeverity::Critical => "Critical",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration, Utc};
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    use crate::insights::{
        AverageResult, CongestionTrends, CurrentInsights, DataQuality, FeeSpike, RollingAverages,
        SpikeSeverity, TimeWindow, TrendIndicator, TrendStrength,
    };

    fn build_update_with_spike(severity: SpikeSeverity) -> InsightsUpdate {
        let now = DateTime::parse_from_rfc3339("2025-01-14T10:47:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let spike = FeeSpike {
            peak_fee: 5000,
            baseline_fee: 130.5,
            spike_ratio: 38.3,
            start_time: DateTime::parse_from_rfc3339("2025-01-14T10:45:00Z")
                .unwrap()
                .with_timezone(&Utc),
            duration: Duration::seconds(120),
            severity,
        };
        let window = TimeWindow {
            name: "1h".to_string(),
            duration: Duration::hours(1),
            min_samples: 1,
        };
        let avg = AverageResult {
            value: 130.5,
            sample_count: 10,
            is_partial: false,
            calculated_at: now,
            time_window: window.clone(),
        };

        InsightsUpdate {
            insights: CurrentInsights {
                rolling_averages: RollingAverages {
                    short_term: avg.clone(),
                    medium_term: avg.clone(),
                    long_term: avg,
                },
                extremes: crate::insights::FeeExtremes {
                    current_min: crate::insights::ExtremeValue {
                        value: 100,
                        timestamp: now,
                        transaction_hash: "min".to_string(),
                    },
                    current_max: crate::insights::ExtremeValue {
                        value: 5000,
                        timestamp: now,
                        transaction_hash: "max".to_string(),
                    },
                    period_start: now - Duration::hours(1),
                    period_end: now,
                },
                congestion_trends: CongestionTrends {
                    current_trend: TrendIndicator::Rising,
                    recent_spikes: vec![spike],
                    trend_strength: TrendStrength::Strong,
                    predicted_duration: None,
                },
                last_updated: now,
                data_quality: DataQuality {
                    completeness: 1.0,
                    freshness: Duration::seconds(5),
                    has_gaps: false,
                    last_gap: None,
                },
            },
            processing_time: Duration::milliseconds(1),
            data_points_processed: 1,
        }
    }

    #[tokio::test]
    async fn spike_above_threshold_dispatches_webhook() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let manager = AlertManager::new(
            Some(format!("{}/hook", server.uri())),
            SpikeSeverity::Major,
            "mainnet".to_string(),
        );
        let update = build_update_with_spike(SpikeSeverity::Critical);

        manager.check_and_dispatch(&update).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn spike_below_threshold_is_not_dispatched() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let manager = AlertManager::new(
            Some(format!("{}/hook", server.uri())),
            SpikeSeverity::Critical,
            "mainnet".to_string(),
        );
        let update = build_update_with_spike(SpikeSeverity::Major);

        manager.check_and_dispatch(&update).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn same_spike_is_dispatched_once() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let manager = AlertManager::new(
            Some(format!("{}/hook", server.uri())),
            SpikeSeverity::Major,
            "mainnet".to_string(),
        );
        let update = build_update_with_spike(SpikeSeverity::Major);

        manager.check_and_dispatch(&update).await;
        manager.check_and_dispatch(&update).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
