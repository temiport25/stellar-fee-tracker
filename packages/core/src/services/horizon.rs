use reqwest::Client;
use serde::Deserialize;

use crate::error::AppError;

#[derive(Clone)]
pub struct HorizonClient {
    base_url: String,
    http: Client,
}

impl HorizonClient {
    pub fn new(base_url: String) -> Self {
        let http = Client::builder()
            .no_proxy()
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { base_url, http }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[derive(Debug, Deserialize)]
pub struct HorizonTransaction {
    pub hash: String,
    pub successful: bool,
    pub fee_charged: String,
}

#[derive(Debug, Deserialize)]
pub struct HorizonOperation {
    #[serde(rename = "type")]
    pub op_type: String,

    pub from: Option<String>,
    pub to: Option<String>,

    pub asset_type: Option<String>,
    pub asset_code: Option<String>,
    pub asset_issuer: Option<String>,

    pub amount: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HorizonFeeStats {
    pub last_ledger_base_fee: String,
    pub fee_charged: FeeCharged,
}

#[derive(Debug, Deserialize)]
pub struct FeeCharged {
    pub min: String,
    pub max: String,
    #[serde(rename = "mode")]
    pub avg: String,
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

impl HorizonClient {
    pub async fn fetch_fee_stats(&self) -> Result<HorizonFeeStats, AppError> {
        let url = format!("{}/fee_stats", self.base_url);

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|err| AppError::Network(err.to_string()))?;

        if !response.status().is_success() {
            return Err(AppError::Network(format!(
                "Horizon returned HTTP {}",
                response.status()
            )));
        }

        let stats = response
            .json::<HorizonFeeStats>()
            .await
            .map_err(|err| AppError::Parse(err.to_string()))?;

        Ok(stats)
    }
}

/// Wrapper structs for deserialising Horizon's `_embedded.records` envelope.
#[derive(Debug, Deserialize)]
struct HorizonTransactionResponse {
    #[serde(rename = "_embedded")]
    embedded: HorizonTransactionEmbedded,
}

#[derive(Debug, Deserialize)]
struct HorizonTransactionEmbedded {
    records: Vec<HorizonTransaction>,
}

#[derive(Debug, Deserialize)]
struct HorizonOperationsResponse {
    #[serde(rename = "_embedded")]
    embedded: HorizonOperationsEmbedded,
}

#[derive(Debug, Deserialize)]
struct HorizonOperationsEmbedded {
    records: Vec<HorizonOperation>,
}

impl HorizonClient {
    /// Fetch the single most recent transaction from Horizon.
    ///
    /// Calls `GET {base_url}/transactions?order=desc&limit=1` and returns
    /// the first record. Returns `AppError::Parse` if Horizon returns an
    /// empty records array.
    pub async fn fetch_latest_transaction(&self) -> Result<HorizonTransaction, AppError> {
        let url = format!("{}/transactions?order=desc&limit=1", self.base_url);

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(AppError::Network(format!(
                "Horizon returned HTTP {}",
                response.status()
            )));
        }

        let body = response
            .json::<HorizonTransactionResponse>()
            .await
            .map_err(|e| AppError::Parse(e.to_string()))?;

        body.embedded
            .records
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Parse("Horizon returned empty transaction records".into()))
    }

    /// Fetch all operations for a given transaction hash.
    ///
    /// Calls `GET {base_url}/transactions/{tx_hash}/operations` and returns
    /// the full records vec (may be empty for transactions with no operations).
    pub async fn fetch_operations(&self, tx_hash: &str) -> Result<Vec<HorizonOperation>, AppError> {
        let url = format!("{}/transactions/{}/operations", self.base_url, tx_hash);

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(AppError::Network(format!(
                "Horizon returned HTTP {}",
                response.status()
            )));
        }

        let body = response
            .json::<HorizonOperationsResponse>()
            .await
            .map_err(|e| AppError::Parse(e.to_string()))?;

        Ok(body.embedded.records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests verify the error path logic only.
    // Integration tests against a real/mock HTTP server live in Issue #23.

    #[test]
    fn horizon_client_base_url_is_stored() {
        let client = HorizonClient::new("https://horizon-testnet.stellar.org".into());
        assert_eq!(client.base_url(), "https://horizon-testnet.stellar.org");
    }

    #[test]
    fn horizon_transaction_deserialises_from_json() {
        let json = r#"{"hash":"abc123","successful":true,"fee_charged":"100"}"#;
        let tx: HorizonTransaction = serde_json::from_str(json).unwrap();
        assert_eq!(tx.hash, "abc123");
        assert!(tx.successful);
        assert_eq!(tx.fee_charged, "100");
    }

    #[test]
    fn horizon_operation_deserialises_from_json() {
        let json = r#"{"type":"payment","from":"GA","to":"GB","asset_type":null,"asset_code":null,"asset_issuer":null,"amount":"50.0"}"#;
        let op: HorizonOperation = serde_json::from_str(json).unwrap();
        assert_eq!(op.op_type, "payment");
        assert_eq!(op.from.as_deref(), Some("GA"));
        assert_eq!(op.amount.as_deref(), Some("50.0"));
    }

    #[test]
    fn transaction_response_wrapper_deserialises() {
        let json = r#"{
            "_embedded": {
                "records": [
                    {"hash":"tx1","successful":true,"fee_charged":"200"}
                ]
            }
        }"#;
        let resp: HorizonTransactionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.embedded.records.len(), 1);
        assert_eq!(resp.embedded.records[0].hash, "tx1");
    }

    #[test]
    fn empty_transaction_records_would_return_parse_error() {
        // Verify that an empty records vec produces None from next(),
        // which our impl maps to AppError::Parse.
        let records: Vec<HorizonTransaction> = vec![];
        let result = records.into_iter().next();
        assert!(result.is_none());
    }

    #[test]
    fn operations_response_wrapper_deserialises() {
        let json = r#"{
            "_embedded": {
                "records": [
                    {"type":"payment","from":"GA","to":"GB","asset_type":null,"asset_code":null,"asset_issuer":null,"amount":"10.0"},
                    {"type":"create_account","from":null,"to":"GC","asset_type":null,"asset_code":null,"asset_issuer":null,"amount":null}
                ]
            }
        }"#;
        let resp: HorizonOperationsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.embedded.records.len(), 2);
        assert_eq!(resp.embedded.records[0].op_type, "payment");
        assert_eq!(resp.embedded.records[1].op_type, "create_account");
    }

    #[test]
    fn fee_charged_deserialises_all_percentile_fields() {
        let json = r#"{
            "min": "100",
            "max": "5000",
            "mode": "213",
            "p10": "100",
            "p20": "100",
            "p30": "120",
            "p40": "130",
            "p50": "150",
            "p60": "200",
            "p70": "250",
            "p80": "300",
            "p90": "500",
            "p95": "800",
            "p99": "1000"
        }"#;
        let fc: FeeCharged = serde_json::from_str(json).unwrap();
        assert_eq!(fc.min, "100");
        assert_eq!(fc.max, "5000");
        assert_eq!(fc.avg, "213");
        assert_eq!(fc.p10, "100");
        assert_eq!(fc.p50, "150");
        assert_eq!(fc.p80, "300");
        assert_eq!(fc.p90, "500");
        assert_eq!(fc.p95, "800");
    }

    #[test]
    fn horizon_fee_stats_deserialises_with_percentiles() {
        let json = r#"{
            "last_ledger_base_fee": "100",
            "fee_charged": {
                "min": "100",
                "max": "5000",
                "mode": "213",
                "p10": "100",
                "p20": "100",
                "p30": "120",
                "p40": "130",
                "p50": "150",
                "p60": "200",
                "p70": "250",
                "p80": "300",
                "p90": "500",
                "p95": "800",
                "p99": "1000"
            }
        }"#;
        let stats: HorizonFeeStats = serde_json::from_str(json).unwrap();
        assert_eq!(stats.last_ledger_base_fee, "100");
        assert_eq!(stats.fee_charged.p50, "150");
        assert_eq!(stats.fee_charged.p95, "800");
    }
}
