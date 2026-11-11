use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    DependOnWebhookDeliveryRepository, WebhookDeliveryRepository, WebhookIngestReport,
};
use kernel::prelude::entity::{
    ClaimedWebhookDelivery, GateControlSnapshot, GateEventKind, NewWebhookDelivery,
    NewWebhookEndpoint, RegistrationGateEventId, SealedWebhookSecret, StoredWebhookEndpoint,
    WebhookAttempt, WebhookAuditEntry, WebhookCategories, WebhookCategory, WebhookDecision,
    WebhookDelivery, WebhookDeliveryCounts, WebhookDeliveryPolicy, WebhookDeliveryStatus,
    WebhookEndpointId, WebhookEndpointPatch, WebhookEndpointSource, WebhookEndpointStats,
    WebhookEndpointWrite, WebhookFailedDelivery, WebhookFailureCursor, WebhookFailureReason,
    WebhookResultClass, MAX_WEBHOOK_ENDPOINTS,
};
use kernel::KernelError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::types::time::OffsetDateTime;
use sqlx::{Connection as _, PgConnection};

/// 送信先の作成・環境変数の取り込みを直列化する advisory lock のキー (上限・重複の判定用)。
const ENDPOINT_WRITE_LOCK: i64 = 0x0E15_0001;
/// 送信先の更新前の取り込みを区切る 1 回あたりの件数 (全件取り込むまで繰り返す)。
const PRE_UPDATE_INGEST_CHUNK: i64 = 1_000;

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
    endpoint_id: i64,
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
            endpoint_id: WebhookEndpointId::new(row.endpoint_id),
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
struct FailedRow {
    endpoint_id: i64,
    event_id: i64,
    kind: String,
    failed_at: OffsetDateTime,
    last_result: Option<String>,
    last_http_status: Option<i32>,
    failure_reason: Option<String>,
    attempt_count: i32,
}

impl TryFrom<FailedRow> for WebhookFailedDelivery {
    type Error = Report<KernelError>;

    fn try_from(row: FailedRow) -> Result<Self, Self::Error> {
        Ok(WebhookFailedDelivery {
            endpoint_id: WebhookEndpointId::new(row.endpoint_id),
            event_id: RegistrationGateEventId::new(row.event_id),
            kind: from_column(&row.kind)?,
            failed_at: row.failed_at,
            last_result: row.last_result.as_deref().map(from_column).transpose()?,
            last_http_status: to_u16(row.last_http_status),
            failure_reason: row.failure_reason.as_deref().map(from_column).transpose()?,
            attempt_count: to_u32(row.attempt_count),
        })
    }
}

#[derive(sqlx::FromRow)]
struct ClaimRow {
    endpoint_id: i64,
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

#[derive(sqlx::FromRow)]
struct EndpointRow {
    id: i64,
    url: String,
    secret_key_id: String,
    secret_ciphertext: Vec<u8>,
    categories: Vec<String>,
    revision: i64,
    baseline_event_id: i64,
    source: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

const ENDPOINT_COLUMNS: &str = "id, url, secret_key_id, secret_ciphertext, categories, revision, \
     baseline_event_id, source, created_at, updated_at";

fn parse_categories(values: &[String]) -> error_stack::Result<WebhookCategories, KernelError> {
    let categories = values
        .iter()
        .map(|value| {
            WebhookCategory::parse(value).ok_or_else(|| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Unknown webhook category: {value}"))
            })
        })
        .collect::<error_stack::Result<Vec<_>, KernelError>>()?;
    WebhookCategories::new(categories).ok_or_else(|| {
        Report::new(KernelError::Internal).attach_printable("Webhook endpoint has no categories")
    })
}

fn category_columns(categories: &WebhookCategories) -> Vec<String> {
    categories
        .as_slice()
        .iter()
        .map(|category| category.as_str().to_string())
        .collect()
}

fn kind_columns(categories: &WebhookCategories) -> error_stack::Result<Vec<String>, KernelError> {
    categories.kinds().iter().map(to_column).collect()
}

impl TryFrom<EndpointRow> for StoredWebhookEndpoint {
    type Error = Report<KernelError>;

    fn try_from(row: EndpointRow) -> Result<Self, Self::Error> {
        let source = match row.source.as_str() {
            "api" => WebhookEndpointSource::Api,
            "environment_import" => WebhookEndpointSource::EnvironmentImport,
            other => {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable(format!("Unknown webhook endpoint source: {other}")))
            }
        };
        Ok(StoredWebhookEndpoint {
            id: WebhookEndpointId::new(row.id),
            url: row.url,
            sealed_secret: SealedWebhookSecret {
                key_id: row.secret_key_id,
                ciphertext: row.secret_ciphertext,
            },
            categories: parse_categories(&row.categories)?,
            revision: row.revision,
            baseline_event_id: row.baseline_event_id,
            source,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

fn source_column(source: WebhookEndpointSource) -> &'static str {
    match source {
        WebhookEndpointSource::Api => "api",
        WebhookEndpointSource::EnvironmentImport => "environment_import",
        // 環境変数のままの送信先は DB に保存しない。
        WebhookEndpointSource::Environment => "environment_import",
    }
}

pub struct PostgresWebhookDeliveryRepository;

async fn ensure_environment_baseline(
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

async fn max_event_id(con: &mut PgConnection) -> error_stack::Result<i64, KernelError> {
    sqlx::query_scalar::<_, i64>(
        //language=postgresql
        "SELECT COALESCE(MAX(id), 0) FROM registration_gate_events",
    )
    .fetch_one(con)
    .await
    .convert_error()
}

#[allow(clippy::too_many_arguments)]
async fn insert_attempt(
    con: &mut PgConnection,
    endpoint_id: i64,
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
            (endpoint_id, event_id, attempt_no, started_at, finished_at, result_class, http_status)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (endpoint_id, event_id, attempt_no) DO NOTHING
        "#,
    )
    .bind(endpoint_id)
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

/// `ingest_for_endpoint` の結果。
struct IngestOutcome {
    /// 取り込み候補として読んだ件数 (`limit` 未満なら範囲内に残りはない)。
    fetched: usize,
    /// 読んだ最後のイベント ID (次の区間の開始位置)。
    last_id: i64,
    ingested: u64,
}

/// 1 つの送信先について、`after` より後 `upto` 以下で選択カテゴリに属し未取り込みの通知イベントを
/// 古い順に最大 `limit` 件取り込む。
async fn ingest_for_endpoint(
    tx: &mut PgConnection,
    endpoint_id: i64,
    after: i64,
    upto: i64,
    kinds: &[String],
    now: OffsetDateTime,
    limit: i64,
) -> error_stack::Result<IngestOutcome, KernelError> {
    let rows = sqlx::query_as::<_, IngestRow>(
        //language=postgresql
        r#"
        SELECT e.id, e.kind, e.revision, e.payload, e.created_at, a.after_state
        FROM registration_gate_events e
        JOIN registration_gate_audit a ON a.revision_after = e.revision
        WHERE e.id > $1
          AND e.id <= $5
          AND e.kind = ANY($3)
          AND NOT EXISTS (
              SELECT 1 FROM registration_gate_webhook_deliveries d
              WHERE d.endpoint_id = $2 AND d.event_id = e.id
          )
        ORDER BY e.id ASC
        LIMIT $4
        "#,
    )
    .bind(after)
    .bind(endpoint_id)
    .bind(kinds)
    .bind(limit)
    .bind(upto)
    .fetch_all(&mut *tx)
    .await
    .convert_error()?;

    let fetched = rows.len();
    let last_id = rows.last().map_or(after, |row| row.id);
    let mut event_ids: Vec<i64> = Vec::with_capacity(fetched);
    let mut kind_values: Vec<String> = Vec::with_capacity(fetched);
    let mut payloads: Vec<String> = Vec::with_capacity(fetched);
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
        event_ids.push(*delivery.event_id.as_ref());
        kind_values.push(to_column(&delivery.kind)?);
        payloads.push(delivery.payload_json);
    }
    // 区間分を 1 回の INSERT で取り込む (大量の未取り込みイベントでも往復回数を押さえる)。
    let result = sqlx::query(
        //language=postgresql
        r#"
        INSERT INTO registration_gate_webhook_deliveries (
            endpoint_id, event_id, kind, payload, status, cycle_started_at, next_attempt_at,
            created_at, updated_at
        )
        SELECT $1::bigint, u.event_id, u.kind, u.payload, 'pending', $5::timestamptz, $5::timestamptz,
               $5::timestamptz, $5::timestamptz
        FROM UNNEST($2::bigint[], $3::text[], $4::text[]) AS u(event_id, kind, payload)
        ON CONFLICT (endpoint_id, event_id) DO NOTHING
        "#,
    )
    .bind(endpoint_id)
    .bind(&event_ids)
    .bind(&kind_values)
    .bind(&payloads)
    .bind(now)
    .execute(&mut *tx)
    .await
    .convert_error()?;
    let ingested = result.rows_affected();
    Ok(IngestOutcome {
        fetched,
        last_id,
        ingested,
    })
}

#[derive(serde::Deserialize)]
struct StoredEventPayload {
    actor: Option<kernel::prelude::entity::AuthAccountId>,
    observation: kernel::prelude::entity::RegistrationObservation,
}

async fn insert_audit(
    con: &mut PgConnection,
    entry: &WebhookAuditEntry,
    now: OffsetDateTime,
) -> error_stack::Result<(), KernelError> {
    sqlx::query(
        //language=postgresql
        r#"
        INSERT INTO registration_gate_webhook_endpoint_audit
            (id, actor, endpoint_id, operation, outcome, revision, detail, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(kernel::generate_id())
    .bind(entry.actor.as_ref().map(|actor| *actor.as_ref()))
    .bind(entry.endpoint_id.map(|id| id.as_i64()))
    .bind(to_column(&entry.operation)?)
    .bind(to_column(&entry.outcome)?)
    .bind(entry.revision)
    .bind(entry.detail.as_deref())
    .bind(now)
    .execute(con)
    .await
    .convert_error()?;
    Ok(())
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

async fn lock_endpoint(
    tx: &mut PgConnection,
    id: WebhookEndpointId,
) -> error_stack::Result<Option<StoredWebhookEndpoint>, KernelError> {
    sqlx::query_as::<_, EndpointRow>(&format!(
        //language=postgresql
        "SELECT {ENDPOINT_COLUMNS} FROM registration_gate_webhook_endpoints WHERE id = $1 FOR UPDATE"
    ))
    .bind(id.as_i64())
    .fetch_optional(tx)
    .await
    .convert_error()?
    .map(StoredWebhookEndpoint::try_from)
    .transpose()
}

/// `lock_endpoint` の非ブロッキング版 (更新・削除中の行は `None`)。削除済みも `None`。
async fn lock_endpoint_skip_locked(
    tx: &mut PgConnection,
    id: i64,
) -> error_stack::Result<Option<StoredWebhookEndpoint>, KernelError> {
    sqlx::query_as::<_, EndpointRow>(&format!(
        //language=postgresql
        "SELECT {ENDPOINT_COLUMNS} FROM registration_gate_webhook_endpoints WHERE id = $1 FOR UPDATE SKIP LOCKED"
    ))
    .bind(id)
    .fetch_optional(tx)
    .await
    .convert_error()?
    .map(StoredWebhookEndpoint::try_from)
    .transpose()
}

impl WebhookDeliveryRepository for PostgresWebhookDeliveryRepository {
    type Connection = PostgresConnection;

    async fn ingest_gate_events(
        &self,
        executor: &mut Self::Connection,
        now: OffsetDateTime,
        limit: i64,
        include_environment: bool,
    ) -> error_stack::Result<WebhookIngestReport, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let mut report = WebhookIngestReport {
            baseline_event_id: 0,
            ingested: 0,
        };
        if include_environment {
            let baseline = ensure_environment_baseline(&mut tx, now).await?;
            report.baseline_event_id = baseline;
            if limit > 0 {
                let kinds = kind_columns(&WebhookCategories::all())?;
                report.ingested += ingest_for_endpoint(
                    &mut tx,
                    WebhookEndpointId::ENVIRONMENT.as_i64(),
                    baseline,
                    i64::MAX,
                    &kinds,
                    now,
                    limit,
                )
                .await?
                .ingested;
            }
        }
        if limit > 0 {
            let endpoints = sqlx::query_as::<_, EndpointRow>(&format!(
                //language=postgresql
                "SELECT {ENDPOINT_COLUMNS} FROM registration_gate_webhook_endpoints ORDER BY id"
            ))
            .fetch_all(&mut *tx)
            .await
            .convert_error()?;
            for row in endpoints {
                // 送信先の行ロックを取り、ロック後の最新の選択・baseline で取り込む (カテゴリ更新と
                // 並行して古い選択で取り込まない)。更新中 (ロック中) の送信先は次のサイクルに回す。
                let Some(endpoint) = lock_endpoint_skip_locked(&mut tx, row.id).await? else {
                    continue;
                };
                let kinds = kind_columns(&endpoint.categories)?;
                report.ingested += ingest_for_endpoint(
                    &mut tx,
                    endpoint.id.as_i64(),
                    endpoint.baseline_event_id,
                    i64::MAX,
                    &kinds,
                    now,
                    limit,
                )
                .await?
                .ingested;
            }
        }
        tx.commit().await.convert_error()?;
        Ok(report)
    }

    async fn claim_due(
        &self,
        executor: &mut Self::Connection,
        endpoint_id: WebhookEndpointId,
        owner: &str,
        now: OffsetDateTime,
        policy: &WebhookDeliveryPolicy,
    ) -> error_stack::Result<Vec<ClaimedWebhookDelivery>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let endpoint = endpoint_id.as_i64();
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
            WHERE endpoint_id = $4 AND status = 'in_flight' AND lease_expires_at <= $1
            ORDER BY event_id
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(now)
        .bind(max_attempts)
        .bind(age_cutoff)
        .bind(endpoint)
        .fetch_all(&mut *tx)
        .await
        .convert_error()?;
        for (event_id, attempt_no, started_at, exhausted) in &abandoned {
            insert_attempt(
                &mut tx,
                endpoint,
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
                        failed_at = $3,
                        last_result = 'lease_expired', lease_owner = NULL,
                        lease_expires_at = NULL, updated_at = $3
                    WHERE endpoint_id = $1 AND event_id = $2
                    "#,
                )
                .bind(endpoint)
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
                        status = 'retrying', last_result = 'lease_expired', next_attempt_at = $3,
                        lease_owner = NULL, lease_expires_at = NULL, updated_at = $3
                    WHERE endpoint_id = $1 AND event_id = $2
                    "#,
                )
                .bind(endpoint)
                .bind(event_id)
                .bind(now)
                .execute(&mut *tx)
                .await
                .convert_error()?;
            }
        }

        // 再試行中のまま最大期間を超えた配送 (worker 停止後の再開など) は再送せず failed にする。
        // まだ一度も試行していない pending は期間に関係なく送る (再起動後に未完了配送を回収する)。
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE registration_gate_webhook_deliveries SET
                status = 'failed', failure_reason = 'retries_exhausted', failed_at = $1,
                updated_at = $1
            WHERE endpoint_id = $3 AND status = 'retrying' AND cycle_started_at <= $2
            "#,
        )
        .bind(now)
        .bind(age_cutoff)
        .bind(endpoint)
        .execute(&mut *tx)
        .await
        .convert_error()?;

        let ids: Vec<i64> = sqlx::query_scalar(
            //language=postgresql
            r#"
            SELECT event_id FROM registration_gate_webhook_deliveries
            WHERE endpoint_id = $3 AND status IN ('pending', 'retrying') AND next_attempt_at <= $1
            ORDER BY next_attempt_at ASC, event_id ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(now)
        .bind(policy.batch_size)
        .bind(endpoint)
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
                WHERE endpoint_id = $5 AND event_id = ANY($1)
                RETURNING endpoint_id, event_id, kind, payload, attempt_count,
                          cycle_attempt_count, cycle_started_at
                "#,
            )
            .bind(&ids)
            .bind(owner)
            .bind(lease_expires_at)
            .bind(now)
            .bind(endpoint)
            .fetch_all(&mut *tx)
            .await
            .convert_error()?;
            for row in rows {
                claims.push(ClaimedWebhookDelivery {
                    endpoint_id: WebhookEndpointId::new(row.endpoint_id),
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
        let (status, next_attempt_at, failure_reason, delivered_at, failed_at): (
            &str,
            Option<OffsetDateTime>,
            Option<String>,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
        ) = match decision {
            WebhookDecision::Delivered { .. } => ("delivered", None, None, Some(finished_at), None),
            WebhookDecision::Retry { delay, .. } => {
                ("retrying", Some(finished_at + *delay), None, None, None)
            }
            WebhookDecision::Failed { reason, .. } => (
                "failed",
                None,
                Some(to_column::<WebhookFailureReason>(reason)?),
                None,
                Some(finished_at),
            ),
        };
        let endpoint = claim.endpoint_id.as_i64();
        let updated = sqlx::query(
            //language=postgresql
            r#"
            UPDATE registration_gate_webhook_deliveries SET
                status = $5,
                next_attempt_at = COALESCE($6, next_attempt_at),
                failure_reason = $7,
                delivered_at = COALESCE($8, delivered_at),
                failed_at = $9,
                last_result = $10,
                last_http_status = $11,
                lease_owner = NULL,
                lease_expires_at = NULL,
                updated_at = $12
            WHERE endpoint_id = $1 AND event_id = $2 AND status = 'in_flight'
              AND lease_owner = $3 AND attempt_count = $4
            "#,
        )
        .bind(endpoint)
        .bind(claim.event_id.as_ref())
        .bind(owner)
        .bind(i32::try_from(claim.attempt).unwrap_or(i32::MAX))
        .bind(status)
        .bind(next_attempt_at)
        .bind(failure_reason)
        .bind(delivered_at)
        .bind(failed_at)
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
            endpoint,
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
        endpoint_id: Option<WebhookEndpointId>,
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
                failed_at = NULL,
                cycle_attempt_count = 0,
                cycle_started_at = $1,
                next_attempt_at = $1,
                updated_at = $1
            WHERE status = 'failed'
              AND ($2::BIGINT IS NULL OR endpoint_id = $2)
              AND ($3::BIGINT IS NULL OR event_id = $3)
            "#,
        )
        .bind(now)
        .bind(endpoint_id.map(|id| id.as_i64()))
        .bind(event_id.map(|id| *id.as_ref()))
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected())
    }

    async fn endpoint_stats(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<(WebhookEndpointId, WebhookEndpointStats)>, KernelError> {
        let con: &mut PgConnection = executor;
        let counts: Vec<(i64, String, i64)> = sqlx::query_as(
            //language=postgresql
            r#"
            SELECT endpoint_id, status, COUNT(*)
            FROM registration_gate_webhook_deliveries
            GROUP BY endpoint_id, status
            "#,
        )
        .fetch_all(&mut *con)
        .await
        .convert_error()?;
        let times: Vec<(i64, Option<OffsetDateTime>, Option<OffsetDateTime>)> = sqlx::query_as(
            //language=postgresql
            r#"
            SELECT endpoint_id,
                   MAX(finished_at) FILTER (WHERE result_class = 'delivered'),
                   MAX(finished_at) FILTER (WHERE result_class <> 'delivered')
            FROM registration_gate_webhook_attempts
            GROUP BY endpoint_id
            "#,
        )
        .fetch_all(&mut *con)
        .await
        .convert_error()?;

        let mut stats: std::collections::BTreeMap<i64, WebhookEndpointStats> =
            std::collections::BTreeMap::new();
        for (endpoint_id, status, count) in counts {
            let entry = stats.entry(endpoint_id).or_default();
            let counts: &mut WebhookDeliveryCounts = &mut entry.counts;
            match from_column::<WebhookDeliveryStatus>(&status)? {
                WebhookDeliveryStatus::Pending => counts.pending = count,
                WebhookDeliveryStatus::InFlight => counts.in_flight = count,
                WebhookDeliveryStatus::Retrying => counts.retrying = count,
                WebhookDeliveryStatus::Delivered => counts.delivered = count,
                WebhookDeliveryStatus::Failed => counts.failed = count,
            }
        }
        for (endpoint_id, last_success_at, last_failure_at) in times {
            let entry = stats.entry(endpoint_id).or_default();
            entry.last_success_at = last_success_at;
            entry.last_failure_at = last_failure_at;
        }
        Ok(stats
            .into_iter()
            .map(|(id, stats)| (WebhookEndpointId::new(id), stats))
            .collect())
    }

    async fn list_failed(
        &self,
        executor: &mut Self::Connection,
        endpoint_id: Option<WebhookEndpointId>,
        before: Option<&WebhookFailureCursor>,
        limit: i64,
    ) -> error_stack::Result<Vec<WebhookFailedDelivery>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, FailedRow>(
            //language=postgresql
            r#"
            SELECT endpoint_id, event_id, kind, failed_at, last_result, last_http_status,
                   failure_reason, attempt_count
            FROM registration_gate_webhook_deliveries
            WHERE status = 'failed'
              AND ($1::BIGINT IS NULL OR endpoint_id = $1)
              AND ($2::TIMESTAMPTZ IS NULL
                   OR (failed_at, endpoint_id, event_id) < ($2::TIMESTAMPTZ, $3::BIGINT, $4::BIGINT))
            ORDER BY failed_at DESC, endpoint_id DESC, event_id DESC
            LIMIT $5
            "#,
        )
        .bind(endpoint_id.map(|id| id.as_i64()))
        .bind(before.map(|cursor| cursor.failed_at))
        .bind(before.map(|cursor| cursor.endpoint_id))
        .bind(before.map(|cursor| cursor.event_id))
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(WebhookFailedDelivery::try_from)
        .collect()
    }

    async fn list_recent(
        &self,
        executor: &mut Self::Connection,
        endpoint_id: Option<WebhookEndpointId>,
        limit: i64,
    ) -> error_stack::Result<Vec<WebhookDelivery>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, DeliveryRow>(
            //language=postgresql
            r#"
            SELECT endpoint_id, event_id, kind, status, attempt_count, next_attempt_at,
                   last_attempt_at, last_result, last_http_status, failure_reason,
                   lease_expires_at, delivered_at, created_at
            FROM registration_gate_webhook_deliveries
            WHERE ($1::BIGINT IS NULL OR endpoint_id = $1)
            ORDER BY event_id DESC, endpoint_id DESC
            LIMIT $2
            "#,
        )
        .bind(endpoint_id.map(|id| id.as_i64()))
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
        endpoint_id: WebhookEndpointId,
        event_id: &RegistrationGateEventId,
    ) -> error_stack::Result<Vec<WebhookAttempt>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AttemptRow>(
            //language=postgresql
            r#"
            SELECT attempt_no, started_at, finished_at, result_class, http_status
            FROM registration_gate_webhook_attempts
            WHERE endpoint_id = $1 AND event_id = $2
            ORDER BY attempt_no ASC
            "#,
        )
        .bind(endpoint_id.as_i64())
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

    async fn list_endpoints(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<StoredWebhookEndpoint>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, EndpointRow>(&format!(
            //language=postgresql
            "SELECT {ENDPOINT_COLUMNS} FROM registration_gate_webhook_endpoints ORDER BY created_at, id"
        ))
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(StoredWebhookEndpoint::try_from)
        .collect()
    }

    async fn get_endpoint(
        &self,
        executor: &mut Self::Connection,
        id: WebhookEndpointId,
    ) -> error_stack::Result<Option<StoredWebhookEndpoint>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, EndpointRow>(&format!(
            //language=postgresql
            "SELECT {ENDPOINT_COLUMNS} FROM registration_gate_webhook_endpoints WHERE id = $1"
        ))
        .bind(id.as_i64())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(StoredWebhookEndpoint::try_from)
        .transpose()
    }

    async fn create_endpoint(
        &self,
        executor: &mut Self::Connection,
        new: NewWebhookEndpoint,
        actor: &kernel::prelude::entity::AuthAccountId,
        now: OffsetDateTime,
    ) -> error_stack::Result<WebhookEndpointWrite, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(ENDPOINT_WRITE_LOCK)
            .execute(&mut *tx)
            .await
            .convert_error()?;
        let count: i64 = sqlx::query_scalar(
            //language=postgresql
            "SELECT COUNT(*) FROM registration_gate_webhook_endpoints",
        )
        .fetch_one(&mut *tx)
        .await
        .convert_error()?;
        if count >= MAX_WEBHOOK_ENDPOINTS {
            tx.rollback().await.convert_error()?;
            return Ok(WebhookEndpointWrite::LimitReached);
        }
        let baseline = max_event_id(&mut tx).await?;
        let inserted = sqlx::query_as::<_, EndpointRow>(&format!(
            //language=postgresql
            r#"
            INSERT INTO registration_gate_webhook_endpoints
                (id, url, secret_key_id, secret_ciphertext, categories, revision,
                 baseline_event_id, source, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8, $8)
            ON CONFLICT (url) DO NOTHING
            RETURNING {ENDPOINT_COLUMNS}
            "#
        ))
        .bind(new.id.as_i64())
        .bind(new.url.as_str())
        .bind(&new.sealed_secret.key_id)
        .bind(&new.sealed_secret.ciphertext)
        .bind(category_columns(&new.categories))
        .bind(baseline)
        .bind(source_column(new.source))
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .convert_error()?;
        let Some(row) = inserted else {
            tx.rollback().await.convert_error()?;
            return Ok(WebhookEndpointWrite::DuplicateUrl);
        };
        let stored = StoredWebhookEndpoint::try_from(row)?;
        insert_audit(
            &mut tx,
            &WebhookAuditEntry {
                actor: Some(actor.clone()),
                endpoint_id: Some(stored.id),
                operation: kernel::prelude::entity::WebhookAuditOperation::Create,
                outcome: kernel::prelude::entity::WebhookAuditOutcome::Succeeded,
                revision: Some(stored.revision),
                detail: None,
            },
            now,
        )
        .await?;
        tx.commit().await.convert_error()?;
        Ok(WebhookEndpointWrite::Done(Box::new(stored)))
    }

    async fn update_endpoint(
        &self,
        executor: &mut Self::Connection,
        id: WebhookEndpointId,
        expected_revision: i64,
        patch: WebhookEndpointPatch,
        actor: &kernel::prelude::entity::AuthAccountId,
        now: OffsetDateTime,
    ) -> error_stack::Result<WebhookEndpointWrite, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let Some(current) = lock_endpoint(&mut tx, id).await? else {
            tx.rollback().await.convert_error()?;
            return Ok(WebhookEndpointWrite::NotFound);
        };
        if current.revision != expected_revision {
            tx.rollback().await.convert_error()?;
            return Ok(WebhookEndpointWrite::RevisionConflict);
        }
        if let Some(url) = &patch.url {
            let duplicate: bool = sqlx::query_scalar(
                //language=postgresql
                "SELECT EXISTS (SELECT 1 FROM registration_gate_webhook_endpoints WHERE url = $1 AND id <> $2)",
            )
            .bind(url.as_str())
            .bind(id.as_i64())
            .fetch_one(&mut *tx)
            .await
            .convert_error()?;
            if duplicate {
                tx.rollback().await.convert_error()?;
                return Ok(WebhookEndpointWrite::DuplicateUrl);
            }
        }
        let mut new_baseline: Option<i64> = None;
        if patch.categories.is_some() {
            // 選択の変更は変更後に発生したイベントにだけ効く: この transaction で送信先の行ロックを
            // 保持したまま境界 ID (現在の最新イベント) を決め、境界までの未取り込みイベントを
            // **変更前のカテゴリで全件** (件数に関わらず区間を繰り返して) 取り込んでから
            // baseline を境界へ進める。worker の取り込みも同じ行ロックを共有するので、
            // 境界より後のイベントが変更前の選択で取り込まれることもない。
            let boundary = max_event_id(&mut tx).await?.max(current.baseline_event_id);
            new_baseline = Some(boundary);
            let kinds = kind_columns(&current.categories)?;
            let mut cursor = current.baseline_event_id;
            loop {
                let outcome = ingest_for_endpoint(
                    &mut tx,
                    id.as_i64(),
                    cursor,
                    boundary,
                    &kinds,
                    now,
                    PRE_UPDATE_INGEST_CHUNK,
                )
                .await?;
                if i64::try_from(outcome.fetched).unwrap_or(i64::MAX) < PRE_UPDATE_INGEST_CHUNK {
                    break;
                }
                cursor = outcome.last_id;
            }
        }
        let updated = sqlx::query_as::<_, EndpointRow>(&format!(
            //language=postgresql
            r#"
            UPDATE registration_gate_webhook_endpoints SET
                url = COALESCE($2, url),
                secret_key_id = COALESCE($3, secret_key_id),
                secret_ciphertext = COALESCE($4, secret_ciphertext),
                categories = COALESCE($5, categories),
                baseline_event_id = COALESCE($7, baseline_event_id),
                revision = revision + 1,
                updated_at = $6
            WHERE id = $1
            RETURNING {ENDPOINT_COLUMNS}
            "#
        ))
        .bind(id.as_i64())
        .bind(patch.url.as_ref().map(|url| url.as_str().to_string()))
        .bind(patch.sealed_secret.as_ref().map(|s| s.key_id.clone()))
        .bind(patch.sealed_secret.as_ref().map(|s| s.ciphertext.clone()))
        .bind(patch.categories.as_ref().map(category_columns))
        .bind(now)
        .bind(new_baseline)
        .fetch_one(&mut *tx)
        .await;
        let row = match updated {
            Ok(row) => row,
            Err(error) if is_unique_violation(&error) => {
                tx.rollback().await.convert_error()?;
                return Ok(WebhookEndpointWrite::DuplicateUrl);
            }
            Err(error) => return Err(Report::from(error).change_context(KernelError::Internal)),
        };
        let stored = StoredWebhookEndpoint::try_from(row)?;
        insert_audit(
            &mut tx,
            &WebhookAuditEntry {
                actor: Some(actor.clone()),
                endpoint_id: Some(id),
                operation: kernel::prelude::entity::WebhookAuditOperation::Update,
                outcome: kernel::prelude::entity::WebhookAuditOutcome::Succeeded,
                revision: Some(stored.revision),
                detail: None,
            },
            now,
        )
        .await?;
        tx.commit().await.convert_error()?;
        Ok(WebhookEndpointWrite::Done(Box::new(stored)))
    }

    async fn delete_endpoint(
        &self,
        executor: &mut Self::Connection,
        id: WebhookEndpointId,
        expected_revision: i64,
        actor: &kernel::prelude::entity::AuthAccountId,
        now: OffsetDateTime,
    ) -> error_stack::Result<WebhookEndpointWrite, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let Some(current) = lock_endpoint(&mut tx, id).await? else {
            tx.rollback().await.convert_error()?;
            return Ok(WebhookEndpointWrite::NotFound);
        };
        if current.revision != expected_revision {
            tx.rollback().await.convert_error()?;
            return Ok(WebhookEndpointWrite::RevisionConflict);
        }
        // 配送記録 (試行履歴は連鎖削除) を消す: その送信先の未完了配送は配送されなくなる。
        sqlx::query(
            //language=postgresql
            "DELETE FROM registration_gate_webhook_deliveries WHERE endpoint_id = $1",
        )
        .bind(id.as_i64())
        .execute(&mut *tx)
        .await
        .convert_error()?;
        sqlx::query(
            //language=postgresql
            "DELETE FROM registration_gate_webhook_endpoints WHERE id = $1",
        )
        .bind(id.as_i64())
        .execute(&mut *tx)
        .await
        .convert_error()?;
        insert_audit(
            &mut tx,
            &WebhookAuditEntry {
                actor: Some(actor.clone()),
                endpoint_id: Some(id),
                operation: kernel::prelude::entity::WebhookAuditOperation::Delete,
                outcome: kernel::prelude::entity::WebhookAuditOutcome::Succeeded,
                revision: Some(current.revision),
                detail: None,
            },
            now,
        )
        .await?;
        tx.commit().await.convert_error()?;
        Ok(WebhookEndpointWrite::Done(Box::new(current)))
    }

    async fn record_audit(
        &self,
        executor: &mut Self::Connection,
        entry: &WebhookAuditEntry,
        now: OffsetDateTime,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        insert_audit(con, entry, now).await
    }

    async fn environment_import_done(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_scalar::<_, bool>(
            //language=postgresql
            "SELECT EXISTS (SELECT 1 FROM registration_gate_webhook_env_import WHERE id = 1)",
        )
        .fetch_one(con)
        .await
        .convert_error()
    }

    async fn import_environment_endpoint(
        &self,
        executor: &mut Self::Connection,
        new: NewWebhookEndpoint,
        now: OffsetDateTime,
    ) -> error_stack::Result<Option<StoredWebhookEndpoint>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(ENDPOINT_WRITE_LOCK)
            .execute(&mut *tx)
            .await
            .convert_error()?;
        let marked = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO registration_gate_webhook_env_import (id, endpoint_id, imported_at)
            VALUES (1, $1, $2)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(new.id.as_i64())
        .bind(now)
        .execute(&mut *tx)
        .await
        .convert_error()?;
        if marked.rows_affected() == 0 {
            tx.rollback().await.convert_error()?;
            return Ok(None);
        }
        // E15w の「有効化前のイベントは配送しない」baseline と、ID 0 の既存配送記録を引き継ぐ。
        let baseline = ensure_environment_baseline(&mut tx, now).await?;
        let inserted = sqlx::query_as::<_, EndpointRow>(&format!(
            //language=postgresql
            r#"
            INSERT INTO registration_gate_webhook_endpoints
                (id, url, secret_key_id, secret_ciphertext, categories, revision,
                 baseline_event_id, source, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, 1, $6, 'environment_import', $7, $7)
            ON CONFLICT (url) DO NOTHING
            RETURNING {ENDPOINT_COLUMNS}
            "#
        ))
        .bind(new.id.as_i64())
        .bind(new.url.as_str())
        .bind(&new.sealed_secret.key_id)
        .bind(&new.sealed_secret.ciphertext)
        .bind(category_columns(&new.categories))
        .bind(baseline)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .convert_error()?;
        let Some(row) = inserted else {
            // 同じ URL の送信先が既にある (理論上は未取り込みでは起きない)。取り込みは行わない。
            tx.rollback().await.convert_error()?;
            return Ok(None);
        };
        let stored = StoredWebhookEndpoint::try_from(row)?;
        // 試行履歴は外部キーの ON UPDATE CASCADE で追従する。
        sqlx::query(
            //language=postgresql
            "UPDATE registration_gate_webhook_deliveries SET endpoint_id = $1 WHERE endpoint_id = 0",
        )
        .bind(stored.id.as_i64())
        .execute(&mut *tx)
        .await
        .convert_error()?;
        insert_audit(
            &mut tx,
            &WebhookAuditEntry {
                actor: None,
                endpoint_id: Some(stored.id),
                operation: kernel::prelude::entity::WebhookAuditOperation::Import,
                outcome: kernel::prelude::entity::WebhookAuditOutcome::Succeeded,
                revision: Some(stored.revision),
                detail: None,
            },
            now,
        )
        .await?;
        tx.commit().await.convert_error()?;
        Ok(Some(stored))
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
