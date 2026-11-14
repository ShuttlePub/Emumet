use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    DependOnProfileRemoteActorCacheRepository, ProfileRemoteActorCacheRepository,
    RemoteActorCacheEntry, RemoteActorCacheStatus, RemoteActorProfile,
};
use kernel::prelude::entity::GraphActorUrl;
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(sqlx::FromRow)]
struct CacheRow {
    actor_url: String,
    status: String,
    preferred_username: Option<String>,
    display_name: Option<String>,
    icon_url: Option<String>,
    next_refresh_at: OffsetDateTime,
}

impl TryFrom<CacheRow> for RemoteActorCacheEntry {
    type Error = Report<KernelError>;

    fn try_from(row: CacheRow) -> Result<Self, Self::Error> {
        let status = match row.status.as_str() {
            "pending" => RemoteActorCacheStatus::Pending,
            "resolved" => RemoteActorCacheStatus::Resolved,
            "failed" => RemoteActorCacheStatus::Failed,
            other => {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable(format!("unknown remote actor cache status: {other}")))
            }
        };
        let profile = match (status, row.preferred_username) {
            (RemoteActorCacheStatus::Resolved, Some(username)) => Some(
                RemoteActorProfile::restore(username, row.display_name, row.icon_url),
            ),
            _ => None,
        };
        Ok(RemoteActorCacheEntry {
            actor_url: GraphActorUrl::new(row.actor_url)?,
            status,
            profile,
            next_refresh_at: row.next_refresh_at,
        })
    }
}

pub struct PostgresProfileRemoteActorCacheRepository;

impl ProfileRemoteActorCacheRepository for PostgresProfileRemoteActorCacheRepository {
    type Connection = PostgresConnection;

    async fn find_many(
        &self,
        executor: &mut Self::Connection,
        actor_urls: &[GraphActorUrl],
    ) -> error_stack::Result<Vec<RemoteActorCacheEntry>, KernelError> {
        if actor_urls.is_empty() {
            return Ok(Vec::new());
        }
        let con: &mut PgConnection = executor;
        let urls: Vec<&str> = actor_urls.iter().map(GraphActorUrl::as_str).collect();
        sqlx::query_as::<_, CacheRow>(
            //language=postgresql
            r#"
            SELECT actor_url, status, preferred_username, display_name, icon_url, next_refresh_at
            FROM profile_remote_actor_cache
            WHERE actor_url = ANY($1)
            "#,
        )
        .bind(urls)
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(RemoteActorCacheEntry::try_from)
        .collect()
    }

    async fn try_claim_refresh(
        &self,
        executor: &mut Self::Connection,
        actor_url: &GraphActorUrl,
        now: OffsetDateTime,
        lease_until: OffsetDateTime,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        // One statement: the conflict branch only updates (and so only returns a row) while the
        // current lease / refresh time has passed, so concurrent callers get at most one `true`.
        let claimed = sqlx::query_scalar::<_, String>(
            //language=postgresql
            r#"
            INSERT INTO profile_remote_actor_cache (actor_url, status, next_refresh_at)
            VALUES ($1, 'pending', $3)
            ON CONFLICT (actor_url) DO UPDATE
                SET next_refresh_at = EXCLUDED.next_refresh_at
                WHERE profile_remote_actor_cache.next_refresh_at <= $2
            RETURNING actor_url
            "#,
        )
        .bind(actor_url.as_str())
        .bind(now)
        .bind(lease_until)
        .fetch_optional(con)
        .await
        .convert_error()?;
        Ok(claimed.is_some())
    }

    async fn record_resolved(
        &self,
        executor: &mut Self::Connection,
        actor_url: &GraphActorUrl,
        profile: &RemoteActorProfile,
        now: OffsetDateTime,
        next_refresh_at: OffsetDateTime,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profile_remote_actor_cache (
                actor_url, status, preferred_username, display_name, icon_url,
                fetched_at, next_refresh_at
            )
            VALUES ($1, 'resolved', $2, $3, $4, $5, $6)
            ON CONFLICT (actor_url) DO UPDATE
                SET status = 'resolved',
                    preferred_username = EXCLUDED.preferred_username,
                    display_name = EXCLUDED.display_name,
                    icon_url = EXCLUDED.icon_url,
                    fetched_at = EXCLUDED.fetched_at,
                    next_refresh_at = EXCLUDED.next_refresh_at
            "#,
        )
        .bind(actor_url.as_str())
        .bind(profile.preferred_username())
        .bind(profile.display_name())
        .bind(profile.icon_url())
        .bind(now)
        .bind(next_refresh_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn record_failed(
        &self,
        executor: &mut Self::Connection,
        actor_url: &GraphActorUrl,
        next_refresh_at: OffsetDateTime,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profile_remote_actor_cache (actor_url, status, next_refresh_at)
            VALUES ($1, 'failed', $2)
            ON CONFLICT (actor_url) DO UPDATE
                SET status = CASE
                        WHEN profile_remote_actor_cache.status = 'resolved' THEN 'resolved'
                        ELSE 'failed'
                    END,
                    next_refresh_at = EXCLUDED.next_refresh_at
            "#,
        )
        .bind(actor_url.as_str())
        .bind(next_refresh_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnProfileRemoteActorCacheRepository for PostgresDatabase {
    type ProfileRemoteActorCacheRepository = PostgresProfileRemoteActorCacheRepository;

    fn profile_remote_actor_cache_repository(&self) -> &Self::ProfileRemoteActorCacheRepository {
        &PostgresProfileRemoteActorCacheRepository
    }
}
