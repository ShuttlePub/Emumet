//! Alert evaluation of the test environment: alerts.sql (+ the read-only reservation check).
use super::db::Db;
use sqlx::Row;

pub const ALLOWED: &[&str] = &[
    "quota_global_body_count", "quota_global_body_bytes", "quota_global_delivery_rows", "quota_global_delivery_bytes",
    "quota_global_audit_rows", "quota_global_operator_rows", "quota_profile_body_count", "quota_profile_body_bytes",
    "quota_profile_audit_rows", "quota_profile_operator_rows", "quota_profile_delivery_rows", "quota_profile_delivery_bytes",
    "physical_capacity_missing_or_stale", "physical_capacity_threshold", "reservation_flag_inconsistent", "reservation_mismatch",
    "purge_delay", "cancelled_body_backlog", "deadline_near_backlog", "repeated_401", "db_lock_wait", "db_connection_pressure",
    "projector_stalled",
];

#[derive(Debug, Clone)]
pub struct Alert {
    pub name: String,
    pub severity: String,
    pub value: f64,
}

pub async fn evaluate(db: &Db) -> Vec<Alert> {
    let rows = sqlx::query(include_str!("../../../e2e/post-relay/alerts.sql")).fetch_all(&db.iso.pool).await.expect("alerts.sql");
    let mut alerts: Vec<Alert> = rows
        .iter()
        .map(|r| Alert { name: r.get("alert"), severity: r.get("severity"), value: r.get("value") })
        .collect();
    let ok: bool = sqlx::query_scalar(include_str!("../../../driver/src/database/postgres/post_relay/consistency.sql")).fetch_one(&db.iso.pool).await.expect("consistency.sql");
    if !ok {
        alerts.push(Alert { name: "reservation_mismatch".into(), severity: "critical".into(), value: 1.0 });
    }
    for a in &alerts {
        assert!(ALLOWED.contains(&a.name.as_str()), "alert name outside the fixed set: {}", a.name);
        assert!(a.severity == "warning" || a.severity == "critical");
    }
    alerts
}

pub fn firing(alerts: &[Alert], name: &str) -> Option<Alert> {
    alerts.iter().find(|a| a.name == name).cloned()
}
