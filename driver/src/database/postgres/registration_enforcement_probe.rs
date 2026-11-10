use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{
    DependOnRegistrationEnforcementProbeRepository, EnforcementCanary, EnforcementProbeEvidence,
    EnforcementProbeId, RegistrationEnforcementProbeRepository,
};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(Clone, Default)]
pub struct PostgresRegistrationEnforcementProbeRepository;

#[derive(sqlx::FromRow)]
struct EvidenceRow {
    generation: String,
    verified: bool,
    checked_at: OffsetDateTime,
    valid_until: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct CanaryRow {
    probe_id: String,
    target_key: String,
    flow_id: String,
    created_at: OffsetDateTime,
}

impl RegistrationEnforcementProbeRepository for PostgresRegistrationEnforcementProbeRepository {
    type Connection = PostgresConnection;

    async fn begin_cycle(
        &self,
        executor: &mut Self::Connection,
        at: OffsetDateTime,
        interval: time::Duration,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let locked = sqlx::query_scalar::<_, bool>(
            "SELECT pg_try_advisory_xact_lock(hashtext('registration_enforcement_probe'))",
        )
        .fetch_one(&mut *con)
        .await
        .convert_error()?;
        if !locked {
            return Ok(false);
        }
        let reserved = sqlx::query(
            "INSERT INTO registration_enforcement_probe_schedule (id, next_probe_at) VALUES (1, $1) \
             ON CONFLICT (id) DO UPDATE SET next_probe_at = $1 \
             WHERE registration_enforcement_probe_schedule.next_probe_at <= $2",
        )
        .bind(at + interval)
        .bind(at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(reserved.rows_affected() == 1)
    }

    async fn evidence(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Option<EnforcementProbeEvidence>, KernelError> {
        let con: &mut PgConnection = executor;
        Ok(sqlx::query_as::<_, EvidenceRow>(
            "SELECT generation, verified, checked_at, valid_until \
             FROM registration_enforcement_probe_state WHERE id = 1",
        )
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(|row| EnforcementProbeEvidence {
            generation: row.generation,
            verified: row.verified,
            checked_at: row.checked_at,
            valid_until: row.valid_until,
        }))
    }

    async fn save_evidence(
        &self,
        executor: &mut Self::Connection,
        evidence: &EnforcementProbeEvidence,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            "INSERT INTO registration_enforcement_probe_state \
             (id, generation, verified, checked_at, valid_until) VALUES (1,$1,$2,$3,$4) \
             ON CONFLICT (id) DO UPDATE SET generation=$1,verified=$2,checked_at=$3,valid_until=$4",
        )
        .bind(&evidence.generation)
        .bind(evidence.verified)
        .bind(evidence.checked_at)
        .bind(evidence.valid_until)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn prepare(
        &self,
        executor: &mut Self::Connection,
        canary: &EnforcementCanary,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            "INSERT INTO registration_enforcement_canaries \
             (probe_id,target_key,flow_id,created_at) VALUES ($1,$2,$3,$4)",
        )
        .bind(canary.id.as_str())
        .bind(&canary.target_key)
        .bind(&canary.flow_id)
        .bind(canary.created_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn receive(
        &self,
        executor: &mut Self::Connection,
        id: &EnforcementProbeId,
        flow_id: &str,
        now: OffsetDateTime,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let updated = sqlx::query(
            "UPDATE registration_enforcement_canaries SET received_at=COALESCE(received_at,$3) \
             WHERE probe_id=$1 AND flow_id=$2 AND NOT settled \
             AND created_at <= $3 AND created_at > $3 - INTERVAL '2 minutes'",
        )
        .bind(id.as_str())
        .bind(flow_id)
        .bind(now)
        .execute(con)
        .await
        .convert_error()?;
        Ok(updated.rows_affected() == 1)
    }

    async fn was_received(
        &self,
        executor: &mut Self::Connection,
        id: &EnforcementProbeId,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM registration_enforcement_canaries \
             WHERE probe_id=$1 AND received_at IS NOT NULL)",
        )
        .bind(id.as_str())
        .fetch_one(con)
        .await
        .convert_error()
    }

    async fn settle(
        &self,
        executor: &mut Self::Connection,
        id: &EnforcementProbeId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query("UPDATE registration_enforcement_canaries SET settled=true WHERE probe_id=$1")
            .bind(id.as_str())
            .execute(con)
            .await
            .convert_error()?;
        Ok(())
    }

    async fn pending_cleanup(
        &self,
        executor: &mut Self::Connection,
        now: OffsetDateTime,
    ) -> error_stack::Result<Vec<EnforcementCanary>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            "DELETE FROM registration_enforcement_canaries WHERE created_at < $1 - INTERVAL '10 minutes'",
        )
        .bind(now)
        .execute(&mut *con)
        .await
        .convert_error()?;
        let rows = sqlx::query_as::<_, CanaryRow>(
            "SELECT probe_id,target_key,flow_id,created_at FROM registration_enforcement_canaries \
             WHERE NOT settled ORDER BY created_at LIMIT 513",
        )
        .fetch_all(con)
        .await
        .convert_error()?;
        rows.into_iter()
            .map(|row| {
                let id = EnforcementProbeId::parse(&row.probe_id)
                    .ok_or_else(|| error_stack::Report::new(KernelError::Internal))?;
                Ok(EnforcementCanary {
                    id,
                    target_key: row.target_key,
                    flow_id: row.flow_id,
                    created_at: row.created_at,
                })
            })
            .collect()
    }
}

impl DependOnRegistrationEnforcementProbeRepository for PostgresDatabase {
    type RegistrationEnforcementProbeRepository = PostgresRegistrationEnforcementProbeRepository;
    fn registration_enforcement_probe_repository(
        &self,
    ) -> &Self::RegistrationEnforcementProbeRepository {
        &PostgresRegistrationEnforcementProbeRepository
    }
}
