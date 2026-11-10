use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    DependOnRegistrationEmailChallengeRepository, DependOnRegistrationProtectionRepository,
    RegistrationEmailChallengeRepository, RegistrationProtectionRepository,
};
use kernel::prelude::entity::{
    ConfigGeneration, DeliveryBlock, EmailChallenge, Protection, ReadinessRecord, SendCounts,
    RATE_WINDOW,
};
use kernel::KernelError;
use sqlx::types::time::OffsetDateTime;
use sqlx::PgConnection;

use super::registration_gate::{from_column, to_column};

const READINESS_COLUMNS: &str = "protection, provider, enabled, generation, fingerprint, \
    checked_at, check_outcome, check_reason, check_transport, last_success_at, \
    success_valid_until, status, reason, valid_until, delivery_block_outcome, \
    delivery_block_reason, delivery_block_transport, delivery_block_at";

#[derive(sqlx::FromRow)]
struct ReadinessRow {
    protection: String,
    provider: String,
    enabled: bool,
    generation: String,
    fingerprint: String,
    checked_at: OffsetDateTime,
    check_outcome: String,
    check_reason: String,
    check_transport: String,
    last_success_at: Option<OffsetDateTime>,
    success_valid_until: Option<OffsetDateTime>,
    status: String,
    reason: String,
    valid_until: Option<OffsetDateTime>,
    delivery_block_outcome: Option<String>,
    delivery_block_reason: Option<String>,
    delivery_block_transport: Option<String>,
    delivery_block_at: Option<OffsetDateTime>,
}

impl TryFrom<ReadinessRow> for ReadinessRecord {
    type Error = Report<KernelError>;

    fn try_from(row: ReadinessRow) -> Result<Self, Self::Error> {
        Ok(ReadinessRecord {
            protection: from_column(&row.protection)?,
            provider: from_column(&row.provider)?,
            enabled: row.enabled,
            generation: ConfigGeneration::new(row.generation),
            fingerprint: row.fingerprint,
            checked_at: row.checked_at,
            check_outcome: from_column(&row.check_outcome)?,
            check_reason: from_column(&row.check_reason)?,
            check_transport: from_column(&row.check_transport)?,
            last_success_at: row.last_success_at,
            success_valid_until: row.success_valid_until,
            delivery_block: match (
                row.delivery_block_outcome,
                row.delivery_block_reason,
                row.delivery_block_transport,
                row.delivery_block_at,
            ) {
                (Some(outcome), Some(reason), Some(transport), Some(at)) => Some(DeliveryBlock {
                    outcome: from_column(&outcome)?,
                    reason: from_column(&reason)?,
                    transport: from_column(&transport)?,
                    at,
                }),
                (None, None, None, None) => None,
                _ => {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable("Inconsistent readiness delivery block columns"))
                }
            },
            status: from_column(&row.status)?,
            reason: from_column(&row.reason)?,
            valid_until: row.valid_until,
        })
    }
}

pub struct PostgresRegistrationProtectionRepository;

impl RegistrationProtectionRepository for PostgresRegistrationProtectionRepository {
    type Connection = PostgresConnection;

    async fn find(
        &self,
        executor: &mut Self::Connection,
        protection: Protection,
    ) -> error_stack::Result<Option<ReadinessRecord>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ReadinessRow>(&format!(
            "SELECT {READINESS_COLUMNS} FROM registration_protection_readiness WHERE protection = $1"
        ))
        .bind(to_column(&protection)?)
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(ReadinessRecord::try_from)
        .transpose()
    }

    async fn find_for_update(
        &self,
        executor: &mut Self::Connection,
        protection: Protection,
    ) -> error_stack::Result<Option<ReadinessRecord>, KernelError> {
        let con: &mut PgConnection = executor;
        let key = to_column(&protection)?;
        // 行が未作成の初回でも read-modify-write を直列化するため advisory lock を使う。
        sqlx::query(
            //language=postgresql
            "SELECT pg_advisory_xact_lock(hashtextextended('registration_protection_readiness:' || $1, 0))",
        )
        .bind(&key)
        .execute(&mut *con)
        .await
        .convert_error()?;
        sqlx::query_as::<_, ReadinessRow>(&format!(
            "SELECT {READINESS_COLUMNS} FROM registration_protection_readiness \
             WHERE protection = $1 FOR UPDATE"
        ))
        .bind(key)
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(ReadinessRecord::try_from)
        .transpose()
    }

    async fn upsert(
        &self,
        executor: &mut Self::Connection,
        record: &ReadinessRecord,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            "INSERT INTO registration_protection_readiness (protection, provider, enabled, \
             generation, fingerprint, checked_at, check_outcome, check_reason, check_transport, \
             last_success_at, success_valid_until, status, reason, valid_until, \
             delivery_block_outcome, delivery_block_reason, delivery_block_transport, \
             delivery_block_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) \
             ON CONFLICT (protection) DO UPDATE SET provider = EXCLUDED.provider, \
             enabled = EXCLUDED.enabled, generation = EXCLUDED.generation, \
             fingerprint = EXCLUDED.fingerprint, checked_at = EXCLUDED.checked_at, \
             check_outcome = EXCLUDED.check_outcome, check_reason = EXCLUDED.check_reason, \
             check_transport = EXCLUDED.check_transport, \
             last_success_at = EXCLUDED.last_success_at, \
             success_valid_until = EXCLUDED.success_valid_until, status = EXCLUDED.status, \
             reason = EXCLUDED.reason, valid_until = EXCLUDED.valid_until, \
             delivery_block_outcome = EXCLUDED.delivery_block_outcome, \
             delivery_block_reason = EXCLUDED.delivery_block_reason, \
             delivery_block_transport = EXCLUDED.delivery_block_transport, \
             delivery_block_at = EXCLUDED.delivery_block_at",
        )
        .bind(to_column(&record.protection)?)
        .bind(to_column(&record.provider)?)
        .bind(record.enabled)
        .bind(record.generation.as_str())
        .bind(&record.fingerprint)
        .bind(record.checked_at)
        .bind(to_column(&record.check_outcome)?)
        .bind(to_column(&record.check_reason)?)
        .bind(to_column(&record.check_transport)?)
        .bind(record.last_success_at)
        .bind(record.success_valid_until)
        .bind(to_column(&record.status)?)
        .bind(to_column(&record.reason)?)
        .bind(record.valid_until)
        .bind(
            record
                .delivery_block
                .map(|block| to_column(&block.outcome))
                .transpose()?,
        )
        .bind(
            record
                .delivery_block
                .map(|block| to_column(&block.reason))
                .transpose()?,
        )
        .bind(
            record
                .delivery_block
                .map(|block| to_column(&block.transport))
                .transpose()?,
        )
        .bind(record.delivery_block.map(|block| block.at))
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnRegistrationProtectionRepository for PostgresDatabase {
    type RegistrationProtectionRepository = PostgresRegistrationProtectionRepository;

    fn registration_protection_repository(&self) -> &Self::RegistrationProtectionRepository {
        &PostgresRegistrationProtectionRepository
    }
}

const CHALLENGE_COLUMNS: &str = "id, purpose, flow_id, email_hash, generation, code_hash, \
    attempts, max_attempts, status, created_at, expires_at, flow_expires_at, confirmed_at, \
    proof_hash, proof_expires_at, consumed_at";

#[derive(sqlx::FromRow)]
struct ChallengeRow {
    id: String,
    purpose: String,
    flow_id: String,
    email_hash: String,
    generation: String,
    code_hash: String,
    attempts: i32,
    max_attempts: i32,
    status: String,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
    flow_expires_at: OffsetDateTime,
    confirmed_at: Option<OffsetDateTime>,
    proof_hash: Option<String>,
    proof_expires_at: Option<OffsetDateTime>,
    consumed_at: Option<OffsetDateTime>,
}

impl TryFrom<ChallengeRow> for EmailChallenge {
    type Error = Report<KernelError>;

    fn try_from(row: ChallengeRow) -> Result<Self, Self::Error> {
        Ok(EmailChallenge {
            id: row.id,
            purpose: from_column(&row.purpose)?,
            flow_id: row.flow_id,
            email_hash: row.email_hash,
            generation: ConfigGeneration::new(row.generation),
            code_hash: row.code_hash,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            status: from_column(&row.status)?,
            created_at: row.created_at,
            expires_at: row.expires_at,
            flow_expires_at: row.flow_expires_at,
            confirmed_at: row.confirmed_at,
            proof_hash: row.proof_hash,
            proof_expires_at: row.proof_expires_at,
            consumed_at: row.consumed_at,
        })
    }
}

pub struct PostgresRegistrationEmailChallengeRepository;

/// 送信 rate limit の直列化 lock のキー (他の advisory lock と衝突しない固定値)。
const SEND_RATE_LIMIT_LOCK: &str = "registration_email_send_rate_limit";

impl RegistrationEmailChallengeRepository for PostgresRegistrationEmailChallengeRepository {
    type Connection = PostgresConnection;

    async fn lock_send_rate_limit(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        )
        .bind(SEND_RATE_LIMIT_LOCK)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn count_sends(
        &self,
        executor: &mut Self::Connection,
        email_hash: &str,
        flow_id: &str,
        source_hash: &str,
        now: OffsetDateTime,
    ) -> error_stack::Result<SendCounts, KernelError> {
        let con: &mut PgConnection = executor;
        let since = now - RATE_WINDOW;
        let (last, email_in_hour, flow_in_hour, source_in_hour): (
            Option<OffsetDateTime>,
            i64,
            i64,
            i64,
        ) = sqlx::query_as(
            //language=postgresql
            "SELECT \
               (SELECT max(sent_at) FROM registration_email_send_log \
                  WHERE email_hash = $1), \
               (SELECT count(*) FROM registration_email_send_log \
                  WHERE email_hash = $1 AND sent_at > $4), \
               (SELECT count(*) FROM registration_email_send_log \
                  WHERE flow_id = $2 AND sent_at > $4), \
               (SELECT count(*) FROM registration_email_send_log \
                  WHERE source_hash = $3 AND sent_at > $4)",
        )
        .bind(email_hash)
        .bind(flow_id)
        .bind(source_hash)
        .bind(since)
        .fetch_one(con)
        .await
        .convert_error()?;
        Ok(SendCounts {
            email_last_sent_at: last,
            email_in_hour,
            flow_in_hour,
            source_in_hour,
        })
    }

    async fn record_send(
        &self,
        executor: &mut Self::Connection,
        email_hash: &str,
        flow_id: &str,
        source_hash: &str,
        sent_at: OffsetDateTime,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            "INSERT INTO registration_email_send_log (email_hash, flow_id, source_hash, sent_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(email_hash)
        .bind(flow_id)
        .bind(source_hash)
        .bind(sent_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn insert(
        &self,
        executor: &mut Self::Connection,
        challenge: &EmailChallenge,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(&format!(
            "INSERT INTO registration_email_challenges ({CHALLENGE_COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)"
        ))
        .bind(&challenge.id)
        .bind(to_column(&challenge.purpose)?)
        .bind(&challenge.flow_id)
        .bind(&challenge.email_hash)
        .bind(challenge.generation.as_str())
        .bind(&challenge.code_hash)
        .bind(challenge.attempts)
        .bind(challenge.max_attempts)
        .bind(to_column(&challenge.status)?)
        .bind(challenge.created_at)
        .bind(challenge.expires_at)
        .bind(challenge.flow_expires_at)
        .bind(challenge.confirmed_at)
        .bind(&challenge.proof_hash)
        .bind(challenge.proof_expires_at)
        .bind(challenge.consumed_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn find_for_update(
        &self,
        executor: &mut Self::Connection,
        id: &str,
    ) -> error_stack::Result<Option<EmailChallenge>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ChallengeRow>(&format!(
            "SELECT {CHALLENGE_COLUMNS} FROM registration_email_challenges WHERE id = $1 FOR UPDATE"
        ))
        .bind(id)
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(EmailChallenge::try_from)
        .transpose()
    }

    async fn find_by_proof_hash_for_update(
        &self,
        executor: &mut Self::Connection,
        proof_hash: &str,
    ) -> error_stack::Result<Option<EmailChallenge>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ChallengeRow>(&format!(
            "SELECT {CHALLENGE_COLUMNS} FROM registration_email_challenges \
             WHERE proof_hash = $1 FOR UPDATE"
        ))
        .bind(proof_hash)
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(EmailChallenge::try_from)
        .transpose()
    }

    async fn save(
        &self,
        executor: &mut Self::Connection,
        challenge: &EmailChallenge,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            "UPDATE registration_email_challenges SET attempts = $2, status = $3, \
             confirmed_at = $4, proof_hash = $5, proof_expires_at = $6, consumed_at = $7 \
             WHERE id = $1",
        )
        .bind(&challenge.id)
        .bind(challenge.attempts)
        .bind(to_column(&challenge.status)?)
        .bind(challenge.confirmed_at)
        .bind(&challenge.proof_hash)
        .bind(challenge.proof_expires_at)
        .bind(challenge.consumed_at)
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound));
        }
        Ok(())
    }

    async fn purge_expired(
        &self,
        executor: &mut Self::Connection,
        before: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        let con: &mut PgConnection = executor;
        let challenges = sqlx::query(
            //language=postgresql
            "DELETE FROM registration_email_challenges \
             WHERE GREATEST(expires_at, COALESCE(proof_expires_at, expires_at), \
                            COALESCE(consumed_at, expires_at)) < $1",
        )
        .bind(before)
        .execute(&mut *con)
        .await
        .convert_error()?
        .rows_affected();
        let sends = sqlx::query(
            //language=postgresql
            "DELETE FROM registration_email_send_log WHERE sent_at < $1",
        )
        .bind(before)
        .execute(con)
        .await
        .convert_error()?
        .rows_affected();
        Ok(challenges + sends)
    }
}

impl DependOnRegistrationEmailChallengeRepository for PostgresDatabase {
    type RegistrationEmailChallengeRepository = PostgresRegistrationEmailChallengeRepository;

    fn registration_email_challenge_repository(
        &self,
    ) -> &Self::RegistrationEmailChallengeRepository {
        &PostgresRegistrationEmailChallengeRepository
    }
}

#[cfg(test)]
mod tests;
