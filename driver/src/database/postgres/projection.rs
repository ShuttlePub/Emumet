use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::projection::{
    AccountProjectionWriter, DependOnAccountEventLog, DependOnAccountProjectionWriter,
    DependOnMetadataEventLog, DependOnMetadataProjectionWriter, DependOnProfileEventLog,
    DependOnProfileProjectionWriter, DependOnProjectionCheckpointStore, MetadataProjectionWriter,
    ProfileProjectionWriter, ProjectionCheckpointStore, SeqEvent,
};
use kernel::prelude::entity::{
    Account, AccountEvent, AccountStatus, EventEnvelope, EventId, EventVersion, ImageId, Metadata,
    MetadataEvent, MetadataId, Profile, ProfileDisplayName, ProfileEvent, ProfileId,
    ProfileSummary,
};
use kernel::KernelError;
use serde_json;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct SeqEventRow {
    seq: i64,
    version: i64,
    id: i64,
    #[allow(dead_code)]
    event_name: String,
    data: serde_json::Value,
}

impl TryFrom<SeqEventRow> for SeqEvent<AccountEvent, Account> {
    type Error = Report<KernelError>;
    fn try_from(value: SeqEventRow) -> Result<Self, Self::Error> {
        let event: AccountEvent = serde_json::from_value(value.data).convert_error()?;
        Ok(SeqEvent {
            seq: value.seq,
            envelope: EventEnvelope::new(
                EventId::new(value.id),
                event,
                EventVersion::new(value.version),
            ),
        })
    }
}

pub struct PostgresAccountEventLog;

impl kernel::interfaces::projection::AccountEventLog for PostgresAccountEventLog {
    type Connection = PostgresConnection;

    async fn find_by_seq_window(
        &self,
        executor: &mut Self::Connection,
        from_seq_exclusive: i64,
        limit: i64,
    ) -> error_stack::Result<Vec<SeqEvent<AccountEvent, Account>>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = sqlx::query_as::<_, SeqEventRow>(
            //language=postgresql
            r#"
            SELECT seq, version, id, event_name, data
            FROM account_events
            WHERE seq > $1
            ORDER BY seq
            LIMIT $2
            "#,
        )
        .bind(from_seq_exclusive)
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()?;
        rows.into_iter()
            .map(TryFrom::try_from)
            .collect::<error_stack::Result<Vec<_>, KernelError>>()
    }
}

pub struct PostgresProjectionCheckpointStore;

impl ProjectionCheckpointStore for PostgresProjectionCheckpointStore {
    type Connection = PostgresConnection;

    async fn get(
        &self,
        executor: &mut Self::Connection,
        projector_name: &str,
    ) -> error_stack::Result<Option<i64>, KernelError> {
        let con: &mut PgConnection = executor;
        let row: Option<(i64,)> = sqlx::query_as(
            //language=postgresql
            r#"
            SELECT last_seq
            FROM projection_checkpoints
            WHERE projector_name = $1
            "#,
        )
        .bind(projector_name)
        .fetch_optional(con)
        .await
        .convert_error()?;
        Ok(row.map(|row| row.0))
    }

    async fn set(
        &self,
        executor: &mut Self::Connection,
        projector_name: &str,
        seq: i64,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO projection_checkpoints (projector_name, last_seq, updated_at)
            VALUES ($1, $2, now())
            ON CONFLICT (projector_name) DO UPDATE SET
                last_seq = GREATEST(projection_checkpoints.last_seq, EXCLUDED.last_seq),
                updated_at = now()
            "#,
        )
        .bind(projector_name)
        .bind(seq)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

pub struct PostgresAccountProjectionWriter;

impl AccountProjectionWriter for PostgresAccountProjectionWriter {
    type Connection = PostgresConnection;

    async fn upsert(
        &self,
        executor: &mut Self::Connection,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let (suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason) =
            match account.status() {
                AccountStatus::Active => (None, None, None, None, None),
                AccountStatus::Suspended {
                    reason,
                    suspended_at,
                    expires_at,
                } => (
                    Some(*suspended_at),
                    *expires_at,
                    Some(reason.clone()),
                    None,
                    None,
                ),
                AccountStatus::Banned { reason, banned_at } => {
                    (None, None, None, Some(*banned_at), Some(reason.clone()))
                }
            };
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO accounts (id, name, is_bot, version, nanoid, created_at,
                                  suspended_at, suspend_expires_at, suspend_reason,
                                  banned_at, ban_reason, deleted_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (id) DO UPDATE SET
                name = EXCLUDED.name,
                is_bot = EXCLUDED.is_bot,
                version = EXCLUDED.version,
                nanoid = EXCLUDED.nanoid,
                created_at = EXCLUDED.created_at,
                suspended_at = EXCLUDED.suspended_at,
                suspend_expires_at = EXCLUDED.suspend_expires_at,
                suspend_reason = EXCLUDED.suspend_reason,
                banned_at = EXCLUDED.banned_at,
                ban_reason = EXCLUDED.ban_reason,
                deleted_at = EXCLUDED.deleted_at
            WHERE accounts.version < EXCLUDED.version
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.name().as_ref())
        .bind(account.is_bot().as_ref())
        .bind(account.version().as_ref())
        .bind(account.nanoid().as_ref())
        .bind(account.created_at().as_ref())
        .bind(suspended_at)
        .bind(suspend_expires_at)
        .bind(suspend_reason)
        .bind(banned_at)
        .bind(ban_reason)
        .bind(account.deleted_at().as_ref().map(|d| d.as_ref()))
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnAccountEventLog for PostgresDatabase {
    type AccountEventLog = PostgresAccountEventLog;

    fn account_event_log(&self) -> &Self::AccountEventLog {
        &PostgresAccountEventLog
    }
}

impl DependOnProjectionCheckpointStore for PostgresDatabase {
    type ProjectionCheckpointStore = PostgresProjectionCheckpointStore;

    fn projection_checkpoint_store(&self) -> &Self::ProjectionCheckpointStore {
        &PostgresProjectionCheckpointStore
    }
}

impl DependOnAccountProjectionWriter for PostgresDatabase {
    type AccountProjectionWriter = PostgresAccountProjectionWriter;

    fn account_projection_writer(&self) -> &Self::AccountProjectionWriter {
        &PostgresAccountProjectionWriter
    }
}

// --- Profile projection ---

impl TryFrom<SeqEventRow> for SeqEvent<ProfileEvent, Profile> {
    type Error = Report<KernelError>;
    fn try_from(value: SeqEventRow) -> Result<Self, Self::Error> {
        let event: ProfileEvent = serde_json::from_value(value.data).convert_error()?;
        Ok(SeqEvent {
            seq: value.seq,
            envelope: EventEnvelope::new(
                EventId::new(value.id),
                event,
                EventVersion::new(value.version),
            ),
        })
    }
}

pub struct PostgresProfileEventLog;

impl kernel::interfaces::projection::ProfileEventLog for PostgresProfileEventLog {
    type Connection = PostgresConnection;

    async fn find_by_seq_window(
        &self,
        executor: &mut Self::Connection,
        from_seq_exclusive: i64,
        limit: i64,
    ) -> error_stack::Result<Vec<SeqEvent<ProfileEvent, Profile>>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = sqlx::query_as::<_, SeqEventRow>(
            //language=postgresql
            r#"
            SELECT seq, version, id, event_name, data
            FROM profile_events
            WHERE seq > $1
            ORDER BY seq
            LIMIT $2
            "#,
        )
        .bind(from_seq_exclusive)
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()?;
        rows.into_iter()
            .map(TryFrom::try_from)
            .collect::<error_stack::Result<Vec<_>, KernelError>>()
    }
}

pub struct PostgresProfileProjectionWriter;

impl ProfileProjectionWriter for PostgresProfileProjectionWriter {
    type Connection = PostgresConnection;

    async fn upsert(
        &self,
        executor: &mut Self::Connection,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profiles (id, account_id, display, summary, icon_id, banner_id, version, nanoid)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (id) DO UPDATE SET
                account_id = EXCLUDED.account_id,
                display = EXCLUDED.display,
                summary = EXCLUDED.summary,
                icon_id = EXCLUDED.icon_id,
                banner_id = EXCLUDED.banner_id,
                version = EXCLUDED.version,
                nanoid = EXCLUDED.nanoid
            WHERE profiles.version < EXCLUDED.version
            "#,
        )
        .bind(profile.id().as_ref())
        .bind(profile.account_id().as_ref())
        .bind(profile.display_name().as_ref().map(ProfileDisplayName::as_ref))
        .bind(profile.summary().as_ref().map(ProfileSummary::as_ref))
        .bind(profile.icon().as_ref().map(ImageId::as_ref))
        .bind(profile.banner().as_ref().map(ImageId::as_ref))
        .bind(profile.version().as_ref())
        .bind(profile.nanoid().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"DELETE FROM profiles WHERE id = $1"#,
        )
        .bind(profile_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

// --- Metadata projection ---

impl TryFrom<SeqEventRow> for SeqEvent<MetadataEvent, Metadata> {
    type Error = Report<KernelError>;
    fn try_from(value: SeqEventRow) -> Result<Self, Self::Error> {
        let event: MetadataEvent = serde_json::from_value(value.data).convert_error()?;
        Ok(SeqEvent {
            seq: value.seq,
            envelope: EventEnvelope::new(
                EventId::new(value.id),
                event,
                EventVersion::new(value.version),
            ),
        })
    }
}

pub struct PostgresMetadataEventLog;

impl kernel::interfaces::projection::MetadataEventLog for PostgresMetadataEventLog {
    type Connection = PostgresConnection;

    async fn find_by_seq_window(
        &self,
        executor: &mut Self::Connection,
        from_seq_exclusive: i64,
        limit: i64,
    ) -> error_stack::Result<Vec<SeqEvent<MetadataEvent, Metadata>>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = sqlx::query_as::<_, SeqEventRow>(
            //language=postgresql
            r#"
            SELECT seq, version, id, event_name, data
            FROM metadata_events
            WHERE seq > $1
            ORDER BY seq
            LIMIT $2
            "#,
        )
        .bind(from_seq_exclusive)
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()?;
        rows.into_iter()
            .map(TryFrom::try_from)
            .collect::<error_stack::Result<Vec<_>, KernelError>>()
    }
}

pub struct PostgresMetadataProjectionWriter;

impl MetadataProjectionWriter for PostgresMetadataProjectionWriter {
    type Connection = PostgresConnection;

    async fn upsert(
        &self,
        executor: &mut Self::Connection,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO metadatas (id, account_id, label, content, version, nanoid)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (id) DO UPDATE SET
                account_id = EXCLUDED.account_id,
                label = EXCLUDED.label,
                content = EXCLUDED.content,
                version = EXCLUDED.version,
                nanoid = EXCLUDED.nanoid
            WHERE metadatas.version < EXCLUDED.version
            "#,
        )
        .bind(metadata.id().as_ref())
        .bind(metadata.account_id().as_ref())
        .bind(metadata.label().as_ref())
        .bind(metadata.content().as_ref())
        .bind(metadata.version().as_ref())
        .bind(metadata.nanoid().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Connection,
        metadata_id: &MetadataId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"DELETE FROM metadatas WHERE id = $1"#,
        )
        .bind(metadata_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnProfileEventLog for PostgresDatabase {
    type ProfileEventLog = PostgresProfileEventLog;

    fn profile_event_log(&self) -> &Self::ProfileEventLog {
        &PostgresProfileEventLog
    }
}

impl DependOnProfileProjectionWriter for PostgresDatabase {
    type ProfileProjectionWriter = PostgresProfileProjectionWriter;

    fn profile_projection_writer(&self) -> &Self::ProfileProjectionWriter {
        &PostgresProfileProjectionWriter
    }
}

impl DependOnMetadataEventLog for PostgresDatabase {
    type MetadataEventLog = PostgresMetadataEventLog;

    fn metadata_event_log(&self) -> &Self::MetadataEventLog {
        &PostgresMetadataEventLog
    }
}

impl DependOnMetadataProjectionWriter for PostgresDatabase {
    type MetadataProjectionWriter = PostgresMetadataProjectionWriter;

    fn metadata_projection_writer(&self) -> &Self::MetadataProjectionWriter {
        &PostgresMetadataProjectionWriter
    }
}
