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

    /// Expose the shared HTTP client for adapters that need to make
    /// additional requests (e.g. `HorizonFeeDataProvider`).
    pub(crate) fn http_client(&self) -> &Client {
        &self.http
    }
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
    /// The arithmetic mean fee charged across all transactions in the ledger.
    /// Horizon exposes this as `"avg"` in the fee_stats response.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizon_client_base_url_is_stored() {
        let client = HorizonClient::new("https://horizon-testnet.stellar.org".into());
        assert_eq!(client.base_url(), "https://horizon-testnet.stellar.org");
    }

    #[test]
    fn fee_charged_deserialises_all_percentile_fields() {
        let json = r#"{
            "min": "100",
            "max": "5000",
            "avg": "213",
            "p10": "100",
            "p20": "100",
            "p30": "120",
            "p40": "140",
            "p50": "150",
            "p60": "200",
            "p70": "300",
            "p80": "400",
            "p90": "500",
            "p95": "800",
            "p99": "1200"
        }"#;
        let fc: FeeCharged = serde_json::from_str(json).unwrap();
        assert_eq!(fc.min, "100");
        assert_eq!(fc.max, "5000");
        assert_eq!(fc.avg, "213");
        assert_eq!(fc.p10, "100");
        assert_eq!(fc.p20, "100");
        assert_eq!(fc.p30, "120");
        assert_eq!(fc.p40, "140");
        assert_eq!(fc.p50, "150");
        assert_eq!(fc.p60, "200");
        assert_eq!(fc.p70, "300");
        assert_eq!(fc.p80, "400");
        assert_eq!(fc.p90, "500");
        assert_eq!(fc.p95, "800");
        assert_eq!(fc.p99, "1200");
    }

    #[test]
    fn horizon_fee_stats_deserialises_with_percentiles() {
        let json = r#"{
            "last_ledger_base_fee": "100",
            "fee_charged": {
                "min": "100",
                "max": "5000",
                "avg": "213",
                "p10": "100",
                "p20": "100",
                "p30": "120",
                "p40": "140",
                "p50": "150",
                "p60": "200",
                "p70": "300",
                "p80": "400",
                "p90": "500",
                "p95": "800",
                "p99": "1200"
            }
        }"#;
        let stats: HorizonFeeStats = serde_json::from_str(json).unwrap();
        assert_eq!(stats.last_ledger_base_fee, "100");
        assert_eq!(stats.fee_charged.avg, "213");
        assert_eq!(stats.fee_charged.p50, "150");
        assert_eq!(stats.fee_charged.p95, "800");
        assert_eq!(stats.fee_charged.p99, "1200");
    }
}
