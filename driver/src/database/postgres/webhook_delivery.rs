use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    DependOnWebhookDeliveryRepository, WebhookDeliveryRepository, WebhookIngestReport,
};
use kernel::prelude::entity::{
    ClaimedWebhookDelivery, GateControlSnapshot, GateEventKind, NewWebhookDelivery,
    RegistrationGateEventId, WebhookAttempt, WebhookDecision, WebhookDelivery,
    WebhookDeliveryCounts, WebhookDeliveryPolicy, WebhookDeliveryStatus, WebhookDeliverySummary,
    WebhookFailureReason, WebhookResultClass,
};
use kernel::KernelError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::types::time::OffsetDateTime;
use sqlx::{Connection as _, PgConnection};

fn to_column<T: Serialize>(value: &T) -> error_stack::Result<String, KernelError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(value)) => Ok(value),
        Ok(other) => Err(Report::new(KernelError::Internal)
            .attach_printable(format!("Expected string column value, got {other}"))),
        Err(error) => Err(Report::from(error).change_context(KernelError::Internal)),
    }
}

fn from_column<T: DeserializeOwned>(value: &str) -> error_stack::Result<T, KernelError> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(|error| {
        Report::from(error)
            .change_context(KernelError::Internal)
            .attach_printable(format!("Unknown webhook delivery column value: {value}"))
    })
}

fn to_u16(value: Option<i32>) -> Option<u16> {
    value.and_then(|status| u16::try_from(status).ok())
}

fn to_u32(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

#[derive(sqlx::FromRow)]
struct DeliveryRow {
    event_id: i64,
    kind: String,
    status: String,
    attempt_count: i32,
    next_attempt_at: OffsetDateTime,
    last_attempt_at: Option<OffsetDateTime>,
    last_result: Option<String>,
    last_http_status: Option<i32>,
    failure_reason: Option<String>,
    lease_expires_at: Option<OffsetDateTime>,
    delivered_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
}

impl TryFrom<DeliveryRow> for WebhookDelivery {
    type Error = Report<KernelError>;

    fn try_from(row: DeliveryRow) -> Result<Self, Self::Error> {
        let status: WebhookDeliveryStatus = from_column(&row.status)?;
        let next_attempt_at = matches!(
            status,
            WebhookDeliveryStatus::Pending | WebhookDeliveryStatus::Retrying
        )
        .then_some(row.next_attempt_at);
        Ok(WebhookDelivery {
            event_id: RegistrationGateEventId::new(row.event_id),
            kind: from_column(&row.kind)?,
            status,
            attempt_count: to_u32(row.attempt_count),
            next_attempt_at,
            last_attempt_at: row.last_attempt_at,
            last_result: row.last_result.as_deref().map(from_column).transpose()?,
            last_http_status: to_u16(row.last_http_status),
            failure_reason: row.failure_reason.as_deref().map(from_column).transpose()?,
            lease_expires_at: row.lease_expires_at,
            delivered_at: row.delivered_at,
            created_at: row.created_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ClaimRow {
    event_id: i64,
    kind: String,
    payload: String,
    attempt_count: i32,
    cycle_attempt_count: i32,
    cycle_started_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct AttemptRow {
    attempt_no: i32,
    started_at: OffsetDateTime,
    finished_at: OffsetDateTime,
    result_class: String,
    http_status: Option<i32>,
}

#[derive(sqlx::FromRow)]
struct IngestRow {
    id: i64,
    kind: String,
    revision: i64,
    payload: serde_json::Value,
    created_at: OffsetDateTime,
    after_state: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct StoredEventPayload {
    actor: Option<kernel::prelude::entity::AuthAccountId>,
    observation: kernel::prelude::entity::RegistrationObservation,
}

pub struct PostgresWebhookDeliveryRepository;

async fn ensure_baseline(
    con: &mut PgConnection,
    now: OffsetDateTime,
) -> error_stack::Result<i64, KernelError> {
    sqlx::query(
        //language=postgresql
        r#"
        INSERT INTO registration_gate_webhook_state (id, baseline_event_id, enabled_since)
        VALUES (1, COALESCE((SELECT MAX(id) FROM registration_gate_events), 0), $1)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(now)
    .execute(&mut *con)
    .await
    .convert_error()?;
    sqlx::query_scalar::<_, i64>(
        //language=postgresql
        "SELECT baseline_event_id FROM registration_gate_webhook_state WHERE id = 1",
    )
    .fetch_one(&mut *con)
    .await
    .convert_error()
}

async fn insert_attempt(
    con: &mut PgConnection,
    event_id: i64,
    attempt_no: i32,
    started_at: OffsetDateTime,
    finished_at: OffsetDateTime,
    result: WebhookResultClass,
    http_status: Option<u16>,
) -> error_stack::Result<(), KernelError> {
    sqlx::query(
        //language=postgresql
        r#"
        INSERT INTO registration_gate_webhook_attempts
            (event_id, attempt_no, started_at, finished_at, result_class, http_status)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (event_id, attempt_no) DO NOTHING
        "#,
    )
    .bind(event_id)
    .bind(attempt_no)
    .bind(started_at)
    .bind(finished_at)
    .bind(to_column(&result)?)
    .bind(http_status.map(i32::from))
    .execute(con)
    .await
    .convert_error()?;
    Ok(())
}

impl WebhookDeliveryRepository for PostgresWebhookDeliveryRepository {
    type Connection = PostgresConnection;

    async fn ingest_gate_events(
        &self,
        executor: &mut Self::Connection,
        now: OffsetDateTime,
        limit: i64,
    ) -> error_stack::Result<WebhookIngestReport, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let baseline = ensure_baseline(&mut tx, now).await?;
        let rows = sqlx::query_as::<_, IngestRow>(
            //language=postgresql
            r#"
            SELECT e.id, e.kind, e.revision, e.payload, e.created_at, a.after_state
            FROM registration_gate_events e
            JOIN registration_gate_audit a ON a.revision_after = e.revision
            WHERE e.id > $1
              AND NOT EXISTS (
                  SELECT 1 FROM registration_gate_webhook_deliveries d WHERE d.event_id = e.id
              )
            ORDER BY e.id ASC
            LIMIT $2
            "#,
        )
        .bind(baseline)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .convert_error()?;

        let mut ingested = 0u64;
        for row in rows {
            let payload: StoredEventPayload = serde_json::from_value(row.payload)
                .map_err(|error| Report::from(error).change_context(KernelError::Internal))?;
            let gate: GateControlSnapshot = serde_json::from_value(row.after_state)
                .map_err(|error| Report::from(error).change_context(KernelError::Internal))?;
            let kind: GateEventKind = from_column(&row.kind)?;
            let event = kernel::prelude::entity::RegistrationGateEvent {
                id: RegistrationGateEventId::new(row.id),
                kind,
                revision: row.revision,
                actor: payload.actor,
                observation: payload.observation,
                created_at: row.created_at,
            };
            let delivery = NewWebhookDelivery::from_gate_event(&event, &gate)?;
            let result = sqlx::query(
                //language=postgresql
                r#"
                INSERT INTO registration_gate_webhook_deliveries (
                    event_id, kind, payload, status, cycle_started_at, next_attempt_at,
                    created_at, updated_at
                ) VALUES ($1, $2, $3, 'pending', $4, $4, $4, $4)
                ON CONFLICT (event_id) DO NOTHING
                "#,
            )
            .bind(delivery.event_id.as_ref())
            .bind(to_column(&delivery.kind)?)
            .bind(&delivery.payload_json)
            .bind(now)
            .execute(&mut *tx)
            .await
            .convert_error()?;
            ingested += result.rows_affected();
        }
        tx.commit().await.convert_error()?;
        Ok(WebhookIngestReport {
            baseline_event_id: baseline,
            ingested,
        })
    }

    async fn claim_due(
        &self,
        executor: &mut Self::Connection,
        owner: &str,
        now: OffsetDateTime,
        policy: &WebhookDeliveryPolicy,
    ) -> error_stack::Result<Vec<ClaimedWebhookDelivery>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let max_attempts = i32::try_from(policy.max_attempts).unwrap_or(i32::MAX);
        let age_cutoff = now - policy.max_age;
        let lease_expires_at = now + policy.lease_duration;

        // 回収対象 (lease 期限切れの in_flight) のうち、上限を超えたものは failed にして再送しない。
        let abandoned: Vec<(i64, i32, Option<OffsetDateTime>, bool)> = sqlx::query_as(
            //language=postgresql
            r#"
            SELECT event_id, attempt_count, last_attempt_at,
                   (cycle_attempt_count >= $2 OR cycle_started_at <= $3) AS exhausted
            FROM registration_gate_webhook_deliveries
            WHERE status = 'in_flight' AND lease_expires_at <= $1
            ORDER BY event_id
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(now)
        .bind(max_attempts)
        .bind(age_cutoff)
        .fetch_all(&mut *tx)
        .await
        .convert_error()?;
        for (event_id, attempt_no, started_at, exhausted) in &abandoned {
            insert_attempt(
                &mut tx,
                *event_id,
                *attempt_no,
                started_at.unwrap_or(now),
                now,
                WebhookResultClass::LeaseExpired,
                None,
            )
            .await?;
            if *exhausted {
                sqlx::query(
                    //language=postgresql
                    r#"
                    UPDATE registration_gate_webhook_deliveries SET
                        status = 'failed', failure_reason = 'retries_exhausted',
                        last_result = 'lease_expired', lease_owner = NULL,
                        lease_expires_at = NULL, updated_at = $2
                    WHERE event_id = $1
                    "#,
                )
                .bind(event_id)
                .bind(now)
                .execute(&mut *tx)
                .await
                .convert_error()?;
            } else {
                sqlx::query(
                    //language=postgresql
                    r#"
                    UPDATE registration_gate_webhook_deliveries SET
                        status = 'retrying', last_result = 'lease_expired', next_attempt_at = $2,
                        lease_owner = NULL, lease_expires_at = NULL, updated_at = $2
                    WHERE event_id = $1
                    "#,
                )
                .bind(event_id)
                .bind(now)
                .execute(&mut *tx)
                .await
                .convert_error()?;
            }
        }

        let ids: Vec<i64> = sqlx::query_scalar(
            //language=postgresql
            r#"
            SELECT event_id FROM registration_gate_webhook_deliveries
            WHERE status IN ('pending', 'retrying') AND next_attempt_at <= $1
            ORDER BY next_attempt_at ASC, event_id ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(now)
        .bind(policy.batch_size)
        .fetch_all(&mut *tx)
        .await
        .convert_error()?;

        let mut claims = Vec::with_capacity(ids.len());
        if !ids.is_empty() {
            let rows = sqlx::query_as::<_, ClaimRow>(
                //language=postgresql
                r#"
                UPDATE registration_gate_webhook_deliveries SET
                    status = 'in_flight',
                    attempt_count = attempt_count + 1,
                    cycle_attempt_count = cycle_attempt_count + 1,
                    lease_owner = $2,
                    lease_expires_at = $3,
                    last_attempt_at = $4,
                    updated_at = $4
                WHERE event_id = ANY($1)
                RETURNING event_id, kind, payload, attempt_count, cycle_attempt_count,
                          cycle_started_at
                "#,
            )
            .bind(&ids)
            .bind(owner)
            .bind(lease_expires_at)
            .bind(now)
            .fetch_all(&mut *tx)
            .await
            .convert_error()?;
            for row in rows {
                claims.push(ClaimedWebhookDelivery {
                    event_id: RegistrationGateEventId::new(row.event_id),
                    kind: from_column(&row.kind)?,
                    payload_json: row.payload,
                    attempt: to_u32(row.attempt_count),
                    cycle_attempt: to_u32(row.cycle_attempt_count),
                    cycle_started_at: row.cycle_started_at,
                    claimed_at: now,
                });
            }
            claims.sort_by_key(|claim| *claim.event_id.as_ref());
        }
        tx.commit().await.convert_error()?;
        Ok(claims)
    }

    async fn record_attempt(
        &self,
        executor: &mut Self::Connection,
        claim: &ClaimedWebhookDelivery,
        owner: &str,
        finished_at: OffsetDateTime,
        decision: &WebhookDecision,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let http_status = decision.http_status().map(i32::from);
        let result_class = to_column(&decision.class())?;
        let (status, next_attempt_at, failure_reason, delivered_at): (
            &str,
            Option<OffsetDateTime>,
            Option<String>,
            Option<OffsetDateTime>,
        ) = match decision {
            WebhookDecision::Delivered { .. } => ("delivered", None, None, Some(finished_at)),
            WebhookDecision::Retry { delay, .. } => {
                ("retrying", Some(finished_at + *delay), None, None)
            }
            WebhookDecision::Failed { reason, .. } => (
                "failed",
                None,
                Some(to_column::<WebhookFailureReason>(reason)?),
                None,
            ),
        };
        let updated = sqlx::query(
            //language=postgresql
            r#"
            UPDATE registration_gate_webhook_deliveries SET
                status = $4,
                next_attempt_at = COALESCE($5, next_attempt_at),
                failure_reason = $6,
                delivered_at = COALESCE($7, delivered_at),
                last_result = $8,
                last_http_status = $9,
                lease_owner = NULL,
                lease_expires_at = NULL,
                updated_at = $10
            WHERE event_id = $1 AND status = 'in_flight' AND lease_owner = $2
              AND attempt_count = $3
            "#,
        )
        .bind(claim.event_id.as_ref())
        .bind(owner)
        .bind(i32::try_from(claim.attempt).unwrap_or(i32::MAX))
        .bind(status)
        .bind(next_attempt_at)
        .bind(failure_reason)
        .bind(delivered_at)
        .bind(&result_class)
        .bind(http_status)
        .bind(finished_at)
        .execute(&mut *tx)
        .await
        .convert_error()?;
        if updated.rows_affected() == 0 {
            tx.rollback().await.convert_error()?;
            return Ok(false);
        }
        insert_attempt(
            &mut tx,
            *claim.event_id.as_ref(),
            i32::try_from(claim.attempt).unwrap_or(i32::MAX),
            claim.claimed_at,
            finished_at,
            decision.class(),
            decision.http_status(),
        )
        .await?;
        tx.commit().await.convert_error()?;
        Ok(true)
    }

    async fn retry_failed(
        &self,
        executor: &mut Self::Connection,
        event_id: Option<&RegistrationGateEventId>,
        now: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE registration_gate_webhook_deliveries SET
                status = 'pending',
                failure_reason = NULL,
                cycle_attempt_count = 0,
                cycle_started_at = $1,
                next_attempt_at = $1,
                updated_at = $1
            WHERE status = 'failed' AND ($2::BIGINT IS NULL OR event_id = $2)
            "#,
        )
        .bind(now)
        .bind(event_id.map(|id| *id.as_ref()))
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected())
    }

    async fn summary(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<WebhookDeliverySummary, KernelError> {
        let con: &mut PgConnection = executor;
        let counts: Vec<(String, i64)> = sqlx::query_as(
            //language=postgresql
            "SELECT status, COUNT(*) FROM registration_gate_webhook_deliveries GROUP BY status",
        )
        .fetch_all(&mut *con)
        .await
        .convert_error()?;
        let mut summary = WebhookDeliverySummary::default();
        for (status, count) in counts {
            let counts: &mut WebhookDeliveryCounts = &mut summary.counts;
            match from_column::<WebhookDeliveryStatus>(&status)? {
                WebhookDeliveryStatus::Pending => counts.pending = count,
                WebhookDeliveryStatus::InFlight => counts.in_flight = count,
                WebhookDeliveryStatus::Retrying => counts.retrying = count,
                WebhookDeliveryStatus::Delivered => counts.delivered = count,
                WebhookDeliveryStatus::Failed => counts.failed = count,
            }
        }
        let (last_success_at, last_failure_at): (Option<OffsetDateTime>, Option<OffsetDateTime>) =
            sqlx::query_as(
                //language=postgresql
                r#"
                SELECT
                    MAX(finished_at) FILTER (WHERE result_class = 'delivered'),
                    MAX(finished_at) FILTER (WHERE result_class <> 'delivered')
                FROM registration_gate_webhook_attempts
                "#,
            )
            .fetch_one(&mut *con)
            .await
            .convert_error()?;
        summary.last_success_at = last_success_at;
        summary.last_failure_at = last_failure_at;
        Ok(summary)
    }

    async fn list_recent(
        &self,
        executor: &mut Self::Connection,
        limit: i64,
    ) -> error_stack::Result<Vec<WebhookDelivery>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, DeliveryRow>(
            //language=postgresql
            r#"
            SELECT event_id, kind, status, attempt_count, next_attempt_at, last_attempt_at,
                   last_result, last_http_status, failure_reason, lease_expires_at,
                   delivered_at, created_at
            FROM registration_gate_webhook_deliveries
            ORDER BY event_id DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(WebhookDelivery::try_from)
        .collect()
    }

    async fn list_attempts(
        &self,
        executor: &mut Self::Connection,
        event_id: &RegistrationGateEventId,
    ) -> error_stack::Result<Vec<WebhookAttempt>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AttemptRow>(
            //language=postgresql
            r#"
            SELECT attempt_no, started_at, finished_at, result_class, http_status
            FROM registration_gate_webhook_attempts
            WHERE event_id = $1
            ORDER BY attempt_no ASC
            "#,
        )
        .bind(event_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(|row| {
            Ok(WebhookAttempt {
                attempt: to_u32(row.attempt_no),
                started_at: row.started_at,
                finished_at: row.finished_at,
                result: from_column(&row.result_class)?,
                http_status: to_u16(row.http_status),
            })
        })
        .collect()
    }
}

impl DependOnWebhookDeliveryRepository for PostgresDatabase {
    type WebhookDeliveryRepository = PostgresWebhookDeliveryRepository;

    fn webhook_delivery_repository(&self) -> &Self::WebhookDeliveryRepository {
        &PostgresWebhookDeliveryRepository
    }
}

#[cfg(test)]
mod tests;
