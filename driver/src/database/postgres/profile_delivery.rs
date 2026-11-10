use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    DependOnProfileDeliveryEnqueuer, DependOnProfileDeliveryJobRepository, ProfileDeliveryEnqueuer,
    ProfileDeliveryJobRepository,
};
use kernel::prelude::entity::{
    ClaimedProfileDeliveryJob, DeliveryActivityType, DeliveryAttemptOutcome, DeliveryClaimPolicy,
    DeliveryFailureReason, DeliveryResultClass, EnqueueOutcome, GraphActivityId, GraphInboxUrl,
    NewProfileDeliveryJob, ProfileDeliveryAttempt, ProfileDeliveryJob, ProfileDeliveryJobId,
    ProfileDeliveryJobState, ProfileFollowId, ProfileId, RecordAttemptOutcome,
};
use kernel::KernelError;
use sqlx::{Connection, PgConnection, PgExecutor};
use time::OffsetDateTime;

const COLUMNS: &str = "id, sender_profile_id, activity_type, activity_id, target_inbox, payload, \
                       follow_id, state, attempt_count, next_attempt_at, last_attempt_at, \
                       last_result, last_http_status, failure_reason, lease_owner, lease_token, \
                       lease_expires_at, delivered_at, cancelled_at, cancel_reason, created_at, \
                       updated_at";

#[derive(sqlx::FromRow)]
struct JobRow {
    id: i64,
    sender_profile_id: i64,
    activity_type: String,
    activity_id: String,
    target_inbox: String,
    payload: String,
    follow_id: Option<i64>,
    state: String,
    attempt_count: i32,
    next_attempt_at: OffsetDateTime,
    last_attempt_at: Option<OffsetDateTime>,
    last_result: Option<String>,
    last_http_status: Option<i32>,
    failure_reason: Option<String>,
    lease_owner: Option<String>,
    lease_token: Option<String>,
    lease_expires_at: Option<OffsetDateTime>,
    delivered_at: Option<OffsetDateTime>,
    cancelled_at: Option<OffsetDateTime>,
    cancel_reason: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl TryFrom<JobRow> for ProfileDeliveryJob {
    type Error = Report<KernelError>;

    fn try_from(row: JobRow) -> Result<Self, Self::Error> {
        Ok(ProfileDeliveryJob {
            id: ProfileDeliveryJobId::new(row.id),
            sender: ProfileId::new(row.sender_profile_id),
            activity_type: DeliveryActivityType::parse(&row.activity_type)?,
            activity_id: GraphActivityId::new(row.activity_id)?,
            target_inbox: GraphInboxUrl::new(row.target_inbox)?,
            payload: row.payload,
            follow_id: row.follow_id.map(ProfileFollowId::new),
            state: ProfileDeliveryJobState::parse(&row.state)?,
            attempt_count: row.attempt_count,
            next_attempt_at: row.next_attempt_at,
            last_attempt_at: row.last_attempt_at,
            last_result: row
                .last_result
                .as_deref()
                .map(DeliveryResultClass::parse)
                .transpose()?,
            last_http_status: row.last_http_status,
            failure_reason: row
                .failure_reason
                .as_deref()
                .map(DeliveryFailureReason::parse)
                .transpose()?,
            lease_owner: row.lease_owner,
            lease_expires_at: row.lease_expires_at,
            delivered_at: row.delivered_at,
            cancelled_at: row.cancelled_at,
            cancel_reason: row.cancel_reason,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct CandidateRow {
    id: i64,
    state: String,
    attempt_count: i32,
    last_attempt_at: Option<OffsetDateTime>,
}

#[derive(sqlx::FromRow)]
struct AttemptRow {
    job_id: i64,
    attempt_no: i32,
    started_at: OffsetDateTime,
    finished_at: OffsetDateTime,
    result_class: String,
    http_status: Option<i32>,
}

async fn insert_attempt<'e>(
    executor: impl PgExecutor<'e>,
    job_id: i64,
    attempt_no: i32,
    started_at: OffsetDateTime,
    finished_at: OffsetDateTime,
    result: DeliveryResultClass,
    http_status: Option<i32>,
) -> error_stack::Result<(), KernelError> {
    sqlx::query(
        //language=postgresql
        r#"
        INSERT INTO profile_delivery_attempts (
            job_id, attempt_no, started_at, finished_at, result_class, http_status
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (job_id, attempt_no) DO NOTHING
        "#,
    )
    .bind(job_id)
    .bind(attempt_no)
    .bind(started_at)
    .bind(finished_at)
    .bind(result.as_str())
    .bind(http_status)
    .execute(executor)
    .await
    .convert_error()?;
    Ok(())
}

async fn find_job_id_by_key(
    con: &mut PgConnection,
    sender: i64,
    activity_id: &str,
    target_inbox: &str,
) -> error_stack::Result<Option<i64>, KernelError> {
    sqlx::query_scalar(
        //language=postgresql
        r#"
        SELECT id FROM profile_delivery_jobs
        WHERE sender_profile_id = $1 AND activity_id = $2 AND target_inbox = $3
        "#,
    )
    .bind(sender)
    .bind(activity_id)
    .bind(target_inbox)
    .fetch_optional(con)
    .await
    .convert_error()
}

pub struct PostgresProfileDeliveryRepository;

impl ProfileDeliveryEnqueuer for PostgresProfileDeliveryRepository {
    type Connection = PostgresConnection;

    async fn enqueue(
        &self,
        executor: &mut Self::Connection,
        job: &NewProfileDeliveryJob,
    ) -> error_stack::Result<EnqueueOutcome, KernelError> {
        let con: &mut PgConnection = executor;
        let inserted: Option<i64> = sqlx::query_scalar(
            //language=postgresql
            r#"
            INSERT INTO profile_delivery_jobs (
                id, sender_profile_id, activity_type, activity_id, target_inbox, payload,
                follow_id, state, attempt_count, next_attempt_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', 0, $8, $9, $9)
            ON CONFLICT (sender_profile_id, activity_id, target_inbox) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(job.id.as_ref())
        .bind(job.sender.as_ref())
        .bind(job.activity_type.as_str())
        .bind(job.activity_id.as_str())
        .bind(job.target_inbox.as_str())
        .bind(&job.payload)
        .bind(job.follow_id.as_ref().map(ProfileFollowId::as_ref))
        .bind(job.not_before)
        .bind(job.created_at)
        .fetch_optional(&mut *con)
        .await
        .convert_error()?;
        if let Some(id) = inserted {
            return Ok(EnqueueOutcome::Enqueued(ProfileDeliveryJobId::new(id)));
        }
        // 競合した相手が未 commit なら commit まで待つので、ここで既存の job が見える。
        let existing = find_job_id_by_key(
            con,
            *job.sender.as_ref(),
            job.activity_id.as_str(),
            job.target_inbox.as_str(),
        )
        .await?;
        existing
            .map(|id| EnqueueOutcome::AlreadyEnqueued(ProfileDeliveryJobId::new(id)))
            .ok_or_else(|| {
                Report::new(KernelError::Concurrency)
                    .attach_printable("delivery job enqueue conflicted with a concurrent change")
            })
    }
}

impl ProfileDeliveryJobRepository for PostgresProfileDeliveryRepository {
    type Connection = PostgresConnection;

    async fn claim_due(
        &self,
        executor: &mut Self::Connection,
        worker_id: &str,
        now: OffsetDateTime,
        policy: &DeliveryClaimPolicy,
    ) -> error_stack::Result<Vec<ClaimedProfileDeliveryJob>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let lease_expires_at = now + policy.lease_duration;

        let candidates = sqlx::query_as::<_, CandidateRow>(
            //language=postgresql
            r#"
            SELECT id, state, attempt_count, last_attempt_at
            FROM profile_delivery_jobs
            WHERE (state IN ('pending', 'retrying') AND next_attempt_at <= $1)
               OR (state = 'in_flight' AND lease_expires_at <= $1)
            ORDER BY next_attempt_at ASC, id ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(now)
        .bind(policy.limit)
        .fetch_all(&mut *tx)
        .await
        .convert_error()?;

        let mut claims = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let abandoned = candidate.state == "in_flight";
            if abandoned {
                insert_attempt(
                    &mut *tx,
                    candidate.id,
                    candidate.attempt_count,
                    candidate.last_attempt_at.unwrap_or(now),
                    now,
                    DeliveryResultClass::LeaseExpired,
                    None,
                )
                .await?;
                if candidate.attempt_count >= policy.max_attempts {
                    sqlx::query(
                        //language=postgresql
                        r#"
                        UPDATE profile_delivery_jobs SET
                            state = 'failed', failure_reason = 'retries_exhausted',
                            last_result = 'lease_expired', lease_owner = NULL, lease_token = NULL,
                            lease_expires_at = NULL, updated_at = $2
                        WHERE id = $1
                        "#,
                    )
                    .bind(candidate.id)
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .convert_error()?;
                    continue;
                }
            }
            let row = sqlx::query_as::<_, JobRow>(&format!(
                //language=postgresql
                r#"
                UPDATE profile_delivery_jobs SET
                    state = 'in_flight',
                    attempt_count = attempt_count + 1,
                    last_result = CASE WHEN state = 'in_flight' THEN 'lease_expired' ELSE last_result END,
                    lease_owner = $2,
                    lease_token = gen_random_uuid()::text,
                    lease_expires_at = $3,
                    last_attempt_at = $4,
                    updated_at = $4
                WHERE id = $1
                RETURNING {COLUMNS}
                "#
            ))
            .bind(candidate.id)
            .bind(worker_id)
            .bind(lease_expires_at)
            .bind(now)
            .fetch_one(&mut *tx)
            .await
            .convert_error()?;
            let attempt_no = row.attempt_count;
            let lease_token = row.lease_token.clone().ok_or_else(|| {
                Report::new(KernelError::Internal)
                    .attach_printable("claimed job has no lease token")
            })?;
            let job = ProfileDeliveryJob::try_from(row)?;
            claims.push(ClaimedProfileDeliveryJob {
                id: job.id,
                sender: job.sender,
                activity_type: job.activity_type,
                activity_id: job.activity_id,
                target_inbox: job.target_inbox,
                payload: job.payload,
                follow_id: job.follow_id,
                attempt_no,
                started_at: now,
                lease_owner: worker_id.to_owned(),
                lease_token,
                lease_expires_at,
            });
        }
        tx.commit().await.convert_error()?;
        Ok(claims)
    }

    async fn record_attempt(
        &self,
        executor: &mut Self::Connection,
        claim: &ClaimedProfileDeliveryJob,
        now: OffsetDateTime,
        outcome: &DeliveryAttemptOutcome,
    ) -> error_stack::Result<RecordAttemptOutcome, KernelError> {
        let con: &mut PgConnection = executor;
        let mut tx = con.begin().await.convert_error()?;
        let current: Option<(String, i32, Option<String>)> = sqlx::query_as(
            //language=postgresql
            r#"
            SELECT state, attempt_count, lease_token
            FROM profile_delivery_jobs
            WHERE id = $1
            FOR UPDATE
            "#,
        )
        .bind(claim.id.as_ref())
        .fetch_optional(&mut *tx)
        .await
        .convert_error()?;
        let Some((state, attempt_count, lease_token)) = current else {
            return Ok(RecordAttemptOutcome::LeaseLost);
        };
        if attempt_count != claim.attempt_no {
            return Ok(RecordAttemptOutcome::LeaseLost);
        }

        let result = if state == "in_flight" && lease_token.as_deref() == Some(&claim.lease_token) {
            insert_attempt(
                &mut *tx,
                *claim.id.as_ref(),
                claim.attempt_no,
                claim.started_at,
                now,
                outcome.result_class(),
                outcome.http_status(),
            )
            .await?;
            apply_outcome(&mut tx, claim, now, outcome).await?;
            RecordAttemptOutcome::Recorded
        } else if state == "cancelled" {
            insert_attempt(
                &mut *tx,
                *claim.id.as_ref(),
                claim.attempt_no,
                claim.started_at,
                now,
                outcome.result_class(),
                outcome.http_status(),
            )
            .await?;
            RecordAttemptOutcome::Cancelled
        } else {
            RecordAttemptOutcome::LeaseLost
        };
        tx.commit().await.convert_error()?;
        Ok(result)
    }

    async fn cancel(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileDeliveryJobId,
        reason: &str,
        now: OffsetDateTime,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE profile_delivery_jobs SET
                state = 'cancelled', cancelled_at = $3, cancel_reason = $2,
                lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = $3
            WHERE id = $1 AND state IN ('pending', 'in_flight', 'retrying')
            "#,
        )
        .bind(id.as_ref())
        .bind(reason)
        .bind(now)
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected() > 0)
    }

    async fn cancel_undelivered_for_follow(
        &self,
        executor: &mut Self::Connection,
        follow_id: &ProfileFollowId,
        activity_type: DeliveryActivityType,
        reason: &str,
        now: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE profile_delivery_jobs SET
                state = 'cancelled', cancelled_at = $4, cancel_reason = $3,
                lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = $4
            WHERE follow_id = $1 AND activity_type = $2
              AND state IN ('pending', 'in_flight', 'retrying')
            "#,
        )
        .bind(follow_id.as_ref())
        .bind(activity_type.as_str())
        .bind(reason)
        .bind(now)
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected())
    }

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileDeliveryJobId,
    ) -> error_stack::Result<Option<ProfileDeliveryJob>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {COLUMNS} FROM profile_delivery_jobs WHERE id = $1"
        ))
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(ProfileDeliveryJob::try_from)
        .transpose()
    }

    async fn find_by_key(
        &self,
        executor: &mut Self::Connection,
        sender: &ProfileId,
        activity_id: &GraphActivityId,
        target_inbox: &GraphInboxUrl,
    ) -> error_stack::Result<Option<ProfileDeliveryJob>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {COLUMNS} FROM profile_delivery_jobs \
             WHERE sender_profile_id = $1 AND activity_id = $2 AND target_inbox = $3"
        ))
        .bind(sender.as_ref())
        .bind(activity_id.as_str())
        .bind(target_inbox.as_str())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(ProfileDeliveryJob::try_from)
        .transpose()
    }

    async fn find_by_follow(
        &self,
        executor: &mut Self::Connection,
        follow_id: &ProfileFollowId,
        activity_type: DeliveryActivityType,
    ) -> error_stack::Result<Vec<ProfileDeliveryJob>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {COLUMNS} FROM profile_delivery_jobs \
             WHERE follow_id = $1 AND activity_type = $2 ORDER BY id ASC"
        ))
        .bind(follow_id.as_ref())
        .bind(activity_type.as_str())
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(ProfileDeliveryJob::try_from)
        .collect()
    }

    async fn list_attempts(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileDeliveryJobId,
    ) -> error_stack::Result<Vec<ProfileDeliveryAttempt>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AttemptRow>(
            //language=postgresql
            r#"
            SELECT job_id, attempt_no, started_at, finished_at, result_class, http_status
            FROM profile_delivery_attempts
            WHERE job_id = $1
            ORDER BY attempt_no ASC
            "#,
        )
        .bind(id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(|row| {
            Ok(ProfileDeliveryAttempt {
                job_id: ProfileDeliveryJobId::new(row.job_id),
                attempt_no: row.attempt_no,
                started_at: row.started_at,
                finished_at: row.finished_at,
                result: DeliveryResultClass::parse(&row.result_class)?,
                http_status: row.http_status,
            })
        })
        .collect()
    }
}

async fn apply_outcome(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &ClaimedProfileDeliveryJob,
    now: OffsetDateTime,
    outcome: &DeliveryAttemptOutcome,
) -> error_stack::Result<(), KernelError> {
    let query = match outcome {
        DeliveryAttemptOutcome::Delivered { .. } => sqlx::query(
            //language=postgresql
            r#"
            UPDATE profile_delivery_jobs SET
                state = 'delivered', delivered_at = $2, last_result = $3, last_http_status = $4,
                lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = $2
            WHERE id = $1
            "#,
        )
        .bind(claim.id.as_ref())
        .bind(now)
        .bind(outcome.result_class().as_str())
        .bind(outcome.http_status()),
        DeliveryAttemptOutcome::Retry {
            next_attempt_at, ..
        } => sqlx::query(
            //language=postgresql
            r#"
            UPDATE profile_delivery_jobs SET
                state = 'retrying', next_attempt_at = $5, last_result = $3, last_http_status = $4,
                lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = $2
            WHERE id = $1
            "#,
        )
        .bind(claim.id.as_ref())
        .bind(now)
        .bind(outcome.result_class().as_str())
        .bind(outcome.http_status())
        .bind(*next_attempt_at),
        DeliveryAttemptOutcome::Failed { reason, .. } => sqlx::query(
            //language=postgresql
            r#"
            UPDATE profile_delivery_jobs SET
                state = 'failed', failure_reason = $5, last_result = $3, last_http_status = $4,
                lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = $2
            WHERE id = $1
            "#,
        )
        .bind(claim.id.as_ref())
        .bind(now)
        .bind(outcome.result_class().as_str())
        .bind(outcome.http_status())
        .bind(reason.as_str()),
    };
    query.execute(&mut **tx).await.convert_error()?;
    Ok(())
}

impl DependOnProfileDeliveryEnqueuer for PostgresDatabase {
    type ProfileDeliveryEnqueuer = PostgresProfileDeliveryRepository;

    fn profile_delivery_enqueuer(&self) -> &Self::ProfileDeliveryEnqueuer {
        &PostgresProfileDeliveryRepository
    }
}

impl DependOnProfileDeliveryJobRepository for PostgresDatabase {
    type ProfileDeliveryJobRepository = PostgresProfileDeliveryRepository;

    fn profile_delivery_job_repository(&self) -> &Self::ProfileDeliveryJobRepository {
        &PostgresProfileDeliveryRepository
    }
}
