//! Database repository for fee data persistence.
//!
//! All SQLite read/write logic lives here. The scheduler calls
//! [`FeeRepository::insert_fee_points`] after each poll tick and
//! [`FeeRepository::prune_older_than`] to keep the database bounded.
//!
//! On startup, [`FeeRepository::fetch_since`] rehydrates the in-memory
//! [`FeeHistoryStore`] from the last 24 hours of persisted data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::insights::types::FeeDataPoint;
use crate::services::horizon::HorizonFeeStats;

/// Valid threshold values for alert configurations.
pub const VALID_THRESHOLDS: &[&str] = &["Minor", "Major", "Critical"];

/// A single alert webhook configuration row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    pub id: i64,
    pub webhook_url: String,
    pub threshold: String,
    pub enabled: bool,
    pub created_at: String,
}

/// A single fired-alert log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub id: Option<i64>,
    pub config_id: Option<i64>,
    pub severity: String,
    pub peak_fee: i64,
    pub baseline_fee: f64,
    pub spike_ratio: f64,
    pub webhook_url: String,
    pub delivered: bool,
    pub triggered_at: String,
}

/// Repository for reading and writing fee data to SQLite.
pub struct FeeRepository {
    pool: SqlitePool,
}

impl FeeRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Bulk-insert fee data points in a single transaction.
    /// Timestamps are stored as RFC 3339 strings.
    pub async fn insert_fee_points(&self, points: &[FeeDataPoint]) -> Result<(), sqlx::Error> {
        if points.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for point in points {
            let timestamp = point.timestamp.to_rfc3339();
            let fee_amount = point.fee_amount as i64;
            let ledger_sequence = point.ledger_sequence as i64;

            sqlx::query(
                "INSERT INTO fee_data_points
                 (fee_amount, timestamp, transaction_hash, ledger_sequence)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(fee_amount)
            .bind(&timestamp)
            .bind(&point.transaction_hash)
            .bind(ledger_sequence)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Fetch all fee data points with timestamp >= `since`, ordered ascending.
    pub async fn fetch_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<FeeDataPoint>, sqlx::Error> {
        let since_str = since.to_rfc3339();

        let rows = sqlx::query(
            "SELECT fee_amount, timestamp, transaction_hash, ledger_sequence
             FROM fee_data_points
             WHERE timestamp >= ?
             ORDER BY timestamp ASC",
        )
        .bind(&since_str)
        .fetch_all(&self.pool)
        .await?;

        let points = rows
            .into_iter()
            .filter_map(|row| {
                use sqlx::Row;
                let fee_amount: i64 = row.try_get("fee_amount").ok()?;
                let timestamp_str: String = row.try_get("timestamp").ok()?;
                let transaction_hash: String = row.try_get("transaction_hash").ok()?;
                let ledger_sequence: i64 = row.try_get("ledger_sequence").ok()?;

                let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                    .ok()?
                    .with_timezone(&Utc);

                Some(FeeDataPoint {
                    fee_amount: fee_amount as u64,
                    timestamp,
                    transaction_hash,
                    ledger_sequence: ledger_sequence as u64,
                })
            })
            .collect();

        Ok(points)
    }

    /// Insert a fee snapshot (point-in-time Horizon fee_stats capture).
    pub async fn insert_snapshot(&self, snapshot: &HorizonFeeStats) -> Result<(), sqlx::Error> {
        let captured_at = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO fee_snapshots (base_fee, min_fee, max_fee, avg_fee, captured_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&snapshot.last_ledger_base_fee)
        .bind(&snapshot.fee_charged.min)
        .bind(&snapshot.fee_charged.max)
        .bind(&snapshot.fee_charged.avg)
        .bind(&captured_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Delete all fee_data_points with timestamp older than `cutoff`.
    /// Returns the number of rows deleted.
    pub async fn prune_older_than(&self, cutoff: DateTime<Utc>) -> Result<u64, sqlx::Error> {
        let cutoff_str = cutoff.to_rfc3339();

        let result = sqlx::query("DELETE FROM fee_data_points WHERE timestamp < ?")
            .bind(&cutoff_str)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    // ---- Alert config CRUD ----

    /// Insert a new alert webhook config. Returns the new row id.
    pub async fn insert_alert_config(
        &self,
        webhook_url: &str,
        threshold: &str,
    ) -> Result<i64, sqlx::Error> {
        let result =
            sqlx::query("INSERT INTO alert_configs (webhook_url, threshold) VALUES (?, ?)")
                .bind(webhook_url)
                .bind(threshold)
                .execute(&self.pool)
                .await?;

        Ok(result.last_insert_rowid())
    }

    /// List all alert configs (both enabled and disabled).
    pub async fn list_alert_configs(&self) -> Result<Vec<AlertConfig>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, webhook_url, threshold, enabled, created_at FROM alert_configs ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        let configs = rows
            .into_iter()
            .filter_map(|row| {
                use sqlx::Row;
                let id: i64 = row.try_get("id").ok()?;
                let webhook_url: String = row.try_get("webhook_url").ok()?;
                let threshold: String = row.try_get("threshold").ok()?;
                let enabled: i64 = row.try_get("enabled").ok()?;
                let created_at: String = row.try_get("created_at").ok()?;

                Some(AlertConfig {
                    id,
                    webhook_url,
                    threshold,
                    enabled: enabled != 0,
                    created_at,
                })
            })
            .collect();

        Ok(configs)
    }

    /// Update threshold and/or enabled state for an alert config.
    /// Returns `true` if a row was updated, `false` if id not found.
    pub async fn update_alert_config(
        &self,
        id: i64,
        threshold: &str,
        enabled: bool,
    ) -> Result<bool, sqlx::Error> {
        let enabled_int: i64 = if enabled { 1 } else { 0 };

        let result = sqlx::query(
            "UPDATE alert_configs SET threshold = ?, enabled = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(threshold)
        .bind(enabled_int)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Soft-delete an alert config by setting enabled = 0.
    /// Returns `true` if a row was found and updated.
    pub async fn delete_alert_config(&self, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE alert_configs SET enabled = 0, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    // ---- Alert event logging ----

    /// Log a fired alert event (success or failure).
    pub async fn log_alert_event(&self, event: &AlertEvent) -> Result<(), sqlx::Error> {
        let delivered_int: i64 = if event.delivered { 1 } else { 0 };

        sqlx::query(
            "INSERT INTO alert_events
             (config_id, severity, peak_fee, baseline_fee, spike_ratio, webhook_url, delivered, triggered_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event.config_id)
        .bind(&event.severity)
        .bind(event.peak_fee)
        .bind(event.baseline_fee)
        .bind(event.spike_ratio)
        .bind(&event.webhook_url)
        .bind(delivered_int)
        .bind(&event.triggered_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Query alert history with optional filters. `limit` is clamped to 100.
    pub async fn query_alert_history(
        &self,
        limit: i64,
        severity_filter: Option<&str>,
        delivered_filter: Option<bool>,
    ) -> Result<Vec<AlertEvent>, sqlx::Error> {
        let limit = limit.clamp(1, 100);

        // Build query dynamically based on provided filters.
        // SQLite doesn't have great support for optional binds, so we use
        // a WHERE 1=1 pattern and append conditions.
        let mut conditions = vec!["1=1"];
        let mut severity_cond = false;
        let mut delivered_cond = false;

        if severity_filter.is_some() {
            severity_cond = true;
            conditions.push("severity = ?");
        }
        if delivered_filter.is_some() {
            delivered_cond = true;
            conditions.push("delivered = ?");
        }

        let sql = format!(
            "SELECT id, config_id, severity, peak_fee, baseline_fee, spike_ratio, webhook_url, delivered, triggered_at
             FROM alert_events
             WHERE {}
             ORDER BY triggered_at DESC
             LIMIT ?",
            conditions.join(" AND ")
        );

        let _ = (severity_cond, delivered_cond); // suppress warnings

        let rows = {
            let mut q = sqlx::query(&sql);
            if let Some(sev) = severity_filter {
                q = q.bind(sev);
            }
            if let Some(del) = delivered_filter {
                q = q.bind(if del { 1i64 } else { 0i64 });
            }
            q.bind(limit).fetch_all(&self.pool).await?
        };

        let events = rows
            .into_iter()
            .filter_map(|row| {
                use sqlx::Row;
                let id: i64 = row.try_get("id").ok()?;
                let config_id: Option<i64> = row.try_get("config_id").ok()?;
                let severity: String = row.try_get("severity").ok()?;
                let peak_fee: i64 = row.try_get("peak_fee").ok()?;
                let baseline_fee: f64 = row.try_get("baseline_fee").ok()?;
                let spike_ratio: f64 = row.try_get("spike_ratio").ok()?;
                let webhook_url: String = row.try_get("webhook_url").ok()?;
                let delivered: i64 = row.try_get("delivered").ok()?;
                let triggered_at: String = row.try_get("triggered_at").ok()?;

                Some(AlertEvent {
                    id: Some(id),
                    config_id,
                    severity,
                    peak_fee,
                    baseline_fee,
                    spike_ratio,
                    webhook_url,
                    delivered: delivered != 0,
                    triggered_at,
                })
            })
            .collect();

        Ok(events)
    }

    /// Count alert events matching optional filters (for pagination totals).
    pub async fn count_alert_events(
        &self,
        severity_filter: Option<&str>,
        delivered_filter: Option<bool>,
    ) -> Result<i64, sqlx::Error> {
        let mut conditions = vec!["1=1".to_string()];

        if severity_filter.is_some() {
            conditions.push("severity = ?".to_string());
        }
        if delivered_filter.is_some() {
            conditions.push("delivered = ?".to_string());
        }

        let sql = format!(
            "SELECT COUNT(*) as cnt FROM alert_events WHERE {}",
            conditions.join(" AND ")
        );

        let row = {
            let mut q = sqlx::query(&sql);
            if let Some(sev) = severity_filter {
                q = q.bind(sev);
            }
            if let Some(del) = delivered_filter {
                q = q.bind(if del { 1i64 } else { 0i64 });
            }
            q.fetch_one(&self.pool).await?
        };

        use sqlx::Row;
        let count: i64 = row.try_get("cnt").unwrap_or(0);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    use crate::db::create_pool;
    use crate::services::horizon::FeeCharged;

    async fn make_repo() -> FeeRepository {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        FeeRepository::new(pool)
    }

    fn make_point(fee_amount: u64, seconds_ago: i64) -> FeeDataPoint {
        FeeDataPoint {
            fee_amount,
            timestamp: Utc::now() - Duration::seconds(seconds_ago),
            transaction_hash: format!("hash_{}", fee_amount),
            ledger_sequence: 1,
        }
    }

    #[tokio::test]
    async fn insert_and_fetch_roundtrip() {
        let repo = make_repo().await;
        let points = vec![
            make_point(100, 300),
            make_point(200, 200),
            make_point(300, 100),
        ];

        repo.insert_fee_points(&points).await.unwrap();

        let since = Utc::now() - Duration::seconds(400);
        let fetched = repo.fetch_since(since).await.unwrap();

        assert_eq!(fetched.len(), 3);
        assert_eq!(fetched[0].fee_amount, 100);
        assert_eq!(fetched[1].fee_amount, 200);
        assert_eq!(fetched[2].fee_amount, 300);
    }

    #[tokio::test]
    async fn fetch_since_filters_old_points() {
        let repo = make_repo().await;
        let points = vec![
            make_point(100, 7200), // 2 hours ago — outside window
            make_point(200, 1800), // 30 min ago — inside window
            make_point(300, 600),  // 10 min ago — inside window
        ];

        repo.insert_fee_points(&points).await.unwrap();

        let since = Utc::now() - Duration::hours(1);
        let fetched = repo.fetch_since(since).await.unwrap();

        assert_eq!(fetched.len(), 2);
        assert_eq!(fetched[0].fee_amount, 200);
        assert_eq!(fetched[1].fee_amount, 300);
    }

    #[tokio::test]
    async fn insert_empty_slice_is_ok() {
        let repo = make_repo().await;
        let result = repo.insert_fee_points(&[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn prune_older_than_removes_old_rows() {
        let repo = make_repo().await;
        let points = vec![
            make_point(100, 7200), // 2 hours ago — outside window, should be pruned
            make_point(200, 1800), // 30 min ago — clearly inside window, kept
            make_point(300, 600),  // 10 min ago — inside window, kept
        ];

        repo.insert_fee_points(&points).await.unwrap();

        let cutoff = Utc::now() - Duration::hours(1);
        let deleted = repo.prune_older_than(cutoff).await.unwrap();

        assert_eq!(deleted, 1);

        let remaining = repo
            .fetch_since(Utc::now() - Duration::days(1))
            .await
            .unwrap();
        assert_eq!(remaining.len(), 2);
    }

    #[tokio::test]
    async fn prune_older_than_returns_zero_when_nothing_to_prune() {
        let repo = make_repo().await;
        let points = vec![make_point(100, 60)]; // 1 min ago

        repo.insert_fee_points(&points).await.unwrap();

        let cutoff = Utc::now() - Duration::hours(1);
        let deleted = repo.prune_older_than(cutoff).await.unwrap();

        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn insert_snapshot_succeeds() {
        let repo = make_repo().await;
        let stats = HorizonFeeStats {
            last_ledger_base_fee: "100".into(),
            fee_charged: FeeCharged {
                min: "100".into(),
                max: "5000".into(),
                avg: "213".into(),
                p10: "100".into(),
                p20: "100".into(),
                p30: "120".into(),
                p40: "130".into(),
                p50: "150".into(),
                p60: "200".into(),
                p70: "250".into(),
                p80: "300".into(),
                p90: "500".into(),
                p95: "800".into(),
                p99: "1000".into(),
            },
        };

        let result = repo.insert_snapshot(&stats).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn fetch_since_returns_empty_when_no_data() {
        let repo = make_repo().await;
        let fetched = repo
            .fetch_since(Utc::now() - Duration::hours(24))
            .await
            .unwrap();
        assert!(fetched.is_empty());
    }
}
#[cfg(test)]
mod alert_tests {
    use super::*;
    use crate::db::create_pool;

    async fn make_repo() -> FeeRepository {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        FeeRepository::new(pool)
    }

    #[tokio::test]
    async fn insert_and_list_alert_config() {
        let repo = make_repo().await;
        let id = repo
            .insert_alert_config("https://hooks.example.com/webhook", "Major")
            .await
            .unwrap();
        assert!(id > 0);
        let configs = repo.list_alert_configs().await.unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].webhook_url, "https://hooks.example.com/webhook");
        assert_eq!(configs[0].threshold, "Major");
        assert!(configs[0].enabled);
    }

    #[tokio::test]
    async fn update_alert_config_changes_threshold_and_enabled() {
        let repo = make_repo().await;
        let id = repo
            .insert_alert_config("https://hooks.example.com/a", "Minor")
            .await
            .unwrap();
        let updated = repo
            .update_alert_config(id, "Critical", false)
            .await
            .unwrap();
        assert!(updated);
        let configs = repo.list_alert_configs().await.unwrap();
        assert_eq!(configs[0].threshold, "Critical");
        assert!(!configs[0].enabled);
    }

    #[tokio::test]
    async fn update_alert_config_returns_false_for_missing_id() {
        let repo = make_repo().await;
        let updated = repo.update_alert_config(9999, "Major", true).await.unwrap();
        assert!(!updated);
    }

    #[tokio::test]
    async fn delete_alert_config_soft_deletes() {
        let repo = make_repo().await;
        let id = repo
            .insert_alert_config("https://hooks.example.com/b", "Major")
            .await
            .unwrap();
        let deleted = repo.delete_alert_config(id).await.unwrap();
        assert!(deleted);
        let configs = repo.list_alert_configs().await.unwrap();
        assert_eq!(configs.len(), 1);
        assert!(!configs[0].enabled);
    }

    #[tokio::test]
    async fn delete_alert_config_returns_false_for_missing_id() {
        let repo = make_repo().await;
        let deleted = repo.delete_alert_config(9999).await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn full_crud_cycle() {
        let repo = make_repo().await;
        let id = repo
            .insert_alert_config("https://hooks.example.com/cycle", "Minor")
            .await
            .unwrap();
        let configs = repo.list_alert_configs().await.unwrap();
        assert_eq!(configs.len(), 1);
        repo.update_alert_config(id, "Major", true).await.unwrap();
        let configs = repo.list_alert_configs().await.unwrap();
        assert_eq!(configs[0].threshold, "Major");
        repo.delete_alert_config(id).await.unwrap();
        let configs = repo.list_alert_configs().await.unwrap();
        assert!(!configs[0].enabled);
    }
}

#[cfg(test)]
mod alert_event_tests {
    use super::*;
    use crate::db::create_pool;

    async fn make_repo() -> FeeRepository {
        let pool = create_pool("sqlite::memory:").await.unwrap();
        FeeRepository::new(pool)
    }

    fn make_event(severity: &str, delivered: bool) -> AlertEvent {
        AlertEvent {
            id: None,
            config_id: None,
            severity: severity.to_string(),
            peak_fee: 8000,
            baseline_fee: 130.5,
            spike_ratio: 61.3,
            webhook_url: "https://hooks.example.com/test".to_string(),
            delivered,
            triggered_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[tokio::test]
    async fn log_and_query_five_events() {
        let repo = make_repo().await;
        for _ in 0..5 {
            repo.log_alert_event(&make_event("Major", true))
                .await
                .unwrap();
        }
        let events = repo.query_alert_history(20, None, None).await.unwrap();
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn filter_by_severity() {
        let repo = make_repo().await;
        repo.log_alert_event(&make_event("Minor", true))
            .await
            .unwrap();
        repo.log_alert_event(&make_event("Major", true))
            .await
            .unwrap();
        repo.log_alert_event(&make_event("Critical", false))
            .await
            .unwrap();

        let major = repo
            .query_alert_history(20, Some("Major"), None)
            .await
            .unwrap();
        assert_eq!(major.len(), 1);
        assert_eq!(major[0].severity, "Major");

        let critical = repo
            .query_alert_history(20, Some("Critical"), None)
            .await
            .unwrap();
        assert_eq!(critical.len(), 1);
        assert_eq!(critical[0].severity, "Critical");
    }

    #[tokio::test]
    async fn filter_by_delivered() {
        let repo = make_repo().await;
        repo.log_alert_event(&make_event("Major", true))
            .await
            .unwrap();
        repo.log_alert_event(&make_event("Major", false))
            .await
            .unwrap();
        repo.log_alert_event(&make_event("Major", true))
            .await
            .unwrap();

        let delivered = repo
            .query_alert_history(20, None, Some(true))
            .await
            .unwrap();
        assert_eq!(delivered.len(), 2);

        let failed = repo
            .query_alert_history(20, None, Some(false))
            .await
            .unwrap();
        assert_eq!(failed.len(), 1);
    }

    #[tokio::test]
    async fn limit_clamped_to_100() {
        let repo = make_repo().await;
        for _ in 0..5 {
            repo.log_alert_event(&make_event("Major", true))
                .await
                .unwrap();
        }
        // Requesting 999 should be clamped to 100; still only 5 rows in DB
        let events = repo.query_alert_history(999, None, None).await.unwrap();
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn count_alert_events_total() {
        let repo = make_repo().await;
        for _ in 0..5 {
            repo.log_alert_event(&make_event("Major", true))
                .await
                .unwrap();
        }
        let total = repo.count_alert_events(None, None).await.unwrap();
        assert_eq!(total, 5);
    }

    #[tokio::test]
    async fn count_alert_events_filtered() {
        let repo = make_repo().await;
        repo.log_alert_event(&make_event("Minor", true))
            .await
            .unwrap();
        repo.log_alert_event(&make_event("Major", true))
            .await
            .unwrap();
        repo.log_alert_event(&make_event("Critical", false))
            .await
            .unwrap();

        let major_count = repo.count_alert_events(Some("Major"), None).await.unwrap();
        assert_eq!(major_count, 1);

        let delivered_count = repo.count_alert_events(None, Some(true)).await.unwrap();
        assert_eq!(delivered_count, 2);

        let critical_failed = repo
            .count_alert_events(Some("Critical"), Some(false))
            .await
            .unwrap();
        assert_eq!(critical_failed, 1);
    }

    #[tokio::test]
    async fn logged_event_has_assigned_id() {
        let repo = make_repo().await;
        repo.log_alert_event(&make_event("Major", true))
            .await
            .unwrap();
        let events = repo.query_alert_history(1, None, None).await.unwrap();
        assert!(events[0].id.is_some());
        assert!(events[0].id.unwrap() > 0);
    }
}
