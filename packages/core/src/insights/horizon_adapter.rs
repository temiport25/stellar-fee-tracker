//! Horizon Fee Data Provider Adapter
//!
//! Adapts the HorizonClient to implement the FeeDataProvider trait

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::str::FromStr;

// No direct reqwest import needed — we use the pooled client from HorizonClient.

use crate::insights::{
    error::ProviderError,
    provider::{FeeDataProvider, ProviderMetadata, ProviderResult},
    types::FeeDataPoint,
};
use crate::services::horizon::HorizonClient;

/// Adapter that implements FeeDataProvider for HorizonClient
pub struct HorizonFeeDataProvider {
    client: HorizonClient,
    metadata: ProviderMetadata,
}

/// Horizon transaction response for fee data extraction
#[derive(Debug, Deserialize)]
struct HorizonTransactionResponse {
    #[serde(rename = "_embedded")]
    embedded: HorizonEmbedded,
}

#[derive(Debug, Deserialize)]
struct HorizonEmbedded {
    records: Vec<HorizonTransactionRecord>,
}

#[derive(Debug, Deserialize)]
struct HorizonTransactionRecord {
    pub hash: String,
    pub ledger: u64,
    pub created_at: String,
    pub fee_charged: String,
    pub successful: bool,
}

impl HorizonFeeDataProvider {
    /// Create a new Horizon fee data provider
    pub fn new(client: HorizonClient) -> Self {
        let metadata = ProviderMetadata {
            supports_historical: true,
            max_batch_size: 200,               // Horizon's default limit
            rate_limit_per_minute: Some(3600), // Horizon's rate limit
            data_freshness_seconds: 5,         // Stellar ledger close time
        };

        Self { client, metadata }
    }

    /// Fetch recent transactions from Horizon using the shared pooled HTTP client.
    async fn fetch_recent_transactions(
        &self,
        limit: u32,
    ) -> ProviderResult<Vec<HorizonTransactionRecord>> {
        let url = format!(
            "{}/transactions?order=desc&limit={}",
            self.client.base_url(),
            limit
        );

        // Use the pooled client from HorizonClient instead of spawning ephemeral
        // reqwest clients, so we get TCP connection reuse across poll ticks.
        let response = self
            .client
            .http_client()
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::NetworkError {
                message: format!("Failed to fetch transactions: {}", e),
            })?;

        if !response.status().is_success() {
            return Err(ProviderError::NetworkError {
                message: format!("Horizon returned HTTP {}", response.status()),
            });
        }

        let transaction_response: HorizonTransactionResponse =
            response
                .json()
                .await
                .map_err(|e| ProviderError::FormatError {
                    message: format!("Failed to parse transaction response: {}", e),
                })?;

        Ok(transaction_response.embedded.records)
    }

    /// Convert Horizon transaction record to FeeDataPoint
    fn convert_to_fee_data_point(
        &self,
        record: HorizonTransactionRecord,
    ) -> ProviderResult<FeeDataPoint> {
        // Only include successful transactions
        if !record.successful {
            return Err(ProviderError::FormatError {
                message: "Transaction was not successful".to_string(),
            });
        }

        // Parse fee amount
        let fee_amount =
            u64::from_str(&record.fee_charged).map_err(|e| ProviderError::FormatError {
                message: format!("Invalid fee amount '{}': {}", record.fee_charged, e),
            })?;

        // Parse timestamp
        let timestamp = DateTime::parse_from_rfc3339(&record.created_at)
            .map_err(|e| ProviderError::FormatError {
                message: format!("Invalid timestamp '{}': {}", record.created_at, e),
            })?
            .with_timezone(&Utc);

        Ok(FeeDataPoint {
            fee_amount,
            timestamp,
            transaction_hash: record.hash,
            ledger_sequence: record.ledger,
        })
    }
}

#[async_trait]
impl FeeDataProvider for HorizonFeeDataProvider {
    async fn fetch_latest_fees(&self) -> ProviderResult<Vec<FeeDataPoint>> {
        // Fetch recent transactions (last 100 by default)
        let transactions = self.fetch_recent_transactions(100).await?;

        // Convert to fee data points, filtering out failed conversions
        let mut fee_data_points = Vec::new();
        for transaction in transactions {
            match self.convert_to_fee_data_point(transaction) {
                Ok(fee_point) => fee_data_points.push(fee_point),
                Err(e) => {
                    // Log the error but continue processing other transactions
                    tracing::warn!("Failed to convert transaction to fee data point: {}", e);
                }
            }
        }

        if fee_data_points.is_empty() {
            return Err(ProviderError::FormatError {
                message: "No valid fee data points found in recent transactions".to_string(),
            });
        }

        Ok(fee_data_points)
    }

    fn provider_name(&self) -> &str {
        "Horizon"
    }

    async fn health_check(&self) -> ProviderResult<()> {
        // Use the existing fee_stats endpoint for health check
        self.client
            .fetch_fee_stats()
            .await
            .map_err(|e| ProviderError::NetworkError {
                message: format!("Horizon health check failed: {}", e),
            })?;

        Ok(())
    }

    fn get_metadata(&self) -> ProviderMetadata {
        self.metadata.clone()
    }
}
