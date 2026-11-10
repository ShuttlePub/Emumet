use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    DependOnRegistrationGateRepository, RegistrationGateRepository,
};
use kernel::prelude::entity::{
    AuthAccountId, GateEventKind, GateTransitionRecord, OverrideInfo, ProtectionObservation,
    RegistrationGateAudit, RegistrationGateAuditId, RegistrationGateEvent, RegistrationGateEventId,
    RegistrationGateNotification, RegistrationGateState, RegistrationObservation,
};
use kernel::KernelError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlx::types::time::OffsetDateTime;
use sqlx::PgConnection;

const STATE_COLUMNS: &str = "admin_state, override_active, override_activated_by, \
    override_activated_at, config_lost_latch, health_episode, email_configured, email_probe, \
    bot_configured, bot_probe, revision, updated_at";

/// 列の文字列値は kernel の serde (snake_case) 表現と一致させ、CHECK 制約と同じ語彙を使う。
pub(super) fn to_column<T: Serialize>(value: &T) -> error_stack::Result<String, KernelError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(value)) => Ok(value),
        Ok(other) => Err(Report::new(KernelError::Internal)
            .attach_printable(format!("Expected string column value, got {other}"))),
        Err(error) => Err(Report::from(error).change_context(KernelError::Internal)),
    }
}

pub(super) fn from_column<T: DeserializeOwned>(value: &str) -> error_stack::Result<T, KernelError> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(|error| {
        Report::from(error)
            .change_context(KernelError::Internal)
            .attach_printable(format!("Unknown registration gate column value: {value}"))
    })
}

fn to_json<T: Serialize>(value: &T) -> error_stack::Result<serde_json::Value, KernelError> {
    serde_json::to_value(value)
        .map_err(|error| Report::from(error).change_context(KernelError::Internal))
}

#[derive(sqlx::FromRow)]
struct StateRow {
    admin_state: String,
    override_active: bool,
    override_activated_by: Option<i64>,
    override_activated_at: Option<OffsetDateTime>,
    config_lost_latch: bool,
    health_episode: String,
    email_configured: bool,
    email_probe: String,
    bot_configured: bool,
    bot_probe: String,
    revision: i64,
    updated_at: OffsetDateTime,
}

impl TryFrom<StateRow> for RegistrationGateState {
    type Error = Report<KernelError>;

    fn try_from(row: StateRow) -> Result<Self, Self::Error> {
        let override_info = match (
            row.override_active,
            row.override_activated_by,
            row.override_activated_at,
        ) {
            (true, Some(by), Some(at)) => Some(OverrideInfo {
                activated_by: AuthAccountId::new(by),
                activated_at: at,
            }),
            (false, None, None) => None,
            _ => {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable("Inconsistent registration gate override columns"))
            }
        };
        Ok(RegistrationGateState {
            admin: from_column(&row.admin_state)?,
            override_info,
            config_lost_latch: row.config_lost_latch,
            episode: from_column(&row.health_episode)?,
            observation: RegistrationObservation::new(
                ProtectionObservation::new(row.email_configured, from_column(&row.email_probe)?),
                ProtectionObservation::new(row.bot_configured, from_column(&row.bot_probe)?),
            ),
            revision: row.revision,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct EventPayload {
    actor: Option<AuthAccountId>,
    observation: RegistrationObservation,
}

#[derive(sqlx::FromRow)]
struct EventRow {
    id: i64,
    kind: String,
    revision: i64,
    payload: serde_json::Value,
    created_at: OffsetDateTime,
    read_at: Option<OffsetDateTime>,
}

impl TryFrom<EventRow> for RegistrationGateNotification {
    type Error = Report<KernelError>;

    fn try_from(row: EventRow) -> Result<Self, Self::Error> {
        let payload: EventPayload = serde_json::from_value(row.payload)
            .map_err(|error| Report::from(error).change_context(KernelError::Internal))?;
        let kind: GateEventKind = from_column(&row.kind)?;
        Ok(RegistrationGateNotification {
            event: RegistrationGateEvent {
                id: RegistrationGateEventId::new(row.id),
                kind,
                revision: row.revision,
                actor: payload.actor,
                observation: payload.observation,
                created_at: row.created_at,
            },
            read_at: row.read_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct AuditRow {
    id: i64,
    action: String,
    actor_auth_account_id: Option<i64>,
    warning_acknowledged: bool,
    before_state: serde_json::Value,
    after_state: serde_json::Value,
    observation: serde_json::Value,
    created_at: OffsetDateTime,
}

fn from_json<T: DeserializeOwned>(value: serde_json::Value) -> error_stack::Result<T, KernelError> {
    serde_json::from_value(value)
        .map_err(|error| Report::from(error).change_context(KernelError::Internal))
}

impl TryFrom<AuditRow> for RegistrationGateAudit {
    type Error = Report<KernelError>;

    fn try_from(row: AuditRow) -> Result<Self, Self::Error> {
        Ok(RegistrationGateAudit {
            id: RegistrationGateAuditId::new(row.id),
            action: from_column(&row.action)?,
            actor: row.actor_auth_account_id.map(AuthAccountId::new),
            warning_acknowledged: row.warning_acknowledged,
            before: from_json(row.before_state)?,
            after: from_json(row.after_state)?,
            observation: from_json(row.observation)?,
            created_at: row.created_at,
        })
    }
}

pub struct PostgresRegistrationGateRepository;

async fn fetch_state(
    con: &mut PgConnection,
    for_update: bool,
) -> error_stack::Result<RegistrationGateState, KernelError> {
    let sql = format!(
        "SELECT {STATE_COLUMNS} FROM registration_gate_state WHERE id = 1{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    if let Some(row) = sqlx::query_as::<_, StateRow>(&sql)
        .fetch_optional(&mut *con)
        .await
        .convert_error()?
    {
        return row.try_into();
    }
    sqlx::query(
        //language=postgresql
        "INSERT INTO registration_gate_state (id) VALUES (1) ON CONFLICT (id) DO NOTHING",
    )
    .execute(&mut *con)
    .await
    .convert_error()?;
    sqlx::query_as::<_, StateRow>(&sql)
        .fetch_one(&mut *con)
        .await
        .convert_error()?
        .try_into()
}

impl RegistrationGateRepository for PostgresRegistrationGateRepository {
    type Connection = PostgresConnection;

    async fn load(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<RegistrationGateState, KernelError> {
        let con: &mut PgConnection = executor;
        fetch_state(con, false).await
    }

    async fn load_for_update(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<RegistrationGateState, KernelError> {
        let con: &mut PgConnection = executor;
        fetch_state(con, true).await
    }

    async fn save_transition(
        &self,
        executor: &mut Self::Connection,
        record: &GateTransitionRecord,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let state = &record.state;
        let updated = sqlx::query(
            //language=postgresql
            r#"
            UPDATE registration_gate_state SET
                admin_state = $1,
                override_active = $2,
                override_activated_by = $3,
                override_activated_at = $4,
                config_lost_latch = $5,
                health_episode = $6,
                email_configured = $7,
                email_probe = $8,
                bot_configured = $9,
                bot_probe = $10,
                revision = $11,
                updated_at = $12
            WHERE id = 1 AND revision = $13
            "#,
        )
        .bind(to_column(&state.admin)?)
        .bind(state.override_info.is_some())
        .bind(
            state
                .override_info
                .as_ref()
                .map(|o| *o.activated_by.as_ref()),
        )
        .bind(state.override_info.as_ref().map(|o| o.activated_at))
        .bind(state.config_lost_latch)
        .bind(to_column(&state.episode)?)
        .bind(state.observation.email.configured)
        .bind(to_column(&state.observation.email.probe)?)
        .bind(state.observation.bot.configured)
        .bind(to_column(&state.observation.bot.probe)?)
        .bind(state.revision)
        .bind(state.updated_at)
        .bind(record.expected_revision)
        .execute(&mut *con)
        .await
        .convert_error()?;
        if updated.rows_affected() == 0 {
            return Err(Report::new(KernelError::Concurrency)
                .attach_printable("Registration gate revision changed concurrently"));
        }

        let audit = &record.audit;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO registration_gate_audit (
                id, revision_before, revision_after, action, actor_auth_account_id,
                warning_acknowledged, before_state, after_state, observation, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(audit.id.as_ref())
        .bind(audit.before.revision)
        .bind(audit.after.revision)
        .bind(to_column(&audit.action)?)
        .bind(audit.actor.as_ref().map(|actor| *actor.as_ref()))
        .bind(audit.warning_acknowledged)
        .bind(to_json(&audit.before)?)
        .bind(to_json(&audit.after)?)
        .bind(to_json(&audit.observation)?)
        .bind(audit.created_at)
        .execute(&mut *con)
        .await
        .convert_error()?;

        for event in &record.events {
            let payload = to_json(&EventPayload {
                actor: event.actor.clone(),
                observation: event.observation,
            })?;
            sqlx::query(
                //language=postgresql
                r#"
                INSERT INTO registration_gate_events (id, kind, revision, payload, created_at)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(event.id.as_ref())
            .bind(to_column(&event.kind)?)
            .bind(event.revision)
            .bind(payload)
            .bind(event.created_at)
            .execute(&mut *con)
            .await
            .convert_error()?;
        }
        Ok(())
    }

    async fn save_observation(
        &self,
        executor: &mut Self::Connection,
        observation: &RegistrationObservation,
        updated_at: OffsetDateTime,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE registration_gate_state SET
                email_configured = $1,
                email_probe = $2,
                bot_configured = $3,
                bot_probe = $4,
                updated_at = $5
            WHERE id = 1
            "#,
        )
        .bind(observation.email.configured)
        .bind(to_column(&observation.email.probe)?)
        .bind(observation.bot.configured)
        .bind(to_column(&observation.bot.probe)?)
        .bind(updated_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn list_audit(
        &self,
        executor: &mut Self::Connection,
        limit: i64,
    ) -> error_stack::Result<Vec<RegistrationGateAudit>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AuditRow>(
            //language=postgresql
            r#"
            SELECT id, action, actor_auth_account_id, warning_acknowledged,
                   before_state, after_state, observation, created_at
            FROM registration_gate_audit
            ORDER BY revision_after DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(RegistrationGateAudit::try_from)
        .collect()
    }

    async fn list_notifications(
        &self,
        executor: &mut Self::Connection,
        admin: &AuthAccountId,
        before: Option<&RegistrationGateEventId>,
        limit: i64,
    ) -> error_stack::Result<Vec<RegistrationGateNotification>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, EventRow>(
            //language=postgresql
            r#"
            SELECT e.id, e.kind, e.revision, e.payload, e.created_at, r.read_at
            FROM registration_gate_events e
            LEFT JOIN registration_gate_event_reads r
              ON r.event_id = e.id AND r.admin_auth_account_id = $1
            WHERE ($2::BIGINT IS NULL OR e.id < $2)
            ORDER BY e.id DESC
            LIMIT $3
            "#,
        )
        .bind(admin.as_ref())
        .bind(before.map(|id| *id.as_ref()))
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(RegistrationGateNotification::try_from)
        .collect()
    }

    async fn count_unread(
        &self,
        executor: &mut Self::Connection,
        admin: &AuthAccountId,
    ) -> error_stack::Result<i64, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_scalar::<_, i64>(
            //language=postgresql
            r#"
            SELECT COUNT(*) FROM registration_gate_events e
            WHERE NOT EXISTS (
                SELECT 1 FROM registration_gate_event_reads r
                WHERE r.event_id = e.id AND r.admin_auth_account_id = $1
            )
            "#,
        )
        .bind(admin.as_ref())
        .fetch_one(con)
        .await
        .convert_error()
    }

    async fn mark_read(
        &self,
        executor: &mut Self::Connection,
        admin: &AuthAccountId,
        event_ids: &[RegistrationGateEventId],
        read_at: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        let con: &mut PgConnection = executor;
        let ids: Vec<i64> = event_ids.iter().map(|id| *id.as_ref()).collect();
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO registration_gate_event_reads (admin_auth_account_id, event_id, read_at)
            SELECT $1, e.id, $3 FROM registration_gate_events e WHERE e.id = ANY($2)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(admin.as_ref())
        .bind(&ids)
        .bind(read_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected())
    }

    async fn mark_all_read(
        &self,
        executor: &mut Self::Connection,
        admin: &AuthAccountId,
        read_at: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO registration_gate_event_reads (admin_auth_account_id, event_id, read_at)
            SELECT $1, e.id, $2 FROM registration_gate_events e
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(admin.as_ref())
        .bind(read_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected())
    }
}

impl DependOnRegistrationGateRepository for PostgresDatabase {
    type RegistrationGateRepository = PostgresRegistrationGateRepository;

    fn registration_gate_repository(&self) -> &Self::RegistrationGateRepository {
        &PostgresRegistrationGateRepository
    }
}

#[cfg(test)]
mod tests;
